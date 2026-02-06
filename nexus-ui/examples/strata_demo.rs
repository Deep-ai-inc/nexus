//! Strata Demo - Test the Strata shell integration.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`

fn main() -> Result<(), strata::shell::Error> {
    strata::demo::run()
}
