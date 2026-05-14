#!/usr/bin/env bash
# Build SP1 + RISC Zero with CUDA features and run all three fixtures.
# Prerequisites: ./scripts/setup_linux_cuda.sh has succeeded.
set -euo pipefail

cd "$(dirname "$0")/.."

# Toolchain shims; CUDA toolkit on PATH for any cargo-driven C++/nvcc compile.
export PATH="$HOME/.cargo/bin:$HOME/.sp1/bin:/usr/local/cuda/bin:$PATH"

# WSL / Ubuntu-noble fallbacks: if /usr/local/cuda doesn't exist but $HOME/cuda
# was populated (see SETUP_WSL.md), point find_cuda_helper at it via the env
# vars that crate honors on Linux (CUDA_LIBRARY_PATH for lib search, CUDA_PATH
# for include search). Also force gcc-12 as nvcc's host compiler — CUDA 12.0
# refuses gcc 13+ which is the default on Ubuntu 24.04.
if [[ ! -d /usr/local/cuda && -d "$HOME/cuda/lib64" ]]; then
    export PATH="$HOME/cuda/host-cxx:$HOME/cuda/bin:$PATH"
    export CUDA_PATH="$HOME/cuda"
    export CUDA_HOME="$HOME/cuda"
    export CUDA_ROOT="$HOME/cuda"
    export CUDA_LIBRARY_PATH="$HOME/cuda"
    [[ -x /usr/bin/gcc-12 ]] && export CC=gcc-12
    [[ -x /usr/bin/g++-12 ]] && export CXX=g++-12
fi

# SP1 picks the prover backend at runtime via this env var. The cuda backend
# is only available when sp1-sdk was compiled with --features cuda (below).
export SP1_PROVER="${SP1_PROVER:-cuda}"

FIXTURES="spike/common/bench-fixtures"
RESULTS="spike/bench/results"
mkdir -p "$RESULTS"

# ----- Sanity ----------------------------------------------------------------
if ! command -v nvidia-smi >/dev/null 2>&1; then
    echo "error: nvidia-smi not found. Run on a CUDA box (or use bench_all.sh for CPU)." >&2
    exit 1
fi
nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader

# ----- Fixtures --------------------------------------------------------------
echo ""
echo "=== Generating fixtures ==="
bash "$FIXTURES/gen.sh"

# ----- Build SP1 (CUDA) ------------------------------------------------------
echo ""
echo "=== Building SP1 (--features cuda) ==="
(cd spike/sp1 && cargo build --release --features cuda)
echo "SP1 binary: $(ls -lh spike/sp1/target/release/sp1-script | awk '{print $5}')"

# ----- Build RISC Zero (CUDA) ------------------------------------------------
echo ""
echo "=== Building RISC Zero (--features risc0,cuda) ==="
(cd spike/risc0 && cargo build --release --features risc0,cuda)
echo "RISC0 binary: $(ls -lh spike/risc0/target/release/risc0-host | awk '{print $5}')"

# ----- SP1 bench -------------------------------------------------------------
echo ""
echo "=== SP1 bench (SP1_PROVER=$SP1_PROVER) ==="
spike/sp1/target/release/sp1-script bench \
    --fixture-dir "$FIXTURES" --out "$RESULTS/sp1.json"

# ----- RISC Zero bench -------------------------------------------------------
# risc0-zkvm picks GPU automatically when compiled with the cuda feature.
echo ""
echo "=== RISC Zero bench ==="
spike/risc0/target/release/risc0-host bench \
    --fixture-dir "$FIXTURES" --out "$RESULTS/risc0.json"

# ----- Compare ---------------------------------------------------------------
echo ""
echo "=== Results ==="
ls -lh "$RESULTS/"
if command -v python3 >/dev/null 2>&1 && [[ -f "$RESULTS/sp1.json" && -f "$RESULTS/risc0.json" ]]; then
    echo ""
    python3 spike/bench/compare.py --markdown "$RESULTS/risc0.json" "$RESULTS/sp1.json"
fi

echo ""
echo "Done."
