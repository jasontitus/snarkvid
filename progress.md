# Progress

A running log of what's been built, what's been tested, and what's blocked.
Each entry is timestamped and identifies the milestone it belongs to.
Newest entries at the top.

---

## 2026-05-02 — Kicking off execution

Plan documents are complete (design + 5 milestones + spike scaffold).
Switching to building. Working through milestones in order; documenting
results, decisions, and blockers here as I go.

### Environment baseline

| | |
|---|---|
| CPU | 4 cores, x86_64 |
| RAM | 15 GB |
| GPU | **none** |
| Rust | 1.94.1 stable |
| Cargo | 1.94.1, crates.io reachable |
| Node | 22.22.2 |
| Python | 3.11.15 |
| ffmpeg | **not installed** |
| nvidia drivers | **not installed** |

### What this means for milestone 1

The original spike plan compared prove time on a single L4/T4. That number is
unreachable here. I'm proceeding with **CPU-only metrics** and flagging the GPU
column as "deferred to a GPU-equipped runner." The relative cycle counts and
proof sizes between RISC Zero and SP1 still inform the decision; absolute
prove-time targets carry over into milestone 3.

### What this means for milestone 3

ffmpeg is needed to generate the H.264 test corpus. I'll either install it
or, if that hits a blocker, hand-craft a small corpus by linking against
`x264` from source. Logged as a deferred install.

---

## 2026-05-02 — Milestone 1 blocked on sandbox network policy

Tried both zkVMs:

- **`rzup install`** — `rustls` rejects the proxy's TLS-intercepted cert
  for `api.github.com` with `InvalidCertificate(UnknownIssuer)`. The
  Anthropic egress CA is in `/etc/ssl/certs/`, but the rzup binary
  doesn't honor `SSL_CERT_FILE` (it's compiled with rustls, no
  rustls-platform-verifier).
- **`sp1up`** — uses curl, which trusts the proxy CA fine, but
  `api.github.com/repos/succinctlabs/sp1/releases/latest` returns
  HTTP 403 with `API rate limit exceeded for <egress IP>` because
  many sandboxes share that IP. No `GITHUB_TOKEN` is available.

Neither blocker is fixable from inside the sandbox. The original spike
plan (CPU + GPU benchmarks of SHA-256 preimage proofs through both
zkVMs, then pick) needs an environment with either a GitHub token, a
non-rate-limited egress IP, or an alternative cert-trust path for
rustls binaries.

**Decision:** milestone 1 is **deferred** to a runner with unrestricted
GitHub releases access. The spike scaffold under `spike/` is ready;
day-1 work resumes there as soon as that environment exists.

Pivoting to milestone 2 / 3 **native** work, which:

- doesn't depend on a zkVM toolchain at all,
- is a hard prerequisite for milestone 3 (the H.264 decoder must work
  natively before it goes in-circuit), and
- is the bulk of the engineering risk in the project.

Installed ffmpeg (`apt install ffmpeg`) so the H.264 test corpus can be
generated locally.

---

## 2026-05-02 — Milestone 2 native end-to-end working

Built and tested all four native crates plus the CLI. Total: **34 unit
+ integration tests, all passing.**

### Crates

| Crate | Tests | Notes |
|---|---|---|
| `crates/comparator` | 9 | Integer-only SSE / MSE / PSNR with whole-dB threshold table covering 0–80 dB. `no_std`. |
| `crates/manifest` | 10 | Ed25519 SignedManifest + SHA-256 Merkle tree (Bitcoin-style odd duplication). JSON round-trip; tamper detection in body and signature. |
| `crates/toy-codec` | 9 | BlockQuant: 8x8 Walsh–Hadamard, uniform scalar quant. `no_std`. **QP=1 round-trips losslessly** on constants and gradients. |
| `crates/m2-statement` | 6 (integration) | Full milestone-2 statement: signature → Merkle → decode → PSNR. Happy paths at QP=1 and QP=4; four tampering scenarios all fail closed. |

### CLI

`bin/toy-encode` — `encode` subcommand produces `compressed.bin` +
`manifest.json` + dev signing key from a raw YUV input. `verify`
subcommand runs the milestone-2 statement and reports pass/fail. Smoke
test on a 64x32 YUV gradient:

