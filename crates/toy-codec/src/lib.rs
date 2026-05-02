//! BlockQuant — the milestone-2 toy codec.
//!
//! Designed to be the dumbest non-trivial codec we can put end-to-end:
//! 8x8 block partition, 2D Walsh–Hadamard transform, uniform scalar
//! quantization, raw i16 coefficients on the wire. No entropy coding,
//! no prediction, no temporal coding.
//!
//! The point is to exercise the rest of the architecture (manifest +
//! Merkle authentication + comparator + browser verifier) on real image
//! data without having an H.264 decoder yet. The decoder is `no_std`
//! so the same code runs natively and inside a zkVM guest.
//!
//! Bitstream layout:
//!
//! ```text
//!   magic:      "TOY1"          4 bytes
//!   width:      u32 LE          4 bytes  (must be multiple of 16)
//!   height:     u32 LE          4 bytes  (must be multiple of 16)
//!   qp:         u8              1 byte   (1..=64)
//!   chroma_fmt: u8              1 byte   (0 = 4:2:0 only)
//!   reserved:   [u8; 2]         2 bytes
//!   y_coefs:    i16 * (W*H/64)             little-endian
//!   u_coefs:    i16 * (W*H/256)            little-endian
//!   v_coefs:    i16 * (W*H/256)            little-endian
//! ```
//!
//! Each plane's coefficients are emitted block-by-block in raster
//! order; within a block, coefficients are emitted in raster order
//! (`coef_y * 8 + coef_x`).
//!
//! WHT scaling: we use the un-normalized 1D length-8 Walsh–Hadamard,
//! whose 2D form satisfies `WHT(WHT(x)) == 64 * x` exactly. With
//! integer rounding `(acc + 32) >> 6` on the inverse, round-trip at
//! `qp == 1` is bit-exact for any input that fits in the WHT's domain.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

pub const MAGIC: &[u8; 4] = b"TOY1";
pub const HEADER_LEN: usize = 16;
pub const BLOCK: usize = 8;
pub const CHROMA_420: u8 = 0;

#[derive(Debug, PartialEq, Eq)]
pub enum CodecError {
    Truncated { have: usize, need: usize },
    BadMagic,
    BadChromaFormat(u8),
    BadQp(u8),
    BadDimension(u32),
    BadPlaneLengths,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { have, need } => {
                write!(f, "bitstream too short: have {}, need {}", have, need)
            }
            Self::BadMagic => f.write_str("bad magic bytes: expected TOY1"),
            Self::BadChromaFormat(v) => write!(f, "unsupported chroma format {} (only 4:2:0)", v),
            Self::BadQp(v) => write!(f, "bad qp {} (must be 1..=64)", v),
            Self::BadDimension(v) => write!(f, "dimension {} must be a positive multiple of 16", v),
            Self::BadPlaneLengths => f.write_str("YUV plane lengths inconsistent with width/height"),
        }
    }
}

impl core::error::Error for CodecError {}

/// Raw YUV 4:2:0 frame. Each plane is a flat row-major slice of u8.
#[derive(Debug, Clone)]
pub struct YuvFrame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl YuvFrame {
    pub fn check(&self) -> Result<(), CodecError> {
        if self.width == 0 || self.width % 16 != 0 {
            return Err(CodecError::BadDimension(self.width));
        }
        if self.height == 0 || self.height % 16 != 0 {
            return Err(CodecError::BadDimension(self.height));
        }
        let yn = (self.width as usize) * (self.height as usize);
        let cn = yn / 4;
        if self.y.len() != yn || self.u.len() != cn || self.v.len() != cn {
            return Err(CodecError::BadPlaneLengths);
        }
        Ok(())
    }
}

// -------- Walsh–Hadamard primitives --------

/// In-place radix-2 length-8 Walsh–Hadamard transform on i32.
///
/// `WHT(WHT(x)) == 8 * x` in 1D. Forward and inverse use the same
/// butterfly because the matrix is symmetric.
fn wht8_1d(v: &mut [i32; 8]) {
    // Stage 1: pair sums/diffs.
    let (a0, a1) = (v[0] + v[1], v[0] - v[1]);
    let (a2, a3) = (v[2] + v[3], v[2] - v[3]);
    let (a4, a5) = (v[4] + v[5], v[4] - v[5]);
    let (a6, a7) = (v[6] + v[7], v[6] - v[7]);
    // Stage 2: 4-wide butterfly.
    let (b0, b2) = (a0 + a2, a0 - a2);
    let (b1, b3) = (a1 + a3, a1 - a3);
    let (b4, b6) = (a4 + a6, a4 - a6);
    let (b5, b7) = (a5 + a7, a5 - a7);
    // Stage 3: 8-wide butterfly. Output ordering matches the
    // natural Hadamard matrix rows defined in the lib docs.
    v[0] = b0 + b4;
    v[1] = b1 + b5;
    v[2] = b2 + b6;
    v[3] = b3 + b7;
    v[4] = b0 - b4;
    v[5] = b1 - b5;
    v[6] = b2 - b6;
    v[7] = b3 - b7;
}

