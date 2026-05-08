// Jolt host driver. Mirrors the SP1/RISC0 host CLIs so bench scripts
// can call them interchangeably:
//
//   jolt-script prove   --workload <sha256|toy-decode>
//                       --input <fixture> --min-size <N>
//                       --out proof.bin --commit-out commit.hex
//   jolt-script verify  --workload <...> --proof proof.bin
//                       --commit <hex> --min-size <N>
//   jolt-script bench   --workload <...> --fixture-dir <dir>
//                       --out bench.json
//
// Bench output JSON matches the SP1/RISC0 BenchResult schema in
// spike/bench/results/{sp1,risc0}.json so the same compare.py works.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use snarkvid_toy_codec::{decode_toy, BqBitstream, BqHeader};

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Workload {
    /// SHA-256 preimage of N bytes; same statement as M1 spike.
    Sha256,
    /// Run decode_toy on a single 16x16 4:2:0 frame (384 bytes input).
    ToyDecode,
}

#[derive(Parser)]
#[command(name = "jolt-script")]
#[command(about = "Jolt side of the snarkvid M1b spike")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Prove {
        #[arg(long, value_enum)]
        workload: Workload,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        min_size: u32,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        commit_out: Option<PathBuf>,
    },
    Verify {
        #[arg(long, value_enum)]
        workload: Workload,
        #[arg(long)]
        proof: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        min_size: u32,
    },
    Bench {
        #[arg(long, value_enum)]
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

fn bytes_to_hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, byte| {
        let _ = write!(s, "{:02x}", byte);
        s
    })
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    let h = hex.trim().strip_prefix("0x").unwrap_or(hex);
    anyhow::ensure!(h.len() % 2 == 0, "hex must be even-length");
    let mut out = Vec::with_capacity(h.len() / 2);
    for i in (0..h.len()).step_by(2) {
        out.push(u8::from_str_radix(&h[i..i + 2], 16)?);
    }
    Ok(out)
}

