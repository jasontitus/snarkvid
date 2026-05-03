// Host driver for the RISC Zero side of the spike.
//
// CLI (mirrors SP1 side for bench/run.sh interoperability):
//   risc0-host prove   --input <fixture> --min-size <N> --out proof.bin --commit-out commit.hex
//   risc0-host verify  --proof proof.bin --commit <hex> --min-size <N>
//   risc0-host bench   --fixture-dir ../common/bench-fixtures --out bench.json
//
// Bench output schema (consumed by ../bench/compare.py):
//   {
//     "system": "risc0",
//     "toolchain": "<version>",
//     "gpu": "<device or null>",
//     "rows": [
//       { "size_bytes": 1024, "cycles": ..., "prove_ms": ..., "verify_ms": ...,
//         "proof_bytes": ..., "peak_rss_bytes": ... },
//       ...
//     ]
//   }

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use risc0_zkvm::{default_prover_server, ExecutorEnv, Receipt, VerifierContext};
use serde::Serialize;
use sha2::{Digest, Sha256};
use snarkvid_spike_risc0_methods::SHA256_PREIMAGE_ELF;
use std::path::PathBuf;
use std::time::Instant;

/// RISC Zero host for the snarkvid milestone-1 spike.
#[derive(Parser)]
#[command(name = "risc0-host")]
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
        /// Proof (receipt) file
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

fn get_peak_rss() -> usize {
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

fn main() -> Result<()> {
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

            if let Some(path) = &commit_out {
                std::fs::write(path, bytes_to_hex(&commitment))
                    .context("failed to write commitment")?;
            }

            let env = ExecutorEnv::builder()
                .write(&min_size)
                .write(&data)
                .build()
                .context("failed to build executor env")?;

            let session = default_prover_server()
                .execute(env)
                .assume_verified()
                .run()
                .context("execution failed")?;

            // Verify public values match
            let mut executor = risc0_zkvm::vm::Executor::from_elf(
                ExecutorEnv::builder().build().context("failed to build verify env")?,
                &risc0_zkvm::elf::Elf::load(SHA256_PREIMAGE_ELF)
                    .context("failed to load ELF")?,
            )
            .context("failed to create executor")?;

            let t0 = Instant::now();
            let receipt = default_prover_server()
                .prove_env(env)
                .context("failed to build prover env")?
                .run()
                .context("prove failed")?;
            let prove_ms = t0.elapsed().as_millis() as u64;

            // Extract and verify public values from receipt
            let pv: risc0_zkvm::Journal = receipt.journal;
            let pv_bytes = pv.bytes.as_slice();
            let pv_digest: [u8; 32] = pv_bytes[..32]
                .try_into()
                .context("journal too short for digest")?;
            let pv_min_size: u32 = u32::from_le_bytes(
                pv_bytes[32..36]
                    .try_into()
                    .context("journal too short for min_size")?,
            );

            anyhow::ensure!(pv_digest == commitment, "public value digest mismatch");
            anyhow::ensure!(pv_min_size == min_size, "public value min_size mismatch");

            // Save receipt
            let proof_bytes = bincode::serialize(&receipt)
                .context("failed to serialize receipt")?;
            std::fs::write(&out, &proof_bytes).context("failed to save proof")?;

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

            let proof_bytes = std::fs::read(&proof).context("failed to read proof file")?;
            let receipt: Receipt =
                bincode::deserialize(&proof_bytes).context("failed to deserialize receipt")?;

            let t0 = Instant::now();
            let ctx = VerifierContext::default();
            receipt
                .verify(&ctx, snarkvid_spike_risc0_methods::SHA256_PREIMAGE_ID)
                .context("verification failed")?;
            let verify_ms = t0.elapsed().as_millis() as u64;

            // Verify public values
            let pv_bytes = receipt.journal.bytes.as_slice();
            let pv_digest: [u8; 32] = pv_bytes[..32]
                .try_into()
                .context("journal too short for digest")?;
            let pv_min_size: u32 = u32::from_le_bytes(
                pv_bytes[32..36]
                    .try_into()
                    .context("journal too short for min_size")?,
            );

            anyhow::ensure!(
                pv_digest == *expected_commitment.as_slice(),
                "digest mismatch"
            );
            anyhow::ensure!(pv_min_size == min_size, "min_size mismatch");

            println!("verified in {} ms", verify_ms);
        }
        Commands::Bench { fixture_dir, out } => {
            let sizes = [("1k", 1024), ("1m", 1_048_576), ("10m", 10_485_760)];
            let mut rows = Vec::new();

            for (label, _size) in sizes {
                let fixture = fixture_dir.join(format!("fixture-{}.bin", label));
                if !fixture.exists() {
                    eprintln!("skipping {}: not found", fixture.display());
                    continue;
                }

                let data = std::fs::read(&fixture).context(format!("read {}", fixture.display()))?;
                let min_size = data.len() as u32;

                let env = ExecutorEnv::builder()
                    .write(&min_size)
                    .write(&data)
                    .build()
                    .context("failed to build executor env")?;

                // Execute to get cycle count
                let session = default_prover_server()
                    .execute(env.clone())
                    .assume_verified()
                    .run()
                    .context("execute failed")?;

                // Prove
                let t0 = Instant::now();
                let receipt = default_prover_server()
                    .prove_env(env)
                    .context("failed to build prover env")?
                    .run()
                    .context("prove failed")?;
                let prove_ms = t0.elapsed().as_millis() as u64;

                // Verify
                let t1 = Instant::now();
                let ctx = VerifierContext::default();
                receipt
                    .verify(&ctx, snarkvid_spike_risc0_methods::SHA256_PREIMAGE_ID)
                    .context("verify failed")?;
                let verify_ms = t1.elapsed().as_millis() as u64;

                // Proof size
                let proof_bytes = bincode::serialize(&receipt)
                    .context("failed to serialize receipt for size measurement")?
                    .len();

                let peak_rss = get_peak_rss();

                rows.push(Row {
                    size_label: label.to_string(),
                    size_bytes: data.len(),
                    cycles: session.stats.total_cyclotors,
                    prove_ms,
                    verify_native_ms: verify_ms,
                    proof_bytes,
                    peak_rss_bytes: peak_rss,
                });

                println!(
                    "{}: {} cycles, prove {} ms, verify {} ms, proof {} bytes",
                    label, session.stats.total_cyclotors, prove_ms, verify_ms, proof_bytes
                );
            }

            let result = BenchResult {
                system: "risc0".to_string(),
                toolchain: format!("risc0-{}", env!("CARGO_PKG_VERSION")),
                gpu: None,
                verifier_wasm_gz_bytes: None,
                verify_browser_ms: None,
                rows,
            };

            let json = serde_json::to_string_pretty(&result)?;

            if let Some(path) = out {
                std::fs::write(&path, &json).context("failed to write bench JSON")?;
                println!("wrote {}", path.display());
            } else {
                println!("{}", json);
            }
        }
    }

    Ok(())
}
