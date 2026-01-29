//! Text Engine - cosmic-text integration with caching.
//!
//! Provides text shaping and layout using cosmic-text, with an LRU cache
//! to avoid re-shaping unchanged text each frame.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use lru::LruCache;

use crate::strata::primitives::Color;

/// Global font system (expensive to create, shared across engine instances).
static FONT_SYSTEM: OnceLock<Mutex<FontSystem>> = OnceLock::new();

fn get_font_system() -> &'static Mutex<FontSystem> {
    FONT_SYSTEM.get_or_init(|| {
        Mutex::new(FontSystem::new())
    })
}

/// Text attributes for shaping.
#[derive(Debug, Clone, PartialEq)]
pub struct TextAttrs {
    /// Font size in pixels.
    pub font_size: f32,
    /// Line height in pixels.
    pub line_height: f32,
    /// Font family.
    pub family: FontFamily,
    /// Text color.
    pub color: Color,
}

impl Default for TextAttrs {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            line_height: 20.0,
            family: FontFamily::Monospace,
            color: Color::WHITE,
        }
    }
}

impl TextAttrs {
    /// Create a hash for cache lookup.
    fn cache_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.font_size.to_bits().hash(&mut hasher);
        self.line_height.to_bits().hash(&mut hasher);
        std::mem::discriminant(&self.family).hash(&mut hasher);
        hasher.finish()
    }
}

/// Font family specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FontFamily {
    Monospace,
    SansSerif,
    Serif,
    Named(String),
}

impl FontFamily {
    fn to_cosmic(&self) -> Family<'_> {
        match self {
            FontFamily::Monospace => Family::Monospace,
            FontFamily::SansSerif => Family::SansSerif,
            FontFamily::Serif => Family::Serif,
            FontFamily::Named(name) => Family::Name(name),
        }
    }
}

/// Result of text shaping - contains everything needed for rendering and hit-testing.
#[derive(Debug, Clone)]
pub struct ShapedText {
    /// The original text.
    pub text: Cow<'static, str>,

    /// Character X positions relative to text origin.
    /// char_positions[i] = x offset of character i's left edge.
    pub char_positions: Vec<f32>,

    /// Width of each character.
    pub char_widths: Vec<f32>,

    /// Indices where lines break.
    pub line_breaks: Vec<usize>,

    /// Total width of the shaped text.
    pub width: f32,

    /// Total height of the shaped text.
    pub height: f32,

    /// Line height used.
    pub line_height: f32,

    /// Number of lines.
    pub line_count: usize,

    /// Text color.
    pub color: Color,
}

/// Cache key for shaped text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    text_hash: u64,
    attrs_hash: u64,
}

impl CacheKey {
    fn new(text: &str, attrs: &TextAttrs) -> Self {
        let mut text_hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut text_hasher);

        Self {
            text_hash: text_hasher.finish(),
            attrs_hash: attrs.cache_hash(),
        }
    }
}

/// Text engine with cosmic-text and caching.
pub struct TextEngine {
    /// Swash cache for glyph rasterization.
    swash_cache: SwashCache,

    /// LRU cache for shaped text.
    cache: LruCache<CacheKey, ShapedText>,

    /// Default text attributes.
    default_attrs: TextAttrs,
}

impl TextEngine {
    /// Create a new text engine with default cache size.
    pub fn new() -> Self {
        Self::with_capacity(1024)
    }

