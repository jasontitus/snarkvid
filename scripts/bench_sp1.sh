#!/usr/bin/env bash
# Build the SP1 side and run CPU benchmarks on 1KB fixture.
# (1MB OOMs on most machines — needs GPU.)
# Prerequisites: run scripts/setup_sp1.sh first.
set -euo pipefail

cd "$(dirname "$0")/.."

FIXTURES="spike/common/bench-fixtures"
RESULTS="spike/bench/results"

echo "=== Generating fixtures ==="
bash "$FIXTURES/gen.sh"

echo ""
echo "=== Building SP1 ==="
cd spike/sp1
cargo build --release
cd ../..
echo "SP1 binary: $(ls -lh spike/sp1/target/release/sp1-script | awk '{print $5}')"

echo ""
echo "=== SP1 bench ==="
mkdir -p "$RESULTS"
spike/sp1/target/release/sp1-script bench \
    --fixture-dir "$FIXTURES" --out "$RESULTS/sp1.json" || true

echo ""
echo "=== Results ==="
if [[ -f "$RESULTS/sp1.json" ]]; then
    python3 -m json.tool "$RESULTS/sp1.json" 2>/dev/null || cat "$RESULTS/sp1.json"
else
    echo "No results produced."
fi

echo ""
echo "Done."
