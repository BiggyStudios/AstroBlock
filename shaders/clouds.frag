#version 450

// The weather in the air, at half resolution (clouds_resolve.frag accumulates it over time):
//
// - volumetric clouds in a shell above the planet, shaped by the weather maps and 3D noise
// - the low layer under them: rain curtains, fog, dust storms, blizzards, volcanic ash
// - tornado funnels, the northern lights
//
// Output: rgb = light added (relative to the sun's illuminance), a = how much of what is
// behind gets through; sky.frag composites it with `color * a + rgb * sun`. The air between
// the camera and each layer is already accounted for in rgb.

#include "atmosphere.glsl"
#include "weather.glsl"

layout(set = 0, binding = 8) uniform texture2D u_depth; // reverse-Z (0 = sky)

layout(location = 0) in vec2 v_ndc;
layout(location = 0) out vec4 out_color;

const float CLOUD_SIGMA = 0.085;
const float HAZE_TOP = 1700.0;

vec2 ray_sphere(vec3 o, vec3 d, float radius) {
    float r = length(o);
    float b = dot(o, d);
    float c = (r - radius) * (r + radius);
    float disc = b * b - c;
    if (disc < 0.0) {
        return vec2(-1.0, -1.0);
    }
    float s = sqrt(disc);
    return vec2(-b - s, -b + s);
}

float saturate(float x) {
    return clamp(x, 0.0, 1.0);
}

float remap(float x, float a, float b, float c, float d) {
    return c + (x - a) / max(b - a, 1e-4) * (d - c);
}

float henyey(float c, float g) {
    float g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * c, 1e-4), 1.5));
}

// The air between the camera and a distance along the ray: transmittance and in-scattering.
void air_segment(vec3 cam, vec3 dir, float distance_to, out vec3 t_out, out vec3 i_out) {
    t_out = vec3(1.0);
    i_out = vec3(0.0);
    vec2 top = ray_sphere(cam, dir, frame.radii.y);
    float t0 = max(top.x, 0.0);
    float t1 = min(top.y, distance_to);
    if (top.y <= 0.0 || t1 <= t0) {
        return;
    }
    float cos_theta = dot(dir, frame.sun.xyz);
    float phase_r = rayleigh_phase(cos_theta);
    float phase_m = mie_phase(cos_theta);
    const int STEPS = 5;
    for (int i = 0; i < STEPS; i++) {
        float dt = (t1 - t0) / float(STEPS);
        vec3 p = cam + dir * (t0 + (float(i) + 0.5) * dt);
        float r = length(p);
        float mu_s = dot(p, frame.sun.xyz) / r;
        Medium m = medium(r);
        vec3 scattering = sun_transmittance(r, mu_s) * (m.rayleigh * phase_r + m.mie * phase_m)
            + (m.rayleigh + m.mie) * multiscattering(r, mu_s);
        vec3 step_t = exp(-m.extinction * dt);
        i_out += t_out * (scattering - scattering * step_t) / max(m.extinction, vec3(1e-12));
        t_out *= step_t;
    }
}

// Cloud density at `p` (home-relative), given the weather there. `fine` adds the detail noise.
float cloud_density(vec3 p, float r, vec4 a, vec4 b, float lod, bool fine) {
    float hf = (r - frame.radii.x - frame.clouds.x) / frame.clouds.y;
    if (hf < 0.0 || hf > 1.0) {
        return 0.0;
    }
    float ash = smoothstep(0.2, 0.85, b.a);
    float cover = max(a.r, ash * 0.72);
    float top = max(a.g, ash * 0.55);
    if (cover < 0.01) {
        return 0.0;
    }
    float upper = saturate((top - hf) / (top * 0.45 + 0.015));
    if (upper <= 0.0) {
        return 0.0;
    }
    vec3 x = to_home(p) + frame.cloud_offset.xyz;
    float shape = cloud_shape(x, lod);
    // The base undulates a little with the same noise instead of being a flat lid.
    float bottom = saturate((hf - 0.012 - (1.0 - shape) * 0.05) / 0.03);
    float profile = bottom * sqrt(upper);
    if (profile <= 0.0) {
        return 0.0;
    }
    // Cover decides how much of the noise becomes cloud; the profile rounds it off.
    float c = remap(shape * profile, 1.0 - cover, 1.0, 0.0, 1.0);
    if (c <= 0.0) {
        return 0.0;
    }
    if (fine) {
        // Bite small billows out of the edges: wispy below, puffy above.
        float d = cloud_detail(x + vec3(0.0, frame.radii.z * 0.4, 0.0), lod);
        d = mix(1.0 - d, d, saturate(hf * 4.0));
        c = remap(c, d * 0.38, 1.0, 0.0, 1.0);
    }
    return saturate(c) * (1.0 + a.a);
}