/// Forward 2D 8x8 WHT on an i32 block. Output is `block_in * 64` worth
/// of coefficients (because we run the un-normalized 1D twice).
fn wht8_2d(block: &mut [i32; 64]) {
    // Rows.
    for r in 0..8 {
        let mut row = [0i32; 8];
        for c in 0..8 {
            row[c] = block[r * 8 + c];
        }
        wht8_1d(&mut row);
        for c in 0..8 {
            block[r * 8 + c] = row[c];
        }
    }
    // Columns.
    for c in 0..8 {
        let mut col = [0i32; 8];
        for r in 0..8 {
            col[r] = block[r * 8 + c];
        }
        wht8_1d(&mut col);
        for r in 0..8 {
            block[r * 8 + c] = col[r];
        }
    }
}

/// Inverse 2D 8x8 WHT (same butterfly), then divide by 64 with
/// rounding to recover spatial samples.
fn iwht8_2d_round(block: &mut [i32; 64]) {
    wht8_2d(block);
    for v in block.iter_mut() {
        *v = (*v + 32) >> 6;
    }
}

// -------- Encoder --------

/// Compress a YUV frame at quantization parameter `qp` (1..=64).
pub fn encode(frame: &YuvFrame, qp: u8) -> Result<Vec<u8>, CodecError> {
    frame.check()?;
    if qp == 0 || qp > 64 {
        return Err(CodecError::BadQp(qp));
    }
    let w = frame.width as usize;
    let h = frame.height as usize;
    let cw = w / 2;
    let ch = h / 2;
    let n_y = (w / BLOCK) * (h / BLOCK);
    let n_c = (cw / BLOCK) * (ch / BLOCK);
    let mut out = Vec::with_capacity(HEADER_LEN + (n_y + 2 * n_c) * 64 * 2);

    // Header.
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(frame.width as u32).to_le_bytes());
    out.extend_from_slice(&(frame.height as u32).to_le_bytes());
    out.push(qp);
    out.push(CHROMA_420);
    out.extend_from_slice(&[0u8, 0u8]); // reserved

    // Body.
    encode_plane(&frame.y, w, h, qp, &mut out);
    encode_plane(&frame.u, cw, ch, qp, &mut out);
    encode_plane(&frame.v, cw, ch, qp, &mut out);

    Ok(out)
}

fn encode_plane(plane: &[u8], w: usize, h: usize, qp: u8, out: &mut Vec<u8>) {
    let qp_i = qp as i32;
    for by in 0..(h / BLOCK) {
        for bx in 0..(w / BLOCK) {
            let mut block = [0i32; 64];
            for ry in 0..BLOCK {
                for rx in 0..BLOCK {
                    let px = plane[(by * BLOCK + ry) * w + (bx * BLOCK + rx)] as i32;
                    block[ry * 8 + rx] = px;
                }
            }
            wht8_2d(&mut block);
            for v in block.iter() {
                // Symmetric rounding division for negative values.
                let q = round_div(*v, qp_i);
                let q16 = q.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                out.extend_from_slice(&q16.to_le_bytes());
            }
        }
    }
}

fn round_div(num: i32, den: i32) -> i32 {
    debug_assert!(den > 0);
    if num >= 0 {
        (num + den / 2) / den
    } else {
        -(((-num) + den / 2) / den)
    }
}

// -------- Decoder --------

#[derive(Debug, Clone)]
pub struct ToyHeader {
    pub width: u32,
    pub height: u32,
    pub qp: u8,
}

pub fn parse_header(bytes: &[u8]) -> Result<ToyHeader, CodecError> {
    if bytes.len() < HEADER_LEN {
        return Err(CodecError::Truncated {
            have: bytes.len(),
            need: HEADER_LEN,
        });
    }
    if &bytes[0..4] != MAGIC {
        return Err(CodecError::BadMagic);
    }
    let width = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let height = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let qp = bytes[12];
    let cf = bytes[13];
    if cf != CHROMA_420 {
        return Err(CodecError::BadChromaFormat(cf));
    }
    if qp == 0 || qp > 64 {
        return Err(CodecError::BadQp(qp));
    }
    if width == 0 || width % 16 != 0 {
        return Err(CodecError::BadDimension(width));
    }
    if height == 0 || height % 16 != 0 {
        return Err(CodecError::BadDimension(height));
    }
    Ok(ToyHeader { width, height, qp })
}

pub fn decode(bytes: &[u8]) -> Result<YuvFrame, CodecError> {
    let header = parse_header(bytes)?;
    let w = header.width as usize;
    let h = header.height as usize;
    let cw = w / 2;
    let ch = h / 2;
    let n_y_coefs = (w * h / 64) * 64;
    let n_c_coefs = (cw * ch / 64) * 64;
    let need = HEADER_LEN + 2 * (n_y_coefs + 2 * n_c_coefs);
    if bytes.len() < need {
        return Err(CodecError::Truncated {
            have: bytes.len(),
            need,
        });
    }

    let qp = header.qp as i32;
    let mut cursor = HEADER_LEN;
    let y = decode_plane(&bytes[cursor..], w, h, qp);
    cursor += n_y_coefs * 2;
    let u = decode_plane(&bytes[cursor..], cw, ch, qp);
    cursor += n_c_coefs * 2;
    let v = decode_plane(&bytes[cursor..], cw, ch, qp);

    Ok(YuvFrame {
        width: header.width,
        height: header.height,
        y,
        u,
        v,
    })
}

