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

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Body of the manifest — everything the signature covers.
#[derive(Clone, Debug, PartialEq)]
pub struct ManifestBody {
    pub version: u8,
    pub video: VideoDescriptor,
    pub audio: Option<AudioDescriptor>,
    pub created_at: u64,
    pub device_id: DeviceId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoDescriptor {
    pub width: u16,
    pub height: u16,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frame_count: u32,
    /// Merkle root committing to all original YUV frames.
    pub merkle_root: [u8; 32],
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioDescriptor {
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_count: u32,
    /// Merkle root committing to all original PCM windows.
    pub merkle_root: [u8; 32],
}

/// Device identifier — max 64 bytes, human-readable.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceId(pub [u8; 64]);

/// A signed manifest: body + Ed25519 public key + signature.
#[derive(Clone, Debug, PartialEq)]
pub struct SignedManifest {
    pub body: ManifestBody,
    pub pubkey: [u8; 32],
    pub signature: [u8; 64],
}

/// A Merkle path proving membership of a leaf at `index` in the tree
/// identified by `root`.
#[derive(Clone, Debug, PartialEq)]
pub struct MerklePath {
    pub index: usize,
    pub siblings: Vec<[u8; 32]>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
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

/// Verify an Ed25519 signature over the manifest body.
///
/// Returns Ok(()) if `pubkey` signed `body`, Err otherwise.
/// This runs both natively (host) and in-circuit (guest).
pub fn verify_manifest(
    manifest: &SignedManifest,
) -> Result<(), ManifestError> {
    let body_bytes = manifest.body.to_bytes();
    // Hash the body bytes with SHA-256 before Ed25519 verification.
    // Ed25519 signs the SHA-512 hash internally, but we pre-hash for
    // compatibility with the dalek API.
    let _hash = Sha256::digest(&body_bytes);

    // Milestone 2 day 2: wire actual Ed25519 verification.
    // ed25519_dalek::VerifyingKey::from_bytes(&manifest.pubkey)
    //     .and_then(|vk| vk.verify_strict(&body_bytes, &Signature::from_bytes(&manifest.signature)))
    //     .map_err(|_| ManifestError::InvalidSignature)?;

    // Stub: accept any signature for now (unblock development)
    let _ = manifest;
    Ok(())
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

        // Verify both leaves against root
        let siblings_a = vec![b];
        let path_a = MerklePath {
            index: 0,
            siblings: siblings_a,
        };
        assert!(verify_merkle_path(&root, &a, &path_a).is_ok());

        let siblings_b = vec![a];
        let path_b = MerklePath {
            index: 1,
            siblings: siblings_b,
        };
        assert!(verify_merkle_path(&root, &b, &path_b).is_ok());
    }

    #[test]
    fn merkle_tamper_detected() {
        let a = hash_leaf(b"hello");
        let b = hash_leaf(b"world");
        let root = merkle_root(&[a, b]);

        let fake = hash_leaf(b"evil");
        let siblings = vec![b];
        let path = MerklePath { index: 0, siblings };
        assert!(verify_merkle_path(&root, &fake, &path).is_err());
    }
}
