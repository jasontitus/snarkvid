# Milestone 1 — Spike Results & Decision

This is the DECISION.md called for in `milestones/01-spike.md` §2 ac-4.
It records the head-to-head numbers, the picked system, and the risks
accepted at the close of the spike.

## TL;DR

**Picked: SP1 (v6.1, CUDA backend).**
**Backed by:** ~19× faster prove and ~2.7× smaller proofs than RISC Zero
on the 1 MB and 10 MB fixtures, on the same A10. Verifier is also ~5×
faster than RISC Zero's at those sizes.

## Hardware & toolchains

| | Value |
|---|---|
| GPU | NVIDIA A10, 24 GB, driver 580.105.08, CUDA 13.0 |
| Host | Lambda Labs Ubuntu 22.04 |
| SP1 | sp1-sdk 6.1.0 (`SP1_PROVER=cuda`, `--features cuda`) |
| RISC Zero | risc0-zkvm 3.0.5, risc0-build 3.0.4, r0vm 3.0.5, RISC Zero rust 1.94.1 (`--features risc0,cuda`) |

`bench_cuda.sh` writes `spike/bench/results/{sp1,risc0}.json`. Re-runnable.

## Head-to-head

All three fixtures are the SHA-256-of-private-witness statement defined in
`milestones/01-spike.md` §1. Numbers are from runs where each system had
exclusive use of the GPU (see "Anomalies" below).

| Fixture | Metric | RISC Zero 3.0.5 | SP1 6.1 (CUDA) | Winner |
|---|---|---:|---:|:---:|
| **1 KB**  | cycles               |   228,654 |    90,887 | SP1 (2.5× fewer) |
|           | prove (ms)           |       935 |       818 | ≈ tie           |
|           | verify native (ms)   |        20 |       114 | RISC Zero (5.7×) |
|           | proof size (bytes)   |   256,226 | 2,778,995 | RISC Zero (11×)  |
| **1 MB**  | cycles               | 219,756,271 |  77,920,728 | SP1 (2.8× fewer) |
|           | prove (ms)           |   465,165 |    23,912 | **SP1 (19.4×)**  |
|           | verify native (ms)   |     4,891 |       952 | SP1 (5.1×)       |
|           | proof size (bytes)   | 61,582,678 | 22,641,573 | SP1 (2.7×)       |
| **10 MB** | cycles               | 2,209,233,159 | 784,974,692 | SP1 (2.8× fewer) |
|           | prove (ms)           | 4,725,341 |   238,372 | **SP1 (19.8×)**  |
|           | verify native (ms)   |    49,240 |     9,796 | SP1 (5.0×)       |
|           | proof size (bytes)   | 621,423,010 | 231,666,073 | SP1 (2.7×)       |

In wall-clock terms: 10 MB on SP1/CUDA is **~4 minutes**; on RISC Zero/CUDA
it's **~79 minutes**.

## Decision

**SP1.** Spike §2 says "we pick the system that wins on prove time at
10 MB on a single GPU, with proof size and browser verifier viability as
tiebreakers." SP1 wins prove time ~20×, also wins proof size, also wins
native verify. The only category RISC Zero wins is the 1 KB row (smaller
fixed-size proofs, faster verify on tiny inputs) — not the deciding axis.

The browser-verifier numbers (verifier WASM bundle gz, browser verify ms)
called for in §4 are still missing — see "Out of scope, deferred" below.
They could move the answer in principle, but at a 19× prove-time gap the
bar to flip the decision is high.

## Risks accepted

- **Browser verifier viability not yet measured.** The decision rests on
  prove time and proof size only. The web/ harness was not built in this
  spike. Mitigation: SP1's Groth16-wrapped proofs are a fixed ~256 bytes
  with a published Solidity verifier and a small WASM verifier; this is
  expected to be cheap to verify in a browser, but it's *expected*, not
  measured.
- **Core-proof size is large in absolute terms.** 232 MB at 10 MB input is
  unacceptable for any user-facing flow. Production will compress via
  `prove(...).compress()` to a constant-size succinct proof (~100 KB) and
  optionally `groth16()` to ~256 bytes. The numbers above are uncompressed
  cores — the worst case.
- **SP1 prove time still scales linearly.** 19× faster than RISC Zero is
  not 19× faster than feasible. Extrapolating to H.264 workloads (see
  conversation in commits) is firmly in "shard + recurse" territory; the
  spike does not validate that the production architecture proves cheaply,
  only that *this* system proves cheaper than the alternative.
- **Single-GPU only.** A100 / H100 numbers were not measured in this
  spike. Lambda A10 was the cheapest box that fit 10 MB; bigger GPUs are
  expected to scale roughly with their memory bandwidth.

## Anomalies observed during the spike

- **Initial RISC Zero CUDA build was broken** against the published
  `risc0-circuit-recursion 1.2.6` + `risc0-sys 1.5.0` combination — the
  Rust code references `sppark_calc_prefix_operation` (new ABI), but the
  resolved `risc0-sys` only ships `sppark_prefix_product` (old ABI). Fixed
  by upgrading the spike to RISC Zero 3.0.5; commits document the change
  in `scripts/setup_risc0.sh`, `spike/risc0/host/Cargo.toml`,
  `spike/risc0/methods/Cargo.toml`, and `spike/risc0/methods/guest/Cargo.toml`.
- **`sp1-gpu-server` holds GPU memory across calls.** When `bench_cuda.sh`
  ran SP1 and then RISC Zero in the same script, RISC Zero hit either OOM
  or "verify segment / proof is invalid" — *not* a real RISC Zero bug, just
  GPU memory contention with SP1's lingering server. Reproducible: run the
  systems in the same parent process and you'll see it. Mitigation
  documented in `SETUP_LINUX_CUDA.md` (run the two systems as separate
  invocations; the troubleshooting section has the exact commands).
- **Bench output was previously all-or-nothing.** The original host code
  held all rows in memory and wrote `*.json` only at the very end. A crash
  during the 10 MB row erased the 1 KB and 1 MB results we'd already paid
  for. Fixed in both hosts: each fixture row is now atomically rewritten
  to the output JSON the moment it completes, and stdout is flushed per
  print so live progress shows when piped.

## Out of scope, deferred

- **Browser verifier WASM** — `spike/web/` harness, verifier WASM bundle
  size, browser verify ms. M1 acceptance criteria 2 and 4 (the bundle-size
  / browser-verify columns of the comparison table). Not built in this
  spike; the decision is made on prove + native-verify alone. Re-open if
  the picked system's WASM verifier turns out to be unfit for the browser.
- **Recursion / proof aggregation** — surveyed in `milestones/01-spike.md`
  §7 as out of scope.
- **A100/H100 numbers** — A10 only.
