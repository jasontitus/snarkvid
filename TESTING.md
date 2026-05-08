# Testing inventory

This document tracks **what's been tested where**, and **what each
machine can/can't run**, so you can spin up a GPU instance, run one
script, and shut it down again.

## TL;DR — running the full test battery on a GPU box

```bash
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid
./scripts/full_test_gpu.sh   # ~90–150 min on a Lambda A10
```

The script prints a `PASS/FAIL` line per phase and a final summary; exit
0 means safe to terminate the instance.

## What's currently tested locally (CPU sandbox, 15 GiB RAM)

These are the things that have actually run green in development and
whose numbers are already in this repo.

| Component | Test | Status | Evidence |
|---|---|---|---|
| `crates/toy-codec` | 10 unit tests: WHT self-inverse, qp=0 lossless on flat/ramp/noise frames, qp=8 ≥ 40 dB PSNR on noise, qp monotonicity, dim/qp validation | PASS | `cargo test -p snarkvid-toy-codec` |
| `crates/comparator` | 2 unit tests: identical→∞ PSNR, off-by-one→~48 dB | PASS | `cargo test -p snarkvid-comparator` |
| `crates/manifest` | 3 unit tests: Merkle root single/two leaves, tampering detected | PASS (Ed25519 verify is **stubbed** — see "Known gaps" below) | `cargo test -p snarkvid-manifest` |
| `bin/toy-encode` | Compiles + smoke-encodes a YUV file | PASS | builds clean as part of workspace |
| Jolt SHA-256 1KB | CPU prove + verify round-trip, cycles + proof bytes captured | PASS — 53,032 cycles / 80,281 B / 4,031 ms prove / 120 ms verify | `spike/bench/results/jolt-sha256.json` |
| Jolt toy-decode 16×16 | CPU prove + verify with real WHT decoder in the loop | PASS — 108,816 cycles / 83,817 B / 5,962 ms prove / 132 ms verify | `spike/bench/results/jolt-toy-decode.json` |
| Sonobe Nova SHA-256 chain | CPU IVC bench, several fold counts | PASS (numbers in `01b-newer-frameworks.md` §4) | `spike/bench/results/sonobe-sha256.json` |
| Sonobe Nova toy-decode | CPU IVC bench (per-coefficient clamp) | PASS | `spike/bench/results/sonobe-toy-decode.json` |
| All four spike crates | `cargo build --release` workspace + per-spike | PASS | `cargo build --workspace --release` |

## What's deferred to a GPU box

These either need GPU wall-clock, more RAM than the sandbox has, or a
network path to GitHub releases that the sandbox doesn't have. Each is
covered by `scripts/full_test_gpu.sh`.

| Phase | Component | Why deferred | Where it runs |
|---|---|---|---|
| 4 | SP1 SHA-256 1KB / 1MB / 10MB on CUDA | M1's go/no-go numbers; need an actual GPU | `bench_cuda.sh` |
| 4 | RISC Zero SHA-256 sweep on CUDA | Head-to-head against SP1 | `bench_cuda.sh` |
| **5** | **SP1 toy-decode (3-way parity row)** | **`sp1up` couldn't reach `api.github.com` from the sandbox; the RISC-V `toy-decode` guest never built. Code is committed and ready.** | first build on the GPU box cross-compiles the new guest |
| 6 | Jolt SHA-256 1MB / 10MB | CPU-only; sandbox can't budget the wall-clock for the bigger fixtures | `bench_jolt.sh` |
| 7 | Sonobe Nova at higher step counts (`--max-steps 1024`) | Each fold step is ~650 ms; >1k steps wasn't worth burning sandbox minutes on | `bench_sonobe.sh` |
| **8** | **Sonobe Decider (Groth16 wrap → ~200 B proof)** | **OOM'd in the 15 GiB sandbox at anon-rss = 16 GB during Groth16 setup over the Nova augmented circuit. Code is committed and ready (`sonobe-script bench --decider`).** | needs ≥ 32 GB RAM (Lambda A10 default is fine) |

## What's NOT yet wired up anywhere (known gaps)

These are real holes — the GPU script won't catch them, because there's
nothing to run yet. Tracking here so they don't get lost.

| Gap | Where | Impact |
|---|---|---|
| Real Ed25519 verify | `crates/manifest::verify_manifest` accepts any signature today | Blocks M2 §3.4 tampering tests ("manifest signed by unknown key → fails closed") |
| Manifest signing helper | `crates/manifest` has no `sign_manifest()` | Host can't produce signed-manifest fixtures for the prover to consume |
| Merkle path generator | `crates/manifest::verify_merkle_path` exists; `merkle_path()` builder doesn't | Host has to construct paths by hand to feed the prover |
| Browser verifier (Sonobe Decider) | `spike/web/` exists for SP1 but not yet for the Decider's Groth16/BN254 proof | M2 §3.3 acceptance ("verify in a browser, < 2 s") not yet exercised |
| Jolt `verify` subcommand | Currently stubbed — needs `--input` carried through to verify-from-proof | Doesn't block bench; nice-to-have for round-trip robustness |
| Sonobe `verify` subcommand | Re-runs preprocessing instead of loading verifier params from disk | Verify times in JSON include preprocessing; flag in report |

## Re-running individual phases

`scripts/full_test_gpu.sh` deliberately keeps phases independent. To run
just one:

```bash
# Just the SP1 toy-decode 3-way parity row
spike/sp1/target/release/sp1-script bench \
    --workload toy-decode \
    --fixture-dir spike/common/bench-fixtures \
    --out spike/bench/results/sp1-toy-decode.json

# Just the Sonobe Decider (the load-bearing browser-verifier evidence)
spike/sonobe/target/release/sonobe-script bench \
    --workload sha256-chain \
    --fixture-dir spike/common/bench-fixtures \
    --max-steps 8 --decider \
    --out spike/bench/results/sonobe-sha256-decider.json
```

The result JSONs all share the same schema (`spike/bench/compare.py`
consumes them).

## Hardware notes for the GPU run

- **Lambda Labs A10 24 GB** is the cheapest box that fits everything;
  ~$0.75/hr. One A10 was the M1 reference machine.
- **A100 / H100** cut the 10 MB SP1 prove time roughly proportionally to
  memory bandwidth. Diminishing returns past A100 for the sizes here.
- **Memory ≥ 32 GB** is the bar for phase 8 (Sonobe Decider); A10
  instances on Lambda ship with 64 GB, so this is non-issue there.
