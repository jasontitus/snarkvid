// Host driver for the RISC Zero side of the spike.
//
// CLI (mirrors SP1 side for bench/run.sh interoperability):
//   risc0-host prove   --input <fixture> --min-size <N> --out proof.bin --commit-out commit.hex
//   risc0-host verify  --proof proof.bin --commit <hex> --min-size <N>
//   risc0-host bench   --fixture-dir ../common/bench-fixtures --out bench.json
//
// Requires risc0-zkvm to be available (uncomment deps in Cargo.toml and
// rebuild with --features risc0). Without it, prints a helpful error.

// ---------------------------------------------------------------------------
// Without the "risc0" feature, print a helpful error and exit.
// ---------------------------------------------------------------------------
#[cfg(not(feature = "risc0"))]
fn main() {
    eprintln!("RISC Zero host: toolchain not available on this machine.");
    eprintln!();
    eprintln!("To enable the RISC Zero side:");
    eprintln!("  1. Install risc0-zkvm (see https://dev.risczero.com/api/zkvm/install)");
    eprintln!("  2. Uncomment risc0-zkvm deps in spike/risc0/{host,methods,methods/guest}/Cargo.toml");
    eprintln!("  3. Rebuild with: cargo build --release --features risc0");
    eprintln!();
    eprintln!("Meanwhile, use the SP1 side:");
    eprintln!("  cd spike/sp1 && cargo run --release -- prove --input <file> --min-size <N> --out proof.bin");
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// Full implementation when the "risc0" feature is enabled.
// ---------------------------------------------------------------------------
#[cfg(feature = "risc0")]
use anyhow::{Context, Result};
#[cfg(feature = "risc0")]
use clap::{Parser, Subcommand};
#[cfg(feature = "risc0")]
use risc0_zkvm::{default_executor, default_prover, ExecutorEnv, Receipt};
#[cfg(feature = "risc0")]
use serde::Serialize;
#[cfg(feature = "risc0")]
use sha2::{Digest, Sha256};
#[cfg(feature = "risc0")]
use snarkvid_spike_risc0_methods::{SHA256_PREIMAGE_ELF, SHA256_PREIMAGE_ID};
#[cfg(feature = "risc0")]
use std::io::Write as _;
#[cfg(feature = "risc0")]
use std::path::{Path, PathBuf};
#[cfg(feature = "risc0")]
use std::time::Instant;

// ---------------------------------------------------------------------------
// Shared types and helpers (available in both modes for future use)
// ---------------------------------------------------------------------------
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

/// Atomically write `result` as JSON to `path`. Writes to `<path>.tmp`, fsyncs,
/// then renames into place — so a crash mid-run never leaves a half-file, and
/// the latest fully-completed row is always durable on disk.
#[cfg(feature = "risc0")]
fn write_partial(path: &Path, result: &BenchResult) -> anyhow::Result<()> {
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

// ---------------------------------------------------------------------------
// CLI and implementation
// ---------------------------------------------------------------------------
#[cfg(feature = "risc0")]
#[derive(Parser)]
#[command(name = "risc0-host")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[cfg(feature = "risc0")]
#[derive(Subcommand)]
enum Commands {
    /// Prove SHA-256 preimage knowledge
    Prove {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        min_size: u32,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        commit_out: Option<PathBuf>,
    },
    /// Verify a proof
    Verify {
        #[arg(long)]
        proof: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        min_size: u32,
    },
    /// Run benchmarks on all fixtures
    Bench {
        #[arg(long)]
        fixture_dir: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[cfg(feature = "risc0")]
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

            let env = {
                let mut b = ExecutorEnv::builder();
                b.write(&min_size).context("write min_size")?;
                b.write(&data).context("write data")?;
                b.build().context("failed to build executor env")?
            };

            let prover = default_prover();
            let t0 = Instant::now();
            let receipt = prover
                .prove(env, SHA256_PREIMAGE_ELF)
                .context("prove failed")?
                .receipt;
            let prove_ms = t0.elapsed().as_millis() as u64;

            // Verify the receipt
            receipt
                .verify(SHA256_PREIMAGE_ID)
                .context("self-verify failed")?;

            // Extract and verify public values from journal
            let pv_bytes = receipt.journal.bytes.as_slice();
            anyhow::ensure!(pv_bytes.len() >= 36, "journal too short");
            let pv_digest: [u8; 32] = pv_bytes[..32].try_into().unwrap();
            let pv_min_size: u32 = u32::from_le_bytes(pv_bytes[32..36].try_into().unwrap());

            anyhow::ensure!(pv_digest == commitment, "public value digest mismatch");
            anyhow::ensure!(pv_min_size == min_size, "public value min_size mismatch");

            // Serialize and save receipt
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
            receipt
                .verify(SHA256_PREIMAGE_ID)
                .context("verification failed")?;
            let verify_ms = t0.elapsed().as_millis() as u64;

            // Verify public values
            let pv_bytes = receipt.journal.bytes.as_slice();
            anyhow::ensure!(pv_bytes.len() >= 36, "journal too short");
            let pv_digest: [u8; 32] = pv_bytes[..32].try_into().unwrap();
            let pv_min_size: u32 = u32::from_le_bytes(pv_bytes[32..36].try_into().unwrap());

            anyhow::ensure!(
                pv_digest == *expected_commitment.as_slice(),
                "digest mismatch"
            );
            anyhow::ensure!(pv_min_size == min_size, "min_size mismatch");

            println!("verified in {} ms", verify_ms);
        }
        Commands::Bench { fixture_dir, out } => {
            let sizes = [("1k", 1024), ("1m", 1_048_576), ("10m", 10_485_760)];
            let prover = default_prover();
            let executor = default_executor();

            // Re-serialize this after every fixture so a crash mid-run still
            // leaves the completed rows durable on disk.
            let mut partial = BenchResult {
                system: "risc0".to_string(),
                toolchain: format!("risc0-{}", env!("CARGO_PKG_VERSION")),
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

                // Build env for execution (to count cycles)
                let exec_env = {
                    let mut b = ExecutorEnv::builder();
                    b.write(&min_size).context("write min_size")?;
                    b.write(&data).context("write data")?;
                    b.build().context("failed to build executor env")?
                };

                // Execute to get cycle count
                let session = executor
                    .execute(exec_env, SHA256_PREIMAGE_ELF)
                    .context("execute failed")?;

                // Build a fresh env for proving (env is consumed by execute)
                let prove_env = {
                    let mut b = ExecutorEnv::builder();
                    b.write(&min_size).context("write min_size")?;
                    b.write(&data).context("write data")?;
                    b.build().context("failed to build prover env")?
                };

                // Prove
                let t0 = Instant::now();
                let receipt = prover
                    .prove(prove_env, SHA256_PREIMAGE_ELF)
                    .context("prove failed")?
                    .receipt;
                let prove_ms = t0.elapsed().as_millis() as u64;

                // Verify
                let t1 = Instant::now();
                receipt
                    .verify(SHA256_PREIMAGE_ID)
                    .context("verify failed")?;
                let verify_ms = t1.elapsed().as_millis() as u64;

                // Proof size
                let proof_bytes = bincode::serialize(&receipt)
                    .context("failed to serialize receipt for size measurement")?
                    .len();

                let peak_rss = get_peak_rss();

                // RISC Zero reports segments, not a single cycle count.
                // Sum segment cycles for the total.
                let total_cycles: u64 = session
                    .segments
                    .iter()
                    .map(|s| s.cycles as u64)
                    .sum();

                partial.rows.push(Row {
                    size_label: label.to_string(),
                    size_bytes: data.len(),
                    cycles: total_cycles,
                    prove_ms,
                    verify_native_ms: verify_ms,
                    proof_bytes,
                    peak_rss_bytes: peak_rss,
                });

                println!(
                    "{}: {} cycles, prove {} ms, verify {} ms, proof {} bytes",
                    label, total_cycles, prove_ms, verify_ms, proof_bytes
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
