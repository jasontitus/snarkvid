// Annex-B NAL unit framer + emulation-prevention byte stripper.
//
// The H.264 byte stream as written by x264 (and as ffmpeg/libav
// emit by default) uses "Annex-B" framing: each NAL unit is preceded
// by a 3- or 4-byte start code (`00 00 01` or `00 00 00 01`). Inside
// a NAL unit, the encoder inserts an "emulation prevention" byte
// (`03`) any time the raw payload contains `00 00 00`, `00 00 01`,
// `00 00 02`, or `00 00 03`, so the start-code scanner can run over
// the whole stream without confusing payload bytes for delimiters.
//
// This module exposes two things:
//
//   - `NalUnitIterator<'a>` walks an Annex-B byte stream and yields
//     one `NalUnit<'a>` per start code.
//   - `strip_emulation_prevention` decodes a NAL payload's RBSP by
//     removing the 0x03 bytes the encoder inserted. The H.264 spec
//     calls this NAL → RBSP conversion (§7.3.1).
//
// Both are no_std and zero-allocation on the iterator path; the
// stripper allocates a Vec<u8> for the output RBSP.
//
// We don't yet parse slice headers or NAL unit headers beyond the
// 1-byte type field. Those land in slice.rs / mb.rs.

use alloc::vec::Vec;

use crate::DecodeError;

/// One framed NAL unit out of an Annex-B byte stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NalUnit<'a> {
    /// The full NAL unit including its 1-byte header (`forbidden_zero_bit`
    /// + `nal_ref_idc` + `nal_unit_type`). Does NOT include the start code.
    pub bytes: &'a [u8],
}

impl<'a> NalUnit<'a> {
    /// `nal_unit_type` (5 bits, low 5 of the first byte).
    pub fn unit_type(&self) -> Result<u8, DecodeError> {
        let header = *self.bytes.first().ok_or(DecodeError::InvalidNalFraming)?;
        // Spec §7.3.1: forbidden_zero_bit must be 0.
        if header & 0x80 != 0 {
            return Err(DecodeError::InvalidNalFraming);
        }
        Ok(header & 0x1f)
    }

    /// `nal_ref_idc` (2 bits, bits 5-6 of the first byte).
    pub fn ref_idc(&self) -> Result<u8, DecodeError> {
        let header = *self.bytes.first().ok_or(DecodeError::InvalidNalFraming)?;
        Ok((header >> 5) & 0x03)
    }

    /// The NAL unit's payload (everything after the 1-byte header).
    pub fn payload(&self) -> &'a [u8] {
        &self.bytes[1..]
    }
}

/// NAL unit type constants from H.264 spec Table 7-1.
pub mod nut {
    pub const NON_IDR_SLICE: u8 = 1;
    pub const IDR_SLICE: u8 = 5;
    pub const SEI: u8 = 6;
    pub const SPS: u8 = 7;
    pub const PPS: u8 = 8;
    pub const AU_DELIMITER: u8 = 9;
    pub const END_OF_SEQUENCE: u8 = 10;
    pub const END_OF_STREAM: u8 = 11;
    pub const FILLER_DATA: u8 = 12;
}

/// Iterator over Annex-B NAL units in a byte stream.
///
/// Walks left-to-right looking for start codes (`00 00 01` or
/// `00 00 00 01`); each match emits the bytes BETWEEN that start
/// code and the next start code (or EOF) as a `NalUnit`. Trailing
/// zero bytes immediately before the next start code are stripped
/// per §B.1.2.
pub struct NalUnitIterator<'a> {
    stream: &'a [u8],
    pos: usize,
}

impl<'a> NalUnitIterator<'a> {
    pub fn new(stream: &'a [u8]) -> Self {
        Self { stream, pos: 0 }
    }

    /// Find the next start-code position (the index of the first byte
    /// of the start code) at or after `from`. Returns `(start_idx,
    /// after_start_idx)` — where `start_idx` is the first `00` and
    /// `after_start_idx` is one past the trailing `01`, so the NAL
    /// payload begins at `after_start_idx`.
    fn find_start_code(stream: &[u8], from: usize) -> Option<(usize, usize)> {
        if stream.len() < 3 {
            return None;
        }
        let mut i = from;
        while i + 2 < stream.len() {
            if stream[i] == 0 && stream[i + 1] == 0 {
                if stream[i + 2] == 1 {
                    return Some((i, i + 3));
                }
                if stream[i + 2] == 0 && i + 3 < stream.len() && stream[i + 3] == 1 {
                    return Some((i, i + 4));
                }
            }
            i += 1;
        }
        None
    }
}

impl<'a> Iterator for NalUnitIterator<'a> {
    type Item = Result<NalUnit<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Find the start code that begins this NAL.
        let (_, payload_start) = Self::find_start_code(self.stream, self.pos)?;

        // Find the next start code that ends this NAL (or EOF).
        let payload_end = match Self::find_start_code(self.stream, payload_start) {
            Some((next_start_idx, _)) => next_start_idx,
            None => self.stream.len(),
        };

        // Strip trailing zero bytes per spec §B.1.2 ("trailing_zero_8bits"
        // following the NAL unit, before the next start code).
        let mut end = payload_end;
        while end > payload_start && self.stream[end - 1] == 0 {
            end -= 1;
        }

        if end <= payload_start {
            return Some(Err(DecodeError::InvalidNalFraming));
        }

        self.pos = payload_end;
        Some(Ok(NalUnit { bytes: &self.stream[payload_start..end] }))
    }
}

