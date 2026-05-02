//! Picture Parameter Set parser (spec §7.3.2.2).
//!
//! Milestone-3 scope: baseline profile, so:
//!   - `entropy_coding_mode_flag` MUST be 0 (CAVLC).
//!   - No second slice group.
//!   - No `transform_8x8_mode_flag` (that's High-profile only anyway).
//!
//! Fields not needed for the decode path are still read so the bit
//! cursor stays aligned for any subsequent syntax we add later.

use crate::bitreader::BitReader;
use crate::error::DecodeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pps {
    pub pic_parameter_set_id: u32,
    pub seq_parameter_set_id: u32,
    pub entropy_coding_mode_flag: bool,
    pub bottom_field_pic_order_in_frame_present_flag: bool,
    pub num_slice_groups_minus1: u32,
    pub num_ref_idx_l0_default_active_minus1: u32,
    pub num_ref_idx_l1_default_active_minus1: u32,
    pub weighted_pred_flag: bool,
    pub weighted_bipred_idc: u32,
    pub pic_init_qp_minus26: i32,
    pub pic_init_qs_minus26: i32,
    pub chroma_qp_index_offset: i32,
    pub deblocking_filter_control_present_flag: bool,
    pub constrained_intra_pred_flag: bool,
    pub redundant_pic_cnt_present_flag: bool,
}

pub fn parse_pps(rbsp: &[u8]) -> Result<Pps, DecodeError> {
    let mut r = BitReader::new(rbsp);
    let pic_parameter_set_id = r.read_ue()?;
    let seq_parameter_set_id = r.read_ue()?;
    let entropy_coding_mode_flag = r.read_u1()? != 0;
    if entropy_coding_mode_flag {
        return Err(DecodeError::UnsupportedFeature("CABAC entropy"));
    }
    let bottom_field_pic_order_in_frame_present_flag = r.read_u1()? != 0;
    let num_slice_groups_minus1 = r.read_ue()?;
    if num_slice_groups_minus1 != 0 {
        return Err(DecodeError::UnsupportedFeature("FMO / multi-slice-group"));
    }
    let num_ref_idx_l0_default_active_minus1 = r.read_ue()?;
    let num_ref_idx_l1_default_active_minus1 = r.read_ue()?;
    let weighted_pred_flag = r.read_u1()? != 0;
    let weighted_bipred_idc = r.read_bits(2)?;
    let pic_init_qp_minus26 = r.read_se()?;
    let pic_init_qs_minus26 = r.read_se()?;
    let chroma_qp_index_offset = r.read_se()?;
    let deblocking_filter_control_present_flag = r.read_u1()? != 0;
    let constrained_intra_pred_flag = r.read_u1()? != 0;
    let redundant_pic_cnt_present_flag = r.read_u1()? != 0;
    Ok(Pps {
        pic_parameter_set_id,
        seq_parameter_set_id,
        entropy_coding_mode_flag,
        bottom_field_pic_order_in_frame_present_flag,
        num_slice_groups_minus1,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        weighted_pred_flag,
        weighted_bipred_idc,
        pic_init_qp_minus26,
        pic_init_qs_minus26,
        chroma_qp_index_offset,
        deblocking_filter_control_present_flag,
        constrained_intra_pred_flag,
        redundant_pic_cnt_present_flag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn parse_pps_baseline_minimal() {
        // Minimal baseline-style PPS body: pps_id=0, sps_id=0,
        // entropy_coding_mode=0, bottom_field=0, slice_groups=0,
        // ref_idx_l0=0, ref_idx_l1=0, weighted_pred=0, weighted_bipred=0,
        // pic_init_qp=0, pic_init_qs=0, chroma_qp_offset=0,
        // deblock_present=1, constrained_intra=0, redundant=0
        // ue(0) → "1", se(0) → "1"
        // Bit stream:
        //   pps_id=0           → 1
        //   sps_id=0           → 1
        //   entropy=0          → 0
        //   bf=0               → 0
        //   sg=0 (ue 0)        → 1
        //   ref0=0             → 1
        //   ref1=0             → 1
        //   wpred=0            → 0
        //   wbipred=00         → 00
        //   piq=0              → 1
        //   pis=0              → 1
        //   cqp_off=0          → 1
        //   deblock=1          → 1
        //   constr=0           → 0
        //   redund=0           → 0
        // Concatenated: 11 0 0 1 1 1 0 00 1 1 1 1 0 0  (16 bits)
        //   → 1100 1110 0011 1100 = 0xCE3C
        // Plus stop bit + alignment to byte: append "1000_0000"
        // → 0xCE 0x3C 0x80
        let rbsp = vec![0xCE, 0x3C, 0x80];
        let pps = parse_pps(&rbsp).unwrap();
        assert_eq!(pps.pic_parameter_set_id, 0);
        assert_eq!(pps.seq_parameter_set_id, 0);
        assert!(!pps.entropy_coding_mode_flag);
        assert_eq!(pps.num_slice_groups_minus1, 0);
        assert_eq!(pps.pic_init_qp_minus26, 0);
        assert!(pps.deblocking_filter_control_present_flag);
    }

    #[test]
    fn cabac_rejected() {
        // Same as above but with entropy_coding_mode_flag = 1.
        // Bit stream: 1 1 1 0 1 1 1 0 00 1 1 1 1 0 0 → 11101110 00111100 → 0xEE 0x3C
        let rbsp = vec![0xEE, 0x3C, 0x80];
        let r = parse_pps(&rbsp);
        assert!(matches!(
            r,
            Err(DecodeError::UnsupportedFeature(s)) if s.contains("CABAC")
        ));
    }
}
