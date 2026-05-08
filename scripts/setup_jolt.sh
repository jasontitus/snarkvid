#!/usr/bin/env bash
# Install everything needed to build the Jolt spike. Jolt is alpha and
# has no semver / crates.io publication, so we consume it as a git dep
# pinned to a specific commit in spike/jolt/Cargo.toml.
#
# Prerequisite: rustup must be installed.

set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"

# Jolt's rust-toolchain.toml pins stable 1.94 with the RV32 + RV64 IMAC
# bare-metal targets. We need at least the targets installed for our
# host crate to compile the guest.
echo "Installing RISC-V targets..."
rustup target add riscv32imac-unknown-none-elf
rustup target add riscv64imac-unknown-none-elf

# Install the `jolt` CLI binary that the host shells out to at runtime
# to cross-compile the guest. Without this, the host panics with
# "failed to run jolt - make sure it's installed".
echo ""
echo "Installing the jolt CLI (cargo install --git ...). Heavy build."
cargo install --git https://github.com/a16z/jolt --branch main --force --bins jolt

# Pre-fetch the Jolt git dep. Heavy: ~60-crate workspace + arkworks.
echo ""
echo "Pre-fetching Jolt dependencies (this is heavy)..."
cd "$(dirname "$0")/.."
cd spike/jolt
cargo fetch
cd ../..

echo ""
echo "Done."
echo ""
echo "Note: Jolt has no semver release in May 2026. Pin a specific commit"
echo "in spike/jolt/Cargo.toml [workspace.dependencies] before benchmarking."
echo "API has moved twice in 9 months: Aug 2025 (Twist-and-Shout) split"
echo "preprocess into 3 calls; Mar 2026 (NovaBlindFold) added an Option arg."
echo ""
echo "Then run: bash scripts/bench_jolt.sh"