// Optical depth towards the sun from `p`.
float light_depth(vec3 p, vec4 a, vec4 b, float lod) {
    vec3 sun = frame.sun.xyz;
    float tau = 0.0;
    float prev = 0.0;
    float t = 45.0;
    for (int i = 0; i < 5; i++) {
        vec3 s = p + sun * t;
        float dt = t - prev;
        tau += cloud_density(s, length(s), a, b, lod + 1.0, false) * dt;
        prev = t;
        t *= 2.3;
    }
    return tau * CLOUD_SIGMA;
}

// Distant storms flicker by themselves: one random cell in a thousand, a few times a second.
float storm_flicker(vec3 q, float storm) {
    if (storm < 0.45) {
        return 0.0;
    }
    vec3 cell = floor(q / length(q) * 38.0);
    float n = fract(sin(dot(cell, vec3(12.9898, 78.233, 37.719)) + floor(frame.radii.z * 11.0) * 1.37) * 43758.5453);
    return step(0.993, n) * storm * storm * 0.5;
}

void march_clouds(vec3 cam, vec3 dir, float t_scene, float jitter, out vec3 rad, out float trans, out float mean_t) {
    rad = vec3(0.0);
    trans = 1.0;
    mean_t = 0.0;
    float base = frame.radii.x + frame.clouds.x;
    float upper = base + frame.clouds.y;
    float r0 = length(cam);
    vec2 outer = ray_sphere(cam, dir, upper);
    if (outer.y <= 0.0) {
        return;
    }
    vec2 inner = ray_sphere(cam, dir, base);
    float t_start;
    float t_end;
    if (r0 < base) {
        t_start = inner.y;
        t_end = outer.y;
    } else if (r0 < upper) {
        t_start = 0.0;
        t_end = inner.x > 0.0 ? inner.x : outer.y;
    } else {
        t_start = outer.x;
        t_end = inner.x > 0.0 ? inner.x : outer.y;
    }
    t_end = min(t_end, t_scene);
    if (t_end <= t_start) {
        return;
    }

    vec3 sun = frame.sun.xyz;
    float cos_theta = dot(dir, sun);
    float phase = mix(henyey(cos_theta, 0.72), henyey(cos_theta, -0.2), 0.3) + 0.05 / (4.0 * PI);
    int steps = clamp(int((t_end - t_start) / 130.0), 22, 56);
    float dt = (t_end - t_start) / float(steps);
    float t = t_start + dt * jitter;
    float weight = 0.0;
    float pixel = frame.radii.w * 2.0;

    for (int i = 0; i < steps; i++) {
        vec3 p = cam + dir * t;
        float r = length(p);
        vec3 q = to_home(p);
        vec4 a = weather_a(q);
        vec4 b = weather_b(q);
        float lod = clamp(log2(max(t * pixel, 1.0) / 75.0), 0.0, 5.0);
        float density = cloud_density(p, r, a, b, lod, true);
        if (density > 0.001) {
            float sigma = density * CLOUD_SIGMA;
            float mu_s = dot(p, sun) / r;
            vec3 sun_t = sun_transmittance(r, mu_s);
            float tau = light_depth(p, a, b, lod);
            // Beer's law plus a softer term for light scattered many times inside the cloud.
            float through = exp(-tau);
            float bounced = exp(-tau * 0.1) * 0.8 + 0.2;
            // Dark bases under thick and rainy cloud.
            float hf = saturate((r - base) / frame.clouds.y);
            float heavy = saturate(a.b * 1.2 + a.a * 0.6);
            float base_dark = mix(1.0, mix(0.55, 0.12, heavy), pow(1.0 - saturate(hf * 2.5), 2.0));
            // Broad lighter and darker patches, so a full cover isn't a flat lid.
            float mottle = cloud_shape((q + frame.cloud_offset.xyz) * 0.37, lod + 1.0);
            base_dark *= mix(0.42, 1.28, smoothstep(0.25, 0.8, mottle));
            vec3 irradiance = sky_irradiance(r, mu_s);
            vec3 ambient = irradiance * mix(0.45, 1.25, hf) * 0.55
                + vec3(0.45, 0.6, 1.0) * frame.misc.z / frame.sun_color.r * 0.3;
            vec3 albedo = mix(vec3(1.0), vec3(0.28, 0.26, 0.24), saturate(b.a * 1.4));
            // Direct light with its bright forward lobe, plus light that has scattered around
            // inside the cloud on its way here (what makes clouds white).
            float direct = phase * through * 2.4;
            float spread = 0.34 * bounced * clamp(mu_s * 0.8 + 0.5, 0.0, 1.0);
            vec3 source = albedo * (sun_t * (direct + spread) + ambient) * base_dark;
            // Lightning inside the cloud.
            float flash = storm_flicker(q, a.a) * 0.4;
            for (int k = 0; k < 2; k++) {
                vec3 to_flash = frame.flashes[k].xyz - dir * t;
                flash += frame.flashes[k].w / (1.0 + dot(to_flash, to_flash) / (900.0 * 900.0));
            }
            source += vec3(0.82, 0.88, 1.0) * flash * 0.02;
            float step_t = exp(-sigma * dt);
            rad += trans * source * (1.0 - step_t);
            mean_t += t * trans * (1.0 - step_t);
            weight += trans * (1.0 - step_t);
            trans *= step_t;
            if (trans < 0.015) {
                break;
            }
        }
        t += dt;
    }
    mean_t = weight > 1e-4 ? mean_t / weight : t_start;
}

