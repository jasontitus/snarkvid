# spike — Milestone 1: zkVM Selection

This tree implements the milestone-1 spike defined in `../milestones/01-spike.md`.

The goal is to pick between **RISC Zero** and **SP1** by running the same workload — proving knowledge of a `data: Vec<u8>` whose `SHA-256` matches a public commitment and whose length meets a public minimum — through both, then comparing prove time, proof size, and browser verifier viability.

## Layout

```
spike/
  risc0/       RISC Zero implementation (host + methods + guest)
  sp1/         SP1 implementation (script + program)
  common/      shared fixtures (1 KB / 1 MB / 10 MB random files)
  web/         Vite + TS browser verifier loading both WASM verifiers
  bench/       benchmark harness; produces comparison.json + DECISION.md
```

Each side is self-contained and intentionally not abstracted behind a shared trait — the spike is about feeling the differences between the two systems, not papering over them.

## Day-1 plan

See `../milestones/01-spike.md` §9.

## Status

Scaffold only — no proving logic yet. Stubs reference upstream examples that should be ported in:

- RISC Zero `examples/sha`: https://github.com/risc0/risc0
- SP1 `examples/sha2`: https://github.com/succinctlabs/sp1

Pin exact toolchain versions in `rust-toolchain.toml` on day 1 of the spike.
