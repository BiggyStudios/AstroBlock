#version 450

// Variants (see build.rs):
//   default - opaque blocks (texture alpha = biome tint mask)
//   WATER   - translucent water (texture alpha = opacity)
//   FAR     - "clip" variant of the opaque shader: LOD terrain tiles (hidden near the camera
//             where real chunks exist) and chunks that are fading in or out
//
// Output is HDR radiance (in units of the sun's illuminance). The atmosphere between the
// camera and the surface (haze) is added afterwards by sky.frag.

layout(set = 0, binding = 0) uniform texture2DArray u_textures;
layout(set = 0, binding = 1) uniform sampler u_sampler;

#include "atmosphere.glsl"
#include "weather.glsl"

layout(push_constant) uniform Push {
    mat4 mvp;
    vec4 offset;
    vec4 params;
} pc;

layout(location = 0) in vec3 v_rel;
layout(location = 1) in vec2 v_uv;
layout(location = 4) flat in uint v_layer;
layout(location = 2) in vec4 v_tint;
layout(location = 3) in vec3 v_light; // ambient occlusion, sky light, block light

layout(location = 0) out vec4 out_color;

// The texture layer of `Tex::Lava` (checked by the test `lava_layer_matches_the_shader`).
const uint LAVA_LAYER = 117u;

