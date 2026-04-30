// SP1 host driver. Exposes the same CLI shape as the RISC Zero host so
// bench/run.sh can call them interchangeably.
//
// CLI:
//   sp1-script prove   --input <fixture> --min-size <N> --out proof.bin --commit-out commit.hex
//   sp1-script verify  --proof proof.bin --commit <hex> --min-size <N>
//   sp1-script bench   --fixture-dir ../common/bench-fixtures --out bench.json

fn main() {
    eprintln!("spike scaffold — implement per milestones/01-spike.md §9");
    std::process::exit(2);
}
