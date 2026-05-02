//! `toy-encode` — milestone-2 producer-side CLI.
//!
//! Reads a raw YUV 4:2:0 frame, encodes it with the toy codec, signs a
//! manifest committing to the original frame, and writes everything to
//! disk so a verifier can run the milestone-2 statement.
//!
//! Outputs:
//!   --out-compressed FILE   the toy-codec bitstream
//!   --out-manifest   FILE   signed manifest JSON
//!   --out-key        FILE   Ed25519 secret key (32 bytes raw); for
//!                           dev use, real producers wouldn't dump this
//!
//! For convenience there's also a `verify` subcommand that runs the
//! milestone-2 statement against the produced files. This is the
//! native simulation of what the prover/verifier will do in-circuit.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use snarkvid_m2_statement::{check, frame_leaf_bytes};
use snarkvid_manifest::{merkle_proof, merkle_root, ManifestBody, SignedManifest, VideoMeta};
use snarkvid_toy_codec::{encode, YuvFrame};

#[derive(Parser, Debug)]
#[command(version, about = "snarkvid milestone-2 toy encoder + verifier")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Encode a raw YUV 4:2:0 frame and produce a signed manifest.
    Encode {
        /// Path to raw YUV 4:2:0 frame (Y plane then U then V, all u8).
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        /// Quantization parameter (1..=64). 1 is lossless.
        #[arg(long, default_value_t = 4)]
        qp: u8,
        #[arg(long)]
        out_compressed: PathBuf,
        #[arg(long)]
        out_manifest: PathBuf,
        /// Optional path to write the dev signing key used for this run.
        #[arg(long)]
        out_key: Option<PathBuf>,
        /// Optional pre-existing key file to sign with.
        #[arg(long)]
        in_key: Option<PathBuf>,
    },
    /// Verify a compressed bitstream + manifest against a witnessed original.
    Verify {
        #[arg(long)]
        compressed: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        original: PathBuf,
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        /// PSNR floor in whole dB. The statement passes iff each plane
        /// is at or above this floor.
        #[arg(long, default_value_t = 36)]
        tolerance_db: u32,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Encode {
            input,
            width,
            height,
            qp,
            out_compressed,
            out_manifest,
            out_key,
            in_key,
        } => cmd_encode(
            &input,
            width,
            height,
            qp,
            &out_compressed,
            &out_manifest,
            out_key.as_deref(),
            in_key.as_deref(),
        ),
        Cmd::Verify {
            compressed,
            manifest,
            original,
            width,
            height,
            tolerance_db,
        } => cmd_verify(&compressed, &manifest, &original, width, height, tolerance_db),
    }
}

fn read_yuv(path: &Path, width: u32, height: u32) -> Result<YuvFrame> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let yn = (width as usize) * (height as usize);
    let cn = yn / 4;
    if bytes.len() != yn + 2 * cn {
        bail!(
            "expected {} bytes for {}x{} 4:2:0, got {}",
            yn + 2 * cn,
            width,
            height,
            bytes.len()
        );
    }
    let y = bytes[..yn].to_vec();
    let u = bytes[yn..yn + cn].to_vec();
    let v = bytes[yn + cn..].to_vec();
    Ok(YuvFrame { width, height, y, u, v })
}

fn cmd_encode(
    input: &Path,
    width: u32,
    height: u32,
    qp: u8,
    out_compressed: &Path,
    out_manifest: &Path,
    out_key: Option<&Path>,
    in_key: Option<&Path>,
) -> Result<()> {
    let frame = read_yuv(input, width, height)?;
    let key = match in_key {
        Some(p) => {
            let bytes = fs::read(p).with_context(|| format!("read key {}", p.display()))?;
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .context("key file must be exactly 32 bytes")?;
            SigningKey::from_bytes(&arr)
        }
        None => SigningKey::generate(&mut OsRng),
    };

    let leaf = frame_leaf_bytes(&frame);
    let leaves: &[&[u8]] = &[&leaf];
    let root = merkle_root(leaves);
    let _path = merkle_proof(leaves, 0)?;

    let body = ManifestBody {
        version: 1,
        created_at: now_secs(),
        device_id: "toy-encode-cli".into(),
        video: VideoMeta {
            width,
            height,
            frame_count: 1,
            fps_num: 30,
            fps_den: 1,
            merkle_root: root,
        },
        audio: None,
    };
    let manifest = SignedManifest::sign(body, &key)?;

    let bitstream = encode(&frame, qp).map_err(|e| anyhow::anyhow!("{e}"))?;
    fs::write(out_compressed, &bitstream)
        .with_context(|| format!("write {}", out_compressed.display()))?;
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;
    fs::write(out_manifest, &manifest_json)
        .with_context(|| format!("write {}", out_manifest.display()))?;

    if let Some(p) = out_key {
        fs::write(p, key.to_bytes()).with_context(|| format!("write key {}", p.display()))?;
    }

    println!(
        "encoded: {} bytes ({}x{}, qp={})",
        bitstream.len(),
        width,
        height,
        qp
    );
    println!("manifest: {}", out_manifest.display());
    println!("signing pubkey: {}", hex32(&key.verifying_key().to_bytes()));
    Ok(())
}

fn cmd_verify(
    compressed: &Path,
    manifest_path: &Path,
    original: &Path,
    width: u32,
    height: u32,
    tolerance_db: u32,
) -> Result<()> {
    let bitstream = fs::read(compressed)?;
    let manifest_bytes = fs::read(manifest_path)?;
    let manifest: SignedManifest = serde_json::from_slice(&manifest_bytes)?;
    let frame = read_yuv(original, width, height)?;

    let leaf = frame_leaf_bytes(&frame);
    let leaves: &[&[u8]] = &[&leaf];
    let path = merkle_proof(leaves, 0)?;

    match check(
        &bitstream,
        &manifest,
        &manifest.pubkey,
        tolerance_db,
        &frame,
        &path,
    ) {
        Ok(()) => {
            println!("OK: derivation proof valid (signed by {})", hex32(&manifest.pubkey));
            println!("    tolerance: {} dB PSNR floor", tolerance_db);
            Ok(())
        }
        Err(e) => bail!("verification failed: {:?}", e),
    }
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hex32(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}
