//! NAL unit framing and emulation-prevention stripping.
//!
//! H.264 Annex B framing: each NAL unit is preceded by either a 3-byte
//! `00 00 01` or 4-byte `00 00 00 01` start code, then a 1-byte NAL
//! header, then the EBSP (emulation-prevention bytes embedded). Inside
//! the EBSP, any `00 00 03` triplet is the encoder's escape for
//! `00 00` followed by a byte in `00..=03`; the decoder strips the
//! `03` to recover the original RBSP.
//!
//! NAL header byte layout (spec §7.3.1):
//!   bit 0   forbidden_zero_bit
//!   bits 1-2 nal_ref_idc
//!   bits 3-7 nal_unit_type

use alloc::vec::Vec;

use crate::error::DecodeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalHeader {
    pub forbidden_zero_bit: u8,
    pub nal_ref_idc: u8,
    pub nal_unit_type: u8,
}

impl NalHeader {
    pub fn parse(byte: u8) -> Self {
        Self {
            forbidden_zero_bit: byte >> 7,
            nal_ref_idc: (byte >> 5) & 0x3,
            nal_unit_type: byte & 0x1F,
        }
    }
}

/// Common NAL unit types in the milestone-3 subset.
pub mod nal_unit_type {
    pub const NON_IDR_SLICE: u8 = 1;
    pub const IDR_SLICE: u8 = 5;
    pub const SEI: u8 = 6;
    pub const SPS: u8 = 7;
    pub const PPS: u8 = 8;
    pub const AUD: u8 = 9;
}

/// One framed NAL unit with its RBSP payload (emulation prevention
/// bytes already stripped).
#[derive(Debug, Clone)]
pub struct Nalu {
    pub header: NalHeader,
    pub rbsp: Vec<u8>,
}

/// Iterate the NAL units in an Annex-B-framed bitstream.
///
/// Returns an error if the bitstream doesn't begin with a start code.
/// Bytes between the last NAL unit and end-of-stream are tolerated (they
/// are typically zero-padding from the encoder).
pub fn iter_nalus(stream: &[u8]) -> Result<Vec<Nalu>, DecodeError> {
    let starts = find_start_codes(stream);
    if starts.is_empty() {
        return Err(DecodeError::NoStartCode);
    }
    let mut out = Vec::with_capacity(starts.len());
    for (i, &(sc_start, sc_len)) in starts.iter().enumerate() {
        let payload_start = sc_start + sc_len;
        let payload_end = if i + 1 < starts.len() {
            starts[i + 1].0
        } else {
            stream.len()
        };
        if payload_start >= payload_end {
            continue; // empty NAL — skip rather than error
        }
        let header = NalHeader::parse(stream[payload_start]);
        let ebsp = &stream[payload_start + 1..payload_end];
        let rbsp = strip_emulation_prevention(ebsp);
        out.push(Nalu { header, rbsp });
    }
    Ok(out)
}

/// Find every start code in `data`. Returns `(start_index, code_length)`
/// for each occurrence. `code_length` is 3 for `00 00 01` and 4 for
/// `00 00 00 01`.
fn find_start_codes(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                out.push((i, 3));
                i += 3;
                continue;
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                out.push((i, 4));
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Strip `00 00 03` emulation-prevention escapes (spec §7.4.1.1).
///
/// In an EBSP: any occurrence of bytes `00 00 03` where the trailing
/// byte's value is in `{00, 01, 02, 03}` represents the pair `00 00`
/// followed by that single byte; the encoder inserted the `03` to
/// prevent a false start code. We strip it on the decoder side.
fn strip_emulation_prevention(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut zero_run = 0usize;
    for &b in ebsp {
        if zero_run >= 2 && b == 0x03 {
            // Drop this byte. The next byte (the trailing one) is
            // copied verbatim. Reset the run so we don't accidentally
            // strip another 03.
            zero_run = 0;
            continue;
        }
        out.push(b);
        if b == 0x00 {
            zero_run += 1;
        } else {
            zero_run = 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn header_idr_slice() {
        // 0x65 = 0b0110_0101: ref_idc=3, type=5
        let h = NalHeader::parse(0x65);
        assert_eq!(h.forbidden_zero_bit, 0);
        assert_eq!(h.nal_ref_idc, 3);
        assert_eq!(h.nal_unit_type, nal_unit_type::IDR_SLICE);
    }

    #[test]
    fn finds_three_and_four_byte_start_codes() {
        let data = [
            0x00, 0x00, 0x00, 0x01, 0x67, 0xAA, // 4-byte SC + 2 bytes
            0x00, 0x00, 0x01, 0x68, // 3-byte SC + 1 byte
        ];
        let starts = find_start_codes(&data);
        assert_eq!(starts, vec![(0, 4), (6, 3)]);
    }

    #[test]
    fn strip_emulation_basic() {
        // 00 00 03 XX → 00 00 XX, only when XX in {00, 01, 02, 03}
        let ebsp = [0x00, 0x00, 0x03, 0x01];
        assert_eq!(strip_emulation_prevention(&ebsp), vec![0x00, 0x00, 0x01]);
    }

    #[test]
    fn strip_emulation_doesnt_eat_legit_03() {
        // 00 03 (only one zero) is legitimate and should be preserved.
        let ebsp = [0x00, 0x03, 0xAA];
        assert_eq!(strip_emulation_prevention(&ebsp), vec![0x00, 0x03, 0xAA]);
    }

    #[test]
    fn strip_emulation_consecutive() {
        // 00 00 03 00 00 03 01 → 00 00 00 00 01
        let ebsp = [0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x01];
        assert_eq!(
            strip_emulation_prevention(&ebsp),
            vec![0x00, 0x00, 0x00, 0x00, 0x01]
        );
    }

    #[test]
    fn iter_nalus_two_units() {
        let stream = [
            0x00, 0x00, 0x00, 0x01, // SC
            0x67, // header (ref_idc=3, type=7=SPS)
            0xAA, 0xBB, // body
            0x00, 0x00, 0x01, // SC
            0x68, // header (type=8=PPS)
            0xCC, // body
        ];
        let nalus = iter_nalus(&stream).unwrap();
        assert_eq!(nalus.len(), 2);
        assert_eq!(nalus[0].header.nal_unit_type, nal_unit_type::SPS);
        assert_eq!(nalus[0].rbsp, vec![0xAA, 0xBB]);
        assert_eq!(nalus[1].header.nal_unit_type, nal_unit_type::PPS);
        assert_eq!(nalus[1].rbsp, vec![0xCC]);
    }

    #[test]
    fn iter_nalus_no_start_code_errors() {
        let stream = [0x67, 0xAA];
        assert!(matches!(iter_nalus(&stream), Err(DecodeError::NoStartCode)));
    }

    #[test]
    fn iter_nalus_strips_emulation() {
        // SPS NAL with an emulation byte embedded in its body:
        // body bytes: 00 00 03 01 → stripped to 00 00 01
        let stream = [
            0x00, 0x00, 0x00, 0x01, // SC
            0x67, // header
            0x00, 0x00, 0x03, 0x01, // EBSP
        ];
        let nalus = iter_nalus(&stream).unwrap();
        assert_eq!(nalus.len(), 1);
        assert_eq!(nalus[0].rbsp, vec![0x00, 0x00, 0x01]);
    }
}