// The layer near the ground: rain curtains, fog, dust, blown snow, ash.
void march_haze(vec3 cam, vec3 dir, float t_scene, float jitter, out vec3 rad, out float trans, out float mean_t) {
    rad = vec3(0.0);
    trans = 1.0;
    mean_t = 0.0;
    float R = frame.radii.x;
    float top = R + HAZE_TOP;
    vec2 outer = ray_sphere(cam, dir, top);
    if (outer.y <= 0.0) {
        return;
    }
    vec2 ground = ray_sphere(cam, dir, R);
    float t0 = max(outer.x, 0.0);
    float t1 = min(outer.y, t_scene);
    if (ground.x > 0.0) {
        t1 = min(t1, ground.x);
    }
    t1 = min(t1, t0 + 32000.0);
    if (t1 <= t0) {
        return;
    }
    vec3 sun = frame.sun.xyz;
    float cos_theta = dot(dir, sun);
    float phase = 0.6 / (4.0 * PI) + 0.4 * henyey(cos_theta, 0.6);
    float rain_top = frame.clouds.x;
    float weight = 0.0;
    const int STEPS = 12;
    float prev = 0.0;
    for (int i = 0; i < STEPS; i++) {
        float s = (float(i) + jitter) / float(STEPS);
        s = s * s;
        float dt = (s - prev) * (t1 - t0);
        prev = s;
        float t = t0 + s * (t1 - t0);
        vec3 p = cam + dir * t;
        float r = length(p);
        float h = max(r - R, 0.0);
        vec3 q = to_home(p);
        vec4 a = weather_a(q);
        vec4 b = weather_b(q);
        float rain = a.b * (1.0 - b.g) * smoothstep(rain_top + 200.0, rain_top - 100.0, h);
        float snow = a.b * b.g * smoothstep(rain_top + 200.0, rain_top - 100.0, h);
        float fog = b.b * exp(-h / 130.0);
        // (Thin dust and ash stay invisible, or a whole region would look hazy brown.)
        float dust = smoothstep(0.12, 0.55, b.r) * exp(-h / 380.0);
        float ash = smoothstep(0.1, 0.5, b.a) * exp(-h / 900.0);
        // Billows, so dust and blizzards aren't a uniform wall.
        float billow = cloud_shape(q * 3.0 + frame.cloud_offset.xyz * 3.0, 1.0);
        dust *= smoothstep(0.1, 0.75, billow) * 1.7;
        snow *= 0.55 + billow * 0.9;
        float sigma = 0.0013 * rain + 0.009 * snow + 0.016 * fog + 0.009 * dust + 0.0035 * ash;
        if (sigma > 1e-6) {
            vec3 albedo = (vec3(0.62, 0.66, 0.72) * 0.0013 * rain + vec3(1.0) * 0.009 * snow
                + vec3(0.95) * 0.016 * fog + vec3(1.0, 0.68, 0.38) * 0.009 * dust
                + vec3(0.2, 0.19, 0.18) * 0.0035 * ash) / sigma;
            float mu_s = dot(p, sun) / r;
            float sunlit = 1.0 - 0.85 * max(a.r, b.a);
            // Under cloud the light is the clouds' own: grey, soft and from everywhere.
            vec3 sky = sky_irradiance(r, mu_s);
            float grey = dot(sky, vec3(0.2126, 0.7152, 0.0722));
            float cover = max(a.r, b.a);
            sky = mix(sky, vec3(grey), cover);
            vec3 sun_light = sun_transmittance(r, mu_s);
            vec3 light = sun_light * sunlit * phase * 2.2
                + sky * (1.0 - 0.55 * cover) * 0.32
                + sun_light * max(mu_s, 0.0) * cover * (1.0 - a.a * 0.6) * 0.1
                + vec3(0.45, 0.6, 1.0) * frame.misc.z / frame.sun_color.r * 0.5;
            float flash = 0.0;
            for (int k = 0; k < 2; k++) {
                flash += frame.flashes[k].w * 0.00004;
            }
            vec3 source = albedo * light + vec3(0.8, 0.86, 1.0) * flash;
            float step_t = exp(-sigma * dt);
            rad += trans * source * (1.0 - step_t);
            mean_t += t * trans * (1.0 - step_t);
            weight += trans * (1.0 - step_t);
            trans *= step_t;
        }
    }
    mean_t = weight > 1e-4 ? mean_t / weight : t0;
}

