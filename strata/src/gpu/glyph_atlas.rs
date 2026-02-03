//! Glyph Atlas for GPU text rendering.
//!
//! Lazily rasterizes glyphs using cosmic-text's SwashCache (font-fallback-aware)
//! and packs them into a texture atlas. Supports multiple font sizes in a single
//! shared atlas. Uses CacheKey from cosmic-text for glyph lookup.

use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics,
    Shaping, SwashCache, SwashContent,
};

/// Maximum atlas size (8K is safe for most GPUs).
const MAX_ATLAS_SIZE: u32 = 8192;

/// Get font metrics for a given size without needing a GlyphAtlas instance.
///
/// Uses cosmic-text shaping for measurement. Suitable for layout
/// calculations outside the render path (ghost previews, text measurement).
pub fn metrics_for_size(font_size: f32) -> SizeMetrics {
    let fs_mutex = crate::text_engine::get_font_system();
    let mut font_system = fs_mutex.lock().unwrap();
    metrics_for_size_with_fs(font_size, &mut font_system)
}

/// Get font metrics for a given size using an already-locked FontSystem.
pub fn metrics_for_size_with_fs(font_size: f32, font_system: &mut FontSystem) -> SizeMetrics {
    let metrics = Metrics::new(font_size, font_size * 1.2);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(font_system, Some(f32::MAX), Some(f32::MAX));
    let attrs = Attrs::new().family(Family::Monospace);
    buffer.set_text(font_system, "M", attrs, Shaping::Advanced);
    buffer.shape_until_scroll(font_system, false);

    let mut cell_width = font_size * 0.6; // fallback
    let mut ascent = font_size * 0.8;
    let mut cell_height = font_size * 1.2;

    for run in buffer.layout_runs() {
        if let Some(g) = run.glyphs.first() {
            cell_width = g.w;
        }
        cell_height = run.line_height;
        ascent = run.line_y; // line_y is the baseline offset from top
    }

    SizeMetrics {
        cell_width,
        cell_height,
        ascent,
    }
}

/// Per-size font metrics.
#[derive(Debug, Clone, Copy)]
pub struct SizeMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub ascent: f32,
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
    /// Whether this glyph contains color data (e.g. emoji bitmaps).
    pub is_color: bool,
}

/// Glyph atlas that lazily rasterizes and packs glyphs at multiple sizes.
///
/// Uses cosmic-text's SwashCache for font-fallback-aware rasterization.
/// All sizes share a single atlas texture.
pub struct GlyphAtlas {
    /// Base font size.
    font_size: f32,
    /// Cache for glyphs keyed by cosmic-text CacheKey.
    glyphs: HashMap<CacheKey, CachedGlyph>,
    /// Per-size font metrics, lazily computed.
    size_metrics: HashMap<u16, SizeMetrics>,
    /// SwashCache for rasterization.
    swash_cache: SwashCache,
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
    /// Generation counter — incremented on every atlas clear/grow.
    /// Used to invalidate shape cache entries that store atlas-dependent UV data.
    generation: u32,
    /// Base font metrics (convenience accessors for the common case).
    pub cell_width: f32,
    pub cell_height: f32,
    pub ascent: f32,
}

impl GlyphAtlas {
    /// Create a new glyph atlas with the given base font size.
    pub fn new(font_size: f32, font_system: &mut FontSystem) -> Self {
        let base_metrics = metrics_for_size_with_fs(font_size, font_system);

        let base_key = size_key(font_size);
        let mut size_metrics = HashMap::new();
        size_metrics.insert(base_key, base_metrics);

        // Start at 1024x1024 to accommodate color emoji bitmaps.
        let atlas_width = 1024;
        let atlas_height = 1024;
        let mut atlas_data = vec![0u8; (atlas_width * atlas_height * 4) as usize];

        // Reserve a 1x1 white pixel at (0,0) for solid quads (selection, backgrounds).
        atlas_data[0] = 255; // R
        atlas_data[1] = 255; // G
        atlas_data[2] = 255; // B
        atlas_data[3] = 255; // A (fully opaque)

        Self {
            font_size,
            glyphs: HashMap::new(),
            swash_cache: SwashCache::new(),
            size_metrics,
            atlas_data,
            atlas_width,
            atlas_height,
            pack_x: 1,  // Start packing after the white pixel
            pack_y: 1,
            row_height: 0,
            dirty_region: Some((0, 0, 1, 1)), // Mark white pixel dirty for initial upload
            resized: false,
            generation: 0,
            cell_width: base_metrics.cell_width,
            cell_height: base_metrics.cell_height,
            ascent: base_metrics.ascent,
        }
    }

