// Signed manifest and Merkle tree for snarkvid.
//
// The manifest is the root of trust: a content producer signs a commitment
// to the original media (Merkle root over raw frames + audio), and the
// prover authenticates witness data against that root inside the zkVM.
//
// Format sketch (see milestones/02-toy-transform.md §3):
//
//   SignedManifest {
//     body: ManifestBody {
//       version: 1,
//       video: { width, height, fps, frame_count, merkle_root_yuv },
//       audio: { sample_rate, channels, sample_count, merkle_root_pcm },
//       created_at: u64,        // UNIX timestamp
//       device_id: String,      // identifying the capture device
//       tolerance: { psnr_db: f64, audio_mse: f64 },
//     },
//     pubkey: [u8; 32],         // Ed25519 public key
//     signature: [u8; 64],      // Ed25519 signature over body
//   }
//
// Merkle tree: each leaf is a hash of one frame (YUV planes concatenated)
// or one audio window (PCM samples). The tree uses SHA-256 for milestone 2;
// milestone 3+ may switch to Poseidon for smaller in-circuit footprint.
//
// This crate is no_std. The in-circuit guest calls verify_manifest and
// verify_merkle_path; the host uses the same code natively.

#![no_std]

extern crate alloc;
use alloc::vec::Vec;

#[cfg(feature = "std")]
extern crate std;

use sha2::{Digest, Sha256};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_big_array::BigArray;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Body of the manifest — everything the signature covers.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ManifestBody {
    pub version: u8,
    pub video: VideoDescriptor,
    pub audio: Option<AudioDescriptor>,
    pub created_at: u64,
    pub device_id: DeviceId,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VideoDescriptor {
    pub width: u16,
    pub height: u16,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frame_count: u32,
    /// Merkle root committing to all original YUV frames.
    pub merkle_root: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioDescriptor {
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_count: u32,
    /// Merkle root committing to all original PCM windows.
    pub merkle_root: [u8; 32],
}

/// Device identifier — max 64 bytes, human-readable.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeviceId(#[serde(with = "BigArray")] pub [u8; 64]);

/// A signed manifest: body + Ed25519 public key + signature.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SignedManifest {
    pub body: ManifestBody,
    pub pubkey: [u8; 32],
    #[serde(with = "BigArray")]
    pub signature: [u8; 64],
}

/// A Merkle path proving membership of a leaf at `index` in the tree
/// identified by `root`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MerklePath {
    pub index: usize,
    pub siblings: Vec<[u8; 32]>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ManifestError {
    InvalidSignature,
    InvalidMerkleRoot,
    InvalidPath,
    UnsupportedVersion,
    MissingAudio,
    BufferTooSmall,
}

// ---------------------------------------------------------------------------
// Serialization helpers (no_std compatible)
// ---------------------------------------------------------------------------

impl ManifestBody {
    /// Serialize the manifest body to bytes for signing/verification.
    /// The format is deliberately simple and deterministic:
    ///   u8 version
    ///   u16le width, height
    ///   u32le fps_num, fps_den, frame_count
    ///   32 bytes merkle_root (video)
    ///   if audio present:
    ///     u32le sample_rate
    ///     u8 channels
    ///     u32le sample_count
    ///     32 bytes merkle_root (audio)
    ///   u64le created_at
    ///   64 bytes device_id
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.version);
        buf.extend_from_slice(&self.video.width.to_le_bytes());
        buf.extend_from_slice(&self.video.height.to_le_bytes());
        buf.extend_from_slice(&self.video.fps_num.to_le_bytes());
        buf.extend_from_slice(&self.video.fps_den.to_le_bytes());
        buf.extend_from_slice(&self.video.frame_count.to_le_bytes());
        buf.extend_from_slice(&self.video.merkle_root);
        if let Some(ref audio) = self.audio {
            buf.push(1); // audio present flag
            buf.extend_from_slice(&audio.sample_rate.to_le_bytes());
            buf.extend_from_slice(&[audio.channels]);
            buf.extend_from_slice(&audio.sample_count.to_le_bytes());
            buf.extend_from_slice(&audio.merkle_root);
        } else {
            buf.push(0); // audio absent
        }
        buf.extend_from_slice(&self.created_at.to_le_bytes());
        buf.extend_from_slice(&self.device_id.0);
        buf
    }
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

/// Sign a manifest body with an Ed25519 signing key. Embeds the
/// matching public key alongside the signature so verifiers don't
/// need an out-of-band PKI lookup.
pub fn sign_manifest(body: ManifestBody, signing_key: &SigningKey) -> SignedManifest {
    let body_bytes = body.to_bytes();
    let signature: Signature = signing_key.sign(&body_bytes);
    SignedManifest {
        body,
        pubkey: signing_key.verifying_key().to_bytes(),
        signature: signature.to_bytes(),
    }
}

