# snarkvid — Design

A system for proving that a compressed video (H.264 + audio) shows and sounds like a cryptographically signed original, without revealing the original.

## 1. Goal

Maintain a hardware-backed chain of trust through web compression. Given a "verified original" (e.g., a body-cam or evidence file signed by a hardware-attested capture device), allow anyone — in a browser — to confirm that a small, web-friendly compressed file depicts the same content as that signed original.

The semantic the system guarantees:

> **The compressed file, when played back, shows the same thing and sounds the same as the verified original.**

It does *not* guarantee anything about how the compression was performed. Any encoder, any parameters, any tool chain is acceptable as long as the playback content matches within a stated tolerance.

## 2. Trust model

- **Trusted source.** A hardware key or organizational key signs a commitment to the original media. Specifically: a Merkle root over raw decoded YUV frames + raw PCM audio samples, plus metadata (resolution, sample rate, frame count, duration). Format candidate: C2PA manifest with a custom assertion, or a lightweight Ed25519 signature over the commitment.
- **Untrusted prover.** The server holding the original and producing the proof is not trusted. It cannot produce a valid proof for content that does not match.
- **Untrusted distribution.** The compressed file and proof can travel through any CDN, mirror, or social platform.
- **Trusted verifier code.** The browser/CLI verifier and the embedded verification key are part of the user's trust base. Distribute via signed releases.

## 3. The formal claim

Public inputs to the verifier:

- `compressed_bytes` — the H.264 + AAC file the user is watching (or its hash).
- `original_commitment` — the signed Merkle root committing to the original frames + audio.
- `signing_pubkey` — identifies the trusted source.
- `tolerance` — the per-frame and per-audio-window similarity bounds (e.g., PSNR ≥ 36 dB, audio MSE ≤ τ).

Private inputs (witness, only the prover sees):

- The original YUV frames and PCM audio samples.
- Merkle authentication paths for each frame and each audio window.

The proof attests:

1. `Sig.Verify(signing_pubkey, original_commitment) == true`.
2. Each original frame `F_i` and audio window `A_j` is consistent with `original_commitment` via the supplied Merkle paths.
3. `decode_h264(compressed_bytes) → (decoded_frames, decoded_audio)` runs to completion under the spec.
4. For each `i`: `dist_video(decoded_frames[i], F_i) ≤ tolerance.video`.
5. For each `j`: `dist_audio(decoded_audio[j], A_j) ≤ tolerance.audio`.

The original is never revealed.

## 4. Architecture

```
┌────────────────────────┐        ┌──────────────────────────┐
│  Capture device        │        │  Server (untrusted)      │
│  (signs original)      │        │                          │
│                        │  orig  │  ┌────────────────────┐  │
│  hardware key ─────────┼───────▶│  │ encode (any tool)  │  │
│                        │        │  └─────────┬──────────┘  │
└────────────────────────┘        │            ▼             │
                                  │  ┌────────────────────┐  │
                                  │  │ prover (zkVM,GPU)  │  │
                                  │  │  - decode in-circuit│ │
                                  │  │  - compare to orig │  │
                                  │  │  - check signature │  │
                                  │  └─────────┬──────────┘  │
                                  │            ▼             │
                                  │  compressed.mp4 + proof  │
                                  └────────────┬─────────────┘
                                               │
                                               ▼
                              ┌──────────────────────────────┐
                              │  Browser / CLI verifier      │
                              │  (WASM, milliseconds)        │
                              │  inputs: compressed + proof  │
                              │          + trusted pubkeys   │
                              │  output: ✓ signed by <id>    │
                              └──────────────────────────────┘
```

## 5. Why decode-and-compare (not encode-in-circuit)

H.264 encoders are mostly *unspecified*: rate control, mode decision, motion estimation are heuristic, and every encoder makes different valid choices. Putting an encoder in a circuit means freezing one specific implementation and proving bit-exact reproduction — brittle and enormous.

H.264 decoders are *fully specified*: given a bitstream, exactly one output is correct. A minimal baseline-profile decoder is ~5–10 K lines of straight-line code. AAC-LC decoding is similarly well-specified.

