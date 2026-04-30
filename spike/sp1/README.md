# SP1 side of the spike

Mirrors the layout of the [SP1 examples](https://github.com/succinctlabs/sp1):

```
sp1/
  Cargo.toml          nested workspace: program + script
  program/            the guest program (sha256 preimage check)
  script/             native CLI: prove + verify + dump bench JSON
```

## Day-1 work

1. Add `sp1-zkvm` (program) and `sp1-sdk` (script) deps; pin versions verified against upstream README.
2. Port the `sha2` example from the SP1 examples repo, widen for `min_size` public input.
3. Wire the script CLI to the same surface as the RISC Zero host:
   ```
   sp1-script prove   --input <fixture> --min-size <N> --out proof.bin --commit-out commit.hex
   sp1-script verify  --proof proof.bin --commit <hex> --min-size <N>
   sp1-script bench   --fixture-dir ../common/bench-fixtures --out bench.json
   ```
4. GPU: turn on the SP1 GPU prover after CPU baseline is recorded.
