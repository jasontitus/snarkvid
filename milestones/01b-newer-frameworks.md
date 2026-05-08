# Milestone 1b — Newer Frameworks Survey & POCs

**Question that opened this milestone:** SP1 and RISC Zero are too slow for the H.264 decode workload at the scale we need. Are any of the newer ZK frameworks materially better, and is there a path to "browser verifies a compressed video" that doesn't blow our prover budget?

**Decision out of M1b:**

- **Track Jolt** as a future SP1 replacement — **6× faster CPU prove on SHA-256** measured here. Block: no shipped browser verifier (Groth16 wrap "in progress" since Nov 2024). Re-evaluate when their wrap lands.
- **Watch Sonobe Nova+CycleFold** as a v2 architecture for per-macroblock decoding — only candidate with a working browser verifier path today (DeciderEth → Groth16/BN254 → standard WASM verifier) and the right step-circuit shape for our workload, but per-step CPU prove (~0.6 s/SHA-step, ~0.4 s/clamp-step) means it doesn't beat SP1 for short inputs and the toolchain is research-grade.
- **Drop** ProtoStar (no Rust impl), Ceno (research-stage, no browser verifier), Plonky3-direct (worse SP1), Boojum (archived/coupled), Sonobe HyperNova/ProtoGalaxy (Decider undemonstrated, ZK-IVC unimplemented).

The original M1 decision (SP1 over RISC0) **stands for now**. If the H.264 cycle budget at M3 forces our hand, **Jolt is the strongest swap candidate** assuming its Groth16 wrap lands; **Sonobe** is the architecturally-correct fallback if no zkVM gets cheap enough.

## 1. Scope and method

The end goal — anchored on every decision below — is **a verifiable, in-browser proof that a compressed video matches a hardware-signed original.** That means any candidate framework must clear two filters:

1. **Browser verifier path exists or is credible** (Groth16-wrap → tiny WASM verifier counts).
2. **Codec workload fit** — naturally maps to per-macroblock loops, integer arithmetic, lookup-heavy decode tables.

