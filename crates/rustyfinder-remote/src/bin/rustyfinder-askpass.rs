//! Stand-alone askpass helper, used by the integration tests. In the app
//! the RustyFinder binary plays this role itself.
fn main() {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    std::process::exit(rustyfinder_remote::relay_main(&prompt));
}
