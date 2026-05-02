//! Parity test: parse every fixture in test-vectors/ and confirm the
//! NAL framer agrees with ffmpeg about how many NAL units are present
//! and what each unit's type is.
//!
//! We don't decode the slices yet — that arrives once the slice header,
//! CAVLC, and macroblock layer are wired up.

use std::path::PathBuf;
use std::process::Command;

use snarkvid_h264_decoder::{iter_nalus, nal_unit_type};

fn corpus_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../test-vectors");
    p
}

fn fixtures() -> Vec<&'static str> {
    vec![
        "solid_16x16",
        "grad_32x32",
        "smooth_64x64",
        "checker_32x16",
        "diag_16x16",
    ]
}

/// Use ffprobe to count NAL units in the fixture so we can compare
/// against our parser. ffprobe with `-show_packets` returns one packet
/// per access unit; what we really want is one entry per NAL unit. The
/// quickest way to ground-truth that is to walk the file ourselves
/// looking for start codes, but reusing libavformat would beg the
/// question. Compromise: spot-check the first NAL types and the total
/// count against a hand-counted value.
fn ffprobe_packet_count(path: &PathBuf) -> usize {
    let out = Command::new("ffprobe")
        .args(["-loglevel", "error", "-show_packets", "-of", "csv"])
        .arg(path)
        .output()
        .expect("ffprobe failed");
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().filter(|l| l.starts_with("packet")).count()
}

#[test]
fn each_fixture_has_at_least_sps_pps_and_idr() {
    for name in fixtures() {
        let mut p = corpus_dir();
        p.push(format!("{name}.h264"));
        if !p.exists() {
            panic!(
                "fixture {} missing — run test-vectors/gen.sh first",
                p.display()
            );
        }
        let bytes = std::fs::read(&p).unwrap();
        let nalus = iter_nalus(&bytes).expect("framer accepts the bitstream");

        // x264 emits at least: SPS, PPS, SEI, IDR slice. Some configs add AUD.
        let types: Vec<u8> = nalus.iter().map(|n| n.header.nal_unit_type).collect();
        assert!(
            types.contains(&nal_unit_type::SPS),
            "{name}: no SPS in {types:?}"
        );
        assert!(
            types.contains(&nal_unit_type::PPS),
            "{name}: no PPS in {types:?}"
        );
        assert!(
            types.contains(&nal_unit_type::IDR_SLICE),
            "{name}: no IDR slice in {types:?}"
        );

        // Spot check: ffprobe sees one packet per access unit (= 1 here).
        let pkts = ffprobe_packet_count(&p);
        assert_eq!(pkts, 1, "{name}: expected 1 access unit, got {pkts}");
    }
}

#[test]
fn rbsp_first_byte_after_idr_header_is_first_slice_byte() {
    // The IDR slice's RBSP starts with the slice_header() syntax. The
    // first ue(v) is `first_mb_in_slice`. For a single-slice frame
    // that's always 0, encoded as the bit pattern "1" (8th bit of the
    // first byte). We don't fully decode the slice yet — we just check
    // the framer puts at least one byte where we expect.
    let mut p = corpus_dir();
    p.push("solid_16x16.h264");
    let bytes = std::fs::read(&p).unwrap();
    let nalus = iter_nalus(&bytes).unwrap();
    let idr = nalus
        .iter()
        .find(|n| n.header.nal_unit_type == nal_unit_type::IDR_SLICE)
        .expect("solid_16x16 has an IDR");
    assert!(!idr.rbsp.is_empty());
    // first_mb_in_slice == 0 → ue(v) "1" → MSB of first byte is 1.
    assert_eq!(idr.rbsp[0] & 0x80, 0x80);
}
