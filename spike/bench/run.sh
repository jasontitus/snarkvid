#!/usr/bin/env bash
# Run both zkVM sides on all fixtures and emit comparison.json.
#
# Usage:
#   bench/run.sh             # CPU only
#   bench/run.sh --gpu       # enable GPU features on both sides
#
# Prerequisites:
#   - Fixtures generated: spike/common/bench-fixtures/gen.sh
#   - Each side's host built: cargo build --release in spike/risc0 and spike/sp1
#
# Output:
#   bench/results/<timestamp>/risc0.json
#   bench/results/<timestamp>/sp1.json
#   bench/comparison.json (latest)

set -euo pipefail

cd "$(dirname "$0")/.."

USE_GPU=0
for arg in "$@"; do
    case "$arg" in
        --gpu) USE_GPU=1 ;;
        *) echo "unknown arg: $arg" >&2; exit 2 ;;
    esac
done

FIXTURES=common/bench-fixtures
[ -f "$FIXTURES/fixture-1k.bin" ] || {
    echo "fixtures missing — run $FIXTURES/gen.sh first" >&2
    exit 1
}

ts=$(date -u +%Y%m%dT%H%M%SZ)
out=bench/results/$ts
mkdir -p "$out"

run_side() {
    local name="$1" cmd="$2"
    echo ">>> $name"
    if [ "$USE_GPU" = "1" ]; then
        cmd="$cmd --features gpu"
    fi
    # Each host's `bench` subcommand emits a single JSON document on stdout.
    # See its source for the schema.
    eval "$cmd" bench --fixture-dir "$FIXTURES" > "$out/$name.json"
}

run_side risc0 "cargo run --release --manifest-path risc0/host/Cargo.toml --"
run_side sp1   "cargo run --release --manifest-path sp1/script/Cargo.toml --"

# Latest pointer.
cp "$out/risc0.json" bench/results/latest-risc0.json
cp "$out/sp1.json"   bench/results/latest-sp1.json

python3 bench/compare.py "$out/risc0.json" "$out/sp1.json" > bench/comparison.json
python3 bench/compare.py --markdown "$out/risc0.json" "$out/sp1.json" > bench/DECISION.md

echo "wrote bench/comparison.json and bench/DECISION.md"
