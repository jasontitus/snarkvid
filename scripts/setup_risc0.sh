#!/usr/bin/env bash
# Install the RISC Zero toolchain (rzup + RISC-V cross-toolchain).
#
# RISC Zero 3.x switched from `cargo risczero install` to `rzup` as the
# canonical installer. rzup pulls down the riscv32im-risc0-zkvm-elf rust
# toolchain and the r0vm runtime/cargo-risczero CLI.
#
# Platform notes:
#   - macOS aarch64: requires full Xcode (Metal toolchain) for the Metal prover.
#   - Linux x86_64:  requires nvcc (CUDA toolkit) so the cuda prover backend builds.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$HOME/.risc0/bin:$PATH"

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
            echo "RISC Zero 3.x with --features cuda needs the CUDA toolkit." >&2
            echo "Install it (e.g. apt install nvidia-cuda-toolkit), or ensure" >&2
            echo "/usr/local/cuda/bin is on PATH on a Lambda Labs / cloud GPU box." >&2
            exit 1
        fi
        ;;
    *)
        echo "warning: unrecognized OS $(uname -s); proceeding anyway" >&2
        ;;
esac

# ----- rzup ------------------------------------------------------------------
if command -v rzup >/dev/null 2>&1; then
    echo "rzup already installed: $(rzup --version 2>&1 | head -1)"
else
    echo "Installing rzup..."
    curl -L https://risczero.com/install | bash
    export PATH="$HOME/.risc0/bin:$PATH"
fi

# rzup install (no args) installs the default toolchain components for the
# current host. This is the Linux/CUDA + RISC-V cross-toolchain bundle, or
# the macOS/Metal + RISC-V cross-toolchain bundle.
echo "Installing RISC Zero toolchain components via rzup..."
rzup install

echo ""
echo "Done. Verify with: rzup show && cargo-risczero --version"
