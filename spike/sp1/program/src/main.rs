// SP1 guest program for the milestone-1 spike.
// Same statement as the RISC Zero side; see milestones/01-spike.md §1.
//
// Day 1: port the `sha2` example from the SP1 examples repo and widen
// it for the `min_size` public input.

#![no_main]
// sp1_zkvm::entrypoint!(main);

pub fn main() {
    // let min_size: u32 = sp1_zkvm::io::read();
    // let data: Vec<u8> = sp1_zkvm::io::read_vec();
    // assert!(data.len() as u32 >= min_size, "witness shorter than min_size");
    // let digest: [u8; 32] = Sha256::digest(&data).into();
    // sp1_zkvm::io::commit(&digest);
    // sp1_zkvm::io::commit(&min_size);
    unimplemented!("spike scaffold — see milestones/01-spike.md §9 for day-1 plan")
}
