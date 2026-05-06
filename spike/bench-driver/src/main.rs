// Bench driver: invokes both zkVM hosts in `bench` mode against the
// shared fixtures and concatenates their JSON output into one
// comparison.json. The actual prove/verify logic lives in each side's
// host crate; this file only orchestrates.
//
// Usage:
//   cargo run -p bench-driver -- --fixture-dir spike/common/bench-fixtures
//
// Falls back to whichever side is available. If neither is, prints a
// helpful error and exits.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(clap::Parser)]
#[command(name = "bench-driver")]
struct Cli {
    /// Directory containing fixture files
    #[arg(long, default_value = "common/bench-fixtures")]
    fixture_dir: PathBuf,

    /// Output directory for results
    #[arg(long, default_value = "bench/results")]
    out_dir: PathBuf,

    /// Only run a specific side
    #[arg(long)]
    only: Option<String>,
}

#[derive(serde::Serialize)]
struct Comparison {
    generated: String,
    risc0: Option<serde_json::Value>,
    sp1: Option<serde_json::Value>,
}

fn run_side(
    name: &str,
    manifest_path: &str,
    bin_name: &str,
    fixture_dir: &PathBuf,
    out_path: &PathBuf,
) -> Option<serde_json::Value> {
    let mut cmd = if name == "sp1" {
        // SP1 needs --release for the ELF embedding to work
        let mut c = Command::new("cargo");
        c.args([
            "run",
            "--release",
            "--manifest-path",
            manifest_path,
            "--",
        ]);
        c
    } else {
        let mut c = Command::new("cargo");
        c.args([
            "run",
            "--release",
            "--manifest-path",
            manifest_path,
            "--features",
            "risc0",
            "--",
        ]);
        c
    };

    cmd.arg("bench")
        .arg("--fixture-dir")
        .arg(fixture_dir)
        .arg("--out")
        .arg(out_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    println!(">>> {}: spawning {:?}", name, cmd);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("  {} FAILED to spawn: {}", name, e);
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("  {} FAILED (exit {}): {}", name, output.status, stderr);
        return None;
    }

    // Read the JSON output from the result file
    match std::fs::read_to_string(out_path) {
        Ok(json_str) => match serde_json::from_str::<serde_json::Value>(&json_str) {
            Ok(val) => {
                println!("  {} OK — {} rows", name, val["rows"].as_array().map_or(0, |r| r.len()));
                Some(val)
            }
            Err(e) => {
                eprintln!("  {} produced invalid JSON: {}", name, e);
                None
            }
        },
        Err(e) => {
            eprintln!("  {} output file not found: {}", name, e);
            None
        }
    }
}

fn main() {
    let cli = <Cli as clap::Parser>::parse();

    // Resolve paths relative to the workspace root (spike/)
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("bench-driver must be inside spike/")
        .to_path_buf();

    let fixture_dir = if cli.fixture_dir.is_absolute() {
        cli.fixture_dir
    } else {
        workspace_root.join(&cli.fixture_dir)
    };

    let out_dir = if cli.out_dir.is_absolute() {
        cli.out_dir
    } else {
        workspace_root.join(&cli.out_dir)
    };

    std::fs::create_dir_all(&out_dir).expect("failed to create output directory");

    let mut comparison = Comparison {
        generated: chrono::Utc::now().to_rfc3339(),
        risc0: None,
        sp1: None,
    };

    let run_sp1 = cli.only.as_deref().map_or(true, |o| o == "sp1");
    let run_risc0 = cli.only.as_deref().map_or(true, |o| o == "risc0");

    if run_sp1 {
        let sp1_manifest = workspace_root.join("sp1/script/Cargo.toml");
        let sp1_out = out_dir.join("sp1.json");
        comparison.sp1 = run_side("sp1", sp1_manifest.to_str().unwrap(), "sp1-script", &fixture_dir, &sp1_out);
    }

    if run_risc0 {
        let risc0_manifest = workspace_root.join("risc0/host/Cargo.toml");
        let risc0_out = out_dir.join("risc0.json");
        comparison.risc0 = run_side("risc0", risc0_manifest.to_str().unwrap(), "risc0-host", &fixture_dir, &risc0_out);
    }

    if comparison.risc0.is_none() && comparison.sp1.is_none() {
        eprintln!("Neither zkVM side produced results.");
        eprintln!("Ensure at least one side's toolchain is installed and the guest program builds.");
        std::process::exit(2);
    }

    // Write comparison.json
    let comparison_path = out_dir.join("comparison.json");
    let json = serde_json::to_string_pretty(&comparison).expect("serialize comparison");
    std::fs::write(&comparison_path, &json).expect("write comparison.json");
    println!("\nwrote {}", comparison_path.display());

    // Print summary
    println!("\n=== Summary ===");
    if let Some(ref sp1) = comparison.sp1 {
        println!("SP1:   {} rows", sp1["rows"].as_array().map_or(0, |r| r.len()));
    }
    if let Some(ref risc0) = comparison.risc0 {
        println!("RISC0: {} rows", risc0["rows"].as_array().map_or(0, |r| r.len()));
    }

    // If both sides present, compare head-to-head on the 1k row (fastest)
    if let (Some(ref sp1), Some(ref risc0)) = (&comparison.sp1, &comparison.risc0) {
        let sp1_rows = sp1["rows"].as_array();
        let r0_rows = risc0["rows"].as_array();
        if let (Some(sr), Some(rr)) = (sp1_rows, r0_rows) {
            for (s, r) in sr.iter().zip(rr.iter()) {
                let label = s["size_label"].as_str().unwrap_or("?");
                let sp1_cycles = s["cycles"].as_u64().unwrap_or(0);
                let r0_cycles = r["cycles"].as_u64().unwrap_or(0);
                let sp1_prove = s["prove_ms"].as_u64().unwrap_or(0);
                let r0_prove = r["prove_ms"].as_u64().unwrap_or(0);
                println!(
                    "  {label:>4}: SP1 {sp1_cycles:>12} cycles / {sp1_prove:>8} ms  vs  RISC0 {r0_cycles:>12} cycles / {r0_prove:>8} ms",
                );
            }
        }
    }
}
