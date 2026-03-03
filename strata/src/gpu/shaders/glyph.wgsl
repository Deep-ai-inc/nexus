// Strata unified rendering shader (WGSL)

struct Globals {
    transform: mat4x4f,
    atlas_size: vec2f,
    _padding: vec2f,
}

struct Instance {
    @location(0) position: vec2f,
    @location(1) size: vec2f,
    @location(2) uv_tl: vec2f,
    @location(3) uv_br: vec2f,
    @location(4) color: u32,
    @location(5) mode: u32,
    @location(6) corner_radius: f32,
    @location(7) texture_layer: u32,
    @location(8) clip_rect: vec4f,
}

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) center_pos_px: vec2f,
    @location(3) half_size_px: vec2f,
    @location(4) corner_radius: f32,
    @location(5) @interpolate(flat) mode: u32,
    @location(6) extra: vec2f,
    @location(7) clip_rect: vec4f,
}

const QUAD_VERTICES: array<vec2f, 6> = array<vec2f, 6>(
    vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
    vec2f(1.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0),
);

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn unpack_color(packed: u32) -> vec4f {
    let srgb = vec4f(
        f32(packed & 0xFFu),
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
        f32((packed >> 24u) & 0xFFu),
    ) / 255.0;
    return vec4f(
        srgb_to_linear(srgb.r),
        srgb_to_linear(srgb.g),
        srgb_to_linear(srgb.b),
        srgb.a,
    );
}

@group(0) @binding(0) var<uniform> globals: Globals;

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    instance: Instance,
) -> VertexOutput {
    var out: VertexOutput;
    out.color = unpack_color(instance.color);
    out.mode = instance.mode;
    out.clip_rect = instance.clip_rect;
    out.extra = vec2f(0.0);

    let quad_pos = QUAD_VERTICES[vertex_index];
    let base_mode = instance.mode & 0xFFu;

    var pos_px: vec2f;
    var size_px: vec2f;

    if base_mode == 1u {
        // --- MODE 1: LINE ---
        let p1 = instance.position;
        let p2 = instance.size;
        let delta = p2 - p1;
        let len = length(delta);
        let thickness = instance.corner_radius;

        let dir = select(vec2f(1.0, 0.0), delta / len, len > 0.001);
        let normal = vec2f(-dir.y, dir.x);

        pos_px = mix(p1, p2, vec2f(quad_pos.x)) + (normal * (thickness * 0.5) * (quad_pos.y * 2.0 - 1.0));

        out.uv = instance.uv_tl;
        out.center_pos_px = vec2f(quad_pos.x * len, (quad_pos.y - 0.5) * thickness);
        out.half_size_px = vec2f(len, thickness);
        out.corner_radius = 0.0;
    } else {
        // --- MODES 0, 2, 3, 4: QUAD BASED ---
        var origin = instance.position;
        size_px = instance.size;
        var sdf_size = size_px;

        if base_mode == 3u {
            let blur = instance.uv_br.x;
            let expand = blur * 2.0;
            origin -= vec2f(expand);
            size_px += vec2f(expand * 2.0);
            out.extra = vec2f(blur, expand);
            sdf_size = instance.size;
        } else if base_mode == 2u {
            out.extra.x = instance.uv_tl.x;
            out.uv = instance.uv_br;
        } else {
            out.uv = mix(instance.uv_tl, instance.uv_br, quad_pos);
        }

        pos_px = origin + quad_pos * size_px;

        out.center_pos_px = (quad_pos - 0.5) * size_px;
        out.half_size_px = sdf_size * 0.5;
        out.corner_radius = instance.corner_radius;
    }

    out.position = globals.transform * vec4f(pos_px, 0.0, 1.0);
    return out;
}

fn sdf_rounded_box(p: vec2f, b: vec2f, r: f32) -> f32 {
    let q = abs(p) - b + vec2f(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2f(0.0))) - r;
}

@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(0) @binding(3) var image_texture: texture_2d<f32>;
@group(0) @binding(4) var image_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // --- CLIPPING ---
    if in.clip_rect.z > 0.0 {
        let frag = in.position.xy;
        let val = step(in.clip_rect.xy, frag) * step(frag, in.clip_rect.xy + in.clip_rect.zw);
        if val.x * val.y == 0.0 { discard; }
    }

    let base_mode = in.mode & 0xFFu;

    // --- MODE 1: LINES ---
    if base_mode == 1u {
        let line_style = (in.mode >> 8u) & 0xFFu;
        let atlas_a = textureSample(atlas_texture, atlas_sampler, in.uv).a;

        let dist_y = abs(in.center_pos_px.y) * 2.0;
        let alpha_aa = 1.0 - smoothstep(in.half_size_px.y - 1.0, in.half_size_px.y, dist_y);

        var alpha = in.color.a * atlas_a * alpha_aa;

        if line_style > 0u {
            let p_along = in.center_pos_px.x;
            let period = select(6.0, 14.0, line_style == 1u);
            let fill_len = select(2.0, 8.0, line_style == 1u);
            let p_mod = p_along - floor(p_along / period) * period;
            let p_edge = fwidth(p_along);
            let pat_a = smoothstep(fill_len, fill_len + p_edge, p_mod) * (1.0 - smoothstep(period - p_edge, period, p_mod));
            alpha *= (1.0 - pat_a);
        }
        return vec4f(in.color.rgb, alpha);
    }

    // --- MODES 0, 2, 3, 4: SDF BASED ---
    let dist = sdf_rounded_box(in.center_pos_px, in.half_size_px, in.corner_radius);

    var shape_alpha = 1.0;

    if base_mode == 3u {
        let blur = in.extra.x;
        shape_alpha = 1.0 - smoothstep(-blur, blur, dist);
    } else if base_mode == 2u {
        let width = in.extra.x;
        let aa = fwidth(dist);
        let outer = 1.0 - smoothstep(-aa, aa, dist);
        let inner = 1.0 - smoothstep(-aa, aa, dist + width);
        shape_alpha = outer - inner;
    } else {
        if in.corner_radius > 0.0 {
            let aa = fwidth(dist);
            shape_alpha = 1.0 - smoothstep(-aa, aa, dist);
        }
    }

    var final_color = in.color.rgb;
    var final_alpha = in.color.a;

    if base_mode == 4u {
        let tex = textureSample(image_texture, image_sampler, in.uv);
        final_color *= tex.rgb;
        final_alpha *= tex.a;
    } else if base_mode == 5u {
        let tex = textureSample(atlas_texture, atlas_sampler, in.uv);
        final_color = tex.rgb;
        final_alpha = tex.a;
    } else if base_mode == 0u {
        let atlas_a = textureSample(atlas_texture, atlas_sampler, in.uv).a;
        final_alpha *= atlas_a;
    }

    return vec4f(final_color, final_alpha * shape_alpha);
}
