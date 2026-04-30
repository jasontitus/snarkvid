import { verifyProof, type System } from "./verify";

const $ = (id: string) => document.getElementById(id)!;

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.trim().replace(/^0x/, "");
  if (clean.length % 2 !== 0) {
    throw new Error("commitment hex must have even length");
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.substr(i * 2, 2), 16);
  }
  return out;
}

$("go").addEventListener("click", async () => {
  const result = $("result");
  result.textContent = "verifying...";
  try {
    const system = ($("system") as HTMLSelectElement).value as System;
    const proofFile = ($("proof") as HTMLInputElement).files?.[0];
    if (!proofFile) throw new Error("pick a proof file");
    const proof = new Uint8Array(await proofFile.arrayBuffer());
    const commitment = hexToBytes(($("commit") as HTMLInputElement).value);
    const minSize = parseInt(($("min-size") as HTMLInputElement).value, 10);

    const r = await verifyProof(system, proof, commitment, minSize);
    result.textContent = JSON.stringify(r, null, 2);
  } catch (err) {
    result.textContent = `error: ${(err as Error).message}`;
  }
});
