//! GPU rendering for Strata.
//!
//! Unified ubershader pipeline for GPU-accelerated 2D rendering.
//! Uses the "white pixel" trick for branchless rendering of glyphs and solid quads.

mod glyph_atlas;
mod pipeline;

pub use glyph_atlas::GlyphAtlas;
pub use pipeline::{GpuInstance, StrataPipeline, SELECTION_COLOR};
