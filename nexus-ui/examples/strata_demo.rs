//! Strata Demo - Test the Strata shell integration.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`

fn main() -> Result<(), nexus_ui::strata::shell::Error> {
    nexus_ui::strata::demo::run()
}
