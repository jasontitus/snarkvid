// Build script for the RISC Zero methods crate.
// Compiles the guest program(s) into RISC-V ELF binaries and generates
// the image ID constants that the host crate uses for proof verification.
//
// Without the "risc0" feature, this is a no-op (the methods crate
// provides stub constants instead).

#[cfg(feature = "risc0")]
fn main() {
    risc0_build::embed_methods();
}

#[cfg(not(feature = "risc0"))]
fn main() {}
