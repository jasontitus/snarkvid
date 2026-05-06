# snarkvid — M1 Mac Setup & Benchmarks

## What works on M1

**SP1: ✅ fully working.** Prove, verify, bench — all functional.

**RISC Zero: ❌ does not build on aarch64 macOS.** The v1.2.x C++ circuit libraries have no prebuilt binaries for this platform and fail to link. This is a known upstream limitation. Run RISC Zero on Linux x86_64 instead.

## Prerequisites

- macOS 13+ (Ventura or later)
- 16 GB RAM minimum
- ~5 GB free disk
- Xcode Command Line Tools: `xcode-select --install`

## Quick start (SP1 only)

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Restart shell, then:

# 2. Clone
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid

# 3. Install SP1 toolchain
./scripts/setup_sp1.sh

# 4. Build & benchmark
./scripts/bench_sp1.sh
```

Expected: ~15–30 minutes (mostly compilation). Produces `spike/bench/results/sp1.json`.

## Scripts

| Script | What it does |
|---|---|
| `scripts/setup_sp1.sh` | Installs `sp1up` → `cargo-prove` + RISC-V toolchain |
| `scripts/bench_sp1.sh` | Generates fixtures, builds SP1, runs benchmarks |

## Head-to-head (RISC Zero vs SP1)

RISC Zero needs x86_64 Linux. Use a cloud instance:

```bash
# On Linux x86_64:
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid
./scripts/setup_sp1.sh
./scripts/setup_risc0.sh
./scripts/uncomment_risc0_deps.sh
./scripts/bench_all.sh
```

## What you'll get

From `bench_sp1.sh` on M1:

| Fixture | Prove time | Verify time | Proof size |
|---|---|---|---|
| 1 KB | ~20 s | ~85 ms | ~2.7 MB |
| 1 MB | OOM (killed) | — | — |

From `bench_all.sh` on Linux x86_64: full head-to-head comparison table with both zkVMs at 1KB + 1MB + 10MB.

## After the benchmarks

1. Copy the output into `DECISION.md` at the repo root
2. Fill in "Decision", "Rationale", and "Risks accepted"
3. Commit and push

## 10 MB GPU benchmark

The CPU numbers tell you which zkVM is more cycle-efficient. For the 10 MB wall-clock (the go/no-go number), rent a CUDA GPU instance:

- **AWS**: `g4dn.xlarge` (T4 GPU, ~$0.50/hr)
- **Run**: clone the repo, `./scripts/bench_all.sh`, record the 10 MB row
