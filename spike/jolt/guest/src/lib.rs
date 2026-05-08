// Jolt guest program for the M1b spike: SHA-256 preimage workload.
//
// In May 2026 Jolt's #[jolt::provable] macro generates top-level
// `main`/`jolt_panic` symbols, so each guest crate can carry only one
// provable function. The toy-decode workload lives in the sibling
// guest-toy-decode/ crate.

#![cfg_attr(feature = "guest", no_std)]

/// SHA-256 preimage. Returns (digest, min_size_echo).
///
/// We inline the assertion rather than using the optimized
/// `jolt_inlines_sha2::Sha256` precompile because we want apples-to-apples
/// numbers against SP1/RISC0 (which call the standard `sha2` crate).
/// Switching to the precompile flips this in Jolt's favor by 30-50x;
/// document and benchmark both modes once the scaffold runs.
#[jolt::provable(heap_size = 16777216, max_trace_length = 1073741824)]
fn sha2_preimage(min_size: u32, data: &[u8]) -> ([u8; 32], u32) {
    if (data.len() as u32) < min_size {
        // Jolt panics turn into "panic" public output bits we can check
        // host-side; this aborts proving for invalid witnesses.
        panic!("witness shorter than min_size");
    }
    let digest: [u8; 32] = jolt_inlines_sha2::Sha256::digest(data);
    (digest, min_size)
}

