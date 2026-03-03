//! GPU rendering for Strata.
//!
//! Unified ubershader pipeline for GPU-accelerated 2D rendering.
//! Uses the "white pixel" trick for branchless rendering of glyphs and solid quads.

mod glyph_atlas;
mod pipeline;

#[cfg(target_os = "macos")]
mod metal_pipeline;
#[cfg(not(target_os = "macos"))]
mod wgpu_pipeline;

pub use glyph_atlas::{GlyphAtlas, SizeMetrics, metrics_for_size};
pub use pipeline::{GpuInstance, ImageHandle, ImageStore, LineStyle, PendingImage, StrataPipeline, SELECTION_COLOR, GRID_SELECTION_BG, GRID_SELECTION_FG, is_box_drawing, is_block_element, is_custom_drawn};

#[cfg(target_os = "macos")]
pub use metal_pipeline::MetalRenderer;
#[cfg(not(target_os = "macos"))]
pub use wgpu_pipeline::WgpuRenderer;
