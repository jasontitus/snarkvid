#!/usr/bin/env bash
# Install the RISC Zero toolchain (cargo-risczero + RISC-V target).
# Prerequisites: Rust, full Xcode (Metal toolchain, not just Command Line Tools).
set -euo pipefail

# Prefer rustup shims (cargo risczero install adds a `risc0` toolchain via
# `rustup toolchain link`, which only works when cargo/rustc are shims).
export PATH="$HOME/.cargo/bin:$PATH"

if ! xcrun -f metal >/dev/null 2>&1; then
    echo "error: Metal compiler not found via xcrun." >&2
    echo "Install full Xcode and run: sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
    exit 1
fi

if command -v cargo-risczero &>/dev/null; then
    echo "cargo-risczero already installed: $(cargo-risczero --version 2>&1 | head -1)"
else
    echo "Installing cargo-risczero v1.2 (Metal-enabled; CPU-only proving still works at runtime)..."
    cargo install cargo-risczero --version "^1.2"
fi

echo "Installing RISC-V toolchain (prebuilt for Apple Silicon)..."
cargo risczero install

echo ""
echo "Done. Verify with: cargo risczero --version"
