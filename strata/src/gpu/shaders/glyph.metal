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

// Gradient header in storage buffer (32 bytes).
struct GpuGradient {
    uint kind;          // 0=linear, 1=radial, 2=conic
    uint stop_offset;   // index into stops buffer
    uint stop_count;    // number of color stops
    uint spread;        // 0=pad, 1=repeat, 2=reflect
    float4 params;      // linear: start.xy, end.xy / radial: center.xy, radius, 0 / conic: center.xy, angle, 0
};

// Gradient color stop in storage buffer (32 bytes).
// NOTE: float3 has 16-byte alignment in Metal, so use 3 individual floats for padding.
struct GpuGradientStop {
    float4 color;       // Oklab [L, a, b, alpha]  (offset 0, 16 bytes)
    float offset;       // 0.0–1.0                 (offset 16, 4 bytes)
    float _pad0;        //                          (offset 20)
    float _pad1;        //                          (offset 24)
    float _pad2;        //                          (offset 28)
};                      // total: 32 bytes

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
    float2 local_pos;       // fragment position relative to quad origin (pixels)
    float2 quad_size;       // quad size in pixels (for gradient coordinate mapping)
    uint gradient_index [[flat]]; // gradient index (from color field for mode 6)
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

// Oklab → linear sRGB conversion
static float3 oklab_to_linear_srgb(float3 lab) {
    float l_ = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
    float m_ = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
    float s_ = lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z;

    float l = l_ * l_ * l_;
    float m = m_ * m_ * m_;
    float s = s_ * s_ * s_;

    float3 rgb = float3(
        +4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s
    );
    return max(rgb, float3(0.0));
}

vertex VertexOutput vs_main(
    uint vertex_index [[vertex_id]],
    Instance instance [[stage_in]],
    constant Globals& globals [[buffer(1)]]
) {
    VertexOutput out;
    out.mode = instance.mode;
    out.clip_rect = instance.clip_rect;
    out.extra = float2(0.0);
    out.local_pos = float2(0.0);
    out.quad_size = float2(0.0);
    out.gradient_index = 0u;

    float2 quad_pos = QUAD_VERTICES[vertex_index];
    uint base_mode = instance.mode & 0xFFu;

    // For mode 6 (gradient), pass gradient index instead of unpacking color
    if (base_mode == 6u) {
        out.color = float4(0.0);
        out.gradient_index = instance.color;
    } else {
        out.color = unpack_color(instance.color);
    }

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
        // --- MODES 0, 2, 3, 4, 6: QUAD BASED ---
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

        // For gradient mode, pass local position within the quad
        if (base_mode == 6u) {
            out.local_pos = quad_pos * instance.size;
            out.quad_size = instance.size;
        }
    }

    out.position = globals.transform * float4(pos_px, 0.0, 1.0);
    return out;
}

static float sdf_rounded_box(float2 p, float2 b, float r) {
    float2 q = abs(p) - b + float2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, float2(0.0))) - r;
}

// Apply spread mode to raw t value
static float apply_spread(float t, uint spread) {
    if (spread == 0u) {
        // Pad
        return clamp(t, 0.0, 1.0);
    } else if (spread == 1u) {
        // Repeat
        return fract(t);
    } else {
        // Reflect
        float t2 = fmod(abs(t), 2.0);
        return t2 <= 1.0 ? t2 : 2.0 - t2;
    }
}

// Sample gradient stops at parameter t, returning Oklab + alpha
static float4 sample_gradient(
    float t,
    uint stop_offset,
    uint stop_count,
    const device GpuGradientStop* stops
) {
    if (stop_count == 0u) return float4(0.0);
    if (stop_count == 1u) return stops[stop_offset].color;

    // Clamp to first/last stop
    if (t <= stops[stop_offset].offset) {
        return stops[stop_offset].color;
    }
    uint last = stop_offset + stop_count - 1u;
    if (t >= stops[last].offset) {
        return stops[last].color;
    }

    // Find the two surrounding stops (stops are ordered, so only check upper bound)
    for (uint i = stop_offset; i < last; i++) {
        float off1 = stops[i + 1u].offset;
        if (t <= off1) {
            float off0 = stops[i].offset;
            float range = off1 - off0;
            float frac = (range > 0.0001) ? (t - off0) / range : 0.0;
            return mix(stops[i].color, stops[i + 1u].color, frac);
        }
    }

    return stops[last].color;
}

fragment float4 fs_main(
    VertexOutput in [[stage_in]],
    texture2d<float> atlas_texture [[texture(0)]],
    sampler atlas_sampler [[sampler(0)]],
    texture2d<float> image_texture [[texture(1)]],
    sampler image_sampler [[sampler(1)]],
    constant Globals& globals [[buffer(0)]],
    const device GpuGradient* gradients [[buffer(1)]],
    const device GpuGradientStop* gradient_stops [[buffer(2)]]
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

    // --- MODES 0, 2, 3, 4, 5, 6: SDF BASED ---
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

    // --- MODE 6: GRADIENT ---
    if (base_mode == 6u) {
        GpuGradient grad = gradients[in.gradient_index];
        float2 pos = in.local_pos;
        float t = 0.0;

        if (grad.kind == 0u) {
            // Linear: un-normalize params from unit space to local pixels
            float2 start = grad.params.xy * in.quad_size;
            float2 end = grad.params.zw * in.quad_size;
            float2 delta = end - start;
            float len_sq = dot(delta, delta);
            t = (len_sq > 0.0001) ? dot(pos - start, delta) / len_sq : 0.0;
        } else if (grad.kind == 1u) {
            // Radial: center in unit space, radius relative to width
            float2 center = grad.params.xy * in.quad_size;
            float radius = grad.params.z * in.quad_size.x;
            t = (radius > 0.0001) ? length(pos - center) / radius : 0.0;
        } else {
            // Conic: center in unit space, angle in radians
            float2 center = grad.params.xy * in.quad_size;
            float start_angle = grad.params.z;
            float angle = atan2(pos.y - center.y, pos.x - center.x) - start_angle;
            t = fract(angle / (2.0 * M_PI_F));
        }

        t = apply_spread(t, grad.spread);

        // Sample in Oklab space
        float4 oklab_color = sample_gradient(t, grad.stop_offset, grad.stop_count, gradient_stops);

        // Convert Oklab → linear sRGB
        float3 linear_rgb = oklab_to_linear_srgb(oklab_color.xyz);

        return float4(linear_rgb, oklab_color.a * shape_alpha);
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