// ---- Columns standing on the ground: tornado funnels and eruption clouds ----------------
//
// Both are real volumes: a density function, marched with steps that jump over the empty
// space around it, and lit by a short march towards the light so that it shades itself.

struct Column {
    vec3 foot;     // camera-relative (m)
    vec3 axis;     // local up
    vec3 e1;
    vec3 e2;
    float height;  // (m)
    float radius;  // a funnel's core, or the vent an eruption rises from (m)
    float power;   // 0..1
    vec2 lean;     // an eruption's drift downwind 1 km up, along e1 and e2 (m)
    bool plume;
};

float fade_out(float a, float b, float x) {
    return 1.0 - smoothstep(a, b, x);
}

// The funnel's centre line wanders, so that it hangs like a rope instead of a straight pole.
vec2 funnel_bend(Column c, float h) {
    float f = clamp(h / c.height, 0.0, 1.0);
    float time = frame.radii.z;
    float amount = c.radius * (0.4 + 3.6 * smoothstep(0.0, 0.7, f));
    return amount * vec2(
        sin(h * 0.0052 + time * 0.37) + 0.45 * sin(h * 0.0137 - time * 0.71),
        cos(h * 0.0043 - time * 0.29) + 0.45 * cos(h * 0.0121 + time * 0.59)
    );
}

