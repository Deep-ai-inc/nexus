//! Shell Integration
//!
//! This module provides the bridge between Strata and the underlying
//! window system. Currently uses iced as the shell.
//!
//! **Important:** This is the ONLY module that imports iced directly.
//! All other Strata code should use types re-exported from this module.

mod iced_adapter;
pub mod subscription;

pub use iced_adapter::{run, run_with_config, Error};

// Re-export wgpu from iced so GPU code doesn't need to import iced directly.
// This ensures version compatibility with iced's shader pipeline.
pub use iced::wgpu;

// Re-export Rectangle for clip bounds in GPU code
pub use iced::Rectangle;

// Re-export time utilities for subscriptions
pub mod time {
    pub use iced::time::every;
}

// Result type for main() return using strata's Error type
pub type Result = std::result::Result<(), Error>;
