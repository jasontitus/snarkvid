#!/usr/bin/env bash
# Full-coverage test/bench script for a CUDA-equipped Linux box.
#
# Runs everything that this sandbox couldn't run (and re-runs the
# things it could, to confirm parity). Intended use:
#
#   git clone https://github.com/jasontitus/snarkvid.git
#   cd snarkvid
#   ./scripts/full_test_gpu.sh
#   # ... wait for green PASS line; review summary; shut the box down ...
#
# Phases (each phase is gated on its prereq, but a failure in one
# phase does NOT abort later phases — the goal is to get as much
# coverage as possible per instance-hour):
#
#   1. Sanity        nvidia-smi, rustup, cargo, protoc, sp1up
#   2. Setup         install SP1 / RISC0 / Sonobe / Jolt toolchains
#   3. Unit tests    workspace cargo test (toy-codec, comparator, manifest)
#   4. CUDA benches  SP1 + RISC0 SHA-256 fixtures (1k/1m/10m) on GPU
#   5. SP1 toy-decode  SP1 16x16 frame, 3-way parity row
#   6. Jolt          SHA-256 (1k) + toy-decode (16x16); CPU-only
#   7. Sonobe IVC    SHA-256 chain + toy-decode; CPU-only
#   8. Sonobe Decider  Groth16 wrap on top of IVC accumulator (load-bearing
#                    browser-verifier evidence; OOM'd in the 15 GiB sandbox)
#   9. Summary       PASS/FAIL line per phase, plus consolidated JSON tree
#
# The script prints PHASE markers and a final summary block, so it's easy
# to scrape exit status from the tail of the log.
#
# Time budget on a Lambda A10 (~$0.75/hr): roughly 90–150 min depending
# on whether RISC Zero compiles from scratch. Most of phase 4 is GPU
# wall-clock for the 10 MB row.

set -uo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
RESULTS="$ROOT/spike/bench/results"
mkdir -p "$RESULTS"

# Path includes for tools installed under $HOME by the various setup scripts.
export PATH="$HOME/.cargo/bin:$HOME/.sp1/bin:$HOME/.risc0/bin:/usr/local/cuda/bin:$PATH"

# Per-phase status tracking. We never `set -e` outside the per-step
# wrappers — a failure in phase N is recorded and reported but does NOT
# abort phase N+1.
declare -A STATUS
declare -A NOTES
declare -a PHASE_ORDER

phase_start() {
    local name="$1"
    PHASE_ORDER+=("$name")
    STATUS[$name]="RUN"
    echo ""
    echo "════════════════════════════════════════════════════════════════"
    echo "PHASE: $name"
    echo "════════════════════════════════════════════════════════════════"
}

phase_pass() {
    local name="$1"
    STATUS[$name]="PASS"
    NOTES[$name]="${2:-}"
    echo "→ PHASE $name: PASS ${NOTES[$name]}"
}

phase_fail() {
    local name="$1"
    STATUS[$name]="FAIL"
    NOTES[$name]="${2:-failed}"
    echo "→ PHASE $name: FAIL — ${NOTES[$name]}" >&2
}

phase_skip() {
    local name="$1"
    STATUS[$name]="SKIP"
    NOTES[$name]="${2:-skipped}"
    echo "→ PHASE $name: SKIP — ${NOTES[$name]}"
}

# Run cmd; on success call phase_pass, on failure phase_fail. The cmd
# is run inside a subshell so its `set -e` and trap state don't bleed.
run_phase() {
    local name="$1"; shift
    local note_on_pass="${RUN_NOTE:-}"
    if "$@"; then
        phase_pass "$name" "$note_on_pass"
        return 0
    else
        phase_fail "$name" "exit $? from: $*"
        return 1
    fi
}

# ─────────────────────────────────────────────────────────────────────
# 1. Sanity
# ─────────────────────────────────────────────────────────────────────
phase_start "1-sanity"
SANITY_OK=1
if ! command -v nvidia-smi >/dev/null 2>&1; then
    echo "no nvidia-smi → CUDA-only phases will SKIP"
    HAS_GPU=0
else
    nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader || true
    HAS_GPU=1
