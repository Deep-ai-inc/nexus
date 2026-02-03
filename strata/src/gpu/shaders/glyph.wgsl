// Strata unified rendering shader (Optimized)

struct Globals {
    transform: mat4x4<f32>,
    atlas_size: vec2<f32>,
    _padding: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var atlas_texture: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;
@group(1) @binding(2) var image_texture: texture_2d<f32>;
@group(1) @binding(3) var image_sampler: sampler;

struct Instance {
    @location(0) position: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_tl: vec2<f32>,
    @location(3) uv_br: vec2<f32>,
    @location(4) color: u32,
    @location(5) mode: u32,
    @location(6) corner_radius: f32,
    @location(7) texture_layer: u32,
    @location(8) clip_rect: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Optimization: Pass centered pixel coordinates directly to avoid FS math
    @location(2) center_pos_px: vec2<f32>,
    // Optimization: Pass half-size directly for SDF
    @location(3) half_size_px: vec2<f32>,
    @location(4) corner_radius: f32,
    @location(5) @interpolate(flat) mode: u32,
    @location(6) @interpolate(flat) extra: vec2<f32>,
    @location(7) clip_rect: vec4<f32>,
}

var<private> QUAD_VERTICES: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

fn srgb_to_linear(c: f32) -> f32 {
    // Convert a single sRGB channel to linear.
    // Instance colors are specified in sRGB but the pipeline operates in linear
    // space (sRGB render target applies linear→sRGB on output).
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn unpack_color(packed: u32) -> vec4<f32> {
    let srgb = vec4<f32>(
        f32(packed & 0xFFu),
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
        f32((packed >> 24u) & 0xFFu)
    ) / 255.0;
    // Convert RGB from sRGB to linear; alpha stays linear
    return vec4<f32>(
        srgb_to_linear(srgb.r),
        srgb_to_linear(srgb.g),
        srgb_to_linear(srgb.b),
        srgb.a
    );
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    var out: VertexOutput;
    out.color = unpack_color(instance.color);
    out.mode = instance.mode;
    out.clip_rect = instance.clip_rect;
    out.extra = vec2<f32>(0.0);

    let quad_pos = QUAD_VERTICES[vertex_index]; // 0..1
    let base_mode = instance.mode & 0xFFu;

    // Common calculations
    var pos_px: vec2<f32>;
    var size_px: vec2<f32>;

    if (base_mode == 1u) {
        // --- MODE 1: LINE ---
        let p1 = instance.position;
        let p2 = instance.size; // Stored in size
        let delta = p2 - p1;
        let len = length(delta);
        let thickness = instance.corner_radius;

        // Branchless normal calculation
        let dir = select(vec2<f32>(1.0, 0.0), delta / len, len > 0.001);
        let normal = vec2<f32>(-dir.y, dir.x);

        // Expand line to quad
        pos_px = mix(p1, p2, vec2<f32>(quad_pos.x)) + (normal * (thickness * 0.5) * (quad_pos.y * 2.0 - 1.0));

        out.uv = instance.uv_tl;
        // For lines, center_pos_px.x is distance along line, y is distance from center
        out.center_pos_px = vec2<f32>(quad_pos.x * len, (quad_pos.y - 0.5) * thickness);
        out.half_size_px = vec2<f32>(len, thickness); // Not strictly half-size, used for pattern
        out.corner_radius = 0.0;
    } else {
        // --- MODES 0, 2, 3, 4: QUAD BASED ---
        var origin = instance.position;
        size_px = instance.size;
        var sdf_size = size_px; // The size used for SDF calculation

        if (base_mode == 3u) {
            // Shadow: Expand geometry
            let blur = instance.uv_br.x;
            let expand = blur * 2.0;
            origin -= vec2<f32>(expand);
            size_px += vec2<f32>(expand * 2.0);
            out.extra = vec2<f32>(blur, expand);
            // SDF logic needs original size, not expanded size
            sdf_size = instance.size;
        } else if (base_mode == 2u) {
            // Border: Pass width
            out.extra.x = instance.uv_tl.x;
            out.uv = instance.uv_br; // Use white pixel
        } else {
            // Standard/Image
            out.uv = mix(instance.uv_tl, instance.uv_br, quad_pos);
        }

        if (base_mode != 2u && base_mode != 3u) {
             // Mode 2 handled above, Mode 3 uses uv_tl
             // This ensures Mode 0/4 get correct UV interpolation
        }

        pos_px = origin + quad_pos * size_px;

        // OPTIMIZATION: Calculate centered position here (VS) instead of FS
        out.center_pos_px = (quad_pos - 0.5) * size_px;
        out.half_size_px = sdf_size * 0.5;
        out.corner_radius = instance.corner_radius;
    }

    out.position = globals.transform * vec4<f32>(pos_px, 0.0, 1.0);
    return out;
}

fn sdf_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // --- CLIPPING ---
    // Vectorized check. If clip_rect.z (width) > 0, clipping is active.
    if (in.clip_rect.z > 0.0) {
        let frag = in.position.xy;
        let val = step(in.clip_rect.xy, frag) * step(frag, in.clip_rect.xy + in.clip_rect.zw);
        if (val.x * val.y == 0.0) { discard; }
    }

    let base_mode = in.mode & 0xFFu;

    // --- MODE 1: LINES (Special Path) ---
    if (base_mode == 1u) {
        let line_style = (in.mode >> 8u) & 0xFFu;
        let atlas_a = textureSample(atlas_texture, atlas_sampler, in.uv).a;

        // Anti-alias across width
        let dist_y = abs(in.center_pos_px.y) * 2.0; // center_pos_px.y was (0..1-0.5)*thick
        let alpha_aa = 1.0 - smoothstep(in.half_size_px.y - 1.0, in.half_size_px.y, dist_y);

        var alpha = in.color.a * atlas_a * alpha_aa;

        // Patterns
        if (line_style > 0u) {
            let p_along = in.center_pos_px.x; // Calculated in VS
            // 1=Dashed (8,6), 2=Dotted (2,4)
            let period = select(6.0, 14.0, line_style == 1u);
            let fill   = select(2.0, 8.0,  line_style == 1u);
            let p_mod  = p_along % period;
            let p_edge = fwidth(p_along);
            let pat_a  = smoothstep(fill, fill + p_edge, p_mod) * (1.0 - smoothstep(period - p_edge, period, p_mod));
            alpha *= (1.0 - pat_a);
        }
        return vec4<f32>(in.color.rgb, alpha);
    }

    // --- MODES 0, 2, 3, 4: SDF BASED ---

    // 1. Calculate signed distance to the box edge
    // Optimized: center_pos_px and half_size_px pre-calculated in VS
    let dist = sdf_rounded_box(in.center_pos_px, in.half_size_px, in.corner_radius);

    // 2. Calculate Shape Alpha (Geometry mask)
    var shape_alpha = 1.0;

    if (base_mode == 3u) {
        // Shadow: Blur the SDF
        let blur = in.extra.x;
        shape_alpha = 1.0 - smoothstep(-blur, blur, dist);
    } else if (base_mode == 2u) {
        // Border: Ring logic
        // Inside if dist < 0. Inside inner hole if dist < -width.
        // Result = (dist < 0) && (dist > -width)
        let width = in.extra.x;
        let aa = fwidth(dist);
        // Optimization: Single SDF, two smoothsteps define the ring
        let outer = 1.0 - smoothstep(-aa, aa, dist);
        let inner = 1.0 - smoothstep(-aa, aa, dist + width);
        shape_alpha = outer - inner;
    } else {
        // Mode 0 (Quad) & Mode 4 (Image)
        // Only apply SDF AA if we actually have rounded corners.
        // This preserves perfect seams for tiled rectangular geometry.
        if (in.corner_radius > 0.0) {
            let aa = fwidth(dist);
            shape_alpha = 1.0 - smoothstep(-aa, aa, dist);
        }
    }

    // 3. Apply Texture / Color
    var final_color = in.color.rgb;
    var final_alpha = in.color.a;

    if (base_mode == 4u) {
        // Image Mode
        let tex = textureSample(image_texture, image_sampler, in.uv);
        final_color *= tex.rgb;
        final_alpha *= tex.a;
    } else if (base_mode == 5u) {
        // Color glyph mode (emoji bitmaps)
        let tex = textureSample(atlas_texture, atlas_sampler, in.uv);
        final_color = tex.rgb;
        final_alpha = tex.a;
    } else if (base_mode == 0u) {
        // Glyph/Quad Mode
        let atlas_a = textureSample(atlas_texture, atlas_sampler, in.uv).a;
        final_alpha *= atlas_a;
    }
    // Mode 2 and 3 use solid color (uv points to white pixel)

    return vec4<f32>(final_color, final_alpha * shape_alpha);
}
