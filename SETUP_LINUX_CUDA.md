# snarkvid — Linux + CUDA Setup (Lambda Labs)

The CPU benchmarks on M1 (`SETUP_MAC.md`) tell you which zkVM is more
cycle-efficient. For real wall-clock numbers — especially the 10 MB row,
which is the actual go/no-go for the project — you need an NVIDIA GPU.
This document covers Lambda Labs (or any Ubuntu + CUDA box).

## Pick an instance

Both provers use a single GPU; multi-GPU buys you nothing here.

| Need | Recommended | Notes |
|---|---|---|
| Full head-to-head incl. 10 MB | `gpu_1x_a10` (24 GB) | Measured ~8 GB peak GPU mem on 10 MB for either system; cheapest at ~$0.75/hr. Run SP1 and RISC Zero as separate invocations (see Troubleshooting). |
| Faster wall-clock on 10 MB | `gpu_1x_a100` (40/80 GB) | ~$1.30/hr; ~3× faster prove than A10 in published bench numbers |
| Fastest, if available | `gpu_1x_h100` | Best wall-clock; check on-demand availability |

Lambda images come with Ubuntu 22.04, NVIDIA drivers, and CUDA preinstalled
under `/usr/local/cuda`. You generally don't need to install CUDA yourself.

## Quick start

SSH into the box, then:

```bash
# 1. Clone
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid

# 2. Install everything (Rust, SP1, RISC Zero, build deps)
./scripts/setup_linux_cuda.sh

# 3. Build with CUDA features and run all three fixtures on both zkVMs
./scripts/bench_cuda.sh
```

`bench_cuda.sh` writes `spike/bench/results/sp1.json` and
`spike/bench/results/risc0.json` and prints a head-to-head markdown table.

Wall-clock budget: expect the install to take ~20–40 min on an A100 (mostly
RISC Zero compile, which builds ~600 crates including CUDA kernels). The
bench itself is highly GPU-dependent — A100 1 MB is minutes, 10 MB is tens
of minutes.

## What the scripts do

| Script | What it does |
|---|---|
| `scripts/setup_linux_cuda.sh` | Verifies GPU + nvcc; apt-installs build deps (incl. `protobuf-compiler`); rustup; runs `setup_sp1.sh`, `setup_risc0.sh`, `uncomment_risc0_deps.sh` |
| `scripts/bench_cuda.sh` | Builds SP1 with `--features cuda`, RISC Zero with `--features risc0,cuda`; runs all three fixtures with `SP1_PROVER=cuda` |

The underlying `setup_sp1.sh` / `setup_risc0.sh` / `uncomment_risc0_deps.sh`
are shared with the macOS path; they detect the platform.

## How CUDA gets enabled

- **SP1 6.1**: `sp1-sdk` has a `cuda` feature that pulls in `sp1-cuda`. The
  bench host calls `ProverClient::from_env().await`, which picks the backend
  from the `SP1_PROVER` env var (`cpu` / `cuda` / `network` / `mock`).
  `bench_cuda.sh` exports `SP1_PROVER=cuda`. SP1's CUDA path runs the prover
  in an out-of-process `sp1-gpu-server`; that server holds GPU memory across
  prove calls and only releases it when `sp1-script` exits.
- **RISC Zero 3.x**: `risc0-zkvm` has a `cuda` feature; the host crate's
  `Cargo.toml` declares `cuda = ["risc0-zkvm/cuda"]` so `cargo build
  --features risc0,cuda` brings in the CUDA backend. `default_prover()`
  picks it automatically. The toolchain is installed via **`rzup`** (RISC
  Zero's installer; replaces the older `cargo install cargo-risczero` flow);
  `setup_risc0.sh` does this for you.

### Why protobuf-compiler is required

SP1's `sp1-prover-types` crate has a `build.rs` that runs `prost-build` over
`.proto` files and so needs `protoc` on `PATH`. `setup_linux_cuda.sh` apt-
installs `protobuf-compiler`. Without it the SP1 build fails partway with
`Could not find 'protoc'`.

## Troubleshooting

### `nvcc: command not found`

Lambda images sometimes don't put `/usr/local/cuda/bin` on the default
`PATH`. The setup script adds it, but for ad-hoc commands:

```bash
export PATH="/usr/local/cuda/bin:$PATH"
```

To make it permanent for new shells:

```bash
echo 'export PATH="$HOME/.cargo/bin:$HOME/.sp1/bin:/usr/local/cuda/bin:$PATH"' >> ~/.bashrc
```

### `error: linker `cc` failed` during the RISC Zero or SP1 build

Usually means the CUDA libs aren't on the linker path. Check:

```bash
ldconfig -p | grep -i cuda     # should list libcudart, libcuda, etc.
ls /usr/local/cuda/lib64       # libcudart.so etc.
```

If missing, your image lacks the CUDA toolkit. On Lambda this is rare —
file a support ticket or pick a different instance type.

### SP1 falls back to CPU even though I set `SP1_PROVER=cuda`

The binary must have been built with `--features cuda`. `bench_cuda.sh` does
this. If you ran a different build, rebuild with:

```bash
(cd spike/sp1 && cargo build --release --features cuda)
```

### RISC Zero is using CPU instead of GPU

Check the host was built with `--features risc0,cuda`. The default
`bench_all.sh` does **not** pass `cuda`; only `bench_cuda.sh` does.

### Out-of-GPU-memory or "verify segment / proof is invalid"

Both of these have shown up when the bench script runs SP1 then RISC Zero
back-to-back on a 24 GB GPU. SP1's `sp1-gpu-server` holds GPU memory after
its bench finishes and is only torn down when `sp1-script` exits — which
in `bench_cuda.sh` happens *after* the RISC Zero step. RISC Zero then
contends for GPU memory, and depending on timing you'll see either an OOM
panic in `risc0-zkp/src/hal/cuda.rs` or a misleading
"verify segment / verification indicates proof is invalid" (a CUDA error
that didn't propagate cleanly).

Mitigation: run the two systems as separate invocations rather than letting
the script chain them. After the SP1 bench writes its JSON, kill any
lingering `sp1-gpu-server`, then run the RISC Zero step:

```bash
# After SP1 completes
pkill -9 -f sp1-gpu-server || true
spike/risc0/target/release/risc0-host bench \
    --fixture-dir spike/common/bench-fixtures \
    --out spike/bench/results/risc0.json
```

A 24 GB A10 *is* sufficient for 10 MB on either system in isolation
(measured: ~8 GB GPU at peak for either). It only fails when both systems
fight for the same GPU.

## Comparing CUDA numbers to the Mac numbers

The CPU JSON from `SETUP_MAC.md`'s `bench_all.sh` and the GPU JSON from
`bench_cuda.sh` are the same schema. `spike/bench/compare.py` accepts any
two JSON files, so you can do whatever pairing you like:

```bash
python3 spike/bench/compare.py --markdown \
    mac/risc0.json mac/sp1.json
python3 spike/bench/compare.py --markdown \
    lambda/risc0.json lambda/sp1.json
```