fi
command -v cargo >/dev/null || { echo "missing cargo"; SANITY_OK=0; }
command -v protoc >/dev/null || {
    echo "installing protoc..."
    sudo apt-get install -y protobuf-compiler || SANITY_OK=0
}
if [[ "$SANITY_OK" -eq 1 ]]; then
    phase_pass "1-sanity" "GPU=$HAS_GPU"
else
    phase_fail "1-sanity" "missing toolchain prerequisites"
fi

# ─────────────────────────────────────────────────────────────────────
# 2. Setup (idempotent)
# ─────────────────────────────────────────────────────────────────────
phase_start "2-setup"
SETUP_OK=1
if [[ "$HAS_GPU" -eq 1 ]]; then
    bash scripts/setup_linux_cuda.sh || SETUP_OK=0
fi
bash scripts/setup_sonobe.sh || SETUP_OK=0
bash scripts/setup_jolt.sh    || SETUP_OK=0
# SP1 toolchain (cargo-prove + +succinct rust); already invoked by
# setup_linux_cuda.sh, but re-run defensively for non-GPU paths.
bash scripts/setup_sp1.sh     || SETUP_OK=0
if [[ "$SETUP_OK" -eq 1 ]]; then
    phase_pass "2-setup" "all toolchains installed"
else
    phase_fail "2-setup" "one or more setup_*.sh exited non-zero"
fi

# ─────────────────────────────────────────────────────────────────────
# 3. Unit tests (workspace)
# ─────────────────────────────────────────────────────────────────────
phase_start "3-unit-tests"
if cargo test --release --workspace 2>&1 | tail -20; then
    phase_pass "3-unit-tests" "all unit tests green"
else
    phase_fail "3-unit-tests" "cargo test failed"
fi

# ─────────────────────────────────────────────────────────────────────
# 4. CUDA SHA-256 sweep (SP1 + RISC0)
# ─────────────────────────────────────────────────────────────────────
phase_start "4-cuda-sha256"
if [[ "$HAS_GPU" -eq 1 ]]; then
    if bash scripts/bench_cuda.sh; then
        phase_pass "4-cuda-sha256" "sp1.json + risc0.json written"
    else
        phase_fail "4-cuda-sha256" "bench_cuda.sh exit non-zero"
    fi
else
    phase_skip "4-cuda-sha256" "no GPU"
fi

# ─────────────────────────────────────────────────────────────────────
# 5. SP1 toy-decode (3-way parity workload)
#    Code-complete in this branch but never built in the sandbox
#    (sp1up couldn't reach api.github.com). First build on the A10
#    cross-compiles the new toy-decode RISC-V guest.
# ─────────────────────────────────────────────────────────────────────
phase_start "5-sp1-toy-decode"
SP1_TOY_OK=1
(
    cd spike/sp1
    cargo build --release ${HAS_GPU:+--features cuda}
) || SP1_TOY_OK=0
if [[ "$SP1_TOY_OK" -eq 1 ]]; then
    spike/sp1/target/release/sp1-script bench \
        --workload toy-decode \
        --fixture-dir spike/common/bench-fixtures \
        --out "$RESULTS/sp1-toy-decode.json" || SP1_TOY_OK=0
fi
if [[ "$SP1_TOY_OK" -eq 1 ]]; then
    phase_pass "5-sp1-toy-decode" "$(jq -r '.rows[0] | "\(.cycles) cycles, prove \(.prove_ms)ms, proof \(.proof_bytes)B"' "$RESULTS/sp1-toy-decode.json" 2>/dev/null || echo done)"
else
    phase_fail "5-sp1-toy-decode" "build or bench failed"
fi

# ─────────────────────────────────────────────────────────────────────
# 6. Jolt
# ─────────────────────────────────────────────────────────────────────
phase_start "6-jolt"
JOLT_OK=1
(cd spike/jolt && cargo build --release) || JOLT_OK=0
if [[ "$JOLT_OK" -eq 1 ]]; then
    (
        cd spike/jolt
        timeout 1800 ./target/release/jolt-script bench --workload sha256 \
            --fixture-dir ../common/bench-fixtures --out ../bench/results/jolt-sha256.json
        timeout 1800 ./target/release/jolt-script bench --workload toy-decode \
            --fixture-dir ../common/bench-fixtures --out ../bench/results/jolt-toy-decode.json
    ) || JOLT_OK=0
