# Milestone 5 — Hardening to v1

**Duration target:** ongoing; complete to ship v1.
**Prerequisite:** Milestone 4 complete; the system can prove and verify a real GOP of video + audio.
**Outcome:** the system is shippable as a public service. Everything that's been deferred for "later" gets resolved here.

This milestone is structured as a checklist rather than a single deliverable. Each item below is independently scoped and can be parallelized.

## 1. Browser verifier UX

Today the verifier is a developer harness. v1 is a tool a non-technical person can use.

- **Single drop target.** Drop an `.mp4` (with embedded proof in a C2PA assertion) or a `.mp4` + sidecar `.proof`. Auto-detect.
- **Inline player** with verification status overlaid before any pixels render. Do not autoplay if invalid.
- **Status states** rendered distinctly:
  - **Verified by `<identity>`** — green badge, identity name, capture timestamp from manifest.
  - **Unverifiable** — gray badge with reason: no proof, unknown signing key, expired key, etc.
  - **Tampered** — red banner, video does not play.
- **Identity display** uses the human-readable name from the trusted-issuer registry (see §3), not the raw pubkey hex.
- **Copyable evidence summary** — one click to copy a JSON blob containing manifest, proof hash, verification timestamp, identity. For incident reports.

## 2. Signing identity and trust roots

The verifier needs a curated list of trusted issuers. Without it, every proof is just "signed by some random key."

