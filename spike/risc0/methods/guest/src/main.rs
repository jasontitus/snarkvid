// Guest program for the RISC Zero side of the milestone-1 spike.
//
// Statement (see milestones/01-spike.md §1):
//   Public:  commitment: [u8; 32], min_size: u32
//   Private: data: Vec<u8>
//   Claim:   sha256(data) == commitment ∧ data.len() ≥ min_size
//
// Day 1: port the `sha` example from the risc0 repo, then widen it to:
//   1. read `min_size: u32` from public inputs
//   2. read `data: Vec<u8>` from private inputs
//   3. assert data.len() >= min_size
//   4. compute sha256(data)
//   5. commit the digest (and the min_size, for binding)

#![no_main]
// risc0_zkvm::guest::entry!(main);

fn main() {
    // let min_size: u32 = env::read();
    // let data: Vec<u8> = env::read();
    // assert!(data.len() as u32 >= min_size, "witness shorter than min_size");
    // let digest: [u8; 32] = Sha256::digest(&data).into();
    // env::commit(&(digest, min_size));
    unimplemented!("spike scaffold — see milestones/01-spike.md §9 for day-1 plan")
}