    /// Create a new text engine with specified cache capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            swash_cache: SwashCache::new(),
            cache: LruCache::new(std::num::NonZeroUsize::new(capacity).unwrap()),
            default_attrs: TextAttrs::default(),
        }
    }

    /// Set the default text attributes.
    pub fn set_default_attrs(&mut self, attrs: TextAttrs) {
        self.default_attrs = attrs;
    }

    /// Get the default text attributes.
    pub fn default_attrs(&self) -> &TextAttrs {
        &self.default_attrs
    }

    /// Shape text with caching.
    ///
    /// If the text+attrs combination is in the cache, returns the cached result.
    /// Otherwise, shapes the text and caches the result.
    pub fn shape(&mut self, text: impl Into<Cow<'static, str>>, attrs: &TextAttrs) -> ShapedText {
        let text = text.into();
        let key = CacheKey::new(&text, attrs);

        // Check cache first
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }

        // Shape the text
        let shaped = self.shape_uncached(&text, attrs);

        // Cache the result
        self.cache.put(key, shaped.clone());

        shaped
    }

    /// Shape text without caching (always recomputes).
    pub fn shape_uncached(&mut self, text: &str, attrs: &TextAttrs) -> ShapedText {
        let font_system = get_font_system();
        let mut font_system = font_system.lock().unwrap();

        let metrics = Metrics::new(attrs.font_size, attrs.line_height);
        let mut buffer = Buffer::new(&mut font_system, metrics);

        // Set up the buffer
        buffer.set_size(&mut font_system, Some(f32::MAX), Some(f32::MAX));

        let cosmic_attrs = Attrs::new()
            .family(attrs.family.to_cosmic());

        buffer.set_text(&mut font_system, text, cosmic_attrs, Shaping::Advanced);

        // Shape all lines
        buffer.shape_until_scroll(&mut font_system, false);

        // Extract character positions
        let mut char_positions = Vec::new();
        let mut char_widths = Vec::new();
        let mut line_breaks = Vec::new();
        let mut max_width: f32 = 0.0;
        let mut char_index = 0;

        for run in buffer.layout_runs() {
            let line_start = char_index;

            for glyph in run.glyphs.iter() {
                char_positions.push(glyph.x);
                char_widths.push(glyph.w);
                char_index += 1;
            }

            // Track line width
            if let Some(last_glyph) = run.glyphs.last() {
                max_width = max_width.max(last_glyph.x + last_glyph.w);
            }

            // Record line break if this isn't the first line
            if !line_breaks.is_empty() || line_start > 0 {
                if char_index > line_start {
                    line_breaks.push(line_start);
                }
            }
        }

        // Handle empty text
        if char_positions.is_empty() && !text.is_empty() {
            // Fallback: estimate positions for each character
            let char_width = attrs.font_size * 0.6; // Rough estimate
            for (i, _) in text.chars().enumerate() {
                char_positions.push(i as f32 * char_width);
                char_widths.push(char_width);
            }
            max_width = text.chars().count() as f32 * char_width;
        }

        let line_count = buffer.lines.len().max(1);
        let height = line_count as f32 * attrs.line_height;

        ShapedText {
            text: Cow::Owned(text.to_string()),
            char_positions,
            char_widths,
            line_breaks,
            width: max_width,
            height,
            line_height: attrs.line_height,
            line_count,
            color: attrs.color,
        }
    }

    /// Invalidate a specific cache entry.
    pub fn invalidate(&mut self, text: &str, attrs: &TextAttrs) {
        let key = CacheKey::new(text, attrs);
        self.cache.pop(&key);
    }

    /// Clear the entire cache.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Get cache statistics.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shape_simple() {
        let mut engine = TextEngine::new();
        let attrs = TextAttrs::default();

        let shaped = engine.shape("Hello", &attrs);

        assert_eq!(shaped.char_positions.len(), 5);
        assert_eq!(shaped.line_count, 1);
        assert!(shaped.width > 0.0);
    }

    #[test]
    fn test_cache_hit() {
        let mut engine = TextEngine::new();
        let attrs = TextAttrs::default();

        // First call - cache miss
        let _ = engine.shape("Hello", &attrs);
        assert_eq!(engine.cache_len(), 1);

        // Second call - cache hit
        let _ = engine.shape("Hello", &attrs);
        assert_eq!(engine.cache_len(), 1); // Still 1, not 2
    }

    #[test]
    fn test_different_attrs_different_cache() {
        let mut engine = TextEngine::new();

        let attrs1 = TextAttrs { font_size: 14.0, ..Default::default() };
        let attrs2 = TextAttrs { font_size: 16.0, ..Default::default() };

        let _ = engine.shape("Hello", &attrs1);
        let _ = engine.shape("Hello", &attrs2);

        // Different attrs = different cache entries
        assert_eq!(engine.cache_len(), 2);
    }
}
