//! Shell Integration
//!
//! This module provides the bridge between Strata and the underlying
//! window system. Uses a native macOS backend (NSApplication + Metal)
//! on macOS and a winit + wgpu backend on Linux.
//!
//! **Important:** This is the ONLY module that interacts with the window system.
//! All other Strata code should use types re-exported from this module.

#[cfg(target_os = "macos")]
mod native_backend;
#[cfg(target_os = "linux")]
mod linux_backend;

mod populate;
pub mod subscription;

#[cfg(target_os = "macos")]
pub use native_backend::{run, run_with_config, ClipBounds, Error};
#[cfg(target_os = "linux")]
pub use linux_backend::{run, run_with_config, ClipBounds, Error};

// Result type for main() return using strata's Error type
pub type Result = std::result::Result<(), Error>;
