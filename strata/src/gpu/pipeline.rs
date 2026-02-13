//! GPU Pipeline for Strata rendering.
//!
//! Unified ubershader pipeline that renders all 2D content in a single draw call.
//! Uses the "white pixel" trick: a 1x1 white pixel at atlas (0,0) enables solid
//! quads (selection, backgrounds) to render with the same shader as glyphs.
//!
//! # Rendering Modes
//!
//! | Mode | Name    | uv_tl           | uv_br            | corner_radius    |
//! |------|---------|-----------------|------------------|------------------|
//! | 0    | Quad    | Atlas UV TL     | Atlas UV BR      | SDF radius       |
//! | 1    | Line    | (unused)        | (unused)         | Line thickness   |
//! | 2    | Border  | [border_w, 0]   | (unused)         | SDF radius       |
//! | 3    | Shadow  | (unused)        | [blur, 0]        | SDF radius       |
//! | 4    | Image   | Atlas UV TL     | Atlas UV BR      | SDF mask radius  |
//!
//! All modes support per-instance `clip_rect` for SDF-based clipping
//! without breaking the single draw call.
//!
//! # Apple Silicon Optimization
//!
//! Uses `StorageMode::Shared` buffers for zero-copy uploads on unified memory (M1/M2/M3/M4).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, Style,
    SubpixelBin, Weight, fontdb,
};
use lru::LruCache;

/// Number of in-flight frames for triple-buffered dynamic buffers.
const MAX_FRAMES_IN_FLIGHT: usize = 3;

use super::glyph_atlas::GlyphAtlas;
use crate::primitives::{Color, Rect};

/// Opaque handle to a loaded image in the pipeline's image atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageHandle(pub u32);

/// An image queued for GPU upload (decoded RGBA data, no GPU resources yet).
pub struct PendingImage {
    pub handle: ImageHandle,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// CPU-side image store for dynamic loading and unloading.
///
/// Call `load_rgba()` or `load_png()` at any time to get a handle immediately.
/// Decoded pixel data is queued internally; the shell adapter drains pending
/// uploads each frame and pushes them to the GPU atlas during `prepare()`.
///
/// Call `unload()` to release an image from the GPU atlas.
pub struct ImageStore {
    pending: std::sync::Mutex<Vec<PendingImage>>,
    pending_unloads: std::sync::Mutex<Vec<ImageHandle>>,
    next_handle: u32,
}

impl ImageStore {
    /// Create an empty image store.
    pub fn new() -> Self {
        Self {
            pending: std::sync::Mutex::new(Vec::new()),
            pending_unloads: std::sync::Mutex::new(Vec::new()),
            next_handle: 0,
        }
    }

    /// Load raw RGBA pixel data. Returns a handle immediately.
    ///
    /// The actual GPU upload happens on the next frame's prepare pass.
    pub fn load_rgba(&mut self, width: u32, height: u32, data: Vec<u8>) -> ImageHandle {
        assert_eq!(data.len(), (width * height * 4) as usize, "RGBA data size mismatch");
        let handle = ImageHandle(self.next_handle);
        self.next_handle += 1;
        self.pending.get_mut().unwrap().push(PendingImage {
            handle,
            width,
            height,
            data,
        });
        handle
    }

    /// Decode a PNG file and queue it for upload. Returns a handle immediately.
    pub fn load_png(&mut self, path: impl AsRef<std::path::Path>) -> Result<ImageHandle, String> {
        let img = image::open(path.as_ref()).map_err(|e| format!("Failed to load image: {}", e))?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Ok(self.load_rgba(w, h, rgba.into_raw()))
    }

    /// Generate a procedural test image (gradient pattern).
    pub fn load_test_gradient(&mut self, width: u32, height: u32) -> ImageHandle {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let u = x as f32 / width as f32;
                let v = y as f32 / height as f32;
                let r = (u * 100.0 + 50.0) as u8;
                let g = (v * 80.0 + 40.0) as u8;
                let b = ((1.0 - u) * 180.0 + 75.0) as u8;
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        self.load_rgba(width, height, data)
    }

    /// Queue an image for unloading from the GPU atlas.
    ///
    /// The actual GPU-side removal happens on the next frame's prepare pass.
    /// After unloading, the handle becomes invalid and `add_image` will skip it.
    pub fn unload(&self, handle: ImageHandle) {
        self.pending_unloads.lock().unwrap().push(handle);
    }

    /// Drain all pending image uploads (called by the shell adapter).
    ///
    /// Uses `&self` with internal locking so it can be called from contexts
    /// that only have shared access (e.g., view → StrataPrimitive → prepare).
    pub(crate) fn drain_pending(&self) -> Vec<PendingImage> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }

    /// Drain all pending image unloads (called by the shell adapter).
    pub(crate) fn drain_pending_unloads(&self) -> Vec<ImageHandle> {
        std::mem::take(&mut *self.pending_unloads.lock().unwrap())
    }
}

/// Metadata for a loaded image within the image atlas.
#[derive(Debug, Clone)]
struct LoadedImage {
    /// UV region in the image atlas (normalized 0–1).
    uv_tl: [f32; 2],
    uv_br: [f32; 2],
    /// Original pixel dimensions.
    width: u32,
    height: u32,
}

/// Image atlas — packs loaded images into a single RGBA texture using shelf packing.
struct ImageAtlas {
    texture: metal::Texture,
    width: u32,
    height: u32,
    /// Shelf packer state.
    cursor_x: u32,
    cursor_y: u32,
    shelf_height: u32,
    /// Raw RGBA pixel data (kept for atlas regrow/reupload).
    data: Vec<u8>,
    /// Loaded image metadata (`None` = unloaded / slot freed).
    images: Vec<Option<LoadedImage>>,
}

/// Instance for GPU rendering (64 bytes — one cache line).
///
/// Universal primitive for the ubershader. Supports text glyphs, solid quads,
/// rounded rects, lines, borders, shadows, images, and per-instance clipping
/// — all in a single draw call.
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct GpuInstance {
    /// Position (x, y) in pixels.
    /// Mode 0/2/3/4: top-left of quad. Mode 1: line start point (P1).
    pub pos: [f32; 2],             // 8 bytes
    /// Size (width, height) in pixels.
    /// Mode 0/2/3/4: quad dimensions. Mode 1: line end point (P2).
    pub size: [f32; 2],            // 8 bytes
    /// UV top-left (normalized 0-1).
    /// Mode 0/4: atlas UV origin. Mode 2: [border_width, 0]. Others: unused.
    pub uv_tl: [f32; 2],          // 8 bytes
    /// UV bottom-right (normalized 0-1).
    /// Mode 0/4: atlas UV extent. Mode 3: [blur_radius, 0]. Others: unused.
    pub uv_br: [f32; 2],          // 8 bytes
    /// Color as packed RGBA8.
    pub color: u32,                // 4 bytes
    /// Rendering mode (low 8 bits) and sub-flags (upper bits).
    /// Low byte: 0=Quad, 1=Line, 2=Border, 3=Shadow, 4=Image.
    /// For lines, bits 8..15 = line style (0=solid, 1=dashed, 2=dotted).
    pub mode: u32,                 // 4 bytes
    /// SDF corner radius (modes 0/2/3/4) or line thickness (mode 1).
    pub corner_radius: f32,        // 4 bytes
    /// Image texture array layer index (mode 4). Reserved for other modes.
    pub texture_layer: u32,        // 4 bytes
    /// Per-instance clip rectangle (x, y, w, h). w=0 disables clipping.
    /// Enables nested scroll regions without breaking the single draw call.
    pub clip_rect: [f32; 4],       // 16 bytes
}
// Total: 64 bytes (one cache line)

/// Line style for GPU rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineStyle {
    /// Solid line (default).
    #[default]
    Solid = 0,
    /// Dashed line (repeating dash-gap pattern).
    Dashed = 1,
    /// Dotted line (repeating dot-gap pattern).
    Dotted = 2,
}

impl LineStyle {
    /// Encode line style into the mode field for a line instance.
    #[inline]
    fn encode_mode(self) -> u32 {
        1 | ((self as u32) << 8)
    }
}

/// Uniform data for the shader.
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Globals {
    /// Transform matrix (orthographic projection).
    transform: [[f32; 4]; 4],  // 64 bytes
    /// Atlas size for UV normalization.
    atlas_size: [f32; 2],      // 8 bytes
    /// Padding for alignment.
    _padding: [f32; 2],        // 8 bytes
}

/// Default selection highlight color (blue with transparency).
pub const SELECTION_COLOR: Color = Color {
    r: 0.3,
    g: 0.5,
    b: 0.8,
    a: 0.35,
};

/// No-clip sentinel value.
const NO_CLIP: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// A pre-computed shaped glyph with position, atlas UVs, and rendering data.
///
/// Stores everything needed to emit a `GpuInstance` on a cache hit, so the
/// hot path avoids both Vec cloning and per-glyph HashMap lookups.
#[derive(Clone, Copy)]
struct CachedShapedGlyph {
    /// Relative position from text origin.
    x: f32,
    y: f32,
    /// Glyph bitmap size (pixels).
    width: f32,
    height: f32,
    /// Pre-computed atlas UVs (already converted to f32).
    uv_tl: [f32; 2],
    uv_br: [f32; 2],
    /// Rendering mode (0 = mask glyph, 5 = color emoji).
    mode: u32,
}

/// Cached GPU instances for a single terminal grid row.
///
/// Instances are stored with `pos.y` relative to row baseline (0.0).
/// The gather phase adds the absolute Y offset for the current frame.
struct CachedRow {
    /// GPU instances for this row, with pos.y relative (0.0 = row top).
    instances: Vec<GpuInstance>,
    /// Content signature (hash of all runs' text, colors, styles).
    signature: u64,
    /// Atlas generation when this row was cached.
    /// If atlas is repacked, UVs change and the row must be rebuilt.
    atlas_gen: u32,
}

