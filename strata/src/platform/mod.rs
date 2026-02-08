//! Platform-specific functionality.
//!
//! Provides native OS integration that Iced doesn't expose directly,
//! such as initiating outbound drag operations and Quick Look previews.

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::{start_drag, preview_file, preview_file_with_rect, close_quicklook, preview_file_with_local_rect, install_reopen_handler, take_reopen_receiver, setup_menu_bar, show_definition, install_force_click_handler, take_force_click_receiver, setup_force_click_monitor};

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

#[cfg(not(target_os = "macos"))]
pub fn show_definition(_text: &str, _position: crate::primitives::Point, _font_size: f32) -> Result<(), String> {
    Err("Dictionary lookup not supported on this platform".into())
}

#[cfg(not(target_os = "macos"))]
pub fn install_force_click_handler() {}

#[cfg(not(target_os = "macos"))]
pub fn take_force_click_receiver() -> Option<std::sync::mpsc::Receiver<(f32, f32)>> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn setup_force_click_monitor() {}
