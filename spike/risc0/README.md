# RISC Zero side of the spike

Mirrors the layout of the `examples/` programs in the [risc0 repo](https://github.com/risc0/risc0):

```
risc0/
  Cargo.toml          nested workspace: host + methods
  host/               native CLI: prove + verify + dump bench JSON
  methods/            wraps the guest crate; emits ELF + image ID via build.rs
    guest/            the actual guest program (sha256 preimage check)
```

## Day-1 work

1. Add `risc0-zkvm` (host crate) and `risc0-zkvm-platform` (guest) deps with whatever versions are current — verify against upstream README.
2. Port the `sha` guest from the risc0 examples; widen it to take the witness from stdin and accept the `min_size` public input.
3. Wire the host CLI:
   ```
   risc0-host prove   --input <fixture> --min-size <N> --out proof.bin --commit-out commit.hex
   risc0-host verify  --proof proof.bin --commit <hex> --min-size <N>
   risc0-host bench   --fixture-dir ../common/bench-fixtures --out bench.json
   ```
4. GPU: enable the `cuda` feature once CPU baseline is recorded.
