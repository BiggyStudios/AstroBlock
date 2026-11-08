#version 450

// Averages the noisy clouds over time: last frame's result, moved to where the same part of
// the sky is now and kept within the range of this frame's neighbouring pixels (so nothing
// smears when the clouds or the view change).

#include "atmosphere.glsl"

layout(set = 0, binding = 17) uniform texture2D u_clouds_raw;
layout(set = 0, binding = 18) uniform texture2D u_clouds_prev;

layout(location = 0) in vec2 v_ndc;
layout(location = 0) out vec4 out_color;

void main() {
    ivec2 px = ivec2(gl_FragCoord.xy);
    ivec2 size = textureSize(sampler2D(u_clouds_raw, u_linear), 0);
    vec4 current = texelFetch(sampler2D(u_clouds_raw, u_linear), px, 0);
    vec4 low = current;
    vec4 high = current;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            if (x != 0 || y != 0) {
                vec4 c = texelFetch(sampler2D(u_clouds_raw, u_linear), clamp(px + ivec2(x, y), ivec2(0), size - 1), 0);
                low = min(low, c);
                high = max(high, c);
            }
        }
    }
    vec4 result = current;
    if (frame.prev_offset.w > 0.5) {
        // Where this ray pointed last frame, taking the clouds as a few km away.
        vec4 near_h = frame.inv_view_proj * vec4(v_ndc, 1.0, 1.0);
        vec3 dir = normalize(near_h.xyz / near_h.w);
        vec4 clip = frame.prev_view_proj * vec4(dir * 3000.0 + frame.prev_offset.xyz, 1.0);
        vec2 uv = clip.xy / clip.w * 0.5 + 0.5;
        if (clip.w > 0.0 && uv.x >= 0.0 && uv.y >= 0.0 && uv.x <= 1.0 && uv.y <= 1.0) {
            vec4 history = textureLod(sampler2D(u_clouds_prev, u_linear), uv, 0.0);
            vec4 margin = (high - low) * 0.15 + vec4(1e-5);
            history = clamp(history, low - margin, high + margin);
            result = mix(history, current, 0.12);
        }
    }
    // (A NaN would otherwise live on in the history forever.)
    if (!(result.r >= 0.0) || !(result.a >= 0.0)) {
        result = vec4(0.0, 0.0, 0.0, 1.0);
    }
    out_color = result;
}
