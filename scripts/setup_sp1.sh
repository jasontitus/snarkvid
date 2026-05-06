#!/usr/bin/env bash
# Install the SP1 toolchain (cargo-prove + RISC-V target).
# Prerequisite: Rust (rustup) must already be installed.
set -euo pipefail

if command -v cargo-prove &>/dev/null; then
    echo "cargo-prove already installed: $(cargo-prove --version 2>&1 | head -1)"
    exit 0
fi

echo "Installing sp1up..."
curl -L https://sp1.succinct.xyz | bash

echo "Running sp1up to install cargo-prove + RISC-V toolchain..."
sp1up

echo ""
echo "Done. Verify with: cargo prove --version"
