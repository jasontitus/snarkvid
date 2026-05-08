// Sonobe (Nova+CycleFold) host driver for the M1b spike.
//
// CLI is shaped to match the SP1/RISC0 hosts so bench/run.sh can call
// them interchangeably:
//
//   sonobe-script prove   --workload <sha256-chain|toy-decode>
//                         --input <fixture> --min-size <N>
//                         --out proof.bin --commit-out commit.hex
//                         [--max-steps N]
//
//   sonobe-script verify  --workload <...> --proof proof.bin
//                         --commit <hex> --min-size <N>
//
//   sonobe-script bench   --workload <...> --fixture-dir <dir>
//                         --out bench.json [--max-steps N]
//
// "cycles" in the BenchResult JSON is overloaded for Sonobe — there is
// no zkVM cycle counter, so we record the number of fold steps. The
// milestone report calls this out explicitly.

use anyhow::{Context, Result};
use ark_bn254::{Bn254, Fr, G1Projective as Projective};
use ark_groth16::Groth16;
use ark_grumpkin::Projective as Projective2;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use clap::{Parser, Subcommand, ValueEnum};
use folding_schemes::commitment::{kzg::KZG, pedersen::Pedersen};
use folding_schemes::folding::nova::decider_eth::Decider as DeciderEth;
use folding_schemes::folding::nova::{Nova, PreprocessorParam};
use folding_schemes::folding::traits::CommittedInstanceOps;
use folding_schemes::frontend::FCircuit;
use folding_schemes::transcript::poseidon::poseidon_canonical_config;
use folding_schemes::{Decider as DeciderTrait, FoldingScheme};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::time::Instant;

mod sha256_circuit;
mod toy_decode_circuit;

use sha256_circuit::Sha256FCircuit;
use toy_decode_circuit::{ToyDecodeExt, ToyDecodeFCircuit};

// Type aliases for the two Nova instantiations we use. Same curve cycle
// (BN254/Grumpkin) and commitment schemes for both — only the
// step-circuit type differs.
type NovaSha = Nova<
    Projective,
    Projective2,
    Sha256FCircuit<Fr>,
    KZG<'static, Bn254>,
    Pedersen<Projective2>,
    false,
>;
type NovaToy = Nova<
    Projective,
    Projective2,
    ToyDecodeFCircuit<Fr>,
    KZG<'static, Bn254>,
    Pedersen<Projective2>,
    false,
>;

// DeciderEth wraps the IVC accumulator into a Groth16/BN254 proof
// (~200 bytes, verifiable by any standard Groth16 verifier including
// off-the-shelf WASM ones). This is the load-bearing browser-verify path.
type DeciderSha = DeciderEth<
    Projective,
    Projective2,
    Sha256FCircuit<Fr>,
    KZG<'static, Bn254>,
    Pedersen<Projective2>,
    Groth16<Bn254>,
    NovaSha,
>;
type DeciderToy = DeciderEth<
    Projective,
    Projective2,
    ToyDecodeFCircuit<Fr>,
    KZG<'static, Bn254>,
    Pedersen<Projective2>,
    Groth16<Bn254>,
    NovaToy,
>;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Workload {
    /// z_{i+1} = SHA256(z_i). num_steps = ceil(input_len / 32).
    Sha256Chain,
    /// Per-coefficient clamp folding. num_steps = input_len bytes.
    ToyDecode,
}

#[derive(Parser)]
#[command(name = "sonobe-script")]
#[command(about = "Sonobe (Nova+CycleFold) side of the snarkvid M1b spike")]
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
        /// Cap fold-step count for sane CPU benches. Each step is real
        /// work; a 1MB SHA-256 fixture would otherwise need ~32k steps.
        #[arg(long, default_value_t = 1024)]
        max_steps: usize,
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
        #[arg(long, default_value_t = 1024)]
        max_steps: usize,
        /// Run DeciderEth on top of the IVC accumulator to produce a
        /// Groth16/BN254 proof (~200 bytes). This is the load-bearing
        /// browser-verifier experiment. Decider proving + preprocessing
        /// is heavy (single-shot Groth16 over a large constraint system)
        /// so this is gated off by default.
        #[arg(long, default_value_t = false)]
        decider: bool,
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
    /// For Sonobe this is num_fold_steps (no zkVM cycle counter).
    cycles: u64,
    prove_ms: u64,
    verify_native_ms: u64,
    proof_bytes: usize,
    peak_rss_bytes: usize,
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{:02x}", b);
        s
    })
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    let hex = hex.trim().strip_prefix("0x").unwrap_or(hex);
    anyhow::ensure!(hex.len() % 2 == 0, "hex must be even-length");
    let mut out = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        out.push(u8::from_str_radix(&hex[i..i + 2], 16)?);
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

