// BlockQuant toy codec — milestone 2's deliberately simple image codec.
//
// Goal (see milestones/02-toy-transform.md §2): the dumbest possible
// codec that still exercises real integer arithmetic over real image
// data, without entropy coding or motion compensation. Lets us shake
// out the full architecture (manifest → Merkle → in-circuit decoder →
// comparator → browser verifier) before swapping in H.264 in M3.
//
// Pipeline:
//   1. Partition each YUV plane into 8×8 blocks (Y full-res, U/V 4:2:0).
//      Frame dims must be multiples of 16 so chroma blocks land cleanly.
//   2. Forward 2D Walsh–Hadamard 8×8 over centered pixels (pixel - 128).
//      Unnormalized: Y = H · X · H where H is the ±1 8×8 Hadamard matrix.
//      Coefficients fit in i16 (DC magnitude ≤ 8·8·128 = 8192).
//   3. Uniform quantization: q = round(Y / step) with step = max(1, qp).
//      qp=0 → step=1 → bit-exact round trip (H X H is divisible by 64).
//      qp=8 → step=8 → ~54 dB PSNR (well above M2's 40 dB floor).
//   4. Bitstream: header (6 bytes) + i16 coefficients in block-raster
//      order, Y plane then U then V.
//   5. Decode is the reverse: dequantize → inverse 2D WHT → de-center
//      (+128) → clamp to [0,255].
//
// Both encode and decode are deterministic, panic-free, and produce
// bit-exact output for a given input regardless of platform. The
// crate is no_std so the same code runs natively (toy-encode CLI,
// tests, host) and inside the zkVM guest. Same discipline we'll need
// for the H.264 decoder in M3.

#![no_std]

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

#[cfg(feature = "std")]
extern crate std;

/// A single YUV 4:2:0 frame.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct YuvFrame {
    pub width: u16,
    pub height: u16,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// BlockQuant bitstream header.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BqHeader {
    pub width: u16,
    pub height: u16,
    pub qp: u8,
    pub chroma_format: u8, // always 1 (4:2:0) for milestone 2
}

/// Compressed representation: header + coefficient stream.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BqBitstream {
    pub header: BqHeader,
    pub coeffs_y: Vec<i16>,
    pub coeffs_u: Vec<i16>,
    pub coeffs_v: Vec<i16>,
}

/// Errors that can occur during encoding or decoding.
#[derive(Clone, Debug, PartialEq)]
pub enum ToyCodecError {
    InvalidDimensions,
    InvalidChromaFormat,
    BufferTooSmall,
    CoefficientOverflow,
    Unsupported,
}

#[cfg(feature = "std")]
impl std::error::Error for ToyCodecError {}

#[cfg(feature = "std")]
impl core::fmt::Display for ToyCodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidDimensions => write!(f, "frame dimensions must be multiples of 16"),
            Self::InvalidChromaFormat => write!(f, "only 4:2:0 chroma format is supported"),
            Self::BufferTooSmall => write!(f, "coefficient buffer size doesn't match frame dimensions"),
            Self::CoefficientOverflow => write!(f, "coefficient value out of range"),
            Self::Unsupported => write!(f, "unsupported codec parameter"),
        }
    }
}

const BLOCK: usize = 8;
const BLOCK2: usize = BLOCK * BLOCK;

/// Quantization step for a given QP. step=1 at qp=0 keeps the round
/// trip bit-exact (H X H is divisible by 64 so the inverse rounds to
/// the original integer). Larger steps increase reconstruction error
/// linearly — at step=8, RMS error is ≈0.5 LSB (~54 dB PSNR).
fn quant_step(qp: u8) -> i32 {
    if qp == 0 {
        1
    } else {
        qp as i32
    }
}

/// 8-point unnormalized Walsh–Hadamard transform (in place, length 8).
/// H X (where H is the symmetric ±1 8×8 Hadamard matrix). Self-inverse
/// up to a factor of 8: applying it twice scales by 8.
fn wht8(v: &mut [i32; BLOCK]) {
    // Stage 1: pair sums/differences
    let a0 = v[0] + v[1]; let a1 = v[0] - v[1];
    let a2 = v[2] + v[3]; let a3 = v[2] - v[3];
    let a4 = v[4] + v[5]; let a5 = v[4] - v[5];
    let a6 = v[6] + v[7]; let a7 = v[6] - v[7];
    // Stage 2: 4-point butterfly
    let b0 = a0 + a2; let b2 = a0 - a2;
    let b1 = a1 + a3; let b3 = a1 - a3;
    let b4 = a4 + a6; let b6 = a4 - a6;
    let b5 = a5 + a7; let b7 = a5 - a7;
    // Stage 3: 8-point butterfly
    v[0] = b0 + b4;
    v[1] = b1 + b5;
    v[2] = b2 + b6;
    v[3] = b3 + b7;
    v[4] = b0 - b4;
    v[5] = b1 - b5;
    v[6] = b2 - b6;
    v[7] = b3 - b7;
}

