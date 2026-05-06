#!/usr/bin/env bash
# Install the SP1 toolchain (cargo-prove + RISC-V target).
# Prerequisite: Rust (rustup) must already be installed.
set -euo pipefail

# Prefer rustup shims over a parallel Homebrew rust install — sp1-build uses
# `cargo +succinct ...`, which requires the rustup shim to interpret +toolchain.
export PATH="$HOME/.cargo/bin:$PATH"

if command -v cargo-prove &>/dev/null; then
    echo "cargo-prove already installed: $(cargo-prove --version 2>&1 | head -1)"
    exit 0
fi

echo "Installing sp1up..."
curl -L https://sp1.succinct.xyz | bash

# sp1up writes itself to ~/.sp1/bin and adds that path to ~/.zshenv, but the
# current shell hasn't sourced that yet — invoke it by its absolute path.
export PATH="$HOME/.sp1/bin:$PATH"

echo "Running sp1up to install cargo-prove + RISC-V toolchain..."
"$HOME/.sp1/bin/sp1up"

echo ""
echo "Done. Verify with: cargo prove --version"
