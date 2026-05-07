// SP1 host driver. Exposes the same CLI shape as the RISC Zero host so
// bench/run.sh can call them interchangeably.
//
// CLI:
//   sp1-script prove   --input <fixture> --min-size <N> --out proof.bin --commit-out commit.hex
//   sp1-script verify  --proof proof.bin --commit <hex> --min-size <N>
//   sp1-script bench   --fixture-dir ../common/bench-fixtures --out bench.json

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sp1_sdk::prelude::*;
use sp1_sdk::ProverClient;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The ELF we want to execute inside the zkVM.
const ELF: Elf = include_elf!("sha256-preimage");

#[derive(Parser)]
#[command(name = "sp1-script")]
#[command(about = "SP1 side of the snarkvid milestone-1 spike")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Prove SHA-256 preimage knowledge
    Prove {
        /// Input fixture file (private witness)
        #[arg(long)]
        input: PathBuf,
        /// Minimum size constraint
        #[arg(long)]
        min_size: u32,
        /// Output proof file
        #[arg(long)]
        out: PathBuf,
        /// Output commitment hex file
        #[arg(long)]
        commit_out: Option<PathBuf>,
    },
    /// Verify a proof
    Verify {
        /// Proof file
        #[arg(long)]
        proof: PathBuf,
        /// Expected commitment hex
        #[arg(long)]
        commit: String,
        /// Expected min size
        #[arg(long)]
        min_size: u32,
    },
    /// Run benchmarks on all fixtures
    Bench {
        /// Directory containing fixture files
        #[arg(long)]
        fixture_dir: PathBuf,
        /// Output JSON file
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Serialize)]
struct BenchResult {
    system: String,
    toolchain: String,
    gpu: Option<String>,
    verifier_wasm_gz_bytes: Option<usize>,
    verify_browser_ms: Option<f64>,
    rows: Vec<Row>,
}

#[derive(Serialize)]
struct Row {
    size_label: String,
    size_bytes: usize,
    cycles: u64,
    prove_ms: u64,
    verify_native_ms: u64,
    proof_bytes: usize,
    peak_rss_bytes: usize,
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut output, b| {
        use std::fmt::Write;
        let _ = write!(output, "{:02x}", b);
        output
    })
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    let hex = hex.trim().strip_prefix("0x").unwrap_or(hex);
    if hex.len() % 2 != 0 {
        anyhow::bail!("hex string must have even length");
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        bytes.push(u8::from_str_radix(&hex[i..i + 2], 16)?);
    }
    Ok(bytes)
}

