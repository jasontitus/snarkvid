#!/usr/bin/env bash
# One-shot setup for a CUDA Linux box (e.g. Lambda Labs A100/H100).
# Installs system deps, Rust (rustup), SP1 toolchain, RISC Zero toolchain,
# and uncomments the RISC Zero deps so the next `bench_cuda.sh` will build.
set -euo pipefail

cd "$(dirname "$0")/.."

# ----- Sanity: this script is for Linux only ---------------------------------
if [[ "$(uname -s)" != "Linux" ]]; then
    echo "error: this script targets Linux. Use SETUP_MAC.md on macOS." >&2
    exit 1
fi

# ----- Sanity: GPU + CUDA toolkit --------------------------------------------
if ! command -v nvidia-smi >/dev/null 2>&1; then
    echo "error: nvidia-smi not found. This box appears to have no NVIDIA GPU." >&2
    exit 1
fi
nvidia-smi --query-gpu=name,memory.total --format=csv,noheader

# nvcc may live in /usr/local/cuda/bin on Lambda images — add it to PATH so
# subsequent installs (especially cargo-risczero, which builds CUDA kernels)
# can find it.
if ! command -v nvcc >/dev/null 2>&1; then
    if [[ -x /usr/local/cuda/bin/nvcc ]]; then
        export PATH="/usr/local/cuda/bin:$PATH"
    else
        echo "error: nvcc not found and /usr/local/cuda/bin/nvcc doesn't exist." >&2
        echo "Install the CUDA toolkit (apt install nvidia-cuda-toolkit) or fix PATH." >&2
        exit 1
    fi
fi
echo "nvcc: $(nvcc --version | tail -1)"

# ----- System deps -----------------------------------------------------------
# Lambda images already have build-essential; this is a defensive install.
# Skip the apt step entirely if every required package is already present,
# so that re-runs (and WSL boxes without passwordless sudo) don't get stuck.
if command -v apt-get >/dev/null 2>&1; then
    APT_PKGS=(build-essential pkg-config libssl-dev curl git ca-certificates clang protobuf-compiler)
    MISSING=()
    for pkg in "${APT_PKGS[@]}"; do
        dpkg -s "$pkg" >/dev/null 2>&1 || MISSING+=("$pkg")
    done
    if [[ ${#MISSING[@]} -eq 0 ]]; then
        echo "All apt build deps already installed; skipping apt step."
    else
        echo "Installing missing build deps via apt: ${MISSING[*]}"
        sudo apt-get update -qq
        sudo apt-get install -y --no-install-recommends "${MISSING[@]}"
    fi
fi

# ----- Rust (rustup) ---------------------------------------------------------
if ! command -v rustup >/dev/null 2>&1; then
    echo "Installing rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi
# shellcheck disable=SC1091
source "$HOME/.cargo/env"
export PATH="$HOME/.cargo/bin:$PATH"
rustup default stable
rustup update stable

# ----- SP1 -------------------------------------------------------------------
./scripts/setup_sp1.sh
export PATH="$HOME/.sp1/bin:$PATH"

# ----- RISC Zero -------------------------------------------------------------
# setup_risc0.sh detects Linux and re-checks for nvcc on PATH (already set above).
./scripts/setup_risc0.sh

# ----- Uncomment RISC Zero deps (idempotent) ---------------------------------
./scripts/uncomment_risc0_deps.sh

echo ""
echo "================================================================"
echo "Setup complete. Run benchmarks with:"
echo ""
echo "  ./scripts/bench_cuda.sh"
echo ""
echo "Or persist PATH for new shells by adding to ~/.bashrc:"
echo "  export PATH=\"\$HOME/.cargo/bin:\$HOME/.sp1/bin:/usr/local/cuda/bin:\$PATH\""
echo "================================================================"
