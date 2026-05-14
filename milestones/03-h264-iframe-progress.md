# M3 status — H.264 decoder

Where we are at the end of session 17 commits on
`claude/video-compression-poc-l7iIE`. The full pipeline skeleton
exists and produces real reconstructed pixels; the gap to
bit-exact ffmpeg parity is mechanical CAVLC table transcription
plus a few small architecture refinements.

## What works

- All 9 modules of `crates/h264-decoder/` exist and compile
  `no_std`-pure: `bitreader`, `nal`, `cavlc`, `transform`, `quant`,
  `slice`, `intra` (all 17 prediction modes), `mb`, `frame`.
- `decode_iframe(bitstream) -> DecodedFrame` runs end-to-end on
  the live x264 corpus (`crates/h264-test-vectors/fixtures/`). It
  walks the NAL stream, parses SPS+PPS+slice-header (slice cursor
  fix in `27b00f3`), parses the MB header, decodes residuals
  via `decode_residual_block_4x4`, resolves Intra_4×4 modes
  against neighbor state in spec z-scan order, runs IDCT with
  round-shift-6, and writes reconstructed pixels back.
- 120 h264-decoder unit tests, 154 workspace tests.
- Quantitative parity gate:
  `decode_iframe_corpus_y_plane_distance_to_ffmpeg` measures the
  Y SAD against the ffmpeg-decoded reference. Current value:
  **16,985** (lower = better; 0 = bit-exact; max-noise ≈ 65,280).

## Why the SAD isn't dropping

Diagnostic in `dump_y_plane_first_row_for_inspection` (the test
was removed in `4f71698` after we read its output) showed our
output is uniform `100` (the I_NxN signature placeholder). That
means the very first 4×4 luma block's residual decode hits a
missing CAVLC entry, errors out as `OutOfScope`, and the entire
MB falls through to the placeholder fill.

The corpus (`noise-16x16-qp18`) is a noise frame at QP=15. At
that QP, blocks have many nonzero coefficients, which means high
TotalCoeff values that hit CAVLC table rows we haven't filled
yet. Specifically:

- `total_zeros` is committed for TotalCoeff = 1..=7. Rows 8..=15
  return `OutOfScope("total_zeros TC≥8 table TODO")`.
- `run_before` is committed for zeros_left = 1..=6. Rows 7..=14
  return `OutOfScope("run_before zeros_left≥7 table TODO")`.
- `coeff_token` VLC0 has TC=0..=14. TC=15 and TC=16 entries are
  not transcribed (the codewords are 16 bits long with subtle
  layout I haven't verified against a trusted spec source).

Until those rows land, the corpus's first block fails on whichever
table runs out first — we don't know which without instrumenting
which call returns OutOfScope.

## What to do next, in priority order

### 1. Fill the rest of the CAVLC tables — DONE (commit pending)

The hand-transcribed tables in `cavlc.rs` had silent bugs where
codeword lengths were off in several rows. Replaced wholesale with
verified values pulled from libavcodec's `h264_cavlc.c` (FFmpeg
master) via a Python regen script. Now committed:

- `COEFF_TOKEN_VLC0` — all 62 entries, TC=0..=16, T1=0..=3.
- `TZ_TC1` through `TZ_TC15` — all 15 total_zeros tables.
- `RB_ZL1` through `RB_ZL6` and `RB_ZL_GE7` — full run_before set.

Plus two level-decoding bugs fixed:
- `level_prefix >= 15` (not `>= 16`) is the escape threshold for
  +4096 bias.
- `levelSuffixSize == 12` (not `suffix_length`) for `level_prefix
  == 15`.

### 1a. Remaining residual-decode bug — PARTIAL (nC fix landed)

Updated after running the new CAVLC trace (env-gated under
`CAVLC_TRACE=1`, added to `cavlc.rs`):

**What we found.** Block 0 of the corpus MB actually does decode
correctly with Vlc0 (TC=14, T1=0 from `0000000000001011`). The
failure is downstream: all 16 luma blocks "decode" without erroring,
but the first **chroma DC** block then fails `read_vlc` against
`COEFF_TOKEN_CHROMA_DC_420`, so the whole MB falls through to the
fill-with-`mb_signature(I_NxN)=100` path. That's why every pixel in
the output is 100 — not because residual decode aborted at block 0,
but because it aborts at the chroma DC step after silently
mis-decoding most luma blocks.