// ---------------------------------------------------------------------------
// SHA-256 chain workload
// ---------------------------------------------------------------------------

/// Number of fold steps for an `n`-byte fixture under the chain workload.
/// One step rehashes a 32-byte digest, so we cover roughly len/32 bytes
/// of effective SHA-256 work. Capped by `max_steps` so 10MB fixtures
/// don't blow up the wall-clock; the report flags the cap.
fn sha_steps(len: usize, max_steps: usize) -> usize {
    ((len + 31) / 32).max(1).min(max_steps)
}

/// Native chain reference: z_{i+1} = first_field_element(SHA256(z_i.to_bytes_le())).
/// We mirror exactly what the in-circuit gadget computes so the reported
/// commitment is reproducible without running a prover.
fn sha_chain_native(initial: Fr, num_steps: usize) -> Fr {
    use ark_ff::{BigInteger, PrimeField, ToConstraintField};
    let mut z = initial;
    for _ in 0..num_steps {
        let mut bytes = z.into_bigint().to_bytes_le();
        // Match arkworks Sha256Gadget input encoding (no padding).
        let digest = Sha256::digest(&bytes);
        bytes.clear();
        bytes.extend_from_slice(&digest);
        let fields: Vec<Fr> = bytes.to_field_elements().unwrap_or_default();
        z = fields[0];
    }
    z
}

/// Output of a Nova IVC run that we may want to feed into the Decider.
struct ShaIvcRun {
    /// Folding scheme instance after `num_steps` proves; consumed by Decider.
    nova: NovaSha,
    nova_params: <NovaSha as FoldingScheme<Projective, Projective2, Sha256FCircuit<Fr>>>::ProverParam,
    nova_vp: <NovaSha as FoldingScheme<Projective, Projective2, Sha256FCircuit<Fr>>>::VerifierParam,
    f_circuit: Sha256FCircuit<Fr>,
    final_z: Fr,
    ivc_proof_bytes: Vec<u8>,
    prove_ms: u64,
    verify_ms: u64,
}

fn run_sha_chain_full(num_steps: usize) -> Result<ShaIvcRun> {
    let initial_state = vec![Fr::from(1u32)];
    let f_circuit = Sha256FCircuit::<Fr>::new(())?;

    let poseidon_config = poseidon_canonical_config::<Fr>();
    let mut rng = rand::rngs::OsRng;

    let pp = PreprocessorParam::new(poseidon_config, f_circuit);
    let nova_params = NovaSha::preprocess(&mut rng, &pp)?;

    let mut fs = NovaSha::init(&nova_params, f_circuit, initial_state)?;

    let t_prove = Instant::now();
    for _ in 0..num_steps {
        fs.prove_step(rng, (), None)?;
    }
    let prove_ms = t_prove.elapsed().as_millis() as u64;

    let ivc_proof = fs.ivc_proof();
    let final_z = ivc_proof.z_i[0];

    let t_verify = Instant::now();
    NovaSha::verify(nova_params.1.clone(), ivc_proof.clone())?;
    let verify_ms = t_verify.elapsed().as_millis() as u64;

    let mut buf = Vec::new();
    ivc_proof.serialize_compressed(&mut buf)?;

    Ok(ShaIvcRun {
        nova: fs,
        nova_params: nova_params.0,
        nova_vp: nova_params.1,
        f_circuit,
        final_z,
        ivc_proof_bytes: buf,
        prove_ms,
        verify_ms,
    })
}

fn run_sha_chain(
    num_steps: usize,
    out_proof: Option<&Path>,
) -> Result<(Vec<u8>, Fr, u64, u64)> {
    let r = run_sha_chain_full(num_steps)?;
    if let Some(p) = out_proof {
        std::fs::write(p, &r.ivc_proof_bytes)?;
    }
    Ok((r.ivc_proof_bytes, r.final_z, r.prove_ms, r.verify_ms))
}

/// Numbers from a Decider (Groth16/BN254 wrap) run on top of an IVC accumulator.
struct DeciderRun {
    proof_bytes: usize,
    decider_prove_ms: u64,
    decider_verify_ms: u64,
    decider_setup_ms: u64,
}

