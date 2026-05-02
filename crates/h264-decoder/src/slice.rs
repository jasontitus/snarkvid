//! Slice header parser (spec §7.3.3).
//!
//! Milestone-3 scope: we parse just enough to support I-slices in
//! baseline streams. P/B/SP/SI branches are accepted at the field
//! level (so we keep the bit cursor synchronized) but the resulting
//! `SliceType` is checked against {I_ALL_MB} before any caller can
//! drive the macroblock decoder.

use crate::bitreader::BitReader;
use crate::error::DecodeError;
use crate::nal::NalHeader;
use crate::pps::Pps;
use crate::sps::Sps;

/// `slice_type` values per spec §7.4.3 Table 7-6.
///
/// Values 5..=9 mean "all macroblocks of the slice are of the
/// corresponding type"; we collapse the two halves to one logical
/// `SliceType` since the decode path doesn't differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceType {
    P,
    B,
    I,
    SP,
    SI,
}

impl SliceType {
    pub fn from_raw(v: u32) -> Result<Self, DecodeError> {
        match v % 5 {
            0 => Ok(SliceType::P),
            1 => Ok(SliceType::B),
            2 => Ok(SliceType::I),
            3 => Ok(SliceType::SP),
            4 => Ok(SliceType::SI),
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceHeader {
    pub first_mb_in_slice: u32,
    pub slice_type: SliceType,
    pub pic_parameter_set_id: u32,
    pub frame_num: u32,
    pub idr_pic_id: Option<u32>,
    pub pic_order_cnt_lsb: Option<u32>,
    pub slice_qp_delta: i32,
    pub disable_deblocking_filter_idc: u32,
    pub slice_alpha_c0_offset_div2: i32,
    pub slice_beta_offset_div2: i32,
}

impl SliceHeader {
    /// Initial QP for the slice = pic_init_qp + slice_qp_delta + 26.
    pub fn slice_qp(&self, pps: &Pps) -> i32 {
        26 + pps.pic_init_qp_minus26 + self.slice_qp_delta
    }
}

pub fn parse_slice_header<'a>(
    rbsp: &'a [u8],
    nal_header: &NalHeader,
    sps: &Sps,
    pps: &Pps,
) -> Result<(SliceHeader, BitReader<'a>), DecodeError> {
    let mut r = BitReader::new(rbsp);

    let first_mb_in_slice = r.read_ue()?;
    let slice_type_raw = r.read_ue()?;
    if slice_type_raw > 9 {
        return Err(DecodeError::OutOfRange("slice_type"));
    }
    let slice_type = SliceType::from_raw(slice_type_raw)?;

    let pic_parameter_set_id = r.read_ue()?;
    if pic_parameter_set_id != pps.pic_parameter_set_id {
        return Err(DecodeError::OutOfRange("slice references unknown PPS id"));
    }

    // The milestone-3 subset is I-slices only.
    if slice_type != SliceType::I {
        return Err(DecodeError::UnsupportedFeature("non-I slice"));
    }

    let frame_num_bits = sps.log2_max_frame_num_minus4 + 4;
    let frame_num = r.read_bits(frame_num_bits)?;

    // frame_mbs_only_flag is required to be 1 by the milestone-3 SPS
    // gate, so no field_pic_flag.

    let is_idr = nal_header.nal_unit_type == crate::nal::nal_unit_type::IDR_SLICE;
    let idr_pic_id = if is_idr { Some(r.read_ue()?) } else { None };

    let pic_order_cnt_lsb = if sps.pic_order_cnt_type == 0 {
        let bits = sps
            .log2_max_pic_order_cnt_lsb_minus4
            .ok_or(DecodeError::OutOfRange("missing log2_max_pic_order_cnt_lsb"))?
            + 4;
        Some(r.read_bits(bits)?)
    } else {
        None
    };
    if sps.pic_order_cnt_type == 1 {
        // Skip delta_pic_order_cnt[0] and possibly [1] — we don't
        // actually need their values for I-slice decode, just for
        // POC tracking, which the milestone-3 single-frame path
        // doesn't exercise.
        return Err(DecodeError::UnsupportedFeature("pic_order_cnt_type=1"));
    }

    if pps.redundant_pic_cnt_present_flag {
        let _redundant_pic_cnt = r.read_ue()?;
    }

    // ref_pic_list_modification(): for I-slices it has no fields per
    // spec §7.3.3.1.

    // dec_ref_pic_marking() for IDR (§7.3.3.3): two flags.
    if nal_header.nal_ref_idc != 0 {
        if is_idr {
            let _no_output_of_prior_pics_flag = r.read_u1()?;
            let _long_term_reference_flag = r.read_u1()?;
        } else {
            // Adaptive marking loop. For I-slices in our corpus this
            // shouldn't appear (every fixture is IDR), but we keep
            // the parse correct for future test data.
            let adaptive = r.read_u1()? != 0;
            if adaptive {
                loop {
                    let mmco = r.read_ue()?;
                    if mmco == 0 {
                        break;
                    }
                    if mmco == 1 || mmco == 3 {
                        let _difference_of_pic_nums_minus1 = r.read_ue()?;
                    }
                    if mmco == 2 {
                        let _long_term_pic_num = r.read_ue()?;
                    }
                    if mmco == 3 || mmco == 6 {
                        let _long_term_frame_idx = r.read_ue()?;
                    }
                    if mmco == 4 {
                        let _max_long_term_frame_idx_plus1 = r.read_ue()?;
                    }
                    if mmco > 6 {
                        return Err(DecodeError::OutOfRange("mmco"));
                    }
                }
            }
        }
    }

    let slice_qp_delta = r.read_se()?;

    let mut disable_deblocking_filter_idc = 0;
    let mut slice_alpha_c0_offset_div2 = 0;
    let mut slice_beta_offset_div2 = 0;
    if pps.deblocking_filter_control_present_flag {
        disable_deblocking_filter_idc = r.read_ue()?;
        if disable_deblocking_filter_idc != 1 {
            slice_alpha_c0_offset_div2 = r.read_se()?;
            slice_beta_offset_div2 = r.read_se()?;
        }
    }

    Ok((
        SliceHeader {
            first_mb_in_slice,
            slice_type,
            pic_parameter_set_id,
            frame_num,
            idr_pic_id,
            pic_order_cnt_lsb,
            slice_qp_delta,
            disable_deblocking_filter_idc,
            slice_alpha_c0_offset_div2,
            slice_beta_offset_div2,
        },
        r,
    ))
}