// x: density 0..1, y: how much of it is dust, z: how close to the outside it is (0..1).
// `margin` > 0 is how far (m) the point is outside the whole shape.
vec3 funnel_sample(Column c, vec3 rel, bool detail, out float margin) {
    float h = dot(rel, c.axis);
    float f = h / c.height;
    vec2 p = vec2(dot(rel, c.e1), dot(rel, c.e2)) - funnel_bend(c, h);
    float d = length(p);
    float core = c.radius;
    float g = clamp(f, 0.0, 1.0);
    // A narrow rope near the ground, flaring out into the cloud base.
    float wall = core * (0.7 + 1.7 * pow(g, 0.7) + 8.0 * g * g * g * g);
    // The boiling cloud of dust around its foot.
    float skirt_h = core * 2.4 + 16.0;
    float skirt = core * (1.0 + 1.7 * sqrt(clamp(h / skirt_h, 0.0, 1.0))) + 6.0;
    bool low = h < skirt_h * 1.3;
    float outer = max(wall * 1.5, low ? skirt * 1.6 : 0.0);
    margin = max(d - outer, max(-h, h - c.height));
    if (margin > 0.0) {
        return vec3(0.0, 0.0, 1.0);
    }
    float time = frame.radii.z;
    float spin = time * 4.2 - h * 0.017;
    float cs = cos(spin);
    float sn = sin(spin);
    vec2 s = vec2(p.x * cs - p.y * sn, p.x * sn + p.y * cs);
    float n = 0.5;
    if (detail) {
        vec3 q = vec3(s.x, h * 0.4 - time * 16.0, s.y);
        n = 0.62 * cloud_detail(q * 2.4, 0.0) + 0.38 * cloud_shape(q * 12.0, 0.0);
    }
    // Ropes of cloud winding up around it stand out from the surface.
    float helix = sin(atan(p.y, p.x) * 3.0 + h * 0.045 - time * 5.5);
    float rough = d + (n - 0.5) * wall * 0.95 + helix * wall * 0.17;
    float body = fade_out(wall * 0.86, wall * 1.18, rough) * fade_out(0.93, 1.0, f) * smoothstep(0.0, 0.02, f);
    float dust = 0.0;
    float dust_side = 1.0;
    if (low) {
        float m = 0.5;
        if (detail) {
            vec3 q = vec3(s.x, h * 0.7 - time * 9.0, s.y);
            m = 0.45 * cloud_shape(q * 16.0, 0.0) + 0.55 * cloud_detail(q * 3.6, 0.0);
        }
        float edge = d + (m - 0.5) * skirt * 1.5;
        dust = fade_out(skirt * 0.78, skirt * 1.12, edge)
            * fade_out(skirt_h * 0.5, skirt_h * 1.2, h + (m - 0.5) * skirt_h * 1.2);
        dust_side = clamp(smoothstep(0.35, 1.1, d / skirt) * (0.6 + 0.8 * m), 0.0, 1.0);
    }
    float density = max(body, dust);
    float dusty = max(dust > body ? 1.0 : 0.0, fade_out(0.04, 0.3, g));
    float outside = smoothstep(0.4, 1.1, d / max(wall, 1.0)) * (0.7 + 0.6 * n);
    return vec3(density, dusty, dust > body ? dust_side : clamp(outside, 0.0, 1.0));
}

// The same for an eruption: a billowing column leaning downwind, spreading out on top.
// y: how hot it is.
vec3 plume_sample(Column c, vec3 rel, bool detail, out float margin) {
    float h = dot(rel, c.axis);
    float f = h / c.height;
    float up_km = max(h, 0.0) / 1000.0;
    vec2 p = vec2(dot(rel, c.e1), dot(rel, c.e2)) - c.lean * pow(up_km, 1.4);
    float d = length(p);
    float g = clamp(f, 0.0, 1.0);
    float umbrella = smoothstep(0.7, 0.97, g);
    float wide = c.radius + h * 0.2 + umbrella * umbrella * c.height * 0.34;
    float outer = wide * 1.4;
    margin = max(d - outer, max(-h, h - c.height));
    if (margin > 0.0) {
        return vec3(0.0, 0.0, 1.0);
    }
    float time = frame.radii.z;
    float n = 0.5;
    if (detail) {
        vec3 q = vec3(p.x, h - time * 42.0, p.y);
        n = 0.62 * cloud_shape(q * 1.8, 0.0) + 0.38 * cloud_detail(q * 0.75, 0.0);
    }
    // (Billows of a limited size: big holes would open up in the spreading top.)
    float body = fade_out(wide * 0.62, wide * 1.1, d + (n - 0.5) * min(wide, 420.0) * 1.5);
    body *= fade_out(0.86, 1.0, f + (n - 0.5) * 0.2);
    float heat = c.power * exp(-max(h, 0.0) / 150.0);
    float outside = smoothstep(0.3, 1.0, d / max(wide, 1.0)) * (0.65 + 0.7 * n);
    return vec3(body * smoothstep(0.0, 0.25, c.power), heat, clamp(outside, 0.0, 1.0));
}

