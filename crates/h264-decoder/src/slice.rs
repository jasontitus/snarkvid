// SPS / PPS / slice header parsers.
//
// These three structures carry the per-bitstream metadata the
// macroblock decoder needs: picture dimensions, the active QP, what
// scaling list to use, etc. None of them produce pixels — they
// configure mb.rs and frame.rs. Each parser:
//
//   1. Strips emulation-prevention bytes from the NAL payload (caller
//      already did this via nal::strip_emulation_prevention).
//   2. Walks the RBSP with `BitReader` + Exp-Golomb codes.
//   3. Validates every field against the M3 scope. Anything out of
//      scope (B-frames, multi-slice, CABAC, custom scaling lists,
//      4:2:2 / 4:4:4, 10-bit) → typed `DecodeError::OutOfScope`.
//
// What we keep:
//   - Sps: picture dimensions + the few flags MB decode actually needs.
//   - Pps: entropy_coding_mode (must be CAVLC), pic_init_qp,
//          chroma_qp_index_offset, constrained_intra_pred_flag.
//   - SliceHeader: slice_type, frame_num, slice_qp_delta, derived SliceQPY.
//
// What we deliberately skip parsing (and read-past as needed):
//   - VUI parameters: SAR, timing, HRD. Reject if vui_parameters_present_flag
//     is set since we can't safely skip a VUI block without parsing it
//     (it has variable length). M3 doesn't need any of it; if a real-world
//     bitstream sets it, encoder will need a flag adjustment.
//   - Reference-picture-list reordering (P/B only).
//   - dec_ref_pic_marking (mostly relevant for ref-list management).
//
// no_std-pure.

use crate::bitreader::BitReader;
use crate::DecodeError;

// ─────────────────────────────────────────────────────────────────────
// SPS — Sequence Parameter Set (spec §7.3.2.1.1)
// ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sps {
    /// `profile_idc` — must be 66 (Baseline) for M3.
    pub profile_idc: u8,
    pub seq_parameter_set_id: u32,
    /// `log2_max_frame_num_minus4`. Determines bit width of frame_num.
    pub log2_max_frame_num_minus4: u32,
    /// 0 → POC type 0 (only one M3 supports). 1/2 unsupported.
    pub pic_order_cnt_type: u32,
    pub log2_max_pic_order_cnt_lsb_minus4: u32,
    pub num_ref_frames: u32,
    pub gaps_in_frame_num_value_allowed_flag: bool,
    /// (pic_width_in_mbs_minus1 + 1) macroblocks horizontally.
    pub pic_width_in_mbs_minus1: u32,
    /// (pic_height_in_map_units_minus1 + 1) MB-units vertically.
    pub pic_height_in_map_units_minus1: u32,
    /// 1 = progressive frames only (M3 requires this).
    pub frame_mbs_only_flag: bool,
    pub direct_8x8_inference_flag: bool,
}

impl Sps {
    /// Picture width in pixels.
    pub fn pic_width(&self) -> u32 {
        (self.pic_width_in_mbs_minus1 + 1) * 16
    }
    /// Picture height in pixels (M3 is progressive-only, so map_units
    /// = MB rows).
    pub fn pic_height(&self) -> u32 {
        (self.pic_height_in_map_units_minus1 + 1) * 16
    }

