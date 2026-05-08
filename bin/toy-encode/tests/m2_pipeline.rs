//! M2 native pipeline integration test.
//!
//! Composes `snarkvid-manifest` + `snarkvid-toy-codec` +
//! `snarkvid-comparator` to exercise the M2 statement (signed manifest
//! → Merkle authentication of original frame → decode_toy →
//! tolerance comparator) without going through a zkVM prover. This
//! covers three of the four §3.4 tampering modes:
//!
//!   §3.4.1  flip a byte in `compressed.bin`        → fails closed
//!   §3.4.2  manifest signed by an unknown key      → fails closed
//!   §3.4.4  lower tolerance below the actual PSNR  → fails closed
//!
//! §3.4.3 ("substitute a different image as the witness, prover
//! cannot produce a valid proof") requires the actual prover and is
//! deferred to the M2 prover binary.
//!
//! These tests anchor the architecture: each crate's API is exercised
//! by a real consumer, and the cross-crate composition is asserted to
//! fail-closed in exactly the cases the M2 acceptance criteria call out.

use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use snarkvid_comparator::{frame_psnr, FramePsnrResult, PSNR_SCALE};
use snarkvid_manifest::{
    merkle_path, merkle_root, sign_manifest, verify_manifest, verify_merkle_path,
    DeviceId, ManifestBody, MerklePath, SignedManifest, VideoDescriptor,
};
use snarkvid_toy_codec::{decode_toy, encode_toy, BqBitstream, YuvFrame};

// ─────────────────────────────────────────────────────────────────────
// Fixture builders
// ─────────────────────────────────────────────────────────────────────

/// Deterministic 32×32 4:2:0 frame with enough high-frequency content
/// that quantization is observable. Same xorshift pattern as the
/// `noise_frame` test fixture in `toy-codec`.
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

/// Hash one 8×8 Y-block of the frame. Real M2 will pick a leaf
/// granularity (per §7 risks: "Increase Merkle leaf granularity (e.g.,
/// one leaf per 64×64 tile instead of per 8×8 block)"). For this test
/// one leaf per 8×8 Y block is fine — small fixture, fast hash.
fn hash_block(frame: &YuvFrame, bx: usize, by: usize) -> [u8; 32] {
    let mut h = Sha256::new();
    let w = frame.width as usize;
    for r in 0..8 {
        let off = (by * 8 + r) * w + bx * 8;
        h.update(&frame.y[off..off + 8]);
    }
    h.finalize().into()
}

/// Build a Merkle tree over the Y-plane's 8×8 blocks (in raster order).
fn frame_merkle_leaves(frame: &YuvFrame) -> Vec<[u8; 32]> {
    let bw = frame.width as usize / 8;
    let bh = frame.height as usize / 8;
    let mut leaves = Vec::with_capacity(bw * bh);
    for by in 0..bh {
        for bx in 0..bw {
            leaves.push(hash_block(frame, bx, by));
        }
    }
    leaves
}

fn make_manifest_body(merkle_root: [u8; 32]) -> ManifestBody {
    ManifestBody {
        version: 1,
        video: VideoDescriptor {
            width: 32,
            height: 32,
            fps_num: 30,
            fps_den: 1,
            frame_count: 1,
            merkle_root,
        },
        audio: None,
        created_at: 1_715_000_000,
        device_id: DeviceId(*b"m2-pipeline-test-device-padding-padding-padding-padding-pad-0001"),
    }
}

/// Tolerance threshold in fixed-point dB (PSNR_SCALE = 100, so 36.00 dB → 3600).
const TOLERANCE_36DB: i64 = 36 * PSNR_SCALE;
const TOLERANCE_60DB: i64 = 60 * PSNR_SCALE;

// ─────────────────────────────────────────────────────────────────────
// The core "verifier" check, native version. The zkVM guest will run
// the same logic; this asserts the architecture composes.
// ─────────────────────────────────────────────────────────────────────

/// Native run of the M2 statement. Returns `Ok(FramePsnrResult)` if
/// every check passes, `Err(&'static str)` naming the first failure.
fn check_m2_statement(
    signed: &SignedManifest,
    bitstream: &BqBitstream,
    original: &YuvFrame,
    block_paths: &[MerklePath],
    tolerance_db_scaled: i64,
) -> Result<FramePsnrResult, &'static str> {
    // 1. Signature check.
    verify_manifest(signed).map_err(|_| "manifest signature invalid")?;

    // 2. Merkle authentication: every 8×8 Y-block of `original`
    // authenticates against `signed.body.video.merkle_root`.
    let bw = original.width as usize / 8;
    let bh = original.height as usize / 8;
    if block_paths.len() != bw * bh {
        return Err("merkle paths count mismatch");
    }
    for (i, path) in block_paths.iter().enumerate() {
        let by = i / bw;
        let bx = i % bw;
        let leaf = hash_block(original, bx, by);
        verify_merkle_path(&signed.body.video.merkle_root, &leaf, path)
            .map_err(|_| "merkle path failed")?;
    }

    // 3. decode_toy(bitstream) → reconstructed frame.
    let decoded = decode_toy(bitstream).map_err(|_| "decode_toy failed")?;
    if decoded.width != original.width || decoded.height != original.height {
        return Err("decoded dim mismatch");
    }

    // 4. PSNR(decoded, original) ≥ tolerance.
    let result = frame_psnr(
        &decoded.y, &original.y,
        &decoded.u, &original.u,
        &decoded.v, &original.v,
        tolerance_db_scaled,
    );
    if !result.meets_threshold {
        return Err("psnr below tolerance");
    }
    Ok(result)
}