#[tokio::main]
async fn main() -> Result<()> {
    sp1_sdk::utils::setup_logger();

    let cli = Cli::parse();

    match cli.command {
        Commands::Prove {
            input,
            min_size,
            out,
            commit_out,
        } => {
            let data = std::fs::read(&input).context("failed to read input file")?;
            let commitment: [u8; 32] = Sha256::digest(&data).into();

            // Write commitment to file if requested
            if let Some(path) = &commit_out {
                std::fs::write(path, bytes_to_hex(&commitment))
                    .context("failed to write commitment")?;
            }

            let mut stdin = SP1Stdin::new();
            stdin.write(&min_size);
            stdin.write(&data);

            let client = ProverClient::from_env().await;
            let pk = client.setup(ELF).await.context("setup failed")?;

            let t0 = Instant::now();
            let mut proof = client
                .prove(&pk, stdin)
                .core()
                .await
                .context("prove failed")?;
            let prove_ms = t0.elapsed().as_millis() as u64;

            // Read public values from proof
            let pv_digest: [u8; 32] = proof.public_values.read();
            let pv_min_size: u32 = proof.public_values.read();

            anyhow::ensure!(
                pv_digest == commitment,
                "public value digest mismatch"
            );
            anyhow::ensure!(
                pv_min_size == min_size,
                "public value min_size mismatch"
            );

            client
                .verify(&proof, pk.verifying_key(), None)
                .context("verification failed")?;

            proof
                .save(&out)
                .context("failed to save proof")?;

            println!("proved in {} ms", prove_ms);
            println!("proof saved to {}", out.display());
        }
        Commands::Verify {
            proof,
            commit,
            min_size,
        } => {
            let expected_commitment = hex_to_bytes(&commit)?;
            anyhow::ensure!(
                expected_commitment.len() == 32,
                "commitment must be 32 bytes"
            );

            let mut proof =
                SP1ProofWithPublicValues::load(&proof).context("failed to load proof")?;

            let client = ProverClient::from_env().await;
            let pk = client.setup(ELF).await.context("setup failed")?;

            let t0 = Instant::now();
            client
                .verify(&proof, pk.verifying_key(), None)
                .context("verification failed")?;
            let verify_ms = t0.elapsed().as_millis() as u64;

            // Verify public values
            let pv_digest: [u8; 32] = proof.public_values.read();
            let pv_min_size: u32 = proof.public_values.read();

            anyhow::ensure!(
                pv_digest == *expected_commitment.as_slice(),
                "digest mismatch"
            );
            anyhow::ensure!(
                pv_min_size == min_size,
                "min_size mismatch"
            );

            println!("verified in {} ms", verify_ms);
        }
        Commands::Bench { fixture_dir, out } => {
            let client = ProverClient::from_env().await;
            let pk = client.setup(ELF).await.context("setup failed")?;

            let sizes = [("1k", 1024), ("1m", 1_048_576), ("10m", 10_485_760)];

            // Re-serialize this after every fixture so a crash mid-run still
            // leaves the completed rows durable on disk.
            let mut partial = BenchResult {
                system: "sp1".to_string(),
                toolchain: format!("sp1-{}", env!("CARGO_PKG_VERSION")),
                gpu: None,
                verifier_wasm_gz_bytes: None,
                verify_browser_ms: None,
                rows: Vec::new(),
            };

            for (label, _size) in sizes {
                let fixture = fixture_dir.join(format!("fixture-{}.bin", label));
                if !fixture.exists() {
                    eprintln!("skipping {}: not found", fixture.display());
                    continue;
                }

                let data = std::fs::read(&fixture).context(format!("read {}", fixture.display()))?;
                let min_size = data.len() as u32;

                let mut stdin = SP1Stdin::new();
                stdin.write(&min_size);
                stdin.write(&data);

                // Execute to get cycle count
                let (_, report) = client
                    .execute(ELF, stdin.clone())
                    .await
                    .context("execute failed")?;

                // Prove
                let t0 = Instant::now();
                let proof = client
                    .prove(&pk, stdin.clone())
                    .core()
                    .await
                    .context("prove failed")?;
                let prove_ms = t0.elapsed().as_millis() as u64;

                // Verify
                let t1 = Instant::now();
                client
                    .verify(&proof, pk.verifying_key(), None)
                    .context("verify failed")?;
                let verify_ms = t1.elapsed().as_millis() as u64;

                // Proof size — save to temp path and measure file size.
                // `.bytes()` panics for Core proofs (onchain-only serialization).
                let proof_path = std::env::temp_dir().join(format!("sp1-bench-{}.bin", label));
                proof
                    .save(&proof_path)
                    .context("failed to save bench proof")?;
                let proof_bytes = std::fs::metadata(&proof_path)
                    .context("failed to stat bench proof")?
                    .len() as usize;
                let _ = std::fs::remove_file(&proof_path);

                // Peak RSS (approximate from /proc/self/status on Linux)
                let peak_rss = get_peak_rss();

                partial.rows.push(Row {
                    size_label: label.to_string(),
                    size_bytes: data.len(),
                    cycles: report.total_instruction_count(),
                    prove_ms,
                    verify_native_ms: verify_ms,
                    proof_bytes,
                    peak_rss_bytes: peak_rss,
                });

                println!(
                    "{}: {} cycles, prove {} ms, verify {} ms, proof {} bytes",
                    label, report.total_instruction_count(), prove_ms, verify_ms, proof_bytes
                );
                let _ = std::io::stdout().flush();

                // Persist the partial result after every fixture so a later
                // crash never erases earlier rows.
                if let Some(path) = out.as_ref() {
                    write_partial(path, &partial)
                        .with_context(|| format!("write partial bench results to {}", path.display()))?;
                }
            }

            if let Some(path) = out.as_ref() {
                println!("wrote {}", path.display());
            } else {
                println!("{}", serde_json::to_string_pretty(&partial)?);
            }
            let _ = std::io::stdout().flush();
        }
    }

    Ok(())
}

fn get_peak_rss() -> usize {
    // Read Peak RSS from /proc/self/status on Linux
    if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
        for line in content.lines() {
            if line.starts_with("VmHWM:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<usize>() {
                        return kb * 1024;
                    }
                }
            }
        }
    }
    0
}

/// Atomically write `result` as JSON to `path`. Writes to `<path>.tmp`, fsyncs,
/// then renames into place — so a crash mid-run never leaves a half-file, and
/// the latest fully-completed row is always durable on disk.
fn write_partial(path: &Path, result: &BenchResult) -> Result<()> {
    let json = serde_json::to_string_pretty(result)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}
