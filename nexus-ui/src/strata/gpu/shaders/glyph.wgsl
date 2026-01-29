// Strata unified rendering shader (ubershader).
//
// Renders all 2D content in a single draw call:
// - Mode 0 (Quad):   Glyphs, solid quads, rounded rects, circles
// - Mode 1 (Line):   Thick line segments rendered as rotated quads
// - Mode 2 (Border): Hollow rounded rects via SDF ring
// - Mode 3 (Shadow): Soft drop shadows via SDF blur
// - Mode 4 (Image):  Texture array sampling (stubbed)
//
// Uses the "white pixel" trick: a 1x1 white pixel at atlas (0,0) enables
// solid quads to render with the same shader path as textured glyphs.
//
// Per-instance clip_rect enables nested scroll regions without breaking
// the single draw call. clip_rect.z == 0 disables clipping.

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
    @location(0) position: vec2<f32>,      // pos
    @location(1) size: vec2<f32>,          // size
    @location(2) uv_tl: vec2<f32>,        // UV top-left (normalized 0-1)
    @location(3) uv_br: vec2<f32>,        // UV bottom-right (normalized 0-1)
    @location(4) color: u32,              // Packed RGBA8
    @location(5) mode: u32,               // Low 8 bits: render mode, bits 8..15: line style
    @location(6) corner_radius: f32,      // SDF radius or line thickness
    @location(7) texture_layer: u32,      // Image texture layer (mode 4)
    @location(8) clip_rect: vec4<f32>,    // Per-instance clip (x, y, w, h); w=0 disables
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local_pos: vec2<f32>,    // Position within quad (0-1)
    @location(3) size: vec2<f32>,         // Quad size in pixels
    @location(4) corner_radius: f32,
    @location(5) @interpolate(flat) mode: u32,
    @location(6) @interpolate(flat) extra: vec2<f32>,  // Mode-specific data
    @location(7) clip_rect: vec4<f32>,    // Per-instance clip rect
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
    out.clip_rect = instance.clip_rect;
    out.extra = vec2<f32>(0.0, 0.0);

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

        // Map quad_pos to rotated line quad corners
        let along = mix(p1, p2, vec2<f32>(quad_pos.x));
        let offset = normal * half_thick * (quad_pos.y * 2.0 - 1.0);
        let pos = along + offset;

        out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

        // UV: sample white pixel (solid color)
        out.uv = instance.uv_tl;

        // Pass line dimensions for AA in fragment shader
        out.local_pos = quad_pos;
        out.size = vec2<f32>(len, thickness);
        out.corner_radius = 0.0;
    } else if (base_mode == 3u) {
        // ============================================================
        // MODE 3: SHADOW
        // Expand quad by blur_radius on all sides for soft falloff.
        // blur_radius stored in uv_br.x
        // ============================================================
        let blur = instance.uv_br.x;
        let expand = blur * 2.0;  // Expand enough for smooth falloff

        let expanded_pos = instance.position - vec2<f32>(expand);
        let expanded_size = instance.size + vec2<f32>(expand * 2.0);

        let pos = expanded_pos + quad_pos * expanded_size;
        out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

        out.uv = instance.uv_tl;  // White pixel for solid color
        out.local_pos = quad_pos;
        out.size = expanded_size;
        out.corner_radius = instance.corner_radius;
        out.extra = vec2<f32>(blur, expand);  // Pass blur and expand to fragment
    } else {
        // ============================================================
        // MODES 0, 2, 4: STANDARD QUAD
        // ============================================================
        let pos = instance.position + quad_pos * instance.size;
        out.position = globals.transform * vec4<f32>(pos, 0.0, 1.0);

        // Interpolate UV from tl to br
        out.uv = mix(instance.uv_tl, instance.uv_br, quad_pos);

        out.local_pos = quad_pos;
        out.size = instance.size;
        out.corner_radius = instance.corner_radius;

        // For border mode, pass border_width via extra
        if (base_mode == 2u) {
            out.extra = vec2<f32>(instance.uv_tl.x, 0.0);  // border_width
            // Border uses white pixel UV (solid color)
            let wp = instance.uv_br;
            out.uv = wp;
        }
    }

    return out;
}

