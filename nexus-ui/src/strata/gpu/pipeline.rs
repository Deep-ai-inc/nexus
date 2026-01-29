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

use iced::widget::shader::wgpu;
use wgpu::util::StagingBelt;

use super::glyph_atlas::GlyphAtlas;
use crate::strata::primitives::{Color, Rect};

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
    atlas_bind_group: wgpu::BindGroup,
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    atlas_sampler: wgpu::Sampler,
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
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, font_size: f32) -> Self {
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

        // Create atlas bind group layout
        let atlas_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Strata Atlas Layout"),
                entries: &[
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
                ],
            });

        // Create pipeline layout
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

        // Create atlas bind group
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Strata Atlas Bind Group"),
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
    // Mode 4: Image (texture array — stubbed, requires texture array setup)
    // =========================================================================

    /// Add an image from the texture array (future: requires texture array setup).
    pub fn add_image(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        layer: u32,
        corner_radius: f32,
        tint: Color,
    ) {
        self.instances.push(GpuInstance {
            pos: [x, y],
            size: [width, height],
            uv_tl: [0.0, 0.0],
            uv_br: [1.0, 1.0],
            color: tint.pack(),
            mode: 4,
            corner_radius,
            texture_layer: layer,
            clip_rect: NO_CLIP,
        });
    }

    // =========================================================================
    // Text rendering
    // =========================================================================

    /// Add a text string to render.
    pub fn add_text(&mut self, text: &str, x: f32, y: f32, color: Color) {
        let packed_color = color.pack();
        let mut cursor_x = x;
        let ascent = self.glyph_atlas.ascent;

        for ch in text.chars() {
            if ch == '\n' {
                cursor_x = x;
                continue;
            }

            let glyph = self.glyph_atlas.get_glyph(ch);

            // Skip zero-size glyphs (spaces, etc.) but advance cursor
            if glyph.width == 0 || glyph.height == 0 {
                cursor_x += self.glyph_atlas.cell_width;
                continue;
            }

            let glyph_x = cursor_x + glyph.offset_x as f32;
            let glyph_y = y + ascent - glyph.offset_y as f32 - glyph.height as f32;

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

            cursor_x += self.glyph_atlas.cell_width;
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
            self.glyph_atlas.mark_clean();
        } else if self.glyph_atlas.is_dirty() {
            self.upload_atlas(queue);
            self.glyph_atlas.mark_clean();
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

        // Resize instance buffer if needed
        if self.instances.len() > self.instance_capacity {
            self.instance_capacity = self.instances.len().next_power_of_two();
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
        let clear_color = wgpu::Color {
            r: self.background.r as f64,
            g: self.background.g as f64,
            b: self.background.b as f64,
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

        let atlas_view = self
            .atlas_texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Strata Atlas Bind Group"),
            layout: &self.atlas_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.atlas_sampler),
                },
            ],
        });

        self.upload_atlas(queue);
    }

    fn upload_atlas(&self, queue: &wgpu::Queue) {
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
                bytes_per_row: Some(self.glyph_atlas.atlas_width * 4),
                rows_per_image: Some(self.glyph_atlas.atlas_height),
            },
            wgpu::Extent3d {
                width: self.glyph_atlas.atlas_width,
                height: self.glyph_atlas.atlas_height,
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