/// GPU pipeline for Strata rendering.
///
/// Uses a unified ubershader that renders all 2D primitives in one draw call.
/// Instances are rendered in buffer order, enabling perfect Z-ordering.
pub struct StrataPipeline {
    pipeline: metal::RenderPipelineState,
    /// Triple-buffered globals uniform (one per in-flight frame).
    globals_buffers: Vec<metal::Buffer>,
    atlas_texture: metal::Texture,
    atlas_sampler: metal::SamplerState,
    /// Image atlas (separate texture from glyph atlas — full RGBA).
    image_atlas: ImageAtlas,
    image_sampler: metal::SamplerState,
    /// Triple-buffered instance vertex buffer (one per in-flight frame).
    instance_buffers: Vec<metal::Buffer>,
    instance_capacity: usize,
    glyph_atlas: GlyphAtlas,
    /// All instances to render, in draw order.
    instances: Vec<GpuInstance>,
    /// Background color.
    background: Color,
    /// Frame index for triple-buffer slot selection (frame_index % 3).
    frame_index: u64,
    /// LRU shape cache: avoids re-shaping unchanged text each frame.
    /// Key is hash(text, font_size_bits), value is (atlas_generation, glyphs).
    /// When atlas generation mismatches, the entry is stale and must be rebuilt.
    shape_cache: LruCache<u64, (u32, Arc<Vec<CachedShapedGlyph>>)>,
    /// Per-frame cache hit/miss counters (for timing diagnostics).
    pub cache_hits: u32,
    pub cache_misses: u32,
    /// Cumulative shaping time for cache misses this frame.
    pub shaping_time: std::time::Duration,
    /// Shape keys that caused cosmic-text panics — skip on future frames.
    poisoned_texts: std::collections::HashSet<u64>,
    /// Reusable cosmic-text buffer — avoids allocation + font resolution per cache miss.
    reusable_buffer: Option<Buffer>,
    /// Per-character glyph cache for monospace grid text.
    /// Populated lazily; avoids cosmic-text Buffer/shaping entirely for known chars.
    char_glyph_cache: CharGlyphCache,
    /// Cached baseline offset for grid text (single font size in practice).
    grid_line_y: Option<(u32, f32)>,
    /// Per-row instance cache for terminal grid content.
    /// Persists across frames; only rows with changed content are rebuilt.
    grid_row_cache: Vec<Option<CachedRow>>,
    /// Identity of the grid being cached (hash of cols + rows + x-origin).
    /// If grid identity changes, the entire row cache is invalidated.
    grid_cache_id: u64,
}

/// Fast per-character glyph lookup for terminal grid text.
///
/// Uses a flat array for ASCII (0-127) × 4 style combos = 512 slots.
/// Falls back to HashMap for non-ASCII (emoji, CJK, etc.).
struct CharGlyphCache {
    /// Font size these entries were cached for.
    font_size_bits: u32,
    /// Flat array: index = char_code * 4 + style_bits (bold=1, italic=2).
    /// `None` = not yet cached for this char+style.
    ascii: Vec<Option<(fontdb::ID, u16, CacheKeyFlags)>>,
    /// Overflow for non-ASCII single-codepoint characters.
    other: HashMap<(char, bool, bool), (fontdb::ID, u16, CacheKeyFlags)>,
    /// Cache for multi-codepoint grapheme clusters (combining marks, ZWJ emoji, etc.).
    /// Key: (grapheme string, bold, italic).
    /// Value: positioned glyphs — (font_id, glyph_id, flags, x_offset, y_offset) per glyph.
    graphemes: HashMap<(String, bool, bool), Vec<(fontdb::ID, u16, CacheKeyFlags, i32, i32)>>,
}

impl CharGlyphCache {
    fn new() -> Self {
        Self {
            font_size_bits: 0,
            ascii: vec![None; 128 * 4],
            other: HashMap::new(),
            graphemes: HashMap::new(),
        }
    }

    /// Invalidate if font size changed.
    #[inline]
    fn ensure_size(&mut self, font_size_bits: u32) {
        if self.font_size_bits != font_size_bits {
            self.font_size_bits = font_size_bits;
            self.ascii.fill(None);
            self.other.clear();
            self.graphemes.clear();
        }
    }

    #[inline]
    fn style_bits(bold: bool, italic: bool) -> usize {
        (bold as usize) | ((italic as usize) << 1)
    }

    #[inline]
    fn get(&self, ch: char, bold: bool, italic: bool) -> Option<(fontdb::ID, u16, CacheKeyFlags)> {
        let code = ch as u32;
        if code < 128 {
            self.ascii[code as usize * 4 + Self::style_bits(bold, italic)]
        } else {
            self.other.get(&(ch, bold, italic)).copied()
        }
    }

    #[inline]
    fn insert(&mut self, ch: char, bold: bool, italic: bool, val: (fontdb::ID, u16, CacheKeyFlags)) {
        let code = ch as u32;
        if code < 128 {
            self.ascii[code as usize * 4 + Self::style_bits(bold, italic)] = Some(val);
        } else {
            self.other.insert((ch, bold, italic), val);
        }
    }

}

impl StrataPipeline {
    /// Compile the Metal shader library. Call once at init, pass to `new()`.
    pub fn compile_library(device: &metal::DeviceRef) -> metal::Library {
        let options = metal::CompileOptions::new();
        device
            .new_library_with_source(include_str!("shaders/glyph.metal"), &options)
            .expect("Failed to compile Metal shader")
    }

    /// Create a new pipeline with a pre-compiled shader library.
    pub fn new(device: &metal::DeviceRef, library: &metal::Library, format: metal::MTLPixelFormat, font_size: f32, font_system: &mut FontSystem) -> Self {
        let mut glyph_atlas = GlyphAtlas::new(font_size, font_system);
        glyph_atlas.precache_ascii(font_system);

        let vs_fn = library
            .get_function("vs_main", None)
            .expect("Missing vs_main");
        let fs_fn = library
            .get_function("fs_main", None)
            .expect("Missing fs_main");

        // Build vertex descriptor (9 attributes matching GpuInstance layout, buffer index 0)
        let vertex_desc = metal::VertexDescriptor::new();
        let layouts = vertex_desc.layouts();
        let layout0 = layouts.object_at(0).unwrap();
        layout0.set_stride(std::mem::size_of::<GpuInstance>() as u64);
        layout0.set_step_function(metal::MTLVertexStepFunction::PerInstance);
        layout0.set_step_rate(1);

        let attrs = vertex_desc.attributes();
        let attr_defs: [(u64, metal::MTLVertexFormat); 9] = [
            (0, metal::MTLVertexFormat::Float2),   // pos
            (8, metal::MTLVertexFormat::Float2),   // size
            (16, metal::MTLVertexFormat::Float2),  // uv_tl
            (24, metal::MTLVertexFormat::Float2),  // uv_br
            (32, metal::MTLVertexFormat::UInt),    // color
            (36, metal::MTLVertexFormat::UInt),    // mode
            (40, metal::MTLVertexFormat::Float),   // corner_radius
            (44, metal::MTLVertexFormat::UInt),    // texture_layer
            (48, metal::MTLVertexFormat::Float4),  // clip_rect
        ];
        for (i, (offset, fmt)) in attr_defs.iter().enumerate() {
            let a = attrs.object_at(i as u64).unwrap();
            a.set_format(*fmt);
            a.set_offset(*offset);
            a.set_buffer_index(0);
        }

        // Build render pipeline descriptor
        let rpd = metal::RenderPipelineDescriptor::new();
        rpd.set_vertex_function(Some(&vs_fn));
        rpd.set_fragment_function(Some(&fs_fn));
        rpd.set_vertex_descriptor(Some(&vertex_desc));

        // Color attachment with alpha blending
        let color_attach = rpd
            .color_attachments()
            .object_at(0)
            .unwrap();
        color_attach.set_pixel_format(format);
        color_attach.set_blending_enabled(true);
        color_attach.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
        color_attach.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
        color_attach.set_source_rgb_blend_factor(metal::MTLBlendFactor::SourceAlpha);
        color_attach.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
        // Alpha blend uses One (not SourceAlpha) to match wgpu's ALPHA_BLENDING.
        // With SourceAlpha the output alpha becomes α² + dst*(1-α), dropping below
        // 1.0 at anti-aliased glyph edges and making the window semi-transparent.
        color_attach.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
        color_attach.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

        let pipeline = device
            .new_render_pipeline_state(&rpd)
            .expect("Failed to create render pipeline state");

        // Triple-buffered globals (uniform) buffers
        let globals_size = std::mem::size_of::<Globals>() as u64;
        let globals_buffers: Vec<metal::Buffer> = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| device.new_buffer(globals_size, metal::MTLResourceOptions::StorageModeShared))
            .collect();

        // Glyph atlas texture
        let (atlas_width, atlas_height) = (glyph_atlas.atlas_width, glyph_atlas.atlas_height);
        let atlas_texture = create_rgba_texture(device, atlas_width, atlas_height);

        // Samplers (linear filtering)
        let sampler_desc = metal::SamplerDescriptor::new();
        sampler_desc.set_mag_filter(metal::MTLSamplerMinMagFilter::Linear);
        sampler_desc.set_min_filter(metal::MTLSamplerMinMagFilter::Linear);
        let atlas_sampler = device.new_sampler(&sampler_desc);
        let image_sampler = device.new_sampler(&sampler_desc);

        // 1×1 white placeholder image atlas
        let placeholder_texture = create_rgba_texture(device, 1, 1);
        let white_pixel: [u8; 4] = [255, 255, 255, 255];
        upload_texture_region(&placeholder_texture, 0, 0, 1, 1, &white_pixel, 4);

        let image_atlas = ImageAtlas {
            texture: placeholder_texture,
            width: 1,
            height: 1,
            cursor_x: 0,
            cursor_y: 0,
            shelf_height: 0,
            data: vec![255u8; 4],
            images: Vec::new(),
        };

