//! Shell Integration
//!
//! This module provides the bridge between Strata and the underlying
//! window system. Uses a native macOS backend (NSApplication + wgpu).
//!
//! **Important:** This is the ONLY module that interacts with the window system.
//! All other Strata code should use types re-exported from this module.

mod native_backend;
pub mod subscription;

pub use native_backend::{run, run_with_config, ClipBounds, Error};

// Re-export wgpu directly (no longer through iced).
pub use wgpu;

// Result type for main() return using strata's Error type
pub type Result = std::result::Result<(), Error>;