/// Verify the Ed25519 signature on a manifest. Returns `Ok(())` iff
/// `manifest.pubkey` signed `manifest.body`. Used by both the host
/// (sanity check before proving) and the guest (in-circuit assertion
/// against the public manifest).
///
/// Uses `verify_strict`, which rejects malleable encodings of `R` —
/// this matters because we hash signed bytes elsewhere; a
/// non-canonical signature on the same body would produce a different
/// commitment.
pub fn verify_manifest(manifest: &SignedManifest) -> Result<(), ManifestError> {
    let body_bytes = manifest.body.to_bytes();
    let vk = VerifyingKey::from_bytes(&manifest.pubkey)
        .map_err(|_| ManifestError::InvalidSignature)?;
    let sig = Signature::from_bytes(&manifest.signature);
    vk.verify_strict(&body_bytes, &sig)
        .map_err(|_| ManifestError::InvalidSignature)
}

// ---------------------------------------------------------------------------
// Merkle tree
// ---------------------------------------------------------------------------

/// Compute the root of a Merkle tree over `leaves`, where each leaf is
/// already a SHA-256 hash.
pub fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    if leaves.len() == 1 {
        return leaves[0];
    }

    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity((level.len() + 1) / 2);
        for pair in level.chunks(2) {
            let mut hasher = Sha256::new();
            hasher.update(&pair[0]);
            if pair.len() > 1 {
                hasher.update(&pair[1]);
            } else {
                hasher.update(&pair[0]); // duplicate last if odd
            }
            let digest: [u8; 32] = hasher.finalize().into();
            next.push(digest);
        }
        level = next;
    }
    level[0]
}

/// Hash a single leaf (used to build the leaf layer before tree construction).
pub fn hash_leaf(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Build the authentication path for `leaves[index]`. Returns the
/// siblings the verifier needs to recompute the root, level by level
/// from the leaf upward. Mirrors `merkle_root`'s "duplicate the last
/// odd-out node" rule, so every level emitted by `merkle_root`
/// composes correctly with these paths.
pub fn merkle_path(leaves: &[[u8; 32]], index: usize) -> Result<MerklePath, ManifestError> {
    if leaves.is_empty() || index >= leaves.len() {
        return Err(ManifestError::InvalidPath);
    }
    let mut siblings = Vec::new();
    let mut current_idx = index;
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        let sibling_idx = if current_idx % 2 == 0 { current_idx + 1 } else { current_idx - 1 };
        // Odd-length level → last node duplicates itself, so its sibling is itself.
        let sibling = if sibling_idx < level.len() {
            level[sibling_idx]
        } else {
            level[current_idx]
        };
        siblings.push(sibling);

        // Compute the next level the same way merkle_root does.
        let mut next = Vec::with_capacity((level.len() + 1) / 2);
        for pair in level.chunks(2) {
            let mut hasher = Sha256::new();
            hasher.update(&pair[0]);
            if pair.len() > 1 {
                hasher.update(&pair[1]);
            } else {
                hasher.update(&pair[0]);
            }
            next.push(hasher.finalize().into());
        }
        level = next;
        current_idx /= 2;
    }
    Ok(MerklePath { index, siblings })
}

