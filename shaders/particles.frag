#version 450

// Particle colour, lit like the blocks (sun through the atmosphere, sky, night, torches).
// Variants: default = cut-out (alpha test, writes depth); BLEND = translucent.

layout(set = 0, binding = 0) uniform texture2DArray u_textures;
layout(set = 0, binding = 1) uniform sampler u_sampler;

#include "atmosphere.glsl"
#include "weather.glsl"
#include "particle.glsl"

layout(location = 0) in vec3 v_rel;
layout(location = 1) in vec2 v_uv;
layout(location = 2) flat in uint v_look;
layout(location = 3) in vec4 v_tint;
layout(location = 4) in vec2 v_corner;
layout(location = 5) in float v_alpha;

layout(location = 0) out vec4 out_color;

void main() {
    uint flags = (v_look >> 8u) & 255u;
    vec4 c;
    if ((flags & P_ROUND) != 0u) {
        float r = length(v_corner);
#ifdef BLEND
        // Soft all the way to the middle: big puffs read as smoke, not discs.
        float edge = clamp(1.0 - r, 0.0, 1.0);
        c = vec4(v_tint.rgb, v_tint.a * edge * edge * (3.0 - 2.0 * edge));
#else
        c = vec4(v_tint.rgb, v_tint.a * clamp((1.0 - r) * 3.0, 0.0, 1.0));
#endif
    } else {
        vec4 t = texture(sampler2DArray(u_textures, u_sampler), vec3(v_uv, float(v_look & 255u)));
        if ((flags & P_TINT_MASK) != 0u) {
            c = vec4(t.rgb * mix(vec3(1.0), v_tint.rgb, t.a), 1.0);
        } else {
            c = t * v_tint;
        }
    }
#ifndef BLEND
    if (c.a < 0.5) {
        discard;
    }
#endif

    vec3 p = frame.camera.xyz + v_rel;
    float r = length(p);
    vec3 up = p / r;
    float mu_s = dot(up, frame.sun.xyz);
    vec3 sun_light = frame.sun_color.rgb * sun_transmittance(r, mu_s);
    vec3 sky_light = frame.sun_color.rgb * sky_irradiance(r, mu_s);
    vec2 through_clouds = cloud_light(p, frame.sun.xyz);
    sky_light = sky_light * through_clouds.y + sun_light * max(mu_s, 0.0) * (1.0 - through_clouds.x) * 0.42;
    sun_light *= through_clouds.x;
    float level = float((v_look >> 16u) & 15u) / 15.0;
    float sky = float((v_look >> 20u) & 15u) / 15.0;
    float torch = level / (4.0 - 3.0 * level);
    // A small flat sprite has no real normal: take most of the sun, all of the sky.
    vec3 irradiance = sun_light * (0.7 * sky)
        + sky_light * mix(0.03, 1.0, sky)
        + vec3(0.45, 0.6, 1.0) * frame.misc.z
        + vec3(0.8, 0.86, 1.0) * frame.clouds.w * sky
        + vec3(1.0, 0.78, 0.55) * torch * TORCH_IRRADIANCE;
    vec3 color = c.rgb / PI * irradiance;
    if ((flags & P_GLOW) != 0u) {
        // Self-lit, several times brighter than a sunlit white surface.
        color = c.rgb * frame.sun_color.rgb * 0.9;
    }

#ifdef BLEND
    out_color = vec4(color, c.a * v_alpha);
#else
    out_color = vec4(color, 1.0);
#endif
}