fn run_decider_sha(r: ShaIvcRun) -> Result<DeciderRun> {
    let mut rng = rand::rngs::OsRng;
    let t_setup = Instant::now();
    let (decider_pp, decider_vp) = DeciderSha::preprocess(
        &mut rng,
        ((r.nova_params, r.nova_vp.clone()), r.f_circuit.state_len()),
    )?;
    let decider_setup_ms = t_setup.elapsed().as_millis() as u64;

    let t_prove = Instant::now();
    let proof = DeciderSha::prove(rng, decider_pp, r.nova.clone())?;
    let decider_prove_ms = t_prove.elapsed().as_millis() as u64;

    let mut buf = Vec::new();
    proof.serialize_compressed(&mut buf)?;
    let proof_bytes = buf.len();

    let t_verify = Instant::now();
    let ok = DeciderSha::verify(
        decider_vp,
        r.nova.i,
        r.nova.z_0.clone(),
        r.nova.z_i.clone(),
        &r.nova.U_i.get_commitments(),
        &r.nova.u_i.get_commitments(),
        &proof,
    )?;
    let decider_verify_ms = t_verify.elapsed().as_millis() as u64;
    anyhow::ensure!(ok, "DeciderEth verify returned false");

    Ok(DeciderRun {
        proof_bytes,
        decider_prove_ms,
        decider_verify_ms,
        decider_setup_ms,
    })
}

// ---------------------------------------------------------------------------
// Toy-decode workload
// ---------------------------------------------------------------------------

/// Native reference: clamp the i16 representation of `coeff_u16` to [0,255].
/// Used to compute the public commitment without running a prover.
fn clamp_native(coeff_u16: u16) -> u8 {
    let signed = coeff_u16 as i16;
    if signed < 0 {
        0
    } else if signed > 255 {
        255
    } else {
        signed as u8
    }
}

struct ToyIvcRun {
    nova: NovaToy,
    nova_params: <NovaToy as FoldingScheme<Projective, Projective2, ToyDecodeFCircuit<Fr>>>::ProverParam,
    nova_vp: <NovaToy as FoldingScheme<Projective, Projective2, ToyDecodeFCircuit<Fr>>>::VerifierParam,
    f_circuit: ToyDecodeFCircuit<Fr>,
    final_state: (Fr, Fr),
    ivc_proof_bytes: Vec<u8>,
    prove_ms: u64,
    verify_ms: u64,
}

fn run_toy_decode_full(coeffs: &[u16]) -> Result<ToyIvcRun> {
    let initial_state = vec![Fr::from(0u32), Fr::from(0u32)];
    let f_circuit = ToyDecodeFCircuit::<Fr>::new(())?;

    let poseidon_config = poseidon_canonical_config::<Fr>();
    let mut rng = rand::rngs::OsRng;

    let pp = PreprocessorParam::new(poseidon_config, f_circuit);
    let nova_params = NovaToy::preprocess(&mut rng, &pp)?;

    let mut fs = NovaToy::init(&nova_params, f_circuit, initial_state)?;

    let t_prove = Instant::now();
    for &c in coeffs {
        let ext = ToyDecodeExt { coeff_u16: c };
        fs.prove_step(rng, ext, None)?;
    }
    let prove_ms = t_prove.elapsed().as_millis() as u64;

    let ivc_proof = fs.ivc_proof();
    let final_state = (ivc_proof.z_i[0], ivc_proof.z_i[1]);

    let t_verify = Instant::now();
    NovaToy::verify(nova_params.1.clone(), ivc_proof.clone())?;
    let verify_ms = t_verify.elapsed().as_millis() as u64;

    let mut buf = Vec::new();
    ivc_proof.serialize_compressed(&mut buf)?;

    Ok(ToyIvcRun {
        nova: fs,
        nova_params: nova_params.0,
        nova_vp: nova_params.1,
        f_circuit,
        final_state,
        ivc_proof_bytes: buf,
        prove_ms,
        verify_ms,
    })
}

fn run_toy_decode(
    coeffs: &[u16],
    out_proof: Option<&Path>,
) -> Result<(Vec<u8>, (Fr, Fr), u64, u64)> {
    let r = run_toy_decode_full(coeffs)?;
    if let Some(p) = out_proof {
        std::fs::write(p, &r.ivc_proof_bytes)?;
    }
    Ok((r.ivc_proof_bytes, r.final_state, r.prove_ms, r.verify_ms))
}

