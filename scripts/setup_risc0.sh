#!/usr/bin/env bash
# Install the RISC Zero toolchain (cargo-risczero + RISC-V target).
# - macOS aarch64: requires full Xcode (Metal toolchain), risc0-r0vm 1.2 force-enables `metal`.
# - Linux x86_64: requires nvcc (CUDA toolkit), risc0-r0vm 1.2 force-enables `cuda`.
set -euo pipefail

# Prefer rustup shims (cargo risczero install adds a `risc0` toolchain via
# `rustup toolchain link`, which only works when cargo/rustc are shims).
export PATH="$HOME/.cargo/bin:$PATH"

case "$(uname -s)" in
    Darwin)
        if ! xcrun -f metal >/dev/null 2>&1; then
            echo "error: Metal compiler not found via xcrun." >&2
            echo "Install full Xcode and run: sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
            exit 1
        fi
        ;;
    Linux)
        if ! command -v nvcc >/dev/null 2>&1; then
            echo "error: nvcc not found." >&2
            echo "risc0-r0vm v1.2 enables the cuda feature on x86_64 by default and requires" >&2
            echo "the CUDA toolkit. Install it (e.g. apt install nvidia-cuda-toolkit), or" >&2
            echo "ensure /usr/local/cuda/bin is on PATH on a Lambda Labs / cloud GPU box." >&2
            exit 1
        fi
        ;;
    *)
        echo "warning: unrecognized OS $(uname -s); proceeding anyway" >&2
        ;;
esac

if command -v cargo-risczero &>/dev/null; then
    echo "cargo-risczero already installed: $(cargo-risczero --version 2>&1 | head -1)"
else
    echo "Installing cargo-risczero v1.2..."
    cargo install cargo-risczero --version "^1.2"
fi

echo "Installing RISC-V toolchain..."
cargo risczero install

echo ""
echo "Done. Verify with: cargo risczero --version"
