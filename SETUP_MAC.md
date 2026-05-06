# snarkvid — M1 Mac Setup & Benchmarks

Copy this file to your M1 Mac and follow along.

## Prerequisites

- macOS 13+ (Ventura or later)
- At least 16 GB RAM (32+ preferred for 1MB benchmark)
- ~10 GB free disk space
- Terminal (Terminal.app, iTerm2, etc.)

---

## Step 1: Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Choose "1" for default install
# Restart your shell or run: source ~/.cargo/env
```

Verify:
```bash
rustc --version   # should be 1.80+
```

---

## Step 2: Clone the repo

```bash
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid
```

Or if you already have it checked out locally, just `cd` into it.

---

## Step 3: Run the setup script

Save the script at the bottom of this file as `setup_and_bench.sh`, then:

```bash
chmod +x setup_and_bench.sh
./setup_and_bench.sh
```

It will:
1. Install `cargo-prove` (SP1 CLI)
2. Install `cargo-risczero` (RISC Zero CLI)
3. Install both RISC-V toolchains
4. Uncomment RISC Zero dependencies in Cargo.toml files
5. Generate benchmark fixtures
6. Build both zkVM sides
7. Run CPU benchmarks on 1KB + 1MB fixtures
8. Print a summary table

Expected time: 20–60 minutes (mostly compilation).

---

## What the benchmarks will tell you

| Metric | Why it matters |
|---|---|
| SP1 prove 1KB time | Sanity check — should match ~20-30s |
| RISC Zero prove 1KB time | Head-to-head CPU comparison |
| SP1 prove 1MB time/crash | Does M1 swap handle it? |
| RISC Zero prove 1MB time | Same test |
| Proof sizes | Which is smaller? |
| Verify times | Browser viability |

---

## If 1MB OOMs or hangs

macOS will swap to SSD rather than OOM-kill (unlike Linux). If the 1MB benchmark hasn't finished after 15 minutes, check Activity Monitor for memory pressure (red graph = too much swap). If it's red, Ctrl-C and skip to the comparison — the 1KB data point is still useful.

-------------------------------------------------------------------------------
# setup_and_bench.sh
# Copy everything below into a file and run it on your M1 Mac.
-------------------------------------------------------------------------------

```bash
#!/usr/bin/env bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log()  { echo -e "${GREEN}[+]${NC} $*"; }
warn() { echo -e "${YELLOW}[!]${NC} $*"; }
err()  { echo -e "${RED}[X]${NC} $*"; }

# ---- Check we're on macOS aarch64 ----
if [[ "$(uname)" != "Darwin" ]]; then
    err "This script is for macOS only."
    exit 1
fi

log "snarkvid M1 setup starting..."
log "Arch: $(uname -m), RAM: $(sysctl -n hw.memsize | awk '{printf "%.0f GB", $1/1024/1024/1024}')"

# ---- Make sure we're in the repo root ----
if [[ ! -f "DESIGN.md" ]]; then
    err "Run this from the snarkvid repo root."
    exit 1
fi

# ---- Install SP1 toolchain ----
log "Installing cargo-prove (SP1 CLI)..."
if ! command -v cargo-prove &>/dev/null; then
    cargo install cargo-prove 2>&1 | tail -3
fi

log "Installing SP1 RISC-V toolchain..."
cargo prove install 2>&1 | tail -5

# ---- Install RISC Zero toolchain ----
log "Installing cargo-risczero (v1.2, CPU-only)..."
if ! command -v cargo-risczero &>/dev/null; then
    # Pin v1.2 — v3.x has a completely different API and our code targets 1.x.
    # Skip Metal GPU kernels since we're doing CPU benchmarks.
    RISC0_SKIP_BUILD_KERNELS=1 cargo install cargo-risczero --version "^1.2" 2>&1 | tail -3
fi

log "Installing RISC Zero RISC-V toolchain (prebuilt for Apple Silicon)..."
cargo risczero install 2>&1 | tail -5

# ---- Uncomment RISC Zero dependencies ----
log "Enabling RISC Zero dependencies in Cargo.toml files..."

# Function to uncomment all commented dep lines in a file (risc0-specific ones)
uncomment_risc0_deps() {
    local file="$1"
    # Uncomment lines with risc0-zkvm, risc0-build, snarkvid-spike-risc0, or bincode (the risc0-related ones)
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' 's/^# \(risc0-zkvm\|risc0-build\|bincode\|snarkvid-spike-risc0\)/\1/' "$file"
    else
        sed -i 's/^# \(risc0-zkvm\|risc0-build\|bincode\|snarkvid-spike-risc0\)/\1/' "$file"
    fi
}

uncomment_risc0_deps spike/risc0/host/Cargo.toml
uncomment_risc0_deps spike/risc0/methods/Cargo.toml
uncomment_risc0_deps spike/risc0/methods/guest/Cargo.toml

# Verify the uncommenting worked
for f in spike/risc0/host/Cargo.toml spike/risc0/methods/Cargo.toml spike/risc0/methods/guest/Cargo.toml; do
    if grep -q '^# \(risc0\|bincode\|snarkvid\)' "$f"; then
        warn "Some deps still commented in $f — check manually"
    fi