vec3 column_sample(Column c, vec3 rel, bool detail, out float margin) {
    if (c.plume) {
        return plume_sample(c, rel, detail, margin);
    }
    return funnel_sample(c, rel, detail, margin);
}

// Density met on the way out from `rel` towards the light (m of full density).
float column_shadow(Column c, vec3 rel, vec3 towards, float reach) {
    float depth = 0.0;
    float t = 0.0;
    float dt = reach * 0.06;
    for (int i = 0; i < 6; i++) {
        t += dt;
        float margin;
        vec3 s = column_sample(c, rel + towards * t, false, margin);
        depth += s.x * dt;
        dt *= 1.7;
    }
    return depth;
}

void march_column(vec3 dir, float t_scene, float jitter, Column c, out vec3 rad, out float trans, out float mean_t) {
    rad = vec3(0.0);
    trans = 1.0;
    mean_t = 1e12;
    vec3 mid = c.foot + c.axis * c.height * 0.5;
    float reach = c.height * 0.52 + c.radius * 16.0;
    if (c.plume) {
        vec2 drift = c.lean * pow(c.height * 0.001, 1.4);
        mid += (c.e1 * drift.x + c.e2 * drift.y) * 0.5;
        reach = c.height * 0.85 + length(drift) * 0.6 + c.radius;
    }
    vec2 hit = ray_sphere(-mid, dir, reach);
    if (hit.y <= 0.0) {
        return;
    }
    float t0 = max(hit.x, 0.0);
    float t1 = min(hit.y, t_scene);
    if (t1 <= t0) {
        return;
    }
    vec3 cam = frame.camera.xyz;
    vec3 sun = frame.sun.xyz;
    float sigma = c.plume ? 0.05 : 0.2;
    float fine = c.plume ? max(c.radius * 0.5, 6.0) : max(c.radius * 0.16, 1.0);
    float weight = 0.0;
    float mean = 0.0;
    float t = t0 + jitter * fine;
    vec2 through = vec2(-1.0);
    for (int i = 0; i < 76; i++) {
        if (t >= t1 || trans < 0.012) {
            break;
        }
        float step_size = max(fine, t * 0.0028);
        vec3 rel = dir * t - c.foot;
        float margin;
        vec3 s = column_sample(c, rel, true, margin);
        if (margin > 0.0) {
            t += max(margin * 0.8, step_size);
            continue;
        }
        if (s.x > 0.003) {
            vec3 p = cam + dir * t;
            float r = length(p);
            vec3 up = p / r;
            float mu_s = dot(up, sun);
            if (through.x < 0.0) {
                through = cloud_light(p, sun);
            }
            vec3 sun_light = sun_transmittance(r, mu_s);
            vec3 sky = sky_irradiance(r, mu_s);
            // (The sky under a thick cloud deck is grey, not blue.)
            sky = mix(vec3(dot(sky, vec3(0.2126, 0.7152, 0.0722))), sky, through.y) * through.y
                + sun_light * max(mu_s, 0.0) * (1.0 - through.x) * 0.42;
            // Under a storm deck the light comes from above, not from the hidden sun.
            vec3 key = normalize(mix(up + sun * 0.5, sun, through.x));
            float shadow_reach = c.plume ? max(c.radius * 3.0, 160.0) : c.radius * 5.0;
            float lit = exp(-column_shadow(c, rel, key, shadow_reach) * sigma);
            vec3 albedo;
            vec3 glow = vec3(0.0);
            if (c.plume) {
                albedo = mix(vec3(0.05, 0.047, 0.045), vec3(0.14, 0.13, 0.125), s.z);
                glow = vec3(1.0, 0.32, 0.07) * s.y * s.y * 0.09;
            } else {
                vec3 grey = mix(vec3(0.2, 0.2, 0.21), vec3(0.5, 0.5, 0.52), s.z);
                albedo = mix(grey, vec3(0.2, 0.155, 0.115) * (0.6 + 0.8 * s.z), s.y);
            }
            vec3 light = sun_light * through.x * lit * 0.5
                + sky * (0.18 + 0.82 * lit) * (0.4 + 0.6 * s.z) * 0.62
                + vec3(0.45, 0.6, 1.0) * frame.misc.z / frame.sun_color.r;
            float step_t = exp(-s.x * sigma * step_size);
            vec3 source = albedo * light + glow;
            rad += trans * source * (1.0 - step_t);
            mean += t * trans * (1.0 - step_t);
            weight += trans * (1.0 - step_t);
            trans *= step_t;
        }
        t += step_size;
    }
    mean_t = weight > 1e-4 ? mean / weight : 1e12;
}