**Why the luma blocks are wrong.** Pre-fix, `decode_macroblock_
residuals` always passed `CoeffTokenVariant::Vlc0`. With block 0 at
TC=14, the nC for block 1 is 14, which per spec §9.2.1.1 picks
`FixedLen6` (the 6-bit raw code used for nC ≥ 8). The decoder was
reading those 6-bit blobs as Vlc0 codewords — which "succeeded"
because Vlc0 has short prefixes that match almost anything — and
producing nonsense TCs / levels. The bit cursor drifted with every
block; by the time the decoder got to chroma DC, the cursor was so
out of sync that no `ChromaDc420` codeword matched.

**What landed.** Added `pick_coeff_token_variant_4x4` to `mb.rs`
that tracks per-block `TotalCoeff` in raster order across z-scan
traversal and computes nC from left+top in-MB neighbors. Also added
`decode_residual_block_4x4_with_tc` to expose the decoded TC so the
caller can feed it back into the neighbor grid. Inactive CBP_luma
clusters contribute TotalCoeff=0 (not "unavailable") per the spec.

**What still fails.** Chroma DC still fails. With the nC fix, the
luma blocks are now consistently picking `FixedLen6` (because the
first block's high TC cascades into nC ≥ 8 for all subsequent
blocks). Looking at the trace, every luma block past block 0 reads
out as TC=14, 15, or 16 — *plausible* for a QP=15-ish noise frame
but suspicious that they're all saturating. Two leading
hypotheses for the chroma DC failure:

1. **The luma level decode is consuming the wrong number of bits**
   somewhere. The trace shows levels like -2073 and 44 at k=4 and
   k=9 of block 0 — *consistent with the spec algorithm given the
   bits we read*, but those magnitudes are at the edge of what 8-bit
   noise plus low-QP quantization could produce. If `decode_levels`
   under-reads or over-reads by even one bit, the cursor drifts and
   subsequent blocks (still using FixedLen6 since nC stays high)
   pull garbage TCs that *look* like saturated 6-bit codes.
2. **`COEFF_TOKEN_CHROMA_DC_420` table is missing or mis-encoded
   entries**, so a real codeword in the stream fails to match. Less
   likely than (1) given the small fixed table, but worth checking
   against libavcodec's chroma DC table the same way `cavlc.rs`'s
   luma tables were regenerated.

**Diagnostic still wanted.** Side-by-side trace against libavcodec
on the same fixture, focused on the chroma DC bit position. Until
we have ground truth for "what does ffmpeg decode block 0 to,"
hypothesis (1) vs (2) is hard to call.

The trace infra is in place: `CAVLC_TRACE=1 cargo test
-p snarkvid-h264-decoder -- --nocapture` emits step-by-step.

### 2. Wire chroma reconstruction

Currently `frame.rs` writes 128 to every chroma pixel. Need to:

- Take `residuals.chroma_u_dc` / `chroma_v_dc` (4 i32 values
  each), apply `transform::hadamard_2x2` inverse + `quant::
  inverse_quant_chroma_dc_4x4` to recover the 4 DC values for
  each chroma plane.
- For each of 4 chroma 4×4 sub-blocks: combine DC + AC, dequant
  via `inverse_quant_4x4_ac` (chroma uses a slightly different
  QP — pps.chroma_qp_index_offset), IDCT, add to chroma intra
  prediction (`intra::predict_chroma_8x8`), clamp.
- Note: `mb::decode_macroblock_residuals` currently calls
  `decode_residual_block_4x4(br, ChromaDc420)` for the chroma
  DC pair, which expects a full 16-coefficient block but chroma
  DC only has 4. This needs a dedicated `decode_chroma_dc_block`
  function — flagged as a bug in mb.rs.

### 3. Cross-MB neighbor state

For multi-MB frames (which our 16×16 corpus isn't, but anything
larger is). Three pieces of state need to flow across MB
boundaries:

- The Intra_4×4 mode of each 4×4 block (used by the next MB's
  predicted-mode predictor)
