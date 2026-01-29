//! GPU rendering for Strata.
//!
//! Provides GPU-accelerated text rendering using WGPU.

mod glyph_atlas;
mod pipeline;

pub use glyph_atlas::GlyphAtlas;
pub use pipeline::{StrataPipeline, GlyphInstance};
