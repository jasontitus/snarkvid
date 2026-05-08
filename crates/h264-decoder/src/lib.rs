// Baseline H.264 I-frame decoder, no_std.
//
// Scope (milestones/03-h264-iframe.md §2):
//   IN:  baseline profile, single IDR slice, 4:2:0 8-bit, Intra_4x4 (all 9
//        modes), Intra_16x16 (all 4 modes), I_PCM, chroma Intra_8x8 (all 4
//        modes), 4x4 integer IDCT + DC Hadamard, CAVLC entropy decode,
//        Annex-B NAL framing with emulation-prevention byte removal,
//        Exp-Golomb codes.
//   OUT: P-frames (M4), B-frames (perm), CABAC (perm), deblocking (M4),
//        multi-slice (perm), 10-bit / 4:2:2 / 4:4:4 (perm).
//
// Module layout follows §4. Modules are introduced bottom-up so each
// one is fully tested before higher layers consume it:
//
//   bitreader  — raw bit reader + Exp-Golomb ue/se/te codes.    [STARTED]
//   nal        — Annex-B framing + emulation-prevention strip.   [STARTED]
//   cavlc      — five CAVLC tables + coeff parsing.              [pending]
//   slice      — slice header parser.                            [pending]
//   transform  — 4x4 integer IDCT + DC Hadamard.                 [pending]
//   quant      — inverse quantization tables.                    [pending]
//   intra      — Intra_4x4, Intra_16x16, chroma 8x8 prediction.  [pending]
//   mb         — per-MB decode loop.                             [pending]
//   frame      — the top-level decode_iframe entry point.        [pending]

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bitreader;
pub mod nal;

/// Errors that can occur during H.264 decoding.
///
/// One enum for the whole crate so the guest can commit a small,
/// stable status code per milestones/03-h264-iframe.md §3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Bit reader hit EOF mid-codeword.
    BitstreamTruncated,
    /// Invalid NAL unit framing.
    InvalidNalFraming,
    /// NAL unit type out of M3 scope (e.g. P-frame slice, multi-slice IDR).
    UnsupportedNalUnitType(u8),
    /// Exp-Golomb codeword exceeded the maximum length we accept.
    ExpGolombTooLong,
    /// SPS/PPS parsing decided the bitstream uses a feature M3 doesn't support.
    UnsupportedProfile,
    /// CAVLC reached an out-of-table state (placeholder for later work).
    CavlcInvalid,
    /// Generic "this bitstream violates the M3 scope" — accompanied by a
    /// short static reason string for triage.
    OutOfScope(&'static str),
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}

#[cfg(feature = "std")]
impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}
