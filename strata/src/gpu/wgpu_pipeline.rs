//! wgpu GPU backend for Strata rendering.
//!
//! Cross-platform GPU backend using wgpu. Manages device, surface, render pipeline,
//! buffers, textures, and bind groups. Delegates instance building / caching to the
//! platform-independent `StrataPipeline`.

use cosmic_text::FontSystem;

use super::glyph_atlas::GlyphAtlas;
use super::pipeline::{
    create_orthographic_matrix, Globals, GpuInstance, ImageAtlas, ImageHandle, StrataPipeline,
};

/// wgpu-backed GPU renderer.
///
/// Wraps the platform-independent `StrataPipeline` and manages wgpu GPU resources.
/// The shell accesses drawing methods via the `pipeline` field.
pub struct WgpuRenderer {
    /// Platform-independent pipeline (instance building, caching, text shaping).
    pub pipeline: StrataPipeline,
    /// wgpu render pipeline (compiled shaders + vertex layout + blend state).
    render_pipeline: wgpu::RenderPipeline,
    /// Globals uniform buffer.
    globals_buffer: wgpu::Buffer,
    /// Glyph atlas GPU texture.
    atlas_texture: wgpu::Texture,
    /// Glyph atlas texture view.
    atlas_view: wgpu::TextureView,
    /// Glyph atlas sampler (linear filtering).
    atlas_sampler: wgpu::Sampler,
    /// Image atlas GPU texture.
    image_atlas_texture: wgpu::Texture,
    /// Image atlas texture view.
    image_atlas_view: wgpu::TextureView,
    /// Image atlas sampler (linear filtering).
    image_sampler: wgpu::Sampler,
    /// Instance vertex buffer.
    instance_buffer: wgpu::Buffer,
    /// Current instance buffer capacity (number of GpuInstances).
    instance_capacity: usize,
    /// Bind group layout for the shader.
    bind_group_layout: wgpu::BindGroupLayout,
    /// Current bind group (rebuilt when textures change).
    bind_group: wgpu::BindGroup,
    /// Surface texture format (kept for potential future resize/reconfiguration).
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,
}

