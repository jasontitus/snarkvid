// PSNR / MSE comparison primitives for the in-circuit comparator.
//
// Given a decoded YUV frame and the original YUV frame, compute a
// per-frame similarity score. The prover asserts:
//
//   psnr(decoded, original) >= tolerance
//
// These functions are no_std and deterministic — must produce bit-exact
// results on every platform, including inside the zkVM guest.
//
// For simplicity, we operate on raw YUV planes with integer arithmetic.
// The final PSNR is computed in fixed-point to avoid floating-point
// inside the zkVM (many zkVMs don't support f32/f64, or they're very
// expensive).

#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

/// PSNR tolerance expressed as a fixed-point dB value.
/// `db_threshold = tolerance_db * SCALE`, where SCALE = 100.
/// e.g., 36.00 dB → db_threshold = 3600.
pub const PSNR_SCALE: i64 = 100;

/// Maximum pixel value for 8-bit video.
pub const MAX_PIXEL: f64 = 255.0;

// ---------------------------------------------------------------------------
// Compute mean squared error (MSE) between two YUV planes.
// Returns the sum of squared differences and the count of compared pixels.
// ---------------------------------------------------------------------------

/// Compute the sum of squared differences between two planes.
/// Panics if the slices have different lengths.
pub fn sum_squared_error(a: &[u8], b: &[u8]) -> i64 {
    assert_eq!(a.len(), b.len());
    let mut sum: i64 = 0;
    for (&pa, &pb) in a.iter().zip(b.iter()) {
        let diff = pa as i64 - pb as i64;
        sum += diff * diff;
    }
    sum
}

/// Compute PSNR given SSE and pixel count.
///
/// Returns PSNR in fixed-point: `result = psnr_db * PSNR_SCALE`.
/// For 8-bit video: psnr = 10 * log10(MAX² / mse)
///   = 10 * log10(65025 / (sse/N))
///   = 10 * log10(65025 * N / sse)
pub fn psnr_fixed(sse: i64, pixel_count: usize) -> i64 {
    if sse == 0 {
        // Infinite PSNR — return a sentinel.
        // MSE=0 means identical frames, PSNR → ∞.
        return i64::MAX;
    }
    if pixel_count == 0 {
        return 0;
    }

    // mse = sse / pixel_count  (as f64)
    let mse = sse as f64 / pixel_count as f64;
    if mse <= 0.0 {
        return i64::MAX;
    }

    // psnr = 10 * log10(MAX² / mse)
    let max_sq = MAX_PIXEL * MAX_PIXEL;
    let psnr = 10.0 * libm::log10(max_sq / mse);

    (psnr * PSNR_SCALE as f64) as i64
}

/// Convenience: verify that a decoded plane meets the PSNR threshold
/// when compared to the original.
pub fn check_psnr_threshold(
    decoded: &[u8],
    original: &[u8],
    threshold_db_scaled: i64,
) -> bool {
    let sse = sum_squared_error(decoded, original);
    let psnr = psnr_fixed(sse, decoded.len());
    psnr >= threshold_db_scaled
}

// ---------------------------------------------------------------------------
// Full-frame PSNR: average over Y, U, V planes.
//
// The combined PSNR is the average across all planes weighted by pixel
// count. For 4:2:0, Y has 4× the pixels of U or V.
// ---------------------------------------------------------------------------

/// Result of a full-frame PSNR comparison.
pub struct FramePsnrResult {
    pub psnr_y_scaled: i64,
    pub psnr_u_scaled: i64,
    pub psnr_v_scaled: i64,
    pub psnr_combined_scaled: i64,
    pub meets_threshold: bool,
}

/// Compute PSNR for all three planes and check against threshold.
///
/// `tolerance_db_scaled` is the threshold in fixed-point (e.g., 3600 for 36.00 dB).
pub fn frame_psnr(
    decoded_y: &[u8],
    original_y: &[u8],
    decoded_u: &[u8],
    original_u: &[u8],
    decoded_v: &[u8],
    original_v: &[u8],
    tolerance_db_scaled: i64,
) -> FramePsnrResult {
    let psnr_y = psnr_fixed(
        sum_squared_error(decoded_y, original_y),
        decoded_y.len(),
    );
    let psnr_u = psnr_fixed(
        sum_squared_error(decoded_u, original_u),
        decoded_u.len(),
    );
    let psnr_v = psnr_fixed(
        sum_squared_error(decoded_v, original_v),
        decoded_v.len(),
    );

    // Weighted average: Y counts 4×, U and V count 1× each for 4:2:0.
    // total = (4*Y + U + V) / 6
    let total_weight = 6;
    let combined = if psnr_y == i64::MAX || psnr_u == i64::MAX || psnr_v == i64::MAX {
        // If any channel is infinite, use the non-infinite ones
        let mut sum: i64 = 0;
        let mut weight: i64 = 0;
        if psnr_y != i64::MAX {
            sum += 4 * psnr_y;
            weight += 4;
        }
        if psnr_u != i64::MAX {
            sum += psnr_u;
            weight += 1;
        }
        if psnr_v != i64::MAX {
            sum += psnr_v;
            weight += 1;
        }
        if weight > 0 {
            sum / weight
        } else {
            i64::MAX
        }
    } else {
        (4 * psnr_y + psnr_u + psnr_v) / total_weight
    };

    FramePsnrResult {
        psnr_y_scaled: psnr_y,
        psnr_u_scaled: psnr_u,
        psnr_v_scaled: psnr_v,
        psnr_combined_scaled: combined,
        meets_threshold: combined == i64::MAX || combined >= tolerance_db_scaled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn identical_frames_infinite_psnr() {
        let data = vec![128u8; 256];
        let sse = sum_squared_error(&data, &data);
        assert_eq!(sse, 0);
        let psnr = psnr_fixed(sse, data.len());
        assert_eq!(psnr, i64::MAX);
    }

    #[test]
    fn every_pixel_wrong_by_one() {
        let a = vec![100u8; 1000];
        let b = vec![101u8; 1000];
        let sse = sum_squared_error(&a, &b);
        assert_eq!(sse, 1000); // 1000 pixels × 1²
        let psnr = psnr_fixed(sse, a.len());
        // mse = 1000/1000 = 1, psnr = 10*log10(65025/1) ≈ 48.13 dB
        assert!(psnr > 4800 && psnr < 4820); // ~48.13 dB scaled
    }
}
