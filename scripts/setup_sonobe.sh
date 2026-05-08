#!/usr/bin/env bash
# Install everything needed to build the Sonobe (Nova+CycleFold) spike.
# Sonobe is consumed as a git dep, so this script just confirms the
# prerequisites exist and warms up the cargo registry.

set -euo pipefail

cd "$(dirname "$0")/.."

# Sonobe pins stable Rust 1.88.0 internally; our spike needs >= 1.88
# stable. Check the active rustc.
RUSTC_VERSION=$(rustc --version | awk '{print $2}')
echo "rustc: $RUSTC_VERSION"

# Heavy arkworks build. Warn the user about disk + time.
echo ""
echo "Sonobe pulls a heavy arkworks tree."
echo "Cold build takes ~6-10 min and ~3 GB of target/ space."
echo ""

# Pre-fetch the git dep so the first cargo build doesn't time out on a
# slow network. Optional but helpful in CI.
echo "Pre-fetching Sonobe dependencies..."
cd spike/sonobe
cargo fetch
cd ../..

echo ""
echo "Done. Now run: bash scripts/bench_sonobe.sh"
echo ""
echo "Note: there is no Sonobe semver release. Pin a specific commit"
echo "in spike/sonobe/Cargo.toml under [dependencies].folding-schemes"
echo "before benchmarking — HEAD churns and the API has moved twice in"
echo "the last six months."