void main() {
    float dist = length(v_rel);
    vec3 uvl = vec3(v_uv, float(v_layer & 255u));
    bool lava = (v_layer & 255u) == LAVA_LAYER;
    if (lava) {
        // Molten rock creeps and churns.
        float time = frame.radii.z;
        uvl.xy += vec2(sin(v_uv.y * 0.9 + time * 0.31), cos(v_uv.x * 1.1 - time * 0.23)) * 0.22
            + vec2(time * 0.07, time * 0.11);
    }

#if defined(FAR) || defined(WATER)
    // Clip pipelines (LOD terrain, fading chunks, all water):
    // * LOD terrain is hidden where the real chunks are loaded (inner radius, offset.w), and
    //   chunk water hands over to LOD water at that same radius (outer radius, params.y), so
    //   the two translucent layers never stack.
    if (dist < pc.offset.w || dist >= pc.params.y) {
        discard;
    }
    // * Dithered cross-fade (params.z = progress, params.w = +1 in / -1 out): a mesh fading
    //   in keeps the pixels whose threshold is below the progress, one fading out keeps
    //   exactly the others, so a swap of detail levels blends without holes or overlap.
    if (pc.params.w != 0.0) {
        float threshold = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
        if ((threshold < pc.params.z) != (pc.params.w > 0.0)) {
            discard;
        }
    }
#endif
    vec4 t = texture(sampler2DArray(u_textures, u_sampler), uvl);
#ifdef WATER
    vec3 albedo = t.rgb * v_tint.rgb;
    float alpha = t.a;
#else
    vec3 albedo = t.rgb * mix(vec3(1.0), v_tint.rgb, t.a);
    float alpha = 1.0;
#endif
#ifdef FAR
    // Cutout sprites (dropped items; only drawn with the clip pipeline, so the main opaque
    // pipeline keeps early depth testing): alpha is transparency, not a tint mask.
    if ((v_layer & 256u) != 0u) {
        if (t.a < 0.5) {
            discard;
        }
        albedo = t.rgb;
    }
#endif

    // Flat face normal from screen-space derivatives (no per-vertex normals needed,
    // and correct for the slightly curved blocks of a spherical planet).
    vec3 n = normalize(cross(dFdx(v_rel), dFdy(v_rel)));
    if (dot(n, v_rel) > 0.0) {
        n = -n;
    }

    // Light arriving here: the sun through the atmosphere (red at sunset, gone at night,
    // shadowed by the planet itself) and the sky dome.
    vec3 p = frame.camera.xyz + v_rel; // relative to the planet centre
    float r = length(p);
    vec3 up = p / r;
    vec3 sun_dir = frame.sun.xyz;
    float mu_s = dot(up, sun_dir);
    vec3 sun_light = frame.sun_color.rgb * sun_transmittance(r, mu_s);
    vec3 sky_light = frame.sun_color.rgb * sky_irradiance(r, mu_s);
    // Cloud shadows moving over the land; an overcast sky is dimmer.
    vec2 through_clouds = cloud_light(p, sun_dir);
    // (What the clouds take from the sun comes back as soft light from everywhere.)
    sky_light = sky_light * through_clouds.y + sun_light * max(mu_s, 0.0) * (1.0 - through_clouds.x) * 0.42;
    sun_light *= through_clouds.x;

    float ao = v_light.x;
    // Fake sky visibility (1 in the open, 0 deep in caves) until there is real light
    // propagation / shadowing.
    float sky = v_light.y;
    float up_n = dot(n, up);
    // Walls see half the sky plus light bounced off the ground; floors see the whole sky.
    vec3 bounce = (sun_light * max(mu_s, 0.0) + sky_light) * frame.misc.y;
    vec3 ambient = sky_light * (0.5 + 0.5 * up_n) + bounce * (0.5 - 0.5 * up_n);
    // Starlight / airglow, so nights are dark but not black.
    vec3 night = vec3(0.45, 0.6, 1.0) * frame.misc.z
        + vec3(0.8, 0.86, 1.0) * frame.clouds.w * sky;
    // Block light (torches, flood-filled on the CPU, see world::light): warm, with
    // Minecraft's brightness curve (level 15 = 1, each step down noticeably dimmer).
    float level = v_light.z;
    float torch = level / (4.0 - 3.0 * level);
    // Headlamp: a faint light near the camera in dark places (caves), so they're never pitch
    // black; torches are the real light.
    float lamp = (1.0 - sky) * pow(clamp(1.0 - dist / 16.0, 0.0, 1.0), 2.0) * 0.08;
    float ndl = max(dot(n, sun_dir), 0.0);
    vec3 irradiance = sun_light * ndl * sky
        + (ambient * mix(0.03, 1.0, sky) + night) * ao
        + vec3(1.0, 0.85, 0.7) * lamp
        + vec3(1.0, 0.78, 0.55) * torch * TORCH_IRRADIANCE * mix(1.0, ao, 0.6);
    vec3 color = albedo / PI * irradiance;
    if (lava) {
        // It shines by itself, day and night.
        float hot = dot(t.rgb, vec3(0.3, 0.6, 0.1));
        float pulse = 0.82 + 0.18 * sin(frame.radii.z * 1.3 + v_uv.x * 2.1 + v_uv.y * 1.7);
        color += t.rgb * vec3(1.0, 0.72, 0.5) * (0.3 + 1.6 * hot * hot) * pulse * frame.sun_color.rgb * 0.2;
    }

#ifndef WATER
    // Outline of the block the player is looking at (and how far mining has got).
    vec3 local = (frame.target * vec4(v_rel, 1.0)).xyz;
    float edge_width = max(0.02, 1.5 * length(fwidth(local)));
    // (params.z < 0: the first-person view model, which is never outlined.)
    if (frame.target_info.x > 0.5 && pc.params.z >= 0.0) {
        vec3 a = abs(local);
        float largest = max(a.x, max(a.y, a.z));
        if (largest < 1.02) {
            float second = a.x + a.y + a.z - largest - min(a.x, min(a.y, a.z));
            if (second > 1.0 - edge_width) {
                // A soft light edge (brightened, not a dark line).
                color = color * 1.8 + albedo * 0.02 * frame.sun_color.rgb;
            } else {
                // Mining progress: the block fades a little.
                color *= 1.0 - 0.45 * frame.target_info.y;
            }
        }
    }
#endif

#ifdef WATER
    // Fresnel reflection of the sky, and the sun's glint.
    vec3 view = -v_rel / max(dist, 1e-4);
    float cos_v = max(dot(n, view), 0.0);
    float fresnel = 0.02 + 0.98 * pow(1.0 - cos_v, 5.0);
    vec3 sky_radiance = sky_light / PI * (1.0 + 1.5 * (1.0 - cos_v));
    vec3 h = normalize(view + sun_dir);
    float glint = pow(max(dot(n, h), 0.0), 600.0) * 75.0 * step(0.0, dot(n, sun_dir));
    color = mix(color, sky_radiance, fresnel) + sun_light * glint * fresnel * sky;
    alpha = mix(alpha, 1.0, fresnel);
#endif

    out_color = vec4(color, alpha);
}
