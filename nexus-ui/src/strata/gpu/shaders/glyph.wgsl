// Strata unified rendering shader (ubershader).
//
// Renders all 2D content in a single draw call:
// - Mode 0 (Quad): Glyphs, solid quads, rounded rects, circles
// - Mode 1 (Line): Thick line segments rendered as rotated quads
//
// Uses the "white pixel" trick: a 1x1 white pixel at atlas (0,0) enables
// solid quads to render with the same shader path as textured glyphs.

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
    @location(2) uv: vec4<u32>,       // [uv_x, uv_y, uv_w, uv_h] as normalized u16
    @location(3) color: u32,          // Packed RGBA8
    @location(4) corner_radius: f32,  // Mode 0: corner radius. Mode 1: line thickness.
    @location(5) mode: u32,           // 0 = Quad, 1 = Line Segment
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local_pos: vec2<f32>,  // Position within quad (0-1)
    @location(3) size: vec2<f32>,       // Quad size in pixels (line: length, thickness)
    @location(4) corner_radius: f32,
    @location(5) @interpolate(flat) mode: u32,  // Low 8 bits: render mode, bits 8..15: line style
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
    out.color = unpack_color(instance.color);
    out.mode = instance.mode;

    let quad_pos = QUAD_VERTICES[vertex_index];
    let base_mode = instance.mode & 0xFFu;

    if (base_mode == 1u) {
        // ============================================================
        // MODE 1: LINE SEGMENT
        // position = P1, size = P2, corner_radius = thickness
        // Expand endpoints into a rotated quad (thick line).
        // ============================================================
        let p1 = instance.position;
        let p2 = instance.size;  // P2 stored in size field
        let thickness = instance.corner_radius;

        let delta = p2 - p1;
        let len = length(delta);

        // Direction and perpendicular normal
        var dir: vec2<f32>;
        var normal: vec2<f32>;
        if (len > 0.001) {
            dir = delta / len;
            normal = vec2<f32>(-dir.y, dir.x);
        } else {
            dir = vec2<f32>(1.0, 0.0);
            normal = vec2<f32>(0.0, 1.0);
        }

        let half_thick = thickness * 0.5;

        // 4 corners of the line quad:
        //   c0 = p1 - normal * half_thick  (bottom-left)
        //   c1 = p1 + normal * half_thick  (top-left)
        //   c2 = p2 - normal * half_thick  (bottom-right)
        //   c3 = p2 + normal * half_thick  (top-right)
        //
        // Map quad_pos (0,0)→c0, (1,0)→c2, (0,1)→c1, (1,1)→c3
        let along = mix(p1, p2, vec2<f32>(quad_pos.x));
        let offset = normal * half_thick * (quad_pos.y * 2.0 - 1.0);
        let pos = along + offset;

        out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

        // UV: sample white pixel (solid color)
        let uv_scale = 1.0 / 65535.0;
        let uv_x = f32(instance.uv.x) * uv_scale;
        let uv_y = f32(instance.uv.y) * uv_scale;
        out.uv = vec2<f32>(uv_x, uv_y);

        // Pass line dimensions for potential AA in fragment shader
        out.local_pos = quad_pos;
        out.size = vec2<f32>(len, thickness);
        out.corner_radius = 0.0;
    } else {
        // ============================================================
        // MODE 0: STANDARD QUAD (text, solid rects, rounded rects)
        // ============================================================
        let pos = instance.position + quad_pos * instance.size;
        out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

        // Unpack UV coordinates (normalized u16 -> f32)
        let uv_scale = 1.0 / 65535.0;
        let uv_x = f32(instance.uv.x) * uv_scale;
        let uv_y = f32(instance.uv.y) * uv_scale;
        let uv_w = f32(instance.uv.z) * uv_scale;
        let uv_h = f32(instance.uv.w) * uv_scale;

        out.uv = vec2<f32>(uv_x, uv_y) + quad_pos * vec2<f32>(uv_w, uv_h);

        out.local_pos = quad_pos;
        out.size = instance.size;
        out.corner_radius = instance.corner_radius;
    }

    return out;
}

// Signed Distance Function for a rounded rectangle.
// Returns negative inside, positive outside, zero on edge.
// p: position relative to center of rect
// b: half-size of rect (width/2, height/2)
// r: corner radius
fn sdf_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_mode = in.mode & 0xFFu;

    if (base_mode == 1u) {
        // ============================================================
        // MODE 1: LINE SEGMENT
        // Solid, dashed, or dotted with edge anti-aliasing.
        // ============================================================
        let line_style = (in.mode >> 8u) & 0xFFu;
        let atlas_alpha = textureSample(atlas_texture, atlas_sampler, in.uv).a;
        var alpha = in.color.a * atlas_alpha;

        // Anti-alias along the perpendicular (y) axis of the line
        // local_pos.y is 0..1 across the thickness
        let dist_from_center = abs(in.local_pos.y - 0.5) * 2.0; // 0 at center, 1 at edge
        let edge_aa = fwidth(dist_from_center);
        let line_alpha = 1.0 - smoothstep(1.0 - edge_aa, 1.0, dist_from_center);
        alpha = alpha * line_alpha;

        // Apply line style pattern along the line direction
        // in.local_pos.x is 0..1 along the segment, in.size.x is length in pixels
        let pixel_along = in.local_pos.x * in.size.x;

        if (line_style == 1u) {
            // DASHED: 8px dash, 6px gap (14px period)
            let pattern_pos = pixel_along % 14.0;
            let dash_edge = fwidth(pixel_along);
            // Smooth transition at dash boundaries for AA
            let dash_alpha = smoothstep(8.0, 8.0 + dash_edge, pattern_pos)
                           * (1.0 - smoothstep(14.0 - dash_edge, 14.0, pattern_pos));
            alpha = alpha * (1.0 - dash_alpha);
        } else if (line_style == 2u) {
            // DOTTED: 2px dot, 4px gap (6px period)
            let pattern_pos = pixel_along % 6.0;
            let dot_edge = fwidth(pixel_along);
            let dot_alpha = smoothstep(2.0, 2.0 + dot_edge, pattern_pos)
                          * (1.0 - smoothstep(6.0 - dot_edge, 6.0, pattern_pos));
            alpha = alpha * (1.0 - dot_alpha);
        }

        return vec4<f32>(in.color.rgb, alpha);
    }

    // ============================================================
    // MODE 0: STANDARD QUAD
    // ============================================================
    // Sample the atlas.
    let atlas_alpha = textureSample(atlas_texture, atlas_sampler, in.uv).a;

    // Start with atlas-modulated alpha
    var alpha = in.color.a * atlas_alpha;

    // Apply SDF rounded corner mask if corner_radius > 0
    if (in.corner_radius > 0.0) {
        // Convert local_pos (0-1) to position relative to rect center
        let centered_pos = (in.local_pos - 0.5) * in.size;
        let half_size = in.size * 0.5;

        // Calculate signed distance to rounded rect edge
        let dist = sdf_rounded_box(centered_pos, half_size, in.corner_radius);

        // Anti-aliased edge using screen-space derivatives
        let edge_aa = fwidth(dist);
        let sdf_alpha = 1.0 - smoothstep(-edge_aa, edge_aa, dist);

        // Combine SDF mask with existing alpha
        alpha = alpha * sdf_alpha;
    }

    return vec4<f32>(in.color.rgb, alpha);
}
