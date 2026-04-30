# common — shared bench fixtures

`bench-fixtures/gen.sh` produces three deterministic random files that both
zkVM sides ingest as the witness for the SHA-256 preimage proof:

- `fixture-1k.bin`   1 KB
- `fixture-1m.bin`   1 MB
- `fixture-10m.bin`  10 MB

Each comes with a `.sha256` companion that is the public commitment for
that fixture. Re-runs are byte-identical across machines (AES-CTR
keystream from a fixed seed), so bench numbers from different runs are
comparable.

The `.bin` and `.sha256` files are git-ignored; regenerate with:

```bash
cd spike/common/bench-fixtures && ./gen.sh
```
