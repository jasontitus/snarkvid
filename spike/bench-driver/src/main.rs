// Bench driver: invokes both zkVM hosts in `bench` mode against the
// shared fixtures and concatenates their JSON output into one
// comparison.json. The actual prove/verify logic lives in each side's
// host crate; this file only orchestrates.
//
// See ../bench/run.sh for the shell-based equivalent used during the
// spike's first week before this driver is fully wired.

fn main() {
    eprintln!("spike scaffold — see milestones/01-spike.md §4 for the metrics this should emit");
    std::process::exit(2);
}