/// 2D 8×8 WHT: apply 1D WHT along rows then columns.
/// Identical for forward and inverse (only the post-scale differs).
fn wht8x8(block: &mut [i32; BLOCK2]) {
    // Rows
    for r in 0..BLOCK {
        let mut row: [i32; BLOCK] = [0; BLOCK];
        for c in 0..BLOCK {
            row[c] = block[r * BLOCK + c];
        }
        wht8(&mut row);
        for c in 0..BLOCK {
            block[r * BLOCK + c] = row[c];
        }
    }
    // Columns
    for c in 0..BLOCK {
        let mut col: [i32; BLOCK] = [0; BLOCK];
        for r in 0..BLOCK {
            col[r] = block[r * BLOCK + c];
        }
        wht8(&mut col);
        for r in 0..BLOCK {
            block[r * BLOCK + c] = col[r];
        }
    }
}

/// Round-half-away-from-zero division by `d > 0`.
fn div_round(n: i32, d: i32) -> i32 {
    if n >= 0 {
        (n + d / 2) / d
    } else {
        -((-n + d / 2) / d)
    }
}

/// Forward-transform + quantize one 8×8 block of pixels into 64 i16
/// coefficients. `pixels` are u8; centered to [-128, 127] before WHT.
fn encode_block(pixels: &[u8; BLOCK2], step: i32) -> [i16; BLOCK2] {
    let mut x: [i32; BLOCK2] = [0; BLOCK2];
    for i in 0..BLOCK2 {
        x[i] = pixels[i] as i32 - 128;
    }
    wht8x8(&mut x);
    let mut out: [i16; BLOCK2] = [0; BLOCK2];
    for i in 0..BLOCK2 {
        let q = div_round(x[i], step);
        // Clip to i16 range. Worst case unquantized magnitude is 8192,
        // step ≥ 1 → ≤ 8192; well within ±32767.
        out[i] = q.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
    out
}

/// Dequantize + inverse-transform one block. Mirrors encode_block.
fn decode_block(coeffs: &[i16; BLOCK2], step: i32) -> [u8; BLOCK2] {
    let mut y: [i32; BLOCK2] = [0; BLOCK2];
    for i in 0..BLOCK2 {
        y[i] = coeffs[i] as i32 * step;
    }
    wht8x8(&mut y);
    // 2D WHT applied twice = ×64. Divide by 64 with rounding, then
    // de-center and clamp.
    let mut out: [u8; BLOCK2] = [0; BLOCK2];
    for i in 0..BLOCK2 {
        let pixel = div_round(y[i], 64) + 128;
        out[i] = pixel.clamp(0, 255) as u8;
    }
    out
}

/// Encode one whole plane (block-raster order: outer over block rows,
/// inner over blocks, raster within block). `width` and `height` must
/// already be multiples of 8.
fn encode_plane(pixels: &[u8], width: usize, height: usize, step: i32) -> Vec<i16> {
    let mut out = vec![0i16; width * height];
    let bw = width / BLOCK;
    let bh = height / BLOCK;
    let mut block_pixels: [u8; BLOCK2] = [0; BLOCK2];
    let mut idx = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            for r in 0..BLOCK {
                for c in 0..BLOCK {
                    block_pixels[r * BLOCK + c] = pixels[(by * BLOCK + r) * width + bx * BLOCK + c];
                }
            }
            let coeffs = encode_block(&block_pixels, step);
            out[idx..idx + BLOCK2].copy_from_slice(&coeffs);
            idx += BLOCK2;
        }
    }
    out
}

/// Decode one whole plane. Inverse of encode_plane.
fn decode_plane(coeffs: &[i16], width: usize, height: usize, step: i32) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    let bw = width / BLOCK;
    let bh = height / BLOCK;
    let mut block_coeffs: [i16; BLOCK2] = [0; BLOCK2];
    let mut idx = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            block_coeffs.copy_from_slice(&coeffs[idx..idx + BLOCK2]);
            idx += BLOCK2;
            let pixels = decode_block(&block_coeffs, step);
            for r in 0..BLOCK {
                for c in 0..BLOCK {
                    out[(by * BLOCK + r) * width + bx * BLOCK + c] = pixels[r * BLOCK + c];
                }
            }
        }
    }
    out
}

