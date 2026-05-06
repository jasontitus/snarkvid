#!/usr/bin/env bash
# Build both zkVM sides and run CPU benchmarks on all fixtures.
# Prerequisites: run setup_sp1.sh, setup_risc0.sh, uncomment_risc0_deps.sh first.
# Gracefully skips RISC Zero if it failed to build.
set -euo pipefail

cd "$(dirname "$0")/.."

FIXTURES="spike/common/bench-fixtures"
RESULTS="spike/bench/results"
mkdir -p "$RESULTS"

# ---- Generate fixtures ----
echo "=== Generating fixtures ==="
bash "$FIXTURES/gen.sh"

# ---- Build SP1 ----
echo ""
echo "=== Building SP1 ==="
cd spike/sp1
cargo build --release
cd ../..
echo "SP1 binary: $(ls -lh spike/sp1/target/release/sp1-script | awk '{print $5}')"

# ---- Build RISC Zero ----
echo ""
echo "=== Building RISC Zero ==="
RISC0_OK=0
cd spike/risc0
if cargo build --release --features risc0 2>&1 | tail -5; then
    RISC0_OK=1
fi
cd ../..
if [[ "$RISC0_OK" -eq 1 && -f spike/risc0/target/release/risc0-host ]]; then
    echo "RISC0 binary: $(ls -lh spike/risc0/target/release/risc0-host | awk '{print $5}')"
else
    echo "RISC Zero did not build — skipping (expected on aarch64 macOS)"
    echo "Run on Linux x86_64 for the full head-to-head comparison."
fi

# ---- Run SP1 benchmarks ----
echo ""
echo "=== SP1 bench ==="
spike/sp1/target/release/sp1-script bench \
    --fixture-dir "$FIXTURES" --out "$RESULTS/sp1.json" || true

# ---- Run RISC Zero benchmarks ----
echo ""
echo "=== RISC Zero bench ==="
if [[ "$RISC0_OK" -eq 1 && -f spike/risc0/target/release/risc0-host ]]; then
    spike/risc0/target/release/risc0-host bench \
        --fixture-dir "$FIXTURES" --out "$RESULTS/risc0.json" || true
else
    echo "Skipping — RISC Zero not built"
fi

# ---- Print results ----
echo ""
echo "=== Results ==="
ls -lh "$RESULTS/"

if command -v python3 &>/dev/null && [[ -f "$RESULTS/sp1.json" && -f "$RESULTS/risc0.json" ]]; then
    echo ""
    python3 spike/bench/compare.py --markdown "$RESULTS/risc0.json" "$RESULTS/sp1.json"
elif [[ -f "$RESULTS/sp1.json" ]]; then
    echo ""
    echo "SP1 results:"
    python3 -m json.tool "$RESULTS/sp1.json" 2>/dev/null || cat "$RESULTS/sp1.json"
fi

echo ""
echo "Done."
