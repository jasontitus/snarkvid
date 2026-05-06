# snarkvid — M1 Mac Setup & Benchmarks

## What works on M1

Both zkVMs build and run on Apple Silicon.

- **SP1 6.1**: works out of the box. Prove, verify, bench all functional.
- **RISC Zero 1.2**: works *if* you have full Xcode (not just Command Line Tools), so the Metal toolchain is available — `risc0-r0vm` 1.2 force-enables the `metal` feature on `aarch64-apple-darwin` and needs to compile Metal shaders during install.

Memory matters more than CPU for the larger fixtures: 1 MB / 10 MB fixtures generate large segment counts, and CPU proving on M1 will swap heavily on machines with ≤16 GB RAM. See "Memory and the larger fixtures" below.

## Prerequisites

- macOS 13+ (Ventura or later)
- 16 GB RAM minimum for 1 KB; 32 GB recommended for 1 MB
- ~10 GB free disk
- **Full Xcode** (App Store) if you want the RISC Zero side. Command Line Tools alone are not enough.

After installing Xcode, point `xcode-select` at it:

```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
xcrun -f metal   # should resolve a path under .../Metal.xctoolchain/usr/bin/metal
```

## Quick start (SP1 only)

Smallest setup. Useful if you only have CLT, or only want SP1 numbers.

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Restart shell, then:

# 2. Clone
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid

# 3. Install SP1 toolchain
./scripts/setup_sp1.sh

# 4. Build & benchmark (1 KB fixture)
./scripts/bench_sp1.sh
```

Expected: ~15–30 minutes (mostly compilation). Produces `spike/bench/results/sp1.json`.

## Quick start (full head-to-head)

Runs both zkVMs on all fixtures.

```bash
# 1. Install Rust (rustup)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Restart shell, then:

# 2. Clone
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid

# 3. Install both toolchains
./scripts/setup_sp1.sh
./scripts/setup_risc0.sh

# 4. Enable RISC Zero deps
./scripts/uncomment_risc0_deps.sh

# 5. Build & benchmark (1 KB + 1 MB + 10 MB on both)
./scripts/bench_all.sh
```

Expected wall-clock: 60+ minutes for the full 1 KB + 1 MB run on M1; 10 MB on CPU is hours, not minutes (see GPU note below).

## Scripts

| Script | What it does |
|---|---|
| `scripts/setup_sp1.sh` | Installs `sp1up` → `cargo-prove` + `succinct` rustup toolchain |
| `scripts/setup_risc0.sh` | Installs `cargo-risczero` v1.2 + `risc0` rustup toolchain (needs Metal toolchain) |
| `scripts/uncomment_risc0_deps.sh` | Removes `#` prefixes from RISC Zero deps in three `Cargo.toml` files |
| `scripts/bench_sp1.sh` | Generates fixtures, builds SP1, runs SP1 bench |
| `scripts/bench_all.sh` | Generates fixtures, builds both, runs both benches; gracefully skips RISC0 if its build fails |

## What you'll get

`bench_all.sh` writes `spike/bench/results/sp1.json` and `spike/bench/results/risc0.json` and prints a head-to-head markdown table.

## Memory and the larger fixtures

CPU proving doesn't OOM-kill on macOS — it swaps to SSD, hard. Watch Activity Monitor → Memory: if the pressure graph is red, you're thrashing. The 1 MB row is the first place this bites; the 10 MB row is impractical on CPU regardless of RAM. For real 10 MB wall-clock numbers, rent a CUDA GPU instance (`g4dn.xlarge` on AWS, ~$0.50/hr) — neither prover has a Metal prover backend in 1.2 / 6.1.

## Troubleshooting

### "missing Metal Toolchain" during `setup_risc0.sh`

On `aarch64-apple-darwin`, `risc0-r0vm` v1.2 force-enables the `metal` feature, so the install must be able to compile Metal shaders. You need **full Xcode**, not just CLT.

```bash
# 1. Install Xcode from the App Store, then:
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer

# 2. Verify the Metal compiler resolves:
xcrun -f metal

# 3. Reinstall:
rm -f ~/.cargo/bin/cargo-risczero ~/.cargo/bin/r0vm
./scripts/setup_risc0.sh
```

Do **not** set `RISC0_SKIP_BUILD_KERNELS=1`. That flag skips the **CPU** C++ kernels too, producing undefined-symbol linker errors like `_risc0_circuit_rv32im_cpu_witgen`, `_risc0_circuit_recursion_step_*`. It is not a "skip Metal only" flag.

### `cargo +succinct` / `cargo +risc0` silently runs the wrong toolchain

This happens when Homebrew rust is installed alongside rustup — `/opt/homebrew/bin/cargo` precedes `~/.cargo/bin/cargo` in `PATH`, and Homebrew's cargo doesn't understand `+toolchain` syntax (it just ignores the arg). The setup scripts work around this by prepending `~/.cargo/bin` to `PATH` themselves. If you invoke cargo manually from the project, do the same:

```bash
export PATH="$HOME/.cargo/bin:$HOME/.sp1/bin:$PATH"
```

Or `brew uninstall rust` to remove the conflicting binary.

### `cargo prove` not found after `setup_sp1.sh`

`sp1up` writes to `~/.sp1/bin` and adds it to `~/.zshenv`. New terminals pick it up automatically; current shell needs:

```bash
source ~/.zshenv
# or:
export PATH="$HOME/.sp1/bin:$PATH"
```

### `cargo-risczero` installed v3+ instead of v1.2

```bash
rm -f ~/.cargo/bin/cargo-risczero ~/.cargo/bin/r0vm
cargo install cargo-risczero --version "^1.2"
```

### 1 MB benchmark hangs or swaps heavily

macOS swaps to SSD rather than OOM-kill. Check Activity Monitor → Memory. If the graph is red, the machine is thrashing — Ctrl-C and skip. The 1 KB data point alone is still a useful comparison.

### RISC Zero host doesn't build

Make sure you ran `uncomment_risc0_deps.sh` and are building with `--features risc0`. The host prints a helpful error if the feature isn't on.

## After the benchmarks

1. Copy the output table into `DECISION.md` at the repo root
2. Fill in "Decision", "Rationale", and "Risks accepted"
3. Commit and push
