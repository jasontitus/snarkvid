# snarkvid — Linux + CUDA Setup (Lambda Labs)

The CPU benchmarks on M1 (`SETUP_MAC.md`) tell you which zkVM is more
cycle-efficient. For real wall-clock numbers — especially the 10 MB row,
which is the actual go/no-go for the project — you need an NVIDIA GPU.
This document covers Lambda Labs (or any Ubuntu + CUDA box).

## Pick an instance

Both provers use a single GPU; multi-GPU buys you nothing here.

| Need | Recommended | Notes |
|---|---|---|
| 1 KB + 1 MB head-to-head only | `gpu_1x_a10` (24 GB) | Cheapest, ~$0.75/hr |
| 10 MB row, comfortable | `gpu_1x_a100` (40/80 GB) | ~$1.30/hr; 80 GB variant safer for 10 MB |
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
| `scripts/setup_linux_cuda.sh` | Verifies GPU + nvcc; apt-installs build deps; rustup; runs `setup_sp1.sh`, `setup_risc0.sh`, `uncomment_risc0_deps.sh` |
| `scripts/bench_cuda.sh` | Builds SP1 with `--features cuda`, RISC Zero with `--features risc0,cuda`; runs all three fixtures with `SP1_PROVER=cuda` |

The underlying `setup_sp1.sh` / `setup_risc0.sh` / `uncomment_risc0_deps.sh`
are shared with the macOS path; they detect the platform.

## How CUDA gets enabled

- **SP1 6.1**: `sp1-sdk` has a `cuda` feature that pulls in `sp1-cuda`. The
  bench host calls `ProverClient::from_env().await`, which picks the backend
  from the `SP1_PROVER` env var (`cpu` / `cuda` / `network` / `mock`).
  `bench_cuda.sh` exports `SP1_PROVER=cuda`.
- **RISC Zero 1.2**: `risc0-zkvm` has a `cuda` feature. On `x86_64`,
  `risc0-r0vm`'s target dep already force-enables it (`features = ["prove",
  "cuda"]`), which is why `cargo install cargo-risczero` requires `nvcc`
  even if you didn't ask for GPU explicitly. With the feature on,
  `default_prover()` selects the CUDA backend automatically.

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

### `error: linker `cc` failed` during cargo-risczero install

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

### Out-of-GPU-memory on 10 MB

Pick the 80 GB A100 variant or H100. The 10 MB fixture pushes prover memory
significantly; 24 GB A10 is not safe for 10 MB.

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
