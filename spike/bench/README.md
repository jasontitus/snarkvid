# bench — milestone 1 measurement harness

Runs both zkVM sides on the shared fixtures and produces a head-to-head
report.

## Files

- `run.sh` — orchestrates: `cargo run --release` each side's `bench`
  subcommand, dump per-side JSON, then call `compare.py`.
- `compare.py` — combines two sides' JSON into either a single
  comparison object (default) or the `DECISION.md` markdown template.

## Per-side JSON schema (produced by each host's `bench` subcommand)

```json
{
  "system": "risc0",
  "toolchain": "1.x.y",
  "gpu": "NVIDIA L4" ,
  "verifier_wasm_gz_bytes": 1234567,
  "verify_browser_ms": 42,
  "rows": [
    {
      "size_label": "1k",
      "size_bytes": 1024,
      "cycles": 12345,
      "prove_ms": 1234,
      "verify_native_ms": 5,
      "proof_bytes": 200000,
      "peak_rss_bytes": 4000000000
    }
  ]
}
```

The browser verify time and WASM bundle size are filled in from the
`web/` harness, not from the host CLI.