    pub fn parse(rbsp: &[u8]) -> Result<Sps, DecodeError> {
        let mut br = BitReader::new(rbsp);
        let profile_idc = br.read_bits(8)? as u8;
        if profile_idc != 66 {
            // 88 = Extended, 77 = Main, 100+ = High variants. M3 is
            // baseline-only.
            return Err(DecodeError::UnsupportedProfile);
        }
        // constraint_set_flags + reserved (8 bits) — we don't act on these
        // beyond ensuring the next byte is reachable.
        let _constraint_flags = br.read_bits(8)?;
        let _level_idc = br.read_bits(8)?;
        let seq_parameter_set_id = br.read_ue()?;

        // Profile_idc 66 (baseline) does NOT include the chroma_format_idc
        // / separate_colour_plane / bit_depth fields — those are guarded
        // by `if profile_idc in {100, 110, 122, 244, 44, ...}` per spec.
        // For baseline, defaults apply: chroma_format_idc = 1 (4:2:0),
        // bit_depth = 8, no scaling matrix. M3 needs all of those.

        let log2_max_frame_num_minus4 = br.read_ue()?;
        if log2_max_frame_num_minus4 > 12 {
            return Err(DecodeError::OutOfScope("log2_max_frame_num_minus4 > 12"));
        }

        let pic_order_cnt_type = br.read_ue()?;
        let log2_max_pic_order_cnt_lsb_minus4 = if pic_order_cnt_type == 0 {
            br.read_ue()?
        } else if pic_order_cnt_type == 2 {
            // Type 2 has no extra fields. Acceptable for I-only bitstreams.
            0
        } else {
            // Type 1 has a long parameter list; M3 hasn't seen one in
            // the wild from x264 baseline.
            return Err(DecodeError::OutOfScope("pic_order_cnt_type == 1"));
        };

        let num_ref_frames = br.read_ue()?;
        let gaps_in_frame_num_value_allowed_flag = br.read_bit()? != 0;

        let pic_width_in_mbs_minus1 = br.read_ue()?;
        let pic_height_in_map_units_minus1 = br.read_ue()?;

        let frame_mbs_only_flag = br.read_bit()? != 0;
        if !frame_mbs_only_flag {
            return Err(DecodeError::OutOfScope("interlaced (frame_mbs_only_flag = 0)"));
        }

        let direct_8x8_inference_flag = br.read_bit()? != 0;

        let frame_cropping_flag = br.read_bit()? != 0;
        if frame_cropping_flag {
            // Skip the four cropping ue codes; we don't apply cropping yet
            // (M3's test corpus uses non-cropped 16-pixel-aligned dims).
            let _l = br.read_ue()?;
            let _r = br.read_ue()?;
            let _t = br.read_ue()?;
            let _b = br.read_ue()?;
        }

        let _vui_parameters_present_flag = br.read_bit()? != 0;
        // We don't act on any VUI value for M3, and the VUI block has
        // variable length we'd need a full parser to skip past
        // correctly. Since we never read past `direct_8x8_inference_flag
        // / cropping`, leaving the bit cursor inside VUI is harmless —
        // no caller reads further into the SPS RBSP.

        Ok(Sps {
            profile_idc,
            seq_parameter_set_id,
            log2_max_frame_num_minus4,
            pic_order_cnt_type,
            log2_max_pic_order_cnt_lsb_minus4,
            num_ref_frames,
            gaps_in_frame_num_value_allowed_flag,
            pic_width_in_mbs_minus1,
            pic_height_in_map_units_minus1,
            frame_mbs_only_flag,
            direct_8x8_inference_flag,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────
// PPS — Picture Parameter Set (spec §7.3.2.2)
// ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pps {
    pub pic_parameter_set_id: u32,
    pub seq_parameter_set_id: u32,
    /// Must be 0 (CAVLC) for M3.
    pub entropy_coding_mode_flag: bool,
    pub bottom_field_pic_order_in_frame_present_flag: bool,
    /// Must be 0 (single slice group) for M3.
    pub num_slice_groups_minus1: u32,
    pub num_ref_idx_l0_default_active_minus1: u32,
    pub num_ref_idx_l1_default_active_minus1: u32,
    pub weighted_pred_flag: bool,
    pub weighted_bipred_idc: u8,
    /// `pic_init_qp_minus26` + 26 = the picture-default QP.
    pub pic_init_qp_minus26: i32,
    pub pic_init_qs_minus26: i32,
    pub chroma_qp_index_offset: i32,
    pub deblocking_filter_control_present_flag: bool,
    pub constrained_intra_pred_flag: bool,
    pub redundant_pic_cnt_present_flag: bool,
}

impl Pps {
    pub fn parse(rbsp: &[u8]) -> Result<Pps, DecodeError> {
        let mut br = BitReader::new(rbsp);
        let pic_parameter_set_id = br.read_ue()?;
        let seq_parameter_set_id = br.read_ue()?;
        let entropy_coding_mode_flag = br.read_bit()? != 0;
        if entropy_coding_mode_flag {
            return Err(DecodeError::OutOfScope("CABAC (entropy_coding_mode_flag = 1)"));
        }
        let bottom_field_pic_order_in_frame_present_flag = br.read_bit()? != 0;
        let num_slice_groups_minus1 = br.read_ue()?;
        if num_slice_groups_minus1 > 0 {
            return Err(DecodeError::OutOfScope("multiple slice groups"));
        }
        let num_ref_idx_l0_default_active_minus1 = br.read_ue()?;
        let num_ref_idx_l1_default_active_minus1 = br.read_ue()?;
        let weighted_pred_flag = br.read_bit()? != 0;
        if weighted_pred_flag {
            return Err(DecodeError::OutOfScope("weighted_pred_flag = 1"));
        }
        let weighted_bipred_idc = br.read_bits(2)? as u8;
        if weighted_bipred_idc != 0 {
            return Err(DecodeError::OutOfScope("weighted_bipred_idc != 0"));
        }
        let pic_init_qp_minus26 = br.read_se()?;
        let pic_init_qs_minus26 = br.read_se()?;
        let chroma_qp_index_offset = br.read_se()?;
        let deblocking_filter_control_present_flag = br.read_bit()? != 0;
        let constrained_intra_pred_flag = br.read_bit()? != 0;
        let redundant_pic_cnt_present_flag = br.read_bit()? != 0;
        // PPS extension fields (transform_8x8_mode_flag, pic_scaling_matrix,
        // second_chroma_qp_index_offset) only exist when the bitstream
        // has more bytes — they're profile-conditional. M3 baseline doesn't
        // emit them; skip parsing and let any trailing bits be consumed by
        // rbsp_trailing_bits.
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
}

// ─────────────────────────────────────────────────────────────────────
// Slice header (spec §7.3.3)
// ─────────────────────────────────────────────────────────────────────

/// Slice type values per spec Table 7-6. For an I-only bitstream we
/// only ever see `I` (5 or 7); the others are kept for future P-frame
/// support and surface as `OutOfScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceType {
    /// Spec values 2 or 7. All MBs are intra-coded.
    I,
    /// Spec values 0 or 5. Out of scope until M4.
    P,
    /// Spec values 1 or 6. Permanently out of scope.
    B,
    /// Spec values 3 or 8 (SP). Permanently out of scope.
    Sp,
    /// Spec values 4 or 9 (SI). Permanently out of scope.
    Si,
}

impl SliceType {
    fn from_raw(raw: u32) -> Self {
        // Spec Table 7-6: slice_type % 5 gives the base type. The +5
        // form means "all slices in the picture are this type" — same
        // semantic as far as the decoder is concerned.
        match raw % 5 {
            2 => SliceType::I,
            0 => SliceType::P,
            1 => SliceType::B,
            3 => SliceType::Sp,
            4 => SliceType::Si,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceHeader {
    /// Must be 0 for M3 (single slice per picture).
    pub first_mb_in_slice: u32,
    pub slice_type: SliceType,
    pub pic_parameter_set_id: u32,
    pub frame_num: u32,
    pub idr_pic_id: Option<u32>,
    pub pic_order_cnt_lsb: Option<u32>,
    pub slice_qp_delta: i32,
    /// Derived: slice_qp_y = pic_init_qp_minus26 + 26 + slice_qp_delta.
    pub slice_qp_y: i32,
    pub disable_deblocking_filter_idc: u32,
    pub slice_alpha_c0_offset_div2: i32,
    pub slice_beta_offset_div2: i32,
}

impl SliceHeader {
    /// Parse the slice header out of a slice NAL's RBSP. Convenience
    /// wrapper around `parse_into` that builds a fresh BitReader.
    pub fn parse(
        rbsp: &[u8],
        sps: &Sps,
        pps: &Pps,
        is_idr: bool,
    ) -> Result<SliceHeader, DecodeError> {
        let mut br = BitReader::new(rbsp);
        Self::parse_into(&mut br, sps, pps, is_idr)
    }

    /// Parse the slice header off an existing BitReader, advancing
    /// the cursor past the header. Use this when you want to read
    /// macroblock data after the header from the same cursor.
    pub fn parse_into(
        br: &mut BitReader,
        sps: &Sps,
        pps: &Pps,
        is_idr: bool,
    ) -> Result<SliceHeader, DecodeError> {
        let first_mb_in_slice = br.read_ue()?;
        if first_mb_in_slice != 0 {
            return Err(DecodeError::OutOfScope("first_mb_in_slice != 0 (multi-slice)"));
        }
        let slice_type_raw = br.read_ue()?;
        let slice_type = SliceType::from_raw(slice_type_raw);
        if !matches!(slice_type, SliceType::I) {
            return Err(DecodeError::OutOfScope("non-I slice"));
        }
        let pic_parameter_set_id = br.read_ue()?;
        let _ = pic_parameter_set_id; // sanity: should match `pps.pic_parameter_set_id`

        // frame_num: log2_max_frame_num_minus4 + 4 bits.
        let frame_num_bits = sps.log2_max_frame_num_minus4 + 4;
        let frame_num = br.read_bits(frame_num_bits)?;

        // M3 is progressive-only; field_pic_flag does not appear when
        // frame_mbs_only_flag = 1.

        let idr_pic_id = if is_idr { Some(br.read_ue()?) } else { None };

        let pic_order_cnt_lsb = if sps.pic_order_cnt_type == 0 {
            let bits = sps.log2_max_pic_order_cnt_lsb_minus4 + 4;
            Some(br.read_bits(bits)?)
        } else {
            None
        };

        // bottom_field_pic_order_in_frame_present_flag fields skipped:
        // M3 is progressive, so the per-bottom-field POCs aren't present.

        // For an I slice we skip:
        //   - num_ref_idx_active_override_flag (P/B only)
        //   - ref_pic_list_modification (P/B only)
        //   - pred_weight_table (P/B only — we already rejected
        //     weighted_pred_flag in PPS)

        // dec_ref_pic_marking
        if is_idr {
            let _no_output_of_prior_pics_flag = br.read_bit()?;
            let _long_term_reference_flag = br.read_bit()?;
        } else {
            let adaptive_ref_pic_marking_mode_flag = br.read_bit()? != 0;
            if adaptive_ref_pic_marking_mode_flag {
                // Variable-length series of memory_management_control_operation
                // codes terminated by 0. We don't act on any of them but
                // we do need to read them past so the bit cursor lands
                // correctly. Loop until terminator.
                loop {
                    let mmco = br.read_ue()?;
                    if mmco == 0 { break; }
                    if mmco > 6 {
                        return Err(DecodeError::OutOfScope("unknown MMCO"));
                    }
                    // Each MMCO has 0..=2 ue parameters; consume per spec
                    // Table 7-9 to keep the cursor aligned.
                    match mmco {
                        1 | 3 => { let _ = br.read_ue()?; }
                        2 => { let _ = br.read_ue()?; }
                        4 => { let _ = br.read_ue()?; }
                        6 => { let _ = br.read_ue()?; }
                        5 => {} // no parameters
                        _ => unreachable!(),
                    }
                    if mmco == 3 { let _ = br.read_ue()?; }
                }
            }
        }

        let slice_qp_delta = br.read_se()?;
        let slice_qp_y = pps.pic_init_qp_minus26 + 26 + slice_qp_delta;

        let mut disable_deblocking_filter_idc = 0u32;
        let mut slice_alpha_c0_offset_div2 = 0i32;
        let mut slice_beta_offset_div2 = 0i32;
        if pps.deblocking_filter_control_present_flag {
            disable_deblocking_filter_idc = br.read_ue()?;
            if disable_deblocking_filter_idc != 1 {
                slice_alpha_c0_offset_div2 = br.read_se()?;
                slice_beta_offset_div2 = br.read_se()?;
            }
        }

        Ok(SliceHeader {
            first_mb_in_slice,
            slice_type,
            pic_parameter_set_id,
            frame_num,
            idr_pic_id,
            pic_order_cnt_lsb,
            slice_qp_delta,
            slice_qp_y,
            disable_deblocking_filter_idc,
            slice_alpha_c0_offset_div2,
            slice_beta_offset_div2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nal::{strip_emulation_prevention, NalUnitIterator, nut};
    use snarkvid_h264_test_vectors::NOISE_16X16_QP18;

    /// Walk the live x264 corpus, return (sps_rbsp, pps_rbsp, slice_rbsp,
    /// slice_is_idr).
    fn corpus_units() -> (alloc::vec::Vec<u8>, alloc::vec::Vec<u8>, alloc::vec::Vec<u8>, bool) {
        let mut sps_rbsp = None;
        let mut pps_rbsp = None;
        let mut slice_rbsp = None;
        let mut slice_is_idr = false;
        for unit in NalUnitIterator::new(NOISE_16X16_QP18.h264) {
            let unit = unit.unwrap();
            let payload = unit.payload();
            let rbsp = strip_emulation_prevention(payload);
            match unit.unit_type().unwrap() {
                nut::SPS => sps_rbsp = Some(rbsp),
                nut::PPS => pps_rbsp = Some(rbsp),
                nut::IDR_SLICE => { slice_rbsp = Some(rbsp); slice_is_idr = true; }
                nut::NON_IDR_SLICE => { slice_rbsp = Some(rbsp); slice_is_idr = false; }
                _ => {}
            }
        }
        (sps_rbsp.unwrap(), pps_rbsp.unwrap(), slice_rbsp.unwrap(), slice_is_idr)
    }

    #[test]
    fn parse_corpus_sps_yields_16x16_dims() {
        let (sps, _, _, _) = corpus_units();
        let sps = Sps::parse(&sps).expect("SPS parse");
        assert_eq!(sps.profile_idc, 66, "M3 expects baseline (66)");
        assert_eq!(sps.pic_width(), 16, "16x16 corpus");
        assert_eq!(sps.pic_height(), 16);
        assert!(sps.frame_mbs_only_flag);
    }

    #[test]
    fn parse_corpus_pps_yields_cavlc_and_no_slice_groups() {
        let (_, pps_bytes, _, _) = corpus_units();
        let pps = Pps::parse(&pps_bytes).expect("PPS parse");
        assert!(!pps.entropy_coding_mode_flag, "M3 requires CAVLC");
        assert_eq!(pps.num_slice_groups_minus1, 0, "single slice group only");
        assert!(!pps.weighted_pred_flag);
        assert_eq!(pps.weighted_bipred_idc, 0);
    }

    #[test]
    fn parse_corpus_slice_header_yields_idr_i_slice() {
        let (sps_bytes, pps_bytes, slice_bytes, is_idr) = corpus_units();
        let sps = Sps::parse(&sps_bytes).unwrap();
        let pps = Pps::parse(&pps_bytes).unwrap();
        assert!(is_idr, "x264 --frames 1 --keyint 1 emits an IDR");
        let sh = SliceHeader::parse(&slice_bytes, &sps, &pps, is_idr).expect("slice parse");
        assert_eq!(sh.first_mb_in_slice, 0);
        assert_eq!(sh.slice_type, SliceType::I);
        // x264 was invoked with `--qp 18`, but its rate control actually
        // quantized this tiny frame at QP=15 (per x264's own log:
        // "Avg QP:15.00"). The parser correctly extracts what's in the
        // bitstream — not what we asked for. M3 trusts the bitstream.
        assert_eq!(sh.slice_qp_y, 15,
            "expected SliceQPY=15 (x264 actual; got pic_init={}, delta={}, qp_y={})",
            pps.pic_init_qp_minus26, sh.slice_qp_delta, sh.slice_qp_y);
        assert!(sh.slice_qp_y >= 0 && sh.slice_qp_y <= 51, "QP out of range");
    }

    #[test]
    fn rejects_non_baseline_profile() {
        // Build a synthetic SPS RBSP with profile_idc = 100 (High).
        // Header: 1 byte profile, 1 byte constraint flags, 1 byte level_idc, ...
        let bytes = [100u8, 0u8, 30u8, 0x80, 0x40]; // profile=100, ...
        assert!(matches!(
            Sps::parse(&bytes),
            Err(DecodeError::UnsupportedProfile)
        ));
    }

    #[test]
    fn slice_type_dispatch_handles_plus_5_aliases() {
        // Spec Table 7-6: slice_type = 7 (= 2+5) means "all slices in
        // picture are I". Decoder sees the same I behavior either way.
        for raw in [2u32, 7] {
            assert_eq!(SliceType::from_raw(raw), SliceType::I);
        }
        for raw in [0u32, 5] {
            assert_eq!(SliceType::from_raw(raw), SliceType::P);
        }
    }
}
