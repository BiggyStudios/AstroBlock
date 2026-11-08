#version 450

// Final full-screen pass: atmospheric scattering over the HDR scene, the star field and the
// sun, then exposure and tone mapping into the swapchain.
//
// For every pixel the view ray is marched through the atmosphere, from the camera to the
// terrain (depth buffer) or out of the atmosphere (sky). That one integral gives the blue
// sky, sunsets, aerial perspective (haze over distant terrain) and the glowing limb of the
// planet seen from space. Single scattering uses the real phase functions; higher orders
// come from the multiple scattering LUT.

#include "atmosphere.glsl"

layout(set = 0, binding = 7) uniform texture2D u_scene; // HDR scene colour
layout(set = 0, binding = 8) uniform texture2D u_depth; // reverse-Z depth (0 = sky)
layout(set = 0, binding = 9) uniform texture2D u_galaxy; // Milky Way, equirectangular
layout(set = 0, binding = 19) uniform texture2D u_clouds; // half resolution: light added, transmittance

layout(location = 0) in vec2 v_ndc;
layout(location = 0) out vec4 out_color;

// (near, far) distances along a ray from `o` to a sphere around the planet centre; far < 0
// on a miss.
vec2 ray_sphere(vec3 o, vec3 d, float radius) {
    float r = length(o);
    float b = dot(o, d);
    // (r - R)(r + R) instead of r^2 - R^2: precise near the surface.
    float c = (r - radius) * (r + radius);
    float disc = b * b - c;
    if (disc < 0.0) {
        return vec2(-1.0, -1.0);
    }
    float s = sqrt(disc);
    return vec2(-b - s, -b + s);
}

