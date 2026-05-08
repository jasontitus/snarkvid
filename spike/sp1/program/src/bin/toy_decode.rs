// SP1 guest for the toy-decode workload (M1b 3-way parity).
//
// Statement:
//   Public:  digest: [u8; 32]   (SHA-256 of decoded YUV bytes)
//            width: u16, height: u16  (echo of header dims for the verifier)
//   Private: bitstream: BqBitstream
//   Claim:   sha256(decode_toy(bitstream)) == digest
//
// Same shape as Jolt's guest-toy-decode and Sonobe's toy_decode_circuit;
// gives a directly comparable per-system number for the M2 codec kernel.

#![no_main]
sp1_zkvm::entrypoint!(main);

use sha2::Digest;
use snarkvid_toy_codec::{decode_toy, BqBitstream, BqHeader};

pub fn main() {
    // Read the bitstream as raw fields (avoids pulling serde into the
    // guest just for one struct).
    let width: u16 = sp1_zkvm::io::read::<u16>();
    let height: u16 = sp1_zkvm::io::read::<u16>();
    let qp: u8 = sp1_zkvm::io::read::<u8>();
    let chroma_format: u8 = sp1_zkvm::io::read::<u8>();
    let coeffs_y: Vec<i16> = sp1_zkvm::io::read::<Vec<i16>>();
    let coeffs_u: Vec<i16> = sp1_zkvm::io::read::<Vec<i16>>();
    let coeffs_v: Vec<i16> = sp1_zkvm::io::read::<Vec<i16>>();

    let bs = BqBitstream {
        header: BqHeader {
            width,
            height,
            qp,
            chroma_format,
        },
        coeffs_y,
        coeffs_u,
        coeffs_v,
    };

    let frame = decode_toy(&bs).expect("decode_toy failed");

    let mut h = sha2::Sha256::new();
    h.update(&frame.y);
    h.update(&frame.u);
    h.update(&frame.v);
    let digest: [u8; 32] = h.finalize().into();

    sp1_zkvm::io::commit(&digest);
    sp1_zkvm::io::commit(&width);
    sp1_zkvm::io::commit(&height);
}
