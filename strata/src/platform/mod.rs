//! Platform-specific functionality.
//!
//! Provides native OS integration that Iced doesn't expose directly,
//! such as initiating outbound drag operations.

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::start_drag;

#[cfg(not(target_os = "macos"))]
pub fn start_drag(_source: &crate::app::DragSource) -> Result<(), String> {
    Err("Outbound drag not supported on this platform".into())
}