/// Convert a NAL unit payload (NAL → RBSP) by removing emulation-prevention
/// bytes. The encoder inserts a `03` byte after any `00 00` sequence whose
/// next byte is `00`, `01`, `02`, or `03`. The decoder reverses this.
///
/// Allocates a fresh `Vec<u8>` of length ≤ input.len(); deduplicating in
/// place would save the alloc but the input is borrowed and short anyway.
pub fn strip_emulation_prevention(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut i = 0;
    while i < payload.len() {
        // Look for the pattern `00 00 03` and skip the `03`.
        if i + 2 < payload.len()
            && payload[i] == 0
            && payload[i + 1] == 0
            && payload[i + 2] == 0x03
        {
            out.push(0);
            out.push(0);
            i += 3; // skip the 0x03
        } else {
            out.push(payload[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterator_walks_three_byte_and_four_byte_start_codes() {
        // Stream: [0,0,0,1, A,B] [0,0,1, C,D]
        let stream = [0, 0, 0, 1, 0xAA, 0xBB, 0, 0, 1, 0xCC, 0xDD];
        let mut it = NalUnitIterator::new(&stream);
        assert_eq!(it.next().unwrap().unwrap().bytes, &[0xAA, 0xBB]);
        assert_eq!(it.next().unwrap().unwrap().bytes, &[0xCC, 0xDD]);
        assert!(it.next().is_none());
    }

    #[test]
    fn iterator_strips_trailing_zero_padding() {
        // x264 sometimes emits trailing 0x00 bytes between NALs as
        // alignment padding — they must not appear in the payload.
        let stream = [0, 0, 0, 1, 0x67, 0xAB, 0x00, 0x00, 0, 0, 1, 0x68, 0x42];
        let mut it = NalUnitIterator::new(&stream);
        assert_eq!(it.next().unwrap().unwrap().bytes, &[0x67, 0xAB]);
        assert_eq!(it.next().unwrap().unwrap().bytes, &[0x68, 0x42]);
        assert!(it.next().is_none());
    }

    #[test]
    fn nal_unit_extracts_type_and_ref_idc_correctly() {
        // 0x67 = 0110_0111 → forbidden=0, ref_idc=11 (3), type=00111 (7=SPS).
        let nal = NalUnit { bytes: &[0x67, 0x42] };
        assert_eq!(nal.unit_type().unwrap(), nut::SPS);
        assert_eq!(nal.ref_idc().unwrap(), 3);
        assert_eq!(nal.payload(), &[0x42]);

        // 0x65 = 0110_0101 → ref_idc=11, type=00101 (5=IDR slice).
        let nal = NalUnit { bytes: &[0x65, 0xFF] };
        assert_eq!(nal.unit_type().unwrap(), nut::IDR_SLICE);
        assert_eq!(nal.ref_idc().unwrap(), 3);
    }

    #[test]
    fn nal_unit_rejects_forbidden_zero_bit() {
        // Top bit set → forbidden_zero_bit = 1 → reject.
        let nal = NalUnit { bytes: &[0x80] };
        assert!(matches!(nal.unit_type(), Err(DecodeError::InvalidNalFraming)));
    }

    #[test]
    fn strip_no_emulation_bytes_round_trips() {
        let payload = [0x65, 0x88, 0x77, 0x42, 0x10];
        assert_eq!(strip_emulation_prevention(&payload), payload.to_vec());
    }

    #[test]
    fn strip_emulation_byte_in_middle() {
        // Pattern `00 00 03 40` → `00 00 40` after stripping. Matches
        // the `00 00 03 00 40` we saw at offset 12 of the live x264 SPS.
        let payload = [0x67, 0x42, 0xC0, 0x0A, 0xDD, 0xE8, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40];
        let rbsp = strip_emulation_prevention(&payload);
        assert_eq!(rbsp, vec![0x67, 0x42, 0xC0, 0x0A, 0xDD, 0xE8, 0x40, 0x00, 0x00, 0x00, 0x40]);
    }

    #[test]
    fn strip_multiple_emulation_bytes() {
        // `00 00 03 00 00 03 01` → `00 00 00 00 01` (two emulation bytes removed).
        let payload = [0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x01];
        let rbsp = strip_emulation_prevention(&payload);
        assert_eq!(rbsp, vec![0x00, 0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn strip_does_not_remove_03_unless_preceded_by_two_zeros() {
        // `01 03 04 03 00` — neither `03` follows `00 00`, so neither is stripped.
        let payload = [0x01, 0x03, 0x04, 0x03, 0x00];
        let rbsp = strip_emulation_prevention(&payload);
        assert_eq!(rbsp, payload.to_vec());
    }

    // Live integration test against the corpus fixture, end-to-end.
    #[test]
    fn parses_x264_corpus_sps_pps_idr_in_order() {
        use snarkvid_h264_test_vectors::NOISE_16X16_QP18;
        let units: Vec<_> = NalUnitIterator::new(NOISE_16X16_QP18.h264)
            .collect::<Result<_, _>>()
            .expect("iterator");
        // x264 with --frames 1 typically writes SPS, PPS, SEI, IDR.
        let types: Vec<u8> = units.iter().map(|u| u.unit_type().unwrap()).collect();
        assert!(types.contains(&nut::SPS), "expected SPS in {:?}", types);
        assert!(types.contains(&nut::PPS), "expected PPS in {:?}", types);
        assert!(types.contains(&nut::IDR_SLICE), "expected IDR slice in {:?}", types);
        // SPS must come before PPS, PPS must come before IDR.
        let idx_sps = types.iter().position(|&t| t == nut::SPS).unwrap();
        let idx_pps = types.iter().position(|&t| t == nut::PPS).unwrap();
        let idx_idr = types.iter().position(|&t| t == nut::IDR_SLICE).unwrap();
        assert!(idx_sps < idx_pps);
        assert!(idx_pps < idx_idr);
    }
}
