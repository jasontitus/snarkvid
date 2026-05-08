//! M2 native pipeline integration test.
//!
//! Builds end-to-end fixtures (signed manifest + Merkle tree over an
//! original frame + encoded bitstream + per-block paths) and feeds
//! them to `snarkvid_m2_statement::verify_m2_claim` — the same
//! function the SP1 guest calls in-circuit. Tests cover:
//!
//!   §3.4.1  flip a byte in `compressed.bin`        → fails closed
//!   §3.4.2  manifest signed by an unknown key      → fails closed
//!   §3.4.4  lower tolerance below the actual PSNR  → fails closed
//!
//! §3.4.3 ("substitute a different image as the witness, prover
//! cannot produce a valid proof") requires the actual prover and is
//! deferred to the M2 prover binary.
//!
//! The point of this test, separate from `crates/m2-statement`'s own
//! unit tests, is to exercise the *fixture builder* — the host-side
//! code that constructs all the inputs the guest will see. The guest
//! never builds the fixture; the host does.

use ed25519_dalek::SigningKey;
use snarkvid_comparator::PSNR_SCALE;
use snarkvid_m2_statement::{frame_merkle_leaves, verify_m2_claim, ClaimError};
use snarkvid_manifest::{
    merkle_path, merkle_root, sign_manifest, DeviceId, ManifestBody, MerklePath,
    SignedManifest, VideoDescriptor,
};
use snarkvid_toy_codec::{encode_toy, BqBitstream, YuvFrame};

fn make_frame() -> YuvFrame {
    fn xs(mut s: u32) -> u32 {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    }
    let w = 32usize;
    let h = 32usize;
    let cw = w / 2;
    let ch = h / 2;
    let mut state = 0xfeedbeefu32;
    let mut next = || {
        state = xs(state);
        (state & 0xff) as u8
    };
    let y: Vec<u8> = (0..w * h).map(|_| next()).collect();
    let u: Vec<u8> = (0..cw * ch).map(|_| next()).collect();
    let v: Vec<u8> = (0..cw * ch).map(|_| next()).collect();
    YuvFrame { width: w as u16, height: h as u16, y, u, v }
}

fn make_manifest_body(merkle_root: [u8; 32]) -> ManifestBody {
    ManifestBody {
        version: 1,
        video: VideoDescriptor {
            width: 32, height: 32,
            fps_num: 30, fps_den: 1, frame_count: 1,
            merkle_root,
        },
        audio: None,
        created_at: 1_715_000_000,
        device_id: DeviceId(*b"m2-pipeline-test-device-padding-padding-padding-padding-pad-0001"),
    }
}

const TOLERANCE_36DB: i64 = 36 * PSNR_SCALE;
const TOLERANCE_60DB: i64 = 60 * PSNR_SCALE;

/// Canonical fixture builder. Real prover host will call something
/// like this to assemble its inputs.
fn build_fixture(qp: u8, signing_key: &SigningKey) -> (
    SignedManifest, BqBitstream, YuvFrame, Vec<MerklePath>,
) {
    let original = make_frame();
    let leaves = frame_merkle_leaves(&original);
    let root = merkle_root(&leaves);
    let body = make_manifest_body(root);
    let signed = sign_manifest(body, signing_key);
    let bitstream = encode_toy(&original, qp).expect("encode_toy");
    let paths: Vec<MerklePath> = (0..leaves.len())
        .map(|i| merkle_path(&leaves, i).expect("path"))
        .collect();
    (signed, bitstream, original, paths)
}

#[test]
fn happy_path_qp0_passes_high_threshold() {
    let key = SigningKey::from_bytes(&[1u8; 32]);
    let (signed, bs, original, paths) = build_fixture(0, &key);
    let r = verify_m2_claim(&signed, &bs, &original, &paths, TOLERANCE_60DB).unwrap();
    assert_eq!(r.psnr_y_scaled, i64::MAX); // lossless
}

#[test]
fn happy_path_qp8_passes_36db_threshold() {
    let key = SigningKey::from_bytes(&[2u8; 32]);
    let (signed, bs, original, paths) = build_fixture(8, &key);
    verify_m2_claim(&signed, &bs, &original, &paths, TOLERANCE_36DB)
        .expect("qp=8 should clear 36 dB");
}

#[test]
fn tamper_compressed_byte_fails_closed() {
    // §3.4.1
    let key = SigningKey::from_bytes(&[3u8; 32]);
    let (signed, mut bs, original, paths) = build_fixture(0, &key);
    bs.coeffs_y[100] ^= 0x7f;
    let err = verify_m2_claim(&signed, &bs, &original, &paths, TOLERANCE_60DB).unwrap_err();
    assert!(matches!(err, ClaimError::PsnrBelowTolerance(_)));
}

#[test]
fn tamper_manifest_unknown_key_fails_closed() {
    // §3.4.2
    let known = SigningKey::from_bytes(&[4u8; 32]);
    let attacker = SigningKey::from_bytes(&[0xa5u8; 32]);
    let (mut signed, bs, original, paths) = build_fixture(0, &known);
    signed.signature = sign_manifest(signed.body.clone(), &attacker).signature;
    assert_eq!(
        verify_m2_claim(&signed, &bs, &original, &paths, TOLERANCE_60DB),
        Err(ClaimError::ManifestSignatureInvalid),
    );
}

#[test]
fn tamper_lower_tolerance_below_actual_psnr_fails_closed() {
    // §3.4.4
    let key = SigningKey::from_bytes(&[5u8; 32]);
    let (signed, bs, original, paths) = build_fixture(32, &key);
    let err = verify_m2_claim(&signed, &bs, &original, &paths, TOLERANCE_60DB).unwrap_err();
    assert!(matches!(err, ClaimError::PsnrBelowTolerance(_)));
}

#[test]
fn tamper_merkle_path_fails_closed() {
    let key = SigningKey::from_bytes(&[6u8; 32]);
    let (signed, bs, original, mut paths) = build_fixture(0, &key);
    paths[0].siblings[0][0] ^= 0xff;
    let err = verify_m2_claim(&signed, &bs, &original, &paths, TOLERANCE_60DB).unwrap_err();
    assert!(matches!(err, ClaimError::MerklePathInvalid { block_index: 0 }));
}

#[test]
fn merkle_path_count_mismatch_fails_closed() {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let (signed, bs, original, mut paths) = build_fixture(0, &key);
    paths.pop();
    assert_eq!(
        verify_m2_claim(&signed, &bs, &original, &paths, TOLERANCE_60DB),
        Err(ClaimError::MerklePathCountMismatch),
    );
}
