//! Snarkvid H.264 baseline-profile decoder.
//!
//! Builds bottom-up:
//!   bitreader  →  nal  →  cavlc  →  transform / quant  →  intra  →  mb  →  frame
//!
//! Scope (milestones/03-h264-iframe.md §2):
//!   - Baseline profile, single IDR slice per NAL stream, 4:2:0 8-bit
//!   - Intra_4x4, Intra_16x16, I_PCM; chroma Intra_8x8
//!   - 4x4 integer transform + DC Hadamard for 16x16 luma DC + chroma DC
//!   - CAVLC entropy
//!   - No deblocking
//!
//! Out: P/B frames, CABAC, multi-slice, FMO/ASO, weighted prediction,
//!      10-bit, 4:2:2 / 4:4:4.

#![no_std]
extern crate alloc;

pub mod bitreader;
pub mod error;
pub mod nal;

pub use error::DecodeError;
pub use nal::{iter_nalus, nal_unit_type, NalHeader, Nalu};
