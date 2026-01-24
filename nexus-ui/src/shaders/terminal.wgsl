// Terminal glyph rendering shader.
// Renders textured quads for each character cell using instanced rendering.

// Uniforms passed from Iced
struct Globals {
    transform: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> globals: Globals;

// Glyph atlas texture
@group(1) @binding(0)
var atlas_texture: texture_2d<f32>;

@group(1) @binding(1)
var atlas_sampler: sampler;

// Per-instance data for each character cell
struct CellInstance {
    // Screen position (x, y) and size (w, h) in pixels
    @location(0) pos_size: vec4<f32>,
    // UV coordinates in atlas (u, v, w, h)
    @location(1) uv: vec4<f32>,
    // Foreground color (RGBA)
    @location(2) fg_color: vec4<f32>,
    // Background color (RGBA)
    @location(3) bg_color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
}

// Quad vertices (two triangles)
var<private> QUAD_VERTICES: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), // Top-left
    vec2<f32>(1.0, 0.0), // Top-right
    vec2<f32>(0.0, 1.0), // Bottom-left
    vec2<f32>(1.0, 0.0), // Top-right
    vec2<f32>(1.0, 1.0), // Bottom-right
    vec2<f32>(0.0, 1.0), // Bottom-left
);

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
    instance: CellInstance,
) -> VertexOutput {
    var out: VertexOutput;

    // Get quad vertex position (0-1 range)
    let quad_pos = QUAD_VERTICES[vertex_index];

    // Scale to cell size and translate to cell position
    let pos = instance.pos_size.xy + quad_pos * instance.pos_size.zw;

    // Apply transform (Iced's projection matrix)
    out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

    // Calculate UV for this vertex
    out.uv = instance.uv.xy + quad_pos * instance.uv.zw;

    out.fg_color = instance.fg_color;
    out.bg_color = instance.bg_color;

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample glyph alpha from atlas
    let glyph = textureSample(atlas_texture, atlas_sampler, in.uv);

    // Use glyph alpha to blend foreground over background
    let alpha = glyph.a;
    let color = mix(in.bg_color.rgb, in.fg_color.rgb, alpha);

    // Output with background alpha (for transparency support)
    return vec4<f32>(color, in.bg_color.a + alpha * (1.0 - in.bg_color.a));
}