fi
if [[ "$JOLT_OK" -eq 1 ]]; then
    phase_pass "6-jolt" "sha256 + toy-decode rows written"
else
    phase_fail "6-jolt" "build or bench failed"
fi

# ─────────────────────────────────────────────────────────────────────
# 7. Sonobe IVC (no Decider)
# ─────────────────────────────────────────────────────────────────────
phase_start "7-sonobe-ivc"
SONOBE_OK=1
(cd spike/sonobe && cargo build --release) || SONOBE_OK=0
if [[ "$SONOBE_OK" -eq 1 ]]; then
    spike/sonobe/target/release/sonobe-script bench \
        --workload sha256-chain --fixture-dir spike/common/bench-fixtures \
        --max-steps 1024 --out "$RESULTS/sonobe-sha256.json" || SONOBE_OK=0
    spike/sonobe/target/release/sonobe-script bench \
        --workload toy-decode --fixture-dir spike/common/bench-fixtures \
        --max-steps 1024 --out "$RESULTS/sonobe-toy-decode.json" || SONOBE_OK=0
fi
if [[ "$SONOBE_OK" -eq 1 ]]; then
    phase_pass "7-sonobe-ivc" "IVC bench rows written"
else
    phase_fail "7-sonobe-ivc" "build or bench failed"
fi

# ─────────────────────────────────────────────────────────────────────
# 8. Sonobe Decider (Groth16 wrap)
#    OOM'd in the 15 GiB sandbox at ~16 GB anon-rss during Groth16 setup.
#    A Lambda A10 instance has ≥ 32 GB; should fit comfortably.
# ─────────────────────────────────────────────────────────────────────
phase_start "8-sonobe-decider"
DECIDER_OK=1
if [[ -x spike/sonobe/target/release/sonobe-script ]]; then
    # Use a small step count — Decider cost is dominated by the constant-
    # sized Groth16 setup, not the IVC fold count.
    spike/sonobe/target/release/sonobe-script bench \
        --workload sha256-chain --fixture-dir spike/common/bench-fixtures \
        --max-steps 8 --decider \
        --out "$RESULTS/sonobe-sha256-decider.json" || DECIDER_OK=0
else
    DECIDER_OK=0
fi
if [[ "$DECIDER_OK" -eq 1 ]]; then
    phase_pass "8-sonobe-decider" "$(jq -r '.rows[0] | "proof \(.proof_bytes)B, verify \(.verify_native_ms)ms"' "$RESULTS/sonobe-sha256-decider.json" 2>/dev/null || echo done)"
else
    phase_fail "8-sonobe-decider" "Groth16 wrap failed (OOM? rerun with bigger instance)"
fi

# ─────────────────────────────────────────────────────────────────────
# 9. Summary
# ─────────────────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════════════════════════════"
echo "SUMMARY"
echo "════════════════════════════════════════════════════════════════"
PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
for p in "${PHASE_ORDER[@]}"; do
    s="${STATUS[$p]:-?}"
    n="${NOTES[$p]:-}"
    case "$s" in
        PASS) PASS_COUNT=$((PASS_COUNT+1)); printf "  [PASS]  %-22s  %s\n" "$p" "$n" ;;
        FAIL) FAIL_COUNT=$((FAIL_COUNT+1)); printf "  [FAIL]  %-22s  %s\n" "$p" "$n" ;;
        SKIP) SKIP_COUNT=$((SKIP_COUNT+1)); printf "  [SKIP]  %-22s  %s\n" "$p" "$n" ;;
        *)    printf "  [%-4s]  %-22s  %s\n" "$s" "$p" "$n" ;;
    esac
done
echo ""
echo "Results JSON written to:"
ls -1 "$RESULTS"/*.json 2>/dev/null | sed 's/^/  /'

echo ""
if [[ "$FAIL_COUNT" -eq 0 ]]; then
    echo "DONE — $PASS_COUNT phases passed, $SKIP_COUNT skipped, 0 failed."
    echo "Safe to terminate the instance."
    exit 0
else
    echo "DONE — $PASS_COUNT phases passed, $SKIP_COUNT skipped, $FAIL_COUNT FAILED."
    echo "Review the FAIL lines above before terminating."
    exit 1
fi
