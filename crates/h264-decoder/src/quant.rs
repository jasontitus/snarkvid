// H.264 inverse quantization for 4×4 blocks (spec §8.5.9 / Table 7-3).
//
// CAVLC entropy decode produces "levels" — small signed integers,
// one per block coefficient, that the encoder quantized at some QP.
// Inverse quantization undoes that step, producing the dequantized
// coefficients that feed into the IDCT.
//
// Only the **AC residual** path is implemented here:
//
//   inverse_quant_4x4_ac(level, qp) → coeff   (spec §8.5.9 / §8.5.12.2)
//
// Used for:
//   - Luma 4×4 residual blocks when mb_type != Intra_16x16
//   - Chroma 4×4 AC residual blocks
//
// Out of scope this session (next session, alongside the rest of
// CAVLC and the Intra_16x16 path):
//   - inverse_quant_luma_dc_intra16x16  (post-4×4-Hadamard luma DC)
//   - inverse_quant_chroma_dc_4x4       (post-2×2-Hadamard chroma DC)
//
// Both Hadamard variants share the same NORM_ADJUST_4X4 table; the
// only difference is the shift constants. Adding them is mechanical
// once the AC path is anchored.
//
// no_std-pure. No allocation. No floating point.

use crate::DecodeError;

// ─────────────────────────────────────────────────────────────────────
// Spec Table 7-3: normAdjust4×4(m, v).
//
//   row m    = qP % 6              (0..=5)
//   column v = position class      (0..=2):
//     v=0 if (i, j) both even      → "DC-like" positions (0,0), (0,2),
//                                                       (2,0), (2,2)
//     v=1 if (i, j) both odd       → (1,1), (1,3), (3,1), (3,3)
//     v=2 otherwise                → the other 8 positions
//
// LevelScale4×4(m, i, j) = normAdjust4×4(m, v(i,j)) * weightScale4×4(i,j).
// For baseline profile with no per-PPS scaling list the weightScale is
// 16 everywhere (spec §7.4.2.2). This is the universal default and the
// only configuration M3 supports.
// ─────────────────────────────────────────────────────────────────────

const NORM_ADJUST_4X4: [[i32; 3]; 6] = [
    // v=0   v=1   v=2
    [ 10,    16,   13 ],   // m=0
    [ 11,    18,   14 ],   // m=1
    [ 13,    20,   16 ],   // m=2
    [ 14,    23,   18 ],   // m=3
    [ 16,    25,   20 ],   // m=4
    [ 18,    29,   23 ],   // m=5
];

const DEFAULT_WEIGHT_SCALE_4X4: i32 = 16;

#[inline]
fn position_class_4x4(i: usize, j: usize) -> usize {
    let i_even = i % 2 == 0;
    let j_even = j % 2 == 0;
    if i_even && j_even { 0 }
    else if !i_even && !j_even { 1 }
    else { 2 }
}

/// LevelScale4×4 from spec §8.5.9. With the default scaling list:
///
///   level_scale_4x4(m, i, j) = NORM_ADJUST_4X4[m][v(i,j)] * 16
#[inline]
fn level_scale_4x4(m: u32, i: usize, j: usize) -> i32 {
    NORM_ADJUST_4X4[m as usize][position_class_4x4(i, j)] * DEFAULT_WEIGHT_SCALE_4X4
}

