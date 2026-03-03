//! Metal GPU backend for Strata rendering.
//!
//! Manages Metal-specific GPU resources (device, pipeline state, buffers, textures)
//! and delegates instance building / caching to the platform-independent `StrataPipeline`.

use cosmic_text::FontSystem;

use super::glyph_atlas::GlyphAtlas;
use super::pipeline::{
    create_orthographic_matrix, Globals, GpuInstance, ImageAtlas, ImageHandle, StrataPipeline,
    MAX_FRAMES_IN_FLIGHT,
};
use crate::primitives::Color;

/// Metal-backed GPU renderer.
///
/// Wraps the platform-independent `StrataPipeline` and manages Metal GPU resources.
/// The shell accesses drawing methods via the `pipeline` field.
pub struct MetalRenderer {
    /// Platform-independent pipeline (instance building, caching, text shaping).
    pub pipeline: StrataPipeline,
    /// Metal render pipeline state (compiled shaders + vertex descriptor).
    metal_pipeline: metal::RenderPipelineState,
    /// Triple-buffered globals uniform (one per in-flight frame).
    globals_buffers: Vec<metal::Buffer>,
    /// Glyph atlas GPU texture.
    atlas_texture: metal::Texture,
    /// Glyph atlas sampler (linear filtering).
    atlas_sampler: metal::SamplerState,
    /// Image atlas GPU texture.
    image_atlas_texture: metal::Texture,
    /// Image atlas sampler (linear filtering).
    image_sampler: metal::SamplerState,
    /// Triple-buffered instance vertex buffer (one per in-flight frame).
    instance_buffers: Vec<metal::Buffer>,
    /// Current instance buffer capacity (number of GpuInstances).
    instance_capacity: usize,
    /// Frame index for triple-buffer slot selection (frame_index % 3).
    frame_index: u64,
}

impl MetalRenderer {
    /// Compile the Metal shader library. Call once at init, pass to `new()`.
    pub fn compile_library(device: &metal::DeviceRef) -> metal::Library {
        let options = metal::CompileOptions::new();
        device
            .new_library_with_source(include_str!("shaders/glyph.metal"), &options)
            .expect("Failed to compile Metal shader")
    }