So we invert the problem. The prover may use any encoder it likes. In-circuit, we run the *decoder* on the resulting bitstream and check that what comes out matches the signed original within tolerance.

This matches the user-facing claim: "shows and sounds the same as the original."

### What this approach does not prove

- Not bit-exact reproduction. Two different encoders both meeting the tolerance both pass.
- Not compression efficiency. A maliciously huge "compressed" file would still verify — but file size is publicly visible, so this is a non-issue.
- Not freedom from edits that fall under the perceptual threshold. Tolerance must be chosen accordingly.

## 6. Comparison metrics

### Video

Per-frame L2 / SAD between decoded YUV and original YUV, expressed as PSNR for human readability.

```
mse_i = (1/N) * Σ (decoded[i,p] - original[i,p])²       per pixel p
psnr_i = 10 * log10(255² / mse_i)
require: psnr_i ≥ tolerance.video_psnr_db
```

In-circuit cost is dominated by the subtraction-and-square loop over pixels. For 720p: ~2.7 M ops/frame — large but tractable inside a zkVM.

We avoid SSIM/VMAF — too expensive (means, variances, divisions, log).

### Audio

Per-window MSE between decoded PCM and original PCM, with windows aligned on AAC frame boundaries (1024 samples typically).

```
mse_j = (1/M) * Σ (decoded[j,s] - original[j,s])²
require: mse_j ≤ tolerance.audio_mse
```

Audio is cheap relative to video.

### Tolerance choice

Defaults targeting "visually and audibly indistinguishable to a typical viewer":

- `video_psnr_db ≥ 36`  (industry rule-of-thumb for "transparent")
- `audio_mse` corresponding to ≥ ~40 dB SNR on 16-bit PCM

These are configuration knobs, not constants — operators choose based on their threat model.

## 7. Proof system

**Selection: STARK-based zkVM (RISC Zero or SP1).**

Reasoning:

- No per-circuit trusted setup — operationally critical for a system that may iterate on the in-circuit decoder.
- Lets us write the decoder in normal Rust and compile it for the guest, instead of hand-coding it in Circom/Halo2.
- Both have GPU provers and recursive proof aggregation in production.
- Verifier compiles to WASM for browser use; both have working examples.
- Tradeoff: proofs are larger (tens to hundreds of KB) than Groth16 (~200 B). Fine for our use case — the proof rides alongside a multi-MB video file.

We will pick between RISC Zero and SP1 after the milestone-1 spike, based on cycle count and proving time on the same workload.

## 8. Scoping the in-circuit decoder

Even the decoder is not free. The in-circuit decoder is restricted to a subset of H.264 chosen for circuit feasibility:

- **Baseline profile only** — uses CAVLC entropy coding (table lookups), avoiding CABAC's stateful arithmetic coding, which is hostile to zkVMs.
- **I and P frames only** — no B-frames.
- **Single slice per frame** — no FMO/ASO.
- **Constrained macroblock types** — initially I-frames only, then add P-frame motion compensation.
- **Deblocking filter optional** — initially off (loosen PSNR threshold to compensate); add later.

Audio: **AAC-LC** only (no SBR, no PS, no HE-AAC v2).

Encoders that produce these subsets are common (`x264 --profile baseline`, `ffmpeg -profile:a aac_low`).

## 9. Per-frame proofs and recursion

Proving a 10-second clip in a single proof is infeasible. Architecture:

- **Per-frame (or per-GOP) proofs.** Each proves a small chunk of the bitstream decodes to frames matching the corresponding Merkle-authenticated originals.
- **Audio chunk proofs.** Each AAC frame batched similarly.
- **Recursive aggregation.** All chunk proofs are folded into one final proof. RISC Zero (`Receipt::compose`) and SP1 (recursion) both support this.
- The verifier sees one proof at the end.

This also gives us GPU parallelism: chunk proofs run concurrently on independent workers.

## 10. Components

### 10.1 Original signing format

