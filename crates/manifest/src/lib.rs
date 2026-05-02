//! Signed manifest format for snarkvid.
//!
//! The manifest commits to the original media (raw YUV frames + raw PCM
//! audio windows) via two Merkle roots, plus metadata about the source
//! (dimensions, sample rate, etc.) and a creation timestamp. The whole
//! manifest body is signed with Ed25519.
//!
//! Verifying a proof requires:
//! 1. checking the manifest signature against a trusted pubkey,
//! 2. authenticating each original frame / audio window the proof
//!    inspects against the corresponding Merkle root.
//!
//! This crate provides both halves: building / signing on the producer
//! side, and verifying / authenticating on the consumer (in-circuit)
//! side.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Hash output of the Merkle tree. SHA-256 for now (broadly available
/// in zkVMs); milestone 4 may switch to a circuit-friendlier hash.
pub type Hash = [u8; 32];

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("signature verification failed")]
    BadSignature,
    #[error("manifest body could not be canonically serialized: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("merkle path does not authenticate leaf")]
    BadMerklePath,
    #[error("merkle path length {got} does not match tree depth {expected}")]
    PathLengthMismatch { got: usize, expected: usize },
    #[error("leaf index {index} out of range for tree of {len} leaves")]
    LeafIndexOutOfRange { index: usize, len: usize },
}

/// Per-stream video metadata committed to in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoMeta {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Merkle root over each original frame's raw YUV bytes (one leaf
    /// per frame). For a single-frame manifest, the root equals
    /// `leaf_hash(frame_bytes)`.
    pub merkle_root: Hash,
}

/// Per-stream audio metadata. Optional for v1 where milestone 2 is
/// video-only; populated starting at milestone 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioMeta {
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_count: u64,
    pub merkle_root: Hash,
}

/// The manifest body — everything the signature covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestBody {
    pub version: u32,
    pub created_at: u64,
    pub device_id: String,
    pub video: VideoMeta,
    pub audio: Option<AudioMeta>,
}

/// Full signed manifest as it travels alongside the compressed video.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    pub body: ManifestBody,
    /// Signer's Ed25519 public key (32 bytes), hex-encoded for JSON portability.
    #[serde(with = "hex_array_32")]
    pub pubkey: [u8; 32],
    #[serde(with = "hex_array_64")]
    pub signature: [u8; 64],
}

impl SignedManifest {
    /// Sign a body and produce a manifest. The serialized JSON of the
    /// body (no whitespace, sorted keys) is the byte string that gets
    /// signed.
    pub fn sign(body: ManifestBody, key: &SigningKey) -> Result<Self, ManifestError> {
        let bytes = canonical_body_bytes(&body)?;
        let sig = key.sign(&bytes);
        Ok(Self {
            body,
            pubkey: key.verifying_key().to_bytes(),
            signature: sig.to_bytes(),
        })
    }

    /// Verify the manifest's signature against an expected trusted
    /// pubkey. Returns `Ok(())` on success.
    pub fn verify(&self, expected_pubkey: &[u8; 32]) -> Result<(), ManifestError> {
        if &self.pubkey != expected_pubkey {
            return Err(ManifestError::BadSignature);
        }
        let vk = VerifyingKey::from_bytes(&self.pubkey).map_err(|_| ManifestError::BadSignature)?;
        let sig = Signature::from_bytes(&self.signature);
        let bytes = canonical_body_bytes(&self.body)?;
        vk.verify(&bytes, &sig).map_err(|_| ManifestError::BadSignature)
    }
}

/// Canonical bytes-of-body for signing/verification.
///
/// Uses `serde_json` with sorted keys (default behavior) and no
/// whitespace. We do NOT use a more compact format because:
/// (a) the manifest is small,
/// (b) human-readable is useful for debugging,
/// (c) inside the circuit we recompute the same bytes from the same
///     struct, so as long as both sides are deterministic we're fine.
fn canonical_body_bytes(body: &ManifestBody) -> Result<Vec<u8>, ManifestError> {
    Ok(serde_json::to_vec(body)?)
}

// ---------- Merkle tree ----------