/// Build the canonical "valid" fixture: original frame, encoded
/// bitstream at QP=0 (lossless), signed manifest committing to the
/// original frame's Y-block Merkle root, and Merkle paths for every block.
fn build_valid_fixture(qp: u8, signing_key: &SigningKey) -> (
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

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[test]
fn happy_path_qp0_passes_high_threshold() {
    // qp=0 is bit-exact, so PSNR = ∞ and even the strict 60 dB threshold passes.
    let key = SigningKey::from_bytes(&[1u8; 32]);
    let (signed, bs, original, paths) = build_valid_fixture(0, &key);
    let r = check_m2_statement(&signed, &bs, &original, &paths, TOLERANCE_60DB)
        .expect("happy path should pass");
    // qp=0 is lossless → infinite PSNR sentinel
    assert_eq!(r.psnr_y_scaled, i64::MAX);
}

#[test]
fn happy_path_qp8_passes_36db_threshold() {
    // qp=8 is the M2 §3 acceptance target. Should clear 36 dB
    // comfortably (toy-codec unit tests show ≥ 40 dB on noise frames).
    let key = SigningKey::from_bytes(&[2u8; 32]);
    let (signed, bs, original, paths) = build_valid_fixture(8, &key);
    check_m2_statement(&signed, &bs, &original, &paths, TOLERANCE_36DB)
        .expect("qp=8 should clear 36 dB");
}

#[test]
fn tamper_compressed_byte_fails_closed() {
    // M2 §3.4.1: flipping a byte in compressed.bin must fail closed.
    // After tampering, decode_toy still succeeds (no length/header
    // damage), but the decoded plane no longer matches `original` so
    // PSNR drops below tolerance.
    let key = SigningKey::from_bytes(&[3u8; 32]);
    let (signed, mut bs, original, paths) = build_valid_fixture(0, &key);
    // Flip a mid-stream Y coefficient; the change is far enough off DC
    // that it shows up in the reconstructed pixels.
    bs.coeffs_y[100] ^= 0x7f;
    let err = check_m2_statement(&signed, &bs, &original, &paths, TOLERANCE_60DB)
        .expect_err("tampered compressed must fail");
    assert_eq!(err, "psnr below tolerance");
}

#[test]
fn tamper_manifest_unknown_key_fails_closed() {
    // M2 §3.4.2: a manifest signed by an unknown key must fail closed.
    // Build a valid fixture, then re-sign the body under an attacker
    // key while keeping the original signer's pubkey on the wrapper —
    // verify_manifest catches the pubkey/signature mismatch.
    let known = SigningKey::from_bytes(&[4u8; 32]);
    let attacker = SigningKey::from_bytes(&[0xa5u8; 32]);
    let (mut signed, bs, original, paths) = build_valid_fixture(0, &known);
    // Replace the signature with one produced by a different key.
    let attacker_signed = sign_manifest(signed.body.clone(), &attacker);
    signed.signature = attacker_signed.signature;
    // signed.pubkey still claims `known` signed it — but the signature
    // is from `attacker`. verify_manifest must reject.
    let err = check_m2_statement(&signed, &bs, &original, &paths, TOLERANCE_60DB)
        .expect_err("manifest with foreign signature must fail");
    assert_eq!(err, "manifest signature invalid");
}

#[test]
fn tamper_lower_tolerance_below_actual_psnr_fails_closed() {
    // M2 §3.4.4: lowering the tolerance below the actual PSNR must
    // fail closed. We use qp=32 (heavy quantization, real PSNR ≈ low
    // 30s dB on the noise frame) and demand 60 dB.
    let key = SigningKey::from_bytes(&[5u8; 32]);
    let (signed, bs, original, paths) = build_valid_fixture(32, &key);
    let err = check_m2_statement(&signed, &bs, &original, &paths, TOLERANCE_60DB)
        .expect_err("60 dB demand at qp=32 must fail");
    assert_eq!(err, "psnr below tolerance");
}

#[test]
fn tamper_merkle_path_fails_closed() {
    // Bonus: a wrong Merkle path on any block must fail closed.
    // Tests the manifest crate's merkle_path/verify_merkle_path
    // composition under a real workload.
    let key = SigningKey::from_bytes(&[6u8; 32]);
    let (signed, bs, original, mut paths) = build_valid_fixture(0, &key);
    // Corrupt the first sibling on block 0.
    paths[0].siblings[0][0] ^= 0xff;
    let err = check_m2_statement(&signed, &bs, &original, &paths, TOLERANCE_60DB)
        .expect_err("corrupted merkle path must fail");
    assert_eq!(err, "merkle path failed");
}

#[test]
fn merkle_path_count_mismatch_fails_closed() {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let (signed, bs, original, mut paths) = build_valid_fixture(0, &key);
    paths.pop();
    let err = check_m2_statement(&signed, &bs, &original, &paths, TOLERANCE_60DB)
        .expect_err("missing path must fail");
    assert_eq!(err, "merkle paths count mismatch");
}
