#!/usr/bin/env bash
# Build the Sonobe side and run CPU benchmarks on 1KB fixture by default.
# (10MB SHA-256 chain ~= 327k fold steps; would not finish on a laptop.
# Larger fixtures need a beefy CPU box or a long --max-steps cap.)
#
# Prerequisites: run scripts/setup_sonobe.sh first.

set -euo pipefail

cd "$(dirname "$0")/.."

FIXTURES="spike/common/bench-fixtures"
RESULTS="spike/bench/results"
MAX_STEPS="${MAX_STEPS:-1024}"

echo "=== Generating fixtures ==="
bash "$FIXTURES/gen.sh"

echo ""
echo "=== Building Sonobe ==="
cd spike/sonobe
cargo build --release
cd ../..
echo "Sonobe binary: $(ls -lh spike/sonobe/target/release/sonobe-script 2>/dev/null | awk '{print $5}')"

mkdir -p "$RESULTS"

echo ""
echo "=== Sonobe bench: sha256-chain (max_steps=$MAX_STEPS) ==="
spike/sonobe/target/release/sonobe-script bench \
    --workload sha256-chain \
    --fixture-dir "$FIXTURES" \
    --max-steps "$MAX_STEPS" \
    --out "$RESULTS/sonobe-sha256.json" || true

echo ""
echo "=== Sonobe bench: toy-decode (max_steps=$MAX_STEPS) ==="
spike/sonobe/target/release/sonobe-script bench \
    --workload toy-decode \
    --fixture-dir "$FIXTURES" \
    --max-steps "$MAX_STEPS" \
    --out "$RESULTS/sonobe-toy-decode.json" || true

echo ""
echo "=== Results ==="
for f in "$RESULTS/sonobe-sha256.json" "$RESULTS/sonobe-toy-decode.json"; do
    if [[ -f "$f" ]]; then
        echo "--- $f ---"
        python3 -m json.tool "$f" 2>/dev/null || cat "$f"
        echo ""
    fi
done

echo "Done."
