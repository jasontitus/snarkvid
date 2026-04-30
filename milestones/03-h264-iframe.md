# Milestone 3 — Baseline H.264 I-Frame Decoder In-Circuit

**Duration target:** 2–3 months.
**Prerequisite:** Milestone 2 complete; manifest, Merkle tree, comparator, and browser verifier all working with the toy codec.
**Outcome:** the toy codec is replaced by a real (heavily-scoped) H.264 decoder running inside the proof. The system can verify a still I-frame from a real `.h264` bitstream.

This is the milestone where the project actually demonstrates the core thesis. Once an I-frame works, milestone 4 adds P-frames, audio, and aggregation.

## 1. The statement we prove

Identical to milestone 2, with `decode_toy` swapped for the real decoder.

> **Public input:** `bitstream: Vec<u8>` (one H.264 NAL stream containing a single IDR I-frame), `manifest: SignedManifest`, `tolerance: PsnrDb`
> **Private input (witness):** `original_yuv: YuvFrame`, `merkle_paths: Vec<Path>`
> **Claim:**
> 1. `Sig.Verify(manifest.pubkey, manifest.body) == true`.
> 2. Each block of `original_yuv` authenticates against `manifest.body.video.merkle_root`.
> 3. `decode_h264_baseline_iframe(bitstream) == reconstructed`.
> 4. `psnr(reconstructed, original_yuv) ≥ tolerance`.

## 2. Scope — exactly what subset of H.264

**In:**

- Baseline profile (`profile_idc = 66`).
- Single IDR slice per NAL stream.
- Single slice per frame.
- `chroma_format_idc = 1` (YUV 4:2:0), 8-bit.
- Intra prediction modes:
  - `Intra_4x4` (all 9 modes).
  - `Intra_16x16` (all 4 modes).
  - `I_PCM` (escape hatch; rarely emitted but spec requires support).
- Chroma `Intra_8x8` prediction (all 4 modes).
- Residual: 4×4 integer transform + quantization, including the DC Hadamard pass for `Intra_16x16` luma DC and chroma DC.
- CAVLC entropy decoder (with all five lookup tables — `coeff_token`, `total_zeros`, `run_before`, level prefix, level suffix).
- Bitstream parsing: NAL unit framing with emulation-prevention byte removal, `ue(v)` / `se(v)` Exp-Golomb codes, slice header, MB layer.

**Out (deferred to milestone 4):**

- P-frames (motion compensation, motion-vector prediction, reference picture lists).
- Deblocking filter — accept lower PSNR floor instead.
- Audio (AAC-LC).
- Multi-frame aggregation.

**Out (permanently for v1):**

- B-frames, CABAC, multiple slices, FMO/ASO, 10-bit, 4:2:2 / 4:4:4, High profile features, redundant pictures.

A `--profile baseline --bf 0 --refs 1 --weightb 0 --8x8dct 0 --no-deblock` invocation of `x264` produces bitstreams the milestone-3 decoder accepts.

## 3. In-circuit decoder pipeline

Per-frame, top to bottom:

```
bitstream ──▶ NAL framer + ep-byte strip ──▶ slice-header parser
                                                       │
                                                       ▼
                                          for each macroblock in raster order:
                                            ┌─────────────────────────────┐
                                            │ mb_type + intra-mode flags  │
                                            │  via Exp-Golomb / CAVLC     │
                                            └────────────┬────────────────┘
                                                         │
                                            ┌────────────▼───────────────┐
                                            │ residual coeffs (CAVLC)    │
                                            └────────────┬───────────────┘
                                                         │
                                            ┌────────────▼───────────────┐
                                            │ inverse quant + 4×4 IDCT   │
                                            │ + DC Hadamard if needed    │
                                            └────────────┬───────────────┘
                                                         │
                                            ┌────────────▼───────────────┐
                                            │ intra prediction from      │
                                            │ already-decoded neighbors  │
                                            └────────────┬───────────────┘
                                                         │
                                            ┌────────────▼───────────────┐
                                            │ add residual + clamp       │
                                            └────────────┬───────────────┘
                                                         ▼
                                                 reconstructed MB

after all MBs: PSNR(reconstructed_frame, original_yuv) ≥ tolerance
```

All of this is straight-line, deterministic Rust. No dynamic allocation in the guest; all buffers are sized from the parsed `pic_width / pic_height`. Pre-allocate once per frame.

## 4. New crates

```
snarkvid/
  crates/
    h264-decoder/         # the decoder; no_std; the heart of milestone 3
      src/
        nal.rs            #  NAL unit framing + emulation-prevention strip
        bitreader.rs      #  Exp-Golomb + raw bits + CAVLC primitive
        cavlc.rs          #  the five CAVLC tables + coeff parsing
        slice.rs          #  slice header
        mb.rs             #  per-MB decode: parse → predict → reconstruct
        intra.rs          #  intra_4x4 / intra_16x16 / chroma 8x8 modes
        transform.rs      #  4x4 integer IDCT + DC Hadamard
        quant.rs          #  inverse quantization tables
        frame.rs          #  ties it together
    h264-test-vectors/    # JM-reference outputs for regression tests
```

