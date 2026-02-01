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
//! Uses `StagingBelt` for buffer uploads to exploit unified memory on M1/M2/M3.

use std::num::NonZeroU64;
use std::path::Path;

use iced::widget::shader::wgpu;
use wgpu::util::StagingBelt;

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
    texture: wgpu::Texture,
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

/// GPU pipeline for Strata rendering.
///
/// Uses a unified ubershader that renders all 2D primitives in one draw call.
/// Instances are rendered in buffer order, enabling perfect Z-ordering.
pub struct StrataPipeline {
    pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    atlas_texture: wgpu::Texture,
    /// Combined bind group for glyph atlas (bindings 0–1) + image atlas (bindings 2–3).
    atlas_bind_group: wgpu::BindGroup,
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    atlas_sampler: wgpu::Sampler,
    /// Image atlas (separate texture from glyph atlas — full RGBA).
    image_atlas: ImageAtlas,
    image_sampler: wgpu::Sampler,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    glyph_atlas: GlyphAtlas,
    /// All instances to render, in draw order.
    instances: Vec<GpuInstance>,
    /// Background color.
    background: Color,
    /// Staging belt for unified memory uploads (Apple Silicon optimization).
    staging_belt: StagingBelt,
}

impl StrataPipeline {
    /// Create a new pipeline.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, font_size: f32) -> Self {
        let mut glyph_atlas = GlyphAtlas::new(font_size);
        glyph_atlas.precache_ascii();