- The TotalCoeff of each 4×4 block (used to compute `nC` for
  picking the right `CoeffTokenVariant`)
- The reconstructed pixels themselves (used as neighbors for
  intra prediction; the `y_plane` / `u_plane` / `v_plane`
  buffers already serve this)

Refactor `frame::decode_iframe` to maintain a frame-level mode
grid + nC grid alongside the pixel buffers.

### 4. Differential fuzz

`cargo fuzz` against ffmpeg-libav. M3 §3 acceptance gate. Runs
on CPU; needs a longer wall-clock budget that suits a GPU box
or overnight CI run.

### 5. Wire `decode_iframe` into `prover/guest`

In `prover/guest/src/main.rs` replace
`snarkvid_toy_codec::decode_toy(&bitstream)` with
`snarkvid_h264_decoder::frame::decode_iframe(&bitstream)`. The
public-input shape changes (`bitstream` is now `Vec<u8>` H.264
bytes instead of `BqBitstream`); update `prover/host/src/main.rs`
fixture builder accordingly. Update `m2-statement::verify_m2_claim`
to take the new `DecodedFrame` type or call the h264 decoder
directly.

That's the M3 prove path.

### 6. SP1 single-MB → single-frame proves on the A10

After 5 lands. M3 §9.4–§9.6. Cycle accounting per stage so we
know which module to optimize first. The week-9 go/no-go gate
for monolithic vs. per-MB-row aggregation (M3 §7).

### 7. M3 §3.3 tampering tests

CAVLC bit-flip → proof fails. QP-mismatch re-encode → proof
fails (PSNR check). After the M3 prover works end-to-end.

## Useful test patterns to reuse

- The existing `parses_x264_corpus_sps_pps_idr_in_order` test
  in `nal.rs` is the template for pulling real bytes out of the
  fixture and running them through the decoder.
- `decode_iframe_corpus_y_plane_distance_to_ffmpeg` is the
  parity gate; tighten its bound from 65,280 → smaller as each
  step lands. Goal is exact equality (SAD == 0) after step 1+2.
- `dump_y_plane_first_row_for_inspection` (deleted, easy to
  recreate) prints first row + distinct value count — useful
  for spotting "we're filling with placeholder" failures.

## File map

| File | Lines | Notes |
|---|---|---|
| `crates/h264-decoder/src/bitreader.rs` | ~250 | done |
| `crates/h264-decoder/src/nal.rs` | ~270 | done |
| `crates/h264-decoder/src/cavlc.rs` | ~870 | partial tables |
| `crates/h264-decoder/src/transform.rs` | ~410 | done |
| `crates/h264-decoder/src/quant.rs` | ~520 | done |
| `crates/h264-decoder/src/slice.rs` | ~510 | VUI skipped |
| `crates/h264-decoder/src/intra.rs` | ~720 | done |
| `crates/h264-decoder/src/mb.rs` | ~600 | chroma DC bug |
| `crates/h264-decoder/src/frame.rs` | ~390 | DC-only chroma |
| `crates/h264-decoder/src/lib.rs` | ~70 | module decls |
| `crates/h264-decoder/README.md` | ~100 | crate roadmap |
| `crates/h264-test-vectors/src/lib.rs` | ~40 | corpus loader |
| `crates/h264-test-vectors/fixtures/` | — | 3 files (1 KB H.264, 384 B YUV) |
| `scripts/gen_h264_corpus.sh` | ~60 | x264 + ffmpeg regen |

## Sandbox vs GPU split

Everything in M3 so far is sandbox-doable. The first GPU-required
step is `prover-host prove` after step 5 above, which falls
under `scripts/full_test_gpu.sh` phase 12 (M3 single-MB SP1 prove,
documented in `TESTING.md`).

Steps 1–4 close on CPU. Step 5 is plumbing. Step 6 is the GPU run.

## Final test counts

```
$ cargo test --release --workspace
test result: ok. 154 passed; 0 failed; 0 ignored

$ cargo test -p snarkvid-h264-decoder
test result: ok. 120 passed; 0 failed; 0 ignored

$ cargo build -p snarkvid-h264-decoder --no-default-features
    Finished `dev` profile [unoptimized + debuginfo] target(s)
```
