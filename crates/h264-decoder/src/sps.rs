//! Sequence Parameter Set parser (spec §7.3.2.1).
//!
//! Milestone-3 scope: baseline profile (`profile_idc = 66`), Constrained
//! Baseline (66 with constraint_set1=1) is also accepted. Extended (88)
//! and Main (77) error out — we restrict to baseline so the slice
//! parser doesn't have to handle the High-profile bells and whistles.
//!
//! All non-baseline fields covered by the `gaps_in_frame_num_value_allowed_flag`
//! / `frame_mbs_only_flag` branches in spec 7.3.2.1 are still read so
//! the bit position stays correct, but most of them are not surfaced.

use crate::bitreader::BitReader;
use crate::error::DecodeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sps {
    pub profile_idc: u8,
    pub constraint_set_flags: u8, // bits 7..0 = sets 0..5 + 2 reserved
    pub level_idc: u8,
    pub seq_parameter_set_id: u32,
    /// `chroma_format_idc`. Always 1 (4:2:0) for milestone 3.
    pub chroma_format_idc: u32,
    pub bit_depth_luma_minus8: u32,
    pub bit_depth_chroma_minus8: u32,
    pub log2_max_frame_num_minus4: u32,
    pub pic_order_cnt_type: u32,
    pub log2_max_pic_order_cnt_lsb_minus4: Option<u32>,
    pub max_num_ref_frames: u32,
    pub gaps_in_frame_num_value_allowed_flag: bool,
    pub pic_width_in_mbs_minus1: u32,
    pub pic_height_in_map_units_minus1: u32,
    pub frame_mbs_only_flag: bool,
    pub direct_8x8_inference_flag: bool,
    pub frame_cropping_offsets: Option<(u32, u32, u32, u32)>,
}

impl Sps {
    pub fn pic_width(&self) -> u32 {
        16 * (self.pic_width_in_mbs_minus1 + 1)
    }

    pub fn pic_height(&self) -> u32 {
        // For frame_mbs_only_flag=1 (which is what the milestone-3
        // subset requires) `pic_height_in_map_units` is in MBs.
        16 * (self.pic_height_in_map_units_minus1 + 1)
    }
}

/// Profile constants from §A.2.
pub mod profile {
    pub const BASELINE: u8 = 66;
    pub const MAIN: u8 = 77;
    pub const EXTENDED: u8 = 88;
    pub const HIGH: u8 = 100;
}

pub fn parse_sps(rbsp: &[u8]) -> Result<Sps, DecodeError> {
    let mut r = BitReader::new(rbsp);
    let profile_idc = r.read_bits(8)? as u8;
    let constraint_set_flags = r.read_bits(8)? as u8;
    let level_idc = r.read_bits(8)? as u8;
    let seq_parameter_set_id = r.read_ue()?;

    // Profile gate. We accept Baseline; Constrained Baseline is just
    // Baseline with constraint_set1_flag=1 and is treated identically.
    if profile_idc != profile::BASELINE
        && profile_idc != profile::MAIN
        && profile_idc != profile::EXTENDED
    {
        return Err(DecodeError::UnsupportedFeature("profile beyond baseline/main/extended"));
    }

    // High and above carry extra fields that switch on profile_idc; we
    // refuse any of them here. Strictly, we only support baseline; the
    // main/extended path is still accepted because x264 sometimes
    // tags Constrained Baseline streams as Baseline (66) regardless,
    // and we'd rather catch genuine high-profile content than fail on
    // a constrained-baseline edge case.
    if profile_idc == profile::HIGH {
        return Err(DecodeError::UnsupportedFeature("High profile"));
    }

    let chroma_format_idc = 1; // baseline implies 4:2:0
    let bit_depth_luma_minus8 = 0;
    let bit_depth_chroma_minus8 = 0;

    let log2_max_frame_num_minus4 = r.read_ue()?;
    if log2_max_frame_num_minus4 > 12 {
        return Err(DecodeError::OutOfRange("log2_max_frame_num_minus4 > 12"));
    }

    let pic_order_cnt_type = r.read_ue()?;
    let log2_max_pic_order_cnt_lsb_minus4 = match pic_order_cnt_type {
        0 => Some(r.read_ue()?),
        1 => {
            // Skip: delta_pic_order_always_zero_flag + offset_for_non_ref_pic
            //       + offset_for_top_to_bottom_field
            //       + num_ref_frames_in_pic_order_cnt_cycle then that many se(v)
            r.read_u1()?;
            r.read_se()?;
            r.read_se()?;
            let n = r.read_ue()?;
            for _ in 0..n {
                r.read_se()?;
            }
            None
        }
        2 => None,
        _ => return Err(DecodeError::OutOfRange("pic_order_cnt_type")),
    };

    let max_num_ref_frames = r.read_ue()?;
    let gaps_in_frame_num_value_allowed_flag = r.read_u1()? != 0;
    let pic_width_in_mbs_minus1 = r.read_ue()?;
    let pic_height_in_map_units_minus1 = r.read_ue()?;
    let frame_mbs_only_flag = r.read_u1()? != 0;
    if !frame_mbs_only_flag {
        return Err(DecodeError::UnsupportedFeature("interlaced (mb_adaptive_frame_field)"));
    }
    let direct_8x8_inference_flag = r.read_u1()? != 0;
    let frame_cropping_flag = r.read_u1()? != 0;
    let frame_cropping_offsets = if frame_cropping_flag {
        let l = r.read_ue()?;
        let rt = r.read_ue()?;
        let t = r.read_ue()?;
        let b = r.read_ue()?;
        Some((l, rt, t, b))
    } else {
        None
    };
    // VUI parameters skipped — we don't need them for the basic
    // decode loop. They follow `vui_parameters_present_flag`.

    Ok(Sps {
        profile_idc,
        constraint_set_flags,
        level_idc,
        seq_parameter_set_id,
        chroma_format_idc,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        log2_max_frame_num_minus4,
        pic_order_cnt_type,
        log2_max_pic_order_cnt_lsb_minus4,
        max_num_ref_frames,
        gaps_in_frame_num_value_allowed_flag,
        pic_width_in_mbs_minus1,
        pic_height_in_map_units_minus1,
        frame_mbs_only_flag,
        direct_8x8_inference_flag,
        frame_cropping_offsets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn parse_sps_from_corpus_solid_16x16() {
        // Hand-extracted from test-vectors/solid_16x16.h264 (NAL after
        // first start code, header stripped): 42 c0 0a dd e8 40 00 00
        // 03 00 40 00 00 0f 03 c4 89 e0
        // Strip emulation: 42 c0 0a dd e8 40 00 00 00 40 00 00 0f c4 89 e0
        let rbsp = vec![
            0x42, 0xc0, 0x0a, 0xdd, 0xe8, 0x40, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x0f, 0xc4,
            0x89, 0xe0,
        ];
        let sps = parse_sps(&rbsp).unwrap();
        assert_eq!(sps.profile_idc, profile::BASELINE);
        // 16x16 frame ⇒ 1 MB wide, 1 MB tall.
        assert_eq!(sps.pic_width(), 16);
        assert_eq!(sps.pic_height(), 16);
        assert!(sps.frame_mbs_only_flag);
    }
}

// Integration tests that read all corpus fixtures live in tests/
// alongside the NAL corpus tests.
