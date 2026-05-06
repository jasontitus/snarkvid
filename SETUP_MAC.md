# snarkvid — M1 Mac Setup & Benchmarks

## Prerequisites

- macOS 13+ (Ventura or later)
- 16 GB RAM minimum (32 GB preferred for the 1 MB benchmark)
- ~10 GB free disk
- Xcode Command Line Tools: `xcode-select --install`

## Quick start

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Restart your shell, then:

# 2. Clone
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid

# 3. Install toolchains
./scripts/setup_sp1.sh
./scripts/setup_risc0.sh

# 4. Enable RISC Zero dependencies
./scripts/uncomment_risc0_deps.sh

# 5. Build & benchmark
./scripts/bench_all.sh
```

Expected wall-clock: 20–60 minutes (mostly compilation).

## What each script does

| Script | What it does |
|---|---|
| `scripts/setup_sp1.sh` | Installs `sp1up` → `cargo-prove` + RISC-V toolchain |
| `scripts/setup_risc0.sh` | Installs `cargo-risczero` v1.2 (CPU-only, skips Metal GPU kernels) + RISC-V toolchain |
| `scripts/uncomment_risc0_deps.sh` | Removes `#` prefixes from RISC Zero deps in three `Cargo.toml` files |
| `scripts/bench_all.sh` | Generates fixtures, builds both sides, runs benchmarks, prints comparison |

## What you'll get

A head-to-head comparison table:

| Metric | RISC Zero | SP1 | Winner |
|---|---|---|---|
| Prove 1KB (ms) | ? | ? | ? |
| Prove 1MB (ms) | ? | ? | ? |
| Proof size (bytes) | ? | ? | ? |
| Verify native (ms) | ? | ? | ? |

Plus the raw JSON files in `spike/bench/results/`.

## Troubleshooting

### "missing Metal Toolchain" during `setup_risc0.sh`

The script sets `RISC0_SKIP_BUILD_KERNELS=1` which should prevent this. If it still fails:

```bash
rm -f ~/.cargo/bin/cargo-risczero ~/.cargo/bin/r0vm
RISC0_SKIP_BUILD_KERNELS=1 cargo install cargo-risczero --version "^1.2"
cargo risczero install
```

### `cargo prove` not found after `setup_sp1.sh`

```bash
curl -L https://sp1.succinct.xyz | bash
sp1up
```

### `cargo-risczero` installed v3.x instead of v1.2

```bash
rm -f ~/.cargo/bin/cargo-risczero ~/.cargo/bin/r0vm
RISC0_SKIP_BUILD_KERNELS=1 cargo install cargo-risczero --version "^1.2"
```

### 1 MB benchmark hangs or swaps heavily

macOS swaps to SSD rather than OOM-kill. Check Activity Monitor → Memory tab. If the graph is red, the machine is thrashing — Ctrl-C and skip. The 1 KB data point is still useful.

### RISC Zero host doesn't build

Make sure you ran `uncomment_risc0_deps.sh` and are building with `--features risc0`. The host prints a helpful error if the feature isn't on.

## After the benchmarks

1. Copy the output table into `DECISION.md` at the repo root
2. Fill in "Decision", "Rationale", and "Risks accepted"
3. Commit and push

## 10 MB GPU benchmark

The CPU benchmarks above tell you which zkVM is more cycle-efficient per byte. For the **10 MB wall-clock number** (the actual go/no-go for the project), you still need a CUDA GPU. Rent one:

- **AWS**: `g4dn.xlarge` (T4 GPU, ~$0.50/hr)
- **Run**: clone the repo, `./scripts/bench_all.sh`, record the 10 MB row

The M1's Metal GPU won't work for this — neither SP1 nor RISC Zero has a Metal prover backend.
