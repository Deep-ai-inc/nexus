//! Shell Integration
//!
//! This module provides the bridge between Strata and the underlying
//! window system. Currently uses iced as the shell.
//!
//! **Important:** This is the ONLY module that imports iced directly.
//! All other Strata code should use Strata primitives.

mod iced_adapter;

pub use iced_adapter::{run, run_with_config, Error};