- **Trusted issuer registry** — JSON document hosted at a stable URL, listing pubkeys with metadata: `{ pubkey, name, organization, valid_from, valid_until, revoked_at? }`.
- **Distribution** — signed by a project root key; verifier embeds the root key, fetches the registry on load, falls back to a cached copy with a staleness warning if offline.
- **Update cadence** — daily refresh; verifier refuses proofs signed by keys not in the most recent successfully-fetched registry.
- **Per-issuer scoping** — registry entries can constrain what a key may sign for (e.g., a body-cam vendor's key signs only manifests with `device_class = "body-camera"`). Verifier enforces.

## 3. Revocation

A signing key gets compromised. We must be able to invalidate every proof it ever signed without re-issuing all proofs.

- **Two layers:**
  - **Soft revocation** — registry entry's `revoked_at` field. Verifier rejects manifests whose `created_at` is after `revoked_at`.
  - **Hard revocation** — the registry entry disappears entirely. Verifier rejects all manifests signed by that key, regardless of timestamp.
- **Default: soft.** Only escalate to hard if the key is known to have signed fraudulent manifests.
- **Audit trail** — each revocation entry includes a reason and a link to a public statement; verifier surfaces this in the UI.
- **No on-chain revocation in v1.** A signed-and-mirrored JSON file is enough; revisit if a participant requires trustless revocation.

## 4. C2PA integration

We're not the only people thinking about media provenance. Don't reinvent the manifest container.

- **Manifest mapping** — express our `SignedManifest` as a custom C2PA assertion (`com.snarkvid.derivation_proof.v1`).
- **Proof assertion** — embed the milestone-4 aggregated proof as a separate C2PA assertion inside the same manifest.
- **Standard C2PA tools** (Adobe `c2patool`, Truepic verifiers) can read everything; only our verifier checks the proof bytes.
- **Compatibility** — for users without our verifier, a C2PA-aware viewer still shows "signed by `<identity>`" via the standard claim signature; they just don't see the derivation guarantee.

## 5. Encoder presets

The in-circuit decoder accepts a strict subset of H.264 + AAC-LC. Producers need a foolproof way to emit conforming bitstreams.

- **`snarkvid-encode` CLI** — wraps `ffmpeg` with the canonical params:
  ```
  ffmpeg -i in.mov \
    -c:v libx264 -profile:v baseline -bf 0 -refs 1 -weightb 0 -8x8dct 0 \
                 -x264-params no-deblock=1 -pix_fmt yuv420p \
    -c:a aac -profile:a aac_low -ar 48000 -ac 2 -b:a 128k \
    out.mp4
  ```
- **`snarkvid-validate` CLI** — parses any input and emits `accepted` / `rejected (<reason>)`. Wraps the milestone-3 `validate-bitstream` and adds AAC-LC checks.
- **Presets bundled with the prover** — submitting a non-conforming file to the prover API fails fast with a pointer to `snarkvid-encode`.

## 6. Prover service operations

The prover stops being a `cargo run` and becomes a service.

- **Container image** — pinned toolchains, GPU drivers, model files baked in; reproducible build.
- **Deployment target** — Kubernetes with one Deployment per worker class (per-GOP workers, aggregator workers, coordinator).
- **Autoscaling** — keyed on queue depth; max-scale capped to keep cloud costs predictable.
- **Job persistence** — Redis (queue) + S3-compatible object store (originals, proofs).
- **Observability** — per-stage cycle counters, per-GOP wall-clock, failure rate, queue depth — Prometheus exporter on every worker.
- **Cost dashboard** — "minutes of video proved per dollar" tracked per day.

## 7. API surface

What third parties integrate against.

- **`POST /jobs`** — submit `{ original_url, compressed_url, manifest_url }`; returns `job_id`.
- **`GET /jobs/{id}`** — status (`queued | proving | aggregating | done | failed`), progress fraction, proof URL when ready.
- **`POST /verify`** (server-side, for clients without WASM) — submit `{ compressed_url, proof_url }`; returns the same result the browser verifier produces.
- **Auth** — API key per submitter; rate limits per key; per-organization signing keys for the manifests they submit.
- **Webhooks** — optional callback URL for `job_done` events.

## 8. Distribution and verifier release

Verifier code is part of users' trust base. It must be auditable and pinned.

- **Releases tagged in git**, with the verifier WASM bundles built reproducibly in CI.
- **Subresource integrity** — the verifier HTML pins the WASM bundle by SHA-384.
- **Release transparency log** — append-only log of `(release_tag, wasm_sha384)` so anyone can detect a swapped binary.
- **Optional browser extension** — auto-attaches the verifier to videos on social platforms; same WASM bundle, different host.

## 9. Documentation

What we ship for users to read.

- **`docs/threat-model.md`** — what the proof guarantees, what it doesn't, who you trust, why.
- **`docs/integration-producer.md`** — how a content producer signs originals and submits jobs.
- **`docs/integration-verifier.md`** — how a publisher embeds the verifier on their site.
- **`docs/troubleshooting.md`** — common encoder mistakes, registry issues, etc.
- **Reference implementation videos** — a short library of known-good signed originals + their proofs for testing.

## 10. Security review

Before going public.

- **External audit of the in-circuit decoder.** Any spec deviation is a soundness bug; a malicious prover could craft bitstreams that decode correctly under our decoder but display differently in real players.
- **External audit of the manifest format and signature handling.** Off-by-one in Merkle path verification, signature malleability, replay between manifests, etc.
- **Fuzzing in CI** kept running indefinitely once stood up: bitstream fuzzer (random bytes in, decoder must not crash), manifest fuzzer (random JSON in, parser must not panic), proof fuzzer (random bytes as proof, verifier must reject without crashing).
- **Reproducible build verification** — third party can reproduce the WASM bundle byte-for-byte from the tagged source.
- **Coordinated disclosure** — `security.txt` and a published response process before launch.

## 11. Out of scope (still)

These are real problems but not v1.

- **On-chain (Solidity) verifier** — desirable for some applications; defer until there's a concrete consumer.
- **Live streaming / HLS chunked proofs** — requires per-segment proofs and a different aggregation shape; defer.
- **Edits and redactions** — blurring faces, cutting clips. Each is its own derivation proof; the architecture supports them but the decoders aren't written.
- **B-frames, CABAC, High profile** — explicit non-goals for v1.
- **HE-AAC, Opus, multichannel** — same.

## 12. v1 launch checklist

Concrete go/no-go gates for declaring v1:

- [ ] Browser verifier handles all status states (verified / unverifiable / tampered) on a curated test corpus of 50 videos.
- [ ] Trusted issuer registry hosted, signed, and refreshed automatically.
- [ ] Revocation tested end-to-end (key marked revoked → verifier rejects within 24 h).
- [ ] At least one C2PA-aware viewer reads our manifest correctly (without proof verification — that's our value-add).
- [ ] `snarkvid-encode` produces conforming bitstreams from a corpus of 100 source clips with zero `snarkvid-validate` failures.
- [ ] Prover service holds 99% job-success rate over a 7-day soak.
- [ ] External security audit completed; all critical findings closed.
- [ ] Reproducible build verified by an independent party.
- [ ] Threat model document published; integration docs published.
- [ ] Cost dashboard shows headline number ("minutes of video per GPU-hour") and it's within the project's economic target.
