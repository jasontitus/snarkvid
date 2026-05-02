//! Integration test for the full milestone-2 derivation proof.
//!
//! Builds a synthetic original image, signs a manifest, encodes with
//! the toy codec, then checks the milestone-2 statement passes for the
//! happy path and fails closed for each tampering scenario.

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use snarkvid_m2_statement::{check, frame_leaf_bytes};
use snarkvid_manifest::{merkle_proof, merkle_root, ManifestBody, SignedManifest, VideoMeta};
use snarkvid_toy_codec::{encode, YuvFrame};

const W: u32 = 64;
const H: u32 = 32;

fn synthetic_frame() -> YuvFrame {
    let w = W as usize;
    let h = H as usize;
    let cw = w / 2;
    let ch = h / 2;
    // Smooth gradient in luma, constant chroma. The toy codec round-
    // trips this losslessly at QP=1, so the PSNR check passes
    // comfortably regardless of tolerance choice.
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = ((r * 4) as u8).wrapping_add((c * 2) as u8);
        }
    }
    YuvFrame {
        width: W,
        height: H,
        y,
        u: vec![128u8; cw * ch],
        v: vec![128u8; cw * ch],
    }
}

fn build_manifest_and_path(
    frame: &YuvFrame,
    key: &SigningKey,
) -> (SignedManifest, snarkvid_manifest::MerklePath) {
    let leaf = frame_leaf_bytes(frame);
    let leaves: &[&[u8]] = &[&leaf];
    let root = merkle_root(leaves);
    let path = merkle_proof(leaves, 0).unwrap();
    let body = ManifestBody {
        version: 1,
        created_at: 1700000000,
        device_id: "test-cam".into(),
        video: VideoMeta {
            width: frame.width,
            height: frame.height,
            frame_count: 1,
            fps_num: 30,
            fps_den: 1,
            merkle_root: root,
        },
        audio: None,
    };
    (SignedManifest::sign(body, key).unwrap(), path)
}

#[test]
fn happy_path_qp1_lossless() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let frame = synthetic_frame();
    let (manifest, path) = build_manifest_and_path(&frame, &key);
    let compressed = encode(&frame, 1).unwrap();

    check(
        &compressed,
        &manifest,
        &key.verifying_key().to_bytes(),
        50,
        &frame,
        &path,
    )
    .unwrap();
}

#[test]
fn happy_path_qp4_high_psnr() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let frame = synthetic_frame();
    let (manifest, path) = build_manifest_and_path(&frame, &key);
    let compressed = encode(&frame, 4).unwrap();

    // 36 dB is the design-doc default for "visually transparent."
    check(
        &compressed,
        &manifest,
        &key.verifying_key().to_bytes(),
        36,
        &frame,
        &path,
    )
    .unwrap();
}

#[test]
fn tampered_compressed_fails() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let frame = synthetic_frame();
    let (manifest, path) = build_manifest_and_path(&frame, &key);
    let mut compressed = encode(&frame, 1).unwrap();

    // Tamper the QP byte. Now every coefficient is dequantized with
    // the wrong scale, so the reconstructed frame is wildly off.
    let qp_offset = 12; // see toy-codec header layout
    compressed[qp_offset] = 32;

    let r = check(
        &compressed,
        &manifest,
        &key.verifying_key().to_bytes(),
        36,
        &frame,
        &path,
    );
    assert!(r.is_err(), "tampered compressed should fail: got {:?}", r);
}

#[test]
fn untrusted_signer_fails() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let other_key = SigningKey::generate(&mut rng);
    let frame = synthetic_frame();
    let (manifest, path) = build_manifest_and_path(&frame, &key);
    let compressed = encode(&frame, 1).unwrap();

    let r = check(
        &compressed,
        &manifest,
        &other_key.verifying_key().to_bytes(), // wrong expected pubkey
        50,
        &frame,
        &path,
    );
    assert!(matches!(
        r,
        Err(snarkvid_m2_statement::StatementError::BadSignature)
    ));
}

#[test]
fn substituted_witness_fails() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    let frame = synthetic_frame();
    let (manifest, path) = build_manifest_and_path(&frame, &key);
    let compressed = encode(&frame, 1).unwrap();

    // Build a *different* original and try to pass it off as the witness.
    let mut other = frame.clone();
    other.y[0] ^= 1;

    let r = check(
        &compressed,
        &manifest,
        &key.verifying_key().to_bytes(),
        50,
        &other,
        &path,
    );
    assert!(r.is_err(), "substituted witness should fail Merkle check or PSNR");
}

#[test]
fn over_strict_tolerance_fails_at_high_qp() {
    let mut rng = OsRng;
    let key = SigningKey::generate(&mut rng);
    // PRNG-style high-entropy content + max QP guarantees real distortion.
    let mut frame = synthetic_frame();
    let w = W as usize;
    for r in 0..H as usize {
        for c in 0..w {
            // LCG-like deterministic noise across [0, 255].
            let v = ((r.wrapping_mul(73)).wrapping_add(c.wrapping_mul(151))) as u32;
            frame.y[r * w + c] = ((v ^ (v >> 5)) & 0xFF) as u8;
        }
    }
    let (manifest, path) = build_manifest_and_path(&frame, &key);
    let compressed = encode(&frame, 64).unwrap();

    // Demand 50 dB. With QP=64 and noise content, actual PSNR will be
    // far below this; the comparator must reject.
    let strict = check(
        &compressed,
        &manifest,
        &key.verifying_key().to_bytes(),
        50,
        &frame,
        &path,
    );
    assert!(
        matches!(
            strict,
            Err(snarkvid_m2_statement::StatementError::PsnrBelowFloor { .. })
        ),
        "expected PsnrBelowFloor, got {:?}",
        strict
    );
}