fn run_decider_toy(r: ToyIvcRun) -> Result<DeciderRun> {
    let mut rng = rand::rngs::OsRng;
    let t_setup = Instant::now();
    let (decider_pp, decider_vp) = DeciderToy::preprocess(
        &mut rng,
        ((r.nova_params, r.nova_vp.clone()), r.f_circuit.state_len()),
    )?;
    let decider_setup_ms = t_setup.elapsed().as_millis() as u64;

    let t_prove = Instant::now();
    let proof = DeciderToy::prove(rng, decider_pp, r.nova.clone())?;
    let decider_prove_ms = t_prove.elapsed().as_millis() as u64;

    let mut buf = Vec::new();
    proof.serialize_compressed(&mut buf)?;
    let proof_bytes = buf.len();

    let t_verify = Instant::now();
    let ok = DeciderToy::verify(
        decider_vp,
        r.nova.i,
        r.nova.z_0.clone(),
        r.nova.z_i.clone(),
        &r.nova.U_i.get_commitments(),
        &r.nova.u_i.get_commitments(),
        &proof,
    )?;
    let decider_verify_ms = t_verify.elapsed().as_millis() as u64;
    anyhow::ensure!(ok, "DeciderEth verify returned false");

    Ok(DeciderRun {
        proof_bytes,
        decider_prove_ms,
        decider_verify_ms,
        decider_setup_ms,
    })
}

fn fr_to_hex(f: &Fr) -> String {
    use ark_ff::{BigInteger, PrimeField};
    let bytes = f.into_bigint().to_bytes_be();
    bytes_to_hex(&bytes)
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
    max_steps: usize,
) -> Result<()> {
    let data = std::fs::read(&input).context("read input")?;
    anyhow::ensure!(
        data.len() as u32 >= min_size,
        "input shorter than min_size",
    );

    match workload {
        Workload::Sha256Chain => {
            let n = sha_steps(data.len(), max_steps);
            println!("sha256-chain: {} fold steps (data {} bytes)", n, data.len());
            let (_buf, final_z, prove_ms, verify_ms) = run_sha_chain(n, Some(&out))?;
            let commitment = fr_to_hex(&final_z);
            if let Some(p) = commit_out {
                std::fs::write(&p, &commitment).context("write commitment")?;
            }
            println!("proved in {} ms, verified in {} ms", prove_ms, verify_ms);
            println!("commitment={}", commitment);
        }
        Workload::ToyDecode => {
            // Each input byte is interpreted as u16 (high byte zero).
            // For a real codec fixture this would be i16 coefficients
            // packed little-endian; we keep it simple here.
            let coeffs: Vec<u16> =
                data.iter().take(max_steps).map(|&b| b as u16).collect();
            println!(
                "toy-decode: {} fold steps (data {} bytes)",
                coeffs.len(),
                data.len()
            );
            let (_buf, (s0, s1), prove_ms, verify_ms) =
                run_toy_decode(&coeffs, Some(&out))?;
            let commitment = format!("{}:{}", fr_to_hex(&s0), fr_to_hex(&s1));
            if let Some(p) = commit_out {
                std::fs::write(&p, &commitment).context("write commitment")?;
            }
            println!("proved in {} ms, verified in {} ms", prove_ms, verify_ms);
            println!("commitment={}", commitment);
        }
    }
    Ok(())
}

fn cmd_verify(
    workload: Workload,
    proof: PathBuf,
    _commit: String,
    _min_size: u32,
) -> Result<()> {
    // Reload the IVC proof + re-run the verifier. We don't redo
    // preprocessing here because nova_params.1 (verifier params) lives
    // in the proof's params bundle in production; for the spike we
    // re-derive them on the fly. This makes verify slower than it would
    // be in a real verifier — flagged in the report.
    let bytes = std::fs::read(&proof).context("read proof")?;

    match workload {
        Workload::Sha256Chain => {
            use folding_schemes::folding::nova::IVCProof;
            let f_circuit = Sha256FCircuit::<Fr>::new(())?;
            let poseidon_config = poseidon_canonical_config::<Fr>();
            let mut rng = rand::rngs::OsRng;
            let pp = PreprocessorParam::new(poseidon_config, f_circuit);
            let nova_params = NovaSha::preprocess(&mut rng, &pp)?;
            let ivc_proof: IVCProof<Projective, Projective2> =
                IVCProof::deserialize_compressed(&bytes[..])?;
            let t = Instant::now();
            NovaSha::verify(nova_params.1, ivc_proof)?;
            println!("verified in {} ms", t.elapsed().as_millis());
        }
        Workload::ToyDecode => {
            use folding_schemes::folding::nova::IVCProof;
            let f_circuit = ToyDecodeFCircuit::<Fr>::new(())?;
            let poseidon_config = poseidon_canonical_config::<Fr>();
            let mut rng = rand::rngs::OsRng;
            let pp = PreprocessorParam::new(poseidon_config, f_circuit);
            let nova_params = NovaToy::preprocess(&mut rng, &pp)?;
            let ivc_proof: IVCProof<Projective, Projective2> =
                IVCProof::deserialize_compressed(&bytes[..])?;
            let t = Instant::now();
            NovaToy::verify(nova_params.1, ivc_proof)?;
            println!("verified in {} ms", t.elapsed().as_millis());
        }
    }
    Ok(())
}

