//! GPU-accelerated terminal renderer using custom shaders.
//!
//! Uses instanced rendering to draw all visible character cells in a single draw call.
//! Optimized for minimal GPU bandwidth and CPU overhead.

use iced::widget::shader::{self, wgpu, Storage};
use iced::{Length, Rectangle, Size};

use nexus_term::TerminalGrid;

use crate::glyph_cache::GlyphCache;

/// Compressed instance data for a single character cell (32 bytes instead of 64).
/// Colors are packed as u32 RGBA8, position uses grid coordinates.
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct CellInstance {
    /// Grid position (col, row) as u16, glyph size (w, h) as u16.
    pub grid_pos_size: [u16; 4],  // 8 bytes
    /// UV coordinates in atlas (u, v, w, h) as u16 (normalized * 65535).
    pub uv: [u16; 4],             // 8 bytes
    /// Foreground color as packed RGBA8.
    pub fg_color: u32,            // 4 bytes
    /// Background color as packed RGBA8.
    pub bg_color: u32,            // 4 bytes
    /// Font metric offsets (offset_x, offset_y) for glyph alignment.
    pub offsets: [i32; 2],        // 8 bytes
}
// Total: 32 bytes per instance (was 64)

/// Extracted cell data for GPU rendering (Send + Sync).
#[derive(Debug, Clone)]
pub struct CellData {
    pub c: char,
    pub fg: u32,  // Packed RGBA8
    pub bg: u32,  // Packed RGBA8
}

/// Pack [f32; 4] RGBA into u32.
#[inline]
fn pack_color(rgba: [f32; 4]) -> u32 {
    let r = (rgba[0] * 255.0) as u32;
    let g = (rgba[1] * 255.0) as u32;
    let b = (rgba[2] * 255.0) as u32;
    let a = (rgba[3] * 255.0) as u32;
    r | (g << 8) | (b << 16) | (a << 24)
}

/// Check if background is effectively transparent (alpha near zero).
#[inline]
fn is_transparent_bg(bg: u32) -> bool {
    // Alpha is in the high byte
    (bg >> 24) < 10  // Alpha < ~4%
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

        // Extract cell data for visible rows (pre-pack colors)
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
                    fg: pack_color(cell.fg.to_rgba(true)),
                    bg: pack_color(cell.bg.to_rgba(false)),
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
            let pipeline = create_pipeline(device, queue, format, physical_font_size);
            storage.store(pipeline);
        }

        let pipeline = storage.get_mut::<TerminalPipeline>().unwrap();

        // Scale bounds to physical pixels for GPU rendering
        let physical_bounds = Rectangle {
            x: bounds.x * scale,
            y: bounds.y * scale,
            width: bounds.width * scale,
            height: bounds.height * scale,
        };

        // Build instance data for visible cells (in physical pixels)
        // This may add new glyphs to the cache
        let instances = build_instances(
            &self.cells,
            self.cols,
            &mut pipeline.glyph_cache,
            physical_bounds,
        );

        // Partial texture upload if atlas changed
        if pipeline.glyph_cache.is_dirty() {
            upload_dirty_region(queue, pipeline);
            pipeline.glyph_cache.mark_clean();
        }

        // Update instance buffer
        if !instances.is_empty() {
            let instance_bytes = bytemuck::cast_slice(&instances);
            if instance_bytes.len() > pipeline.instance_buffer.size() as usize {
                // Need a bigger buffer - grow by 2x
                let new_size = (instance_bytes.len() * 2) as u64;
                pipeline.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Terminal Instance Buffer"),
                    size: new_size,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            queue.write_buffer(&pipeline.instance_buffer, 0, instance_bytes);
        }
        pipeline.instance_count = instances.len() as u32;

        // Update globals (transform matrix + cell metrics + origin + ascent)
        let target_size = viewport.physical_size();
        let globals = Globals {
            transform: create_transform(Size::new(target_size.width, target_size.height)),
            cell_size: [pipeline.glyph_cache.cell_width, pipeline.glyph_cache.cell_height],
            origin: [physical_bounds.x, physical_bounds.y],
            ascent: pipeline.glyph_cache.ascent,
            _padding: [0.0; 3],
        };
        queue.write_buffer(
            &pipeline.globals_buffer,
            0,
            bytemuck::bytes_of(&globals),
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

        if pipeline.instance_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Terminal Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

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
        pass.draw(0..6, 0..pipeline.instance_count);
    }
}