        // Triple-buffered instance buffers
        let initial_capacity = 4096;
        let instance_buf_size = (initial_capacity * std::mem::size_of::<GpuInstance>()) as u64;
        let instance_buffers: Vec<metal::Buffer> = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| device.new_buffer(instance_buf_size, metal::MTLResourceOptions::StorageModeShared))
            .collect();

        Self {
            pipeline,
            globals_buffers,
            atlas_texture,
            atlas_sampler,
            image_atlas,
            image_sampler,
            instance_buffers,
            instance_capacity: initial_capacity,
            glyph_atlas,
            instances: Vec::new(),
            background: Color::BLACK,
            frame_index: 0,
            shape_cache: LruCache::new(NonZeroUsize::new(16384).unwrap()),
            cache_hits: 0,
            cache_misses: 0,
            shaping_time: std::time::Duration::ZERO,
            poisoned_texts: std::collections::HashSet::new(),
            reusable_buffer: None,
            char_glyph_cache: CharGlyphCache::new(),
            grid_line_y: None,
            grid_row_cache: Vec::new(),
            grid_cache_id: 0,
        }
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    /// Convert a glyph atlas u16 UV to f32 normalized (0-1).
    #[inline]
    fn uv_to_f32(v: u16) -> f32 {
        v as f32 / 65535.0
    }

    /// Get white pixel UVs as f32 (tl == br == center of white pixel).
    fn white_pixel_uv_f32(&self) -> ([f32; 2], [f32; 2]) {
        let (ux, uy, _, _) = self.glyph_atlas.white_pixel_uv();
        let tl = [Self::uv_to_f32(ux), Self::uv_to_f32(uy)];
        (tl, tl) // Same point for solid color sampling
    }

    /// Set the background color.
    pub fn set_background(&mut self, color: Color) {
        self.background = color;
    }

    /// Clear instances for new frame.
    pub fn clear(&mut self) {
        self.instances.clear();
        self.cache_hits = 0;
        self.cache_misses = 0;
        self.shaping_time = std::time::Duration::ZERO;
    }

    /// Return the current instance count (used to mark a range start).
    #[inline]
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Apply a clip rect to all instances added since `start`.
    #[inline]
    pub fn apply_clip_since(&mut self, start: usize, clip: [f32; 4]) {
        for inst in &mut self.instances[start..] {
            inst.clip_rect = clip;
        }
    }

    // =========================================================================
    // Grid row cache (row-dirty tracking)
    // =========================================================================

    /// Ensure the grid row cache matches the current grid dimensions.
    ///
    /// If the grid identity changes (different cols, rows, or x-origin),
    /// the entire cache is invalidated and resized.
    pub fn ensure_grid_cache(&mut self, cols: u16, num_rows: usize, bounds_x: f32) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&cols, &mut hasher);
        std::hash::Hash::hash(&num_rows, &mut hasher);
        std::hash::Hash::hash(&bounds_x.to_bits(), &mut hasher);
        let id = std::hash::Hasher::finish(&hasher);

        if id != self.grid_cache_id || self.grid_row_cache.len() != num_rows {
            self.grid_cache_id = id;
            self.grid_row_cache.clear();
            self.grid_row_cache.resize_with(num_rows, || None);
        }
    }

    /// Check if a grid row is cached with matching content.
    ///
    /// Returns `None` on cache hit (row will be gathered later — skip building).
    /// Returns `Some(start_index)` on cache miss — the caller should build the
    /// row's instances into `self.instances`, then call `end_grid_row()`.
    ///
    /// On miss, the old `CachedRow`'s Vec is recycled (cleared, not dropped).
    pub fn begin_grid_row(&mut self, row_index: usize, signature: u64) -> Option<usize> {
        let atlas_gen = self.glyph_atlas.generation();

        if let Some(cached) = &self.grid_row_cache[row_index] {
            if cached.signature == signature && cached.atlas_gen == atlas_gen {
                return None; // Cache hit
            }
        }

        // Cache miss — recycle the Vec if it exists
        if let Some(cached) = &mut self.grid_row_cache[row_index] {
            cached.instances.clear();
        }

        Some(self.instances.len())
    }

    /// Finalize a grid row after building its instances.
    ///
    /// Drains `instances[start..]` into the row cache with Y coordinates
    /// made relative (subtract `row_y_used`). The recycled Vec from
    /// `begin_grid_row` is reused to avoid allocation.
    pub fn end_grid_row(&mut self, row_index: usize, signature: u64, start: usize, row_y_used: f32) {
        let atlas_gen = self.glyph_atlas.generation();

        if let Some(cached) = &mut self.grid_row_cache[row_index] {
            // Recycle: Vec was cleared in begin_grid_row.
            // drain() yields owned values — no intermediate Vec allocation.
            cached.instances.extend(self.instances.drain(start..).map(|mut inst| {
                inst.pos[1] -= row_y_used;
                inst
            }));
            cached.signature = signature;
            cached.atlas_gen = atlas_gen;
        } else {
            // First time caching this row
            let instances: Vec<GpuInstance> = self.instances.drain(start..).map(|mut inst| {
                inst.pos[1] -= row_y_used;
                inst
            }).collect();
            self.grid_row_cache[row_index] = Some(CachedRow {
                instances,
                signature,
                atlas_gen,
            });
        }
    }

    /// Gather all cached grid rows into the instance buffer.
    ///
    /// For each row, copies cached instances with the absolute Y offset
    /// (`base_y + row_idx * cell_h`) and optional clip rect applied in a
    /// single pass (fused copy + transform for better cache locality).
    pub fn gather_grid_rows(&mut self, base_y: f32, cell_h: f32, num_rows: usize, clip: Option<[f32; 4]>) {
        for row_idx in 0..num_rows.min(self.grid_row_cache.len()) {
            if let Some(cached) = &self.grid_row_cache[row_idx] {
                if cached.instances.is_empty() {
                    continue;
                }
                let row_y = base_y + row_idx as f32 * cell_h;
                self.instances.reserve(cached.instances.len());
                for src in &cached.instances {
                    let mut inst = *src;
                    inst.pos[1] += row_y;
                    if let Some(c) = clip {
                        inst.clip_rect = c;
                    }
                    self.instances.push(inst);
                }
            }
        }
    }

    /// Invalidate the entire grid row cache (e.g. after atlas resize).
    pub fn invalidate_grid_row_cache(&mut self) {
        for row in &mut self.grid_row_cache {
            *row = None;
        }
    }

    // =========================================================================
    // Mode 0: Quad (text, solid rects, rounded rects, circles)
    // =========================================================================

    /// Add a solid colored rectangle.
    pub fn add_solid_rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: Color) {
        let (uv_tl, uv_br) = self.white_pixel_uv_f32();
        self.instances.push(GpuInstance {
            pos: [x, y],
            size: [width, height],
            uv_tl,
            uv_br,
            color: color.pack(),
            mode: 0,
            corner_radius: 0.0,
            texture_layer: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Add a solid colored rectangle from a Rect.
    pub fn add_solid_rect_from(&mut self, rect: &Rect, color: Color) {
        self.add_solid_rect(rect.x, rect.y, rect.width, rect.height, color);
    }

    /// Add multiple solid rectangles with the same color (for selection).
    pub fn add_solid_rects(&mut self, rects: &[Rect], color: Color) {
        let (uv_tl, uv_br) = self.white_pixel_uv_f32();
        let packed_color = color.pack();
        for rect in rects {
            self.instances.push(GpuInstance {
                pos: [rect.x, rect.y],
                size: [rect.width, rect.height],
                uv_tl,
                uv_br,
                color: packed_color,
                mode: 0,
                corner_radius: 0.0,
                texture_layer: 0,
                clip_rect: NO_CLIP,
            });
        }
    }

    /// Add a rounded rectangle (SDF-based smooth edges).
    pub fn add_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        corner_radius: f32,
        color: Color,
    ) {
        let (uv_tl, uv_br) = self.white_pixel_uv_f32();
        self.instances.push(GpuInstance {
            pos: [x, y],
            size: [width, height],
            uv_tl,
            uv_br,
            color: color.pack(),
            mode: 0,
            corner_radius,
            texture_layer: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Add a circle (a rounded rect where radius = size/2).
    pub fn add_circle(&mut self, center_x: f32, center_y: f32, radius: f32, color: Color) {
        let diameter = radius * 2.0;
        self.add_rounded_rect(
            center_x - radius,
            center_y - radius,
            diameter,
            diameter,
            radius,
            color,
        );
    }

    // =========================================================================
    // Mode 1: Line (solid, dashed, dotted)
    // =========================================================================

    /// Add a solid line segment.
    pub fn add_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: Color) {
        self.add_line_styled(x1, y1, x2, y2, thickness, color, LineStyle::Solid);
    }

    /// Add a styled line segment (solid, dashed, or dotted).
    pub fn add_line_styled(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        color: Color,
        style: LineStyle,
    ) {
        let (uv_tl, uv_br) = self.white_pixel_uv_f32();
        self.instances.push(GpuInstance {
            pos: [x1, y1],
            size: [x2, y2],
            uv_tl,
            uv_br,
            color: color.pack(),
            mode: style.encode_mode(),
            corner_radius: thickness,
            texture_layer: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Add a solid polyline (N-1 line segment instances).
    pub fn add_polyline(&mut self, points: &[[f32; 2]], thickness: f32, color: Color) {
        self.add_polyline_styled(points, thickness, color, LineStyle::Solid);
    }

    /// Add a styled polyline.
    pub fn add_polyline_styled(
        &mut self,
        points: &[[f32; 2]],
        thickness: f32,
        color: Color,
        style: LineStyle,
    ) {
        if points.len() < 2 {
            return;
        }
        let (uv_tl, uv_br) = self.white_pixel_uv_f32();
        let packed_color = color.pack();
        let mode = style.encode_mode();
        for i in 0..points.len() - 1 {
            self.instances.push(GpuInstance {
                pos: points[i],
                size: points[i + 1],
                uv_tl,
                uv_br,
                color: packed_color,
                mode,
                corner_radius: thickness,
                texture_layer: 0,
                clip_rect: NO_CLIP,
            });
        }
    }

    // =========================================================================
    // Mode 2: Border (SDF ring / outline)
    // =========================================================================

    /// Add a border/outline (hollow rounded rect via SDF ring).
    pub fn add_border(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        corner_radius: f32,
        border_width: f32,
        color: Color,
    ) {
        let (_, uv_br) = self.white_pixel_uv_f32();
        self.instances.push(GpuInstance {
            pos: [x, y],
            size: [width, height],
            uv_tl: [border_width, 0.0], // Store border width in uv_tl.x
            uv_br,
            color: color.pack(),
            mode: 2,
            corner_radius,
            texture_layer: 0,
            clip_rect: NO_CLIP,
        });
    }

    // =========================================================================
    // Mode 3: Shadow (soft SDF)
    // =========================================================================

    /// Add a drop shadow (SDF-based Gaussian approximation).
    ///
    /// Draw this BEFORE the content it shadows. The blur_radius controls
    /// how soft/spread the shadow appears.
    pub fn add_shadow(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        corner_radius: f32,
        blur_radius: f32,
        color: Color,
    ) {
        let (uv_tl, _) = self.white_pixel_uv_f32();
        self.instances.push(GpuInstance {
            pos: [x, y],
            size: [width, height],
            uv_tl,
            uv_br: [blur_radius, 0.0], // Store blur radius in uv_br.x
            color: color.pack(),
            mode: 3,
            corner_radius,
            texture_layer: 0,
            clip_rect: NO_CLIP,
        });
    }

    // =========================================================================
    // Mode 4: Image (atlas-based)
    // =========================================================================

    /// Add an image instance.
    pub fn add_image(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        handle: ImageHandle,
        corner_radius: f32,
        tint: Color,
    ) {
        let Some(Some(img)) = self.image_atlas.images.get(handle.0 as usize) else {
            return; // Image not yet uploaded or has been unloaded.
        };
        self.instances.push(GpuInstance {
            pos: [x, y],
            size: [width, height],
            uv_tl: img.uv_tl,
            uv_br: img.uv_br,
            color: tint.pack(),
            mode: 4,
            corner_radius,
            texture_layer: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Load a PNG image and return a handle for rendering.
    pub fn load_image_png(
        &mut self,
        device: &metal::DeviceRef,
        path: &Path,
    ) -> ImageHandle {
        let img = image::open(path)
            .unwrap_or_else(|e| panic!("Failed to load image {}: {}", path.display(), e))
            .to_rgba8();
        let (w, h) = img.dimensions();
        self.load_image_rgba(device, w, h, &img.into_raw())
    }

    /// Load raw RGBA pixel data and return a handle for rendering.
    pub fn load_image_rgba(
        &mut self,
        device: &metal::DeviceRef,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> ImageHandle {
        assert_eq!(data.len(), (width * height * 4) as usize);

        let atlas = &mut self.image_atlas;

        // Check if we need a new shelf row
        if atlas.cursor_x + width > atlas.width {
            atlas.cursor_y += atlas.shelf_height;
            atlas.cursor_x = 0;
            atlas.shelf_height = 0;
        }

        // Grow atlas if needed
        let needed_width = (atlas.cursor_x + width).max(atlas.width);
        let needed_height = atlas.cursor_y + height;
        if needed_width > atlas.width || needed_height > atlas.height {
            let new_width = needed_width.next_power_of_two().max(256);
            let new_height = needed_height.next_power_of_two().max(256);
            self.grow_image_atlas(device, new_width, new_height);
        }

        let atlas = &mut self.image_atlas;

        // Copy image data into the atlas buffer
        let ax = atlas.cursor_x;
        let ay = atlas.cursor_y;
        for row in 0..height {
            let src_start = (row * width * 4) as usize;
            let src_end = src_start + (width * 4) as usize;
            let dst_start = ((ay + row) * atlas.width * 4 + ax * 4) as usize;
            let dst_end = dst_start + (width * 4) as usize;
            atlas.data[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
        }

        // Upload the modified region to GPU (contiguous source data)
        upload_texture_region(&atlas.texture, ax, ay, width, height, data, width * 4);

        // Record UV region
        let uv_tl = [ax as f32 / atlas.width as f32, ay as f32 / atlas.height as f32];
        let uv_br = [
            (ax + width) as f32 / atlas.width as f32,
            (ay + height) as f32 / atlas.height as f32,
        ];

        let handle = ImageHandle(atlas.images.len() as u32);
        atlas.images.push(Some(LoadedImage { uv_tl, uv_br, width, height }));

        // Advance shelf packer
        atlas.cursor_x += width;
        atlas.shelf_height = atlas.shelf_height.max(height);

        handle
    }

    /// Query image dimensions. Returns `None` if the image has been unloaded.
    pub fn image_size(&self, handle: ImageHandle) -> Option<(u32, u32)> {
        self.image_atlas.images.get(handle.0 as usize)
            .and_then(|slot| slot.as_ref())
            .map(|img| (img.width, img.height))
    }

    /// Unload an image from the atlas. The handle becomes invalid and
    /// `add_image` will silently skip it. The atlas space is not reclaimed
    /// (shelf packing doesn't support holes), but the pixel data and metadata
    /// are freed.
    pub fn unload_image(&mut self, handle: ImageHandle) {
        if let Some(slot) = self.image_atlas.images.get_mut(handle.0 as usize) {
            *slot = None;
        }
    }

    /// Grow the image atlas to a new size, preserving existing data.
    fn grow_image_atlas(
        &mut self,
        device: &metal::DeviceRef,
        new_width: u32,
        new_height: u32,
    ) {
        let atlas = &mut self.image_atlas;
        let old_width = atlas.width;
        let old_height = atlas.height;

        // Allocate new data buffer
        let mut new_data = vec![0u8; (new_width * new_height * 4) as usize];

        // Copy old data row by row
        let copy_rows = old_height.min(new_height);
        let copy_cols = old_width.min(new_width);
        for row in 0..copy_rows {
            let src_start = (row * old_width * 4) as usize;
            let src_end = src_start + (copy_cols * 4) as usize;
            let dst_start = (row * new_width * 4) as usize;
            let dst_end = dst_start + (copy_cols * 4) as usize;
            new_data[dst_start..dst_end].copy_from_slice(&atlas.data[src_start..src_end]);
        }

        atlas.data = new_data;
        atlas.width = new_width;
        atlas.height = new_height;

        // Recreate GPU texture and upload entire atlas data
        atlas.texture = create_rgba_texture(device, new_width, new_height);
        upload_texture_region(&atlas.texture, 0, 0, new_width, new_height, &atlas.data, new_width * 4);

        // Recompute UV regions for all loaded images
        for slot in &mut atlas.images {
            let Some(img) = slot.as_mut() else { continue };
            let px_x = img.uv_tl[0] * old_width as f32;
            let px_y = img.uv_tl[1] * old_height as f32;
            img.uv_tl = [px_x / new_width as f32, px_y / new_height as f32];
            img.uv_br = [
                (px_x + img.width as f32) / new_width as f32,
                (px_y + img.height as f32) / new_height as f32,
            ];
        }
    }

    // =========================================================================
    // Box drawing characters (custom geometric rendering)
    // =========================================================================

    /// Draw a single box drawing character as solid rectangles.
    ///
    /// Returns `true` if the character was handled (is a box drawing char).
    /// Box drawing characters are rendered as geometric primitives to ensure
    /// perfect cell-boundary alignment — font glyphs have gaps/misalignment.
    pub fn draw_box_char(&mut self, ch: char, x: f32, y: f32, cell_w: f32, cell_h: f32, color: Color) -> bool {
        // Decode the character into line segments.
        // Each segment is (left, right, up, down) where the value indicates:
        //   0 = no line, 1 = light, 2 = heavy, 3 = double
        let segs = match box_drawing_segments(ch) {
            Some(s) => s,
            None => return false,
        };

        let (uv_tl, uv_br) = self.white_pixel_uv_f32();
        let packed = color.pack();

        let mid_x = x + cell_w * 0.5;
        let mid_y = y + cell_h * 0.5;
        let light = (cell_w * 0.1).max(1.0).round();
        let heavy = (cell_w * 0.2).max(2.0).round();

        let (left, right, up, down) = segs;

        // Helper: emit a solid rect
        let mut emit = |rx: f32, ry: f32, rw: f32, rh: f32| {
            self.instances.push(GpuInstance {
                pos: [rx, ry],
                size: [rw, rh],
                uv_tl,
                uv_br,
                color: packed,
                mode: 0,
                corner_radius: 0.0,
                texture_layer: 0,
                clip_rect: NO_CLIP,
            });
        };

        // Horizontal segments
        let draw_h = |emit: &mut dyn FnMut(f32, f32, f32, f32), style: u8, from_x: f32, to_x: f32| {
            if style == 0 { return; }
            let w = to_x - from_x;
            if style == 3 {
                // Double: two thin lines with gap
                let gap = (light * 2.0).max(2.0);
                let t = light;
                emit(from_x, mid_y - gap * 0.5 - t * 0.5, w, t);
                emit(from_x, mid_y + gap * 0.5 - t * 0.5, w, t);
            } else {
                let t = if style == 2 { heavy } else { light };
                emit(from_x, mid_y - t * 0.5, w, t);
            }
        };

        // Vertical segments
        let draw_v = |emit: &mut dyn FnMut(f32, f32, f32, f32), style: u8, from_y: f32, to_y: f32| {
            if style == 0 { return; }
            let h = to_y - from_y;
            if style == 3 {
                let gap = (light * 2.0).max(2.0);
                let t = light;
                emit(mid_x - gap * 0.5 - t * 0.5, from_y, t, h);
                emit(mid_x + gap * 0.5 - t * 0.5, from_y, t, h);
            } else {
                let t = if style == 2 { heavy } else { light };
                emit(mid_x - t * 0.5, from_y, t, h);
            }
        };

        draw_h(&mut emit, left, x, mid_x);
        draw_h(&mut emit, right, mid_x, x + cell_w);
        draw_v(&mut emit, up, y, mid_y);
        draw_v(&mut emit, down, mid_y, y + cell_h);

        // For single/heavy corners and intersections, fill the center junction
        // to avoid a gap where horizontal and vertical strokes meet.
        // Skip for double lines (style 3) — they have two separate strokes
        // with a deliberate gap that a center fill would bridge.
        let has_h = left > 0 || right > 0;
        let has_v = up > 0 || down > 0;
        let any_double = left == 3 || right == 3 || up == 3 || down == 3;
        if has_h && has_v && !any_double {
            let h_style = left.max(right);
            let v_style = up.max(down);
            let tw = if v_style == 2 { heavy } else { light };
            let th = if h_style == 2 { heavy } else { light };
            emit(mid_x - tw * 0.5, mid_y - th * 0.5, tw, th);
        }

        true
    }

    /// Draw a block element character (U+2580-U+259F) as solid rectangle(s).
    ///
    /// Returns `true` if the character was handled.
    pub fn draw_block_char(&mut self, ch: char, x: f32, y: f32, cell_w: f32, cell_h: f32, color: Color) -> bool {
        // Handle shade characters with alpha adjustment
        match ch {
            '░' => { // LIGHT SHADE — 25% alpha
                let c = Color { a: color.a * 0.25, ..color };
                self.add_solid_rect(x, y, cell_w, cell_h, c);
                return true;
            }
            '▒' => { // MEDIUM SHADE — 50% alpha
                let c = Color { a: color.a * 0.5, ..color };
                self.add_solid_rect(x, y, cell_w, cell_h, c);
                return true;
            }
            '▓' => { // DARK SHADE — 75% alpha
                let c = Color { a: color.a * 0.75, ..color };
                self.add_solid_rect(x, y, cell_w, cell_h, c);
                return true;
            }
            _ => {}
        }

        // Handle multi-quadrant characters
        let hw = cell_w * 0.5;
        let hh = cell_h * 0.5;
        match ch {
            '▙' => { // UL + LL + LR (all except UR)
                self.add_solid_rect(x, y, hw, hh, color);      // UL
                self.add_solid_rect(x, y + hh, cell_w, hh, color); // full bottom
                return true;
            }
            '▚' => { // UL + LR (diagonal)
                self.add_solid_rect(x, y, hw, hh, color);           // UL
                self.add_solid_rect(x + hw, y + hh, hw, hh, color); // LR
                return true;
            }
            '▛' => { // UL + UR + LL (all except LR)
                self.add_solid_rect(x, y, cell_w, hh, color);  // full top
                self.add_solid_rect(x, y + hh, hw, hh, color); // LL
                return true;
            }
            '▜' => { // UL + UR + LR (all except LL)
                self.add_solid_rect(x, y, cell_w, hh, color);       // full top
                self.add_solid_rect(x + hw, y + hh, hw, hh, color); // LR
                return true;
            }
            '▞' => { // UR + LL (diagonal)
                self.add_solid_rect(x + hw, y, hw, hh, color);  // UR
                self.add_solid_rect(x, y + hh, hw, hh, color);  // LL
                return true;
            }
            '▟' => { // UR + LL + LR (all except UL)
                self.add_solid_rect(x + hw, y, hw, hh, color);     // UR
                self.add_solid_rect(x, y + hh, cell_w, hh, color); // full bottom
                return true;
            }
            _ => {}
        }

        // Simple single-rect block elements
        let (rx, ry, rw, rh) = match block_element_rect(ch) {
            Some(r) => r,
            None => return false,
        };

        let px = x + rx * cell_w;
        let py = y + ry * cell_h;
        let pw = rw * cell_w;
        let ph = rh * cell_h;

        self.add_solid_rect(px, py, pw, ph, color);
        true
    }

    // =========================================================================
    // Text rendering
    // =========================================================================

    /// Add a text string to render at a specific font size.
    ///
    /// Uses cosmic-text for shaping (proper Unicode support: CJK, emoji, Arabic,
    /// combining marks, ZWJ sequences). Results are cached in an LRU shape cache
    /// to avoid re-shaping unchanged text each frame.
    pub fn add_text(&mut self, text: &str, x: f32, y: f32, color: Color, font_size: f32, font_system: &mut FontSystem) {
        self.add_text_styled(text, x, y, color, font_size, false, false, font_system);
    }

    /// Add shaped text for terminal grid content.
    ///
    /// Bypasses cosmic-text's Buffer/shaping pipeline entirely. Uses a per-character
    /// glyph cache (flat array for ASCII) that maps `char → CacheKey` directly, then
    /// looks up glyphs in the atlas. Only falls back to cosmic-text shaping for
    /// characters not yet seen. After warmup, every call is the fast path.
    pub fn add_text_grid(&mut self, text: &str, x: f32, y: f32, color: Color, font_size: f32, bold: bool, italic: bool, font_system: &mut FontSystem) {
        use unicode_width::UnicodeWidthChar;

        if text.is_empty() {
            return;
        }

        let packed_color = color.pack();
        let font_size_bits = font_size.to_bits();
        let cell_width = self.glyph_atlas.cell_width;

        // Invalidate char cache if font size changed (e.g. scale factor change)
        self.char_glyph_cache.ensure_size(font_size_bits);

        // Get or compute baseline offset for this font size
        let line_y = match self.grid_line_y {
            Some((bits, ly)) if bits == font_size_bits => ly,
            _ => {
                let metrics = Metrics::new(font_size, font_size * 1.2);
                let mut buffer = Buffer::new(font_system, metrics);
                buffer.set_size(font_system, Some(f32::MAX), Some(f32::MAX));
                buffer.set_text(font_system, "M", Attrs::new().family(Family::Monospace), Shaping::Advanced);
                buffer.shape_until_scroll(font_system, false);
                let ly = buffer.layout_runs().next().map(|r| r.line_y).unwrap_or(font_size * 0.8);
                self.grid_line_y = Some((font_size_bits, ly));
                ly
            }
        };

        // ── Tier A: single-pass fast path ────────────────────────────────
        // Attempt to render each char directly from the per-char cache.
        // Bail on first non-simple char (wide, uncached) and fall through
        // to the general path. Processes each char exactly once (no
        // separate predicate pass) — covers 99%+ of terminal output.
        {
            let fast_start = self.instances.len();
            let mut cursor_x = x;
            let mut fast_ok = true;
            for ch in text.chars() {
                if UnicodeWidthChar::width(ch) != Some(1) {
                    fast_ok = false;
                    break;
                }
                if let Some((font_id, glyph_id, flags)) = self.char_glyph_cache.get(ch, bold, italic) {
                    let cache_key = CacheKey {
                        font_id,
                        glyph_id,
                        font_size_bits,
                        x_bin: SubpixelBin::Zero,
                        y_bin: SubpixelBin::Zero,
                        flags,
                    };
                    let ag = self.glyph_atlas.get_glyph(cache_key, font_system);
                    if ag.width > 0 && ag.height > 0 {
                        let mode = if ag.is_color { 5 } else { 0 };
                        self.instances.push(GpuInstance {
                            pos: [(cursor_x + ag.offset_x as f32).round(), (y + line_y - ag.offset_y as f32).round()],
                            size: [ag.width as f32, ag.height as f32],
                            uv_tl: [Self::uv_to_f32(ag.uv_x), Self::uv_to_f32(ag.uv_y)],
                            uv_br: [Self::uv_to_f32(ag.uv_x + ag.uv_w), Self::uv_to_f32(ag.uv_y + ag.uv_h)],
                            color: packed_color,
                            mode,
                            corner_radius: 0.0,
                            texture_layer: 0,
                            clip_rect: NO_CLIP,
                        });
                    }
                } else {
                    fast_ok = false;
                    break;
                }
                cursor_x += cell_width;
            }
            if fast_ok {
                self.cache_hits += 1;
                return;
            }
            // Bail: undo partial instances from the failed fast path
            self.instances.truncate(fast_start);
        }

        // ── General path: unicode-width–aware, grapheme clusters ─────────
        // Handles wide chars (CJK = 2 columns), zero-width combining marks,
        // and multi-codepoint grapheme clusters (ZWJ emoji, flags, etc.).
        //
        // Grapheme clusters are reconstructed using unicode-width plus
        // special rules for ZWJ continuation and regional indicator pairs.
        self.cache_misses += 1;
        let shape_start = std::time::Instant::now();

        /// Regional Indicator Symbol (U+1F1E6..=U+1F1FF) — pairs form flag emoji.
        #[inline]
        fn is_regional_indicator(ch: char) -> bool {
            ('\u{1F1E6}'..='\u{1F1FF}').contains(&ch)
        }

        /// Emoji Modifier (Fitzpatrick skin tone) U+1F3FB..=U+1F3FF.
        #[inline]
        fn is_skin_tone_modifier(ch: char) -> bool {
            ('\u{1F3FB}'..='\u{1F3FF}').contains(&ch)
        }

        /// Returns true if `next` should attach to the current cluster
        /// (zero-width, skin tone modifier, or second regional indicator after first).
        #[inline]
        fn is_cluster_continuation(next: char) -> bool {
            UnicodeWidthChar::width(next).unwrap_or(0) == 0
                || is_skin_tone_modifier(next)
        }

        let mut cursor_x = x;
        let mut chars = text.chars().peekable();

        while let Some(ch) = chars.next() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);

            // Determine if this char starts a multi-codepoint cluster:
            // 1. Next char is zero-width (combining mark, VS, ZWJ)
            // 2. Next char is a skin tone modifier (Fitzpatrick)
            // 3. This is a regional indicator followed by another (flag pair)
            let next_continues = chars.peek().map_or(false, |&next| {
                is_cluster_continuation(next)
            });
            let is_flag_pair = is_regional_indicator(ch)
                && chars.peek().map_or(false, |&next| is_regional_indicator(next));
            let is_multi = next_continues || is_flag_pair;

            if !is_multi {
                // ── Single-codepoint grapheme (Tier A/B) ─────────────────
                // Use the per-char cache. Shape on miss via cosmic-text.
                if self.char_glyph_cache.get(ch, bold, italic).is_none() {
                    let mut buffer = self.reusable_buffer.take().unwrap_or_else(|| {
                        let metrics = Metrics::new(font_size, font_size * 1.2);
                        let mut buf = Buffer::new(font_system, metrics);
                        buf.set_size(font_system, Some(f32::MAX), Some(f32::MAX));
                        buf
                    });
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        buffer.set_metrics(font_system, Metrics::new(font_size, font_size * 1.2));
                        let mut attrs = Attrs::new().family(Family::Monospace);
                        if bold { attrs = attrs.weight(Weight::BOLD); }
                        if italic { attrs = attrs.style(Style::Italic); }
                        buffer.set_text(font_system, &ch.to_string(), attrs, Shaping::Advanced);
                        buffer.shape_until_scroll(font_system, false);
                        buffer.layout_runs().next().and_then(|run| {
                            run.glyphs.first().map(|g| g.physical((0., 0.), 1.0).cache_key)
                        })
                    }));
                    if let Ok(Some(ck)) = result {
                        self.char_glyph_cache.insert(ch, bold, italic,
                            (ck.font_id, ck.glyph_id, ck.flags));
                    }
                    self.reusable_buffer = Some(buffer);
                }

                // Render from cache
                if let Some((font_id, glyph_id, flags)) = self.char_glyph_cache.get(ch, bold, italic) {
                    let cache_key = CacheKey {
                        font_id, glyph_id, font_size_bits,
                        x_bin: SubpixelBin::Zero, y_bin: SubpixelBin::Zero, flags,
                    };
                    let ag = self.glyph_atlas.get_glyph(cache_key, font_system);
                    if ag.width > 0 && ag.height > 0 {
                        let mode = if ag.is_color { 5 } else { 0 };
                        self.instances.push(GpuInstance {
                            pos: [(cursor_x + ag.offset_x as f32).round(), (y + line_y - ag.offset_y as f32).round()],
                            size: [ag.width as f32, ag.height as f32],
                            uv_tl: [Self::uv_to_f32(ag.uv_x), Self::uv_to_f32(ag.uv_y)],
                            uv_br: [Self::uv_to_f32(ag.uv_x + ag.uv_w), Self::uv_to_f32(ag.uv_y + ag.uv_h)],
                            color: packed_color,
                            mode,
                            corner_radius: 0.0,
                            texture_layer: 0,
                            clip_rect: NO_CLIP,
                        });
                    }
                }
                cursor_x += ch_width as f32 * cell_width;
            } else {
                // ── Multi-codepoint grapheme cluster (Tier C) ────────────
                // Collect the full cluster using these rules:
                //  - Zero-width chars (combining marks, VS, ZWJ) attach to the cluster
                //  - After ZWJ (U+200D), continue collecting the next primary char
                //    and its zero-width followers (handles ZWJ emoji sequences)
                //  - A regional indicator pair forms one cluster (flag emoji)
                let mut grapheme = String::with_capacity(8);
                grapheme.push(ch);

                // For flag pairs: consume the second regional indicator
                if is_flag_pair {
                    if let Some(next) = chars.next() {
                        grapheme.push(next);
                    }
                }

                // Collect continuation characters:
                //  - Zero-width chars (combining marks, VS16, ZWJ)
                //  - Skin tone modifiers (U+1F3FB..=U+1F3FF)
                //  - After ZWJ (U+200D), the next primary char + its continuations
                loop {
                    match chars.peek() {
                        Some(&next) if is_cluster_continuation(next) => {
                            let is_zwj = next == '\u{200D}';
                            grapheme.push(next);
                            chars.next();
                            // After ZWJ, also consume the next primary char
                            // (and loop back to collect its continuations)
                            if is_zwj {
                                if let Some(&primary) = chars.peek() {
                                    grapheme.push(primary);
                                    chars.next();
                                }
                            }
                        }
                        _ => break,
                    }
                }

                // Check grapheme cache
                let cache_key_tuple = (grapheme.clone(), bold, italic);
                if !self.char_glyph_cache.graphemes.contains_key(&cache_key_tuple) {
                    // Shape the full grapheme via cosmic-text to get correct
                    // GSUB substitutions (composed forms, ligatures, ZWJ).
                    let mut buffer = self.reusable_buffer.take().unwrap_or_else(|| {
                        let metrics = Metrics::new(font_size, font_size * 1.2);
                        let mut buf = Buffer::new(font_system, metrics);
                        buf.set_size(font_system, Some(f32::MAX), Some(f32::MAX));
                        buf
                    });
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        buffer.set_metrics(font_system, Metrics::new(font_size, font_size * 1.2));
                        let mut attrs = Attrs::new().family(Family::Monospace);
                        if bold { attrs = attrs.weight(Weight::BOLD); }
                        if italic { attrs = attrs.style(Style::Italic); }
                        buffer.set_text(font_system, &grapheme, attrs, Shaping::Advanced);
                        buffer.shape_until_scroll(font_system, false);
                        let mut glyphs = Vec::new();
                        for run in buffer.layout_runs() {
                            for g in run.glyphs.iter() {
                                let phys = g.physical((0., 0.), 1.0);
                                glyphs.push((
                                    phys.cache_key.font_id,
                                    phys.cache_key.glyph_id,
                                    phys.cache_key.flags,
                                    phys.x,
                                    phys.y,
                                ));
                            }
                        }
                        glyphs
                    }));
                    if let Ok(glyphs) = result {
                        self.char_glyph_cache.graphemes.insert(cache_key_tuple.clone(), glyphs);
                    }
                    self.reusable_buffer = Some(buffer);
                }

                // Render all glyphs in this grapheme cluster
                if let Some(glyphs) = self.char_glyph_cache.graphemes.get(&cache_key_tuple) {
                    for &(font_id, glyph_id, flags, gx, gy) in glyphs {
                        let cache_key = CacheKey {
                            font_id, glyph_id, font_size_bits,
                            x_bin: SubpixelBin::Zero, y_bin: SubpixelBin::Zero, flags,
                        };
                        let ag = self.glyph_atlas.get_glyph(cache_key, font_system);
                        if ag.width > 0 && ag.height > 0 {
                            let mode = if ag.is_color { 5 } else { 0 };
                            self.instances.push(GpuInstance {
                                pos: [(cursor_x + gx as f32 + ag.offset_x as f32).round(),
                                      (y + line_y + gy as f32 - ag.offset_y as f32).round()],
                                size: [ag.width as f32, ag.height as f32],
                                uv_tl: [Self::uv_to_f32(ag.uv_x), Self::uv_to_f32(ag.uv_y)],
                                uv_br: [Self::uv_to_f32(ag.uv_x + ag.uv_w), Self::uv_to_f32(ag.uv_y + ag.uv_h)],
                                color: packed_color,
                                mode,
                                corner_radius: 0.0,
                                texture_layer: 0,
                                clip_rect: NO_CLIP,
                            });
                        }
                    }
                }
                cursor_x += ch_width as f32 * cell_width;
            }
        }

        self.shaping_time += shape_start.elapsed();
    }

    /// Add shaped text with optional bold/italic styling (non-grid text: UI labels, etc).
    pub fn add_text_styled(&mut self, text: &str, x: f32, y: f32, color: Color, font_size: f32, bold: bool, italic: bool, font_system: &mut FontSystem) {
        if text.is_empty() {
            return;
        }

        let packed_color = color.pack();

        // Compute shape cache key from text content + font size + style
        let shape_key = {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            font_size.to_bits().hash(&mut hasher);
            bold.hash(&mut hasher);
            italic.hash(&mut hasher);
            hasher.finish()
        };

        let atlas_gen = self.glyph_atlas.generation();

        // Check shape cache — hit only if atlas generation matches
        if let Some((cached_gen, cached)) = self.shape_cache.get(&shape_key) {
            if *cached_gen == atlas_gen {
                // Fast path: Rc::clone is a pointer bump, no Vec allocation.
                // All UV/size/mode data is pre-baked, no HashMap lookups needed.
                self.cache_hits += 1;
                let glyphs = Arc::clone(cached);
                for sg in glyphs.iter() {
                    self.instances.push(GpuInstance {
                        pos: [(x + sg.x).round(), (y + sg.y).round()],
                        size: [sg.width, sg.height],
                        uv_tl: sg.uv_tl,
                        uv_br: sg.uv_br,
                        color: packed_color,
                        mode: sg.mode,
                        corner_radius: 0.0,
                        texture_layer: 0,
                        clip_rect: NO_CLIP,
                    });
                }
                return;
            }
            // Atlas generation mismatch — fall through to rebuild
        }

        self.cache_misses += 1;

        // Check if this text previously caused a panic (poisoned)
        if self.poisoned_texts.contains(&shape_key) {
            return;
        }

        // Cache miss (or stale) — shape via cosmic-text
        // Reuse a persistent Buffer to avoid allocation + font resolution overhead.
        // Wrap shaping in catch_unwind because cosmic-text can panic on certain
        // glyph cache operations (e.g. arithmetic overflow in glyph_cache.rs).
        // Atlas insertion happens outside the unwind boundary.
        let shape_start = std::time::Instant::now();

        let mut buffer = self.reusable_buffer.take().unwrap_or_else(|| {
            let metrics = Metrics::new(font_size, font_size * 1.2);
            let mut buf = Buffer::new(font_system, metrics);
            buf.set_size(font_system, Some(f32::MAX), Some(f32::MAX));
            buf
        });

        let shaping_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            buffer.set_metrics(font_system, Metrics::new(font_size, font_size * 1.2));
            let mut attrs = Attrs::new().family(Family::Monospace);
            if bold {
                attrs = attrs.weight(Weight::BOLD);
            }
            if italic {
                attrs = attrs.style(Style::Italic);
            }
            buffer.set_text(font_system, text, attrs, Shaping::Advanced);
            buffer.shape_until_scroll(font_system, false);

            // Extract shaped glyph positions (cache keys + coordinates)
            let mut glyph_data = Vec::new();
            for run in buffer.layout_runs() {
                let line_y = run.line_y;
                for glyph in run.glyphs.iter() {
                    let physical = glyph.physical((0., 0.), 1.0);
                    glyph_data.push((physical.cache_key, physical.x, physical.y, line_y));
                }
            }
            glyph_data
        }));

        // Return buffer for reuse (even if shaping panicked, the buffer may be ok)
        self.reusable_buffer = Some(buffer);
        self.shaping_time += shape_start.elapsed();

        let glyph_data = match shaping_result {
            Ok(data) => data,
            Err(_) => {
                eprintln!("[strata] cosmic-text panic for text len={}, poisoning", text.len());
                self.poisoned_texts.insert(shape_key);
                return;
            }
        };

        // Atlas insertion and instance building (outside catch_unwind)
        let mut shaped_glyphs = Vec::new();
        for (cache_key, phys_x, phys_y, line_y) in glyph_data {
            let atlas_glyph = self.glyph_atlas.get_glyph(cache_key, font_system);

            if atlas_glyph.width == 0 || atlas_glyph.height == 0 {
                continue;
            }

            let rel_x = phys_x as f32 + atlas_glyph.offset_x as f32;
            let rel_y = phys_y as f32 + line_y - atlas_glyph.offset_y as f32;

            let tl_u = Self::uv_to_f32(atlas_glyph.uv_x);
            let tl_v = Self::uv_to_f32(atlas_glyph.uv_y);
            let br_u = Self::uv_to_f32(atlas_glyph.uv_x + atlas_glyph.uv_w);
            let br_v = Self::uv_to_f32(atlas_glyph.uv_y + atlas_glyph.uv_h);
            let mode = if atlas_glyph.is_color { 5 } else { 0 };

            self.instances.push(GpuInstance {
                pos: [(x + rel_x).round(), (y + rel_y).round()],
                size: [atlas_glyph.width as f32, atlas_glyph.height as f32],
                uv_tl: [tl_u, tl_v],
                uv_br: [br_u, br_v],
                color: packed_color,
                mode,
                corner_radius: 0.0,
                texture_layer: 0,
                clip_rect: NO_CLIP,
            });

            shaped_glyphs.push(CachedShapedGlyph {
                x: rel_x,
                y: rel_y,
                width: atlas_glyph.width as f32,
                height: atlas_glyph.height as f32,
                uv_tl: [tl_u, tl_v],
                uv_br: [br_u, br_v],
                mode,
            });
        }

        self.shape_cache.put(shape_key, (atlas_gen, Arc::new(shaped_glyphs)));
    }

    // =========================================================================
    // GPU upload and rendering
    // =========================================================================

    /// Prepare for rendering (upload data to GPU via unified memory).
    ///
    /// Writes globals and instance data directly into the current triple-buffer
    /// slot. No staging belt or command encoder needed — Apple Silicon unified
    /// memory makes CPU writes immediately visible to the GPU.
    pub fn prepare(
        &mut self,
        device: &metal::DeviceRef,
        viewport_width: f32,
        viewport_height: f32,
    ) {
        // Check if glyph atlas was resized
        if self.glyph_atlas.was_resized() {
            self.recreate_atlas_texture(device);
            self.glyph_atlas.ack_resize();
            self.glyph_atlas.take_dirty_region();
            self.invalidate_grid_row_cache();
        } else if let Some(dirty) = self.glyph_atlas.take_dirty_region() {
            self.upload_atlas_region(dirty);
        }

        let slot = (self.frame_index % MAX_FRAMES_IN_FLIGHT as u64) as usize;

        // Write globals directly into the current slot's buffer
        let globals = Globals {
            transform: create_orthographic_matrix(viewport_width, viewport_height),
            atlas_size: [
                self.glyph_atlas.atlas_width as f32,
                self.glyph_atlas.atlas_height as f32,
            ],
            _padding: [0.0, 0.0],
        };
        unsafe {
            let dst = self.globals_buffers[slot].contents() as *mut u8;
            std::ptr::copy_nonoverlapping(
                bytemuck::bytes_of(&globals).as_ptr(),
                dst,
                std::mem::size_of::<Globals>(),
            );
        }

        // Cap instance count (64 bytes each, 2M = 128 MB)
        const MAX_INSTANCES: usize = 2 * 1024 * 1024;
        if self.instances.len() > MAX_INSTANCES {
            self.instances.truncate(MAX_INSTANCES);
        }

        // Resize all 3 instance buffers if needed
        if self.instances.len() > self.instance_capacity {
            self.instance_capacity = self.instances.len().next_power_of_two().min(MAX_INSTANCES);
            let buf_size = (self.instance_capacity * std::mem::size_of::<GpuInstance>()) as u64;
            self.instance_buffers = (0..MAX_FRAMES_IN_FLIGHT)
                .map(|_| device.new_buffer(buf_size, metal::MTLResourceOptions::StorageModeShared))
                .collect();
        }

        // Write instance data directly into the current slot's buffer
        if !self.instances.is_empty() {
            let src = bytemuck::cast_slice::<GpuInstance, u8>(&self.instances);
            unsafe {
                let dst = self.instance_buffers[slot].contents() as *mut u8;
                std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
            }
        }
    }

    /// Render all instances in a single draw call.
    ///
    /// Creates a render pass on the command buffer, sets all pipeline state,
    /// and draws. The caller manages the command buffer lifecycle (present + commit).
    pub fn render(
        &self,
        command_buffer: &metal::CommandBufferRef,
        target: &metal::TextureRef,
        clip_bounds: &crate::shell::ClipBounds,
    ) {
        // Convert sRGB background to linear for the clear color
        fn srgb_to_linear(c: f32) -> f64 {
            let c = c as f64;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }

        let rpd = metal::RenderPassDescriptor::new();
        let color_attach = rpd.color_attachments().object_at(0).unwrap();
        color_attach.set_texture(Some(target));
        color_attach.set_load_action(metal::MTLLoadAction::Clear);
        color_attach.set_store_action(metal::MTLStoreAction::Store);
        color_attach.set_clear_color(metal::MTLClearColor::new(
            srgb_to_linear(self.background.r),
            srgb_to_linear(self.background.g),
            srgb_to_linear(self.background.b),
            self.background.a as f64,
        ));

        let encoder = command_buffer.new_render_command_encoder(&rpd);
        encoder.set_scissor_rect(metal::MTLScissorRect {
            x: clip_bounds.x as u64,
            y: clip_bounds.y as u64,
            width: clip_bounds.width as u64,
            height: clip_bounds.height as u64,
        });

        if !self.instances.is_empty() {
            let slot = (self.frame_index % MAX_FRAMES_IN_FLIGHT as u64) as usize;
            encoder.set_render_pipeline_state(&self.pipeline);
            encoder.set_vertex_buffer(0, Some(&self.instance_buffers[slot]), 0);
            encoder.set_vertex_buffer(1, Some(&self.globals_buffers[slot]), 0);
            encoder.set_fragment_buffer(0, Some(&self.globals_buffers[slot]), 0);
            encoder.set_fragment_texture(0, Some(&self.atlas_texture));
            encoder.set_fragment_sampler_state(0, Some(&self.atlas_sampler));
            encoder.set_fragment_texture(1, Some(&self.image_atlas.texture));
            encoder.set_fragment_sampler_state(1, Some(&self.image_sampler));
            encoder.draw_primitives_instanced(
                metal::MTLPrimitiveType::Triangle,
                0,
                6,
                self.instances.len() as u64,
            );
        }

        encoder.end_encoding();
    }

    /// Advance the triple-buffer frame index. Call after render + commit.
    pub fn advance_frame(&mut self) {
        self.frame_index += 1;
    }

    fn recreate_atlas_texture(&mut self, device: &metal::DeviceRef) {
        let (width, height) = (self.glyph_atlas.atlas_width, self.glyph_atlas.atlas_height);
        self.atlas_texture = create_rgba_texture(device, width, height);
        self.upload_atlas_full();
    }

    /// Upload only the dirty region of the glyph atlas to the GPU.
    fn upload_atlas_region(&self, region: (u32, u32, u32, u32)) {
        let (min_x, min_y, max_x, max_y) = region;
        let atlas_width = self.glyph_atlas.atlas_width;
        let data = self.glyph_atlas.atlas_data();

        // Pointer to the first pixel of the dirty rect within the full atlas data.
        let byte_offset = ((min_y * atlas_width + min_x) * 4) as usize;

        upload_texture_region(
            &self.atlas_texture,
            min_x, min_y,
            max_x - min_x, max_y - min_y,
            &data[byte_offset..],
            atlas_width * 4, // stride = full atlas row width
        );
    }

    /// Upload the entire glyph atlas (used after resize/recreate).
    fn upload_atlas_full(&self) {
        let atlas_width = self.glyph_atlas.atlas_width;
        let atlas_height = self.glyph_atlas.atlas_height;
        upload_texture_region(
            &self.atlas_texture,
            0, 0,
            atlas_width, atlas_height,
            self.glyph_atlas.atlas_data(),
            atlas_width * 4,
        );
    }
}

