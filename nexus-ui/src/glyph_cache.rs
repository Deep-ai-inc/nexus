//! Glyph cache for GPU-accelerated terminal rendering.
//!
//! Uses fontdue for fast cross-platform glyph rasterization.
//! Glyphs are cached lazily and packed into a texture atlas.

use std::collections::HashMap;
use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

/// Embedded font data - loaded once.
static FONT: OnceLock<Font> = OnceLock::new();

/// Get the shared font instance (parsed once).
fn get_font() -> &'static Font {
    FONT.get_or_init(|| {
        let font_bytes = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
        Font::from_bytes(font_bytes as &[u8], FontSettings::default())
            .expect("Failed to load embedded font")
    })
}

/// Get font cell metrics without creating a full cache.
/// Returns (cell_width, cell_height) for the given font size.
/// Uses cached font - safe to call every frame.
pub fn get_cell_metrics(font_size: f32) -> (f32, f32) {
    let font = get_font();
    let metrics = font.metrics('M', font_size);
    let line_metrics = font.horizontal_line_metrics(font_size).unwrap();
    (metrics.advance_width, line_metrics.new_line_size)
}

/// A cached glyph with its metrics (bitmap not stored after atlas copy).
#[derive(Debug, Clone)]
pub struct CachedGlyph {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Horizontal offset from cursor position.
    pub offset_x: i32,
    /// Vertical offset from baseline.
    pub offset_y: i32,
    /// Position in atlas (x, y).
    pub atlas_x: u32,
    pub atlas_y: u32,
}

/// Dirty region tracking for partial texture uploads.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirtyRect {
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
    pub is_dirty: bool,
}

impl DirtyRect {
    /// Expand dirty rect to include a new region.
    pub fn expand(&mut self, x: u32, y: u32, width: u32, height: u32) {
        if !self.is_dirty {
            self.min_x = x;
            self.min_y = y;
            self.max_x = x + width;
            self.max_y = y + height;
            self.is_dirty = true;
        } else {
            self.min_x = self.min_x.min(x);
            self.min_y = self.min_y.min(y);
            self.max_x = self.max_x.max(x + width);
            self.max_y = self.max_y.max(y + height);
        }
    }

    /// Reset after upload.
    pub fn clear(&mut self) {
        self.is_dirty = false;
    }
}

/// Glyph cache that lazily rasterizes and packs glyphs into an atlas.
pub struct GlyphCache {
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
    /// Dirty region for partial uploads.
    dirty_rect: DirtyRect,
    /// Metrics for the font at this size.
    pub cell_width: f32,
    pub cell_height: f32,
    pub ascent: f32,
}

impl GlyphCache {
    /// Create a new glyph cache with the given font size.
    pub fn new(font_size: f32) -> Self {
        let font = get_font();

        // Get metrics for cell sizing
        let metrics = font.metrics('M', font_size);
        let line_metrics = font.horizontal_line_metrics(font_size).unwrap();

        // For monospace, all chars should have same advance
        let cell_width = metrics.advance_width;
        let cell_height = line_metrics.new_line_size;
        let ascent = line_metrics.ascent;

        // Start with a reasonable atlas size
        let atlas_width = 1024;
        let atlas_height = 1024;
        let atlas_data = vec![0u8; (atlas_width * atlas_height * 4) as usize];

        Self {
            font_size,
            glyphs: HashMap::new(),
            atlas_data,
            atlas_width,
            atlas_height,
            pack_x: 1, // Start at 1 to leave padding
            pack_y: 1,
            row_height: 0,
            dirty_rect: DirtyRect::default(),
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

    /// Rasterize a glyph and add it to the cache and atlas.
    fn rasterize_and_cache(&mut self, ch: char) {
        let font = get_font();
        let (metrics, bitmap) = font.rasterize(ch, self.font_size);

        let width = metrics.width as u32;
        let height = metrics.height as u32;

        // Find position in atlas
        let (atlas_x, atlas_y) = match self.pack_glyph(width, height) {
            Some(pos) => pos,
            None => {
                // Atlas overflow - clear and restart
                self.clear_atlas();
                self.pack_glyph(width, height)
                    .expect("Single glyph too large for atlas")
            }
        };

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

        // Track dirty region for partial upload
        self.dirty_rect.expand(atlas_x, atlas_y, width, height);

        let glyph = CachedGlyph {
            width,
            height,
            offset_x: metrics.xmin,
            offset_y: metrics.ymin,
            atlas_x,
            atlas_y,
        };

        self.glyphs.insert(ch, glyph);
    }

    /// Find space in atlas for a glyph. Returns None if atlas is full.
    fn pack_glyph(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let padding = 1;

        // Handle zero-size glyphs (spaces, etc)
        if width == 0 || height == 0 {
            return Some((0, 0));
        }

        // Check if glyph fits in current row
        if self.pack_x + width + padding > self.atlas_width {
            // Move to next row
            self.pack_x = 1;
            self.pack_y += self.row_height + padding;
            self.row_height = 0;
        }

        // Check if we've run out of space
        if self.pack_y + height + padding > self.atlas_height {
            return None;
        }

        let x = self.pack_x;
        let y = self.pack_y;

        self.pack_x += width + padding;
        self.row_height = self.row_height.max(height);

        Some((x, y))
    }

    /// Clear the atlas and glyph cache (called on overflow).
    fn clear_atlas(&mut self) {
        self.glyphs.clear();
        self.atlas_data.fill(0);
        self.pack_x = 1;
        self.pack_y = 1;
        self.row_height = 0;
        // Mark entire atlas as dirty for full re-upload
        self.dirty_rect = DirtyRect {
            min_x: 0,
            min_y: 0,
            max_x: self.atlas_width,
            max_y: self.atlas_height,
            is_dirty: true,
        };
    }

    /// Get the atlas texture data (RGBA).
    pub fn atlas_data(&self) -> &[u8] {
        &self.atlas_data
    }

    /// Get atlas dimensions.
    pub fn atlas_size(&self) -> (u32, u32) {
        (self.atlas_width, self.atlas_height)
    }

    /// Check if atlas has dirty regions needing upload.
    pub fn is_dirty(&self) -> bool {
        self.dirty_rect.is_dirty
    }

    /// Get the dirty rect for partial upload.
    pub fn dirty_rect(&self) -> &DirtyRect {
        &self.dirty_rect
    }

    /// Get the bytes for just the dirty region.
    /// Returns (data_slice, x, y, width, height) or None if not dirty.
    pub fn dirty_region_data(&self) -> Option<DirtyRegionData> {
        if !self.dirty_rect.is_dirty {
            return None;
        }

        let x = self.dirty_rect.min_x;
        let y = self.dirty_rect.min_y;
        let width = self.dirty_rect.max_x - self.dirty_rect.min_x;
        let height = self.dirty_rect.max_y - self.dirty_rect.min_y;

        // For simplicity, we return row-by-row data
        // In practice, we'll upload row by row in the caller
        Some(DirtyRegionData {
            x,
            y,
            width,
            height,
            atlas_width: self.atlas_width,
        })
    }

    /// Mark atlas as uploaded.
    pub fn mark_clean(&mut self) {
        self.dirty_rect.clear();
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

/// Info needed to upload a dirty region.
pub struct DirtyRegionData {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub atlas_width: u32,
}