fn decode_plane(bytes: &[u8], w: usize, h: usize, qp: i32) -> Vec<u8> {
    let mut plane = alloc::vec![0u8; w * h];
    let mut cursor = 0;
    for by in 0..(h / BLOCK) {
        for bx in 0..(w / BLOCK) {
            let mut block = [0i32; 64];
            for k in 0..64 {
                let q = i16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]) as i32;
                cursor += 2;
                block[k] = q * qp; // dequantize
            }
            iwht8_2d_round(&mut block);
            for ry in 0..BLOCK {
                for rx in 0..BLOCK {
                    let pix = block[ry * 8 + rx].clamp(0, 255) as u8;
                    plane[(by * BLOCK + ry) * w + (bx * BLOCK + rx)] = pix;
                }
            }
        }
    }
    plane
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(width: u32, height: u32, fill_y: u8, fill_u: u8, fill_v: u8) -> YuvFrame {
        let yn = (width as usize) * (height as usize);
        let cn = yn / 4;
        YuvFrame {
            width,
            height,
            y: alloc::vec![fill_y; yn],
            u: alloc::vec![fill_u; cn],
            v: alloc::vec![fill_v; cn],
        }
    }

    #[test]
    fn wht_round_trip_constant_block() {
        let mut b = [42i32; 64];
        let original = b;
        wht8_2d(&mut b);
        iwht8_2d_round(&mut b);
        assert_eq!(b, original);
    }

    #[test]
    fn wht_round_trip_random() {
        // PRNG-free deterministic spread.
        let mut b = [0i32; 64];
        for (i, v) in b.iter_mut().enumerate() {
            *v = ((i as i32 * 37 + 7) % 256) as i32;
        }
        let original = b;
        wht8_2d(&mut b);
        iwht8_2d_round(&mut b);
        assert_eq!(b, original);
    }

    #[test]
    fn header_round_trip() {
        let f = fixture(16, 16, 128, 128, 128);
        let bytes = encode(&f, 1).unwrap();
        let h = parse_header(&bytes).unwrap();
        assert_eq!(h.width, 16);
        assert_eq!(h.height, 16);
        assert_eq!(h.qp, 1);
    }

    #[test]
    fn round_trip_constant_qp1_is_lossless() {
        let f = fixture(32, 16, 200, 64, 192);
        let bytes = encode(&f, 1).unwrap();
        let g = decode(&bytes).unwrap();
        assert_eq!(f.y, g.y);
        assert_eq!(f.u, g.u);
        assert_eq!(f.v, g.v);
    }

    #[test]
    fn round_trip_gradient_qp1_is_lossless() {
        // 16x16 horizontal Y gradient, constant chroma.
        let mut f = fixture(16, 16, 0, 128, 128);
        for y in 0..16 {
            for x in 0..16 {
                f.y[y * 16 + x] = ((x * 16) as u8).min(255);
            }
        }
        let bytes = encode(&f, 1).unwrap();
        let g = decode(&bytes).unwrap();
        assert_eq!(f.y, g.y);
    }

    #[test]
    fn higher_qp_is_lossy_but_close() {
        let mut f = fixture(32, 32, 0, 128, 128);
        // A diagonal pattern so AC coefficients are excited.
        for y in 0..32 {
            for x in 0..32 {
                f.y[y * 32 + x] = (((x + y) * 7) as u8).wrapping_mul(3);
            }
        }
        let bytes = encode(&f, 8).unwrap();
        let g = decode(&bytes).unwrap();

        let sse: u64 = f
            .y
            .iter()
            .zip(g.y.iter())
            .map(|(a, b)| {
                let d = (*a as i32) - (*b as i32);
                (d * d) as u64
            })
            .sum();
        let mse = sse as f64 / f.y.len() as f64;
        // Encoder is dumb; quality target is "not absurd" for the toy.
        assert!(mse < 200.0, "mse too high: {mse}");
    }

    #[test]
    fn rejects_bad_dimensions() {
        let mut f = fixture(16, 16, 0, 0, 0);
        f.width = 17;
        f.height = 16;
        f.y = alloc::vec![0u8; 17 * 16];
        // Plane lengths are intentionally inconsistent here, but the
        // dimension check should trip first.
        assert!(matches!(encode(&f, 1), Err(CodecError::BadDimension(17))));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode(&fixture(16, 16, 0, 0, 0), 1).unwrap();
        bytes[0] = b'X';
        assert!(matches!(parse_header(&bytes), Err(CodecError::BadMagic)));
    }

    #[test]
    fn rejects_truncated_bitstream() {
        let bytes = encode(&fixture(16, 16, 0, 0, 0), 1).unwrap();
        assert!(matches!(
            decode(&bytes[..bytes.len() - 1]),
            Err(CodecError::Truncated { .. })
        ));
    }
}
