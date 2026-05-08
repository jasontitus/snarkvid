// SP1 host driver. Exposes the same CLI shape as the RISC Zero host so
// bench/run.sh can call them interchangeably.
//
// CLI:
//   sp1-script prove   --workload <sha256|toy-decode>
//                      --input <fixture> --min-size <N>
//                      --out proof.bin --commit-out commit.hex
//   sp1-script verify  --workload <...> --proof proof.bin
//                      --commit <hex> --min-size <N>
//   sp1-script bench   --workload <...>
//                      --fixture-dir ../common/bench-fixtures --out bench.json
//
// `--workload sha256` (default) keeps the original M1 statement: prove
// SHA-256 of `data` matches `commitment` and `data.len() >= min_size`.
//
// `--workload toy-decode` proves the M1b/M2 toy codec kernel: SHA-256 of
// `decode_toy(bitstream).{y,u,v}` matches `commitment`. Same statement
// shape as the Jolt and Sonobe toy-decode workloads, so numbers compare
// directly across all three systems.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sp1_sdk::prelude::*;
use sp1_sdk::ProverClient;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use snarkvid_toy_codec::{decode_toy, BqHeader, YuvFrame};

const SHA_ELF: Elf = include_elf!("sha256-preimage");
const TOY_ELF: Elf = include_elf!("toy-decode");

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Workload {
    /// M1 statement: SHA-256 preimage of `data`, length >= min_size.
    Sha256,
    /// M1b/M2 statement: SHA-256 of decode_toy(bitstream) outputs.
    ToyDecode,
}

