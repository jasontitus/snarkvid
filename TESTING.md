# Testing inventory

This document tracks **what's been tested where**, and **what each
machine can/can't run**, so you can spin up a GPU instance, run one
script, and shut it down again.

## TL;DR — running the full test battery on a GPU box

```bash
git clone https://github.com/jasontitus/snarkvid.git
cd snarkvid
./scripts/full_test_gpu.sh   # ~90–150 min on a Lambda A10
```

The script prints a `PASS/FAIL` line per phase and a final summary; exit
0 means safe to terminate the instance.

## What's currently tested locally (CPU sandbox, 15 GiB RAM)

These are the things that have actually run green in development and
whose numbers are already in this repo.

| Component | Test | Status | Evidence |
|---|---|---|---|
| `crates/toy-codec` | 10 unit tests: WHT self-inverse, qp=0 lossless on flat/ramp/noise frames, qp=8 ≥ 40 dB PSNR on noise, qp monotonicity, dim/qp validation | PASS | `cargo test -p snarkvid-toy-codec` |
| `crates/comparator` | 2 unit tests: identical→∞ PSNR, off-by-one→~48 dB | PASS | `cargo test -p snarkvid-comparator` |
| `crates/manifest` | 11 unit tests: real Ed25519 sign/verify round-trip, all four M2 §3.4 tampering modes (body bit-flip, sig bit-flip, swapped pubkey, invalid pubkey bytes), Merkle root + path generator round-trip on power-of-two and odd-length leaves | PASS | `cargo test -p snarkvid-manifest` |
| **`crates/m2-statement`** | 4 unit tests for the canonical `verify_m2_claim` function (the one the SP1 guest will call in-circuit): happy path, manifest sig failure → typed error, Merkle path failure carries the failing block index, PSNR below tolerance returns the actual measured PSNR. Crate is no_std-pure (`cargo build --no-default-features` clean). | PASS | `cargo test -p snarkvid-m2-statement` |
| `bin/toy-encode` | Compiles + smoke-encodes a YUV file | PASS | builds clean as part of workspace |
| **M2 native pipeline** (`bin/toy-encode/tests/m2_pipeline.rs`) | 7 integration tests composing the full fixture builder around `verify_m2_claim`: qp=0 happy-path at 60 dB, qp=8 happy-path at 36 dB, the three §3.4 tampering modes that don't need a prover (compressed bit-flip; manifest signed by unknown key; tolerance below actual PSNR), corrupted Merkle path, path-count mismatch | PASS | `cargo test -p toy-encode --test m2_pipeline` |
| **`prover/host` smoke** | Runs `prover-host smoke --input frame.yuv --width 32 --height 32 --qp 8 --tolerance 36.0` end-to-end: reads YUV, builds Merkle tree, signs manifest, encodes, calls `verify_m2_claim`. Confirmed PASS at qp=8/36 dB on a noise frame (PSNR Y=57 dB, combined=58 dB). Fail-closed confirmed at qp=32/60 dB (PSNR=46.65 dB rejected). | PASS | `cargo build --manifest-path prover/Cargo.toml --release && prover/target/release/prover-host smoke ...` |
| **`crates/h264-decoder`** | 120 unit tests across nine modules. **`decode_iframe(bitstream) -> DecodedFrame` produces real reconstructed pixels** end-to-end on the live x264 corpus: NAL walk → SPS/PPS/slice → MB header → residual decode → Intra_4×4 prediction (DC mode, first cut) → inverse quant + IDCT + round_shift_6 → add residual + clamp → write back to plane. **Quantitative parity gate**: Y plane SAD vs ffmpeg reference = **16,985** (0 = bit-exact; max-noise = 65,280). Closing this gap requires resolving real Intra_4×4 modes against cross-MB neighbor state (~50% of corpus blocks use non-DC modes per x264's per-block log), filling the rest of the CAVLC tables, and wiring chroma reconstruction. The pipeline shape is final. Crate is no_std-pure. | PASS | `cargo test -p snarkvid-h264-decoder` |
| **`crates/h264-test-vectors`** | M3 §9.1 corpus loader. One fixture committed: `noise-16x16-qp18` (1018 H.264 bytes encoded by `x264 --profile baseline --bframes 0 --no-cabac --no-deblock --no-8x8dct --frames 1 --keyint 1 --qp 18`, 384 YUV bytes decoded by ffmpeg as the reference output). Regen via `scripts/gen_h264_corpus.sh` (needs apt: x264, ffmpeg). | PASS (corpus committed; regen script tested) | — |
| Jolt SHA-256 1KB | CPU prove + verify round-trip, cycles + proof bytes captured | PASS — 53,032 cycles / 80,281 B / 4,031 ms prove / 120 ms verify | `spike/bench/results/jolt-sha256.json` |
| Jolt toy-decode 16×16 | CPU prove + verify with real WHT decoder in the loop | PASS — 108,816 cycles / 83,817 B / 5,962 ms prove / 132 ms verify | `spike/bench/results/jolt-toy-decode.json` |
| Sonobe Nova SHA-256 chain | CPU IVC bench, several fold counts | PASS (numbers in `01b-newer-frameworks.md` §4) | `spike/bench/results/sonobe-sha256.json` |
| Sonobe Nova toy-decode | CPU IVC bench (per-coefficient clamp) | PASS | `spike/bench/results/sonobe-toy-decode.json` |
| All four spike crates | `cargo build --release` workspace + per-spike | PASS | `cargo build --workspace --release` |

## What's deferred to a GPU box

These either need GPU wall-clock, more RAM than the sandbox has, or a
network path to GitHub releases that the sandbox doesn't have. Each is
covered by `scripts/full_test_gpu.sh`.

| Phase | Component | Why deferred | Where it runs |
|---|---|---|---|
| 4 | SP1 SHA-256 1KB / 1MB / 10MB on CUDA | M1's go/no-go numbers; need an actual GPU | `bench_cuda.sh` |
| 4 | RISC Zero SHA-256 sweep on CUDA | Head-to-head against SP1 | `bench_cuda.sh` |
| **5** | **SP1 toy-decode (3-way parity row)** | **`sp1up` couldn't reach `api.github.com` from the sandbox; the RISC-V `toy-decode` guest never built. Code is committed and ready.** | first build on the GPU box cross-compiles the new guest |
| 6 | Jolt SHA-256 1MB / 10MB | CPU-only; sandbox can't budget the wall-clock for the bigger fixtures | `bench_jolt.sh` |
| 7 | Sonobe Nova at higher step counts (`--max-steps 1024`) | Each fold step is ~650 ms; >1k steps wasn't worth burning sandbox minutes on | `bench_sonobe.sh` |
| **8** | **Sonobe Decider (Groth16 wrap → ~200 B proof)** | **OOM'd in the 15 GiB sandbox at anon-rss = 16 GB during Groth16 setup over the Nova augmented circuit. Code is committed and ready (`sonobe-script bench --decider`).** | needs ≥ 32 GB RAM (Lambda A10 default is fine) |
| **10** | **M2 prover (`prover-host prove`)** | **The full M2 statement (manifest + Merkle + decode_toy + PSNR) inside an SP1 guest. Requires `--features build-guest` which triggers `sp1_build` to cross-compile the RISC-V guest — needs sp1up + +succinct, GitHub-blocked from sandbox.** Smoke subcommand runs natively on CPU and was confirmed green here. | first build on the GPU box cross-compiles the new `prover/guest/` |

### M3 work that lands on the GPU box later

These items don't have a `full_test_gpu.sh` phase yet — each lands when its
corresponding code lands. Tracking here so the GPU script gets updated in
lockstep.

| Future phase | Component | Trigger |
|---|---|---|
| 11 | M3 differential-fuzz (`cargo fuzz` against ffmpeg-libav) | After `crates/h264-decoder::frame::decode_iframe` is wired (currently `bitreader`, `nal`, `cavlc` primitives, and `transform` only). M3 §3 names this as the canonical regression gate. |
| 12 | M3 single-MB SP1 prove (smallest viable in-circuit H.264) | After `frame::decode_iframe` works on the noise-16x16-qp18 corpus fixture (single 16×16 macroblock — exactly one MB). M3 §9.4. |
| 13 | M3 single-row SP1 prove | Single MB row at 720p (45 MBs) — lets us measure whether per-MB-row aggregation (M3 §7) is the path forward. M3 §9.5. |
| 14 | M3 single-frame SP1 prove at 480p / 720p / 1080p | The week-9 go/no-go gate per M3 §9.7. If 720p doesn't fit a single proof on a single GPU under ~5 min, M4 starts with per-MB-row aggregation instead of monolithic frames. |
| 15 | M3 tampering tests (CAVLC bit-flip; QP-mismatch re-encode) | After the M3 prover works end-to-end. M3 §5.3. |
| TBD | M3 corpus expansion (`scripts/gen_h264_corpus.sh`) | Add 64×64, 240p, 480p, 720p, 1080p I-frames at QPs 18 / 28 / 38. The 16×16 corpus exercises framing and one MB; bigger fixtures exercise the row loop. Re-run the script on the GPU box (it's CPU-bound, fast). |
| TBD | M3 corpus integration test: decode noise-16×16-qp18 corpus end-to-end and compare to ffmpeg-decoded reference YUV | `frame::decode_iframe` already runs end-to-end on the corpus and returns a YUV frame; pixels are placeholder until residual decode is wired. Once the missing CAVLC tables are filled in and residual reconstruction lands inside `mb.rs`, diff the output against ffmpeg's reference YUV (already in the corpus alongside the .h264). Bit-exact Y/U/V is M3 §3.1 acceptance ("Native parity"). Runs on CPU but is a meaningful gate before the GPU prove run. |
| TBD | M3 prover SP1 prove of in-circuit `decode_h264_iframe` | Single-MB then single-row then single-frame proves on the A10 (M3 §9.4–§9.6). Cycle accounting per stage so we know which module to optimize first. |
| TBD | M3 §3.3 tampering tests (CAVLC bit-flip; QP-mismatch re-encode) | After the M3 prover works end-to-end. Validates the decoder fails closed under bitstream tampering. |

## What's NOT yet wired up anywhere (known gaps)

These are real holes — the GPU script won't catch them, because there's
nothing to run yet. Tracking here so they don't get lost.

| Gap | Where | Impact |
|---|---|---|
| Browser verifier (Sonobe Decider) | `spike/web/` exists for SP1 but not yet for the Decider's Groth16/BN254 proof | M2 §3.3 acceptance ("verify in a browser, < 2 s") not yet exercised. Blocked by phase 8 producing a Decider proof on disk. |
| Jolt verify-from-proof | Tried; blocked. The `JoltProof<F, C, PCS, FS>` type isn't re-exported by `#[jolt::provable]`. Wiring `CanonicalDeserialize` requires importing `jolt-core`'s field/curve/PCS/transcript types and pinning Jolt's exact instantiation — tighter coupling than the spike warrants. | Doesn't block bench (prove already calls verify in-process). Re-evaluate if Jolt is picked at the M3 prover-pick gate. |
| Sonobe verify-from-proof | Functional but slow — re-runs preprocessing every call instead of caching verifier params | Verify times reported by `cmd_verify` include preprocessing; bench numbers are unaffected (they don't re-preprocess). |
| §3.4.3 tampering test ("substitute different image as witness, prover cannot produce valid proof") | Needs the actual prover; the host's smoke command can't simulate "prover failure" because it's not a prover. | First prove pass on the A10 (phase 10) plus a follow-up tampered-witness prove that asserts non-zero exit / digest mismatch. |
| **M3 H.264 decoder modules still TODO** | Committed: all nine modules (`bitreader`, `nal`, `cavlc`, `transform`, `quant` incl. Hadamard-DC variants, `slice`, `intra` with all 17 modes, `mb` MB-header parser, `frame::decode_iframe`). **Still TODO before pixel-perfect output**: total_zeros tables for TC=3..15 (~250 entries); run_before tables for zeros_left=4..14 (~80 entries); coeff_token VLC0 rows TC=9..16, full VLC1 (nC ∈ [2,4)) and VLC2 (nC ∈ [4,8)) tables (~150 entries); VUI block parser inside SPS (currently skipped — works for our corpus); residual block reads + intra prediction + reconstruction wiring inside `mb.rs` / `frame.rs`; cross-MB neighbor-mode tracking for the Intra_4×4 prediction-mode predictor; ffmpeg-reference parity test on the corpus. The remaining tables are mechanical transcription from spec Tables 9-7 / 9-9 / 9-10. | Differential fuzz against ffmpeg/JM (M3 §3) lands once `decode_iframe` produces pixel-correct output; runs on CPU but takes wall-clock budget that suits a GPU box. |

## Re-running individual phases

`scripts/full_test_gpu.sh` deliberately keeps phases independent. To run
just one:

```bash
# Just the SP1 toy-decode 3-way parity row
spike/sp1/target/release/sp1-script bench \
    --workload toy-decode \
    --fixture-dir spike/common/bench-fixtures \
    --out spike/bench/results/sp1-toy-decode.json

# Just the Sonobe Decider (the load-bearing browser-verifier evidence)
spike/sonobe/target/release/sonobe-script bench \
    --workload sha256-chain \
    --fixture-dir spike/common/bench-fixtures \
    --max-steps 8 --decider \
    --out spike/bench/results/sonobe-sha256-decider.json
```

The result JSONs all share the same schema (`spike/bench/compare.py`
consumes them).

## Hardware notes for the GPU run

- **Lambda Labs A10 24 GB** is the cheapest box that fits everything;
  ~$0.75/hr. One A10 was the M1 reference machine.
- **A100 / H100** cut the 10 MB SP1 prove time roughly proportionally to
  memory bandwidth. Diminishing returns past A100 for the sizes here.
- **Memory ≥ 32 GB** is the bar for phase 8 (Sonobe Decider); A10
  instances on Lambda ship with 64 GB, so this is non-issue there.
