# Milestone 4 — P-frames, Audio, and GOP Aggregation

**Duration target:** 1–2 months.
**Prerequisite:** Milestone 3 complete; a single I-frame proves and verifies on a single GPU within budget.
**Outcome:** the system can prove a real GOP (one I-frame plus N P-frames) of baseline H.264 alongside its matching AAC-LC audio, aggregated into one proof per GOP. This is the first milestone where "video" is actually video, not a still.

## 1. The statement we prove

Per GOP, both video and audio are folded into a single proof.

> **Public input:** `bitstream: Vec<u8>` (one GOP's worth of NAL units + the matching ADTS/AAC frames), `manifest: SignedManifest`, `tolerance: { video_psnr_db, audio_mse }`
> **Private input (witness):** `original_yuv: Vec<YuvFrame>`, `original_pcm: PcmAudio`, `merkle_paths`
> **Claim:**
> 1. Manifest signature valid.
> 2. Every original frame and audio window authenticates against the manifest's Merkle roots.
> 3. `decode_h264_baseline(bitstream.video) == reconstructed_frames`.
> 4. `decode_aac_lc(bitstream.audio) == reconstructed_pcm`.
> 5. For each frame `i`: `psnr(reconstructed_frames[i], original_yuv[i]) ≥ tolerance.video_psnr_db`.
> 6. For each window `j`: `mse(reconstructed_pcm[j], original_pcm[j]) ≤ tolerance.audio_mse`.

GOP size for the first cut: **15 frames** (1 IDR + 14 P-frames at 30 fps = 0.5 s).

## 2. New scope on top of milestone 3

### Video — P-frames

- `slice_type = P` slices.
- One reference picture (the previous decoded frame); no list management beyond `RefPicList0[0]`.
- MB types: `P_L0_16x16`, `P_L0_16x8`, `P_L0_8x16`, `P_8x8` (with `sub_mb_type` ∈ {`P_L0_8x8`, `P_L0_8x4`, `P_L0_4x8`, `P_L0_4x4`}), and `P_Skip`.
- Motion vector prediction (median of top, top-right, left neighbors) and `mvd_l0` decode.
- Quarter-pel luma motion compensation: H.264's 6-tap filter `(1,-5,20,20,-5,1)/32` for half-pel, bilinear average for quarter-pel.
- Eighth-pel chroma motion compensation: bilinear.

### Audio — AAC-LC

- ADTS framing.
- Mono and stereo, 16-bit, 48 kHz only for v1.
- Single AAC profile: LC.
- Decoder pipeline: bitstream parse → Huffman + scalefactor decode → inverse quantization → optional TNS → IMDCT (1024 / 128 samples) → window + overlap-add → PCM out.
- Skip: SBR, PS, HE-AAC, multichannel beyond stereo, M/S except in basic LC form.

### Out

- Multiple reference pictures.
- Long-term references.
- Weighted prediction.
- Deblocking (still off; revisit in milestone 5).
- B-frames.
- CABAC.

## 3. Aggregation strategy — chained per-frame proofs

A 15-frame GOP cannot live in one monolithic proof at 1080p. The architecture:

```
                     ┌──────────────────┐
   I-frame proof ───▶│ commits to        │
   public:           │  reconstructed[0] │──┐
     bitstream[0]    │  bottom-edge[0]   │  │
     manifest        └──────────────────┘  │
                                            │
                     ┌──────────────────┐  │
   P1 proof   ◀──────│ takes recon[0]   │◀─┘
   public:           │ as public input, │
     bitstream[1]    │ uses witnessed   │
     prev_commit[0]  │ recon[0] pixels, │
                     │ checks they hash │
                     │ to prev_commit   │
                     │ commits recon[1] │──┐
                     └──────────────────┘  │
                                            │
                            ⋮               ⋮  (chains for P2 ... P14)

           ┌───────────────────────────────────────┐
           │ aggregator proof folds all 15 chunks  │
           │ into one final proof for the GOP      │
           └───────────────────────────────────────┘
```

Mechanics:

- Each per-frame proof's **public output** includes a hash of its reconstructed YUV (small — Merkle root over the frame's MBs).
- The next P-frame proof's **public input** includes the previous frame's commitment, and its **private witness** includes the actual reference pixels. Step 1 inside the proof: verify the witnessed reference hashes to the claimed commitment.
- Audio chunks ride alongside on a parallel chain (each audio chunk depends only on its own bitstream, not on the previous chunk — overlap-add state is local to the AAC frame in LC mode).
- The aggregator (RISC Zero `Receipt::compose` / SP1 recursion) folds all per-frame video proofs and per-window audio proofs into one terminal proof for the GOP.

