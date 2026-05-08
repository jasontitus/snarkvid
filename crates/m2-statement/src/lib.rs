// The M2 statement, in one library crate.
//
// Captures the claim from milestones/02-toy-transform.md §1, exactly
// once, in one no_std function:
//
//   Public:   compressed: BqBitstream
//             manifest:   SignedManifest
//             tolerance:  PSNR threshold (fixed-point dB scaled by PSNR_SCALE)
//   Private:  original:   YuvFrame                        (witness)
//             paths:      Vec<MerklePath>                  (witness)
//
//   Claim:
//     1. Sig.Verify(manifest.pubkey, manifest.body) == true
//     2. Each 8×8 Y-block of `original` authenticates against
//        manifest.body.video.merkle_root via paths[i].
//     3. decode_toy(compressed) == reconstructed
//     4. psnr(reconstructed, original) >= tolerance
//
// The point of pulling this out of the integration test and into a
// library crate is so the SP1 guest, the host's "smoke" command, and
// the integration test all call the *same* function. Three callers,
// one canonical implementation, no drift.
//
// no_std: this crate compiles for the SP1 guest. The std-y parts
// (file I/O, fixture builder) live in the host crate.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
use alloc::vec::Vec;

use sha2::{Digest, Sha256};
use snarkvid_comparator::{frame_psnr, FramePsnrResult};
use snarkvid_manifest::{verify_manifest, verify_merkle_path, MerklePath, SignedManifest};
use snarkvid_toy_codec::{decode_toy, BqBitstream, YuvFrame};

/// Why a claim was rejected. The guest commits this code as a public
/// output so the verifier can distinguish "manifest tampered" from
/// "PSNR too low" without re-running the prove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimError {
    /// Ed25519 signature on the manifest body didn't validate against
    /// the embedded pubkey.
    ManifestSignatureInvalid,
    /// Merkle path count != number of 8×8 Y-blocks in the original.
    MerklePathCountMismatch,
    /// One of the supplied Merkle paths didn't authenticate the leaf
    /// against the manifest's video merkle_root.
    MerklePathInvalid { block_index: usize },
    /// `decode_toy(compressed)` errored or returned a frame whose
    /// dimensions don't match the original's.
    DecodeFailed,
    /// `decoded` and `original` agree on shape, but combined PSNR is
    /// below the tolerance threshold.
    PsnrBelowTolerance(FramePsnrResult),
}

/// Hash one 8×8 Y-block of `frame` (block at column `bx`, row `by`).
/// Used both as the leaf hash for the original frame's Merkle tree
/// and inside `verify_m2_claim` when checking the supplied paths.
pub fn hash_block(frame: &YuvFrame, bx: usize, by: usize) -> [u8; 32] {
    let mut h = Sha256::new();
    let w = frame.width as usize;
    for r in 0..8 {
        let off = (by * 8 + r) * w + bx * 8;
        h.update(&frame.y[off..off + 8]);
    }
    h.finalize().into()
}