/// Build a Merkle root over the given leaves.
///
/// Tree shape:
/// - Each leaf is hashed with a 0x00 prefix byte: `H(0x00 || leaf_bytes)`.
/// - Each internal node is hashed with a 0x01 prefix: `H(0x01 || left || right)`.
/// - Odd nodes at any level are duplicated (Bitcoin-style padding).
///
/// Returns the root hash and the auxiliary tree levels (used by
/// `merkle_proof`). For single-leaf trees, the root is just the leaf
/// hash.
pub fn merkle_root(leaves: &[&[u8]]) -> Hash {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<Hash> = leaves.iter().map(|l| hash_leaf(l)).collect();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().unwrap());
        }
        let mut next = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks_exact(2) {
            next.push(hash_pair(&pair[0], &pair[1]));
        }
        level = next;
    }
    level[0]
}

/// Authentication path for the leaf at `index`.
///
/// The path is the sequence of sibling hashes from leaf to root, in
/// bottom-up order. Each path step also records whether the sibling
/// was on the left (`true`) or right (`false`) of the cursor at that
/// level. Verifying the path reproduces the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerklePath {
    pub leaf_index: u32,
    pub steps: Vec<MerkleStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerkleStep {
    pub sibling: Hash,
    /// `true` ⇔ sibling is on the **left** of our running hash.
    pub sibling_is_left: bool,
}

pub fn merkle_proof(leaves: &[&[u8]], index: usize) -> Result<MerklePath, ManifestError> {
    if index >= leaves.len() {
        return Err(ManifestError::LeafIndexOutOfRange { index, len: leaves.len() });
    }
    let mut level: Vec<Hash> = leaves.iter().map(|l| hash_leaf(l)).collect();
    let mut idx = index;
    let mut steps = Vec::new();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().unwrap());
        }
        let sibling_index = idx ^ 1;
        let sibling_is_left = sibling_index < idx; // sibling sits on the left iff our idx is odd
        steps.push(MerkleStep {
            sibling: level[sibling_index],
            sibling_is_left,
        });
        let mut next = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks_exact(2) {
            next.push(hash_pair(&pair[0], &pair[1]));
        }
        level = next;
        idx /= 2;
    }
    Ok(MerklePath {
        leaf_index: index as u32,
        steps,
    })
}

/// Verify a path: returns `Ok(())` iff the path authenticates the
/// given leaf bytes against the given root.
pub fn merkle_verify(leaf_bytes: &[u8], path: &MerklePath, root: &Hash) -> Result<(), ManifestError> {
    let mut cursor = hash_leaf(leaf_bytes);
    for step in &path.steps {
        cursor = if step.sibling_is_left {
            hash_pair(&step.sibling, &cursor)
        } else {
            hash_pair(&cursor, &step.sibling)
        };
    }
    if &cursor == root {
        Ok(())
    } else {
        Err(ManifestError::BadMerklePath)
    }
}

fn hash_leaf(bytes: &[u8]) -> Hash {
    let mut h = Sha256::new();
    h.update([0x00]);
    h.update(bytes);
    h.finalize().into()
}

fn hash_pair(left: &Hash, right: &Hash) -> Hash {
    let mut h = Sha256::new();
    h.update([0x01]);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

// ---------- hex helpers for serde ----------

mod hex_array_32 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&to_hex(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s: &str = Deserialize::deserialize(d)?;
        let v = from_hex(s).map_err(D::Error::custom)?;
        v.try_into().map_err(|_| D::Error::custom("expected 32 bytes"))
    }

    fn to_hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push_str(&format!("{:02x}", b));
        }
        out
    }

    fn from_hex(s: &str) -> Result<Vec<u8>, &'static str> {
        if s.len() % 2 != 0 {
            return Err("odd-length hex");
        }
        let mut out = Vec::with_capacity(s.len() / 2);
        for chunk in s.as_bytes().chunks(2) {
            let h = nibble(chunk[0])?;
            let l = nibble(chunk[1])?;
            out.push((h << 4) | l);
        }
        Ok(out)
    }

    fn nibble(c: u8) -> Result<u8, &'static str> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err("invalid hex char"),
        }
    }
}

