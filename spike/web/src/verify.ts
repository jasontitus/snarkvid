// Loads one of the two zkVM WASM verifiers on demand and runs verification.
//
// Day 1 of the spike: replace the placeholder loader with whichever shape
// each zkVM ships (RISC Zero exposes a wasm-bindgen wrapper; SP1 exposes
// a similar surface via sp1-verifier).
//
// The proof + public inputs come in as raw bytes / hex; this module returns
// only a boolean and a wall-clock measurement so the UI can render them.

export type System = "risc0" | "sp1";

export interface VerifyResult {
  ok: boolean;
  ms: number;
  bundleBytes: number;
}

interface VerifierModule {
  // Each zkVM's actual surface differs; these are the methods we'll need.
  verify(
    proof: Uint8Array,
    commitment: Uint8Array,
    minSize: number,
  ): boolean;
}

const cache = new Map<System, Promise<{ mod: VerifierModule; bundleBytes: number }>>();

async function loadVerifier(system: System) {
  let entry = cache.get(system);
  if (!entry) {
    entry = (async () => {
      const url = `/${system}-verifier.wasm`;
      const res = await fetch(url);
      const buf = await res.arrayBuffer();
      // Day-1 TODO: instantiate via the zkVM's own loader (likely
      // wasm-bindgen-generated init() + binding object). Placeholder
      // below just throws at call time.
      const mod: VerifierModule = {
        verify() {
          throw new Error(
            `spike scaffold — wire ${system} verifier instantiation in src/verify.ts`,
          );
        },
      };
      return { mod, bundleBytes: buf.byteLength };
    })();
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
  const { mod, bundleBytes } = await loadVerifier(system);
  const t0 = performance.now();
  const ok = mod.verify(proof, commitment, minSize);
  return { ok, ms: performance.now() - t0, bundleBytes };
}