uvec3 pcg3d(uvec3 v) {
    v = v * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> 16u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

vec3 hash3(uvec3 v) {
    return vec3(pcg3d(v)) * (1.0 / 4294967295.0);
}

// Point-like stars on a cube-map grid (fixed in the inertial frame, so they wheel across
// the night sky as the planet turns). At most one star per cell, kept away from the cell's
// edges, so only the pixel's own cell needs checking. Radiance relative to the sun.
vec3 stars(vec3 d) {
    vec3 a = abs(d);
    vec2 uv;
    uint face;
    if (a.x >= a.y && a.x >= a.z) {
        uv = d.yz / a.x;
        face = d.x > 0.0 ? 0u : 1u;
    } else if (a.y >= a.z) {
        uv = d.xz / a.y;
        face = d.y > 0.0 ? 2u : 3u;
    } else {
        uv = d.xy / a.z;
        face = d.z > 0.0 ? 4u : 5u;
    }
    const float CELLS = 400.0;
    vec2 g = (uv * 0.5 + 0.5) * CELLS;
    vec2 cell = floor(g);
    vec3 h = hash3(uvec3(uvec2(ivec2(cell) + 8192), face));
    if (h.x > 0.45) {
        return vec3(0.0);
    }
    vec3 k = hash3(uvec3(uvec2(ivec2(cell) + 8192), face + 16u));
    vec2 pos = cell + 0.3 + 0.4 * h.yz;
    float pixel = frame.radii.w;
    // Angular size of a grid cell (cells shrink towards the cube corners; close enough).
    float cell_angle = 2.0 / CELLS / (1.0 + dot(uv, uv));
    float d_px = length(g - pos) * cell_angle / pixel;
    // Power-law brightness: many faint stars, a few bright ones.
    float flux = 4e-13 * pow(max(k.x, 2e-4), -1.5);
    vec3 tint = mix(vec3(1.0, 0.72, 0.5), vec3(0.72, 0.82, 1.0), k.y);
    tint = mix(vec3(1.0), tint, k.z);
    float sigma = 0.65; // star size, pixels
    float psf = exp(-d_px * d_px / (2.0 * sigma * sigma)) / (2.0 * PI * sigma * sigma);
    return tint * flux * psf / (pixel * pixel);
}

// The Milky Way, precomputed (src/render/starfield.rs) in an equirectangular map.
vec3 galaxy(vec3 d) {
    vec2 x = vec2(atan(d.z, d.x) / (2.0 * PI) + 0.5, asin(clamp(d.y, -1.0, 1.0)) / PI + 0.5);
    return textureLod(sampler2D(u_galaxy, u_linear), lut_uv(x, vec2(1024.0, 512.0)), 0.0).rgb;
}

// Space behind the atmosphere: stars, galaxy and the sun's disk (with limb darkening).
vec3 space(vec3 dir, bool night) {
    vec3 d = mat3(frame.star_rotation) * dir;
    // Stars and galaxy are expensive (noise) and invisible against a bright sky: skip them.
    vec3 color = night ? (stars(d) + galaxy(d)) * frame.misc.w : vec3(0.0);

    float cos_a = dot(dir, frame.sun.xyz);
    float cos_r = frame.sun.w;
    float sun_r2 = 2.0 * (1.0 - cos_r);    // angular radius^2
    float a2 = 2.0 * (1.0 - cos_a);         // angle to the sun's centre^2
    if (cos_a > cos_r) {
        float mu = sqrt(max(1.0 - a2 / sun_r2, 0.0));
        float limb = 1.0 - 0.6 * (1.0 - mu);
        // Disk radiance so that it integrates to the sun's illuminance.
        color += frame.sun_color.rgb * limb / (PI * sun_r2 * (1.0 - 0.6 / 3.0));
    }
    return color;
}

// The planet's surface where nothing was rendered (e.g. LOD tiles still loading): a dark
// ocean-coloured sphere, lit like the terrain.
vec3 ground(vec3 p) {
    float r = length(p);
    float mu_s = dot(p, frame.sun.xyz) / r;
    vec3 light = sun_transmittance(r, mu_s) * max(mu_s, 0.0) + sky_irradiance(r, mu_s);
    return vec3(0.02, 0.05, 0.08) / PI * light * frame.sun_color.rgb;
}

// Glare around the sun: a cheap stand-in for bloom / scattering in the eye and lens.
// Only while the sun itself is in view (not behind the planet), dimmed and reddened by the
// air in front of it.
vec3 glare(vec3 dir) {
    vec3 cam = frame.camera.xyz;
    float r = length(cam);
    vec3 sun_t = sun_transmittance(r, dot(cam, frame.sun.xyz) / r);
    float a = acos(clamp(dot(dir, frame.sun.xyz), -1.0, 1.0));
    // Exponential falloffs: (energy fraction) * exp(-a / width) / (2 pi width^2).
    const float CORE = 0.012;
    const float WIDE = 0.15;
    float core = 0.02 * exp(-a / CORE) / (2.0 * PI * CORE * CORE);
    float wide = 0.004 * exp(-a / WIDE) / (2.0 * PI * WIDE * WIDE);
    return frame.sun_color.rgb * sun_t * (core + wide);
}

// The half-resolution clouds at this pixel, taking the texels whose depth matches (so they
// don't bleed over the edges of terrain in front of them).
vec4 clouds_at(float depth) {
    ivec2 size = textureSize(sampler2D(u_clouds, u_linear), 0);
    vec2 uv = gl_FragCoord.xy * 0.5 - 0.5;
    ivec2 base = ivec2(floor(uv));
    vec2 f = uv - vec2(base);
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int y = 0; y <= 1; y++) {
        for (int x = 0; x <= 1; x++) {
            ivec2 texel = clamp(base + ivec2(x, y), ivec2(0), size - 1);
            float d = texelFetch(sampler2D(u_depth, u_linear), texel * 2, 0).r;
            float difference = abs(d - depth) / max(max(d, depth), 1e-9);
            float weight = (x == 0 ? 1.0 - f.x : f.x) * (y == 0 ? 1.0 - f.y : f.y);
            weight *= exp(-difference * difference * 60.0) + 1e-4;
            sum += texelFetch(sampler2D(u_clouds, u_linear), texel, 0) * weight;
            total += weight;
        }
    }
    return sum / total;
}

// ACES filmic curve (Narkowicz fit).
vec3 tonemap(vec3 x) {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

vec3 to_srgb(vec3 c) {
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c));
}

