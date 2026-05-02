//! H.264 inverse quantization (spec §8.5.12.2).
//!
//! H.264 baseline uses position-dependent quantization with three
//! categories per 4×4 block:
//!   - position (0,0), (0,2), (2,0), (2,2)        → category A
//!   - position (1,1), (1,3), (3,1), (3,3)        → category B
//!   - everything else                             → category C
//!
//! For each QP in 0..=51, three multipliers (`v_a`, `v_b`, `v_c`) and
//! a shared shift come from a small fixed table indexed by `QP % 6`.
//!
//! `inverse_scale(level, qp, position)` returns `level * v * 2^(qp/6)`,
//! the dequantized coefficient before the inverse transform's
//! `>> 6` step.

/// Quantization scale values from spec Table 8-15. Indexed by
/// `(qp % 6, category)` where `category ∈ {A=0, B=1, C=2}`.
pub const NORMALIZE_ADJUST: [[i32; 3]; 6] = [
    [10, 16, 13],
    [11, 18, 14],
    [13, 20, 16],
    [14, 23, 18],
    [16, 25, 20],
    [18, 29, 23],
];

/// Position categories within a 4×4 block, row-major (idx = row*4 + col).
const POSITION_CATEGORY: [u8; 16] = [
    0, 2, 0, 2, // row 0: A C A C
    2, 1, 2, 1, // row 1: C B C B
    0, 2, 0, 2, // row 2: A C A C
    2, 1, 2, 1, // row 3: C B C B
];

/// Inverse-quantize a single 4×4 coefficient block in place.
///
/// `qp` must be in `0..=51`. The (0,0) DC coefficient is not treated
/// specially here — the macroblock-layer caller is responsible for
/// substituting the dequantized DC from the Hadamard pass when the MB
/// is `Intra_16x16` or chroma DC.
pub fn inverse_scale_4x4(block: &mut [i32; 16], qp: u8) {
    debug_assert!(qp <= 51, "qp out of range: {}", qp);
    let qp_mod6 = (qp % 6) as usize;
    let qp_div6 = (qp / 6) as i32;
    for i in 0..16 {
        let cat = POSITION_CATEGORY[i] as usize;
        let v = NORMALIZE_ADJUST[qp_mod6][cat];
        block[i] = block[i].wrapping_mul(v) << qp_div6;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_block_stays_zero() {
        let mut b = [0i32; 16];
        inverse_scale_4x4(&mut b, 26);
        assert_eq!(b, [0i32; 16]);
    }

    #[test]
    fn dc_position_uses_category_a() {
        // QP=0 → mod6=0, div6=0, category-A multiplier = 10.
        let mut b = [0i32; 16];
        b[0] = 1;
        inverse_scale_4x4(&mut b, 0);
        assert_eq!(b[0], 10);
    }

    #[test]
    fn ac_diagonal_uses_category_b() {
        // Position (1,1) is index 5, category B.
        // QP=0 → mod6=0, B multiplier = 16.
        let mut b = [0i32; 16];
        b[5] = 1;
        inverse_scale_4x4(&mut b, 0);
        assert_eq!(b[5], 16);
    }

    #[test]
    fn other_uses_category_c() {
        // Position (0,1) is index 1, category C.
        // QP=0 → mod6=0, C multiplier = 13.
        let mut b = [0i32; 16];
        b[1] = 1;
        inverse_scale_4x4(&mut b, 0);
        assert_eq!(b[1], 13);
    }

    #[test]
    fn qp6_doubles_via_shift() {
        // QP=6 → mod6=0, div6=1: same multiplier as QP=0 but shifted left by 1.
        let mut b = [0i32; 16];
        b[0] = 1;
        inverse_scale_4x4(&mut b, 6);
        assert_eq!(b[0], 10 << 1);
    }

    #[test]
    fn qp51_uses_max_shift() {
        // QP=51 → mod6=3, div6=8.
        // Category A multiplier at mod6=3 is 14.
        let mut b = [0i32; 16];
        b[0] = 1;
        inverse_scale_4x4(&mut b, 51);
        assert_eq!(b[0], 14 << 8);
    }
}