/// Verify that `leaf` is a member of the Merkle tree with `root`,
/// given its `path`.
pub fn verify_merkle_path(
    root: &[u8; 32],
    leaf: &[u8; 32],
    path: &MerklePath,
) -> Result<(), ManifestError> {
    let mut current = *leaf;
    let mut idx = path.index;

    for sibling in &path.siblings {
        let mut hasher = Sha256::new();
        if idx % 2 == 0 {
            hasher.update(&current);
            hasher.update(sibling);
        } else {
            hasher.update(sibling);
            hasher.update(&current);
        }
        current = hasher.finalize().into();
        idx /= 2;
    }

    if current == *root {
        Ok(())
    } else {
        Err(ManifestError::InvalidMerkleRoot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn fixture_body() -> ManifestBody {
        ManifestBody {
            version: 1,
            video: VideoDescriptor {
                width: 1280,
                height: 720,
                fps_num: 30,
                fps_den: 1,
                frame_count: 100,
                merkle_root: [42u8; 32],
            },
            audio: None,
            created_at: 1_715_000_000,
            device_id: DeviceId(*b"test-device-0123456789abcdef0123456789abcdef0123456789abcdef0123"),
        }
    }

    fn fixture_signing_key() -> SigningKey {
        // Deterministic key for tests. Real callers use OsRng.
        SigningKey::from_bytes(&[7u8; 32])
    }

    #[test]
    fn merkle_root_single_leaf() {
        let leaf = hash_leaf(b"hello");
        let root = merkle_root(&[leaf]);
        assert_eq!(root, leaf);
    }

    #[test]
    fn merkle_root_two_leaves() {
        let a = hash_leaf(b"hello");
        let b = hash_leaf(b"world");
        let root = merkle_root(&[a, b]);

        let path_a = MerklePath { index: 0, siblings: vec![b] };
        assert!(verify_merkle_path(&root, &a, &path_a).is_ok());

        let path_b = MerklePath { index: 1, siblings: vec![a] };
        assert!(verify_merkle_path(&root, &b, &path_b).is_ok());
    }

    #[test]
    fn merkle_tamper_detected() {
        let a = hash_leaf(b"hello");
        let b = hash_leaf(b"world");
        let root = merkle_root(&[a, b]);
        let fake = hash_leaf(b"evil");
        let path = MerklePath { index: 0, siblings: vec![b] };
        assert!(verify_merkle_path(&root, &fake, &path).is_err());
    }

    #[test]
    fn merkle_path_round_trip_power_of_two() {
        let leaves: Vec<[u8; 32]> = (0..8).map(|i| hash_leaf(&[i as u8])).collect();
        let root = merkle_root(&leaves);
        for (i, leaf) in leaves.iter().enumerate() {
            let path = merkle_path(&leaves, i).unwrap();
            assert_eq!(path.siblings.len(), 3, "8-leaf tree → 3-deep path");
            assert!(
                verify_merkle_path(&root, leaf, &path).is_ok(),
                "path[{}] should verify",
                i
            );
        }
    }

    #[test]
    fn merkle_path_round_trip_odd_length() {
        // 5 leaves: tree shape forces last-node duplication at multiple levels.
        let leaves: Vec<[u8; 32]> = (0..5).map(|i| hash_leaf(&[i as u8])).collect();
        let root = merkle_root(&leaves);
        for (i, leaf) in leaves.iter().enumerate() {
            let path = merkle_path(&leaves, i).unwrap();
            assert!(
                verify_merkle_path(&root, leaf, &path).is_ok(),
                "odd-length path[{}] should verify",
                i
            );
        }
    }

    #[test]
    fn merkle_path_rejects_out_of_range() {
        let leaves: Vec<[u8; 32]> = (0..4).map(|i| hash_leaf(&[i as u8])).collect();
        assert_eq!(merkle_path(&leaves, 4), Err(ManifestError::InvalidPath));
        assert_eq!(merkle_path(&[], 0), Err(ManifestError::InvalidPath));
    }

    #[test]
    fn manifest_sign_verify_round_trip() {
        let key = fixture_signing_key();
        let signed = sign_manifest(fixture_body(), &key);
        assert!(verify_manifest(&signed).is_ok());
    }

    #[test]
    fn manifest_tampered_body_fails_verify() {
        // M2 §3.4: flipping a byte in the manifest must fail closed.
        let key = fixture_signing_key();
        let mut signed = sign_manifest(fixture_body(), &key);
        signed.body.video.frame_count += 1;
        assert_eq!(verify_manifest(&signed), Err(ManifestError::InvalidSignature));
    }

    #[test]
    fn manifest_tampered_signature_fails_verify() {
        let key = fixture_signing_key();
        let mut signed = sign_manifest(fixture_body(), &key);
        signed.signature[0] ^= 0xff;
        assert_eq!(verify_manifest(&signed), Err(ManifestError::InvalidSignature));
    }

    #[test]
    fn manifest_unknown_pubkey_fails_verify() {
        // M2 §3.4: "manifest signed by an unknown key → fails".
        // Substituting a different (valid) pubkey on a body signed by
        // someone else must fail — the signature won't match.
        let known = fixture_signing_key();
        let attacker = SigningKey::from_bytes(&[0xa5u8; 32]);
        let mut signed = sign_manifest(fixture_body(), &known);
        signed.pubkey = attacker.verifying_key().to_bytes();
        assert_eq!(verify_manifest(&signed), Err(ManifestError::InvalidSignature));
    }

    #[test]
    fn manifest_invalid_pubkey_bytes_fails_verify() {
        let key = fixture_signing_key();
        let mut signed = sign_manifest(fixture_body(), &key);
        // ed25519_dalek rejects non-canonical / off-curve points.
        // All-zeros is the identity element → off-prime-order subgroup.
        signed.pubkey = [0u8; 32];
        assert_eq!(verify_manifest(&signed), Err(ManifestError::InvalidSignature));
    }
}
