# Milestone 1 — zkVM Spike

**Duration target:** 1–2 weeks.
**Outcome:** a justified pick between RISC Zero and SP1, plus a working end-to-end "hash a file, prove it, verify in the browser" demo.

This milestone is the **go/no-go gate** for the whole project. If proving a 1 MB SHA-256 takes hours on a single GPU, the in-circuit decoder is not feasible at the planned scope and we re-scope before starting milestone 2.

## 1. The statement we prove

We pick a statement that exercises the same shape as the real workload (a private blob committed to by a public hash) but with no codec content, so the spike is purely a measurement of the proof system.

> **Public input:** `commitment: [u8; 32]`, `min_size: u32`
> **Private input (witness):** `data: Vec<u8>`
> **Claim:** `SHA-256(data) == commitment ∧ data.len() ≥ min_size`

Why this shape:

- A private witness that hashes to a public commitment is exactly the structure of "Merkle leaf authenticated against the signed root" we need in milestone 3.
- SHA-256 is a representative heavy circuit primitive (and a useful pessimistic estimate; the real system will prefer Poseidon/Blake3 for in-circuit hashes).
- The size lower bound forces the prover to actually feed the bytes through the circuit; trivial constant-witness optimizations don't apply.

We run the spike at three sizes: **1 KB, 1 MB, 10 MB**. Each becomes one row in the comparison table.

## 2. Acceptance criteria

The spike is "done" when:

1. Both RISC Zero and SP1 implementations exist and produce valid proofs for the three input sizes.
2. A browser page loads each verifier as WASM and verifies the produced proof in under 2 s on a mid-range laptop.
3. `bench/run.sh` produces a single `comparison.json` capturing the metrics in §4 for both systems.
4. A short `DECISION.md` records which system we picked and why, with the numbers backing it.

We pick the system that wins on **proof time at 10 MB on a single GPU**, with proof size and browser verifier viability as tiebreakers.

## 3. Repository layout

```
snarkvid/
  spike/
    risc0/
      Cargo.toml
      methods/
        guest/
          src/main.rs       # the guest program
          Cargo.toml
        Cargo.toml
      host/
        src/main.rs         # CLI: `prove --input <file> --out <proof>`
        Cargo.toml
    sp1/
      Cargo.toml
      program/
        src/main.rs         # the guest program
        Cargo.toml
      script/
        src/main.rs         # CLI: same interface as risc0/host
        Cargo.toml
    common/
      bench-fixtures/
        gen.sh              # produces 1KB / 1MB / 10MB random files
    web/
      package.json
      vite.config.ts
      src/
        verify.ts           # loads WASM verifier, verifies proof
        ui.tsx              # drop-zone for proof + commitment
      public/
        risc0-verifier.wasm
        sp1-verifier.wasm
    bench/
      run.sh                # runs both provers on all sizes, emits JSON
      compare.py            # renders comparison.md from comparison.json
```

The two implementations are deliberately siblings, not abstracted behind a common trait. The point of the spike is to feel the differences, not paper over them.

## 4. What we measure

For each (system, input_size) pair:

| Metric | How |
|---|---|
| **Cycle count / opcode count** | Native counter in each zkVM. |
| **Prover wall-clock (CPU, 16-thread)** | `time` around the prove call. |
| **Prover wall-clock (single GPU)** | Same, with GPU feature flags enabled. |
| **Peak prover RSS** | `/usr/bin/time -v` or equivalent. |
| **Proof size on disk** | `stat`. |
| **Verifier wall-clock (native, Rust)** | Around the verify call. |
| **Verifier wall-clock (browser, WASM)** | `performance.now()` around `verify()`. |
| **Verifier WASM bundle size (gzip)** | After `wasm-opt -Oz` + gzip. |

Each row also records the toolchain version (`cargo zk --version`, etc.) so the numbers are reproducible.

GPU target for the spike: a single L4 or T4 (consumer-priced cloud GPU). If neither system can prove 10 MB SHA-256 in under ~10 minutes on one of these, that's the project's first redesign moment.

## 5. Browser verifier sketch

```ts
// spike/web/src/verify.ts
import init, { verify } from "./pkg/verifier";

export async function verifyProof(
  system: "risc0" | "sp1",
  proof: Uint8Array,
  publicInputs: { commitment: Uint8Array; minSize: number },
): Promise<{ ok: boolean; ms: number }> {
  await init(`/${system}-verifier.wasm`);
  const t0 = performance.now();
  const ok = verify(proof, publicInputs.commitment, publicInputs.minSize);
  return { ok, ms: performance.now() - t0 };
}
```

The UI is intentionally bare — drop a `.proof` file, paste the commitment hex, pick the system, see green/red and ms-to-verify. We're testing whether the verifier *fits* in the browser, not designing the production UX.

## 6. Decision matrix template

`bench/compare.py` produces `DECISION.md` with this structure:

```
| Metric                    | RISC Zero | SP1   | Winner |
|---------------------------|-----------|-------|--------|
| Prove 10MB GPU (s)        |    ?      |   ?   |   ?    |
| Prove 1MB GPU (s)         |    ?      |   ?   |   ?    |
| Proof size (KB)           |    ?      |   ?   |   ?    |
| Verify native (ms)        |    ?      |   ?   |   ?    |
| Verify browser (ms)       |    ?      |   ?   |   ?    |
| Verifier bundle gz (KB)   |    ?      |   ?   |   ?    |
| Recursion supported       |   y/n     |  y/n  |        |
| GPU tooling maturity (1-5)|    ?      |   ?   |        |
| Docs/examples (1-5)       |    ?      |   ?   |        |

Decision: <picked>
Rationale: <2–3 sentences>
Risks accepted: <2–3 bullets>
```

## 7. Out of scope for this milestone

- Real video data — handled in milestone 2.
- Merkle trees — handled in milestone 2.
- Signatures — handled in milestone 2.
- Recursion / proof aggregation — surveyed in §6, not exercised.
- Production verifier UX — milestone 5.

## 8. Risks and mitigations

| Risk | Mitigation |
|---|---|
| 10 MB SHA-256 doesn't prove in reasonable time on either system | Pivot to Blake3 / Poseidon as the in-circuit hash; revisit fixture sizes against realistic per-frame chunk size (~1.5 MB raw 720p frame). |
| Browser verifier WASM is too large or slow | Try the alternative system; if both fail, accept native CLI verifier for v1 and revisit browser later. |
| GPU prover requires CUDA features that don't build cleanly | Run CPU-only spike first; record cycle counts, project GPU times from public benchmarks. |
| RISC Zero / SP1 release a major breaking change mid-spike | Pin toolchain versions in `rust-toolchain.toml` from day one. |

## 9. Concrete first steps (day 1–2)

1. `cargo new --workspace` the `spike/` tree above; pin Rust toolchain.
2. `spike/common/bench-fixtures/gen.sh` — emit deterministic 1 KB / 1 MB / 10 MB files via `head -c N /dev/urandom > fixture-N.bin && sha256sum`.
3. RISC Zero side: copy the `sha2` example from the RISC Zero examples repo, retrofit to take the witness from stdin and emit the proof + receipt to disk.
4. SP1 side: same, against the SP1 sha2 example.
5. Run on CPU first, get a baseline. Wire `bench/run.sh` to call both and dump JSON.
6. Then wire GPU feature flags, re-run.
7. Then the browser harness — minimal Vite app, two WASM verifiers loaded on demand.
