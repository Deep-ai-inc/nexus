// Strata unified rendering shader (ubershader).
//
// Renders all 2D content in a single draw call using the "white pixel" trick:
// - Glyphs: Sample glyph alpha from atlas, multiply by glyph color
// - Solid quads (selection, backgrounds): Sample the 1x1 white pixel at (0,0),
//   so atlas_alpha = 1.0, and final color = quad_color * 1.0 = quad_color
//
// This eliminates pipeline switches and enables perfect Z-ordering by
// simply ordering instances in the buffer.

struct Globals {
    transform: mat4x4<f32>,
    atlas_size: vec2<f32>,
    _padding: vec2<f32>,
}

@group(0) @binding(0)
var<uniform> globals: Globals;

@group(1) @binding(0)
var atlas_texture: texture_2d<f32>;

@group(1) @binding(1)
var atlas_sampler: sampler;

struct Instance {
    @location(0) position: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv: vec4<u32>,   // [uv_x, uv_y, uv_w, uv_h] as normalized u16
    @location(3) color: u32,      // Packed RGBA8
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

// Quad vertices (two triangles forming a quad)
var<private> QUAD_VERTICES: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

fn unpack_color(packed: u32) -> vec4<f32> {
    let r = f32(packed & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let b = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    instance: Instance,
) -> VertexOutput {
    var out: VertexOutput;

    let quad_pos = QUAD_VERTICES[vertex_index];

    // Position the quad
    let pos = instance.position + quad_pos * instance.size;
    out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

    // Unpack UV coordinates (normalized u16 -> f32)
    let uv_scale = 1.0 / 65535.0;
    let uv_x = f32(instance.uv.x) * uv_scale;
    let uv_y = f32(instance.uv.y) * uv_scale;
    let uv_w = f32(instance.uv.z) * uv_scale;
    let uv_h = f32(instance.uv.w) * uv_scale;

    // Interpolate UV across quad
    out.uv = vec2<f32>(uv_x, uv_y) + quad_pos * vec2<f32>(uv_w, uv_h);
    out.color = unpack_color(instance.color);

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample the atlas.
    // - For glyphs: gets the glyph's alpha mask
    // - For solid quads: samples the white pixel (alpha = 1.0)
    let atlas_alpha = textureSample(atlas_texture, atlas_sampler, in.uv).a;

    // Final color = instance color * atlas alpha
    // This is branchless and handles both glyphs and solid quads uniformly.
    return vec4<f32>(in.color.rgb, in.color.a * atlas_alpha);
}
