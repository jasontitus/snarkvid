use core::fmt;

/// Errors emitted by the H.264 decoder.
///
/// Every variant represents a non-recoverable parse / spec violation —
/// the prover treats any of these as "this bitstream is invalid, refuse
/// to produce a proof." Out-of-spec features in the input are
/// distinguished from corruption so we can surface "encoder used a
/// feature we don't support" cleanly to the producer-side encoder
/// preset wrapper.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Reader ran out of bits before the requested read could finish.
    EndOfStream,
    /// Exp-Golomb prefix exceeded the supported width (>32 leading zeros).
    ExpGolombOverflow,
    /// `read_bits(n)` was called with `n > 32`.
    ReadTooWide,
    /// Bitstream did not start with a valid start code.
    NoStartCode,
    /// NAL unit type appears in the bitstream but isn't supported in the
    /// milestone-3 subset (e.g., 4:2:2 chroma, B-slice, CABAC).
    UnsupportedFeature(&'static str),
    /// Bitstream encodes a value outside the milestone-3 supported range
    /// (e.g., profile_idc != 66/77/88, ChromaArrayType != 1).
    OutOfRange(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndOfStream => f.write_str("h264: bitreader ran out of bits"),
            Self::ExpGolombOverflow => f.write_str("h264: Exp-Golomb prefix > 32 leading zeros"),
            Self::ReadTooWide => f.write_str("h264: read_bits called with width > 32"),
            Self::NoStartCode => f.write_str("h264: no NAL start code at expected position"),
            Self::UnsupportedFeature(s) => write!(f, "h264: unsupported feature: {s}"),
            Self::OutOfRange(s) => write!(f, "h264: out-of-range value: {s}"),
        }
    }
}

impl core::error::Error for DecodeError {}