fn cmd_bench(
    workload: Workload,
    fixture_dir: PathBuf,
    out: Option<PathBuf>,
    max_steps: usize,
    decider: bool,
) -> Result<()> {
    let sizes = [("1k", 1024usize), ("1m", 1_048_576), ("10m", 10_485_760)];

    let system_base = match workload {
        Workload::Sha256Chain => "sonobe-nova-sha256-chain",
        Workload::ToyDecode => "sonobe-nova-toy-decode",
    };
    let system = if decider {
        format!("{}-decider", system_base)
    } else {
        system_base.to_string()
    };

    let mut partial = BenchResult {
        system,
        toolchain: format!("sonobe-{}", env!("CARGO_PKG_VERSION")),
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

        // Run the IVC, optionally fold the Decider step on top.
        let (ivc_proof_bytes_len, ivc_prove_ms, ivc_verify_ms, num_steps, decider_run) =
            match workload {
                Workload::Sha256Chain => {
                    let n = sha_steps(data.len(), max_steps);
                    let r = run_sha_chain_full(n)?;
                    let dr = if decider {
                        Some(run_decider_sha(ShaIvcRun {
                            nova: r.nova.clone(),
                            nova_params: r.nova_params.clone(),
                            nova_vp: r.nova_vp.clone(),
                            f_circuit: r.f_circuit,
                            final_z: r.final_z,
                            ivc_proof_bytes: r.ivc_proof_bytes.clone(),
                            prove_ms: r.prove_ms,
                            verify_ms: r.verify_ms,
                        })?)
                    } else {
                        None
                    };
                    (r.ivc_proof_bytes.len(), r.prove_ms, r.verify_ms, n, dr)
                }
                Workload::ToyDecode => {
                    let coeffs: Vec<u16> =
                        data.iter().take(max_steps).map(|&b| b as u16).collect();
                    let n = coeffs.len();
                    let r = run_toy_decode_full(&coeffs)?;
                    let dr = if decider {
                        Some(run_decider_toy(ToyIvcRun {
                            nova: r.nova.clone(),
                            nova_params: r.nova_params.clone(),
                            nova_vp: r.nova_vp.clone(),
                            f_circuit: r.f_circuit,
                            final_state: r.final_state,
                            ivc_proof_bytes: r.ivc_proof_bytes.clone(),
                            prove_ms: r.prove_ms,
                            verify_ms: r.verify_ms,
                        })?)
                    } else {
                        None
                    };
                    (r.ivc_proof_bytes.len(), r.prove_ms, r.verify_ms, n, dr)
                }
            };

        // When --decider is set, the row reports Decider numbers (final
        // proof bytes + Groth16 verify ms); the IVC times are absorbed
        // into prove_ms (= IVC prove + Decider prove).
        let (prove_ms, verify_ms, proof_bytes) = if let Some(d) = decider_run {
            (
                ivc_prove_ms + d.decider_prove_ms + d.decider_setup_ms,
                d.decider_verify_ms,
                d.proof_bytes,
            )
        } else {
            (ivc_prove_ms, ivc_verify_ms, ivc_proof_bytes_len)
        };

        partial.rows.push(Row {
            size_label: label.to_string(),
            size_bytes: data.len(),
            cycles: num_steps as u64,
            prove_ms,
            verify_native_ms: verify_ms,
            proof_bytes,
            peak_rss_bytes: get_peak_rss(),
        });

        println!(
            "{}: {} steps, prove {} ms, verify {} ms, proof {} bytes",
            label, num_steps, prove_ms, verify_ms, proof_bytes
        );
        let _ = std::io::stdout().flush();

        if let Some(p) = out.as_ref() {
            write_partial(p, &partial)
                .with_context(|| format!("write partial {}", p.display()))?;
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Prove {
            workload,
            input,
            min_size,
            out,
            commit_out,
            max_steps,
        } => cmd_prove(workload, input, min_size, out, commit_out, max_steps),
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
            max_steps,
            decider,
        } => cmd_bench(workload, fixture_dir, out, max_steps, decider),
    }
}
