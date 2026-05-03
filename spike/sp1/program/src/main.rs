// SP1 guest program for the milestone-1 spike.
//
// Statement (see milestones/01-spike.md §1):
//   Public:  commitment: [u8; 32], min_size: u32
//   Private: data: Vec<u8>
//   Claim:   sha256(data) == commitment ∧ data.len() ≥ min_size
//
// The guest reads `min_size` and `data` as private inputs, computes
// sha256(data), asserts the length constraint, then commits the
// (digest, min_size) pair as public output.

#![no_main]
sp1_zkvm::entrypoint!(main);

use sha2::Digest;

pub fn main() {
    // Read private inputs
    let min_size: u32 = sp1_zkvm::io::read::<u32>();
    let data: Vec<u8> = sp1_zkvm::io::read::<Vec<u8>>();

    // Assert length constraint
    assert!(
        data.len() as u32 >= min_size,
        "witness shorter than min_size"
    );

    // Compute SHA-256 digest
    let digest: [u8; 32] = sha2::Sha256::digest(&data).into();

    // Commit public outputs: digest then min_size
    sp1_zkvm::io::commit(&digest);
    sp1_zkvm::io::commit(&min_size);
}