fn get_peak_rss() -> usize {
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                if let Some(kb_str) = rest.split_whitespace().next() {
                    if let Ok(kb) = kb_str.parse::<usize>() {
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
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

const TARGET_DIR: &str = "/tmp/jolt-guest-targets";

// ---------------------------------------------------------------------------
// SHA-256 workload
// ---------------------------------------------------------------------------

/// Returns (proof_bytes, prove_ms, verify_ms, cycles_or_zero).
///
/// `cycles` for Jolt comes from RUST_LOG=info traces; the prove_ms call
/// here doesn't expose it programmatically yet. We record 0 in the JSON
/// and rely on the user's logs to read cycle counts. The milestone
/// report flags this.
fn run_sha256_prove(
    data: &[u8],
    min_size: u32,
    out_proof: Option<&Path>,
) -> Result<(Vec<u8>, u64, u64, u64)> {
    let mut program = guest::compile_sha2_preimage(TARGET_DIR);

    let shared_pp = guest::preprocess_shared_sha2_preimage(&mut program)
        .context("preprocess_shared failed")?;
    let prover_pp = guest::preprocess_prover_sha2_preimage(shared_pp.clone());
    let verifier_pp = guest::preprocess_verifier_sha2_preimage(
        shared_pp,
        prover_pp.generators.to_verifier_setup(),
        None,
    );

    let prove = guest::build_prover_sha2_preimage(program, prover_pp);
    let verify = guest::build_verifier_sha2_preimage(verifier_pp);

    let t = Instant::now();
    let (output, proof, program_io) = prove(min_size, data);
    let prove_ms = t.elapsed().as_millis() as u64;

    // Sanity-check the output matches a native SHA-256.
    let expected_digest: [u8; 32] = Sha256::digest(data).into();
    anyhow::ensure!(
        output.0 == expected_digest && output.1 == min_size,
        "guest output mismatch (expected {:x?}, got {:x?})",
        expected_digest,
        output.0
    );

    // Jolt's `JoltProof` type doesn't implement Clone or serde::Serialize
    // in May 2026 — the underlying ark_serialize::CanonicalSerialize impl
    // exists but isn't exposed in the public type. Until it's wired,
    // verify consumes the proof and we record proof_bytes=0 in the JSON.
    // The milestone report flags this as a known gap.
    let _ = out_proof;
    let t = Instant::now();
    let is_valid = verify(min_size, data, output, program_io.panic, proof);
    let verify_ms = t.elapsed().as_millis() as u64;
    anyhow::ensure!(is_valid, "verify returned false");

    Ok((Vec::new(), prove_ms, verify_ms, 0))
}

// ---------------------------------------------------------------------------
// Toy-decode workload
// ---------------------------------------------------------------------------

fn run_toy_decode_prove(
    yuv_bytes: &[u8],
    out_proof: Option<&Path>,
) -> Result<(Vec<u8>, u64, u64, u64)> {
    anyhow::ensure!(
        yuv_bytes.len() == 384,
        "toy-decode workload expects 384 bytes (16x16 4:2:0); got {}",
        yuv_bytes.len()
    );

    let mut program = guest_toy_decode::compile_toy_decode_one_block(TARGET_DIR);
    let shared_pp = guest_toy_decode::preprocess_shared_toy_decode_one_block(&mut program)
        .context("preprocess_shared failed")?;
    let prover_pp = guest_toy_decode::preprocess_prover_toy_decode_one_block(shared_pp.clone());
    let verifier_pp = guest_toy_decode::preprocess_verifier_toy_decode_one_block(
        shared_pp,
        prover_pp.generators.to_verifier_setup(),
        None,
    );

    let prove = guest_toy_decode::build_prover_toy_decode_one_block(program, prover_pp);
    let verify = guest_toy_decode::build_verifier_toy_decode_one_block(verifier_pp);

    let _ = out_proof;
    let t = Instant::now();
    let (output, proof, program_io) = prove(yuv_bytes);
    let prove_ms = t.elapsed().as_millis() as u64;

    // Compute the same decode + digest natively for cross-check.
    let header = BqHeader {
        width: 16,
        height: 16,
        qp: 0,
        chroma_format: 1,
    };
    let coeffs_y: Vec<i16> = yuv_bytes[0..256].iter().map(|&b| b as i16).collect();
    let coeffs_u: Vec<i16> = yuv_bytes[256..320].iter().map(|&b| b as i16).collect();
    let coeffs_v: Vec<i16> = yuv_bytes[320..384].iter().map(|&b| b as i16).collect();
    let bitstream = BqBitstream {
        header,
        coeffs_y,
        coeffs_u,
        coeffs_v,
    };
    let frame = decode_toy(&bitstream).context("native decode_toy failed")?;
    let mut h = Sha256::new();
    h.update(&frame.y);
    h.update(&frame.u);
    h.update(&frame.v);
    let expected: [u8; 32] = h.finalize().into();

    anyhow::ensure!(
        output == expected,
        "guest decode digest mismatch: native={} guest={}",
        bytes_to_hex(&expected),
        bytes_to_hex(&output)
    );

    let t = Instant::now();
    let is_valid = verify(yuv_bytes, output, program_io.panic, proof);
    let verify_ms = t.elapsed().as_millis() as u64;
    anyhow::ensure!(is_valid, "verify returned false");

    Ok((Vec::new(), prove_ms, verify_ms, 0))
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_prove(
    workload: Workload,
    input: PathBuf,
    min_size: u32,
    out: PathBuf,
    commit_out: Option<PathBuf>,
) -> Result<()> {
    let data = std::fs::read(&input).context("read input")?;
    match workload {
        Workload::Sha256 => {
            let commitment: [u8; 32] = Sha256::digest(&data).into();
            if let Some(p) = &commit_out {
                std::fs::write(p, bytes_to_hex(&commitment))?;
            }
            let (_buf, p, v, _c) = run_sha256_prove(&data, min_size, Some(&out))?;
            println!(
                "proved in {} ms, verified in {} ms, commitment={}",
                p,
                v,
                bytes_to_hex(&commitment)
            );
        }
        Workload::ToyDecode => {
            // Pad / truncate to exactly 384 bytes
            let mut buf = data.clone();
            buf.resize(384, 0);
            let (_b, p, v, _c) = run_toy_decode_prove(&buf, Some(&out))?;
            println!("proved in {} ms, verified in {} ms", p, v);
        }
    }
    Ok(())
}

fn cmd_verify(
    workload: Workload,
    proof: PathBuf,
    commit: String,
    min_size: u32,
) -> Result<()> {
    let proof_bytes = std::fs::read(&proof).context("read proof")?;
    match workload {
        Workload::Sha256 => {
            let expected = hex_to_bytes(&commit)?;
            anyhow::ensure!(expected.len() == 32, "commit must be 32 bytes");
            // Re-running verify needs the original `data` (Jolt's verifier
            // takes the inputs alongside the proof). The CLI shape here
            // doesn't carry data through to verify on its own — for
            // milestone-1b we report the full prove+verify round-trip
            // numbers and skip standalone verify-from-proof. SP1 has the
            // same limitation in spike/sp1/script/src/main.rs Verify.
            eprintln!(
                "Jolt verify-from-proof requires the original input bytes \
                 alongside the proof. Run `prove` and capture the verify_ms it \
                 reports, or extend the CLI to carry `--input` through."
            );
            let _ = (proof_bytes, expected, min_size);
        }
        Workload::ToyDecode => {
            eprintln!(
                "Jolt verify-from-proof requires the original input bytes \
                 alongside the proof; see Sha256 branch."
            );
            let _ = proof_bytes;
        }
    }
    Ok(())
}

fn cmd_bench(workload: Workload, fixture_dir: PathBuf, out: Option<PathBuf>) -> Result<()> {
    // SHA-256 fixtures are 1k / 1m / 10m. Toy-decode uses a single 384-byte
    // synthetic fixture for now (one 16x16 frame).
    let sizes_sha = [("1k", 1024usize), ("1m", 1_048_576), ("10m", 10_485_760)];
    let toy_sizes = [("16x16", 384usize)];

    let system = match workload {
        Workload::Sha256 => "jolt-sha256",
        Workload::ToyDecode => "jolt-toy-decode",
    };

    let mut partial = BenchResult {
        system: system.to_string(),
        toolchain: format!("jolt-{}", env!("CARGO_PKG_VERSION")),
        gpu: None,
        verifier_wasm_gz_bytes: None,
        verify_browser_ms: None,
        rows: Vec::new(),
    };

    match workload {
        Workload::Sha256 => {
            for (label, _size) in sizes_sha {
                let fixture = fixture_dir.join(format!("fixture-{}.bin", label));
                if !fixture.exists() {
                    eprintln!("skipping {}: not found", fixture.display());
                    continue;
                }
                let data = std::fs::read(&fixture)
                    .with_context(|| format!("read {}", fixture.display()))?;
                let min_size = data.len() as u32;
                let (buf, prove_ms, verify_ms, cycles) =
                    run_sha256_prove(&data, min_size, None)?;
                partial.rows.push(Row {
                    size_label: label.to_string(),
                    size_bytes: data.len(),
                    cycles,
                    prove_ms,
                    verify_native_ms: verify_ms,
                    proof_bytes: buf.len(),
                    peak_rss_bytes: get_peak_rss(),
                });
                println!(
                    "{}: prove {} ms, verify {} ms, proof {} bytes",
                    label,
                    prove_ms,
                    verify_ms,
                    buf.len()
                );
                let _ = std::io::stdout().flush();
                if let Some(p) = out.as_ref() {
                    write_partial(p, &partial)?;
                }
            }
        }
        Workload::ToyDecode => {
            for (label, size) in toy_sizes {
                // Synthetic fixture: a deterministic ramp 0..size.
                let data: Vec<u8> = (0..size).map(|i| (i & 0xff) as u8).collect();
                let (buf, prove_ms, verify_ms, cycles) =
                    run_toy_decode_prove(&data, None)?;
                partial.rows.push(Row {
                    size_label: label.to_string(),
                    size_bytes: data.len(),
                    cycles,
                    prove_ms,
                    verify_native_ms: verify_ms,
                    proof_bytes: buf.len(),
                    peak_rss_bytes: get_peak_rss(),
                });
                println!(
                    "{}: prove {} ms, verify {} ms, proof {} bytes",
                    label,
                    prove_ms,
                    verify_ms,
                    buf.len()
                );
                let _ = std::io::stdout().flush();
                if let Some(p) = out.as_ref() {
                    write_partial(p, &partial)?;
                }
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
        Commands::Prove {
            workload,
            input,
            min_size,
            out,
            commit_out,
        } => cmd_prove(workload, input, min_size, out, commit_out),
        Commands::Verify {
            workload,
            proof,
            commit,
            min_size,
        } => cmd_verify(workload, proof, commit, min_size),
        Commands::Bench {
            workload,
            fixture_dir,
            out,
        } => cmd_bench(workload, fixture_dir, out),
    }
}