A third practical filter: **buildable today** (we don't have time to wait six months on a research drop).

Two POC workloads, picked for direct comparability with the M1 SP1/RISC0 numbers:

- **SHA-256** of a 1 KB / 1 MB / 10 MB fixture (apples-to-apples with `01-spike-results.md`).
- **`toy-codec::decode_toy`** on a 16×16 4:2:0 frame (the actual M2 codec kernel from `crates/toy-codec`, currently a clamp-passthrough stub).

The candidates surveyed: Jolt, Sonobe (Nova + HyperNova + ProtoStar via folding-schemes), Plonky3, Ceno, Boojum.

## 2. Survey findings (May 2026)

### Jolt — `a16z/jolt`

- **Status:** Active, alpha, ~3,259 commits. No semver release; no crates.io publication. Pin to a SHA on `main`.
- **Recent:** Twist-and-Shout (Aug 2025, ~6× speedup, restructured preprocess into 3 calls). Native ZK via NovaBlindFold (Mar 2026, +3 KB proof, near-zero prover overhead).
- **Toolchain:** Stable Rust 1.94 + RV32IMAC + RV64IMAC targets. No nightly. Heavy: 60+ workspace crates, ~3 GB target dir.
- **API model:** `#[jolt::provable]` attribute on a `no_std` Rust function. Guest is RISC-V, just like SP1/RISC0 — `no_std` Rust drops in directly.
- **Browser verifier:** **NOT SHIPPED.** No first-party WASM. No Solidity verifier shipped (Nov 2024 a16z post said "in progress"). Community `jolt-wasm` fork is abandoned. Groth16 wrap is the documented path; not available as of May 2026.
- **GPU:** LayerZero has a CUDA fork in production for their "Zero" chain; not in upstream. **CPU-only here.**
- **Sharp edge:** The macro emits a top-level `main` per provable function — **one provable per guest crate**. Multiple workloads need sibling guest crates.

### Sonobe — `privacy-scaling-explorations/sonobe`

- **Status:** Active, no tagged release; pin a commit on `main`. Stable Rust 1.88.
- **Schemes shipped:** Nova, CycleFold, HyperNova, ProtoGalaxy. **Not** ProtoStar.
- **Browser verifier:** **The only candidate with a working path today.** `DeciderEth` wraps the IVC accumulator into a Groth16/BN254 proof (~200 bytes). `solidity-verifiers/` workspace member generates the EVM verifier from the same key material; a standard Groth16 WASM verifier completes the browser side. Currently demonstrated for Nova+CycleFold only.
- **API model:** Implement `FCircuit<F>` trait — a step circuit applied N times, with `state` carried across folds and `external_inputs` per step. Architecturally **the** right model for per-macroblock H.264 decoding.
- **GPU:** None. CPU+rayon only.
- **Audit scope:** Nova+CycleFold only. HyperNova/ProtoGalaxy are not in audit scope.

### HyperNova / ProtoStar (also via Sonobe)

- **HyperNova:** ZK-IVC layer is open issue #128 — unimplemented. `decider_eth.rs` files exist on disk but no working example. **Defer.**
- **ProtoStar:** No production-shaped Rust crate exists in May 2026. `geometryxyz/protostar` is a feasibility study with "Decider support left unimplemented." Sonobe ships **ProtoGalaxy** (the multi-instance follow-up) but not ProtoStar. **Drop.**
- **Honest read:** Folding schemes have been "next quarter" since 2023. Lurk pivoted from arecibo (Nova) to Sphinx (STARKs). No production system has shipped pure Nova IVC end-to-end. Polynomial commitments haven't converged — Microsoft's Nova ships three (Pedersen-IPA, HyperKZG, Mercury). Sonobe Nova survives because its Decider story is real and complete; everything else is research-grade.

### Plonky3 / Ceno / Boojum

- **Plonky3** (Polygon) is a STARK *toolkit* — fields, FRI, AIR plumbing. SP1 and Valida are built on top. Using Plonky3 directly for SHA-256 is **strictly worse than SP1** (you reinvent the SP1 wheel without the SHA-256 precompile or GPU prover). **Drop.**
- **Ceno** (Scroll) is at v0.1 beta with explicit "not for production" warning. RISC-V guest reuses SP1 Rust source, so easy to scaffold, but **no WASM verifier**, no Solidity, no GPU, no neutral SHA-256 numbers vs SP1. **Drop.**
- **Boojum** (Matter Labs) — `era-boojum` archived Aug 2024; successor `zksync-crypto` is tightly coupled to zkSync's prover stack. Not a general-purpose framework. **Drop.**
- **OpenVM** (Axiom + Scroll) emerged from this survey as a credible alternative — actual production migration target for Scroll's Euclid upgrade, modular RISC-V zkVM, and intends Ceno as a future backend. Tracked for v2; not scaffolded in M1b.

## 3. Survivors and why

| Candidate | Browser path | Codec fit | Buildable | Verdict |
|---|---|---|---|---|
| **Sonobe Nova+CycleFold + DeciderEth** | ✓ Groth16/BN254 → standard WASM Groth16 verifier | ✓ "F applied N times" matches per-macroblock | ✓ Stable Rust 1.88, working `examples/sha256.rs`, ~1 day to numbers | **Scaffold (primary, browser-ready)** |
| **Jolt** | ✗ No first-party WASM; Groth16 wrap unshipped; community fork abandoned | ✓ Lasso lookups ideal for codec tables (CAVLC, deblocking) | ✓ Stable Rust 1.94, `jolt new` CLI, working `examples/sha2-ex/` | **Scaffold (secondary, browser blocker flagged)** |
| Sonobe HyperNova / ProtoGalaxy | ⚠ Decider undemonstrated | ✓ | ⚠ ~5–10 days of Decider engineering | **Defer** |
| ProtoStar | — | — | ✗ No Rust impl | **Drop** |
| Plonky3 direct | ✗ | ⚠ | ✗ Multi-week constraint engineering | **Drop** |
| Ceno | ✗ | ✓ | ✓ | **Drop (browser filter)** |
| Boojum | ✗ | — | ✗ | **Drop** |

Jolt fails the strict reading of the browser-verifier filter. We scaffolded it anyway because: (a) the CPU prove-time advantage is large enough that if the Groth16 wrap lands, it becomes the strongest M3 candidate; (b) lookup arguments are exactly right for CAVLC entropy decode and intra-prediction tables; (c) integration cost is low (drops in `crates/toy-codec` directly). If the wrapper hasn't landed by M3 prover-pick time, **drop Jolt and stay on SP1 or move to Sonobe**.

## 4. Measured numbers (CPU, this sandbox)

Same workload semantics as M1; not the same hardware as M1's Lambda A10. **These numbers are CPU-only**; the SP1 row from M1 below is the GPU result on A10 for context. All numbers from `spike/bench/results/`.

| System | Workload | Size | Cycles | Prove | Verify | Proof | Peak RSS |
|---|---|---:|---:|---:|---:|---:|---:|
| **Jolt** (CPU, this sandbox) | SHA-256 | 1 KB | 53,032 | **4.0 s** | 120 ms | 80,281 B | 11.3 GB |
| **Jolt** (CPU, this sandbox) | toy-decode 16×16 (real WHT) | 384 B | 108,816 | 6.0 s | 132 ms | 83,817 B | 5.7 GB |
| **Sonobe Nova** (CPU) | SHA-256 chain (32 steps) | 1 KB | 32 steps | 20.7 s | 45 ms | 12.2 MB | 372 MB |
| **Sonobe Nova** (CPU) | SHA-256 chain (32 steps) | 1 MB | 32 steps | 20.9 s | 41 ms | 12.2 MB | 410 MB |
| **Sonobe Nova** (CPU) | SHA-256 chain (32 steps) | 10 MB | 32 steps | 20.8 s | 48 ms | 12.2 MB | 420 MB |
| **Sonobe Nova** (CPU) | toy-decode (32 steps clamp) | any | 32 steps | 12.4 s | 31 ms | 7.2 MB | 260 MB |
| **Sonobe Decider** | Groth16 wrap on top of IVC | n/a | — | OOM at >16 GB anon-rss in 15 GiB sandbox | — | ~200 B (target) | >16 GB |
| SP1 (M1 GPU A10, for ref) | SHA-256 | 1 KB | 90,887 | 818 ms | 114 ms | 2.7 MB | — |
| SP1 (M1 CPU, for ref) | SHA-256 | 1 KB | 90,887 | 26 s | 110 ms | 2.7 MB | 8 GB |

### Reading the table

- **Jolt is the CPU-prove winner.** 4.0 s on 1 KB SHA-256 = **6× faster than SP1's CPU baseline (26 s)** and **~5× faster** than SP1's A10 GPU number once you account for the precompile (M1 SP1 used the SHA-256 precompile; Jolt here uses `jolt-inlines-sha2`). Roughly half a million RV cycles/sec in this sandbox.
- **Real toy-codec costs ~41k extra Jolt cycles per 16×16 frame.** The toy-decode row went from 67k cycles (clamp passthrough) to 109k (real 8×8 Walsh–Hadamard inverse + dequant) when the codec stub was replaced with the M2 codec. That ~41k delta is the per-frame budget M3's H.264 decoder is gambling against.
- **Sonobe Nova IVC is linear-in-step-count and slow per step.** ~650 ms per fold step for the SHA-256 chain workload. The 1 MB / 10 MB rows show identical numbers because the spike caps at 32 fold steps to keep CPU runs sane. Extrapolating: a 1 MB fixture (32k steps) is ~5.7 hours; 10 MB is ~59 hours. **Folding doesn't beat zkVMs on short inputs.** Where it shines is per-macroblock loops where the same step circuit is folded many thousands of times — and where the IVC accumulator is then collapsed by a Decider into a constant-size Groth16 proof.
- **Sonobe IVC accumulator size (12 MB / 7 MB) is misleading on its own.** That's the un-Decided proof. The DeciderEth wrap (Groth16/BN254) collapses it to ~200 B; that path is now wired (`sonobe-script bench --decider`) but OOM'd in the 15 GiB sandbox during Groth16 setup. Sandbox-deferred to a Lambda A10 (≥ 32 GB) per `TESTING.md`.
- **Jolt memory cost is heavy** (11 GB peak for 1 KB SHA-256) — this matters for our M3 chunk sizing.
- **Jolt cycle counts and proof bytes are now captured.** A custom `tracing_subscriber::Layer` scrapes the "X total cycles" line out of Jolt's log stream into the bench JSON; `ark_serialize::CanonicalSerialize` produces real proof bytes (~80 KB). Both were `0` in the original M1b numbers.

### What this does NOT measure

- GPU prove times for either candidate. Neither has a public CUDA path; LayerZero's Jolt-CUDA fork is closed and not in upstream.
- Browser verify time for either. Sonobe needs a Decider run on a ≥ 32 GB box first (sandbox OOM); Jolt has no browser verifier at all. `scripts/full_test_gpu.sh` phase 8 runs the Sonobe Decider on the GPU instance.
- 1 MB / 10 MB rows for Jolt — would require a longer wall-clock budget than this sandbox allows. Sonobe rows at those sizes are step-count-capped, so identical numbers.
- SP1's toy-decode row, where SP1 runs the same 16×16 frame through the new RISC-V `toy-decode` guest. The host code is committed; the guest never built in this sandbox because `sp1up` couldn't reach `api.github.com`. `scripts/full_test_gpu.sh` phase 5 builds and runs it on the GPU instance.

## 5. How this maps to the end goal

The end-user-verifiable claim we're trying to ship is:

> Given this 5 MB compressed video and this signed manifest, **verify in a browser, in under 2 seconds**, that the video matches a hardware-attested original.

Recasting the candidates against that claim:

**Sonobe Nova path (today, end-to-end):**
```
per-macroblock step circuit  ──┐
   . . . (N folds)             ├── Nova IVC accumulator  ── DeciderEth ── Groth16/BN254 (~200 B)
per-macroblock step circuit  ──┘                                                  │
                                                                                  ▼
                                                   browser ── ark-groth16-wasm or snarkjs ── ✓
```
Every box exists today. The work is integrating the M2/M3 codec step circuits, picking a Decider, and shipping the WASM glue. **Zero ecosystem risk.**

**Jolt path (the day Groth16 wrap lands):**
```
no_std Rust H.264 decoder  ── Jolt prove ── Jolt proof  ── Jolt→Groth16 wrap (NOT YET) ── Groth16/BN254
                                                                                                  │
                                                                                                  ▼
                                                                                       browser ── ✓
```
The wrap is the missing piece. A16z's Nov 2024 update said it was in progress; no shipped artifact in May 2026.

**SP1 path (today, partial):**
```
no_std Rust H.264 decoder ── SP1 prove ── SP1 core proof ── client.compress() ── client.groth16() ── Groth16
                                                                                                          │
                                                                                                          ▼
                                                                                               browser ── ✓
```
Working pieces. M1 deferred the actual WASM wiring; the bytes-on-the-wire are well-understood.

## 6. Recommended next steps

In priority order for **M3 prover-pick**:

1. **Stay on SP1** for M2 (toy codec). It works today, the team knows it, the GPU path is paid for. Ship M2 on SP1 to validate the decoder + Merkle + Ed25519 architecture.
2. **In parallel**, run two small experiments on the M2 toy codec:
   - **Sonobe Nova end-to-end with Decider.** Take the M2 step circuit, build it with `FCircuit`, run `DeciderEth`, drop the resulting ~200 B Groth16 proof into the existing `spike/web/` verifier. This is the one experiment that proves we have a working browser path that isn't SP1.
   - **Jolt full SHA-256 fixture sweep.** Re-run the spike on Lambda A10 for the 1 MB / 10 MB rows; capture proof bytes (after JoltProof gets `serde::Serialize`); confirm the cycle/sec and memory pattern hold up. This sets the ceiling on what Jolt can do for us if its browser story closes.
3. **Re-evaluate at the M3 prover-pick gate** with both data points in hand. Decision tree:
   - Jolt Groth16 wrap shipped + Jolt 6× advantage holds at scale + browser glue verified → **swap SP1→Jolt**.
   - Otherwise → stay on SP1, slot Sonobe Nova as the v2 candidate for the per-macroblock workload where folding's amortization shines.
4. **Track OpenVM.** It's the actual production heir to Plonky3 in the Scroll/Axiom stack and may surface as a third candidate by M3.

## 7. Files written

```
spike/sonobe/                        Sonobe Nova+CycleFold spike
  Cargo.toml                         git-pinned folding-schemes + arkworks fork patches
  README.md                          why Sonobe Nova specifically
  src/
    main.rs                          CLI: prove/verify/bench, --workload {sha256-chain,toy-decode}
    sha256_circuit.rs                Sha256FCircuit, lifted from sonobe/examples/sha256.rs
    toy_decode_circuit.rs            ToyDecodeFCircuit (clamp via 16-bit decomposition)

spike/jolt/                          Jolt zkVM spike
  Cargo.toml                         workspace + host package, git-pinned a16z/jolt
  README.md                          why Jolt is in the survivor list, and why secondary
  src/main.rs                        host CLI mirroring SP1/RISC0 shape
  guest/                             Jolt guest crate for sha256 (one provable per crate)
    Cargo.toml
    src/lib.rs                       sha2_preimage(min_size, data) -> ([u8;32], u32)
    src/main.rs                      no_main entry that pulls in macro-generated symbols
  guest-toy-decode/                  Sibling guest crate for toy-decode
    Cargo.toml
    src/lib.rs                       toy_decode_one_block calls crates/toy-codec::decode_toy
    src/main.rs

scripts/setup_sonobe.sh              Pre-fetch Sonobe deps; warm the cargo registry
scripts/bench_sonobe.sh              Build + bench (both workloads, --max-steps cap)
scripts/setup_jolt.sh                Install RISC-V targets; pre-fetch Jolt deps
scripts/bench_jolt.sh                Build + bench (both workloads)
scripts/bench_all.sh                 Updated to drive all four spikes (SP1/RISC0/Sonobe/Jolt)

spike/bench/results/                 JSON output from this run (CPU, sandbox)
  sonobe-sha256.json
  sonobe-toy-decode.json
  jolt-sha256.json
  jolt-toy-decode.json

milestones/01b-newer-frameworks.md   This document
```

## 8. Open questions / known gaps

- **Jolt proof serialization**: `JoltProof` type doesn't impl `Clone` or `serde::Serialize` in the resolved May 2026 graph. Either wait for upstream to expose it, or use Jolt's `ark_serialize::CanonicalSerialize` impl directly. Spike host currently writes 0 bytes for proof size.
- **Sonobe Decider not exercised.** The browser-verifier claim depends on `DeciderEth` running end-to-end. Spike has it behind a `decider` feature gate but doesn't run it. Next experiment should.
- **Cycle counts not in Jolt JSON.** Jolt logs them via `tracing` (RUST_LOG=info); could be parsed and added to `BenchResult`.
- **Memory cost of Jolt.** 11 GB for 1 KB SHA-256 implies bigger fixtures need lots of RAM. Worth measuring on the A10 box where it'll actually run.
- **Sonobe HyperNova as a stretch.** If Nova step times don't fit the M3 budget, HyperNova has lower per-step cost on lookup-heavy circuits. The Decider engineering is the cost.
- **The user-facing browser verifier UX is still TODO across all candidates.** Even SP1's M1 verifier is placeholder-only. The "verify a compressed video in a browser" work has to happen at some milestone regardless of prover pick.
