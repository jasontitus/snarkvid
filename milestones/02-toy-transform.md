# Milestone 2 — Toy Transform End-to-End

**Duration target:** 3–4 weeks.
**Prerequisite:** Milestone 1 complete; a zkVM picked.
**Outcome:** the full architecture (signed manifest → Merkle authentication → in-circuit decoder → tolerance comparator → browser verifier) running end-to-end on a real image, using a trivial codec we define ourselves.

The point is to **shake out the architecture without an H.264 decoder in the way.** Every component except the in-circuit H.264 decoder appears in real form here. If milestone 2 ships, milestone 3 is "swap the toy decoder for a real one."

## 1. The statement we prove

> **Public input:** `compressed: Vec<u8>`, `manifest: SignedManifest`, `tolerance: PsnrDb`
> **Private input (witness):** `original_planes: YuvFrame`, `merkle_paths: Vec<Path>`
> **Claim:**
> 1. `Sig.Verify(manifest.pubkey, manifest.body) == true`
> 2. Each block of `original_planes` authenticates against `manifest.body.merkle_root` via its supplied path.
> 3. `decode_toy(compressed) == reconstructed`.
> 4. `psnr(reconstructed, original_planes) ≥ tolerance`.

Same shape as the eventual milestone-3 statement — just with `decode_toy` instead of `decode_h264`.

## 2. The toy codec — "BlockQuant"

Designed to be the dumbest possible thing that's still non-trivial: it forces real integer arithmetic over real image data without bringing entropy coding or motion compensation into the picture.

- **Partition** each YUV plane into 8×8 blocks (Y full-res, U/V at 4:2:0).
- **Transform** each block with the H.264 integer 4×4 transform tiled to 8×8 (or a 2D Walsh–Hadamard if the integer DCT proves slow in-circuit — pick during milestone 2 day 1).
- **Quantize** with a single uniform `QP` for the whole frame.
- **Bitstream layout:**

  ```
  header: u16 width, u16 height, u8 qp, u8 chroma_format
  body:   i16 coefficients in raster order, Y then U then V
  ```

- **Decode:** parse header, dequantize, inverse transform, clamp to `[0, 255]`.

A separate non-zk binary `toy-encode` produces test inputs. The in-circuit code only ever runs `decode_toy`.

## 3. Acceptance criteria

1. `toy-encode original.yuv qp=8 → compressed.bin` round-trips at PSNR ≥ 40 dB on a 720p test image.
2. `snarkvid-prove --compressed compressed.bin --original original.yuv --manifest m.json → proof.bin` succeeds on a single GPU within the budget set by milestone 1.
3. Browser verifier verifies `proof.bin` in under 2 s on a mid-range laptop and displays the signing identity from the manifest.
4. **Tampering tests all fail closed:**
   - Flip a byte in `compressed.bin` → proof verification fails.
   - Use a manifest signed by an unknown key → fails.
   - Substitute a different image as the witness → prover cannot produce a valid proof.
   - Lower `tolerance` below the actual PSNR → fails.
5. Bench numbers recorded at 480p / 720p / 1080p single-frame inputs.

## 4. New crates

```
snarkvid/
  crates/
    manifest/           # SignedManifest type, Ed25519 verify, Merkle tree (Poseidon if zkvm-friendly, else SHA-256)
    toy-codec/          # encode + decode for BlockQuant; no_std so the decoder runs in-guest
    comparator/         # PSNR / MSE primitives; no_std
  bin/
    toy-encode/         # native CLI, produces compressed.bin from raw YUV
    snarkvid-prove/     # the milestone 2 prover
  prover/
    guest/              # zkvm guest program: imports toy-codec, comparator, manifest
    host/               # zkvm host driver
  web/                  # extends milestone 1 verifier with the new public-input shape
```

`toy-codec`, `comparator`, and `manifest` are deliberately built as plain `no_std` crates so the same code runs natively (for `toy-encode`, tests, and the host) and inside the zkVM guest. This is the same discipline we'll need for the H.264 decoder in milestone 3.

## 5. What we measure

For each input resolution (480p, 720p, 1080p) on a single GPU:

| Metric | Why it matters |
|---|---|
| Cycle count split by component (decoder / Merkle / signature / comparator) | Tells us where milestone 3's per-frame budget will go. |
| Prove wall-clock | Sets the ceiling for milestone 3's per-frame time. |
| Proof size | Should match milestone 1; flag regressions. |
| Browser verify time | Should match milestone 1. |
| Witness size in bytes | Calibrates the I/O cost of streaming frames into the guest in milestone 3. |

A single 1080p YUV 4:2:0 frame is ~3 MB of witness. If feeding that through the chosen zkVM dominates the cycle budget, milestone 3 will need per-tile proving.

## 6. Out of scope

- Multiple frames / temporal coding
- Audio
- Real H.264
- Recursion / aggregation
- Production verifier UX

## 7. Risks

| Risk | Mitigation |
|---|---|
| Integer DCT too slow in the chosen zkVM | Fall back to Walsh–Hadamard for the toy codec; flag for milestone 3 review. |
| Per-block Merkle proofs dominate cost | Increase Merkle leaf granularity (e.g., one leaf per 64×64 tile instead of per 8×8 block). |
| Poseidon implementation isn't production-grade in the chosen zkVM | Use SHA-256 for Merkle; accept the cycle cost; revisit hash choice later. |
| 1080p prove time exceeds budget | Re-scope milestone 3 to 720p maximum. |

## 8. First steps

1. **`crates/toy-codec`** — native Rust, deterministic, fully tested against fixed test vectors. No zkvm involvement yet.
2. **`crates/manifest`** — define on-disk format, Ed25519 signing, Merkle tree. Sign + verify + authenticate-leaf APIs.
3. **`crates/comparator`** — PSNR over `(decoded, original)` slices; small.
4. **`bin/toy-encode`** — CLI wrapping `toy-codec` to make fixtures.
5. **`prover/guest`** — single guest program tying the three crates together. End-to-end smoke test on one fixed image.
6. **`web/`** — extend the milestone 1 verifier with the new public-input shape; show the signing identity.
7. **Tampering test suite** — automated tests for each of the failure modes in §3.4.
8. **Benches at 480p / 720p / 1080p** — emit a `MILESTONE_2_RESULTS.md` with the table from §5.