// Signed Distance Function for a rounded rectangle.
// Returns negative inside, positive outside, zero on edge.
fn sdf_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_mode = in.mode & 0xFFu;

    // ============================================================
    // Per-instance clipping (all modes)
    // clip_rect.z == 0 means no clipping
    // ============================================================
    if (in.clip_rect.z > 0.0) {
        let frag_pos = in.position.xy;
        let clip_min = in.clip_rect.xy;
        let clip_max = in.clip_rect.xy + in.clip_rect.zw;
        if (frag_pos.x < clip_min.x || frag_pos.x > clip_max.x ||
            frag_pos.y < clip_min.y || frag_pos.y > clip_max.y) {
            discard;
        }
    }

    if (base_mode == 1u) {
        // ============================================================
        // MODE 1: LINE SEGMENT
        // Solid, dashed, or dotted with edge anti-aliasing.
        // ============================================================
        let line_style = (in.mode >> 8u) & 0xFFu;
        let atlas_alpha = textureSample(atlas_texture, atlas_sampler, in.uv).a;
        var alpha = in.color.a * atlas_alpha;

        // Anti-alias along the perpendicular (y) axis of the line
        let dist_from_center = abs(in.local_pos.y - 0.5) * 2.0;
        let edge_aa = fwidth(dist_from_center);
        let line_alpha = 1.0 - smoothstep(1.0 - edge_aa, 1.0, dist_from_center);
        alpha = alpha * line_alpha;

        // Apply line style pattern along the line direction
        let pixel_along = in.local_pos.x * in.size.x;

        if (line_style == 1u) {
            // DASHED: 8px dash, 6px gap (14px period)
            let pattern_pos = pixel_along % 14.0;
            let dash_edge = fwidth(pixel_along);
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

    if (base_mode == 2u) {
        // ============================================================
        // MODE 2: BORDER (hollow rounded rect via SDF ring)
        // extra.x = border_width
        // ============================================================
        let border_width = in.extra.x;
        let centered_pos = (in.local_pos - 0.5) * in.size;
        let half_size = in.size * 0.5;

        // Outer edge SDF
        let dist_outer = sdf_rounded_box(centered_pos, half_size, in.corner_radius);

        // Inner edge SDF (inset by border_width)
        let inner_radius = max(in.corner_radius - border_width, 0.0);
        let inner_half = half_size - vec2<f32>(border_width);
        let dist_inner = sdf_rounded_box(centered_pos, inner_half, inner_radius);

        // Anti-aliased edges
        let edge_aa = fwidth(dist_outer);
        let outer_alpha = 1.0 - smoothstep(-edge_aa, edge_aa, dist_outer);
        let inner_alpha = 1.0 - smoothstep(-edge_aa, edge_aa, dist_inner);

        // Ring = outer minus inner
        let ring_alpha = outer_alpha * (1.0 - inner_alpha);

        return vec4<f32>(in.color.rgb, in.color.a * ring_alpha);
    }

    if (base_mode == 3u) {
        // ============================================================
        // MODE 3: SHADOW (soft SDF blur)
        // extra.x = blur_radius, extra.y = expand amount
        // The quad was expanded by `expand` in the vertex shader.
        // ============================================================
        let blur = in.extra.x;
        let expand = in.extra.y;

        // Map local_pos back to the original (un-expanded) rect space
        let original_size = in.size - vec2<f32>(expand * 2.0);
        let centered_pos = (in.local_pos - 0.5) * in.size;
        let half_size = original_size * 0.5;

        // SDF distance to original rect
        let dist = sdf_rounded_box(centered_pos, half_size, in.corner_radius);

        // Smooth falloff using blur radius
        // smoothstep from -blur to +blur creates a soft edge
        let shadow_alpha = 1.0 - smoothstep(-blur, blur, dist);

        return vec4<f32>(in.color.rgb, in.color.a * shadow_alpha);
    }

    // ============================================================
    // MODE 0 (and 4): STANDARD QUAD
    // ============================================================
    // Sample the atlas.
    let atlas_alpha = textureSample(atlas_texture, atlas_sampler, in.uv).a;

    // Start with atlas-modulated alpha
    var alpha = in.color.a * atlas_alpha;

    // Apply SDF rounded corner mask if corner_radius > 0
    if (in.corner_radius > 0.0) {
        let centered_pos = (in.local_pos - 0.5) * in.size;
        let half_size = in.size * 0.5;

        let dist = sdf_rounded_box(centered_pos, half_size, in.corner_radius);

        // Anti-aliased edge using screen-space derivatives
        let edge_aa = fwidth(dist);
        let sdf_alpha = 1.0 - smoothstep(-edge_aa, edge_aa, dist);

        alpha = alpha * sdf_alpha;
    }

    return vec4<f32>(in.color.rgb, alpha);
}