Column funnel_column(vec4 tornado) {
    Column c;
    vec3 cam = frame.camera.xyz;
    c.foot = tornado.xyz;
    c.axis = normalize(cam + tornado.xyz);
    c.e1 = normalize(cross(c.axis, vec3(0.31, 0.87, 0.38)));
    c.e2 = cross(c.axis, c.e1);
    c.height = frame.clouds.x - (length(cam + tornado.xyz) - frame.radii.x) + 120.0;
    c.radius = tornado.w;
    c.power = 1.0;
    c.lean = vec2(0.0);
    c.plume = false;
    return c;
}

Column plume_column(vec4 vent, vec4 wind) {
    Column c;
    vec3 cam = frame.camera.xyz;
    c.foot = vent.xyz;
    c.axis = normalize(cam + vent.xyz);
    c.e1 = normalize(cross(c.axis, vec3(0.31, 0.87, 0.38)));
    c.e2 = cross(c.axis, c.e1);
    // It climbs fast at first, to a height set by how hard it erupts.
    c.height = max(min(wind.w * 130.0, 1400.0 + 3000.0 * vent.w), 60.0);
    c.radius = 95.0;
    c.power = vent.w;
    c.lean = vec2(dot(wind.xyz, c.e1), dot(wind.xyz, c.e2)) * 9.0;
    c.plume = true;
    return c;
}

// The northern (and southern) lights: curtains hanging in a ring around each pole.
vec3 aurora(vec3 cam, vec3 dir, float t_scene, float jitter) {
    float strength = frame.cloud_offset.w;
    if (strength <= 0.0) {
        return vec3(0.0);
    }
    float R = frame.radii.x;
    float low = R + 5500.0;
    float high = R + 15000.0;
    vec2 outer = ray_sphere(cam, dir, high);
    if (outer.y <= 0.0) {
        return vec3(0.0);
    }
    vec2 inner = ray_sphere(cam, dir, low);
    float r0 = length(cam);
    float t0;
    float t1;
    if (r0 < low) {
        t0 = inner.y;
        t1 = outer.y;
    } else if (r0 < high) {
        t0 = 0.0;
        t1 = inner.x > 0.0 ? inner.x : outer.y;
    } else {
        t0 = outer.x;
        t1 = inner.x > 0.0 ? inner.x : outer.y;
    }
    t1 = min(t1, min(t_scene, t0 + 60000.0));
    if (t1 <= t0) {
        return vec3(0.0);
    }
    const int STEPS = 18;
    float dt = (t1 - t0) / float(STEPS);
    float time = frame.radii.z;
    vec3 sum = vec3(0.0);
    for (int i = 0; i < STEPS; i++) {
        vec3 p = cam + dir * (t0 + (float(i) + jitter) * dt);
        vec3 q = normalize(to_home(p));
        float colat = acos(clamp(abs(q.y), 0.0, 1.0));
        float band = exp(-pow((colat - 0.42) / 0.13, 2.0));
        if (band < 0.01) {
            continue;
        }
        float h = (length(p) - low) / (high - low);
        vec3 m = vec3(q.x, q.z, 0.0);
        float warp = cloud_shape(vec3(m.xy * 5200.0, time * 8.0), 1.0);
        float folds = sin(colat * 34.0 + warp * 18.0 + sin(atan(q.z, q.x) * 3.0 + time * 0.05) * 2.0);
        float curtain = pow(1.0 - abs(folds), 10.0);
        float rays = cloud_detail(vec3(m.xy * 3600.0, time * 30.0), 0.0);
        rays = 0.35 + 1.1 * smoothstep(0.35, 0.8, rays);
        float profile = smoothstep(0.0, 0.08, h) * pow(1.0 - h, 2.2);
        vec3 color = mix(vec3(0.1, 1.0, 0.35), vec3(0.75, 0.16, 0.62), smoothstep(0.32, 0.9, h));
        sum += color * curtain * rays * band * profile;
    }
    return sum * dt * strength * 1.6e-6;
}

