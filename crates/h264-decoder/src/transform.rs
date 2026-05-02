//! H.264 inverse 4×4 integer transform (spec §8.5.10) and the 4×4
//! Hadamard used for Intra_16x16 luma DC and 2×2 chroma DC blocks.
//!
//! All ops are i32 with deterministic integer rounding — same code
//! runs natively and inside a zkVM guest.
//!
//! The transform pair is bit-exact: encoder forward + decoder inverse
//! recover the residual (modulo the rounding that's part of the
//! standard) with no floating-point.

/// Inverse 4×4 core transform (spec §8.5.12).
///
/// Operates in place on a row-major 4×4 i32 block. Output is the
/// reconstructed residual scaled by 64 (the spec includes a final
/// `>>6` step in the macroblock-level reconstruction path; do not
/// repeat it here).
pub fn idct4x4(block: &mut [i32; 16]) {
    // Horizontal pass.
    for r in 0..4 {
        let i0 = block[r * 4];
        let i1 = block[r * 4 + 1];
        let i2 = block[r * 4 + 2];
        let i3 = block[r * 4 + 3];

        let e = i0 + i2;
        let f = i0 - i2;
        let g = (i1 >> 1) - i3;
        let h = i1 + (i3 >> 1);

        block[r * 4] = e + h;
        block[r * 4 + 1] = f + g;
        block[r * 4 + 2] = f - g;
        block[r * 4 + 3] = e - h;
    }
    // Vertical pass.
    for c in 0..4 {
        let i0 = block[c];
        let i1 = block[4 + c];
        let i2 = block[8 + c];
        let i3 = block[12 + c];

        let e = i0 + i2;
        let f = i0 - i2;
        let g = (i1 >> 1) - i3;
        let h = i1 + (i3 >> 1);

        block[c] = e + h;
        block[4 + c] = f + g;
        block[8 + c] = f - g;
        block[12 + c] = e - h;
    }
}

/// 4×4 Hadamard transform used for Intra_16x16 luma DC (spec §8.5.11).
///
/// Symmetric: forward and inverse share the butterfly. Output is the
/// transform of the input scaled by 4 in each pass; callers compensate.
pub fn hadamard4x4(block: &mut [i32; 16]) {
    // Horizontal.
    for r in 0..4 {
        let a = block[r * 4];
        let b = block[r * 4 + 1];
        let c = block[r * 4 + 2];
        let d = block[r * 4 + 3];
        block[r * 4] = a + b + c + d;
        block[r * 4 + 1] = a + b - c - d;
        block[r * 4 + 2] = a - b - c + d;
        block[r * 4 + 3] = a - b + c - d;
    }
    // Vertical.
    for c in 0..4 {
        let a = block[c];
        let b = block[4 + c];
        let cc = block[8 + c];
        let d = block[12 + c];
        block[c] = a + b + cc + d;
        block[4 + c] = a + b - cc - d;
        block[8 + c] = a - b - cc + d;
        block[12 + c] = a - b + cc - d;
    }
}

/// 2×2 Hadamard for chroma DC (spec §8.5.11.2).
pub fn hadamard2x2(block: &mut [i32; 4]) {
    let a = block[0];
    let b = block[1];
    let c = block[2];
    let d = block[3];
    block[0] = a + b + c + d;
    block[1] = a - b + c - d;
    block[2] = a + b - c - d;
    block[3] = a - b - c + d;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// idct4x4 of a transform-domain DC = `16N` block must produce the
    /// spatial-domain constant `16N`. (The eventual `>>6` shift in the
    /// macroblock reconstruction path divides by 64 — the remaining
    /// factor of 4 comes from the per-coefficient dequantization
    /// scale, so end-to-end the spatial output is `N`.)
    #[test]
    fn idct_pure_dc_gives_constant() {
        let mut block = [0i32; 16];
        block[0] = 16 * 8;
        idct4x4(&mut block);
        for (i, v) in block.iter().enumerate() {
            assert_eq!(*v, 16 * 8, "index {i}");
        }
    }

    /// Symmetry: idct of a single AC coefficient should leave row 0's
    /// energy in row 0, column 0's energy in column 0, and have zero
    /// mean. Spot-check on the (0,1) coefficient.
    #[test]
    fn idct_ac_01_zero_mean() {
        let mut block = [0i32; 16];
        block[1] = 64;
        idct4x4(&mut block);
        let sum: i32 = block.iter().sum();
        assert_eq!(sum, 0, "AC-only inverse should be zero-mean");
    }

    #[test]
    fn idct_zero_residual_stays_zero() {
        let mut block = [0i32; 16];
        idct4x4(&mut block);
        assert_eq!(block, [0i32; 16]);
    }

    #[test]
    fn hadamard4x4_self_inverse_to_scale() {
        let original = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16i32,
        ];
        let mut block = original;
        hadamard4x4(&mut block);
        hadamard4x4(&mut block);
        // Two passes scale by 16 (4 per dimension).
        for (i, v) in block.iter().enumerate() {
            assert_eq!(*v, original[i] * 16, "index {i}");
        }
    }

    #[test]
    fn hadamard2x2_self_inverse_to_scale() {
        let original = [3, 7, 11, 13i32];
        let mut block = original;
        hadamard2x2(&mut block);
        hadamard2x2(&mut block);
        // 2×2 forward+inverse scales by 4.
        for (i, v) in block.iter().enumerate() {
            assert_eq!(*v, original[i] * 4, "index {i}");
        }
    }
}