Net result: the verifier sees one proof per GOP, with the same statement shape as a single-frame milestone-3 proof.

## 4. GPU prover

Until now we've been "single GPU is enough." Milestone 4 puts that to the test.

- Worker per chunk proof; aggregator on its own worker.
- Job queue (Redis or SQS) keyed by `(video_id, gop_index)`; per-frame jobs fan out within a GOP.
- Per-worker target: a single L4 or T4. The aggregator may need more memory; an A10 / A100 is the fallback.
- Bench across worker count (1, 2, 4, 8) to characterize parallel speedup and identify the GOP-aggregation bottleneck.

## 5. Acceptance criteria

1. **Native parity** — `h264-decoder` (with P-frame support added) and `aac-lc-decoder` both bit-exactly match the JM reference (video) and the FAAD2 / FFmpeg reference (audio) over the test corpus.
2. **End-to-end proof** for a 15-frame 720p baseline GOP plus its stereo 48 kHz AAC-LC audio: produces one aggregated proof per GOP.
3. **Tampering tests pass closed:**
   - Reorder two P-frames in the bitstream → fails.
   - Replace one P-frame's motion vectors → fails.
   - Substitute a different audio track over the same video → fails.
4. **Bench numbers recorded** at 480p / 720p / 1080p for one GOP each, on a single GPU and on a 4-GPU pool.
5. **Browser verifies** the aggregated GOP proof in under 2 s.

## 6. New crates

```
snarkvid/
  crates/
    h264-decoder/       # extend with mc.rs, motion_vec.rs, p_slice.rs
    aac-lc-decoder/     # new: bitreader (shared with h264?), huffman, tns, imdct, window
  prover/
    aggregator/         # new: drives RISC Zero / SP1 recursive aggregation
    coordinator/        # new: per-GOP job fan-out, queue integration
```

`aac-lc-decoder` is a sibling crate; don't fold it into `h264-decoder`. They share `bitreader` (move it to a `crates/bitstream-utils` crate during this milestone).

## 7. What we measure

| Metric | Why |
|---|---|
| Cycles per P-frame, split by stage (CAVLC, MC, intra, residual) | MC is the new hot path; we need to know its cost. |
| Cycles per audio window (per IMDCT, etc.) | Sets the audio budget. |
| Aggregator cycles per GOP | Recursion is its own cost center; first time we measure it. |
| Wall-clock per GOP, single GPU vs 4-GPU pool | Tells us if the architecture scales horizontally. |
| Aggregated proof bytes | Should be ~constant regardless of GOP size; flag if it grows. |
| Browser verify time for the aggregated proof | Should match milestone 3; flag regressions. |
| End-to-end "minute of video proved per GPU-hour" | The headline economic number for the v1 design. |

## 8. Risks

| Risk | Mitigation |
|---|---|
| Motion compensation's 6-tap filter is too costly per pixel | Implement it branchless and table-free; if still slow, switch to an integer-only approximation behind the same proof statement (the reconstructed pixels just have to match the spec). |
| Reference-frame commitment via Merkle is too expensive per P-frame | Use a single-leaf hash of the whole reference frame instead of per-block — the P-frame proof needs the whole reference anyway. |
| Recursive aggregation immature in the chosen zkVM | Fall back to a verifier that accepts a vector of per-frame proofs (one HTTP call uploads them all). Larger total proof size, but unblocks the rest of the milestone. |
| AAC IMDCT cycles surprise us | The IMDCT is a small fixed-size DCT; precompute twiddle tables; if still bad, prove audio in larger chunks (multiple AAC frames per proof) to amortize. |
| GOP-level aggregation memory exceeds single-GPU capacity | Either fold pairwise (binary tree of half-proofs) or move the aggregator to a larger machine; document which. |

## 9. First steps

1. **`h264-decoder` P-frame native** (weeks 1–2). Add MV decode, MC, P-slice parsing. Parity against JM on a P-frame corpus.
2. **`aac-lc-decoder` native** (weeks 1–3, in parallel). Build bottom-up; parity against FAAD2 on an LC corpus.
3. **`prover/aggregator`** (week 3). Skeleton that takes N proofs and produces one; smoke-test on milestone-3-style I-frame proofs first.
4. **Single P-frame in-circuit** (week 4). Smallest end-to-end check.
5. **15-frame GOP, video only** (weeks 5–6). Chain proofs, aggregate, verify.
6. **Add audio chain** (week 6).
7. **Tampering + bench at three resolutions, single GPU then 4-GPU pool** (weeks 7–8).
8. **Write `MILESTONE_4_RESULTS.md`** including the headline "video minutes per GPU-hour" number.
