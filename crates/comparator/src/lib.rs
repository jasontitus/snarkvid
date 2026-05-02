//! Pixel- and PCM-domain similarity primitives.
//!
//! All operations are integer-only and `no_std`. The same code path runs
//! natively (for tests, encoder-side checks, and the host) and inside a
//! zkVM guest where divisions and floating-point are expensive.
//!
//! For PSNR we never compute a logarithm. Whole-dB thresholds are
//! checked via a precomputed integer threshold table — see
//! `psnr_passes_dynamic` and `PSNR_THRESHOLD_X1E9` for the algebra.

#![no_std]

/// Sum of squared errors between two equal-length byte slices.
pub fn sse_u8(a: &[u8], b: &[u8]) -> u64 {
    debug_assert_eq!(a.len(), b.len(), "sse: slice lengths differ");
    let mut acc: u64 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        let d = (*x as i32) - (*y as i32);
        acc += (d * d) as u64;
    }
    acc
}

/// Sum of squared errors between two equal-length i16 slices (PCM).
pub fn sse_i16(a: &[i16], b: &[i16]) -> u64 {
    debug_assert_eq!(a.len(), b.len(), "sse: slice lengths differ");
    let mut acc: u64 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        let d = (*x as i64) - (*y as i64);
        acc += (d * d) as u64;
    }
    acc
}

/// Mean squared error scaled by 1024 to keep integer precision.
pub fn mse_u8_x1024(a: &[u8], b: &[u8]) -> u64 {
    let n = a.len() as u64;
    if n == 0 {
        return 0;
    }
    (sse_u8(a, b) * 1024) / n
}

pub fn mse_i16_x1024(a: &[i16], b: &[i16]) -> u64 {
    let n = a.len() as u64;
    if n == 0 {
        return 0;
    }
    (sse_i16(a, b) * 1024) / n
}

/// Whole-dB PSNR check for 8-bit pixels: passes iff PSNR ≥ floor_db.
pub fn psnr_u8_passes(a: &[u8], b: &[u8], floor_db: u32) -> bool {
    psnr_passes_dynamic(sse_u8(a, b), a.len() as u64, 255, floor_db)
}

/// Whole-dB PSNR check for i16 PCM. `peak` is the absolute peak (32767
/// for full-scale signed audio).
pub fn psnr_i16_passes(a: &[i16], b: &[i16], peak: u32, floor_db: u32) -> bool {
    psnr_passes_dynamic(sse_i16(a, b), a.len() as u64, peak, floor_db)
}

/// Generic PSNR threshold check from precomputed sums.
///
/// Returns `true` iff `10 * log10(peak² * n / sse) ≥ floor_db`, which
/// rearranges to:
///
/// ```text
///   peak² * n  ≥  sse * 10^(floor_db / 10)
/// ```
///
/// Both sides are scaled by `1e9` so the right-hand side can be looked
/// up from `PSNR_THRESHOLD_X1E9` without ever computing a log:
///
/// ```text
///   peak² * n * 1e9  ≥  sse * THRESHOLD[floor_db]
/// ```
///
/// `floor_db` must be in `0..=PSNR_THRESHOLD_X1E9.len()`. Out-of-range
/// floors return `false` (fail closed).
pub fn psnr_passes_dynamic(sse: u64, n: u64, peak: u32, floor_db: u32) -> bool {
    if n == 0 {
        return true;
    }
    if sse == 0 {
        return true;
    }
    let idx = floor_db as usize;
    if idx >= PSNR_THRESHOLD_X1E9.len() {
        return false;
    }
    let lhs = (peak as u128)
        .saturating_mul(peak as u128)
        .saturating_mul(n as u128)
        .saturating_mul(1_000_000_000);
    let rhs = (sse as u128).saturating_mul(PSNR_THRESHOLD_X1E9[idx]);
    lhs >= rhs
}

