#!/usr/bin/env bash
# Build all four spike sides and run CPU benchmarks on all fixtures.
# Prerequisites:
#   scripts/setup_sp1.sh
#   scripts/setup_risc0.sh + scripts/uncomment_risc0_deps.sh
#   scripts/setup_sonobe.sh
#   scripts/setup_jolt.sh
# Gracefully skips any side that fails to build.
set -euo pipefail

# Use rustup shims so `+succinct` / `+risc0` toolchain selection works.
# ~/.sp1/bin is where sp1up installs cargo-prove.
export PATH="$HOME/.cargo/bin:$HOME/.sp1/bin:$PATH"

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

# ---- Build Sonobe ----
echo ""
echo "=== Building Sonobe ==="
SONOBE_OK=0
if (cd spike/sonobe && cargo build --release 2>&1 | tail -5); then
    SONOBE_OK=1
fi
if [[ "$SONOBE_OK" -eq 1 && -f spike/sonobe/target/release/sonobe-script ]]; then
    echo "Sonobe binary: $(ls -lh spike/sonobe/target/release/sonobe-script | awk '{print $5}')"
else
    echo "Sonobe did not build — skipping. See spike/sonobe/README.md for the API churn note."
fi

# ---- Build Jolt ----
echo ""
echo "=== Building Jolt ==="
JOLT_OK=0
if (cd spike/jolt && cargo build --release 2>&1 | tail -5); then
    JOLT_OK=1
fi
if [[ "$JOLT_OK" -eq 1 && -f spike/jolt/target/release/jolt-script ]]; then
    echo "Jolt binary: $(ls -lh spike/jolt/target/release/jolt-script | awk '{print $5}')"
else
    echo "Jolt did not build — skipping. See spike/jolt/README.md for the SHA-pin note."
fi

# ---- Run Sonobe benchmarks ----
echo ""
echo "=== Sonobe bench (sha256-chain, max-steps=${MAX_STEPS:-1024}) ==="
if [[ "$SONOBE_OK" -eq 1 && -f spike/sonobe/target/release/sonobe-script ]]; then
    spike/sonobe/target/release/sonobe-script bench \
        --workload sha256-chain \
        --fixture-dir "$FIXTURES" \
        --max-steps "${MAX_STEPS:-1024}" \
        --out "$RESULTS/sonobe-sha256.json" || true
    spike/sonobe/target/release/sonobe-script bench \
        --workload toy-decode \
        --fixture-dir "$FIXTURES" \
        --max-steps "${MAX_STEPS:-1024}" \
        --out "$RESULTS/sonobe-toy-decode.json" || true
else
    echo "Skipping — Sonobe not built"
fi

# ---- Run Jolt benchmarks ----
echo ""
echo "=== Jolt bench ==="
if [[ "$JOLT_OK" -eq 1 && -f spike/jolt/target/release/jolt-script ]]; then
    # jolt build -p <pkg> needs to run inside the workspace dir.
    (
        cd spike/jolt
        timeout 1800 ./target/release/jolt-script bench \
            --workload sha256 \
            --fixture-dir ../common/bench-fixtures \
            --out ../bench/results/jolt-sha256.json
        timeout 1800 ./target/release/jolt-script bench \
            --workload toy-decode \
            --fixture-dir ../common/bench-fixtures \
            --out ../bench/results/jolt-toy-decode.json
    ) || true
else
    echo "Skipping — Jolt not built"
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
