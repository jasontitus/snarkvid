#!/usr/bin/env bash
# Build both zkVM sides and run CPU benchmarks on 1KB + 1MB fixtures.
# Prerequisites: run setup_sp1.sh, setup_risc0.sh, uncomment_risc0_deps.sh first.
set -euo pipefail

cd "$(dirname "$0")/.."

FIXTURES="spike/common/bench-fixtures"
RESULTS="spike/bench/results"

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
cd spike/risc0
cargo build --release --features risc0
cd ../..
if [[ -f spike/risc0/target/release/risc0-host ]]; then
    echo "RISC0 binary: $(ls -lh spike/risc0/target/release/risc0-host | awk '{print $5}')"
else
    echo "WARNING: risc0-host binary not found — check build output above"
fi

# ---- Run benchmarks ----
mkdir -p "$RESULTS"

echo ""
echo "=== SP1 bench ==="
spike/sp1/target/release/sp1-script bench \
    --fixture-dir "$FIXTURES" --out "$RESULTS/sp1.json" || true

echo ""
echo "=== RISC Zero bench ==="
if [[ -f spike/risc0/target/release/risc0-host ]]; then
    spike/risc0/target/release/risc0-host bench \
        --fixture-dir "$FIXTURES" --out "$RESULTS/risc0.json" || true
else
    echo "Skipping — risc0-host not built"
fi

# ---- Print summary ----
echo ""
echo "=== Results ==="
ls -lh "$RESULTS/"

if command -v python3 &>/dev/null && [[ -f "$RESULTS/sp1.json" && -f "$RESULTS/risc0.json" ]]; then
    echo ""
    python3 spike/bench/compare.py --markdown "$RESULTS/risc0.json" "$RESULTS/sp1.json"
else
    echo ""
    echo "SP1:"
    cat "$RESULTS/sp1.json" 2>/dev/null || echo "(not available)"
    echo ""
    echo "RISC0:"
    cat "$RESULTS/risc0.json" 2>/dev/null || echo "(not available)"
fi

echo ""
echo "Done."
