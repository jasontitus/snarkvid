// Build script for the RISC Zero methods crate.
// Compiles the guest program(s) into RISC-V ELF binaries and generates
// the image ID constants that the host crate uses for proof verification.

fn main() {
    risc0_build::embed_methods();
}
