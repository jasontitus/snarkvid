import { defineConfig } from "vite";

export default defineConfig({
  build: {
    target: "es2022",
    // WASM verifier blobs land in public/ and are loaded at runtime, not
    // bundled — keeps the JS bundle tiny and lets us swap zkVMs by file.
    assetsInlineLimit: 0,
  },
});