    /// Get the current atlas generation. Incremented on every clear/grow.
    pub fn generation(&self) -> u32 {
        self.generation
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
        let center_u = (0.5 / self.atlas_width as f32) * 65535.0;
        let center_v = (0.5 / self.atlas_height as f32) * 65535.0;
        (
            center_u as u16,
            center_v as u16,
            0,
            0,
        )
    }

    /// Get font metrics for a specific size. Lazily computed and cached.
    pub fn metrics_for_size(&mut self, font_size: f32, font_system: &mut FontSystem) -> SizeMetrics {
        let key = size_key(font_size);
        if let Some(&m) = self.size_metrics.get(&key) {
            return m;
        }
        let m = metrics_for_size_with_fs(font_size, font_system);
        self.size_metrics.insert(key, m);
        m
    }

    /// Get or create a cached glyph for the given CacheKey.
    #[inline]
    pub fn get_glyph(&mut self, cache_key: CacheKey, font_system: &mut FontSystem) -> CachedGlyph {
        if let Some(&g) = self.glyphs.get(&cache_key) {
            return g;
        }

        self.rasterize_and_cache(cache_key, font_system);
        // If rasterization failed (no image), return a zero-size glyph
        self.glyphs.get(&cache_key).copied().unwrap_or(CachedGlyph {
            width: 0,
            height: 0,
            offset_x: 0,
            offset_y: 0,
            uv_x: 0,
            uv_y: 0,
            uv_w: 0,
            uv_h: 0,
            is_color: false,
        })
    }