/// Build the leaf layer of the M2 Merkle tree: one leaf per 8×8 Y-block
/// in raster order. Pure helper; the prover host calls this when
/// building manifest fixtures.
pub fn frame_merkle_leaves(frame: &YuvFrame) -> Vec<[u8; 32]> {
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

/// Run the M2 verifier check. Identical contract whether called
/// natively from the host (smoke command), inside the SP1 guest
/// (in-circuit), or from the integration test.
pub fn verify_m2_claim(
    signed: &SignedManifest,
    bitstream: &BqBitstream,
    original: &YuvFrame,
    block_paths: &[MerklePath],
    tolerance_db_scaled: i64,
) -> Result<FramePsnrResult, ClaimError> {
    // 1. Signature.
    verify_manifest(signed).map_err(|_| ClaimError::ManifestSignatureInvalid)?;

    // 2. Merkle authentication: every 8×8 Y-block authenticates against
    //    the manifest's video merkle_root.
    let bw = original.width as usize / 8;
    let bh = original.height as usize / 8;
    if block_paths.len() != bw * bh {
        return Err(ClaimError::MerklePathCountMismatch);
    }
    for (i, path) in block_paths.iter().enumerate() {
        let by = i / bw;
        let bx = i % bw;
        let leaf = hash_block(original, bx, by);
        if verify_merkle_path(&signed.body.video.merkle_root, &leaf, path).is_err() {
            return Err(ClaimError::MerklePathInvalid { block_index: i });
        }
    }

    // 3. Decode.
    let decoded = decode_toy(bitstream).map_err(|_| ClaimError::DecodeFailed)?;
    if decoded.width != original.width || decoded.height != original.height {
        return Err(ClaimError::DecodeFailed);
    }

    // 4. PSNR ≥ tolerance.
    let psnr = frame_psnr(
        &decoded.y, &original.y,
        &decoded.u, &original.u,
        &decoded.v, &original.v,
        tolerance_db_scaled,
    );
    if !psnr.meets_threshold {
        return Err(ClaimError::PsnrBelowTolerance(psnr));
    }
    Ok(psnr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use snarkvid_comparator::PSNR_SCALE;
    use snarkvid_manifest::{
        merkle_path, merkle_root, sign_manifest, DeviceId, ManifestBody, VideoDescriptor,
    };
    use snarkvid_toy_codec::encode_toy;

    fn xs_frame(seed: u32, w: u16, h: u16) -> YuvFrame {
        let wu = w as usize;
        let hu = h as usize;
        let cw = wu / 2;
        let ch = hu / 2;
        let mut state = seed;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state & 0xff) as u8
        };
        let y: Vec<u8> = (0..wu * hu).map(|_| next()).collect();
        let u: Vec<u8> = (0..cw * ch).map(|_| next()).collect();
        let v: Vec<u8> = (0..cw * ch).map(|_| next()).collect();
        YuvFrame { width: w, height: h, y, u, v }
    }

    fn make_body(root: [u8; 32]) -> ManifestBody {
        ManifestBody {
            version: 1,
            video: VideoDescriptor {
                width: 32, height: 32,
                fps_num: 30, fps_den: 1, frame_count: 1,
                merkle_root: root,
            },
            audio: None,
            created_at: 1_715_000_000,
            device_id: DeviceId(*b"m2-statement-test-device-padding-padding-padding-padding-padd-01"),
        }
    }

    fn build_fixture(qp: u8, key: &SigningKey) -> (
        SignedManifest, BqBitstream, YuvFrame, alloc::vec::Vec<MerklePath>,
    ) {
        let original = xs_frame(0xfeedbeef, 32, 32);
        let leaves = frame_merkle_leaves(&original);
        let root = merkle_root(&leaves);
        let signed = sign_manifest(make_body(root), key);
        let bs = encode_toy(&original, qp).expect("encode");
        let paths: alloc::vec::Vec<MerklePath> = (0..leaves.len())
            .map(|i| merkle_path(&leaves, i).expect("path"))
            .collect();
        (signed, bs, original, paths)
    }

    #[test]
    fn happy_path_qp0() {
        let key = SigningKey::from_bytes(&[1u8; 32]);
        let (signed, bs, orig, paths) = build_fixture(0, &key);
        let r = verify_m2_claim(&signed, &bs, &orig, &paths, 60 * PSNR_SCALE).unwrap();
        assert_eq!(r.psnr_y_scaled, i64::MAX); // lossless
    }

    #[test]
    fn manifest_sig_failure_returns_typed_error() {
        let key = SigningKey::from_bytes(&[2u8; 32]);
        let attacker = SigningKey::from_bytes(&[0xa5u8; 32]);
        let (mut signed, bs, orig, paths) = build_fixture(0, &key);
        signed.signature = sign_manifest(signed.body.clone(), &attacker).signature;
        assert_eq!(
            verify_m2_claim(&signed, &bs, &orig, &paths, 60 * PSNR_SCALE),
            Err(ClaimError::ManifestSignatureInvalid),
        );
    }

    #[test]
    fn merkle_path_failure_carries_block_index() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let (signed, bs, orig, mut paths) = build_fixture(0, &key);
        // Corrupt the path for block 7 specifically; the typed error
        // must surface that index so a verifier can localize the fault.
        paths[7].siblings[0][0] ^= 0xff;
        let err = verify_m2_claim(&signed, &bs, &orig, &paths, 60 * PSNR_SCALE).unwrap_err();
        assert!(matches!(err, ClaimError::MerklePathInvalid { block_index: 7 }));
    }

    #[test]
    fn psnr_below_tolerance_returns_actual_psnr() {
        let key = SigningKey::from_bytes(&[4u8; 32]);
        let (signed, bs, orig, paths) = build_fixture(32, &key); // heavy quant
        let err = verify_m2_claim(&signed, &bs, &orig, &paths, 60 * PSNR_SCALE).unwrap_err();
        match err {
            ClaimError::PsnrBelowTolerance(psnr) => {
                // Verifier can read the actual PSNR off the error to
                // explain why the claim was rejected.
                assert!(psnr.psnr_combined_scaled < 60 * PSNR_SCALE);
                assert!(psnr.psnr_combined_scaled > 20 * PSNR_SCALE,
                    "qp=32 should still be reasonable: got {} scaled dB",
                    psnr.psnr_combined_scaled);
            }
            other => panic!("wrong error variant: {:?}", other),
        }
    }
}
