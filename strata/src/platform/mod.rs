//! Platform-specific functionality.
//!
//! Provides native OS integration that Iced doesn't expose directly,
//! such as initiating outbound drag operations and Quick Look previews.

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::{start_drag, preview_file, preview_file_with_rect, close_quicklook, preview_file_with_local_rect, install_reopen_handler, take_reopen_receiver, setup_menu_bar};

#[cfg(not(target_os = "macos"))]
pub fn start_drag(_source: &crate::app::DragSource) -> Result<(), String> {
    Err("Outbound drag not supported on this platform".into())
}

#[cfg(not(target_os = "macos"))]
pub fn preview_file(_path: &std::path::Path) -> Result<(), String> {
    Err("Quick Look not supported on this platform".into())
}

#[cfg(not(target_os = "macos"))]
pub fn preview_file_with_rect(_path: &std::path::Path, _rect: crate::primitives::Rect) -> Result<(), String> {
    Err("Quick Look not supported on this platform".into())
}

#[cfg(not(target_os = "macos"))]
pub fn close_quicklook() {}

#[cfg(not(target_os = "macos"))]
pub fn preview_file_with_local_rect(_path: &std::path::Path, _rect: crate::primitives::Rect) -> Result<(), String> {
    Err("Quick Look not supported on this platform".into())
}
