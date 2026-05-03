use sp1_build::build_program_with_args;

fn main() {
    // Build the SP1 program (guest) using the SP1 toolchain.
    // This compiles the RISC-V ELF that the zkVM executes.
    build_program_with_args("../program", Default::default());
}