    /// Rasterize a glyph via SwashCache and add it to the atlas.
    fn rasterize_and_cache(&mut self, cache_key: CacheKey, font_system: &mut FontSystem) {
        let image = match self.swash_cache.get_image_uncached(font_system, cache_key) {
            Some(img) => img,
            None => {
                // No image available (e.g., space character) — cache a zero-size glyph
                self.glyphs.insert(cache_key, CachedGlyph {
                    width: 0,
                    height: 0,
                    offset_x: 0,
                    offset_y: 0,
                    uv_x: 0,
                    uv_y: 0,
                    uv_w: 0,
                    uv_h: 0,
                    is_color: false,
                });
                return;
            }
        };

        let width = image.placement.width;
        let height = image.placement.height;
        let is_color = matches!(image.content, SwashContent::Color);

        if width == 0 || height == 0 {
            self.glyphs.insert(cache_key, CachedGlyph {
                width: 0,
                height: 0,
                offset_x: image.placement.left,
                offset_y: image.placement.top,
                uv_x: 0,
                uv_y: 0,
                uv_w: 0,
                uv_h: 0,
                is_color,
            });
            return;
        }

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

        // Copy bitmap to atlas based on content type
        match image.content {
            SwashContent::Mask => {
                // Alpha mask → white + alpha RGBA
                for y in 0..height {
                    for x in 0..width {
                        let src_idx = (y * width + x) as usize;
                        let dst_idx = ((atlas_y + y) * self.atlas_width + atlas_x + x) as usize * 4;

                        let alpha = image.data.get(src_idx).copied().unwrap_or(0);
                        self.atlas_data[dst_idx] = 255;     // R
                        self.atlas_data[dst_idx + 1] = 255; // G
                        self.atlas_data[dst_idx + 2] = 255; // B
                        self.atlas_data[dst_idx + 3] = alpha; // A
                    }
                }
            }
            SwashContent::Color => {
                // Color bitmap (emoji) — copy RGBA directly
                for y in 0..height {
                    for x in 0..width {
                        let src_idx = ((y * width + x) * 4) as usize;
                        let dst_idx = ((atlas_y + y) * self.atlas_width + atlas_x + x) as usize * 4;

                        self.atlas_data[dst_idx] = image.data.get(src_idx).copied().unwrap_or(0);
                        self.atlas_data[dst_idx + 1] = image.data.get(src_idx + 1).copied().unwrap_or(0);
                        self.atlas_data[dst_idx + 2] = image.data.get(src_idx + 2).copied().unwrap_or(0);
                        self.atlas_data[dst_idx + 3] = image.data.get(src_idx + 3).copied().unwrap_or(0);
                    }
                }
            }
            SwashContent::SubpixelMask => {
                // Subpixel mask — average RGB channels to produce alpha, render as white
                for y in 0..height {
                    for x in 0..width {
                        let src_idx = ((y * width + x) * 3) as usize;
                        let dst_idx = ((atlas_y + y) * self.atlas_width + atlas_x + x) as usize * 4;

                        let r = image.data.get(src_idx).copied().unwrap_or(0) as u16;
                        let g = image.data.get(src_idx + 1).copied().unwrap_or(0) as u16;
                        let b = image.data.get(src_idx + 2).copied().unwrap_or(0) as u16;
                        let alpha = ((r + g + b) / 3) as u8;
                        self.atlas_data[dst_idx] = 255;
                        self.atlas_data[dst_idx + 1] = 255;
                        self.atlas_data[dst_idx + 2] = 255;
                        self.atlas_data[dst_idx + 3] = alpha;
                    }
                }
            }
        }

        self.mark_dirty(atlas_x, atlas_y, atlas_x + width, atlas_y + height);

        // Pre-calculate UVs
        let inv_w = 65535.0 / self.atlas_width as f32;
        let inv_h = 65535.0 / self.atlas_height as f32;

        let glyph = CachedGlyph {
            width: width as u16,
            height: height as u16,
            offset_x: image.placement.left,
            offset_y: image.placement.top,
            uv_x: (atlas_x as f32 * inv_w) as u16,
            uv_y: (atlas_y as f32 * inv_h) as u16,
            uv_w: (width as f32 * inv_w) as u16,
            uv_h: (height as f32 * inv_h) as u16,
            is_color,
        };

        self.glyphs.insert(cache_key, glyph);
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
        self.generation = self.generation.wrapping_add(1);
        // Keep size_metrics — they don't depend on atlas state
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

    /// Pre-cache common ASCII characters at the base font size.
    pub fn precache_ascii(&mut self, font_system: &mut FontSystem) {
        let font_size = self.font_size;
        let metrics = Metrics::new(font_size, font_size * 1.2);
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_size(font_system, Some(f32::MAX), Some(f32::MAX));
        let attrs = Attrs::new().family(Family::Monospace);

        // Shape all printable ASCII as one string
        let ascii_str: String = (32u8..=126u8).map(|b| b as char).collect();
        buffer.set_text(font_system, &ascii_str, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(font_system, false);

        // Iterate layout glyphs and cache each one
        let cache_keys: Vec<CacheKey> = buffer.layout_runs()
            .flat_map(|run| run.glyphs.iter().map(|g| g.physical((0., 0.), 1.0).cache_key))
            .collect();

        for cache_key in cache_keys {
            self.get_glyph(cache_key, font_system);
        }
    }
}

/// Quantize a font size to 0.5px granularity to bound cache entries.
#[inline]
fn size_key(font_size: f32) -> u16 {
    (font_size * 2.0).round() as u16
}