A signed manifest containing:

- `version`
- `video`: `{ width, height, frame_count, fps_num, fps_den, color_space, merkle_root_yuv }`
- `audio`: `{ sample_rate, channels, sample_count, merkle_root_pcm }`
- `created_at`, `device_id`, optional C2PA assertions
- `signature` (Ed25519 or device-attested key)

The Merkle leaves are individual frames (raw YUV planes) and audio windows (raw PCM). Hash function: a circuit-friendly choice — Poseidon or Rescue — to keep the in-circuit Merkle verification cheap.

### 10.2 Prover (server)

- **API**

  ```
  POST /jobs
    body: { original_url, compressed_url, manifest_url }
    → { job_id }
  GET /jobs/{job_id}
    → { status, progress, proof_url? }
  ```

- **Workers**: GPU-backed (T4 / L4 / A10). One worker per chunk proof; one aggregator worker.
- **Queue**: Redis or SQS. Jobs are minutes-to-hours long; everything async.
- **Storage**: originals stay on the prover host; only the compressed file + proof are exposed.
- **Encoder**: any. Default `ffmpeg` with baseline-profile constraints.

### 10.3 Verifier — browser

- WASM bundle of the chosen zkVM verifier (~1–2 MB after compression).
- Verification key + trusted-pubkey set shipped as static assets.
- UI:
  - User drops or links a `.mp4` and `.proof`.
  - Verifier returns one of: `valid (signed by <identity>)`, `invalid`, `error`.
  - Display playback with a badge showing the signing identity.
- Reverse-DNS load of trusted-pubkey set so revocations propagate.

### 10.4 Verifier — server / CLI

- Same WASM bundle, run under Node (`@anthropic-ai/...` n/a — just `wasi`).
- Rust-native CLI for CI / batch use.
- Optional Solidity verifier contract for on-chain attestation (out of scope for v1).

## 11. Milestones

1. **Spike (1–2 wk).** End-to-end "hello world": prove SHA-256 of a file in the chosen zkVM, verify in the browser via WASM. Validates the verifier UX and proof-size budget. Picks RISC Zero vs SP1.
2. **Toy transform (3–4 wk).** Prove `decoded_pixels(downscale_2x(compressed)) ≈ original` on a single still image. Real Merkle hashing, real signature check, no codec yet.
3. **Baseline H.264 I-frame (2–3 mo).** In-circuit decoder for I-frames only, single slice, no deblocking. Per-frame proof.
4. **P-frames + audio + GPU (1–2 mo).** Add motion compensation and AAC-LC. Move to GPU prover. Recursive aggregation across a GOP.
5. **Hardening (ongoing).** Browser verifier UX, signing-identity display, revocation, C2PA integration, encoder presets that reliably produce the in-circuit-supported subset.

The milestone-1 spike is the **go/no-go gate.** If proof times for SHA-256 + a trivial pixel comparison aren't within an order of magnitude of the budget, scope must change before milestone 3.

## 12. Open questions

- **Proof system pick:** RISC Zero vs SP1 — settle in milestone 1.
- **Original signing format:** C2PA vs custom. C2PA gives ecosystem alignment; custom gives a tighter binding to our Merkle layout.
- **Hash inside the circuit:** Poseidon (smaller circuit, less ecosystem) vs Blake3 / SHA-256 (larger circuit, broader tooling).
- **Frame chunking granularity:** per-frame vs per-GOP — affects parallelism vs proof overhead.
- **Tolerance defaults:** what PSNR / audio-SNR floor matches "visually and audibly indistinguishable" for the target content type (camera footage vs talking head vs screen capture).
- **Color space + chroma subsampling:** the comparator must handle YUV 4:2:0 correctly; decide whether to compare in YUV or RGB.

## 13. Out of scope for v1

- B-frames, CABAC, High Profile.
- HE-AAC, Opus, multi-channel audio beyond stereo.
- Live streaming / chunked HLS proofs.
- Edits, redactions, blurring (would need a separate transformation proof).
- On-chain verification.