#[derive(Parser)]
#[command(name = "sp1-script")]
#[command(about = "SP1 side of the snarkvid spike")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Prove the chosen workload.
    Prove {
        #[arg(long, value_enum, default_value_t = Workload::Sha256)]
        workload: Workload,
        /// Input fixture file (private witness).
        #[arg(long)]
        input: PathBuf,
        /// Minimum size constraint (sha256 only; ignored by toy-decode).
        #[arg(long, default_value_t = 0)]
        min_size: u32,
        /// Output proof file.
        #[arg(long)]
        out: PathBuf,
        /// Output commitment hex file.
        #[arg(long)]
        commit_out: Option<PathBuf>,
    },
    /// Verify a proof.
    Verify {
        #[arg(long, value_enum, default_value_t = Workload::Sha256)]
        workload: Workload,
        #[arg(long)]
        proof: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long, default_value_t = 0)]
        min_size: u32,
    },
    /// Run benchmarks on all fixtures.
    Bench {
        #[arg(long, value_enum, default_value_t = Workload::Sha256)]
        workload: Workload,
        #[arg(long)]
        fixture_dir: PathBuf,
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

/// Build a deterministic 16x16 4:2:0 YUV frame from the leading bytes of
/// `data` (padded with zeros if short). Mirrors the Jolt toy-decode
/// fixture so SP1 / Jolt / Sonobe numbers compare apples-to-apples.
fn build_toy_frame(data: &[u8]) -> YuvFrame {
    let mut buf = data.to_vec();
    buf.resize(384, 0);
    YuvFrame {
        width: 16,
        height: 16,
        y: buf[0..256].to_vec(),
        u: buf[256..320].to_vec(),
        v: buf[320..384].to_vec(),
    }
}

/// Native reference: run encode_toy then decode_toy, hash the decoded
/// YUV. The guest must produce the same digest.
fn toy_native_commitment(frame: &YuvFrame) -> Result<[u8; 32]> {
    let bs = snarkvid_toy_codec::encode_toy(frame, 0)
        .map_err(|e| anyhow::anyhow!("encode_toy: {:?}", e))?;
    let decoded = decode_toy(&bs).map_err(|e| anyhow::anyhow!("decode_toy: {:?}", e))?;
    let mut h = Sha256::new();
    h.update(&decoded.y);
    h.update(&decoded.u);
    h.update(&decoded.v);
    Ok(h.finalize().into())
}

/// Write a toy-decode bitstream to SP1 stdin in the same field order
/// the guest reads it. Keeps host and guest layouts in lockstep.
fn write_toy_stdin(stdin: &mut SP1Stdin, frame: &YuvFrame) -> Result<()> {
    let bs = snarkvid_toy_codec::encode_toy(frame, 0)
        .map_err(|e| anyhow::anyhow!("encode_toy: {:?}", e))?;
    let BqHeader {
        width,
        height,
        qp,
        chroma_format,
    } = bs.header;
    stdin.write(&width);
    stdin.write(&height);
    stdin.write(&qp);
    stdin.write(&chroma_format);
    stdin.write(&bs.coeffs_y);
    stdin.write(&bs.coeffs_u);
    stdin.write(&bs.coeffs_v);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    sp1_sdk::utils::setup_logger();

    let cli = Cli::parse();

    match cli.command {
        Commands::Prove {
            workload,
            input,
            min_size,
            out,
            commit_out,
        } => match workload {
            Workload::Sha256 => prove_sha256(input, min_size, out, commit_out).await,
            Workload::ToyDecode => prove_toy_decode(input, out, commit_out).await,
        },
        Commands::Verify {
            workload,
            proof,
            commit,
            min_size,
        } => match workload {
            Workload::Sha256 => verify_sha256(proof, commit, min_size).await,
            Workload::ToyDecode => verify_toy_decode(proof, commit).await,
        },
        Commands::Bench {
            workload,
            fixture_dir,
            out,
        } => match workload {
            Workload::Sha256 => bench_sha256(fixture_dir, out).await,
            Workload::ToyDecode => bench_toy_decode(fixture_dir, out).await,
        },
    }
}

// ---------------------------------------------------------------------------
// SHA-256 workload (M1)
// ---------------------------------------------------------------------------

async fn prove_sha256(
    input: PathBuf,
    min_size: u32,
    out: PathBuf,
    commit_out: Option<PathBuf>,
) -> Result<()> {
    let data = std::fs::read(&input).context("failed to read input file")?;
    let commitment: [u8; 32] = Sha256::digest(&data).into();

    if let Some(path) = &commit_out {
        std::fs::write(path, bytes_to_hex(&commitment)).context("failed to write commitment")?;
    }

    let mut stdin = SP1Stdin::new();
    stdin.write(&min_size);
    stdin.write(&data);

    let client = ProverClient::from_env().await;
    let pk = client.setup(SHA_ELF).await.context("setup failed")?;

    let t0 = Instant::now();
    let mut proof = client
        .prove(&pk, stdin)
        .core()
        .await
        .context("prove failed")?;
    let prove_ms = t0.elapsed().as_millis() as u64;

    let pv_digest: [u8; 32] = proof.public_values.read();
    let pv_min_size: u32 = proof.public_values.read();
    anyhow::ensure!(pv_digest == commitment, "public value digest mismatch");
    anyhow::ensure!(pv_min_size == min_size, "public value min_size mismatch");

    client
        .verify(&proof, pk.verifying_key(), None)
        .context("verification failed")?;

    proof.save(&out).context("failed to save proof")?;

    println!("proved in {} ms", prove_ms);
    println!("proof saved to {}", out.display());
    Ok(())
}

async fn verify_sha256(proof: PathBuf, commit: String, min_size: u32) -> Result<()> {
    let expected_commitment = hex_to_bytes(&commit)?;
    anyhow::ensure!(expected_commitment.len() == 32, "commitment must be 32 bytes");

    let mut proof =
        SP1ProofWithPublicValues::load(&proof).context("failed to load proof")?;
    let client = ProverClient::from_env().await;
    let pk = client.setup(SHA_ELF).await.context("setup failed")?;

    let t0 = Instant::now();
    client
        .verify(&proof, pk.verifying_key(), None)
        .context("verification failed")?;
    let verify_ms = t0.elapsed().as_millis() as u64;

    let pv_digest: [u8; 32] = proof.public_values.read();
    let pv_min_size: u32 = proof.public_values.read();
    anyhow::ensure!(pv_digest == *expected_commitment.as_slice(), "digest mismatch");
    anyhow::ensure!(pv_min_size == min_size, "min_size mismatch");

    println!("verified in {} ms", verify_ms);
    Ok(())
}

async fn bench_sha256(fixture_dir: PathBuf, out: Option<PathBuf>) -> Result<()> {
    let client = ProverClient::from_env().await;
    let pk = client.setup(SHA_ELF).await.context("setup failed")?;

    let sizes = [("1k", 1024usize), ("1m", 1_048_576), ("10m", 10_485_760)];

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

        let (_, report) = client
            .execute(SHA_ELF, stdin.clone())
            .await
            .context("execute failed")?;

        let t0 = Instant::now();
        let proof = client
            .prove(&pk, stdin.clone())
            .core()
            .await
            .context("prove failed")?;
        let prove_ms = t0.elapsed().as_millis() as u64;

        let t1 = Instant::now();
        client
            .verify(&proof, pk.verifying_key(), None)
            .context("verify failed")?;
        let verify_ms = t1.elapsed().as_millis() as u64;

        let proof_path = std::env::temp_dir().join(format!("sp1-bench-{}.bin", label));
        proof.save(&proof_path).context("save bench proof")?;
        let proof_bytes = std::fs::metadata(&proof_path)?.len() as usize;
        let _ = std::fs::remove_file(&proof_path);

        partial.rows.push(Row {
            size_label: label.to_string(),
            size_bytes: data.len(),
            cycles: report.total_instruction_count(),
            prove_ms,
            verify_native_ms: verify_ms,
            proof_bytes,
            peak_rss_bytes: get_peak_rss(),
        });

        println!(
            "{}: {} cycles, prove {} ms, verify {} ms, proof {} bytes",
            label,
            report.total_instruction_count(),
            prove_ms,
            verify_ms,
            proof_bytes
        );
        let _ = std::io::stdout().flush();

        if let Some(path) = out.as_ref() {
            write_partial(path, &partial)?;
        }
    }

    if let Some(path) = out.as_ref() {
        println!("wrote {}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&partial)?);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Toy-decode workload (M1b 3-way parity / M2 codec kernel)
// ---------------------------------------------------------------------------

async fn prove_toy_decode(
    input: PathBuf,
    out: PathBuf,
    commit_out: Option<PathBuf>,
) -> Result<()> {
    let data = std::fs::read(&input).context("failed to read input file")?;
    let frame = build_toy_frame(&data);
    let commitment = toy_native_commitment(&frame)?;

    if let Some(path) = &commit_out {
        std::fs::write(path, bytes_to_hex(&commitment)).context("failed to write commitment")?;
    }

    let mut stdin = SP1Stdin::new();
    write_toy_stdin(&mut stdin, &frame)?;

    let client = ProverClient::from_env().await;
    let pk = client.setup(TOY_ELF).await.context("setup failed")?;

    let t0 = Instant::now();
    let mut proof = client
        .prove(&pk, stdin)
        .core()
        .await
        .context("prove failed")?;
    let prove_ms = t0.elapsed().as_millis() as u64;

    let pv_digest: [u8; 32] = proof.public_values.read();
    let pv_width: u16 = proof.public_values.read();
    let pv_height: u16 = proof.public_values.read();
    anyhow::ensure!(pv_digest == commitment, "digest mismatch (host vs guest)");
    anyhow::ensure!(pv_width == frame.width, "width mismatch");
    anyhow::ensure!(pv_height == frame.height, "height mismatch");

    client
        .verify(&proof, pk.verifying_key(), None)
        .context("verification failed")?;

    proof.save(&out).context("failed to save proof")?;
    println!("proved in {} ms", prove_ms);
    println!("commitment={}", bytes_to_hex(&commitment));
    Ok(())
}

async fn verify_toy_decode(proof: PathBuf, commit: String) -> Result<()> {
    let expected_commitment = hex_to_bytes(&commit)?;
    anyhow::ensure!(expected_commitment.len() == 32, "commitment must be 32 bytes");

    let mut proof =
        SP1ProofWithPublicValues::load(&proof).context("failed to load proof")?;
    let client = ProverClient::from_env().await;
    let pk = client.setup(TOY_ELF).await.context("setup failed")?;

    let t0 = Instant::now();
    client
        .verify(&proof, pk.verifying_key(), None)
        .context("verification failed")?;
    let verify_ms = t0.elapsed().as_millis() as u64;

    let pv_digest: [u8; 32] = proof.public_values.read();
    anyhow::ensure!(
        pv_digest == *expected_commitment.as_slice(),
        "digest mismatch"
    );
    println!("verified in {} ms", verify_ms);
    Ok(())
}

async fn bench_toy_decode(_fixture_dir: PathBuf, out: Option<PathBuf>) -> Result<()> {
    let client = ProverClient::from_env().await;
    let pk = client.setup(TOY_ELF).await.context("setup failed")?;

    // Synthetic 16x16 4:2:0 ramp, identical to the Jolt toy-decode bench.
    let data: Vec<u8> = (0..384).map(|i| (i & 0xff) as u8).collect();
    let frame = build_toy_frame(&data);

    let mut stdin = SP1Stdin::new();
    write_toy_stdin(&mut stdin, &frame)?;

    let (_, report) = client
        .execute(TOY_ELF, stdin.clone())
        .await
        .context("execute failed")?;

    let t0 = Instant::now();
    let proof = client
        .prove(&pk, stdin.clone())
        .core()
        .await
        .context("prove failed")?;
    let prove_ms = t0.elapsed().as_millis() as u64;

    let t1 = Instant::now();
    client
        .verify(&proof, pk.verifying_key(), None)
        .context("verify failed")?;
    let verify_ms = t1.elapsed().as_millis() as u64;

    let proof_path = std::env::temp_dir().join("sp1-bench-toy-decode.bin");
    proof.save(&proof_path).context("save bench proof")?;
    let proof_bytes = std::fs::metadata(&proof_path)?.len() as usize;
    let _ = std::fs::remove_file(&proof_path);

    let row = Row {
        size_label: "16x16".to_string(),
        size_bytes: 384,
        cycles: report.total_instruction_count(),
        prove_ms,
        verify_native_ms: verify_ms,
        proof_bytes,
        peak_rss_bytes: get_peak_rss(),
    };

    let result = BenchResult {
        system: "sp1-toy-decode".to_string(),
        toolchain: format!("sp1-{}", env!("CARGO_PKG_VERSION")),
        gpu: None,
        verifier_wasm_gz_bytes: None,
        verify_browser_ms: None,
        rows: vec![row],
    };

    println!(
        "16x16: {} cycles, prove {} ms, verify {} ms, proof {} bytes",
        report.total_instruction_count(),
        prove_ms,
        verify_ms,
        proof_bytes
    );
    let _ = std::io::stdout().flush();

    if let Some(path) = out {
        write_partial(&path, &result)?;
        println!("wrote {}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
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
