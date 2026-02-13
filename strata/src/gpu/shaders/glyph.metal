// Strata unified rendering shader (Metal)

#include <metal_stdlib>
using namespace metal;

struct Globals {
    float4x4 transform;
    float2 atlas_size;
    float2 _padding;
};

struct Instance {
    float2 position [[attribute(0)]];
    float2 size     [[attribute(1)]];
    float2 uv_tl    [[attribute(2)]];
    float2 uv_br    [[attribute(3)]];
    uint color      [[attribute(4)]];
    uint mode       [[attribute(5)]];
    float corner_radius [[attribute(6)]];
    uint texture_layer  [[attribute(7)]];
    float4 clip_rect    [[attribute(8)]];
};

struct VertexOutput {
    float4 position [[position]];
    float2 uv;
    float4 color;
    float2 center_pos_px;
    float2 half_size_px;
    float corner_radius;
    uint mode [[flat]];
    float2 extra;
    float4 clip_rect;
};

constant float2 QUAD_VERTICES[] = {
    float2(0.0, 0.0), float2(1.0, 0.0), float2(0.0, 1.0),
    float2(1.0, 0.0), float2(1.0, 1.0), float2(0.0, 1.0),
};

static float srgb_to_linear(float c) {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

static float4 unpack_color(uint packed) {
    float4 srgb = float4(
        float(packed & 0xFFu),
        float((packed >> 8u) & 0xFFu),
        float((packed >> 16u) & 0xFFu),
        float((packed >> 24u) & 0xFFu)
    ) / 255.0;
    return float4(
        srgb_to_linear(srgb.r),
        srgb_to_linear(srgb.g),
        srgb_to_linear(srgb.b),
        srgb.a
    );
}

vertex VertexOutput vs_main(
    uint vertex_index [[vertex_id]],
    Instance instance [[stage_in]],
    constant Globals& globals [[buffer(1)]]
) {
    VertexOutput out;
    out.color = unpack_color(instance.color);
    out.mode = instance.mode;
    out.clip_rect = instance.clip_rect;
    out.extra = float2(0.0);

    float2 quad_pos = QUAD_VERTICES[vertex_index];
    uint base_mode = instance.mode & 0xFFu;

    float2 pos_px;
    float2 size_px;

    if (base_mode == 1u) {
        // --- MODE 1: LINE ---
        float2 p1 = instance.position;
        float2 p2 = instance.size;
        float2 delta = p2 - p1;
        float len = length(delta);
        float thickness = instance.corner_radius;

        float2 dir = select(float2(1.0, 0.0), delta / len, len > 0.001);
        float2 normal = float2(-dir.y, dir.x);

        pos_px = mix(p1, p2, float2(quad_pos.x)) + (normal * (thickness * 0.5) * (quad_pos.y * 2.0 - 1.0));

        out.uv = instance.uv_tl;
        out.center_pos_px = float2(quad_pos.x * len, (quad_pos.y - 0.5) * thickness);
        out.half_size_px = float2(len, thickness);
        out.corner_radius = 0.0;
    } else {
        // --- MODES 0, 2, 3, 4: QUAD BASED ---
        float2 origin = instance.position;
        size_px = instance.size;
        float2 sdf_size = size_px;

        if (base_mode == 3u) {
            float blur = instance.uv_br.x;
            float expand = blur * 2.0;
            origin -= float2(expand);
            size_px += float2(expand * 2.0);
            out.extra = float2(blur, expand);
            sdf_size = instance.size;
        } else if (base_mode == 2u) {
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

    out.position = globals.transform * float4(pos_px, 0.0, 1.0);
    return out;
}

static float sdf_rounded_box(float2 p, float2 b, float r) {
    float2 q = abs(p) - b + float2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, float2(0.0))) - r;
}

fragment float4 fs_main(
    VertexOutput in [[stage_in]],
    texture2d<float> atlas_texture [[texture(0)]],
    sampler atlas_sampler [[sampler(0)]],
    texture2d<float> image_texture [[texture(1)]],
    sampler image_sampler [[sampler(1)]],
    constant Globals& globals [[buffer(0)]]
) {
    // --- CLIPPING ---
    if (in.clip_rect.z > 0.0) {
        float2 frag = in.position.xy;
        float2 val = step(in.clip_rect.xy, frag) * step(frag, in.clip_rect.xy + in.clip_rect.zw);
        if (val.x * val.y == 0.0) { discard_fragment(); }
    }

    uint base_mode = in.mode & 0xFFu;

    // --- MODE 1: LINES ---
    if (base_mode == 1u) {
        uint line_style = (in.mode >> 8u) & 0xFFu;
        float atlas_a = atlas_texture.sample(atlas_sampler, in.uv).a;

        float dist_y = abs(in.center_pos_px.y) * 2.0;
        float alpha_aa = 1.0 - smoothstep(in.half_size_px.y - 1.0, in.half_size_px.y, dist_y);

        float alpha = in.color.a * atlas_a * alpha_aa;

        if (line_style > 0u) {
            float p_along = in.center_pos_px.x;
            float period = select(6.0, 14.0, line_style == 1u);
            float fill   = select(2.0, 8.0,  line_style == 1u);
            float p_mod  = fmod(p_along, period);
            float p_edge = fwidth(p_along);
            float pat_a  = smoothstep(fill, fill + p_edge, p_mod) * (1.0 - smoothstep(period - p_edge, period, p_mod));
            alpha *= (1.0 - pat_a);
        }
        return float4(in.color.rgb, alpha);
    }

    // --- MODES 0, 2, 3, 4: SDF BASED ---
    float dist = sdf_rounded_box(in.center_pos_px, in.half_size_px, in.corner_radius);

    float shape_alpha = 1.0;

    if (base_mode == 3u) {
        float blur = in.extra.x;
        shape_alpha = 1.0 - smoothstep(-blur, blur, dist);
    } else if (base_mode == 2u) {
        float width = in.extra.x;
        float aa = fwidth(dist);
        float outer = 1.0 - smoothstep(-aa, aa, dist);
        float inner = 1.0 - smoothstep(-aa, aa, dist + width);
        shape_alpha = outer - inner;
    } else {
        if (in.corner_radius > 0.0) {
            float aa = fwidth(dist);
            shape_alpha = 1.0 - smoothstep(-aa, aa, dist);
        }
    }

    float3 final_color = in.color.rgb;
    float final_alpha = in.color.a;

    if (base_mode == 4u) {
        float4 tex = image_texture.sample(image_sampler, in.uv);
        final_color *= tex.rgb;
        final_alpha *= tex.a;
    } else if (base_mode == 5u) {
        float4 tex = atlas_texture.sample(atlas_sampler, in.uv);
        final_color = tex.rgb;
        final_alpha = tex.a;
    } else if (base_mode == 0u) {
        float atlas_a = atlas_texture.sample(atlas_sampler, in.uv).a;
        final_alpha *= atlas_a;
    }

    return float4(final_color, final_alpha * shape_alpha);
}