/// Encode a YUV frame into a BlockQuant bitstream.
///
/// QP: 0..=51. 0 = lossless round trip. 8 = ≳40 dB PSNR target. 51 =
/// heavy quantization.
pub fn encode_toy(frame: &YuvFrame, qp: u8) -> Result<BqBitstream, ToyCodecError> {
    if frame.width == 0 || frame.height == 0 || frame.width % 16 != 0 || frame.height % 16 != 0 {
        return Err(ToyCodecError::InvalidDimensions);
    }
    if qp > 51 {
        return Err(ToyCodecError::Unsupported);
    }
    let w = frame.width as usize;
    let h = frame.height as usize;
    let cw = w / 2;
    let ch = h / 2;
    if frame.y.len() != w * h || frame.u.len() != cw * ch || frame.v.len() != cw * ch {
        return Err(ToyCodecError::BufferTooSmall);
    }

    let step = quant_step(qp);
    let coeffs_y = encode_plane(&frame.y, w, h, step);
    let coeffs_u = encode_plane(&frame.u, cw, ch, step);
    let coeffs_v = encode_plane(&frame.v, cw, ch, step);

    Ok(BqBitstream {
        header: BqHeader {
            width: frame.width,
            height: frame.height,
            qp,
            chroma_format: 1,
        },
        coeffs_y,
        coeffs_u,
        coeffs_v,
    })
}