        // Create shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Strata Ubershader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/glyph.wgsl").into()),
        });

        // Create globals bind group layout
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Strata Globals Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Create combined atlas bind group layout (group 1):
        //   binding 0: glyph atlas texture
        //   binding 1: glyph atlas sampler
        //   binding 2: image atlas texture
        //   binding 3: image atlas sampler
        let atlas_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Strata Atlas Layout"),
                entries: &[
                    // Glyph atlas
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // Image atlas
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        // Create pipeline layout (2 bind groups: globals, combined atlas)
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Strata Pipeline Layout"),
            bind_group_layouts: &[&globals_layout, &atlas_bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Strata Ubershader Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        // pos
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        // size
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        // uv_tl
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 16,
                            shader_location: 2,
                        },
                        // uv_br
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 3,
                        },
                        // color
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 32,
                            shader_location: 4,
                        },
                        // mode
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 36,
                            shader_location: 5,
                        },
                        // corner_radius
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 40,
                            shader_location: 6,
                        },
                        // texture_layer
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 44,
                            shader_location: 7,
                        },
                        // clip_rect
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 48,
                            shader_location: 8,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // Create globals buffer
        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Strata Globals Buffer"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create globals bind group
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Strata Globals Bind Group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        // Create atlas texture
        let (atlas_width, atlas_height) = (glyph_atlas.atlas_width, glyph_atlas.atlas_height);
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Strata Atlas Texture"),
            size: wgpu::Extent3d {
                width: atlas_width,
                height: atlas_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Create sampler
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Strata Atlas Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create image sampler (shared across atlas rebuilds)
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Strata Image Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create 1×1 white placeholder image atlas (no images loaded yet)
        let placeholder_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Strata Image Atlas Placeholder"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &placeholder_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );

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

        // Create combined atlas bind group (glyph atlas + image atlas)
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let image_atlas_view = image_atlas.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Strata Combined Atlas Bind Group"),
            layout: &atlas_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&image_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&image_sampler),
                },
            ],
        });

        // Create instance buffer
        let initial_capacity = 4096;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Strata Instance Buffer"),
            size: (initial_capacity * std::mem::size_of::<GpuInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create staging belt for unified memory uploads.
        let staging_belt = StagingBelt::new(8 * 1024 * 1024);

        Self {
            pipeline,
            globals_buffer,
            globals_bind_group,
            atlas_texture,
            atlas_bind_group,
            atlas_bind_group_layout,
            atlas_sampler,
            image_atlas,
            image_sampler,
            instance_buffer,
            instance_capacity: initial_capacity,
            glyph_atlas,
            instances: Vec::new(),
            background: Color::BLACK,
            staging_belt,
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
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &Path,
    ) -> ImageHandle {
        let img = image::open(path)
            .unwrap_or_else(|e| panic!("Failed to load image {}: {}", path.display(), e))
            .to_rgba8();
        let (w, h) = img.dimensions();
        self.load_image_rgba(device, queue, w, h, &img.into_raw())
    }

    /// Load raw RGBA pixel data and return a handle for rendering.
    pub fn load_image_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
            self.grow_image_atlas(device, queue, new_width, new_height);
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

        // Upload the modified region to GPU
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &atlas.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: ax, y: ay, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            // Upload just the rows we wrote (contiguous in source data)
            data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );

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
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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

        // Recreate GPU texture
        atlas.texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Strata Image Atlas"),
            size: wgpu::Extent3d { width: new_width, height: new_height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload entire atlas data
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &atlas.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(new_width * 4),
                rows_per_image: Some(new_height),
            },
            wgpu::Extent3d { width: new_width, height: new_height, depth_or_array_layers: 1 },
        );

        // Recompute UV regions for all loaded images
        for slot in &mut atlas.images {
            let Some(img) = slot.as_mut() else { continue };
            // UVs were based on old atlas dimensions — need to recompute from pixel positions.
            // We don't store pixel positions separately, so derive them from old UVs.
            let px_x = img.uv_tl[0] * old_width as f32;
            let px_y = img.uv_tl[1] * old_height as f32;
            img.uv_tl = [px_x / new_width as f32, px_y / new_height as f32];
            img.uv_br = [
                (px_x + img.width as f32) / new_width as f32,
                (px_y + img.height as f32) / new_height as f32,
            ];
        }

        // Rebuild combined bind group (image atlas texture changed)
        self.rebuild_combined_bind_group(device);
    }

    // =========================================================================
    // Text rendering
    // =========================================================================

    /// Add a text string to render at a specific font size.
    pub fn add_text(&mut self, text: &str, x: f32, y: f32, color: Color, font_size: f32) {
        let packed_color = color.pack();
        let metrics = self.glyph_atlas.metrics_for_size(font_size);
        let ascent = metrics.ascent;
        let cell_width = metrics.cell_width;
        let line_height = metrics.cell_height;
        let mut cursor_x = x;
        let mut cursor_y = y;

        for ch in text.chars() {
            if ch == '\n' {
                cursor_x = x;
                cursor_y += line_height;
                continue;
            }

            let glyph = self.glyph_atlas.get_glyph(ch, font_size);

            // Skip zero-size glyphs (spaces, etc.) but advance cursor
            if glyph.width == 0 || glyph.height == 0 {
                cursor_x += cell_width;
                continue;
            }

            // Pixel-align glyph positions to avoid subpixel blur
            let glyph_x = (cursor_x + glyph.offset_x as f32).round();
            let glyph_y = (cursor_y + ascent - glyph.offset_y as f32 - glyph.height as f32).round();

            // Convert u16 atlas UVs to f32 tl/br
            let tl_u = Self::uv_to_f32(glyph.uv_x);
            let tl_v = Self::uv_to_f32(glyph.uv_y);
            let br_u = Self::uv_to_f32(glyph.uv_x + glyph.uv_w);
            let br_v = Self::uv_to_f32(glyph.uv_y + glyph.uv_h);

            self.instances.push(GpuInstance {
                pos: [glyph_x, glyph_y],
                size: [glyph.width as f32, glyph.height as f32],
                uv_tl: [tl_u, tl_v],
                uv_br: [br_u, br_v],
                color: packed_color,
                mode: 0,
                corner_radius: 0.0,
                texture_layer: 0,
                clip_rect: NO_CLIP,
            });

            cursor_x += cell_width;
        }
    }

    // =========================================================================
    // GPU upload and rendering
    // =========================================================================

    /// Prepare for rendering (upload data to GPU).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        viewport_width: f32,
        viewport_height: f32,
    ) {
        // Check if atlas was resized
        if self.glyph_atlas.was_resized() {
            self.recreate_atlas_texture(device, queue);
            self.glyph_atlas.ack_resize();
            // Drain dirty region — full atlas was already uploaded by recreate
            self.glyph_atlas.take_dirty_region();
        } else if let Some(dirty) = self.glyph_atlas.take_dirty_region() {
            self.upload_atlas_region(queue, dirty);
        }

        // Update globals
        let globals = Globals {
            transform: create_orthographic_matrix(viewport_width, viewport_height),
            atlas_size: [
                self.glyph_atlas.atlas_width as f32,
                self.glyph_atlas.atlas_height as f32,
            ],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        // Cap instance count to stay within wgpu's maximum buffer size (256 MB).
        // Each GpuInstance is 64 bytes, so the hard limit is ~4M instances.
        // We use a slightly lower cap to leave headroom for other allocations.
        const MAX_INSTANCES: usize = 2 * 1024 * 1024; // 2M instances = 128 MB
        if self.instances.len() > MAX_INSTANCES {
            self.instances.truncate(MAX_INSTANCES);
        }

        // Resize instance buffer if needed
        if self.instances.len() > self.instance_capacity {
            self.instance_capacity = self.instances.len().next_power_of_two().min(MAX_INSTANCES);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Strata Instance Buffer"),
                size: (self.instance_capacity * std::mem::size_of::<GpuInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        // Upload instances via staging belt
        if !self.instances.is_empty() {
            let instance_bytes = self.instances.len() * std::mem::size_of::<GpuInstance>();
            if let Some(size) = NonZeroU64::new(instance_bytes as u64) {
                let mut staging_buffer = self.staging_belt.write_buffer(
                    encoder,
                    &self.instance_buffer,
                    0,
                    size,
                    device,
                );
                staging_buffer.copy_from_slice(bytemuck::cast_slice(&self.instances));
            }
        }

        self.staging_belt.finish();
    }

    /// Reclaim staging buffer memory after GPU finishes the frame.
    pub fn after_frame(&mut self) {
        self.staging_belt.recall();
    }

    /// Render all instances in a single draw call.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &iced::Rectangle<u32>,
    ) {
        // Background color is specified in sRGB but the render target is sRGB format,
        // which means the GPU will apply linear→sRGB conversion on output. We must
        // convert to linear here to avoid double-gamma (same as unpack_color in shader).
        fn srgb_to_linear(c: f32) -> f64 {
            let c = c as f64;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        let clear_color = wgpu::Color {
            r: srgb_to_linear(self.background.r),
            g: srgb_to_linear(self.background.g),
            b: srgb_to_linear(self.background.b),
            a: self.background.a as f64,
        };

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Strata Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        render_pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );

        if !self.instances.is_empty() {
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.globals_bind_group, &[]);
            render_pass.set_bind_group(1, &self.atlas_bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..self.instances.len() as u32);
        }
    }

    fn recreate_atlas_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let (width, height) = (self.glyph_atlas.atlas_width, self.glyph_atlas.atlas_height);

        self.atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Strata Atlas Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        self.upload_atlas_full(queue);
        self.rebuild_combined_bind_group(device);
    }

    /// Rebuild the combined bind group (glyph atlas + image atlas in group 1).
    /// Must be called whenever either atlas texture changes.
    fn rebuild_combined_bind_group(&mut self, device: &wgpu::Device) {
        let glyph_view = self.atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let image_view = self.image_atlas.texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Strata Combined Atlas Bind Group"),
            layout: &self.atlas_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&glyph_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.atlas_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
            ],
        });
    }

    /// Upload only the dirty region of the glyph atlas to the GPU.
    fn upload_atlas_region(&self, queue: &wgpu::Queue, region: (u32, u32, u32, u32)) {
        let (min_x, min_y, max_x, max_y) = region;
        let atlas_width = self.glyph_atlas.atlas_width;
        let region_w = max_x - min_x;
        let region_h = max_y - min_y;
        let data = self.glyph_atlas.atlas_data();

        // Offset into atlas_data for the first pixel of the dirty rect.
        let byte_offset = ((min_y * atlas_width + min_x) * 4) as u64;

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: min_x, y: min_y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::ImageDataLayout {
                offset: byte_offset,
                bytes_per_row: Some(atlas_width * 4), // stride = full atlas row width
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: region_w,
                height: region_h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Upload the entire glyph atlas (used after resize/recreate).
    fn upload_atlas_full(&self, queue: &wgpu::Queue) {
        let atlas_width = self.glyph_atlas.atlas_width;
        let atlas_height = self.glyph_atlas.atlas_height;
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            self.glyph_atlas.atlas_data(),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(atlas_width * 4),
                rows_per_image: Some(atlas_height),
            },
            wgpu::Extent3d {
                width: atlas_width,
                height: atlas_height,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Create an orthographic projection matrix.
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
