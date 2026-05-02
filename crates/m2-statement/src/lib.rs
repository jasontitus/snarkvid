//! The milestone-2 statement, expressed as a single Rust function.
//!
//! This is the native simulation of what the prover will eventually
//! run inside a zkVM guest. Keeping it in plain Rust means we can:
//!   - integration-test the full statement with `cargo test`,
//!   - measure cycles end-to-end with a profiler,
//!   - drop the same function into the zkVM later with no logic changes.
//!
//! The statement (from milestones/02-toy-transform.md §1):
//!
//! Public:  `compressed: &[u8]`,  `manifest: &SignedManifest`,
//!          `expected_pubkey: &[u8; 32]`,  `tolerance_db: u32`
//! Private: `original: &YuvFrame`, plus the leaf bytes hashed inside
//!          `merkle_proof_of_original`.
//!
//! Returns `Ok(())` iff the proof of derivation is valid:
//!   1. `manifest.verify(expected_pubkey)` succeeds,
//!   2. the supplied original frame authenticates against the
//!      manifest's video Merkle root (single-frame manifest: leaf =
//!      raw YUV bytes concatenated),
//!   3. `decode(compressed) == reconstructed` (decoder runs to
//!      completion under spec — failures bubble up),
//!   4. PSNR(reconstructed, original) ≥ `tolerance_db`.

use snarkvid_comparator::psnr_u8_passes;
use snarkvid_manifest::{merkle_verify, MerklePath, ManifestError, SignedManifest};
use snarkvid_toy_codec::{decode, CodecError, YuvFrame};

#[derive(Debug)]
pub enum StatementError {
    BadSignature,
    BadMerklePath,
    Decode(CodecError),
    BadDimensions,
    PsnrBelowFloor { plane: &'static str, floor_db: u32 },
}

impl From<ManifestError> for StatementError {
    fn from(e: ManifestError) -> Self {
        match e {
            ManifestError::BadSignature => Self::BadSignature,
            ManifestError::BadMerklePath => Self::BadMerklePath,
            _ => Self::BadMerklePath,
        }
    }
}

impl From<CodecError> for StatementError {
    fn from(e: CodecError) -> Self {
        Self::Decode(e)
    }
}

/// Concatenated YUV bytes for a single-frame Merkle leaf.
///
/// The single-frame manifest layout from milestone 2 commits to one
/// leaf per frame. The leaf contents are the Y, U, and V planes
/// concatenated in that order. This helper keeps producer and verifier
/// in agreement on the layout.
pub fn frame_leaf_bytes(frame: &YuvFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(frame.y.len() + frame.u.len() + frame.v.len());
    out.extend_from_slice(&frame.y);
    out.extend_from_slice(&frame.u);
    out.extend_from_slice(&frame.v);
    out
}

/// Run the milestone-2 statement.
pub fn check(
    compressed: &[u8],
    manifest: &SignedManifest,
    expected_pubkey: &[u8; 32],
    tolerance_db: u32,
    // Witness:
    original: &YuvFrame,
    merkle_path: &MerklePath,
) -> Result<(), StatementError> {
    // 1. Signature.
    manifest.verify(expected_pubkey)?;

    // 2. Merkle authentication of the witnessed original.
    let leaf = frame_leaf_bytes(original);
    merkle_verify(&leaf, merkle_path, &manifest.body.video.merkle_root)?;

    // 3. Decode the public bitstream.
    let reconstructed = decode(compressed)?;
    if reconstructed.width != original.width || reconstructed.height != original.height {
        return Err(StatementError::BadDimensions);
    }

    // 4. PSNR comparator on each plane against the witnessed original.
    if !psnr_u8_passes(&reconstructed.y, &original.y, tolerance_db) {
        return Err(StatementError::PsnrBelowFloor {
            plane: "Y",
            floor_db: tolerance_db,
        });
    }
    if !psnr_u8_passes(&reconstructed.u, &original.u, tolerance_db) {
        return Err(StatementError::PsnrBelowFloor {
            plane: "U",
            floor_db: tolerance_db,
        });
    }
    if !psnr_u8_passes(&reconstructed.v, &original.v, tolerance_db) {
        return Err(StatementError::PsnrBelowFloor {
            plane: "V",
            floor_db: tolerance_db,
        });
    }
    Ok(())
}
