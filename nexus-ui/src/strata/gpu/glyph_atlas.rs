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
    /// Dirty region for partial GPU upload (pixel coords, exclusive max).
    /// `None` means clean; `Some((min_x, min_y, max_x, max_y))` is the bounding box
    /// of all glyph writes since last upload.
    dirty_region: Option<(u32, u32, u32, u32)>,
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
        let mut atlas_data = vec![0u8; (atlas_width * atlas_height * 4) as usize];

        // Reserve a 1x1 white pixel at (0,0) for solid quads (selection, backgrounds).
        // This enables the "white pixel trick" for branchless ubershader rendering:
        // - Glyphs sample their actual texture region
        // - Solid quads sample this white pixel, so color * 1.0 = color
        atlas_data[0] = 255; // R
        atlas_data[1] = 255; // G
        atlas_data[2] = 255; // B
        atlas_data[3] = 255; // A (fully opaque)

        Self {
            font_size,
            glyphs: HashMap::new(),
            ascii_cache: [None; 128],
            atlas_data,
            atlas_width,
            atlas_height,
            pack_x: 1,  // Start packing after the white pixel
            pack_y: 1,
            row_height: 0,
            dirty_region: Some((0, 0, 1, 1)), // Mark white pixel dirty for initial upload
            resized: false,
            cell_width,
            cell_height,
            ascent,
        }
    }

    /// Get the UV coordinates for the white pixel (used for solid quads).
    ///
    /// Returns (uv_x, uv_y, uv_w, uv_h) in normalized u16 range (0-65535).
    /// All solid quads (selection, backgrounds) should use these UVs.
    ///
    /// IMPORTANT: We sample the CENTER of the white pixel and use ZERO size
    /// so the UV doesn't interpolate across the quad. This ensures all fragments
    /// sample the exact same texel regardless of quad size.
    #[inline]
    pub fn white_pixel_uv(&self) -> (u16, u16, u16, u16) {
        // The white pixel is at (0,0) with size 1x1.
        // To avoid bilinear filtering artifacts, we sample the CENTER of the pixel:
        //   center_u = 0.5 / atlas_width
        //   center_v = 0.5 / atlas_height
        // And use ZERO UV size so the UV doesn't change across the quad.
        let center_u = (0.5 / self.atlas_width as f32) * 65535.0;
        let center_v = (0.5 / self.atlas_height as f32) * 65535.0;
        (
            center_u as u16,  // uv_x: center of white pixel
            center_v as u16,  // uv_y: center of white pixel
            0,                // uv_w: ZERO - don't interpolate across quad
            0,                // uv_h: ZERO - don't interpolate across quad
        )
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

        self.mark_dirty(atlas_x, atlas_y, atlas_x + width, atlas_y + height);

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

    /// Clear glyph cache state (preserves the white pixel at 0,0).
    fn clear_atlas(&mut self) {
        self.glyphs.clear();
        self.ascii_cache = [None; 128];
        self.atlas_data.fill(0);
        // Re-write the white pixel at (0,0)
        self.atlas_data[0] = 255; // R
        self.atlas_data[1] = 255; // G
        self.atlas_data[2] = 255; // B
        self.atlas_data[3] = 255; // A
        self.pack_x = 1;
        self.pack_y = 1;
        self.row_height = 0;
        // Full atlas is dirty after clear (white pixel + all re-rasterized glyphs)
        self.dirty_region = Some((0, 0, self.atlas_width, self.atlas_height));
    }

    /// Get the atlas texture data.
    pub fn atlas_data(&self) -> &[u8] {
        &self.atlas_data
    }

    /// Expand the dirty region to include the given pixel rect.
    fn mark_dirty(&mut self, min_x: u32, min_y: u32, max_x: u32, max_y: u32) {
        self.dirty_region = Some(match self.dirty_region {
            Some((ox, oy, ow, oh)) => (ox.min(min_x), oy.min(min_y), ow.max(max_x), oh.max(max_y)),
            None => (min_x, min_y, max_x, max_y),
        });
    }

    /// Take the dirty region (returns `None` if clean, resets to clean).
    pub fn take_dirty_region(&mut self) -> Option<(u32, u32, u32, u32)> {
        self.dirty_region.take()
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
