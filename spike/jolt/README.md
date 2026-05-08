# jolt — Jolt zkVM side of the M1b spike

Jolt is the a16z lookup-centric zkVM (Twist-and-Shout, Aug 2025; native ZK, Mar 2026). On lookup-heavy workloads it benches roughly **2× faster than SP1** in CPU-only settings.

## Why Jolt is in the survivor list (and why it's secondary, not primary)

| Filter | SP1 (M1 winner) | Jolt | Sonobe Nova |
|---|---|---|---|
| Browser-verifier path shipped | partial (Groth16 wrap, WASM glue) | **NO** — Groth16 wrap "in progress" since Nov 2024, no shipped WASM verifier | yes (DeciderEth → standard Groth16/BN254 WASM verifier) |
| Codec workload fit | RISC-V Rust guest | RISC-V Rust guest, lookup advantage | step circuits (re-implement) |
| CPU prove speed (SHA-256) | baseline | ~2× faster | unknown (CPU-only, fold-step model) |

Jolt's **browser verifier is not shipped**. We scaffold it anyway because:
1. The CPU prove-time advantage is large enough that if Jolt's Groth16 wrapper lands, it becomes a strong candidate for the H.264-decode milestone.
2. Lookup arguments are exactly what CAVLC entropy decode and intra-prediction tables want.
3. The integration cost is low — it accepts arbitrary `no_std` Rust like SP1, so `crates/toy-codec` drops in directly.

If Jolt's Groth16 wrapper hasn't shipped by the time we're picking the M3 prover, **drop Jolt and stay on SP1 or Sonobe**.

## Workloads

Two `#[jolt::provable]` entry points in **separate guest crates** (Jolt's macro generates a top-level `main` per provable, so multiple provables in one crate collide):

1. **`guest/src/lib.rs` — `sha2_preimage(min_size: u32, data: &[u8]) -> ([u8; 32], u32)`** — SHA-256 of input matches commitment, length ≥ min_size. Same statement as the SP1/RISC0 M1 guests; numbers compare directly.
2. **`guest-toy-decode/src/lib.rs` — `toy_decode_one_block(yuv_bytes: &[u8]) -> [u8; 32]`** — runs `decode_toy` from `crates/toy-codec` on a 16×16 4:2:0 frame and returns SHA-256 of the decoded YUV. This proves the M2 codec kernel can run unmodified inside a Jolt guest — the value of the zkVM model.

## Layout

```
spike/jolt/
├── Cargo.toml          # workspace root; host package; arkworks fork patches
├── src/main.rs         # host CLI
├── guest/
│   ├── Cargo.toml      # one provable: sha2_preimage
│   ├── src/lib.rs
│   └── src/main.rs     # #![no_main] re-export so cargo emits an ELF
└── guest-toy-decode/
    ├── Cargo.toml      # one provable: toy_decode_one_block
    ├── src/lib.rs
    └── src/main.rs
```

## Build

```
# 1. Install RISC-V targets (RV32 + RV64 IMAC)
rustup target add riscv32imac-unknown-none-elf riscv64imac-unknown-none-elf

# 2. Pin a known-good Jolt commit in spike/jolt/Cargo.toml. As of May 2026
#    Jolt has no semver release; the API shifted in Aug 2025 (Twist-and-
#    Shout) and Mar 2026 (native ZK / NovaBlindFold). Don't track HEAD.

# 3. Build
cd spike/jolt
cargo build --release   # cold compile ~5-10 min, ~3 GB target dir
```

## Run

`jolt-script` shells out to `jolt build -p <guest-pkg>` at runtime, which only resolves when invoked from inside the `spike/jolt` workspace. **Always cd to spike/jolt first.**

```
cd spike/jolt
./target/release/jolt-script prove   --workload sha256 --input ../common/bench-fixtures/fixture-1k.bin --min-size 1024 --out proof.bin --commit-out commit.hex
./target/release/jolt-script bench   --workload sha256 --fixture-dir ../common/bench-fixtures --out ../bench/results/jolt-sha256.json
./target/release/jolt-script bench   --workload toy-decode --fixture-dir ../common/bench-fixtures --out ../bench/results/jolt-toy-decode.json
```

`scripts/bench_jolt.sh` handles the cd for you.

## Risks (and known sandbox numbers)

Measured CPU prove on this sandbox: **1 KB SHA-256 = 4.3 s prove, 126 ms verify, 53k RV cycles, 11.3 GB peak RSS**. **6× faster than SP1's CPU 1 KB number** (26 s in M1 spike). Memory cost is heavy.

- **No browser verifier shipped.** Tracked, not blocked. Re-evaluate when Jolt's Groth16 wrap lands.
- **API churn.** Pin a SHA, don't track HEAD. Recent breaking changes:
  - Aug 2025 Twist-and-Shout — preprocessing split into 3 calls.
  - Mar 2026 NovaBlindFold (native ZK) — `preprocess_verifier_*` gained an `Option` arg.
- **One provable per guest crate.** The macro emits a top-level `main`. Adding a third workload means a third sibling crate.
- **`max_trace_length` is compile-time.** Set generously in `guest/src/lib.rs` (1 GiB SHA, 256 MiB toy-decode). May need to grow for 10 MB SHA fixtures; recompile guest if so.
- **No first-party CUDA.** LayerZero's "Zero" chain has a GPU prover; not in upstream `a16z/jolt`. CPU-only on the A10.
- **Cycle count not in BenchResult.** Jolt reports cycles via `tracing` logs (RUST_LOG=info); the CLI doesn't currently capture them programmatically. Read `tracing` output for now.
- **Proof bytes not yet captured.** `JoltProof` doesn't impl `Clone` or `serde::Serialize` in the resolved May 2026 graph; the host writes `proof_bytes: 0`. The proof is verified in-process during the same `prove` call.
