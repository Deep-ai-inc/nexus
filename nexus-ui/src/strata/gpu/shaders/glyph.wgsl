// Strata glyph rendering shader.
// Renders textured quads for each glyph using instanced rendering.
// Unified pipeline for all text content.

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

struct GlyphInstance {
    @location(0) position: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv: vec4<u32>,
    @location(3) color: u32,
    @location(4) flags: u32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) selected: f32,
}

// Quad vertices (two triangles)
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
    instance: GlyphInstance,
) -> VertexOutput {
    var out: VertexOutput;

    let quad_pos = QUAD_VERTICES[vertex_index];

    // Position glyph quad
    let pos = instance.position + quad_pos * instance.size;
    out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

    // UV unpacking (normalized u16 -> f32)
    let uv_scale = 1.0 / 65535.0;
    let uv_x = f32(instance.uv.x) * uv_scale;
    let uv_y = f32(instance.uv.y) * uv_scale;
    let uv_w = f32(instance.uv.z) * uv_scale;
    let uv_h = f32(instance.uv.w) * uv_scale;

    out.uv = vec2<f32>(uv_x, uv_y) + quad_pos * vec2<f32>(uv_w, uv_h);
    out.color = unpack_color(instance.color);
    out.selected = f32(instance.flags & 1u);

    return out;
}

// Selection highlight color
const SELECTION_COLOR: vec3<f32> = vec3<f32>(0.3, 0.5, 0.8);
const SELECTION_ALPHA: f32 = 0.4;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let glyph = textureSample(atlas_texture, atlas_sampler, in.uv);

    // Alpha from atlas (glyph shape) * color alpha
    let alpha = glyph.a * in.color.a;

    // Use glyph color
    var rgb = in.color.rgb;

    // Apply selection highlight if selected
    if (in.selected > 0.5) {
        rgb = mix(rgb, SELECTION_COLOR, SELECTION_ALPHA);
    }

    return vec4<f32>(rgb, alpha);
}