mod hex_array_64 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&to_hex(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let s: &str = Deserialize::deserialize(d)?;
        let v = from_hex(s).map_err(D::Error::custom)?;
        v.try_into().map_err(|_| D::Error::custom("expected 64 bytes"))
    }

    fn to_hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push_str(&format!("{:02x}", b));
        }
        out
    }

    fn from_hex(s: &str) -> Result<Vec<u8>, &'static str> {
        if s.len() % 2 != 0 {
            return Err("odd-length hex");
        }
        let mut out = Vec::with_capacity(s.len() / 2);
        for chunk in s.as_bytes().chunks(2) {
            let h = nibble(chunk[0])?;
            let l = nibble(chunk[1])?;
            out.push((h << 4) | l);
        }
        Ok(out)
    }

    fn nibble(c: u8) -> Result<u8, &'static str> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err("invalid hex char"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn fixture_body() -> ManifestBody {
        ManifestBody {
            version: 1,
            created_at: 1700000000,
            device_id: "test-camera-001".into(),
            video: VideoMeta {
                width: 1280,
                height: 720,
                frame_count: 1,
                fps_num: 30,
                fps_den: 1,
                merkle_root: [0xab; 32],
            },
            audio: None,
        }
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let manifest = SignedManifest::sign(fixture_body(), &key).unwrap();
        manifest.verify(&key.verifying_key().to_bytes()).unwrap();
    }

    #[test]
    fn verify_fails_for_wrong_pubkey() {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let other = SigningKey::generate(&mut rng);
        let manifest = SignedManifest::sign(fixture_body(), &key).unwrap();
        assert!(matches!(
            manifest.verify(&other.verifying_key().to_bytes()),
            Err(ManifestError::BadSignature)
        ));
    }

    #[test]
    fn verify_fails_after_body_tamper() {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let mut manifest = SignedManifest::sign(fixture_body(), &key).unwrap();
        manifest.body.video.frame_count = 9999;
        assert!(matches!(
            manifest.verify(&key.verifying_key().to_bytes()),
            Err(ManifestError::BadSignature)
        ));
    }

    #[test]
    fn json_round_trip() {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let manifest = SignedManifest::sign(fixture_body(), &key).unwrap();
        let s = serde_json::to_string(&manifest).unwrap();
        let back: SignedManifest = serde_json::from_str(&s).unwrap();
        back.verify(&key.verifying_key().to_bytes()).unwrap();
    }

    #[test]
    fn merkle_single_leaf() {
        let leaf = b"hello";
        let root = merkle_root(&[leaf]);
        let path = merkle_proof(&[leaf], 0).unwrap();
        merkle_verify(leaf, &path, &root).unwrap();
    }

    #[test]
    fn merkle_two_leaves() {
        let a = b"alpha";
        let b = b"beta";
        let leaves: &[&[u8]] = &[a, b];
        let root = merkle_root(leaves);
        for (i, leaf) in leaves.iter().enumerate() {
            let path = merkle_proof(leaves, i).unwrap();
            merkle_verify(leaf, &path, &root).unwrap();
        }
    }

    #[test]
    fn merkle_seven_leaves_with_duplication() {
        let leaves_owned: Vec<Vec<u8>> = (0..7u8).map(|i| vec![i; 16]).collect();
        let leaves: Vec<&[u8]> = leaves_owned.iter().map(|v| v.as_slice()).collect();
        let root = merkle_root(&leaves);
        for (i, leaf) in leaves.iter().enumerate() {
            let path = merkle_proof(&leaves, i).unwrap();
            merkle_verify(leaf, &path, &root).unwrap();
        }
    }

    #[test]
    fn merkle_verify_fails_for_wrong_leaf() {
        let leaves_owned: Vec<Vec<u8>> = (0..4u8).map(|i| vec![i; 16]).collect();
        let leaves: Vec<&[u8]> = leaves_owned.iter().map(|v| v.as_slice()).collect();
        let root = merkle_root(&leaves);
        let path = merkle_proof(&leaves, 1).unwrap();
        let wrong = vec![99u8; 16];
        assert!(matches!(
            merkle_verify(&wrong, &path, &root),
            Err(ManifestError::BadMerklePath)
        ));
    }

    #[test]
    fn merkle_verify_fails_for_wrong_root() {
        let leaves: &[&[u8]] = &[b"a", b"b", b"c", b"d"];
        let path = merkle_proof(leaves, 2).unwrap();
        let bad_root = [0u8; 32];
        assert!(matches!(
            merkle_verify(leaves[2], &path, &bad_root),
            Err(ManifestError::BadMerklePath)
        ));
    }

    #[test]
    fn merkle_proof_out_of_range() {
        let leaves: &[&[u8]] = &[b"a", b"b"];
        assert!(matches!(
            merkle_proof(leaves, 5),
            Err(ManifestError::LeafIndexOutOfRange { index: 5, len: 2 })
        ));
    }
}
