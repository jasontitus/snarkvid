#!/usr/bin/env bash
# Install the RISC Zero toolchain (cargo-risczero + RISC-V target).
# Prerequisites: Rust (rustup), macOS with Xcode Command Line Tools.
# Skips Metal GPU kernels — CPU-only benchmarks don't need them.
set -euo pipefail

if command -v cargo-risczero &>/dev/null; then
    echo "cargo-risczero already installed: $(cargo-risczero --version 2>&1 | head -1)"
else
    echo "Installing cargo-risczero v1.2 (CPU-only, no Metal GPU kernels)..."
    RISC0_SKIP_BUILD_KERNELS=1 cargo install cargo-risczero --version "^1.2"
fi

echo "Installing RISC-V toolchain (prebuilt for Apple Silicon)..."
cargo risczero install

echo ""
echo "Done. Verify with: cargo risczero --version"
