// Cross-compile the SP1 guest only when the host is built with
// --features build-guest. Without the feature, the host compiles
// cleanly even when sp1up isn't installed (sandbox path).

fn main() {
    if std::env::var_os("CARGO_FEATURE_BUILD_GUEST").is_some() {
        sp1_build::build_program_with_args("../guest", Default::default());
    }
}
