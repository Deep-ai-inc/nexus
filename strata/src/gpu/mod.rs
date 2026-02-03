//! GPU rendering for Strata.
//!
//! Unified ubershader pipeline for GPU-accelerated 2D rendering.
//! Uses the "white pixel" trick for branchless rendering of glyphs and solid quads.

mod glyph_atlas;
mod pipeline;

pub use glyph_atlas::{GlyphAtlas, SizeMetrics, metrics_for_size};
pub use pipeline::{GpuInstance, ImageHandle, ImageStore, LineStyle, PendingImage, StrataPipeline, SELECTION_COLOR, is_box_drawing, is_block_element, is_custom_drawn};