done

# ---- Generate fixtures ----
log "Generating benchmark fixtures..."
bash spike/common/bench-fixtures/gen.sh

# ---- Build SP1 side ----
log "Building SP1 (script + program)..."
cd spike/sp1
cargo build --release 2>&1 | tail -3
cd ../..

log "SP1 binary: $(ls -lh spike/sp1/target/release/sp1-script | awk '{print $5}')"

# ---- Build RISC Zero side ----
log "Building RISC Zero (host + methods + guest)..."
cd spike/risc0
# Build the guest first via risc0-build
cargo build --release --features risc0 2>&1 | tail -5
cd ../..

log "RISC0 binary: $(ls -lh spike/risc0/target/release/risc0-host 2>/dev/null | awk '{print $5}' || echo 'not found — check build output')"

# ---- Run benchmarks ----
log "============================================="
log "Running benchmarks..."
log "============================================="

FIXTURES="spike/common/bench-fixtures"
RESULTS="spike/bench/results"
mkdir -p "$RESULTS"

# SP1: probe 1KB
log ""
log "--- SP1 1KB bench ---"
if timeout 120 spike/sp1/target/release/sp1-script bench \
    --fixture-dir "$FIXTURES" --out "$RESULTS/sp1.json" 2>&1; then
    log "SP1 bench complete"
else
    warn "SP1 bench failed or timed out — check $RESULTS/sp1.json"
fi

# RISC Zero: probe 1KB
log ""
log "--- RISC Zero 1KB bench ---"
if timeout 600 spike/risc0/target/release/risc0-host bench \
    --fixture-dir "$FIXTURES" --out "$RESULTS/risc0.json" 2>&1; then
    log "RISC Zero bench complete"
else
    warn "RISC Zero bench failed or timed out — check $RESULTS/risc0.json"
fi

# ---- Print summary ----
log ""
log "============================================="
log "Results"
log "============================================="

echo ""
echo "Files written:"
ls -lh "$RESULTS/"

# Quick comparison if Python3 is available
if command -v python3 &>/dev/null && [[ -f "$RESULTS/sp1.json" && -f "$RESULTS/risc0.json" ]]; then
    echo ""
    python3 spike/bench/compare.py --markdown "$RESULTS/risc0.json" "$RESULTS/sp1.json"
else
    echo ""
    echo "SP1 results:"
    cat "$RESULTS/sp1.json" 2>/dev/null | python3 -m json.tool 2>/dev/null || echo "(not available)"
    echo ""
    echo "RISC0 results:"
    cat "$RESULTS/risc0.json" 2>/dev/null | python3 -m json.tool 2>/dev/null || echo "(not available)"
fi

log ""
log "Done! Save the output above and use it to fill in DECISION.md."
```

-------------------------------------------------------------------------------

## Manual steps if the script fails

### If `cargo install cargo-risczero` fails with "missing Metal Toolchain"

This happens when cargo installs v3.x instead of v1.2. Fix:

```bash
# Clean up the partial v3 install
rm -rf ~/.cargo/bin/cargo-risczero

# Install v1.2 with Metal kernels skipped (CPU-only benchmarks)
RISC0_SKIP_BUILD_KERNELS=1 cargo install cargo-risczero --version "^1.2"

# Then install the RISC-V toolchain
cargo risczero install
```

If you want GPU (Metal) proving later, install the Metal toolchain first:
```bash
xcodebuild -downloadComponent MetalToolchain
# Then reinstall without RISC0_SKIP_BUILD_KERNELS
```

### If `cargo prove install` fails
```bash
# Alternative: use sp1up
curl -L https://sp1.succinct.xyz | bash
sp1up
```

### If RISC Zero deps aren't uncommented
Manually edit these three files and remove the `# ` prefix from lines starting with `# risc0`, `# bincode`, or `# snarkvid`:
- `spike/risc0/host/Cargo.toml`
- `spike/risc0/methods/Cargo.toml`  
- `spike/risc0/methods/guest/Cargo.toml`

### If RISC Zero host doesn't build
The host uses `--features risc0`. Without it, it prints a helpful error. Make sure the feature is enabled:
```bash
cd spike/risc0 && cargo build --release --features risc0
```

### If 1MB benchmark kills the process
On macOS it shouldn't (swap is automatic), but if it does:
```bash
# Check memory pressure
memory_pressure

# Or monitor in Activity Monitor → Memory tab
# Green = ok, Yellow = warning, Red = too much swap
```

---

## After the benchmarks

1. Copy the output table into `DECISION.md` at the repo root
2. Fill in the "Decision", "Rationale", and "Risks accepted" sections
3. Commit and push

For the **10MB GPU benchmark** (the final go/no-go number), you'll still need a CUDA GPU. The CPU numbers from this run tell you which zkVM is faster per-cycle; rent a GPU instance (e.g., AWS `g4dn.xlarge`, ~$0.50/hr) to get the 10MB wall-clock number.
