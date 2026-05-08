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
echo "=== SP1 bench: sha256 ==="
mkdir -p "$RESULTS"
spike/sp1/target/release/sp1-script bench \
    --workload sha256 \
    --fixture-dir "$FIXTURES" --out "$RESULTS/sp1.json" || true

echo ""
echo "=== SP1 bench: toy-decode (3-way parity with Jolt + Sonobe) ==="
spike/sp1/target/release/sp1-script bench \
    --workload toy-decode \
    --fixture-dir "$FIXTURES" --out "$RESULTS/sp1-toy-decode.json" || true

echo ""
echo "=== Results ==="
for f in "$RESULTS/sp1.json" "$RESULTS/sp1-toy-decode.json"; do
    if [[ -f "$f" ]]; then
        echo "--- $f ---"
        python3 -m json.tool "$f" 2>/dev/null || cat "$f"
        echo ""
    fi
done

echo ""
echo "Done."
