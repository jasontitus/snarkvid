// Guest program for the RISC Zero side of the milestone-1 spike.
//
// Statement (see milestones/01-spike.md §1):
//   Public:  commitment: [u8; 32], min_size: u32
//   Private: data: Vec<u8>
//   Claim:   sha256(data) == commitment ∧ data.len() ≥ min_size

#![no_main]
risc0_zkvm::guest::entry!(main);

fn main() {
    let min_size: u32 = risc0_zkvm::io::read::<u32>();
    let data: Vec<u8> = risc0_zkvm::io::read::<Vec<u8>>();

    // Assert length constraint
    require(data.len() as u32 >= min_size, "witness shorter than min_size");

    // Compute SHA-256 digest
    let digest: [u8; 32] = sha2::Sha256::digest(&data).into();

    // Commit public outputs: digest then min_size
    risc0_zkvm::io::commit(&digest);
    risc0_zkvm::io::commit(&min_size);
}

/// Panic-friendly assertion for guest code.
fn require(cond: bool, msg: &str) {
    if !cond {
        panic!("{}", msg);
    }
}
