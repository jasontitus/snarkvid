// BlockQuant toy codec — a deliberately simple image codec for milestone 2.
//
// Designed to be the dumbest possible thing that still exercises real
// integer arithmetic over real image data, without entropy coding or
// motion compensation. This lets us shake out the full architecture
// (manifest → Merkle → in-circuit decoder → comparator → browser
// verifier) without an H.264 decoder in the loop.
//
// Codec sketch (see milestones/02-toy-transform.md §2):
//   1. Partition each YUV plane into 8×8 blocks (Y full-res, U/V 4:2:0).
//   2. Forward transform: 2D Walsh-Hadamard (or integer DCT if fast enough).
//   3. Uniform quantization with a single QP.
//   4. Bitstream: header (width, height, qp, chroma_format) + i16 coeffs.
//
// Decode is the reverse: parse header → dequantize → inverse transform →
// clamp to [0,255].
//
// This crate is no_std and provides both encode and decode. The in-circuit
// decoder only calls decode_toy; the native toy-encode CLI calls encode_toy.

#![no_std]

extern crate alloc;
use alloc::vec::Vec;

#[cfg(feature = "std")]
extern crate std;

/// A single YUV 4:2:0 frame.
#[derive(Clone, Debug, PartialEq)]
pub struct YuvFrame {
    pub width: u16,   // must be multiple of 16 for 4:2:0
    pub height: u16,  // must be multiple of 16 for 4:2:0
    pub y: Vec<u8>,   // width * height
    pub u: Vec<u8>,   // (width/2) * (height/2)
    pub v: Vec<u8>,   // (width/2) * (height/2)
}

/// BlockQuant bitstream header.
#[derive(Clone, Debug, PartialEq)]
pub struct BqHeader {
    pub width: u16,
    pub height: u16,
    pub qp: u8,
    pub chroma_format: u8, // always 1 (4:2:0) for milestone 2
}

/// Compressed representation: header + coefficient stream.
#[derive(Clone, Debug, PartialEq)]
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

/// Encode a YUV frame into a BlockQuant bitstream.
///
/// The caller picks a QP (0 = lossless-ish, 51 = heaviest quantization).
/// Higher QP → smaller coefficients → coarser reconstruction.
pub fn encode_toy(frame: &YuvFrame, qp: u8) -> Result<BqBitstream, ToyCodecError> {
    if frame.width % 16 != 0 || frame.height % 16 != 0 {
        return Err(ToyCodecError::InvalidDimensions);
    }
    if qp > 51 {
        return Err(ToyCodecError::Unsupported);
    }

    // Milestone 2 day 1: implement the actual transform + quantization.
    // For now, return a stub that passes coefficients through unmodified
    // (lossless "QP=0").
    let coeffs_y = frame.y.iter().map(|&b| b as i16).collect();
    let coeffs_u = frame.u.iter().map(|&b| b as i16).collect();
    let coeffs_v = frame.v.iter().map(|&b| b as i16).collect();

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
/// This is the function the in-circuit decoder calls. It must be
/// deterministic, panic-free, and produce bit-exact output for the
/// same input regardless of platform or toolchain.
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

    let y_size = width as usize * height as usize;
    let uv_size = (width as usize / 2) * (height as usize / 2);

    if bitstream.coeffs_y.len() != y_size
        || bitstream.coeffs_u.len() != uv_size
        || bitstream.coeffs_v.len() != uv_size
    {
        return Err(ToyCodecError::BufferTooSmall);
    }

    // Milestone 2 day 1: implement inverse transform + dequantization.
    // Stub: for QP=0, coefficients are raw pixel values.
    // For QP>0, need dequant + inverse transform + clamp.
    let clamp = |v: i16| -> u8 {
        if v < 0 {
            0
        } else if v > 255 {
            255
        } else {
            v as u8
        }
    };

    let y: Vec<u8> = bitstream.coeffs_y.iter().map(|&c| clamp(c)).collect();
    let u: Vec<u8> = bitstream.coeffs_u.iter().map(|&c| clamp(c)).collect();
    let v: Vec<u8> = bitstream.coeffs_v.iter().map(|&c| clamp(c)).collect();

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
    use alloc::vec;

    /// Stub test — replace with real test vectors on milestone 2 day 1.
    #[test]
    fn roundtrip_lossless_stub() {
        let frame = YuvFrame {
            width: 16,
            height: 16,
            y: vec![128u8; 256],
            u: vec![128u8; 64],
            v: vec![128u8; 64],
        };
        let bs = encode_toy(&frame, 0).unwrap();
        let decoded = decode_toy(&bs).unwrap();
        // Stub: encode passes through raw bytes, decode returns them.
        // The real transform will not be lossless at QP>0.
        assert_eq!(decoded.y, frame.y);
        assert_eq!(decoded.u, frame.u);
        assert_eq!(decoded.v, frame.v);
    }
}
