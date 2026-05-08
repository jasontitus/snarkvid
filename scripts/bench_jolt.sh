#!/usr/bin/env bash
# Build the Jolt side and run CPU benchmarks. SHA-256 fixtures are 1KB
# / 1MB / 10MB; toy-decode is a single 16x16 4:2:0 frame.
#
# Prerequisites: run scripts/setup_jolt.sh first.

set -euo pipefail

cd "$(dirname "$0")/.."

FIXTURES="spike/common/bench-fixtures"
RESULTS="spike/bench/results"

echo "=== Generating fixtures ==="
bash "$FIXTURES/gen.sh"

echo ""
echo "=== Building Jolt ==="
cd spike/jolt
cargo build --release
cd ../..
echo "Jolt binary: $(ls -lh spike/jolt/target/release/jolt-script 2>/dev/null | awk '{print $5}')"

mkdir -p "$RESULTS"

echo ""
echo "=== Jolt bench: sha256 ==="
# Jolt's host shells out to `jolt build -p <pkg>`, which only resolves
# the package when invoked from inside spike/jolt's workspace. cd in
# before running the binary; pass paths relative to that directory.
(
    cd spike/jolt
    timeout 1800 ./target/release/jolt-script bench \
        --workload sha256 \
        --fixture-dir ../common/bench-fixtures \
        --out ../bench/results/jolt-sha256.json
) || true

echo ""
echo "=== Jolt bench: toy-decode ==="
(
    cd spike/jolt
    timeout 1800 ./target/release/jolt-script bench \
        --workload toy-decode \
        --fixture-dir ../common/bench-fixtures \
        --out ../bench/results/jolt-toy-decode.json
) || true

echo ""
echo "=== Results ==="
for f in "$RESULTS/jolt-sha256.json" "$RESULTS/jolt-toy-decode.json"; do
    if [[ -f "$f" ]]; then
        echo "--- $f ---"
        python3 -m json.tool "$f" 2>/dev/null || cat "$f"
        echo ""
    fi
done

echo "Done."