// =============================================================================
// Metal helpers
// =============================================================================

/// Create a 2D RGBA8 sRGB texture with shared storage (Apple Silicon unified memory).
fn create_rgba_texture(device: &metal::DeviceRef, width: u32, height: u32) -> metal::Texture {
    let desc = metal::TextureDescriptor::new();
    desc.set_width(width as u64);
    desc.set_height(height as u64);
    desc.set_pixel_format(metal::MTLPixelFormat::RGBA8Unorm_sRGB);
    desc.set_storage_mode(metal::MTLStorageMode::Shared);
    desc.set_usage(metal::MTLTextureUsage::ShaderRead);
    device.new_texture(&desc)
}

/// Upload a rectangle of pixel data to a Metal texture via `replace_region`.
///
/// Works directly for `StorageMode::Shared` — no blit encoder needed.
fn upload_texture_region(
    texture: &metal::TextureRef,
    x: u32, y: u32,
    width: u32, height: u32,
    data: &[u8],
    bytes_per_row: u32,
) {
    let region = metal::MTLRegion {
        origin: metal::MTLOrigin { x: x as u64, y: y as u64, z: 0 },
        size: metal::MTLSize { width: width as u64, height: height as u64, depth: 1 },
    };
    texture.replace_region(region, 0, data.as_ptr() as *const std::ffi::c_void, bytes_per_row as u64);
}