```
$ toy-encode encode --input orig.yuv --width 64 --height 32 --qp 4 ...
encoded: 6160 bytes (64x32, qp=4)
$ toy-encode verify --compressed comp.bin ... --tolerance-db 36
OK: derivation proof valid (signed by 697fda7f...)

$ # Tamper the QP byte then re-verify:
$ toy-encode verify --compressed comp-tampered.bin ...
Error: verification failed: PsnrBelowFloor { plane: "Y", floor_db: 36 }
```

### Issues encountered

- `thiserror` v1 doesn't compile under `#![no_std]`. Hand-rolled
  `Display` + `core::error::Error` impl in `toy-codec` (Rust 1.81+).
- Initial PSNR threshold logic was algebraically wrong for non-multiple-
  of-10 dB floors. Replaced with a precomputed table of
  `round(10^(db/10) * 1e9)` indexed by whole-dB integer. Comparator now
  passes a "PSNR is exactly 60.1 dB at sse=1, n=16, peak=255" boundary
  test.
- First tampering test was too lenient — flipping one coefficient byte
  on a 2 KB plane only nudged PSNR ~30 dB, still above the 36 dB floor.
  Switched the tamper to the QP byte (affects every dequantization).
- First high-QP test used a checkerboard — turns out at QP=32 PSNR was
  still ~50+ dB. Switched to deterministic-noise content + QP=64; now
  reliably drops below 50 dB and the comparator rejects.

### Repo layout after milestone 2

```
crates/
  comparator/         # PSNR / MSE / SSE primitives (no_std)
  manifest/           # SignedManifest + Merkle tree
  toy-codec/          # BlockQuant encoder + decoder (no_std)
  m2-statement/       # the milestone-2 verification statement, native
bin/
  toy-encode/         # CLI: encode + verify
```

---

## 2026-05-02 — Milestone 3 starting: corpus + bitreader + NAL framer

### H.264 test corpus

`test-vectors/gen.sh` produces five baseline-profile I-frame-only
fixtures via `ffmpeg -c:v libx264 -profile:v baseline -x264-params
"keyint=1:bframes=0:cabac=0:no-deblock=1:..."`:

| Fixture | Size | .h264 | .dec.yuv |
|---|---|---|---|
| `solid_16x16` | 16x16 | 652 B | 384 B |
| `diag_16x16` | 16x16 | 773 B | 384 B |
| `checker_32x16` | 32x16 | 848 B | 768 B |
| `grad_32x32` | 32x32 | 691 B | 1536 B |
| `smooth_64x64` | 64x64 | 2226 B | 6144 B |

For each fixture we keep both the H.264 bitstream and the
ffmpeg-decoded YUV. Our decoder must produce byte-identical output to
the latter — note that this is *not* the original raw input, since the
encoder is lossy.

ffprobe confirms each fixture is a single Constrained Baseline access
unit, exactly what milestone 3's scope demands.

### crates/h264-decoder skeleton

Bottom-up, each module independently testable:

| Module | Tests | Status |
|---|---|---|
| `bitreader` | 11 | done — Exp-Golomb tested against spec §9.1 Table 9-1 |
| `nal` | 8 | done — NAL framer + emulation-prevention strip |
| `corpus_nalu` (integration) | 2 | done — every fixture parses to the expected NAL types |
| `slice` (header) | — | next |
| `cavlc` | — | pending |
| `transform` + `quant` | — | pending |
| `intra` | — | pending |
| `mb` + `frame` | — | pending |

21 tests pass. `no_std` discipline maintained (uses
`extern crate alloc` and `core::error::Error`).

### Issues encountered

- `vec!` macro isn't auto-imported under `#![no_std]` even with
  `extern crate alloc` — tests need `use alloc::vec`.
- `Nalu` would have to derive `PartialEq` to support `assert_eq!` on a
  `Result<Vec<Nalu>, _>` — replaced with `matches!` instead.

---

## Status by milestone

| Milestone | Status |
|---|---|
| 1 — zkVM spike | **deferred** (sandbox network blocks rzup / sp1up) |
| 2 — Toy transform (native) | **complete** — 34 tests, CLI smoke-tested |
| 3 — H.264 I-frame decoder (native) | in progress — corpus + bitreader + NAL (21 tests) |
| 4 — P-frames + audio + aggregation | not started |
| 5 — Hardening | not started |
