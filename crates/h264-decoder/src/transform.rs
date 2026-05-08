// H.264 inverse transforms (spec §8.5).
//
// Three transforms total:
//
//  1. `idct_4x4`     — inverse 4×4 integer transform (§8.5.12).
//                      The main residual transform; applied to every
//                      4×4 block of dequantized AC coefficients.
//  2. `hadamard_4x4` — DC Hadamard pass for Intra_16x16 luma DC
//                      (§8.5.10). Decodes the 4×4 grid of DC values
//                      from the 16 luma 4×4 blocks before per-block IDCT.
//  3. `hadamard_2x2` — DC Hadamard pass for chroma DC, 4:2:0 (§8.5.11).
//
// All three operate on `i32` so intermediate sums never overflow on
// the largest legal coefficient values (the spec bounds inputs to
// fit in 16 bits before transform; intermediate products use 32 bits).
// no_std-friendly. No allocation. No floating point.
//
// What's NOT here:
//   - Coefficient scaling / inverse quantization. That's `quant.rs`,
//     which feeds dequantized levels into these functions.
//   - The final `>>6` rounding shift. The spec composes
//     "inverse-quantize → IDCT → +32 → >>6 → clamp(0..255)" as one
//     pipeline; this module returns the raw inverse-transform output
//     so the caller (mb.rs) can interleave whichever scaling step
//     applies to its block type. A `round_shift_6` helper is exposed
//     for callers that just want the canonical AC path.

use crate::DecodeError;

// ─────────────────────────────────────────────────────────────────────
// 4×4 integer IDCT — spec §8.5.12
//
// Two-stage: 1D transform on rows, then 1D transform on columns.
// The 1D inverse is the spec butterfly:
//
//   z0 = d0 + d2
//   z1 = d0 - d2
//   z2 = (d1 >> 1) - d3
//   z3 = d1 + (d3 >> 1)
//
//   f0 = z0 + z3
//   f1 = z1 + z2
//   f2 = z1 - z2
//   f3 = z0 - z3
//
// The final values are integer "transformed residuals" before the
// >>6 scaling shift the spec applies post-transform. See
// `round_shift_6` below for the canonical scaling step.
// ─────────────────────────────────────────────────────────────────────

#[inline]
fn idct_1d(d: [i32; 4]) -> [i32; 4] {
    let z0 = d[0] + d[2];
    let z1 = d[0] - d[2];
    let z2 = (d[1] >> 1) - d[3];
    let z3 = d[1] + (d[3] >> 1);
    [
        z0 + z3,
        z1 + z2,
        z1 - z2,
        z0 - z3,
    ]
}

/// Inverse 4×4 integer transform on a 16-element block in raster
/// (row-major) order. Pure function; output replaces input.
pub fn idct_4x4(block: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    // Pass 1: rows
    for r in 0..4 {
        let row = [block[4*r], block[4*r+1], block[4*r+2], block[4*r+3]];
        let f = idct_1d(row);
        tmp[4*r]   = f[0];
        tmp[4*r+1] = f[1];
        tmp[4*r+2] = f[2];
        tmp[4*r+3] = f[3];
    }
    // Pass 2: columns
    let mut out = [0i32; 16];
    for c in 0..4 {
        let col = [tmp[c], tmp[4+c], tmp[8+c], tmp[12+c]];
        let f = idct_1d(col);
        out[c]    = f[0];
        out[4+c]  = f[1];
        out[8+c]  = f[2];
        out[12+c] = f[3];
    }
    out
}

/// Spec-canonical post-IDCT scaling: add 32 then arithmetic-shift
/// right by 6, equivalent to `(x + 32) / 64` with round-to-nearest
/// (away from zero for negatives, toward positive infinity for
/// half-values — same direction as the spec's `>>` with rounding).
///
/// The +32 makes 32/64 round to 1 instead of 0. Operates in-place.
/// Used by AC residual blocks. `Intra_16x16` luma DC and chroma DC
/// have their own scaling rules (post-Hadamard).
pub fn round_shift_6(block: &mut [i32; 16]) {
    for v in block.iter_mut() {
        *v = (*v + 32) >> 6;
    }
}

// ─────────────────────────────────────────────────────────────────────
// 4×4 Hadamard — Intra_16x16 luma DC pass (§8.5.10)
//
// The 4×4 grid of DC coefficients (one per 4×4 block in a luma MB)
// goes through this transform before per-block IDCT. The 1D pass:
//
//   z0 = d0 + d3
//   z1 = d1 + d2
//   z2 = d1 - d2
//   z3 = d0 - d3
//
//   f0 = z0 + z1
//   f1 = z3 + z2
//   f2 = z0 - z1
//   f3 = z3 - z2
//
// Self-inverse up to a factor of 16 (4×4 Hadamard squared = 16 I).
// ─────────────────────────────────────────────────────────────────────

