//! Glyph cache for GPU-accelerated terminal rendering.
//!
//! Uses fontdue for fast cross-platform glyph rasterization.
//! Glyphs are cached lazily and packed into a texture atlas.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};

/// A cached glyph with its bitmap and metrics.
#[derive(Debug, Clone)]
pub struct CachedGlyph {
    /// Glyph bitmap (grayscale, 1 byte per pixel).
    pub bitmap: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Horizontal offset from cursor position.
    pub offset_x: i32,
    /// Vertical offset from baseline.
    pub offset_y: i32,
    /// Position in atlas (x, y) - set when packed.
    pub atlas_x: u32,
    pub atlas_y: u32,
}

/// Glyph cache that lazily rasterizes and packs glyphs into an atlas.
pub struct GlyphCache {
    font: Font,
    font_size: f32,
    /// Cache of rasterized glyphs by character.
    glyphs: HashMap<char, CachedGlyph>,
    /// Atlas texture data (RGBA).
    atlas_data: Vec<u8>,
    /// Atlas dimensions.
    atlas_width: u32,
    atlas_height: u32,
    /// Current packing position.
    pack_x: u32,
    pack_y: u32,
    /// Height of current row (for packing).
    row_height: u32,
    /// Whether atlas has changed and needs re-upload.
    dirty: bool,
    /// Metrics for the font at this size.
    pub cell_width: f32,
    pub cell_height: f32,
    pub ascent: f32,
}

/// Get font cell metrics without creating a full cache.
/// Returns (cell_width, cell_height) for the given font size.
pub fn get_cell_metrics(font_size: f32) -> (f32, f32) {
    let font_bytes = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
    let font = Font::from_bytes(font_bytes as &[u8], FontSettings::default())
        .expect("Failed to load font");

    let metrics = font.metrics('M', font_size);
    let line_metrics = font.horizontal_line_metrics(font_size).unwrap();

    (metrics.advance_width, line_metrics.new_line_size)
}

impl GlyphCache {
    /// Create a new glyph cache with the given font size.
    pub fn new(font_size: f32) -> Self {
        // Use system monospace font - fontdue needs font bytes
        // For portability, we'll embed a monospace font or use a fallback
        let font_bytes = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");

        let font = Font::from_bytes(font_bytes as &[u8], FontSettings::default())
            .expect("Failed to load font");

        // Get metrics for cell sizing
        let metrics = font.metrics('M', font_size);
        let line_metrics = font.horizontal_line_metrics(font_size).unwrap();

        // For monospace, all chars should have same advance
        let cell_width = metrics.advance_width;
        let cell_height = line_metrics.new_line_size;
        let ascent = line_metrics.ascent;

        // Start with a reasonable atlas size (can grow if needed)
        let atlas_width = 1024;
        let atlas_height = 1024;
        let atlas_data = vec![0u8; (atlas_width * atlas_height * 4) as usize];

        Self {
            font,
            font_size,
            glyphs: HashMap::new(),
            atlas_data,
            atlas_width,
            atlas_height,
            pack_x: 1, // Start at 1 to leave padding
            pack_y: 1,
            row_height: 0,
            dirty: true,
            cell_width,
            cell_height,
            ascent,
        }
    }

    /// Get or create a cached glyph for the given character.
    pub fn get_glyph(&mut self, ch: char) -> &CachedGlyph {
        if !self.glyphs.contains_key(&ch) {
            self.rasterize_and_cache(ch);
        }
        self.glyphs.get(&ch).unwrap()
    }

    /// Check if we have a glyph cached (without rasterizing).
    pub fn has_glyph(&self, ch: char) -> bool {
        self.glyphs.contains_key(&ch)
    }

    /// Rasterize a glyph and add it to the cache and atlas.
    fn rasterize_and_cache(&mut self, ch: char) {
        let (metrics, bitmap) = self.font.rasterize(ch, self.font_size);

        let width = metrics.width as u32;
        let height = metrics.height as u32;

        // Find position in atlas (simple row-based packing)
        let (atlas_x, atlas_y) = self.pack_glyph(width, height);

        // Copy bitmap to atlas (convert grayscale to RGBA)
        for y in 0..height {
            for x in 0..width {
                let src_idx = (y * width + x) as usize;
                let dst_idx = ((atlas_y + y) * self.atlas_width + atlas_x + x) as usize * 4;

                let alpha = bitmap.get(src_idx).copied().unwrap_or(0);
                // White text with alpha from glyph
                self.atlas_data[dst_idx] = 255;     // R
                self.atlas_data[dst_idx + 1] = 255; // G
                self.atlas_data[dst_idx + 2] = 255; // B
                self.atlas_data[dst_idx + 3] = alpha; // A
            }
        }

        let glyph = CachedGlyph {
            bitmap,
            width,
            height,
            offset_x: metrics.xmin,
            offset_y: metrics.ymin,
            atlas_x,
            atlas_y,
        };

        self.glyphs.insert(ch, glyph);
        self.dirty = true;
    }

    /// Find space in atlas for a glyph (simple row-based packing).
    fn pack_glyph(&mut self, width: u32, height: u32) -> (u32, u32) {
        let padding = 1;

        // Check if glyph fits in current row
        if self.pack_x + width + padding > self.atlas_width {
            // Move to next row
            self.pack_x = 1;
            self.pack_y += self.row_height + padding;
            self.row_height = 0;
        }

        // Check if we need to grow atlas (not implemented - just panic for now)
        if self.pack_y + height + padding > self.atlas_height {
            panic!("Atlas overflow - need to implement atlas resizing");
        }

        let x = self.pack_x;
        let y = self.pack_y;

        self.pack_x += width + padding;
        self.row_height = self.row_height.max(height);

        (x, y)
    }

    /// Get UV coordinates for a glyph (normalized 0-1).
    pub fn get_uv(&self, ch: char) -> Option<(f32, f32, f32, f32)> {
        self.glyphs.get(&ch).map(|g| {
            let u = g.atlas_x as f32 / self.atlas_width as f32;
            let v = g.atlas_y as f32 / self.atlas_height as f32;
            let w = g.width as f32 / self.atlas_width as f32;
            let h = g.height as f32 / self.atlas_height as f32;
            (u, v, w, h)
        })
    }

    /// Get the atlas texture data (RGBA).
    pub fn atlas_data(&self) -> &[u8] {
        &self.atlas_data
    }

    /// Get atlas dimensions.
    pub fn atlas_size(&self) -> (u32, u32) {
        (self.atlas_width, self.atlas_height)
    }

    /// Check if atlas needs re-upload to GPU.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark atlas as uploaded.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Pre-cache common ASCII characters.
    pub fn precache_ascii(&mut self) {
        for ch in (32u8..=126u8).map(|b| b as char) {
            if !self.glyphs.contains_key(&ch) {
                self.rasterize_and_cache(ch);
            }
        }
    }
}
