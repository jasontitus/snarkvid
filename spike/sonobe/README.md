# sonobe — folding-schemes side of the M1b spike

Nova+CycleFold IVC via [`privacy-scaling-explorations/sonobe`](https://github.com/privacy-scaling-explorations/sonobe), with a path to a browser-verifiable final proof through the Groth16/BN254 Decider (`DeciderEth`).

## Why Sonobe Nova (not HyperNova, not Microsoft/Nova, not arecibo)

| Candidate | Verdict |
|---|---|
| **Sonobe Nova+CycleFold** | **Picked.** Decider produces Groth16/BN254 (~200 byte proof). Standard Groth16 WASM verifier glue completes the browser-verify-compressed-video story. |
| Sonobe HyperNova | ZK-IVC layer is open issue #128; Decider exists on disk but has no working example. Stretch goal, not the primary scaffold. |
| Microsoft/Nova (`nova-snark 0.71`) | No WASM verifier, no Decider-style Groth16 wrap. We'd be writing all the wrapping ourselves. |
| Lurk's arecibo | Effectively abandoned (Nov 2024, see `arecibo/dev` branch). |

## Workloads

Two workloads, mirroring the SP1/RISC0 milestone-1 fixtures plus the milestone-2 codec kernel:

1. **`sha256-chain`** — Stock Sonobe pattern: `z_{i+1} = SHA256(z_i)`, `state_len=1`. Maps fixture size → number of fold steps (1 step ≈ 32 input bytes ≈ 1 SHA block). Lifted from `sonobe/examples/sha256.rs`.
2. **`toy-decode`** — Per-element clamp step circuit modeling the M2 toy codec's decode kernel. Each fold step takes one quantized coefficient (i16) via `ExternalInputs`, clamps it to `[0,255]`, and updates a running Poseidon hash carried in `state`.

The asymmetry between Jolt (where `decode_toy` drops in as native Rust on the RISC-V guest) and Sonobe (where it must be re-expressed as R1CS gates) is itself one of the report's findings: zkVMs let you prove arbitrary `no_std` Rust; folding schemes require step-circuit authoring. Both end at the same browser-verified proof, but the integration cost differs.

## Build

```
cd spike/sonobe
cargo build --release                # IVC only, no Decider
cargo build --release --features decider   # adds Groth16 wrap path
```

Cold compile: ~6–10 min, ~3 GB target dir (heavy arkworks tree). Stable Rust 1.88 or newer; no nightly required. CPU-only — no CUDA path.

## Run

```
sonobe-script prove   --workload sha256-chain --input <fixture> --min-size <N> --out proof.bin --commit-out commit.hex
sonobe-script verify  --workload sha256-chain --proof proof.bin --commit <hex> --min-size <N>
sonobe-script bench   --workload sha256-chain --fixture-dir ../common/bench-fixtures --out bench.json
sonobe-script bench   --workload toy-decode  --fixture-dir ../common/bench-fixtures --out bench.json
```

Bench output JSON matches the SP1/RISC0 schema in `spike/bench/results/{sp1,risc0}.json` so the same `bench/compare.py` works.

## Browser-verifier path (the load-bearing claim)

```
IVC accumulator
   └── DeciderEth → Groth16/BN254 proof (~200 bytes)
                       └── ark-groth16 verifier compiled to wasm32 (or snarkjs)
                              └── runs in &lt;50ms in any modern browser
```

Sonobe's `solidity-verifiers/` workspace member generates a Solidity verifier from the same key material, so the on-chain story is the same shape.

## Risks (carry into the milestone report)

- **No tagged Sonobe release.** Pin to a SHA before benchmarking; HEAD churns.
- **No CUDA.** All proves are CPU-rayon; A10 GPU is unused. Different baseline than SP1 GPU numbers.
- **HyperNova/ProtoGalaxy Decider not demonstrated.** If folding step performance turns out to favor HyperNova, the additional engineering to wire its Decider is a real cost.
- **Audit scope.** Only Nova+CycleFold is in audit scope. HyperNova/ProtoGalaxy are not.
