#!/usr/bin/env bash
# Install the RISC Zero toolchain (cargo-risczero + RISC-V target).
# Prerequisites: Rust (rustup), macOS with Xcode Command Line Tools.
#
# NOTE: RISC Zero v1.2.x has known linker failures on aarch64 macOS
# (undefined C++ circuit symbols). If this script fails with "Undefined
# symbols for architecture arm64", skip RISC Zero on this machine and
# use a Linux x86_64 instance for the head-to-head benchmark.
#
# Skips Metal GPU kernels — CPU-only benchmarks don't need them.
set -euo pipefail

# RISC Zero v1.2.x C++ circuit libraries don't link on aarch64 macOS.
# Try without default features to skip the r0vm binary (which pulls in
# the full circuit stack). We only need the `cargo risczero` subcommand
# to install the RISC-V toolchain.
if [[ "$(uname)" == "Darwin" && "$(uname -m)" == "arm64" ]]; then
    echo "WARNING: aarch64 macOS detected."
    echo "RISC Zero v1.2.x C++ circuit libraries won't link on this platform."
    echo "Attempting install without default features (skip r0vm binary)..."
    echo "If this fails, skip RISC Zero on this machine and run on Linux x86_64."
    echo ""
fi

if command -v cargo-risczero &>/dev/null; then
    echo "cargo-risczero already installed: $(cargo-risczero --version 2>&1 | head -1)"
    exit 0
fi

# Try installing with --no-default-features to avoid the r0vm binary
# which requires the C++ circuit libraries.
echo "Installing cargo-risczero v1.2 (no default features, CPU-only)..."
if RISC0_SKIP_BUILD_KERNELS=1 cargo install cargo-risczero --version "^1.2" --no-default-features 2>&1; then
    echo "Install succeeded without r0vm binary — that's fine for our use."
else
    echo ""
    echo "================================================================"
    echo "RISC Zero install failed on this machine."
    echo "This is expected on aarch64 macOS — the C++ circuit libraries"
    echo "don't have prebuilt binaries for this platform."
    echo ""
    echo "Recommendation: run only SP1 benchmarks here."
    echo "Use an x86_64 Linux instance for the full head-to-head comparison."
    echo "================================================================"
    exit 1
fi

echo "Installing RISC-V toolchain..."
cargo risczero install

echo ""
echo "Done. Verify with: cargo risczero --version"
