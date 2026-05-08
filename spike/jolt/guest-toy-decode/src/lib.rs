// Toy-decode Jolt guest. Sibling of guest/ — split because Jolt's
// #[jolt::provable] macro generates top-level `main`/`jolt_panic`
// symbols and two provables in one crate collide.

#![cfg_attr(feature = "guest", no_std)]

extern crate alloc;
use alloc::vec::Vec;

use snarkvid_toy_codec::{decode_toy, BqBitstream, BqHeader};

/// Run `decode_toy` on a single 16x16 4:2:0 frame's worth of data
/// (16x16 Y plane = 256 bytes, 8x8 U + 8x8 V = 64 + 64 bytes = 384 total).
/// Output: SHA-256 digest of the decoded YUV bytes (32 bytes, constant).
#[jolt::provable(heap_size = 16777216, max_trace_length = 268435456)]
fn toy_decode_one_block(yuv_bytes: &[u8]) -> [u8; 32] {
    if yuv_bytes.len() != 384 {
        panic!("toy_decode_one_block expects exactly 384 bytes (16x16 4:2:0)");
    }
    let header = BqHeader {
        width: 16,
        height: 16,
        qp: 0,
        chroma_format: 1,
    };
    let coeffs_y: Vec<i16> = yuv_bytes[0..256].iter().map(|&b| b as i16).collect();
    let coeffs_u: Vec<i16> = yuv_bytes[256..320].iter().map(|&b| b as i16).collect();
    let coeffs_v: Vec<i16> = yuv_bytes[320..384].iter().map(|&b| b as i16).collect();
    let bitstream = BqBitstream {
        header,
        coeffs_y,
        coeffs_u,
        coeffs_v,
    };
    let frame = match decode_toy(&bitstream) {
        Ok(f) => f,
        Err(_) => panic!("decode_toy returned an error"),
    };
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(&frame.y);
    h.update(&frame.u);
    h.update(&frame.v);
    let digest: [u8; 32] = h.finalize().into();
    digest
}