#[inline]
fn hadamard_1d_4(d: [i32; 4]) -> [i32; 4] {
    let z0 = d[0] + d[3];
    let z1 = d[1] + d[2];
    let z2 = d[1] - d[2];
    let z3 = d[0] - d[3];
    [
        z0 + z1,
        z3 + z2,
        z0 - z1,
        z3 - z2,
    ]
}

/// 4×4 Hadamard on the DC coefficients of an Intra_16x16 macroblock.
/// Input layout: 16 DC values in raster order (one per 4×4 block,
/// blocks numbered left-to-right then top-to-bottom).
pub fn hadamard_4x4(block: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for r in 0..4 {
        let row = [block[4*r], block[4*r+1], block[4*r+2], block[4*r+3]];
        let f = hadamard_1d_4(row);
        tmp[4*r]   = f[0];
        tmp[4*r+1] = f[1];
        tmp[4*r+2] = f[2];
        tmp[4*r+3] = f[3];
    }
    let mut out = [0i32; 16];
    for c in 0..4 {
        let col = [tmp[c], tmp[4+c], tmp[8+c], tmp[12+c]];
        let f = hadamard_1d_4(col);
        out[c]    = f[0];
        out[4+c]  = f[1];
        out[8+c]  = f[2];
        out[12+c] = f[3];
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// 2×2 Hadamard — chroma DC pass for 4:2:0 (§8.5.11)
//
//   H = [[1, 1], [1, -1]]
//   F = H X H^T
//
// Decodes the four chroma DC coefficients of a chroma plane (one per
// 4×4 chroma block) before per-block IDCT. Self-inverse up to a
// factor of 4.
// ─────────────────────────────────────────────────────────────────────

/// 2×2 Hadamard on the four chroma DC coefficients of one chroma
/// plane (4:2:0). Input layout: `[c00, c01, c10, c11]` row-major.
pub fn hadamard_2x2(block: &[i32; 4]) -> [i32; 4] {
    // 1D rows
    let r0 = [block[0] + block[1], block[0] - block[1]];
    let r1 = [block[2] + block[3], block[2] - block[3]];
    // 1D columns
    [
        r0[0] + r1[0],
        r0[1] + r1[1],
        r0[0] - r1[0],
        r0[1] - r1[1],
    ]
}

// ─────────────────────────────────────────────────────────────────────
// Convenience: an aggregate "decode AC residual" that scales a fully
// dequantized 4×4 block to the residual sample values it should add
// to the predicted pixels.
// ─────────────────────────────────────────────────────────────────────

/// `block` is dequantized AC coefficients (caller does the inverse
/// quantization step). Returns the 16 residual samples ready to be
/// added to the predicted block before clamp-to-u8.
///
/// Equivalent to `idct_4x4` then `round_shift_6`.
pub fn idct_4x4_with_scaling(block: &[i32; 16]) -> Result<[i32; 16], DecodeError> {
    let mut residual = idct_4x4(block);
    round_shift_6(&mut residual);
    Ok(residual)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The H.264 4×4 integer FDCT is NOT a clean inverse of the IDCT
    // standalone — the spec splits a per-coefficient normalization
    // matrix between transform and quantization. Encoder pipeline is
    // FDCT → ⊙ Mf → quant; decoder pipeline is dequant → ⊙ Mi → IDCT
    // → +32 → >>6. The round-trip property is "FDCT then quant then
    // dequant then IDCT then >>6 = original", which `quant.rs` will
    // exercise once it lands. These tests verify the IDCT alone
    // against the spec's mathematical definition.

    #[test]
    fn idct_zero_in_zero_out() {
        let zero = [0i32; 16];
        assert_eq!(idct_4x4(&zero), zero);
    }

    #[test]
    fn idct_dc_only_yields_constant_block() {
        // IDCT of {c, 0, ..., 0} per spec §8.5.12 yields a block
        // whose every entry is c (before the >>6 scaling).
        //
        //   1D row 0: [c,0,0,0] → z0=c, z1=c, z2=0, z3=0 → [c,c,c,c]
        //   Rows 1..3: all zeros (input row is zero, butterfly is linear).
        //   1D column 0: [c,0,0,0] → [c,c,c,c]
        //   Columns 1..3: same shape — [c,0,0,0] → [c,c,c,c].
        // Result: every cell = c.
        let c = 13;
        let mut block = [0i32; 16];
        block[0] = c;
        let out = idct_4x4(&block);
        for v in out.iter() {
            assert_eq!(*v, c, "IDCT of DC-only should be constant; got {:?}", out);
        }
    }

    #[test]
    fn idct_1d_butterfly_matches_spec_for_known_input() {
        // Spec §8.5.12 1D butterfly applied to [4, 8, 0, 0]:
        //   z0 = 4 + 0 = 4
        //   z1 = 4 - 0 = 4
        //   z2 = (8 >> 1) - 0 = 4
        //   z3 = 8 + (0 >> 1) = 8
        //   f0 = z0 + z3 = 12
        //   f1 = z1 + z2 = 8
        //   f2 = z1 - z2 = 0
        //   f3 = z0 - z3 = -4
        assert_eq!(idct_1d([4, 8, 0, 0]), [12, 8, 0, -4]);
    }

    #[test]
    fn idct_1d_butterfly_handles_arithmetic_shift_for_negatives() {
        // d = [0, 1, 0, 3]:
        //   z0 = 0+0 = 0; z1 = 0-0 = 0
        //   z2 = (1 >> 1) - 3 = 0 - 3 = -3
        //   z3 = 1 + (3 >> 1) = 1 + 1 = 2
        //   f = [z0+z3, z1+z2, z1-z2, z0-z3] = [2, -3, 3, -2]
        assert_eq!(idct_1d([0, 1, 0, 3]), [2, -3, 3, -2]);
    }

    // Note: the H.264 inverse butterfly is NOT strictly linear because
    // it uses `>> 1` (arithmetic right-shift) on coefficients 1 and 3.
    // `(a+b) >> 1 ≠ (a>>1) + (b>>1)` for inputs of opposite parity.
    // The transform is *self-inverse up to scaling* in a particular
    // basis, but additive linearity isn't a useful invariant here.

    #[test]
    fn idct_uses_arithmetic_shift_on_d1_d3() {
        // d = [0, 3, 0, 0]:
        //   z0 = 0+0 = 0; z1 = 0; z2 = (3>>1) - 0 = 1; z3 = 3+0 = 3
        //   f = [0+3, 0+1, 0-1, 0-3] = [3, 1, -1, -3]
        // Verifies the >>1 truncates toward zero (3/2 = 1, not 2).
        assert_eq!(idct_1d([0, 3, 0, 0]), [3, 1, -1, -3]);
    }

    #[test]
    fn idct_negative_values_round_correctly() {
        // -32 + 32 = 0, then >> 6 = 0 (rounding boundary).
        // -33 + 32 = -1, then >> 6 = -1 (arithmetic shift on i32).
        let mut block = [0i32; 16];
        block[0] = -32;
        round_shift_6(&mut block);
        assert_eq!(block[0], 0);

        let mut block = [0i32; 16];
        block[0] = -33;
        round_shift_6(&mut block);
        assert_eq!(block[0], -1);
    }

    // ─────────────────────────────────────────────────────────────────
    // 4×4 Hadamard tests
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn hadamard_4x4_zero_in_zero_out() {
        let zero = [0i32; 16];
        assert_eq!(hadamard_4x4(&zero), zero);
    }

    #[test]
    fn hadamard_4x4_dc_only_block() {
        // Hadamard of a constant block c×c is concentrated at the DC
        // position with value 16*c (the 4x4 Hadamard is 4*I in 1D, so
        // 2D applied to a constant gives 16*c at DC, zero elsewhere).
        let c = 7;
        let block = [c; 16];
        let out = hadamard_4x4(&block);
        assert_eq!(out[0], 16 * c);
        for i in 1..16 {
            assert_eq!(out[i], 0, "expected zero at index {}; got {}", i, out[i]);
        }
    }

    #[test]
    fn hadamard_4x4_self_inverse_scaled_by_16() {
        // Applying the 4×4 Hadamard twice multiplies by 16.
        let original: [i32; 16] = [
            1, 2, 3, 4,
            5, 6, 7, 8,
            9, 10, 11, 12,
            13, 14, 15, 16,
        ];
        let once = hadamard_4x4(&original);
        let twice = hadamard_4x4(&once);
        for i in 0..16 {
            assert_eq!(twice[i], original[i] * 16, "index {}", i);
        }
    }

    // ─────────────────────────────────────────────────────────────────
    // 2×2 Hadamard tests
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn hadamard_2x2_zero_in_zero_out() {
        assert_eq!(hadamard_2x2(&[0, 0, 0, 0]), [0, 0, 0, 0]);
    }

    #[test]
    fn hadamard_2x2_constant_yields_dc_only() {
        // 2×2 Hadamard of a constant c×c gives 4c at DC, zero elsewhere.
        let c = 11;
        assert_eq!(hadamard_2x2(&[c, c, c, c]), [4 * c, 0, 0, 0]);
    }

    #[test]
    fn hadamard_2x2_self_inverse_scaled_by_4() {
        let original = [3, -1, 5, 7];
        let once = hadamard_2x2(&original);
        let twice = hadamard_2x2(&once);
        for i in 0..4 {
            assert_eq!(twice[i], original[i] * 4, "index {}", i);
        }
    }

    #[test]
    fn idct_4x4_with_scaling_runs_idct_then_round_shift_6() {
        // The wrapper composes the two passes. Verify on the
        // DC-only block: IDCT(DC=64, others=0) gives a 16×64 block;
        // round_shift_6 ((64+32)>>6 = 1) yields 16×1.
        let mut coeffs = [0i32; 16];
        coeffs[0] = 64;
        let r = idct_4x4_with_scaling(&coeffs).unwrap();
        for v in r.iter() {
            assert_eq!(*v, 1);
        }
    }
}