/// `round(10^(db/10) * 1e9)` for `db` in `0..=80`. Generated with:
///
/// ```python
/// for db in range(81): print(round(10 ** (db / 10) * 1e9))
/// ```
///
/// Covers the full range of v1-relevant PSNR floors with comfortable
/// headroom on both ends. Values fit in u128 (the largest is ~1e17).
pub const PSNR_THRESHOLD_X1E9: [u128; 81] = [
    1_000_000_000,                 // 0
    1_258_925_412,                 // 1
    1_584_893_192,                 // 2
    1_995_262_315,                 // 3
    2_511_886_432,                 // 4
    3_162_277_660,                 // 5
    3_981_071_706,                 // 6
    5_011_872_336,                 // 7
    6_309_573_445,                 // 8
    7_943_282_347,                 // 9
    10_000_000_000,                // 10
    12_589_254_118,                // 11
    15_848_931_925,                // 12
    19_952_623_150,                // 13
    25_118_864_315,                // 14
    31_622_776_602,                // 15
    39_810_717_055,                // 16
    50_118_723_363,                // 17
    63_095_734_448,                // 18
    79_432_823_472,                // 19
    100_000_000_000,               // 20
    125_892_541_179,               // 21
    158_489_319_246,               // 22
    199_526_231_497,               // 23
    251_188_643_151,               // 24
    316_227_766_017,               // 25
    398_107_170_553,               // 26
    501_187_233_627,               // 27
    630_957_344_480,               // 28
    794_328_234_724,               // 29
    1_000_000_000_000,             // 30
    1_258_925_411_794,             // 31
    1_584_893_192_461,             // 32
    1_995_262_314_969,             // 33
    2_511_886_431_510,             // 34
    3_162_277_660_168,             // 35
    3_981_071_705_535,             // 36
    5_011_872_336_273,             // 37
    6_309_573_444_802,             // 38
    7_943_282_347_243,             // 39
    10_000_000_000_000,            // 40
    12_589_254_117_942,            // 41
    15_848_931_924_611,            // 42
    19_952_623_149_689,            // 43
    25_118_864_315_096,            // 44
    31_622_776_601_684,            // 45
    39_810_717_055_350,            // 46
    50_118_723_362_727,            // 47
    63_095_734_448_019,            // 48
    79_432_823_472_428,            // 49
    100_000_000_000_000,           // 50
    125_892_541_179_417,           // 51
    158_489_319_246_111,           // 52
    199_526_231_496_888,           // 53
    251_188_643_150_958,           // 54
    316_227_766_016_838,           // 55
    398_107_170_553_497,           // 56
    501_187_233_627_272,           // 57
    630_957_344_480_193,           // 58
    794_328_234_724_281,           // 59
    1_000_000_000_000_000,         // 60
    1_258_925_411_794_167,         // 61
    1_584_893_192_461_113,         // 62
    1_995_262_314_968_879,         // 63
    2_511_886_431_509_580,         // 64
    3_162_277_660_168_379,         // 65
    3_981_071_705_534_972,         // 66
    5_011_872_336_272_722,         // 67
    6_309_573_444_801_932,         // 68
    7_943_282_347_242_815,         // 69
    10_000_000_000_000_000,        // 70
    12_589_254_117_941_672,        // 71
    15_848_931_924_611_133,        // 72
    19_952_623_149_688_795,        // 73
    25_118_864_315_095_797,        // 74
    31_622_776_601_683_792,        // 75
    39_810_717_055_349_724,        // 76
    50_118_723_362_727_220,        // 77
    63_095_734_448_019_325,        // 78
    79_432_823_472_428_150,        // 79
    100_000_000_000_000_000,       // 80
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_zero_for_identical() {
        let a = [10u8, 20, 30, 40];
        assert_eq!(sse_u8(&a, &a), 0);
        assert!(psnr_u8_passes(&a, &a, 80));
    }

    #[test]
    fn sse_known_value() {
        let a = [0u8, 0, 0, 0];
        let b = [3u8, 4, 0, 0];
        assert_eq!(sse_u8(&a, &b), 9 + 16);
    }

    #[test]
    fn psnr_passes_max_threshold_for_identical() {
        let a = [128u8; 1024];
        assert!(psnr_u8_passes(&a, &a, 80));
    }

    #[test]
    fn psnr_known_value() {
        // Constant 128, perturb every pixel by ±2 (sse=64*4=256, n=64)
        // mse = 4, peak^2/mse = 65025/4, psnr = 10*log10(16256.25) ≈ 42.11 dB
        let a: [u8; 64] = core::array::from_fn(|i| if i % 2 == 0 { 130 } else { 126 });
        let b: [u8; 64] = [128; 64];
        assert!(psnr_u8_passes(&a, &b, 42));
        assert!(!psnr_u8_passes(&a, &b, 43));
    }

    #[test]
    fn psnr_fails_for_max_distortion() {
        let a = [255u8; 16];
        let b = [0u8; 16];
        assert!(!psnr_u8_passes(&a, &b, 1));
    }

    #[test]
    fn out_of_range_floor_fails_closed() {
        let a = [128u8; 16];
        let mut b = [128u8; 16];
        b[0] = 129; // sse > 0 so the early-return doesn't short-circuit.
        // 81 dB is one past the table.
        assert!(!psnr_u8_passes(&a, &b, 81));
    }

    #[test]
    fn mse_x1024_known_value() {
        let a = [0u8, 0, 0, 0];
        let b = [3u8, 4, 0, 0];
        assert_eq!(mse_u8_x1024(&a, &b), 6400);
    }

    #[test]
    fn pcm_psnr_full_scale() {
        let a = [16000i16; 1024];
        let b = [16000i16; 1024];
        assert!(psnr_i16_passes(&a, &b, 32767, 80));
    }

    #[test]
    fn psnr_step_is_correct() {
        // Boundary check: a known-PSNR fixture should pass at floor=N
        // and fail at floor=N+1 for a single carefully-chosen N.
        // sse = 1, n = 16, peak = 255. psnr = 10*log10(255*255*16/1) ≈ 60.1 dB
        assert!(psnr_passes_dynamic(1, 16, 255, 60));
        assert!(!psnr_passes_dynamic(1, 16, 255, 61));
    }
}
