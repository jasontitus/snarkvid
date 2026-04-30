// Host driver for the RISC Zero side of the spike.
//
// CLI (see ../README.md):
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

fn main() {
    eprintln!("spike scaffold — implement per milestones/01-spike.md §9");
    std::process::exit(2);
}