void main() {
    if (frame.sun_color.w < 0.5) {
        out_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }
    vec2 pixel = gl_FragCoord.xy;
    float depth = texelFetch(sampler2D(u_depth, u_linear), ivec2(pixel) * 2, 0).r;
    vec4 near_h = frame.inv_view_proj * vec4(v_ndc, 1.0, 1.0);
    vec3 dir = normalize(near_h.xyz / near_h.w);
    float t_scene = 1e20;
    if (depth > 0.0) {
        vec4 h = frame.inv_view_proj * vec4(v_ndc, depth, 1.0);
        t_scene = length(h.xyz / h.w);
    }

    // Interleaved gradient noise, different every frame (the resolve pass averages it).
    vec2 jittered = pixel + mod(frame.clouds.z, 64.0) * vec2(5.588238, 5.588238);
    float jitter = fract(52.9829189 * fract(dot(jittered, vec2(0.06711056, 0.00583715))));

    vec3 cam = frame.camera.xyz;
    vec3 cloud_rad;
    float cloud_t;
    float cloud_mean;
    march_clouds(cam, dir, t_scene, jitter, cloud_rad, cloud_t, cloud_mean);
    vec3 haze_rad;
    float haze_t;
    float haze_mean;
    march_haze(cam, dir, t_scene, jitter, haze_rad, haze_t, haze_mean);

    // Each layer through the air in front of it: L = Tc L_full + (1 - Tc) I + T C (see the
    // header of this file for how sky.frag uses it).
    vec3 cloud_color = vec3(0.0);
    if (cloud_t < 0.999) {
        vec3 t_air;
        vec3 i_air;
        air_segment(cam, dir, cloud_mean, t_air, i_air);
        cloud_color = (1.0 - cloud_t) * i_air + t_air * cloud_rad;
    }
    vec3 haze_color = vec3(0.0);
    if (haze_t < 0.999) {
        vec3 t_air;
        vec3 i_air;
        air_segment(cam, dir, haze_mean, t_air, i_air);
        haze_color = (1.0 - haze_t) * i_air + t_air * haze_rad;
    }

    // Northern lights are above all of it.
    vec3 lights = aurora(cam, dir, t_scene, jitter);

    vec3 color;
    float through;
    bool above_haze = length(cam) > frame.radii.x + HAZE_TOP;
    if (above_haze) {
        // Looking down: clouds in front of the haze.
        color = lights * cloud_t * haze_t + cloud_color + cloud_t * haze_color;
    } else {
        color = lights * cloud_t * haze_t + haze_color + haze_t * cloud_color;
    }
    through = cloud_t * haze_t;

    // Funnels and eruptions: in front of or behind the rest, whichever they are.
    for (int i = 0; i < 5; i++) {
        vec4 source = i < 3 ? frame.tornadoes[i] : frame.plumes[i - 3];
        if (source.w <= 0.0) {
            continue;
        }
        Column column = i < 3 ? funnel_column(source) : plume_column(source, frame.plume_wind[i - 3]);
        vec3 f_rad;
        float f_t;
        float f_mean;
        march_column(dir, t_scene, jitter, column, f_rad, f_t, f_mean);
        if (f_t < 0.999) {
            vec3 t_air;
            vec3 i_air;
            air_segment(cam, dir, f_mean, t_air, i_air);
            vec3 funnel = (1.0 - f_t) * i_air + t_air * f_rad;
            if (f_mean < cloud_mean || cloud_t > 0.99) {
                // Dust and rain between the camera and it still hide it.
                float front = 0.0;
                if (!above_haze && haze_t < 0.999) {
                    front = clamp(f_mean / max(2.0 * haze_mean, 1.0), 0.0, 1.0);
                }
                float haze_through = pow(max(haze_t, 1e-4), front);
                vec3 before = haze_color * (1.0 - haze_through) / max(1.0 - haze_t, 1e-3);
                color = before + haze_through * funnel + f_t * (color - before);
            } else {
                color = color + through * funnel;
            }
            through *= f_t;
        }
    }
    out_color = vec4(color, through);
}
