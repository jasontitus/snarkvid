//! Parse SPS + PPS from every corpus fixture and confirm the dimensions
//! and profile match what `gen.sh` recorded in the `.meta` file.

use std::fs;
use std::path::PathBuf;

use snarkvid_h264_decoder::{
    iter_nalus, nal_unit_type, parse_pps, parse_slice_header, parse_sps, profile, SliceType,
};

fn corpus_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../test-vectors");
    p
}

fn read_meta(name: &str) -> (u32, u32, u32) {
    let mut p = corpus_dir();
    p.push(format!("{name}.meta"));
    let s = fs::read_to_string(p).unwrap();
    let parts: Vec<&str> = s.split_whitespace().collect();
    (
        parts[0].parse().unwrap(),
        parts[1].parse().unwrap(),
        parts[2].parse().unwrap(),
    )
}

#[test]
fn each_fixture_sps_pps_matches_meta() {
    for name in ["solid_16x16", "grad_32x32", "smooth_64x64", "checker_32x16", "diag_16x16"] {
        let (w, h, _frames) = read_meta(name);
        let mut p = corpus_dir();
        p.push(format!("{name}.h264"));
        let bytes = fs::read(&p).unwrap();
        let nalus = iter_nalus(&bytes).unwrap();

        let sps_n = nalus
            .iter()
            .find(|n| n.header.nal_unit_type == nal_unit_type::SPS)
            .expect(&format!("{name}: no SPS"));
        let pps_n = nalus
            .iter()
            .find(|n| n.header.nal_unit_type == nal_unit_type::PPS)
            .expect(&format!("{name}: no PPS"));

        let sps = parse_sps(&sps_n.rbsp).unwrap();
        let pps = parse_pps(&pps_n.rbsp).unwrap();

        // Profile is baseline (66) for all fixtures by construction.
        assert_eq!(sps.profile_idc, profile::BASELINE, "{name}: profile_idc");

        // Dimensions match the .meta file.
        assert_eq!(sps.pic_width(), w, "{name}: width");
        assert_eq!(sps.pic_height(), h, "{name}: height");

        // CAVLC required.
        assert!(!pps.entropy_coding_mode_flag, "{name}: PPS must be CAVLC");
        // Single slice group.
        assert_eq!(pps.num_slice_groups_minus1, 0, "{name}: slice groups");
        // Frame-only.
        assert!(sps.frame_mbs_only_flag, "{name}: frame_mbs_only_flag");

        // Now also parse the IDR slice header for each fixture.
        let idr_n = nalus
            .iter()
            .find(|n| n.header.nal_unit_type == nal_unit_type::IDR_SLICE)
            .expect(&format!("{name}: no IDR"));
        let (sh, _r) = parse_slice_header(&idr_n.rbsp, &idr_n.header, &sps, &pps).unwrap();
        assert_eq!(sh.first_mb_in_slice, 0, "{name}: first_mb_in_slice");
        assert_eq!(sh.slice_type, SliceType::I, "{name}: slice_type");
        // Initial QP should be in the legal range 0..=51.
        let qp = sh.slice_qp(&pps);
        assert!(
            (0..=51).contains(&qp),
            "{name}: slice_qp out of range: {qp}"
        );
    }
}