/// Create an orthographic projection matrix.
/// Map a box drawing character (U+2500..U+257F) to its line segments.
///
/// Returns `(left, right, up, down)` where each value is:
///   0 = none, 1 = light, 2 = heavy, 3 = double
///
/// Also handles rounded corners (U+256D-U+2570) and some extensions.
fn box_drawing_segments(ch: char) -> Option<(u8, u8, u8, u8)> {
    // (left, right, up, down)
    Some(match ch {
        // Light lines
        '─' => (1, 1, 0, 0), // U+2500 LIGHT HORIZONTAL
        '━' => (2, 2, 0, 0), // U+2501 HEAVY HORIZONTAL
        '│' => (0, 0, 1, 1), // U+2502 LIGHT VERTICAL
        '┃' => (0, 0, 2, 2), // U+2503 HEAVY VERTICAL

        // Light triple-dash / quadruple-dash (render as light line)
        '┄' | '┅' | '┆' | '┇' | '┈' | '┉' | '┊' | '┋' => {
            let cp = ch as u32;
            if cp % 2 == 0 { (1, 1, 0, 0) } else { (0, 0, 1, 1) }
        }

        // Light corners
        '┌' => (0, 1, 0, 1), // U+250C
        '┍' => (0, 2, 0, 1), // U+250D
        '┎' => (0, 1, 0, 2), // U+250E
        '┏' => (0, 2, 0, 2), // U+250F
        '┐' => (1, 0, 0, 1), // U+2510
        '┑' => (2, 0, 0, 1), // U+2511
        '┒' => (1, 0, 0, 2), // U+2512
        '┓' => (2, 0, 0, 2), // U+2513
        '└' => (0, 1, 1, 0), // U+2514
        '┕' => (0, 2, 1, 0), // U+2515
        '┖' => (0, 1, 2, 0), // U+2516
        '┗' => (0, 2, 2, 0), // U+2517
        '┘' => (1, 0, 1, 0), // U+2518
        '┙' => (2, 0, 1, 0), // U+2519
        '┚' => (1, 0, 2, 0), // U+251A
        '┛' => (2, 0, 2, 0), // U+251B

        // T-pieces
        '├' => (0, 1, 1, 1), // U+251C
        '┝' => (0, 2, 1, 1), // U+251D
        '┞' => (0, 1, 2, 1), // U+251E
        '┟' => (0, 1, 1, 2), // U+251F
        '┠' => (0, 1, 2, 2), // U+2520
        '┡' => (0, 2, 2, 1), // U+2521
        '┢' => (0, 2, 1, 2), // U+2522
        '┣' => (0, 2, 2, 2), // U+2523
        '┤' => (1, 0, 1, 1), // U+2524
        '┥' => (2, 0, 1, 1), // U+2525
        '┦' => (1, 0, 2, 1), // U+2526
        '┧' => (1, 0, 1, 2), // U+2527
        '┨' => (1, 0, 2, 2), // U+2528
        '┩' => (2, 0, 2, 1), // U+2529
        '┪' => (2, 0, 1, 2), // U+252A
        '┫' => (2, 0, 2, 2), // U+252B
        '┬' => (1, 1, 0, 1), // U+252C
        '┭' => (2, 1, 0, 1), // U+252D
        '┮' => (1, 2, 0, 1), // U+252E
        '┯' => (2, 2, 0, 1), // U+252F
        '┰' => (1, 1, 0, 2), // U+2530
        '┱' => (2, 1, 0, 2), // U+2531
        '┲' => (1, 2, 0, 2), // U+2532
        '┳' => (2, 2, 0, 2), // U+2533
        '┴' => (1, 1, 1, 0), // U+2534
        '┵' => (2, 1, 1, 0), // U+2535
        '┶' => (1, 2, 1, 0), // U+2536
        '┷' => (2, 2, 1, 0), // U+2537
        '┸' => (1, 1, 2, 0), // U+2538
        '┹' => (2, 1, 2, 0), // U+2539
        '┺' => (1, 2, 2, 0), // U+253A
        '┻' => (2, 2, 2, 0), // U+253B

        // Crosses
        '┼' => (1, 1, 1, 1), // U+253C
        '┽' => (2, 1, 1, 1), // U+253D
        '┾' => (1, 2, 1, 1), // U+253E
        '┿' => (2, 2, 1, 1), // U+253F
        '╀' => (1, 1, 2, 1), // U+2540
        '╁' => (1, 1, 1, 2), // U+2541
        '╂' => (1, 1, 2, 2), // U+2542
        '╃' => (2, 1, 2, 1), // U+2543
        '╄' => (1, 2, 2, 1), // U+2544
        '╅' => (2, 1, 1, 2), // U+2545
        '╆' => (1, 2, 1, 2), // U+2546
        '╇' => (2, 2, 2, 1), // U+2547
        '╈' => (2, 2, 1, 2), // U+2548
        '╉' => (2, 1, 2, 2), // U+2549
        '╊' => (1, 2, 2, 2), // U+254A
        '╋' => (2, 2, 2, 2), // U+254B

        // Light/heavy half-lines
        '╴' => (1, 0, 0, 0), // U+2574 LIGHT LEFT
        '╵' => (0, 0, 1, 0), // U+2575 LIGHT UP
        '╶' => (0, 1, 0, 0), // U+2576 LIGHT RIGHT
        '╷' => (0, 0, 0, 1), // U+2577 LIGHT DOWN
        '╸' => (2, 0, 0, 0), // U+2578 HEAVY LEFT
        '╹' => (0, 0, 2, 0), // U+2579 HEAVY UP
        '╺' => (0, 2, 0, 0), // U+257A HEAVY RIGHT
        '╻' => (0, 0, 0, 2), // U+257B HEAVY DOWN

        // Mixed light/heavy
        '╼' => (1, 2, 0, 0), // U+257C LIGHT LEFT HEAVY RIGHT
        '╽' => (0, 0, 1, 2), // U+257D LIGHT UP HEAVY DOWN
        '╾' => (2, 1, 0, 0), // U+257E HEAVY LEFT LIGHT RIGHT
        '╿' => (0, 0, 2, 1), // U+257F HEAVY UP LIGHT DOWN

        // Double lines
        '═' => (3, 3, 0, 0), // U+2550
        '║' => (0, 0, 3, 3), // U+2551

        // Double corners
        '╔' => (0, 3, 0, 3), // U+2554
        '╗' => (3, 0, 0, 3), // U+2557
        '╚' => (0, 3, 3, 0), // U+255A
        '╝' => (3, 0, 3, 0), // U+255D

        // Double/single mixed corners
        '╒' => (0, 3, 0, 1), // U+2552
        '╓' => (0, 1, 0, 3), // U+2553
        '╕' => (3, 0, 0, 1), // U+2555
        '╖' => (1, 0, 0, 3), // U+2556
        '╘' => (0, 3, 1, 0), // U+2558
        '╙' => (0, 1, 3, 0), // U+2559
        '╛' => (1, 0, 3, 0), // U+255B UP DOUBLE AND LEFT SINGLE
        '╜' => (3, 0, 1, 0), // U+255C UP SINGLE AND LEFT DOUBLE

        // Double T-pieces
        '╠' => (0, 3, 3, 3), // U+2560
        '╣' => (3, 0, 3, 3), // U+2563
        '╦' => (3, 3, 0, 3), // U+2566
        '╩' => (3, 3, 3, 0), // U+2569

        // Double/single T-pieces
        '╞' => (0, 3, 1, 1), // U+255E
        '╟' => (0, 1, 3, 3), // U+255F
        '╡' => (3, 0, 1, 1), // U+2561
        '╢' => (1, 0, 3, 3), // U+2562
        '╤' => (3, 3, 0, 1), // U+2564
        '╥' => (1, 1, 0, 3), // U+2565
        '╧' => (3, 3, 1, 0), // U+2567
        '╨' => (1, 1, 3, 0), // U+2568

        // Double crosses
        '╪' => (3, 3, 1, 1), // U+256A
        '╫' => (1, 1, 3, 3), // U+256B
        '╬' => (3, 3, 3, 3), // U+256C

        // Rounded corners (render as light lines — the rounding is visual sugar)
        '╭' => (0, 1, 0, 1), // U+256D
        '╮' => (1, 0, 0, 1), // U+256E
        '╯' => (1, 0, 1, 0), // U+256F
        '╰' => (0, 1, 1, 0), // U+2570

        // Diagonal lines — not handled (rare, complex geometry)
        '╱' | '╲' | '╳' => return None,

        _ => return None,
    })
}

