//! GPU Pipeline for Strata rendering.
//!
//! Provides text rendering using instanced glyph quads.
//!
//! # Apple Silicon Optimization
//!
//! Uses `StagingBelt` for buffer uploads to exploit unified memory on M1/M2/M3.
//! Instead of `queue.write_buffer()` which may involve intermediate copies,
//! we write directly to mapped memory that lives in the unified address space.

use std::num::NonZeroU64;

use iced::widget::shader::wgpu;
use wgpu::util::StagingBelt;

use super::glyph_atlas::GlyphAtlas;
use crate::strata::primitives::Color;

/// Glyph instance for GPU rendering (32 bytes).
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct GlyphInstance {
    /// Position (x, y) in pixels.
    pub position: [f32; 2],   // 8 bytes
    /// Size (width, height) in pixels.
    pub size: [f32; 2],       // 8 bytes
    /// UV coordinates (u, v, w, h) as normalized u16.
    pub uv: [u16; 4],         // 8 bytes
    /// Color as packed RGBA8.
    pub color: u32,           // 4 bytes
    /// Flags (e.g., selected).
    pub flags: u32,           // 4 bytes
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

/// GPU pipeline for Strata rendering.
///
/// # Future optimization
/// Currently instances are collected into a `Vec` then copied to the staging belt.
/// For maximum throughput, the API could be redesigned to write directly into
/// the staging belt's mapped memory slice, bypassing the intermediate Vec.
/// This would require exposing a "begin frame" / "end frame" pattern where
/// callers write instances directly to a provided slice.
pub struct StrataPipeline {
    glyph_pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    atlas_bind_group_layout: wgpu::BindGroupLayout,
    atlas_sampler: wgpu::Sampler,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    glyph_atlas: GlyphAtlas,
    /// Instances to render this frame (CPU-side collection).
    /// TODO: Future optimization - write directly to staging belt slice.
    instances: Vec<GlyphInstance>,
    /// Background color.
    background: Color,
    /// Staging belt for unified memory uploads (Apple Silicon optimization).
    /// Writes directly to mapped memory, avoiding intermediate copies.
    staging_belt: StagingBelt,
}

impl StrataPipeline {
    /// Create a new pipeline.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, font_size: f32) -> Self {
        let mut glyph_atlas = GlyphAtlas::new(font_size);
        glyph_atlas.precache_ascii();

        // Create shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Strata Glyph Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/glyph.wgsl").into()),
        });

        // Create globals bind group layout
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Strata Globals Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
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
        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Strata Glyph Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        // position
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
                        // uv
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint16x4,
                            offset: 16,
                            shader_location: 2,
                        },
                        // color
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 24,
                            shader_location: 3,
                        },
                        // flags
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 28,
                            shader_location: 4,
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
            size: (initial_capacity * std::mem::size_of::<GlyphInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create staging belt for unified memory uploads.
        // 8MB chunk size to handle large text dumps (e.g., cat'ing huge log files)
        // without needing to allocate additional chunks on the fly.
        let staging_belt = StagingBelt::new(8 * 1024 * 1024);

        Self {
            glyph_pipeline,
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

    /// Set the background color.
    pub fn set_background(&mut self, color: Color) {
        self.background = color;
    }

    /// Clear instances for new frame.
    pub fn clear(&mut self) {
        self.instances.clear();
    }

    /// Add a text string to render.
    pub fn add_text(&mut self, text: &str, x: f32, y: f32, color: Color) {
        let packed_color = color.pack();
        let mut cursor_x = x;
        let line_height = self.glyph_atlas.cell_height;
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

            self.instances.push(GlyphInstance {
                position: [glyph_x, glyph_y],
                size: [glyph.width as f32, glyph.height as f32],
                uv: [glyph.uv_x, glyph.uv_y, glyph.uv_w, glyph.uv_h],
                color: packed_color,
                flags: 0,
            });

            cursor_x += self.glyph_atlas.cell_width;
        }
    }

    /// Prepare for rendering (upload data to GPU).
    ///
    /// Uses StagingBelt to write directly to unified memory on Apple Silicon,
    /// avoiding intermediate buffer copies.
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

        // Update globals (small uniform buffer, write_buffer is fine)
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
                size: (self.instance_capacity * std::mem::size_of::<GlyphInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        // Upload instances using staging belt (unified memory optimization).
        // On Apple Silicon, this writes directly to mapped memory shared by CPU/GPU.
        if !self.instances.is_empty() {
            let instance_bytes = self.instances.len() * std::mem::size_of::<GlyphInstance>();
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

        // Signal that we're done writing to the staging belt for this frame.
        self.staging_belt.finish();
    }

    /// Call after the GPU has finished rendering the frame.
    ///
    /// Reclaims staging buffer memory for reuse, preventing allocations
    /// after the initial warmup phase.
    pub fn after_frame(&mut self) {
        self.staging_belt.recall();
    }

    /// Render to the target.
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

        // Set scissor rect
        render_pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );

        if !self.instances.is_empty() {
            render_pass.set_pipeline(&self.glyph_pipeline);
            render_pass.set_bind_group(0, &self.globals_bind_group, &[]);
            render_pass.set_bind_group(1, &self.atlas_bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            // 6 vertices per quad (2 triangles)
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