    /// Create a new Metal renderer with a pre-compiled shader library.
    pub fn new(
        device: &metal::DeviceRef,
        library: &metal::Library,
        format: metal::MTLPixelFormat,
        font_size: f32,
        font_system: &mut FontSystem,
    ) -> Self {
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
            (0, metal::MTLVertexFormat::Float2),  // pos
            (8, metal::MTLVertexFormat::Float2),  // size
            (16, metal::MTLVertexFormat::Float2), // uv_tl
            (24, metal::MTLVertexFormat::Float2), // uv_br
            (32, metal::MTLVertexFormat::UInt),   // color
            (36, metal::MTLVertexFormat::UInt),   // mode
            (40, metal::MTLVertexFormat::Float),  // corner_radius
            (44, metal::MTLVertexFormat::UInt),   // texture_layer
            (48, metal::MTLVertexFormat::Float4), // clip_rect
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
        let color_attach = rpd.color_attachments().object_at(0).unwrap();
        color_attach.set_pixel_format(format);
        color_attach.set_blending_enabled(true);
        color_attach.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
        color_attach.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
        color_attach.set_source_rgb_blend_factor(metal::MTLBlendFactor::SourceAlpha);
        color_attach
            .set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
        color_attach.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
        color_attach
            .set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

        let metal_pipeline = device
            .new_render_pipeline_state(&rpd)
            .expect("Failed to create render pipeline state");

        // Triple-buffered globals (uniform) buffers
        let globals_size = std::mem::size_of::<Globals>() as u64;
        let globals_buffers: Vec<metal::Buffer> = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
                device.new_buffer(globals_size, metal::MTLResourceOptions::StorageModeShared)
            })
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

        let image_atlas = ImageAtlas::new();

        // Triple-buffered instance buffers
        let initial_capacity = 4096;
        let instance_buf_size =
            (initial_capacity * std::mem::size_of::<GpuInstance>()) as u64;
        let instance_buffers: Vec<metal::Buffer> = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
                device
                    .new_buffer(instance_buf_size, metal::MTLResourceOptions::StorageModeShared)
            })
            .collect();

        let pipeline = StrataPipeline::new(glyph_atlas, image_atlas);

        Self {
            pipeline,
            metal_pipeline,
            globals_buffers,
            atlas_texture,
            atlas_sampler,
            image_atlas_texture: placeholder_texture,
            image_sampler,
            instance_buffers,
            instance_capacity: initial_capacity,
            frame_index: 0,
        }
    }

    /// Load raw RGBA pixel data into the image atlas and return a handle for rendering.
    pub fn load_image_rgba(
        &mut self,
        device: &metal::DeviceRef,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> ImageHandle {
        let needs_regrow = self.pipeline.image_atlas_needs_grow(width, height);
        let handle = self.pipeline.load_image_rgba(width, height, data);

        if needs_regrow {
            let atlas = self.pipeline.image_atlas();
            self.image_atlas_texture =
                create_rgba_texture(device, atlas.width, atlas.height);
            upload_texture_region(
                &self.image_atlas_texture,
                0,
                0,
                atlas.width,
                atlas.height,
                atlas.data(),
                atlas.width * 4,
            );
        } else {
            // Upload just the new image region
            let atlas = self.pipeline.image_atlas();
            let img_meta = atlas.last_placed();
            upload_texture_region(
                &self.image_atlas_texture,
                img_meta.0,
                img_meta.1,
                width,
                height,
                data,
                width * 4,
            );
        }

        handle
    }

    /// Load a PNG image and return a handle for rendering.
    pub fn load_image_png(
        &mut self,
        device: &metal::DeviceRef,
        path: &std::path::Path,
    ) -> ImageHandle {
        let img = image::open(path)
            .unwrap_or_else(|e| panic!("Failed to load image {}: {}", path.display(), e))
            .to_rgba8();
        let (w, h) = img.dimensions();
        self.load_image_rgba(device, w, h, &img.into_raw())
    }

    /// Prepare for rendering (upload data to GPU).
    pub fn prepare(
        &mut self,
        device: &metal::DeviceRef,
        viewport_width: f32,
        viewport_height: f32,
    ) {
        // Check if glyph atlas was resized
        if self.pipeline.glyph_atlas().was_resized() {
            self.recreate_atlas_texture(device);
            self.pipeline.glyph_atlas_mut().ack_resize();
            self.pipeline.glyph_atlas_mut().take_dirty_region();
            self.pipeline.invalidate_grid_row_cache();
        } else if let Some(dirty) = self.pipeline.glyph_atlas_mut().take_dirty_region() {
            self.upload_atlas_region(dirty);
        }

        let slot = (self.frame_index % MAX_FRAMES_IN_FLIGHT as u64) as usize;

        // Write globals
        let globals = Globals {
            transform: create_orthographic_matrix(viewport_width, viewport_height),
            atlas_size: [
                self.pipeline.glyph_atlas().atlas_width as f32,
                self.pipeline.glyph_atlas().atlas_height as f32,
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

        let instances = self.pipeline.instances();

        // Cap instance count
        const MAX_INSTANCES: usize = 2 * 1024 * 1024;
        let instance_count = instances.len().min(MAX_INSTANCES);

        // Resize all 3 instance buffers if needed
        if instance_count > self.instance_capacity {
            self.instance_capacity = instance_count.next_power_of_two().min(MAX_INSTANCES);
            let buf_size =
                (self.instance_capacity * std::mem::size_of::<GpuInstance>()) as u64;
            self.instance_buffers = (0..MAX_FRAMES_IN_FLIGHT)
                .map(|_| {
                    device.new_buffer(
                        buf_size,
                        metal::MTLResourceOptions::StorageModeShared,
                    )
                })
                .collect();
        }

        // Write instance data
        if instance_count > 0 {
            let src =
                bytemuck::cast_slice::<GpuInstance, u8>(&instances[..instance_count]);
            unsafe {
                let dst = self.instance_buffers[slot].contents() as *mut u8;
                std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
            }
        }

        self.pipeline.truncate_instances(MAX_INSTANCES);
    }

    /// Render all instances in a single draw call.
    pub fn render(
        &self,
        command_buffer: &metal::CommandBufferRef,
        target: &metal::TextureRef,
        clip_bounds: &crate::shell::ClipBounds,
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

        let rpd = metal::RenderPassDescriptor::new();
        let color_attach = rpd.color_attachments().object_at(0).unwrap();
        color_attach.set_texture(Some(target));
        color_attach.set_load_action(metal::MTLLoadAction::Clear);
        color_attach.set_store_action(metal::MTLStoreAction::Store);
        color_attach.set_clear_color(metal::MTLClearColor::new(
            srgb_to_linear(bg.r),
            srgb_to_linear(bg.g),
            srgb_to_linear(bg.b),
            bg.a as f64,
        ));

        let encoder = command_buffer.new_render_command_encoder(&rpd);
        encoder.set_scissor_rect(metal::MTLScissorRect {
            x: clip_bounds.x as u64,
            y: clip_bounds.y as u64,
            width: clip_bounds.width as u64,
            height: clip_bounds.height as u64,
        });

        let instances = self.pipeline.instances();
        if !instances.is_empty() {
            let slot = (self.frame_index % MAX_FRAMES_IN_FLIGHT as u64) as usize;
            encoder.set_render_pipeline_state(&self.metal_pipeline);
            encoder.set_vertex_buffer(0, Some(&self.instance_buffers[slot]), 0);
            encoder.set_vertex_buffer(1, Some(&self.globals_buffers[slot]), 0);
            encoder.set_fragment_buffer(0, Some(&self.globals_buffers[slot]), 0);
            encoder.set_fragment_texture(0, Some(&self.atlas_texture));
            encoder.set_fragment_sampler_state(0, Some(&self.atlas_sampler));
            encoder.set_fragment_texture(1, Some(&self.image_atlas_texture));
            encoder.set_fragment_sampler_state(1, Some(&self.image_sampler));
            encoder.draw_primitives_instanced(
                metal::MTLPrimitiveType::Triangle,
                0,
                6,
                instances.len() as u64,
            );
        }

        encoder.end_encoding();
    }

    /// Advance the triple-buffer frame index.
    pub fn advance_frame(&mut self) {
        self.frame_index += 1;
    }

    fn recreate_atlas_texture(&mut self, device: &metal::DeviceRef) {
        let (width, height) = (
            self.pipeline.glyph_atlas().atlas_width,
            self.pipeline.glyph_atlas().atlas_height,
        );
        self.atlas_texture = create_rgba_texture(device, width, height);
        self.upload_atlas_full();
    }

    fn upload_atlas_region(&self, region: (u32, u32, u32, u32)) {
        let (min_x, min_y, max_x, max_y) = region;
        let atlas_width = self.pipeline.glyph_atlas().atlas_width;
        let data = self.pipeline.glyph_atlas().atlas_data();
        let byte_offset = ((min_y * atlas_width + min_x) * 4) as usize;
        upload_texture_region(
            &self.atlas_texture,
            min_x,
            min_y,
            max_x - min_x,
            max_y - min_y,
            &data[byte_offset..],
            atlas_width * 4,
        );
    }

    fn upload_atlas_full(&self) {
        let atlas_width = self.pipeline.glyph_atlas().atlas_width;
        let atlas_height = self.pipeline.glyph_atlas().atlas_height;
        upload_texture_region(
            &self.atlas_texture,
            0,
            0,
            atlas_width,
            atlas_height,
            self.pipeline.glyph_atlas().atlas_data(),
            atlas_width * 4,
        );
    }
}

// =============================================================================
// Metal helpers
// =============================================================================

/// Create a 2D RGBA8 sRGB texture with shared storage.
fn create_rgba_texture(
    device: &metal::DeviceRef,
    width: u32,
    height: u32,
) -> metal::Texture {
    let desc = metal::TextureDescriptor::new();
    desc.set_width(width as u64);
    desc.set_height(height as u64);
    desc.set_pixel_format(metal::MTLPixelFormat::RGBA8Unorm_sRGB);
    desc.set_storage_mode(metal::MTLStorageMode::Shared);
    desc.set_usage(metal::MTLTextureUsage::ShaderRead);
    device.new_texture(&desc)
}

/// Upload a rectangle of pixel data to a Metal texture.
fn upload_texture_region(
    texture: &metal::TextureRef,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    data: &[u8],
    bytes_per_row: u32,
) {
    let region = metal::MTLRegion {
        origin: metal::MTLOrigin {
            x: x as u64,
            y: y as u64,
            z: 0,
        },
        size: metal::MTLSize {
            width: width as u64,
            height: height as u64,
            depth: 1,
        },
    };
    texture.replace_region(
        region,
        0,
        data.as_ptr() as *const std::ffi::c_void,
        bytes_per_row as u64,
    );
}
