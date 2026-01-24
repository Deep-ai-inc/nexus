//! GPU-accelerated terminal renderer using custom shaders.
//!
//! Uses instanced rendering to draw all visible character cells in a single draw call.

use iced::widget::shader::{self, wgpu, Storage};
use iced::{Length, Rectangle, Size};

use nexus_term::TerminalGrid;

use crate::glyph_cache::GlyphCache;

/// Instance data for a single character cell (sent to GPU).
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct CellInstance {
    /// Position (x, y) and size (w, h) in pixels.
    pub pos_size: [f32; 4],
    /// UV coordinates in atlas (u, v, w, h).
    pub uv: [f32; 4],
    /// Foreground color (RGBA).
    pub fg_color: [f32; 4],
    /// Background color (RGBA).
    pub bg_color: [f32; 4],
}

/// Extracted cell data for GPU rendering (Send + Sync).
#[derive(Debug, Clone)]
pub struct CellData {
    pub c: char,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
}

/// Shader program for terminal rendering.
pub struct TerminalProgram<Message> {
    /// Extracted cell data for visible rows.
    cells: Vec<CellData>,
    /// Number of columns.
    cols: usize,
    /// Font size.
    font_size: f32,
    /// Number of rows in this batch.
    num_rows: usize,
    /// Phantom for Message type.
    _message: std::marker::PhantomData<Message>,
}

impl<Message> TerminalProgram<Message> {
    pub fn new(
        grid: &TerminalGrid,
        font_size: f32,
        first_row: usize,
        last_row: usize,
    ) -> Self {
        let (cols, _) = grid.size();
        let cols = cols as usize;
        let grid_cells = grid.cells();

        // Extract cell data for visible rows
        let mut cells = Vec::with_capacity((last_row - first_row) * cols);
        for row_idx in first_row..last_row {
            let row_start = row_idx * cols;
            let row_end = (row_start + cols).min(grid_cells.len());
            if row_start >= grid_cells.len() {
                break;
            }
            for cell in &grid_cells[row_start..row_end] {
                cells.push(CellData {
                    c: if cell.c == '\0' { ' ' } else { cell.c },
                    fg: cell.fg.to_rgba(true),
                    bg: cell.bg.to_rgba(false),
                });
            }
        }

        Self {
            cells,
            cols,
            font_size,
            num_rows: last_row - first_row,
            _message: std::marker::PhantomData,
        }
    }
}

/// Pipeline state stored between frames.
pub struct TerminalPipeline {
    pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_count: u32,
    glyph_cache: GlyphCache,
}

impl<Message: Clone + 'static> shader::Program<Message> for TerminalProgram<Message> {
    type State = ();
    type Primitive = TerminalPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        TerminalPrimitive {
            cells: self.cells.clone(),
            cols: self.cols,
            font_size: self.font_size,
            num_rows: self.num_rows,
            bounds,
        }
    }
}

/// Primitive that carries data to the GPU renderer (Send + Sync).
#[derive(Debug, Clone)]
pub struct TerminalPrimitive {
    cells: Vec<CellData>,
    cols: usize,
    font_size: f32,
    num_rows: usize,
    bounds: Rectangle,
}

impl shader::Primitive for TerminalPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut Storage,
        bounds: &Rectangle,
        viewport: &iced::advanced::graphics::Viewport,
    ) {
        // Get HiDPI scale factor
        let scale = viewport.scale_factor() as f32;
        let physical_font_size = self.font_size * scale;

        // Get or create the pipeline (with scaled font size for crisp rendering)
        if !storage.has::<TerminalPipeline>() {
            let pipeline = create_pipeline(device, format, physical_font_size);
            storage.store(pipeline);
        }

        let pipeline = storage.get_mut::<TerminalPipeline>().unwrap();

        // Update atlas if glyph cache is dirty
        if pipeline.glyph_cache.is_dirty() {
            update_atlas_texture(queue, pipeline);
            pipeline.glyph_cache.mark_clean();
        }

        // Scale bounds to physical pixels for GPU rendering
        let physical_bounds = Rectangle {
            x: bounds.x * scale,
            y: bounds.y * scale,
            width: bounds.width * scale,
            height: bounds.height * scale,
        };

        // Build instance data for visible cells (in physical pixels)
        let instances = build_instances(
            &self.cells,
            self.cols,
            self.num_rows,
            &mut pipeline.glyph_cache,
            physical_bounds,
        );

        // Re-upload atlas if new glyphs were added
        if pipeline.glyph_cache.is_dirty() {
            update_atlas_texture(queue, pipeline);
            pipeline.glyph_cache.mark_clean();
        }

        // Update instance buffer
        let instance_bytes = bytemuck::cast_slice(&instances);
        if instance_bytes.len() > pipeline.instance_buffer.size() as usize {
            // Need a bigger buffer
            pipeline.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Terminal Instance Buffer"),
                size: instance_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&pipeline.instance_buffer, 0, instance_bytes);
        pipeline.instance_count = instances.len() as u32;

        // Update globals (transform matrix)
        let target_size = viewport.physical_size();
        let transform = create_transform(Size::new(target_size.width, target_size.height));
        queue.write_buffer(
            &pipeline.globals_buffer,
            0,
            bytemuck::cast_slice(&transform),
        );
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(pipeline) = storage.get::<TerminalPipeline>() else {
            return;
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Terminal Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load, // Don't clear - we're rendering over existing content
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // Set scissor rect to clip rendering to bounds
        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );

        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.globals_bind_group, &[]);
        pass.set_bind_group(1, &pipeline.atlas_bind_group, &[]);
        pass.set_vertex_buffer(0, pipeline.instance_buffer.slice(..));
        pass.draw(0..6, 0..pipeline.instance_count); // 6 vertices per quad
    }
}

fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat, font_size: f32) -> TerminalPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Terminal Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/terminal.wgsl").into()),
    });

    // Globals bind group layout
    let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Terminal Globals Layout"),
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

    // Atlas bind group layout
    let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Terminal Atlas Layout"),
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

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Terminal Pipeline Layout"),
        bind_group_layouts: &[&globals_layout, &atlas_layout],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Terminal Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<CellInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    // pos_size
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    },
                    // uv
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 16,
                        shader_location: 1,
                    },
                    // fg_color
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 32,
                        shader_location: 2,
                    },
                    // bg_color
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 48,
                        shader_location: 3,
                    },
                ],
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
    });

    // Create glyph cache and pre-cache ASCII
    let mut glyph_cache = GlyphCache::new(font_size);
    glyph_cache.precache_ascii();

    // Create atlas texture
    let (atlas_width, atlas_height) = glyph_cache.atlas_size();
    let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Glyph Atlas"),
        size: wgpu::Extent3d {
            width: atlas_width,
            height: atlas_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Atlas Bind Group"),
        layout: &atlas_layout,
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

    // Create globals buffer
    let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Terminal Globals Buffer"),
        size: 64, // 4x4 matrix
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Globals Bind Group"),
        layout: &globals_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: globals_buffer.as_entire_binding(),
        }],
    });

    // Create instance buffer (start with reasonable size)
    let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Terminal Instance Buffer"),
        size: 1024 * std::mem::size_of::<CellInstance>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    TerminalPipeline {
        pipeline,
        globals_buffer,
        globals_bind_group,
        atlas_texture,
        atlas_bind_group,
        instance_buffer,
        instance_count: 0,
        glyph_cache,
    }
}

fn update_atlas_texture(queue: &wgpu::Queue, pipeline: &mut TerminalPipeline) {
    let (width, height) = pipeline.glyph_cache.atlas_size();
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &pipeline.atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pipeline.glyph_cache.atlas_data(),
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

fn build_instances(
    cells: &[CellData],
    cols: usize,
    num_rows: usize,
    glyph_cache: &mut GlyphCache,
    bounds: Rectangle,
) -> Vec<CellInstance> {
    let cell_width = glyph_cache.cell_width;
    let cell_height = glyph_cache.cell_height;
    let ascent = glyph_cache.ascent;
    let atlas_width = glyph_cache.atlas_size().0 as f32;
    let atlas_height = glyph_cache.atlas_size().1 as f32;

    let mut instances = Vec::with_capacity(cells.len());

    for (idx, cell) in cells.iter().enumerate() {
        let row_idx = idx / cols;
        let col_idx = idx % cols;

        // Ensure glyph is cached and extract data
        let glyph = glyph_cache.get_glyph(cell.c);
        let offset_x = glyph.offset_x;
        let offset_y = glyph.offset_y;
        let glyph_width = glyph.width;
        let glyph_height = glyph.height;
        let atlas_x = glyph.atlas_x;
        let atlas_y = glyph.atlas_y;
        // Drop the borrow here by ending the scope of glyph reference

        // Compute UV from atlas position
        let u = atlas_x as f32 / atlas_width;
        let v = atlas_y as f32 / atlas_height;
        let uv_w = glyph_width as f32 / atlas_width;
        let uv_h = glyph_height as f32 / atlas_height;

        // Calculate screen position
        let x = bounds.x + col_idx as f32 * cell_width;
        let y = bounds.y + row_idx as f32 * cell_height;

        // Adjust position based on glyph metrics
        let glyph_x = x + offset_x as f32;
        let glyph_y = y + ascent - offset_y as f32 - glyph_height as f32;

        instances.push(CellInstance {
            pos_size: [glyph_x, glyph_y, glyph_width as f32, glyph_height as f32],
            uv: [u, v, uv_w, uv_h],
            fg_color: cell.fg,
            bg_color: cell.bg,
        });
    }

    let _ = num_rows; // Suppress unused warning
    instances
}

fn create_transform(target_size: Size<u32>) -> [[f32; 4]; 4] {
    // Create orthographic projection matrix
    let width = target_size.width as f32;
    let height = target_size.height as f32;

    // Transform from pixel coordinates to clip space (-1 to 1)
    [
        [2.0 / width, 0.0, 0.0, 0.0],
        [0.0, -2.0 / height, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0, 1.0],
    ]
}

/// Widget that wraps the shader program.
pub struct TerminalShader<Message> {
    program: TerminalProgram<Message>,
    height: f32,
}

impl<Message: Clone + 'static> TerminalShader<Message> {
    pub fn new(grid: &TerminalGrid, font_size: f32, first_row: usize, last_row: usize, cell_height: f32) -> Self {
        let program = TerminalProgram::new(grid, font_size, first_row, last_row);
        let height = (last_row - first_row) as f32 * cell_height;
        Self { program, height }
    }

    /// Create the shader widget.
    pub fn widget(self) -> shader::Shader<Message, TerminalProgram<Message>> {
        shader::Shader::new(self.program)
            .width(Length::Fill)
            .height(Length::Fixed(self.height))
    }
}