`h264-decoder` is the only new in-circuit crate. `manifest` and `comparator` from milestone 2 are reused unchanged. The prover guest program for milestone 3 looks almost identical to milestone 2's, just with `toy_codec::decode` replaced by `h264_decoder::decode_iframe`.

## 5. Acceptance criteria

1. **Native parity.** `h264-decoder` decodes a corpus of test bitstreams (generated by `x264` with the milestone-3 flag set) and matches the JM reference decoder bit-exactly on Y, U, V planes for every frame.
2. **In-circuit correctness.** The same decoder, run inside the chosen zkVM, produces a valid proof for at least one 720p I-frame.
3. **Tampering tests fail closed.** Same suite as milestone 2 §3.4, plus:
   - Flip a single bit inside a CAVLC `coeff_token` → proof fails.
   - Re-encode the same content with a different QP → proof fails (PSNR check) unless re-Merkled and re-signed.
4. **Bench numbers recorded** at 480p, 720p, 1080p single I-frames on a single GPU. PSNR floor logged for each.
5. **Decision documented.** A short `MILESTONE_3_RESULTS.md` records: cycles per frame split by stage (NAL parse / CAVLC / inverse transform / intra predict / Merkle), prove time, proof size, browser verify time, and a go/no-go for milestone 4.

## 6. What we measure

Per resolution on a single GPU:

| Metric | Why it matters |
|---|---|
| Cycles per frame (and per stage) | Identifies the hot stage to optimize first; sets the budget for adding P-frames in milestone 4. |
| Prove wall-clock per frame | Determines whether monolithic per-frame proving is viable or we need per-row aggregation. |
| Witness bytes per frame | Calibrates I/O cost for streaming originals into the guest. |
| Proof bytes | Should match milestone 1 baseline; flag regressions from any new circuit primitives. |
| PSNR floor that still verifies | The minimum tolerance the decoder achieves without deblocking; sets user-facing default. |

## 7. Aggregation strategy

For a still-image milestone, no aggregation. But milestone 3 must answer the question for milestone 4: **does a single 1080p I-frame fit in one proof, or do we need per-row recursion?**

If a single 1080p I-frame proof exceeds the chosen prove-time budget (~5 min on a single GPU is the working ceiling), introduce per-MB-row proofs:

- Each row proves: "given the bottom-edge pixels of the row above (public input) and the bitstream slice for this row, the reconstructed pixels of this row are X, and their bottom-edge pixels are Y (public output)."
- The first row's "row above" is the constant border defined by the spec.
- Recursive aggregation chains 45 row proofs (for 720p, 16-pixel rows) into one final proof per frame.
- Cost: more total cycles than monolithic, but each individual proof is bounded — fits on a single GPU with predictable memory.

We don't build this until we have monolithic numbers in hand. Milestone 3's go/no-go decides whether milestone 4 starts with row aggregation or assumes monolithic frames.

## 8. Risks

| Risk | Mitigation |
|---|---|
| CAVLC parsing dominates cycles (variable-length codes are awkward in zkVMs) | Prebuild all five CAVLC tables as static arrays; parse with branchless table-driven code. If still slow, try a hybrid where the host pre-parses CAVLC and the guest verifies the parse. |
| 1080p frame doesn't fit in one proof | Per-MB-row aggregation, as in §7. |
| Witness I/O for original YUV (3 MB at 1080p) saturates the zkVM's input channel | Stream originals as Merkle leaves and only authenticate the leaves the comparator actually inspects (per-block PSNR, not per-pixel). |
| JM reference parity is harder than expected | Start with a constrained test set (single-MB frames, then single-row frames); add full frames only after the easy cases pass. |
| Encoder configurations drift outside the supported subset | Ship a `validate-bitstream` CLI that rejects bitstreams using out-of-scope features before they reach the prover. |
| Spec compliance bugs in the in-circuit decoder | Differential-fuzz against the JM reference: random valid bitstreams in, both decoders produce identical YUV out. CI gates the decoder crate. |

## 9. First steps

1. **Generate the test corpus** (week 1). Use `x264` with the milestone-3 flag set on a handful of source images at three resolutions; archive the resulting `.h264` bitstreams + JM-reference YUV outputs as test vectors.
2. **`crates/h264-decoder` native** (weeks 1–4). Build the decoder bottom-up: bitreader → NAL framer → CAVLC → transform/quant → intra prediction → frame loop. Each module CI-tested against the corpus.
3. **Differential fuzz** (week 5). `cargo fuzz` against the JM reference decoder; gate the crate on zero divergences over an overnight run.
4. **In-circuit single-MB proof** (week 6). Strip the test corpus to single-MB bitstreams; run the decoder in the chosen zkVM; prove + verify.
5. **Single-row, then single-frame** (weeks 7–9). Scale up. Bench at each scale.
6. **Tampering tests + browser verifier hookup** (week 10).
7. **Bench at 480p / 720p / 1080p, write `MILESTONE_3_RESULTS.md`** (weeks 11–12).

The week-9 single-frame number is the **next go/no-go gate** after milestone 1's: if 720p doesn't prove in a reasonable budget, milestone 4 starts with per-row aggregation; if even per-row doesn't fit, the project re-scopes (smaller resolution ceiling, or different proof system).
