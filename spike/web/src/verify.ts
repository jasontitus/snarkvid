// Loads one of the two zkVM WASM verifiers on demand and runs verification.
//
// SP1 verifier integration (requires `npm install @sp1/verifier`):
//   The SP1 SDK ships a WASM verifier that verifies SP1 proofs in the
//   browser. After building the program, run:
//     cargo prove build --output-directory ../web/public
//   This produces a verifier .wasm + JS glue. Import it like:
//     import init, { verify } from "./pkg/sp1_verifier";
//
// RISC Zero verifier integration (requires `npm install @risc0/verifier`):
//   The RISC Zero toolchain produces a verifier WASM via:
//     cargo risczero build --verifier
//   Place the .wasm in public/ and use the @risc0/verifier JS wrapper.
//
// Until one of these is wired, the verifier returns a clear "not wired" error.

export type System = "risc0" | "sp1";

export interface VerifyResult {
  ok: boolean;
  ms: number;
  bundleBytes: number;
  error?: string;
}

// ---------------------------------------------------------------------------
// SP1 verifier — wire when @sp1/verifier is installed
// ---------------------------------------------------------------------------
async function loadSp1Verifier(): Promise<{
  verify: (proof: Uint8Array, commitment: Uint8Array, minSize: number) => boolean;
  bundleBytes: number;
}> {
  // When @sp1/verifier is installed:
  //   import init, { verify as sp1Verify } from "../pkg/sp1_verifier";
  //   const wasmUrl = new URL("../pkg/sp1_verifier_bg.wasm", import.meta.url);
  //   const wasmResp = await fetch(wasmUrl);
  //   const wasmBytes = await wasmResp.arrayBuffer();
  //   await init(wasmBytes);
  //   return {
  //     verify(proof, commitment, minSize) {
  //       return sp1Verify(proof, commitment, minSize);
  //     },
  //     bundleBytes: wasmBytes.byteLength,
  //   };

  // Placeholder: try loading the WASM from the public directory
  const wasmUrl = "/sp1-verifier.wasm";
  const resp = await fetch(wasmUrl);
  if (!resp.ok) {
    throw new Error(
      `SP1 verifier WASM not found at ${wasmUrl}. ` +
      `Build it with: cd ../sp1 && cargo prove build --output-directory ../web/public`,
    );
  }
  throw new Error(
    "SP1 verifier WASM found but JS glue not wired. " +
    "Install @sp1/verifier and update src/verify.ts loadSp1Verifier().",
  );
}

// ---------------------------------------------------------------------------
// RISC Zero verifier — wire when @risc0/verifier is installed
// ---------------------------------------------------------------------------
async function loadRisc0Verifier(): Promise<{
  verify: (proof: Uint8Array, commitment: Uint8Array, minSize: number) => boolean;
  bundleBytes: number;
}> {
  const wasmUrl = "/risc0-verifier.wasm";
  const resp = await fetch(wasmUrl);
  if (!resp.ok) {
    throw new Error(
      `RISC Zero verifier WASM not found at ${wasmUrl}. ` +
      `Build it with: cargo risczero build --verifier and place in public/.`,
    );
  }
  throw new Error(
    "RISC Zero verifier WASM found but JS glue not wired. " +
    "Install @risc0/verifier and update src/verify.ts loadRisc0Verifier().",
  );
}

// ---------------------------------------------------------------------------
// Cached loader
// ---------------------------------------------------------------------------
const cache = new Map<System, Promise<{
  verify: (proof: Uint8Array, commitment: Uint8Array, minSize: number) => boolean;
  bundleBytes: number;
}>>();

async function loadVerifier(system: System) {
  let entry = cache.get(system);
  if (!entry) {
    entry = system === "sp1" ? loadSp1Verifier() : loadRisc0Verifier();
    cache.set(system, entry);
  }
  return entry;
}

export async function verifyProof(
  system: System,
  proof: Uint8Array,
  commitment: Uint8Array,
  minSize: number,
): Promise<VerifyResult> {
  try {
    const { verify, bundleBytes } = await loadVerifier(system);
    const t0 = performance.now();
    const ok = verify(proof, commitment, minSize);
    return { ok, ms: performance.now() - t0, bundleBytes };
  } catch (err) {
    return {
      ok: false,
      ms: 0,
      bundleBytes: 0,
      error: (err as Error).message,
    };
  }
}
