# web — browser verifier harness

Bare-bones Vite + TS app that loads either zkVM's WASM verifier on
demand and verifies a proof against a public commitment.

The point is to measure two things from milestone 1:

- **WASM bundle size (gzipped).** `npm run size` after `npm run build`.
- **Browser verify time.** Reported in the UI's result JSON.

## Day-1 work

1. Run `npm install`.
2. Wire the actual WASM instantiation for each system in `src/verify.ts`
   (the placeholder there throws on call). RISC Zero and SP1 each ship
   a WASM verifier; consult their docs for the loader shape.
3. Drop the built `.wasm` files into `public/risc0-verifier.wasm` and
   `public/sp1-verifier.wasm`.
4. `npm run dev`, drop a real proof, confirm green check.