/// Inverse-quantize a 4×4 AC residual block.
///
/// `level` is 16 i16 levels in raster order (j*4 + i). `qp` is the
/// luma or chroma QP in 0..=51 (8-bit profile). Returns 16 i32
/// dequantized coefficients to feed into `transform::idct_4x4`.
///
/// Per spec §8.5.12.2:
///   if qP >= 24:
///     c'[i,j] = level[i,j] * LevelScale4×4(qP%6, i, j) << (qP/6 - 4)
///   else:
///     c'[i,j] = (level[i,j] * LevelScale4×4(qP%6, i, j) + (1 << (3 - qP/6)))
///                >> (4 - qP/6)
///
/// The split at qP=24 keeps the dequantized values bounded across
/// the QP range — at low QP we shift right (with rounding), at high
/// QP we shift left.
pub fn inverse_quant_4x4_ac(
    level: &[i16; 16],
    qp: u32,
) -> Result<[i32; 16], DecodeError> {
    if qp > 51 {
        return Err(DecodeError::OutOfScope("qp > 51 (8-bit baseline only)"));
    }
    let m = qp % 6;
    let qp_div_6 = qp / 6;
    let mut out = [0i32; 16];
    for j in 0..4 {
        for i in 0..4 {
            let idx = j * 4 + i;
            let lvl = level[idx] as i32;
            let scale = level_scale_4x4(m, i, j);
            let prod = lvl * scale;
            out[idx] = if qp_div_6 >= 4 {
                // qP ≥ 24: multiplicative path. qp_div_6 - 4 ∈ 0..=4.
                prod << (qp_div_6 - 4)
            } else {
                // qP < 24: divisive path. qp_div_6 ∈ 0..=3.
                let shift = 4 - qp_div_6;
                let round = 1 << (3 - qp_div_6);
                (prod + round) >> shift
            };
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_classes_split_correctly() {
        // v=0: (0,0), (0,2), (2,0), (2,2)
        for &(i, j) in &[(0, 0), (0, 2), (2, 0), (2, 2)] {
            assert_eq!(position_class_4x4(i, j), 0, "({}, {})", i, j);
        }
        // v=1: (1,1), (1,3), (3,1), (3,3)
        for &(i, j) in &[(1, 1), (1, 3), (3, 1), (3, 3)] {
            assert_eq!(position_class_4x4(i, j), 1, "({}, {})", i, j);
        }
        // v=2: every other pairing — exactly 8 of 16 cells
        let mut count_v2 = 0;
        for i in 0..4 {
            for j in 0..4 {
                if position_class_4x4(i, j) == 2 {
                    count_v2 += 1;
                }
            }
        }
        assert_eq!(count_v2, 8);
    }

    #[test]
    fn zero_levels_yield_zero_coefficients() {
        let zero = [0i16; 16];
        for qp in 0..=51 {
            assert_eq!(inverse_quant_4x4_ac(&zero, qp).unwrap(), [0i32; 16]);
        }
    }

    #[test]
    fn dc_only_level_qp_0_known_value() {
        // qp=0: m=0, qp_div_6=0 → divisive path with shift=4, round=8.
        // At position (0,0): scale = NORM_ADJUST_4X4[0][0] * 16 = 10*16 = 160.
        // level=10 → c'[0,0] = (10*160 + 8) >> 4 = 1608 >> 4 = 100.
        let mut level = [0i16; 16];
        level[0] = 10;
        let out = inverse_quant_4x4_ac(&level, 0).unwrap();
        assert_eq!(out[0], 100);
        for i in 1..16 {
            assert_eq!(out[i], 0, "position {} should be 0", i);
        }
    }

    #[test]
    fn dc_only_level_qp_24_known_value() {
        // qp=24: m=0, qp_div_6=4 → multiplicative path, shift left by 0.
        // At (0,0): scale = 160.
        // level=10 → c'[0,0] = 10 * 160 << 0 = 1600.
        let mut level = [0i16; 16];
        level[0] = 10;
        let out = inverse_quant_4x4_ac(&level, 24).unwrap();
        assert_eq!(out[0], 1600);
    }

    #[test]
    fn dc_only_level_qp_30_known_value() {
        // qp=30: m=0, qp_div_6=5 → multiplicative, shift left by 1.
        // scale = 160. level=3 → c'[0,0] = 3 * 160 << 1 = 960.
        let mut level = [0i16; 16];
        level[0] = 3;
        let out = inverse_quant_4x4_ac(&level, 30).unwrap();
        assert_eq!(out[0], 960);
    }

    #[test]
    fn dc_only_level_qp_18_known_value() {
        // qp=18: m=0, qp_div_6=3 → divisive, shift=1, round=1.
        // At (0,0): scale = 160. level=10 → (10*160 + 1) >> 1 = 800.
        let mut level = [0i16; 16];
        level[0] = 10;
        let out = inverse_quant_4x4_ac(&level, 18).unwrap();
        assert_eq!(out[0], 800);
    }

    #[test]
    fn position_classes_use_distinct_norm_adjust_columns() {
        // qp=6 → m=0, qp_div_6=1 → divisive shift=3, round=4.
        // norm_adjust row 0: [10, 16, 13]
        // scale at (0,0) v=0: 10*16 = 160; level=64 → (64*160 + 4) >> 3 = 1280
        // scale at (1,1) v=1: 16*16 = 256; level=64 → (64*256 + 4) >> 3 = 2048
        // scale at (1,0) v=2: 13*16 = 208; level=64 → (64*208 + 4) >> 3 = 1664
        let mut level = [0i16; 16];
        level[0]  = 64;            // (i=0,j=0) → v=0
        level[5]  = 64;            // (i=1,j=1) → v=1
        level[1]  = 64;            // (i=1,j=0) → v=2
        let out = inverse_quant_4x4_ac(&level, 6).unwrap();
        assert_eq!(out[0], 1280);
        assert_eq!(out[5], 2048);
        assert_eq!(out[1], 1664);
    }

    #[test]
    fn negative_levels_round_with_arithmetic_shift() {
        // qp=0 divisive path: (level*scale + 8) >> 4. With level=-1 and
        // scale=160: (-160 + 8) >> 4 = -152 >> 4 = -10 (arithmetic shift
        // floors toward -∞).
        let mut level = [0i16; 16];
        level[0] = -1;
        let out = inverse_quant_4x4_ac(&level, 0).unwrap();
        assert_eq!(out[0], -10);
    }

    #[test]
    fn rejects_qp_above_51() {
        let zero = [0i16; 16];
        assert!(matches!(
            inverse_quant_4x4_ac(&zero, 52),
            Err(DecodeError::OutOfScope(_))
        ));
    }

    #[test]
    fn idct_after_dequant_recovers_residual_after_round_shift() {
        // The round-trip the transform module deferred until quant
        // landed: encode-side fdct → forward quant → CAVLC (skipped here:
        // the levels go straight into dequant) → IDCT → round_shift_6.
        //
        // Property exercised: dequant gives values whose IDCT after
        // round_shift_6 closely matches the original 4x4 residual block.
        // Forward-quant uses the spec's encoder-side mf table.
        //
        // We verify on a low-energy block at low qP, where round-trip
        // error stays at 0 or ±1.
        use crate::transform::{idct_4x4, round_shift_6};

        // mf4x4(m, v): forward quant multiplier (spec Table 7-2). Inverse of
        // norm_adjust * 16 within rounding.
        const MF_4X4: [[i32; 3]; 6] = [
            [13107, 5243, 8066],
            [11916, 4660, 7490],
            [10082, 4194, 6554],
            [9362,  3647, 5825],
            [8192,  3355, 5243],
            [7282,  2893, 4559],
        ];

        fn fdct_1d(d: [i32; 4]) -> [i32; 4] {
            [
                d[0] + d[1] + d[2] + d[3],
                2*d[0] + d[1] - d[2] - 2*d[3],
                d[0] - d[1] - d[2] + d[3],
                d[0] - 2*d[1] + 2*d[2] - d[3],
            ]
        }
        fn fdct_4x4(block: &[i32; 16]) -> [i32; 16] {
            let mut tmp = [0i32; 16];
            for r in 0..4 {
                let row = [block[4*r], block[4*r+1], block[4*r+2], block[4*r+3]];
                let f = fdct_1d(row);
                tmp[4*r..4*r+4].copy_from_slice(&f);
            }
            let mut out = [0i32; 16];
            for c in 0..4 {
                let col = [tmp[c], tmp[4+c], tmp[8+c], tmp[12+c]];
                let f = fdct_1d(col);
                out[c]    = f[0];
                out[4+c]  = f[1];
                out[8+c]  = f[2];
                out[12+c] = f[3];
            }
            out
        }
        fn position_class(i: usize, j: usize) -> usize {
            if i % 2 == 0 && j % 2 == 0 { 0 }
            else if i % 2 == 1 && j % 2 == 1 { 1 }
            else { 2 }
        }
        fn forward_quant_4x4(coeffs: &[i32; 16], qp: u32) -> [i16; 16] {
            // Spec §8.5.10 forward quant for I_PCM-equivalent path,
            // simplified for default scaling list. f = 1 << (15 + qP/6) / 3
            // is the rounding bias; we use the approximation that's
            // standard for I-frames. (Encoders use a smaller bias for
            // P/B blocks; for our test, large bias → conservative
            // rounding.)
            let m = qp % 6;
            let qp_shift = qp / 6;
            let q_bits = 15 + qp_shift;
            let f = 1i32 << (q_bits - 1);  // rounding to nearest
            let mut out = [0i16; 16];
            for j in 0..4 {
                for i in 0..4 {
                    let idx = j*4 + i;
                    let abs_c = coeffs[idx].unsigned_abs() as i32;
                    let mf = MF_4X4[m as usize][position_class(i, j)];
                    let q_abs = ((abs_c * mf) + f) >> q_bits;
                    let signed = if coeffs[idx] >= 0 { q_abs } else { -q_abs };
                    out[idx] = signed as i16;
                }
            }
            out
        }

        // Tiny block: small DC coefficient + zero AC. Exact recovery
        // expected at qp=0 (lossless except for rounding).
        let original: [i32; 16] = [
            8, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0,
        ];
        let coeffs = fdct_4x4(&original);
        // FDCT of [8,0,...,0]: each 1D pass on a row of zeros = zeros;
        // first row is [8, 16, 8, 8] (per fdct_1d formula on [8,0,0,0]:
        // [8, 16, 8, 8])... wait let me recompute.
        // fdct_1d([8,0,0,0]) = [8+0+0+0, 16+0-0-0, 8-0-0+0, 8-0+0-0]
        //                    = [8, 16, 8, 8]
        // Then column pass on tmp[c] for c=0: tmp[c=0] across rows is
        // [8, 0, 0, 0], same fdct_1d → [8, 16, 8, 8]. So out[0]=8.
        // For c=1: row[c=1] is [16, 0, 0, 0] → [16, 32, 16, 16]. Out[1]=16.
        // etc.
        // So coeffs[0] = 8.
        let qp = 0;
        let levels = forward_quant_4x4(&coeffs, qp);
        let dequant = inverse_quant_4x4_ac(&levels, qp).unwrap();
        let mut residual = idct_4x4(&dequant);
        round_shift_6(&mut residual);
        // At qp=0 the round-trip on this tiny DC-only block should
        // recover the original to within ±1.
        for i in 0..16 {
            let diff = (residual[i] - original[i]).abs();
            assert!(diff <= 1,
                "round-trip diff > 1 at idx {}: got {} expected {} (residual={:?})",
                i, residual[i], original[i], residual);
        }
    }
}