/// Decode a BlockQuant bitstream back into a YUV frame.
///
/// In-circuit code calls this. Deterministic, panic-free, bit-exact.
pub fn decode_toy(bitstream: &BqBitstream) -> Result<YuvFrame, ToyCodecError> {
    let BqHeader {
        width,
        height,
        qp,
        chroma_format,
    } = bitstream.header;

    if width == 0 || height == 0 || width % 16 != 0 || height % 16 != 0 {
        return Err(ToyCodecError::InvalidDimensions);
    }
    if chroma_format != 1 {
        return Err(ToyCodecError::InvalidChromaFormat);
    }
    if qp > 51 {
        return Err(ToyCodecError::Unsupported);
    }

    let w = width as usize;
    let h = height as usize;
    let cw = w / 2;
    let ch = h / 2;

    if bitstream.coeffs_y.len() != w * h
        || bitstream.coeffs_u.len() != cw * ch
        || bitstream.coeffs_v.len() != cw * ch
    {
        return Err(ToyCodecError::BufferTooSmall);
    }

    let step = quant_step(qp);
    let y = decode_plane(&bitstream.coeffs_y, w, h, step);
    let u = decode_plane(&bitstream.coeffs_u, cw, ch, step);
    let v = decode_plane(&bitstream.coeffs_v, cw, ch, step);

    Ok(YuvFrame {
        width,
        height,
        y,
        u,
        v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_frame(w: u16, h: u16, y: u8, u: u8, v: u8) -> YuvFrame {
        let cw = (w / 2) as usize;
        let ch = (h / 2) as usize;
        YuvFrame {
            width: w,
            height: h,
            y: vec![y; w as usize * h as usize],
            u: vec![u; cw * ch],
            v: vec![v; cw * ch],
        }
    }

    fn ramp_frame(w: u16, h: u16) -> YuvFrame {
        let wu = w as usize;
        let hu = h as usize;
        let cw = wu / 2;
        let ch = hu / 2;
        let y: Vec<u8> = (0..wu * hu).map(|i| (i & 0xff) as u8).collect();
        let u: Vec<u8> = (0..cw * ch).map(|i| ((i * 3) & 0xff) as u8).collect();
        let v: Vec<u8> = (0..cw * ch).map(|i| ((i * 5 + 17) & 0xff) as u8).collect();
        YuvFrame { width: w, height: h, y, u, v }
    }

    /// Deterministic pseudo-random pattern via xorshift. Has enough
    /// high-frequency content that quantization is observable.
    fn noise_frame(w: u16, h: u16) -> YuvFrame {
        fn xs(mut s: u32) -> u32 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s
        }
        let wu = w as usize;
        let hu = h as usize;
        let cw = wu / 2;
        let ch = hu / 2;
        let mut y = Vec::with_capacity(wu * hu);
        let mut state = 0xdeadbeefu32;
        for _ in 0..wu * hu {
            state = xs(state);
            y.push((state & 0xff) as u8);
        }
        let mut u = Vec::with_capacity(cw * ch);
        for _ in 0..cw * ch {
            state = xs(state);
            u.push((state & 0xff) as u8);
        }
        let mut v = Vec::with_capacity(cw * ch);
        for _ in 0..cw * ch {
            state = xs(state);
            v.push((state & 0xff) as u8);
        }
        YuvFrame { width: w, height: h, y, u, v }
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        assert_eq!(a.len(), b.len());
        let mut sse: u64 = 0;
        for i in 0..a.len() {
            let d = a[i] as i32 - b[i] as i32;
            sse += (d * d) as u64;
        }
        if sse == 0 {
            return f64::INFINITY;
        }
        let mse = sse as f64 / a.len() as f64;
        10.0 * (255.0_f64 * 255.0 / mse).log10()
    }

    #[test]
    fn wht_self_inverse() {
        // Forward then forward = ×8 (per 1D WHT). Test on a fixed pattern.
        let mut v: [i32; 8] = [10, -3, 5, 17, -8, 0, 4, -1];
        let original = v;
        wht8(&mut v);
        wht8(&mut v);
        for i in 0..8 {
            assert_eq!(v[i], original[i] * 8);
        }
    }

    #[test]
    fn wht8x8_self_inverse_scaled_by_64() {
        let mut block: [i32; 64] = [0; 64];
        for i in 0..64 {
            block[i] = (i as i32) - 32;
        }
        let original = block;
        wht8x8(&mut block);
        wht8x8(&mut block);
        for i in 0..64 {
            assert_eq!(block[i], original[i] * 64);
        }
    }

    #[test]
    fn roundtrip_qp0_flat_block_lossless() {
        let frame = flat_frame(16, 16, 128, 128, 128);
        let bs = encode_toy(&frame, 0).unwrap();
        let decoded = decode_toy(&bs).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn roundtrip_qp0_ramp_lossless() {
        // qp=0 should be bit-exact for any in-range frame.
        let frame = ramp_frame(32, 32);
        let bs = encode_toy(&frame, 0).unwrap();
        let decoded = decode_toy(&bs).unwrap();
        assert_eq!(decoded.y, frame.y);
        assert_eq!(decoded.u, frame.u);
        assert_eq!(decoded.v, frame.v);
    }

    #[test]
    fn qp8_meets_40db_psnr_on_noise() {
        // M2 §3 acceptance: qp=8 should round-trip at PSNR ≥ 40 dB.
        // Uses a high-entropy pattern so quantization is observable.
        let frame = noise_frame(64, 64);
        let bs = encode_toy(&frame, 8).unwrap();
        let decoded = decode_toy(&bs).unwrap();
        let p = psnr(&frame.y, &decoded.y);
        assert!(p >= 40.0, "qp=8 PSNR(Y) = {} dB, expected ≥ 40", p);
    }

    #[test]
    fn qp_monotonic_psnr_on_noise() {
        // PSNR should drop as QP rises on a high-entropy pattern.
        let frame = noise_frame(32, 32);
        let bs0 = encode_toy(&frame, 0).unwrap();
        let bs32 = encode_toy(&frame, 32).unwrap();
        let p0 = psnr(&frame.y, &decode_toy(&bs0).unwrap().y);
        let p32 = psnr(&frame.y, &decode_toy(&bs32).unwrap().y);
        assert!(p0 > p32, "expected qp=0 PSNR > qp=32 PSNR (got {} vs {})", p0, p32);
    }

    #[test]
    fn roundtrip_qp0_noise_lossless() {
        let frame = noise_frame(32, 32);
        let bs = encode_toy(&frame, 0).unwrap();
        let decoded = decode_toy(&bs).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn rejects_non_16_dimensions() {
        let frame = flat_frame(8, 8, 128, 128, 128);
        assert_eq!(encode_toy(&frame, 0), Err(ToyCodecError::InvalidDimensions));
    }

    #[test]
    fn rejects_qp_above_51() {
        let frame = flat_frame(16, 16, 128, 128, 128);
        assert_eq!(encode_toy(&frame, 52), Err(ToyCodecError::Unsupported));
    }

    #[test]
    fn coefficient_count_matches_pixel_count() {
        let frame = flat_frame(32, 32, 128, 128, 128);
        let bs = encode_toy(&frame, 0).unwrap();
        assert_eq!(bs.coeffs_y.len(), 32 * 32);
        assert_eq!(bs.coeffs_u.len(), 16 * 16);
        assert_eq!(bs.coeffs_v.len(), 16 * 16);
    }
}
