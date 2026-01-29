//! Glyph Atlas for GPU text rendering.
//!
//! Lazily rasterizes glyphs using fontdue and packs them into a texture atlas.
//! Optimized with O(1) ASCII lookup and pre-calculated UVs.

use std::collections::HashMap;
use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

/// Maximum atlas size (8K is safe for most GPUs).
const MAX_ATLAS_SIZE: u32 = 8192;

/// Embedded font data - loaded once.
static FONT: OnceLock<Font> = OnceLock::new();

/// Get the shared font instance.
fn get_font() -> &'static Font {
    FONT.get_or_init(|| {
        let font_bytes = include_bytes!("../../../fonts/JetBrainsMono-Regular.ttf");
        Font::from_bytes(font_bytes as &[u8], FontSettings::default())
            .expect("Failed to load embedded font")
    })
}

/// A cached glyph with pre-calculated UVs.
#[derive(Debug, Clone, Copy)]
pub struct CachedGlyph {
    pub width: u16,
    pub height: u16,
    pub offset_x: i32,
    pub offset_y: i32,
    /// Pre-calculated UVs (0-65535 normalized range)
    pub uv_x: u16,
    pub uv_y: u16,
    pub uv_w: u16,
    pub uv_h: u16,
}

/// Glyph atlas that lazily rasterizes and packs glyphs.
pub struct GlyphAtlas {
    font_size: f32,
    /// Cache for non-ASCII glyphs.
    glyphs: HashMap<char, CachedGlyph>,
    /// Fast O(1) lookup for ASCII characters.
    ascii_cache: [Option<CachedGlyph>; 128],
    /// Atlas texture data (RGBA).
    atlas_data: Vec<u8>,
    /// Atlas dimensions.
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Current packing position.
    pack_x: u32,
    pack_y: u32,
    row_height: u32,
    /// Flags for GPU sync.
    dirty: bool,
    resized: bool,
    /// Font metrics.
    pub cell_width: f32,
    pub cell_height: f32,
    pub ascent: f32,
}

impl GlyphAtlas {
    /// Create a new glyph atlas with the given font size.
    pub fn new(font_size: f32) -> Self {
        let font = get_font();

        let metrics = font.metrics('M', font_size);
        let line_metrics = font.horizontal_line_metrics(font_size).unwrap();

        let cell_width = metrics.advance_width;
        let cell_height = line_metrics.new_line_size;
        let ascent = line_metrics.ascent;

        let atlas_width = 512;
        let atlas_height = 512;
        let atlas_data = vec![0u8; (atlas_width * atlas_height * 4) as usize];

        Self {
            font_size,
            glyphs: HashMap::new(),
            ascii_cache: [None; 128],
            atlas_data,
            atlas_width,
            atlas_height,
            pack_x: 1,
            pack_y: 1,
            row_height: 0,
            dirty: false,
            resized: false,
            cell_width,
            cell_height,
            ascent,
        }
    }

    /// Get or create a cached glyph for the given character.
    #[inline]
    pub fn get_glyph(&mut self, ch: char) -> CachedGlyph {
        // Fast path: ASCII
        if ch < '\u{80}' {
            let idx = ch as usize;
            if let Some(g) = self.ascii_cache[idx] {
                return g;
            }
        }

        // Slow path: HashMap or rasterize
        if !self.glyphs.contains_key(&ch) {
            self.rasterize_and_cache(ch);
        }
        *self.glyphs.get(&ch).unwrap()
    }

    /// Rasterize a glyph and add it to the atlas.
    fn rasterize_and_cache(&mut self, ch: char) {
        let font = get_font();
        let (metrics, bitmap) = font.rasterize(ch, self.font_size);

        let width = metrics.width as u32;
        let height = metrics.height as u32;

        // Find position in atlas
        let (atlas_x, atlas_y) = match self.pack_glyph(width, height) {
            Some(pos) => pos,
            None => {
                if self.grow_atlas() {
                    self.pack_glyph(width, height)
                        .expect("Grow failed to make space")
                } else {
                    self.clear_atlas();
                    self.pack_glyph(width, height)
                        .expect("Glyph too large for max atlas")
                }
            }
        };

        // Copy bitmap to atlas (grayscale to RGBA white)
        for y in 0..height {
            for x in 0..width {
                let src_idx = (y * width + x) as usize;
                let dst_idx = ((atlas_y + y) * self.atlas_width + atlas_x + x) as usize * 4;

                let alpha = bitmap.get(src_idx).copied().unwrap_or(0);
                self.atlas_data[dst_idx] = 255;     // R
                self.atlas_data[dst_idx + 1] = 255; // G
                self.atlas_data[dst_idx + 2] = 255; // B
                self.atlas_data[dst_idx + 3] = alpha; // A
            }
        }

        self.dirty = true;

        // Pre-calculate UVs
        let inv_w = 65535.0 / self.atlas_width as f32;
        let inv_h = 65535.0 / self.atlas_height as f32;

        let glyph = CachedGlyph {
            width: width as u16,
            height: height as u16,
            offset_x: metrics.xmin,
            offset_y: metrics.ymin,
            uv_x: (atlas_x as f32 * inv_w) as u16,
            uv_y: (atlas_y as f32 * inv_h) as u16,
            uv_w: (width as f32 * inv_w) as u16,
            uv_h: (height as f32 * inv_h) as u16,
        };

        if ch < '\u{80}' {
            self.ascii_cache[ch as usize] = Some(glyph);
        }
        self.glyphs.insert(ch, glyph);
    }

    /// Find space in atlas for a glyph.
    fn pack_glyph(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if width == 0 || height == 0 {
            return Some((0, 0));
        }

        let padding = 1;

        if self.pack_x + width + padding > self.atlas_width {
            self.pack_x = 1;
            self.pack_y += self.row_height + padding;
            self.row_height = 0;
        }

        if self.pack_y + height + padding > self.atlas_height {
            return None;
        }

        let x = self.pack_x;
        let y = self.pack_y;

        self.pack_x += width + padding;
        self.row_height = self.row_height.max(height);

        Some((x, y))
    }

    /// Grow the atlas by doubling its size.
    fn grow_atlas(&mut self) -> bool {
        let new_width = self.atlas_width * 2;
        let new_height = self.atlas_height * 2;

        if new_width > MAX_ATLAS_SIZE || new_height > MAX_ATLAS_SIZE {
            return false;
        }

        self.atlas_data = vec![0u8; (new_width * new_height * 4) as usize];
        self.atlas_width = new_width;
        self.atlas_height = new_height;
        self.clear_atlas();
        self.resized = true;

        true
    }

    /// Clear glyph cache state.
    fn clear_atlas(&mut self) {
        self.glyphs.clear();
        self.ascii_cache = [None; 128];
        self.atlas_data.fill(0);
        self.pack_x = 1;
        self.pack_y = 1;
        self.row_height = 0;
        self.dirty = true;
    }

    /// Get the atlas texture data.
    pub fn atlas_data(&self) -> &[u8] {
        &self.atlas_data
    }

    /// Check if atlas needs upload.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark atlas as uploaded.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Check if atlas was resized.
    pub fn was_resized(&self) -> bool {
        self.resized
    }

    /// Acknowledge resize.
    pub fn ack_resize(&mut self) {
        self.resized = false;
    }

    /// Pre-cache common ASCII characters.
    pub fn precache_ascii(&mut self) {
        for ch in (32u8..=126u8).map(|b| b as char) {
            if self.ascii_cache[ch as usize].is_none() {
                self.rasterize_and_cache(ch);
            }
        }
    }
}
