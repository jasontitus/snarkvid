# snarkvid-h264-decoder

Baseline H.264 I-frame decoder, `no_std`. This is the M3 in-circuit
decoder per `milestones/03-h264-iframe.md`.

## Pipeline

```
bitstream (Annex-B)
  ──▶ nal::NalUnitIterator + strip_emulation_prevention
  ──▶ slice::Sps::parse / Pps::parse
  ──▶ slice::SliceHeader::parse_into (advances the bit cursor)
  ──▶ frame::decode_iframe (per-MB loop):
        ──▶ mb::parse_macroblock_header
        ──▶ mb::decode_macroblock_residuals
        ──▶ ◯ reconstruction (intra::predict_* + transform::idct_4x4
              + quant::inverse_quant_*  + add residual + clamp)  ← NEXT
  ──▶ DecodedFrame { width, height, y, u, v }
```

The ◯ step is what's still TODO. Every other step is implemented and
tested against the live `noise-16x16-qp18` x264 corpus.

## Module status (committed)

| Module       | What it does                                | Status                       |
|--------------|---------------------------------------------|------------------------------|
| `bitreader`  | u(n), ue(v), se(v), te(v) Exp-Golomb        | done                         |
| `nal`        | Annex-B framing + emulation-prevention strip | done                         |
| `slice`      | SPS / PPS / SliceHeader (parse + parse_into) | done (VUI skipped)           |
| `cavlc`      | coeff_token + level + total_zeros + run_before + decode_residual_block | partial tables (see below) |
| `transform`  | 4×4 integer IDCT + 4×4/2×2 DC Hadamard       | done                         |
| `quant`      | inverse quant 4×4 AC + DC variants          | done                         |
| `intra`      | 9 Intra_4×4 + 4 Intra_16×16 + 4 chroma 8×8  | done                         |
| `mb`         | MB header + residual block reads            | done                         |
| `frame`      | `decode_iframe` skeleton                    | placeholder pixels           |

## What's still open

Ordered by what unlocks pixel-correct output:

1. **Reconstruction inside `frame.rs`** — for each MB, resolve
   Intra_4×4 modes against neighbor-block context (M3 §8.3.1.1),
   call `intra::predict_*` to produce predicted pixels, dequantize
   and IDCT each present residual block, add residual + clamp,
   write the reconstructed pixels back into the plane buffers so
   subsequent MBs can use them as neighbors. ~300 lines.

2. **Fill the rest of the CAVLC tables** (mechanical transcription
   from H.264 spec):
   - `total_zeros` TC=8..15
   - `run_before` zl=7..14
   - `coeff_token` VLC0 rows TC=9..16
   - full VLC1 (nC ∈ [2,4)) and VLC2 (nC ∈ [4,8)) tables
   ~330 entries total.

3. **Proper nC selection for `coeff_token`** — currently always uses
   VLC0. Real H.264 derives nC from the neighbor blocks' TotalCoeff;
   needs the same cross-MB state machine as (1).

4. **Full VUI parser inside SPS** — currently skipped (works for
   our corpus because `decode_iframe` doesn't read past
   `frame_cropping_flag`).

5. **Hadamard-DC pipeline integration** — quant variants exist; need
   to wire `hadamard_4x4` + `inverse_quant_luma_dc_intra16x16` as the
   luma-DC pass for `I_16x16` macroblocks, and the analogous chroma
   path. (Our 16×16 corpus uses `I_NxN`, so this is exercised by
   later corpus fixtures.)

6. **`I_PCM` raw-byte read** — spec requires support; rare in
   practice; not in our corpus.

7. **Differential parity test**: `decode_iframe(corpus.h264)` vs
   `corpus.decoded_yuv` (the ffmpeg reference). Bit-exact Y/U/V is
   M3 §3.1 acceptance.

## Test corpus

`crates/h264-test-vectors/fixtures/` ships:

- `noise-16x16-qp18.h264` — 1018-byte H.264 bitstream (1 IDR I-frame,
  16×16, baseline profile, no CABAC / 8×8 DCT / deblocking, mb_type
  = I_NxN per x264's logs)
- `noise-16x16-qp18-decoded.yuv` — ffmpeg's bit-exact reconstruction,
  the parity target.
- `noise-16x16.yuv` — the original raw input for completeness.

Regenerate with `scripts/gen_h264_corpus.sh` (needs `apt install x264
ffmpeg`).

## Test count

119 unit tests (this crate alone). 153 in the broader workspace.

```
$ cargo test -p snarkvid-h264-decoder
test result: ok. 119 passed; 0 failed
```

`cargo build -p snarkvid-h264-decoder --no-default-features` is also
clean — confirms the crate compiles `no_std` for the eventual SP1
guest.
