#!/usr/bin/env bash
# Build SP1 + RISC Zero with CUDA features and run all three fixtures.
# Prerequisites: ./scripts/setup_linux_cuda.sh has succeeded.
set -euo pipefail

cd "$(dirname "$0")/.."

# Toolchain shims; CUDA toolkit on PATH for any cargo-driven C++/nvcc compile.
export PATH="$HOME/.cargo/bin:$HOME/.sp1/bin:/usr/local/cuda/bin:$PATH"

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