/// Check if a character is a box drawing character that we handle.
#[inline]
pub fn is_box_drawing(ch: char) -> bool {
    let cp = ch as u32;
    (0x2500..=0x257F).contains(&cp) && !matches!(ch, '╱' | '╲' | '╳')
}

/// Check if a character is a block element that we handle.
#[inline]
pub fn is_block_element(ch: char) -> bool {
    let cp = ch as u32;
    (0x2580..=0x259F).contains(&cp)
}

/// Check if a character should be custom-drawn (box drawing or block element).
#[inline]
pub fn is_custom_drawn(ch: char) -> bool {
    is_box_drawing(ch) || is_block_element(ch)
}

/// Map a block element character to its fractional cell rect (x, y, w, h).
///
/// All values are 0.0..1.0 fractions of the cell dimensions.
fn block_element_rect(ch: char) -> Option<(f32, f32, f32, f32)> {
    // (x_frac, y_frac, w_frac, h_frac)
    Some(match ch {
        '▀' => (0.0, 0.0, 1.0, 0.5),    // U+2580 UPPER HALF
        '▁' => (0.0, 7.0/8.0, 1.0, 1.0/8.0), // U+2581 LOWER ONE EIGHTH
        '▂' => (0.0, 3.0/4.0, 1.0, 1.0/4.0), // U+2582 LOWER ONE QUARTER
        '▃' => (0.0, 5.0/8.0, 1.0, 3.0/8.0), // U+2583 LOWER THREE EIGHTHS
        '▄' => (0.0, 0.5, 1.0, 0.5),     // U+2584 LOWER HALF
        '▅' => (0.0, 3.0/8.0, 1.0, 5.0/8.0), // U+2585 LOWER FIVE EIGHTHS
        '▆' => (0.0, 1.0/4.0, 1.0, 3.0/4.0), // U+2586 LOWER THREE QUARTERS
        '▇' => (0.0, 1.0/8.0, 1.0, 7.0/8.0), // U+2587 LOWER SEVEN EIGHTHS
        '█' => (0.0, 0.0, 1.0, 1.0),     // U+2588 FULL BLOCK
        '▉' => (0.0, 0.0, 7.0/8.0, 1.0), // U+2589 LEFT SEVEN EIGHTHS
        '▊' => (0.0, 0.0, 3.0/4.0, 1.0), // U+258A LEFT THREE QUARTERS
        '▋' => (0.0, 0.0, 5.0/8.0, 1.0), // U+258B LEFT FIVE EIGHTHS
        '▌' => (0.0, 0.0, 0.5, 1.0),     // U+258C LEFT HALF
        '▍' => (0.0, 0.0, 3.0/8.0, 1.0), // U+258D LEFT THREE EIGHTHS
        '▎' => (0.0, 0.0, 1.0/4.0, 1.0), // U+258E LEFT ONE QUARTER
        '▏' => (0.0, 0.0, 1.0/8.0, 1.0), // U+258F LEFT ONE EIGHTH
        '▐' => (0.5, 0.0, 0.5, 1.0),     // U+2590 RIGHT HALF
        // Shade characters handled specially in draw_block_char (alpha adjustment)
        '░' | '▒' | '▓' => return None,
        '▔' => (0.0, 0.0, 1.0, 1.0/8.0), // U+2594 UPPER ONE EIGHTH
        '▕' => (7.0/8.0, 0.0, 1.0/8.0, 1.0), // U+2595 RIGHT ONE EIGHTH
        '▖' => (0.0, 0.5, 0.5, 0.5),     // U+2596 QUADRANT LOWER LEFT
        '▗' => (0.5, 0.5, 0.5, 0.5),     // U+2597 QUADRANT LOWER RIGHT
        '▘' => (0.0, 0.0, 0.5, 0.5),     // U+2598 QUADRANT UPPER LEFT
        '▙' => return None, // U+2599 QUADRANT UPPER LEFT AND LOWER LEFT AND LOWER RIGHT (3 quads)
        '▚' => return None, // U+259A QUADRANT UPPER LEFT AND LOWER RIGHT (2 quads, diagonal)
        '▛' => return None, // U+259B QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER LEFT (3 quads)
        '▜' => return None, // U+259C QUADRANT UPPER LEFT AND UPPER RIGHT AND LOWER RIGHT (3 quads)
        '▝' => (0.5, 0.0, 0.5, 0.5),     // U+259D QUADRANT UPPER RIGHT
        '▞' => return None, // U+259E QUADRANT UPPER RIGHT AND LOWER LEFT (2 quads, diagonal)
        '▟' => return None, // U+259F QUADRANT UPPER RIGHT AND LOWER LEFT AND LOWER RIGHT (3 quads)
        _ => return None,
    })
}

fn create_orthographic_matrix(width: f32, height: f32) -> [[f32; 4]; 4] {
    let left = 0.0;
    let right = width;
    let top = 0.0;
    let bottom = height;
    let near = -1.0;
    let far = 1.0;

    let sx = 2.0 / (right - left);
    let sy = 2.0 / (top - bottom);
    let sz = 2.0 / (far - near);
    let tx = -(right + left) / (right - left);
    let ty = -(top + bottom) / (top - bottom);
    let tz = -(far + near) / (far - near);

    [
        [sx, 0.0, 0.0, 0.0],
        [0.0, sy, 0.0, 0.0],
        [0.0, 0.0, sz, 0.0],
        [tx, ty, tz, 1.0],
    ]
}
