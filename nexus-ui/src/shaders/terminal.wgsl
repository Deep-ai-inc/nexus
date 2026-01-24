// Terminal glyph rendering shader.
// Renders textured quads for each character cell using instanced rendering.
// Optimized: compressed instance data (32 bytes per cell).

struct Globals {
    transform: mat4x4<f32>,
    cell_size: vec2<f32>,
    origin: vec2<f32>,    // Widget position (x, y) for scrolling
    ascent: f32,
    _pad1: f32,           // Pad to 96 bytes (multiple of 16)
    _pad2: f32,
    _pad3: f32,
}

@group(0) @binding(0)
var<uniform> globals: Globals;

@group(1) @binding(0)
var atlas_texture: texture_2d<f32>;

@group(1) @binding(1)
var atlas_sampler: sampler;

struct CellInstance {
    @location(0) grid_pos_size: vec4<u32>,
    @location(1) uv: vec4<u32>,
    @location(2) fg_color: u32,
    @location(3) bg_color: u32,
    @location(4) offsets: vec2<i32>,  // Font metric offsets (offset_x, offset_y)
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
}

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
    instance: CellInstance,
) -> VertexOutput {
    var out: VertexOutput;

    let col = f32(instance.grid_pos_size.x);
    let row = f32(instance.grid_pos_size.y);
    let glyph_w = f32(instance.grid_pos_size.z);
    let glyph_h = f32(instance.grid_pos_size.w);

    // Font metric offsets
    let off_x = f32(instance.offsets.x);
    let off_y = f32(instance.offsets.y);

    let quad_pos = QUAD_VERTICES[vertex_index];

    // Calculate cell top-left based on grid + widget origin (for scrolling)
    let cell_x = globals.origin.x + col * globals.cell_size.x;
    let cell_y = globals.origin.y + row * globals.cell_size.y;

    // Apply font metrics for proper glyph alignment
    // Standard formula: y = baseline - offset_y - height
    // ascent pushes the baseline down from the top of the cell
    let glyph_x = cell_x + off_x;
    let glyph_y = cell_y + globals.ascent - off_y - glyph_h;

    let pos = vec2<f32>(glyph_x, glyph_y) + quad_pos * vec2<f32>(glyph_w, glyph_h);

    out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

    // UV Unpacking (normalized u16 -> f32)
    let uv_scale = 1.0 / 65535.0;
    let uv_x = f32(instance.uv.x) * uv_scale;
    let uv_y = f32(instance.uv.y) * uv_scale;
    let uv_w = f32(instance.uv.z) * uv_scale;
    let uv_h = f32(instance.uv.w) * uv_scale;

    out.uv = vec2<f32>(uv_x, uv_y) + quad_pos * vec2<f32>(uv_w, uv_h);
    out.fg_color = unpack_color(instance.fg_color);
    out.bg_color = unpack_color(instance.bg_color);

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let glyph = textureSample(atlas_texture, atlas_sampler, in.uv);

    // Alpha from atlas (glyph shape) * foreground alpha
    let alpha = glyph.a * in.fg_color.a;

    // Mix background and foreground based on glyph shape
    // This enables block cursors, selection highlights, and TUI app styling
    let rgb = mix(in.bg_color.rgb, in.fg_color.rgb, alpha);

    // Combine alphas: background + foreground (masked by glyph)
    let out_alpha = in.bg_color.a + alpha * (1.0 - in.bg_color.a);

    return vec4<f32>(rgb, out_alpha);
}