impl WgpuRenderer {
    /// Create a new wgpu renderer.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        font_size: f32,
        font_system: &mut FontSystem,
    ) -> Self {
        let mut glyph_atlas = GlyphAtlas::new(font_size, font_system);
        glyph_atlas.precache_ascii(font_system);

        // Shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("strata_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/glyph.wgsl").into()),
        });

        // Bind group layout
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("strata_bind_group_layout"),
                entries: &[
                    // @group(0) @binding(0) — globals uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // @group(0) @binding(1) — atlas texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // @group(0) @binding(2) — atlas sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // @group(0) @binding(3) — image texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // @group(0) @binding(4) — image sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        // Pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strata_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Vertex buffer layout (instance step mode, matching GpuInstance)
        let instance_attrs = [
            // pos: vec2f at offset 0
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            // size: vec2f at offset 8
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 8,
                shader_location: 1,
            },
            // uv_tl: vec2f at offset 16
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 16,
                shader_location: 2,
            },
            // uv_br: vec2f at offset 24
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 24,
                shader_location: 3,
            },
            // color: u32 at offset 32
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 32,
                shader_location: 4,
            },
            // mode: u32 at offset 36
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 36,
                shader_location: 5,
            },
            // corner_radius: f32 at offset 40
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 40,
                shader_location: 6,
            },
            // texture_layer: u32 at offset 44
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 44,
                shader_location: 7,
            },
            // clip_rect: vec4f at offset 48
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 48,
                shader_location: 8,
            },
        ];

        // Render pipeline
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("strata_render_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &instance_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
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
            cache: None,
        });

        // Globals uniform buffer
        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strata_globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Glyph atlas texture
        let (atlas_width, atlas_height) = (glyph_atlas.atlas_width, glyph_atlas.atlas_height);
        let atlas_texture = create_rgba_texture(device, atlas_width, atlas_height, "strata_atlas");
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Upload initial atlas data
        upload_texture_region(
            queue,
            &atlas_texture,
            0,
            0,
            atlas_width,
            atlas_height,
            glyph_atlas.atlas_data(),
            atlas_width * 4,
        );

        // Samplers (linear filtering)
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("strata_atlas_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("strata_image_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // 1×1 white placeholder image atlas
        let image_atlas_texture =
            create_rgba_texture(device, 1, 1, "strata_image_atlas");
        let white_pixel: [u8; 4] = [255, 255, 255, 255];
        upload_texture_region(queue, &image_atlas_texture, 0, 0, 1, 1, &white_pixel, 4);
        let image_atlas_view =
            image_atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let image_atlas = ImageAtlas::new();

        // Instance buffer
        let initial_capacity = 4096;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strata_instances"),
            size: (initial_capacity * std::mem::size_of::<GpuInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group
        let bind_group = create_bind_group(
            device,
            &bind_group_layout,
            &globals_buffer,
            &atlas_view,
            &atlas_sampler,
            &image_atlas_view,
            &image_sampler,
        );

        let pipeline = StrataPipeline::new(glyph_atlas, image_atlas);

        Self {
            pipeline,
            render_pipeline,
            globals_buffer,
            atlas_texture,
            atlas_view,
            atlas_sampler,
            image_atlas_texture,
            image_atlas_view,
            image_sampler,
            instance_buffer,
            instance_capacity: initial_capacity,
            bind_group_layout,
            bind_group,
            surface_format,
        }
    }

    /// Load raw RGBA pixel data into the image atlas and return a handle.
    pub fn load_image_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> ImageHandle {
        let needs_regrow = self.pipeline.image_atlas_needs_grow(width, height);
        let handle = self.pipeline.load_image_rgba(width, height, data);

        if needs_regrow {
            let atlas = self.pipeline.image_atlas();
            self.image_atlas_texture =
                create_rgba_texture(device, atlas.width, atlas.height, "strata_image_atlas");
            upload_texture_region(
                queue,
                &self.image_atlas_texture,
                0,
                0,
                atlas.width,
                atlas.height,
                atlas.data(),
                atlas.width * 4,
            );
            self.image_atlas_view = self
                .image_atlas_texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            self.rebuild_bind_group(device);
        } else {
            upload_texture_region(
                queue,
                &self.image_atlas_texture,
                self.pipeline.image_atlas().last_placed().0,
                self.pipeline.image_atlas().last_placed().1,
                width,
                height,
                data,
                width * 4,
            );
        }

        handle
    }

    /// Prepare for rendering (upload data to GPU).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport_width: f32,
        viewport_height: f32,
    ) {
        // Check if glyph atlas was resized
        if self.pipeline.glyph_atlas().was_resized() {
            self.recreate_atlas_texture(device, queue);
            self.pipeline.glyph_atlas_mut().ack_resize();
            self.pipeline.glyph_atlas_mut().take_dirty_region();
            self.pipeline.invalidate_grid_row_cache();
        } else if let Some(dirty) = self.pipeline.glyph_atlas_mut().take_dirty_region() {
            self.upload_atlas_region(queue, dirty);
        }

        // Write globals
        let globals = Globals {
            transform: create_orthographic_matrix(viewport_width, viewport_height),
            atlas_size: [
                self.pipeline.glyph_atlas().atlas_width as f32,
                self.pipeline.glyph_atlas().atlas_height as f32,
            ],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        let instances = self.pipeline.instances();

        // Cap instance count
        const MAX_INSTANCES: usize = 2 * 1024 * 1024;
        let instance_count = instances.len().min(MAX_INSTANCES);

        // Resize instance buffer if needed
        if instance_count > self.instance_capacity {
            self.instance_capacity = instance_count.next_power_of_two().min(MAX_INSTANCES);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strata_instances"),
                size: (self.instance_capacity * std::mem::size_of::<GpuInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        // Write instance data
        if instance_count > 0 {
            let src = bytemuck::cast_slice::<GpuInstance, u8>(&instances[..instance_count]);
            queue.write_buffer(&self.instance_buffer, 0, src);
        }

        self.pipeline.truncate_instances(MAX_INSTANCES);
    }

    /// Render all instances.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
    ) {
        fn srgb_to_linear(c: f32) -> f64 {
            let c = c as f64;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }

        let bg = self.pipeline.background();

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("strata_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: srgb_to_linear(bg.r),
                        g: srgb_to_linear(bg.g),
                        b: srgb_to_linear(bg.b),
                        a: bg.a as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        let instances = self.pipeline.instances();
        if !instances.is_empty() {
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instances.len() as u32);
        }
    }

    fn recreate_atlas_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let (width, height) = (
            self.pipeline.glyph_atlas().atlas_width,
            self.pipeline.glyph_atlas().atlas_height,
        );
        self.atlas_texture = create_rgba_texture(device, width, height, "strata_atlas");
        self.atlas_view = self
            .atlas_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.upload_atlas_full(queue);
        self.rebuild_bind_group(device);
    }

    fn upload_atlas_region(&self, queue: &wgpu::Queue, region: (u32, u32, u32, u32)) {
        let (min_x, min_y, max_x, max_y) = region;
        let atlas_width = self.pipeline.glyph_atlas().atlas_width;
        let data = self.pipeline.glyph_atlas().atlas_data();
        let byte_offset = ((min_y * atlas_width + min_x) * 4) as usize;
        upload_texture_region(
            queue,
            &self.atlas_texture,
            min_x,
            min_y,
            max_x - min_x,
            max_y - min_y,
            &data[byte_offset..],
            atlas_width * 4,
        );
    }

    fn upload_atlas_full(&self, queue: &wgpu::Queue) {
        let atlas_width = self.pipeline.glyph_atlas().atlas_width;
        let atlas_height = self.pipeline.glyph_atlas().atlas_height;
        upload_texture_region(
            queue,
            &self.atlas_texture,
            0,
            0,
            atlas_width,
            atlas_height,
            self.pipeline.glyph_atlas().atlas_data(),
            atlas_width * 4,
        );
    }

    fn rebuild_bind_group(&mut self, device: &wgpu::Device) {
        self.bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &self.globals_buffer,
            &self.atlas_view,
            &self.atlas_sampler,
            &self.image_atlas_view,
            &self.image_sampler,
        );
    }
}

// =============================================================================
// wgpu helpers
// =============================================================================

/// Create a 2D RGBA8 sRGB texture.
fn create_rgba_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
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
    })
}

/// Upload a rectangle of pixel data to a wgpu texture.
fn upload_texture_region(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    data: &[u8],
    bytes_per_row: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Create the bind group for the shader.
fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals_buffer: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
    image_view: &wgpu::TextureView,
    image_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("strata_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(atlas_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(image_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(image_sampler),
            },
        ],
    })
}
