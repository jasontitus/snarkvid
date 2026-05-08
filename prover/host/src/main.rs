// M2 prover host driver.
//
// CLI:
//   prover-host smoke   --input frame.yuv --width W --height H --qp QP
//                       --tolerance 36.0 [--signing-key key.bin]
//
//   prover-host prove   <same args>           --out proof.bin
//                       (requires --features build-guest at build time)
//
//   prover-host verify  --proof proof.bin     --tolerance 36.0
//                       --manifest-pubkey-hex <hex>
//                       --bitstream <path>
//                       (requires --features build-guest)
//
// `smoke` runs verify_m2_claim natively over a real fixture — no zkVM
// involved. The same code path the SP1 guest will take in-circuit, so
// any logic bug surfaces here without paying the prove cost.
//
// `prove` and `verify` go through SP1. They're behind a build feature
// because the guest ELF is cross-compiled by build.rs, which needs the
// SP1 toolchain (sp1up + +succinct). The sandbox can't install that,
// so the default build is smoke-only.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use snarkvid_comparator::PSNR_SCALE;
use snarkvid_m2_statement::{
    frame_merkle_leaves, public_inputs_digest, verify_m2_claim, ClaimError,
};
use snarkvid_manifest::{
    merkle_path, merkle_root, sign_manifest, DeviceId, ManifestBody, MerklePath, SignedManifest,
    VideoDescriptor,
};
use snarkvid_toy_codec::{encode_toy, BqBitstream, YuvFrame};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "prover-host")]
#[command(about = "Milestone 2 prover host (SP1 + native smoke)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the M2 statement natively (no zkVM). Sandbox-friendly.
    Smoke {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        width: u16,
        #[arg(long)]
        height: u16,
        #[arg(long, default_value_t = 8)]
        qp: u8,
        /// PSNR tolerance in dB (e.g. 36.0). The claim asserts
        /// reconstructed-vs-original PSNR ≥ this value.
        #[arg(long, default_value_t = 36.0)]
        tolerance: f64,
        /// 32-byte Ed25519 secret key file. Defaults to a hard-coded
        /// deterministic key — fine for development, do NOT use in
        /// production fixtures.
        #[arg(long)]
        signing_key: Option<PathBuf>,
    },

    /// SP1 prove. Requires --features build-guest at build time.
    #[cfg(feature = "build-guest")]
    Prove {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        width: u16,
        #[arg(long)]
        height: u16,
        #[arg(long, default_value_t = 8)]
        qp: u8,
        #[arg(long, default_value_t = 36.0)]
        tolerance: f64,
        #[arg(long)]
        signing_key: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Smoke {
            input, width, height, qp, tolerance, signing_key,
        } => cmd_smoke(input, width, height, qp, tolerance, signing_key),
        #[cfg(feature = "build-guest")]
        Commands::Prove {
            input, width, height, qp, tolerance, signing_key, out,
        } => sp1::cmd_prove(input, width, height, qp, tolerance, signing_key, out),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Fixture builder — shared between smoke and prove.
// ─────────────────────────────────────────────────────────────────────

/// All inputs the M2 statement needs, structured the same way the SP1
/// guest sees them. The host builds this from a raw YUV file + a
/// signing key + a tolerance; the prove command serializes it to SP1
/// stdin; smoke calls verify_m2_claim on it directly.
pub struct M2Inputs {
    pub signed: SignedManifest,
    pub bitstream: BqBitstream,
    pub original: YuvFrame,
    pub block_paths: Vec<MerklePath>,
    pub tolerance_db_scaled: i64,
}

pub fn build_inputs(
    input: &std::path::Path,
    width: u16,
    height: u16,
    qp: u8,
    tolerance: f64,
    signing_key_path: Option<&std::path::Path>,
) -> Result<M2Inputs> {
    anyhow::ensure!(width % 16 == 0 && height % 16 == 0,
        "width and height must be multiples of 16 (4:2:0 + 8x8 blocks)");

    let raw = std::fs::read(input).with_context(|| format!("read {:?}", input))?;
    let y_size = width as usize * height as usize;
    let uv_size = (width as usize / 2) * (height as usize / 2);
    anyhow::ensure!(raw.len() >= y_size + 2 * uv_size,
        "input too small: got {} bytes, need {} ({}x{} YUV420)",
        raw.len(), y_size + 2 * uv_size, width, height);

    let original = YuvFrame {
        width, height,
        y: raw[0..y_size].to_vec(),
        u: raw[y_size..y_size + uv_size].to_vec(),
        v: raw[y_size + uv_size..y_size + 2 * uv_size].to_vec(),
    };

    let leaves = frame_merkle_leaves(&original);
    let root = merkle_root(&leaves);

    let signing_key = match signing_key_path {
        Some(p) => {
            let bytes = std::fs::read(p).with_context(|| format!("read signing key {:?}", p))?;
            anyhow::ensure!(bytes.len() == 32, "signing key must be 32 bytes");
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            SigningKey::from_bytes(&k)
        }
        None => {
            // Deterministic dev key. Do NOT use for production fixtures.
            SigningKey::from_bytes(&[0x42u8; 32])
        }
    };

    let body = ManifestBody {
        version: 1,
        video: VideoDescriptor {
            width, height,
            fps_num: 30, fps_den: 1, frame_count: 1,
            merkle_root: root,
        },
        audio: None,
        created_at: 1_715_000_000,
        device_id: DeviceId(*b"prover-host-dev-device-padding-padding-padding-padding-padding-1"),
    };
    let signed = sign_manifest(body, &signing_key);

    let bitstream = encode_toy(&original, qp).context("encode_toy")?;

    let block_paths: Vec<MerklePath> = (0..leaves.len())
        .map(|i| merkle_path(&leaves, i).map_err(|e| anyhow::anyhow!("merkle_path[{}]: {:?}", i, e)))
        .collect::<Result<_>>()?;

    let tolerance_db_scaled = (tolerance * PSNR_SCALE as f64) as i64;
    Ok(M2Inputs { signed, bitstream, original, block_paths, tolerance_db_scaled })
}

// ─────────────────────────────────────────────────────────────────────
// `smoke` — native M2 statement check, no zkVM
// ─────────────────────────────────────────────────────────────────────

fn cmd_smoke(
    input: PathBuf, width: u16, height: u16, qp: u8, tolerance: f64,
    signing_key: Option<PathBuf>,
) -> Result<()> {
    let inputs = build_inputs(&input, width, height, qp, tolerance, signing_key.as_deref())?;
    let result = verify_m2_claim(
        &inputs.signed, &inputs.bitstream, &inputs.original,
        &inputs.block_paths, inputs.tolerance_db_scaled,
    );
    match result {
        Ok(psnr) => {
            // Hash of inputs that the verifier would also hash, so we
            // can sanity-check against `prover-host prove` output.
            let pubkey_hex = bytes_to_hex(&inputs.signed.pubkey);
            println!("smoke PASS");
            println!("  manifest_pubkey: {}", pubkey_hex);
            println!("  tolerance_db:    {:.2}", tolerance);
            println!("  psnr_y:          {} (scaled, ÷{} for dB)",
                psnr.psnr_y_scaled, PSNR_SCALE);
            println!("  psnr_combined:   {} (scaled)", psnr.psnr_combined_scaled);
            println!("  fixture digest:  {}",
                bytes_to_hex(&fixture_digest(&inputs)));
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("smoke FAIL: {}", explain_error(&e));
        }
    }
}

fn explain_error(e: &ClaimError) -> String {
    match e {
        ClaimError::ManifestSignatureInvalid => "manifest signature did not verify".into(),
        ClaimError::MerklePathCountMismatch => "Merkle path count != block count".into(),
        ClaimError::MerklePathInvalid { block_index } =>
            format!("Merkle path for block {} did not authenticate", block_index),
        ClaimError::DecodeFailed => "decode_toy(bitstream) failed or returned wrong dims".into(),
        ClaimError::PsnrBelowTolerance(p) =>
            format!("PSNR below tolerance: combined={} scaled, y={} scaled",
                p.psnr_combined_scaled, p.psnr_y_scaled),
    }
}

/// SHA-256 over the public M2 inputs. Thin wrapper over the
/// canonical hasher in m2-statement so host and guest agree byte-for-byte.
pub fn fixture_digest(inputs: &M2Inputs) -> [u8; 32] {
    public_inputs_digest(&inputs.signed, &inputs.bitstream, inputs.tolerance_db_scaled)
}

fn bytes_to_hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

// ─────────────────────────────────────────────────────────────────────
// `prove` — SP1 path. Behind feature flag because guest ELF needs sp1up.
// ─────────────────────────────────────────────────────────────────────

#[cfg(feature = "build-guest")]
mod sp1 {
    use super::*;
    use sp1_sdk::prelude::*;
    use sp1_sdk::ProverClient;

    const ELF: Elf = include_elf!("snarkvid-prover-guest");

    #[tokio::main]
    pub async fn cmd_prove(
        input: PathBuf, width: u16, height: u16, qp: u8, tolerance: f64,
        signing_key: Option<PathBuf>, out: PathBuf,
    ) -> Result<()> {
        sp1_sdk::utils::setup_logger();
        let inputs = build_inputs(&input, width, height, qp, tolerance, signing_key.as_deref())?;

        // Native pre-check so a misbuilt fixture fails before we pay the prove cost.
        verify_m2_claim(
            &inputs.signed, &inputs.bitstream, &inputs.original,
            &inputs.block_paths, inputs.tolerance_db_scaled,
        ).map_err(|e| anyhow::anyhow!("native pre-check failed: {}", explain_error(&e)))?;

        let mut stdin = SP1Stdin::new();
        // Order matches the guest's reads (see prover/guest/src/main.rs).
        stdin.write(&inputs.signed);
        stdin.write(&inputs.bitstream);
        stdin.write(&inputs.tolerance_db_scaled);
        stdin.write(&inputs.original);
        stdin.write(&inputs.block_paths);

        let client = ProverClient::from_env().await;
        let pk = client.setup(ELF).await.context("setup failed")?;

        let t = std::time::Instant::now();
        let mut proof = client.prove(&pk, stdin).core().await.context("prove failed")?;
        let prove_ms = t.elapsed().as_millis();

        // Public outputs: digest of public inputs, then a 1-byte status code (0 = ok).
        let pv_digest: [u8; 32] = proof.public_values.read();
        let pv_status: u8 = proof.public_values.read();
        anyhow::ensure!(pv_status == 0, "guest committed non-zero status: {}", pv_status);
        let expected = fixture_digest(&inputs);
        anyhow::ensure!(pv_digest == expected,
            "public digest mismatch (guest {} vs host {})",
            bytes_to_hex(&pv_digest), bytes_to_hex(&expected));

        client.verify(&proof, pk.verifying_key(), None).context("verify")?;
        proof.save(&out).context("save proof")?;
        println!("M2 prove OK in {} ms; proof saved to {}", prove_ms, out.display());
        Ok(())
    }
}