/// Globals uniform buffer data.
/// WGSL struct alignment requires size to be multiple of 16 (mat4x4 alignment).
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Globals {
    transform: [[f32; 4]; 4],  // 64 bytes
    cell_size: [f32; 2],       // 8 bytes
    origin: [f32; 2],          // 8 bytes - widget position for scrolling
    ascent: f32,               // 4 bytes
    _padding: [f32; 3],        // 12 bytes (pad to 96, multiple of 16)
}

fn create_pipeline(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, font_size: f32) -> TerminalPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Terminal Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/terminal.wgsl").into()),
    });

    // Globals bind group layout
    let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Terminal Globals Layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
                    // grid_pos_size (4x u16)
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint16x4,
                        offset: 0,
                        shader_location: 0,
                    },
                    // uv (4x u16)
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint16x4,
                        offset: 8,
                        shader_location: 1,
                    },
                    // fg_color (u32)
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32,
                        offset: 16,
                        shader_location: 2,
                    },
                    // bg_color (u32)
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32,
                        offset: 20,
                        shader_location: 3,
                    },
                    // offsets (vec2<i32>)
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Sint32x2,
                        offset: 24,
                        shader_location: 4,
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

    // Initial full upload
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        glyph_cache.atlas_data(),
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
    glyph_cache.mark_clean();

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

    // Create globals buffer (larger now with cell metrics)
    let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Terminal Globals Buffer"),
        size: std::mem::size_of::<Globals>() as u64,
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
        size: 4096 * std::mem::size_of::<CellInstance>() as u64,
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

/// Upload only the dirty region of the atlas texture.
fn upload_dirty_region(queue: &wgpu::Queue, pipeline: &mut TerminalPipeline) {
    let Some(region) = pipeline.glyph_cache.dirty_region_data() else {
        return;
    };

    // Upload row by row within the dirty region
    // (wgpu requires contiguous data, but our atlas rows aren't contiguous for subregions)
    let atlas_data = pipeline.glyph_cache.atlas_data();
    let bytes_per_row = region.atlas_width * 4;

    for row in 0..region.height {
        let y = region.y + row;
        let src_offset = (y * region.atlas_width + region.x) as usize * 4;
        let row_bytes = (region.width * 4) as usize;

        if src_offset + row_bytes <= atlas_data.len() {
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &pipeline.atlas_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: region.x,
                        y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &atlas_data[src_offset..src_offset + row_bytes],
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(region.width * 4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: region.width,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
}

fn build_instances(
    cells: &[CellData],
    cols: usize,
    glyph_cache: &mut GlyphCache,
    bounds: Rectangle,
) -> Vec<CellInstance> {
    // Pre-calculate inverse atlas dimensions (avoid division in hot loop)
    let (atlas_w, atlas_h) = glyph_cache.atlas_size();
    let inv_atlas_w = 65535.0 / atlas_w as f32;
    let inv_atlas_h = 65535.0 / atlas_h as f32;

    // Estimate capacity (assume ~50% non-empty cells)
    let mut instances = Vec::with_capacity(cells.len() / 2);

    for (idx, cell) in cells.iter().enumerate() {
        // Cull empty cells (space with transparent background)
        if (cell.c == ' ' || cell.c == '\0') && is_transparent_bg(cell.bg) {
            continue;
        }

        let row_idx = (idx / cols) as u16;
        let col_idx = (idx % cols) as u16;

        // Ensure glyph is cached and extract data
        let glyph = glyph_cache.get_glyph(cell.c);
        let glyph_width = glyph.width as u16;
        let glyph_height = glyph.height as u16;
        let atlas_x = glyph.atlas_x;
        let atlas_y = glyph.atlas_y;
        let offset_x = glyph.offset_x;
        let offset_y = glyph.offset_y;

        // Compute normalized UV (scaled to u16 range 0-65535)
        let u = (atlas_x as f32 * inv_atlas_w) as u16;
        let v = (atlas_y as f32 * inv_atlas_h) as u16;
        let uv_w = (glyph.width as f32 * inv_atlas_w) as u16;
        let uv_h = (glyph.height as f32 * inv_atlas_h) as u16;

        instances.push(CellInstance {
            grid_pos_size: [col_idx, row_idx, glyph_width, glyph_height],
            uv: [u, v, uv_w, uv_h],
            fg_color: cell.fg,
            bg_color: cell.bg,
            offsets: [offset_x, offset_y],
        });
    }

    // Store bounds offset in first instance if needed (or pass via uniform)
    // For now, bounds.x/y are passed through the transform matrix
    let _ = bounds;

    instances
}

fn create_transform(target_size: Size<u32>) -> [[f32; 4]; 4] {
    let width = target_size.width as f32;
    let height = target_size.height as f32;

    // Orthographic projection: pixel coords -> clip space
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