vec3 to_linear(vec3 c) {
    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

void main() {
    ivec2 px = ivec2(gl_FragCoord.xy);
    vec4 scene4 = texelFetch(sampler2D(u_scene, u_linear), px, 0);
    vec3 scene = scene4.rgb;
    float depth = texelFetch(sampler2D(u_depth, u_linear), px, 0).r;

    vec4 near_h = frame.inv_view_proj * vec4(v_ndc, 1.0, 1.0);
    vec3 dir = normalize(near_h.xyz / near_h.w);
    bool geometry = depth > 0.0;
    float dist = 1e20;
    if (geometry) {
        vec4 h = frame.inv_view_proj * vec4(v_ndc, depth, 1.0);
        dist = length(h.xyz / h.w);
    }

    vec3 color;
    if (frame.sun_color.w < 0.5) {
        // Underground: no sky, the loaded caves fade into darkness.
        if (geometry) {
            float f = smoothstep(frame.cave.w * 0.15, frame.cave.w, dist);
            color = mix(scene, frame.cave.rgb, f * (2.0 - f));
        } else {
            color = frame.cave.rgb;
        }
    } else {
        vec3 cam = frame.camera.xyz;
        vec3 sun = frame.sun.xyz;
        float t_end = dist;
        bool hits_ground = false;
        if (!geometry) {
            vec2 g = ray_sphere(cam, dir, frame.radii.x);
            if (g.x > 0.0) {
                t_end = g.x;
                hits_ground = true;
            }
        }

        // March through the atmosphere between entering it and the surface / leaving it.
        vec3 transmittance = vec3(1.0);
        vec3 inscatter = vec3(0.0);
        vec2 top = ray_sphere(cam, dir, frame.radii.y);
        float t0 = max(top.x, 0.0);
        float t1 = min(top.y, t_end);
        if (top.y > 0.0 && t1 > t0) {
            // Fewer steps for short (terrain) rays; quadratic spacing puts samples where the
            // air is dense, near the start of rays leaving the ground.
            int steps = geometry ? int(clamp((t1 - t0) / 400.0, 4.0, 12.0)) : 16;
            float cos_theta = dot(dir, sun);
            float phase_r = rayleigh_phase(cos_theta);
            float phase_m = mie_phase(cos_theta);
            float prev = 0.0;
            for (int i = 0; i < steps; i++) {
                float s = (float(i) + 1.0) / float(steps);
                s *= s;
                float dt = (s - prev) * (t1 - t0);
                float t = t0 + (prev + s) * 0.5 * (t1 - t0);
                prev = s;

                vec3 p = cam + dir * t;
                float r = length(p);
                float mu_s = dot(p, sun) / r;
                Medium m = medium(r);
                vec3 sun_t = sun_transmittance(r, mu_s);
                vec3 ms = multiscattering(r, mu_s);
                vec3 scattering = sun_t * (m.rayleigh * phase_r + m.mie * phase_m)
                    + (m.rayleigh + m.mie) * ms;
                vec3 step_t = exp(-m.extinction * dt);
                inscatter += transmittance * (scattering - scattering * step_t)
                    / max(m.extinction, vec3(1e-12));
                transmittance *= step_t;
            }
        }

        vec3 background;
        if (geometry) {
            background = scene;
        } else if (hits_ground) {
            background = ground(cam + dir * t_end);
        } else {
            // The sky in front of them decides whether stars could be seen at all.
            float sky_brightness = dot(inscatter * frame.sun_color.rgb, vec3(0.2126, 0.7152, 0.0722));
            bool night = sky_brightness * frame.camera.w < 0.05;
            background = space(dir, night) + glare(dir);
        }
        color = background * transmittance + inscatter * frame.sun_color.rgb;

        // The weather in the air: clouds, rain curtains, fog, dust, funnels, aurora.
        vec4 clouds = clouds_at(depth);
        color = color * clouds.a + clouds.rgb * frame.sun_color.rgb;

        // Particles seen against the sky (they write no depth): over all of the above.
        if (!geometry) {
            color = color * (1.0 - clamp(scene4.a, 0.0, 1.0)) + scene;
        }
    }

    // Exposure, tone mapping, and dithering (in sRGB space, against banding in the dark
    // gradients of twilight and space).
    vec3 mapped = tonemap(color * frame.camera.w);
    vec3 noise = hash3(uvec3(uvec2(px), 7u)) - 0.5;
    mapped = to_linear(clamp(to_srgb(mapped) + noise / 255.0, 0.0, 1.0));
    out_color = vec4(mapped, 1.0);
}
