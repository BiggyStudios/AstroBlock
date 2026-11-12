// Shared atmosphere code: frame uniforms, LUT access, the scattering medium.
// Must match src/render/atmosphere.rs (LUT parameterisations and the medium) and
// `FrameUniforms` in src/render/mod.rs (uniform layout).

layout(set = 0, binding = 2) uniform sampler u_linear;          // LUTs: linear, clamp to edge
layout(set = 0, binding = 3) uniform texture2D u_transmittance; // 256 x 64
layout(set = 0, binding = 4) uniform texture2D u_multiscattering; // 32 x 32
layout(set = 0, binding = 5) uniform texture2D u_irradiance;    // 64 x 16

layout(set = 0, binding = 6) uniform Frame {
    mat4 inv_view_proj; // clip space -> camera-relative world space (rotation-only view)
    mat4 star_rotation; // planet body frame -> inertial frame (orients the star field)
    vec4 camera;        // xyz: camera position relative to the planet centre (m), w: exposure
    vec4 sun;           // xyz: direction to the sun, w: cos of the sun's angular radius
    vec4 sun_color;     // rgb: sun illuminance above the atmosphere, w: 1 = sky on, 0 = underground
    vec4 radii;         // x: sea level radius, y: top of the atmosphere, z: time (s), w: pixel angle
    vec4 rayleigh;      // rgb: scattering per m at sea level, w: scale height (m)
    vec4 mie;           // x: scattering, y: extinction, z: scale height, w: phase anisotropy g
    vec4 ozone;         // rgb: absorption per m at the layer's peak, w: peak height (m)
    vec4 misc;          // x: ozone half width (m), y: ground albedo, z: night ambient, w: star boost
    vec4 cave;          // rgb: underground fog colour, w: underground fog distance (m)
    mat4 target;        // camera-relative position -> targeted block box, [-1, 1]^3 inside
    vec4 target_info;   // x: 1 if a block is targeted, y: mining progress (0..1)
    mat4 weather_rotation; // active frame axes -> home's body frame (the weather maps)
    mat4 prev_view_proj;   // last frame's camera-relative world -> clip
    vec4 prev_offset;      // xyz: camera now - camera last frame, w: 1 = last frame's clouds are usable
    vec4 clouds;           // x: cloud base above sea level (m), y: shell thickness (m), z: frame number, w: lightning light on the ground
    vec4 cloud_offset;     // xyz: noise offset carrying the clouds along (m), w: northern lights strength
    vec4 flashes[2];       // xyz: lightning, camera-relative (m), w: intensity
    vec4 tornadoes[3];     // xyz: foot of the funnel, camera-relative (m), w: core radius (0 = none)
    vec4 plumes[2];        // xyz: an erupting volcano's vent, camera-relative (m), w: how hard (0 = none)
    vec4 plume_wind[2];    // xyz: wind at that volcano (m/s), w: seconds since it began
} frame;

const float PI = 3.14159265358979;
// Irradiance of full (level 15) block light, in sun-illuminance units. Keep in sync with
// `TORCH_IRRADIANCE` in app.rs (auto exposure).
const float TORCH_IRRADIANCE = 0.6;

struct Medium {
    vec3 rayleigh;   // scattering
    float mie;       // scattering
    vec3 extinction;
};

Medium medium(float r) {
    float h = max(r - frame.radii.x, 0.0);
    Medium m;
    m.rayleigh = frame.rayleigh.rgb * exp(-h / frame.rayleigh.w);
    float mie_density = exp(-h / frame.mie.z);
    float ozone = max(1.0 - abs(h - frame.ozone.w) / frame.misc.x, 0.0);
    m.mie = frame.mie.x * mie_density;
    m.extinction = m.rayleigh + vec3(frame.mie.y * mie_density) + frame.ozone.rgb * ozone;
    return m;
}

float rayleigh_phase(float c) {
    return 3.0 / (16.0 * PI) * (1.0 + c * c);
}

float mie_phase(float c) {
    float g = frame.mie.w;
    float k = 3.0 / (8.0 * PI) * (1.0 - g * g) / (2.0 + g * g);
    return k * (1.0 + c * c) / pow(1.0 + g * g - 2.0 * g * c, 1.5);
}

float distance_to_top(float r, float mu) {
    float top = frame.radii.y;
    float disc = r * r * (mu * mu - 1.0) + top * top;
    return max(-r * mu + sqrt(max(disc, 0.0)), 0.0);
}

// Zenith cosine of the geometric horizon seen from radius r.
float horizon_mu(float r) {
    float s = min(frame.radii.x / max(r, frame.radii.x), 1.0);
    return -sqrt(max(1.0 - s * s, 0.0));
}

// 0..1 coordinates (texel centres at 0 and 1) -> texture uv.
vec2 lut_uv(vec2 x, vec2 size) {
    return (x * (size - 1.0) + 0.5) / size;
}

vec3 transmittance_lut(float r, float mu) {
    float bottom = frame.radii.x;
    float top = frame.radii.y;
    if (r > top) {
        // Out above the air (in orbit, on the moon): a ray that misses it keeps all its
        // light, one that goes in is looked up from where it enters. (Clamping r to the top
        // instead made any ray sideways of the planet a grazing one: a red sun and moon.)
        float disc = r * r * (mu * mu - 1.0) + top * top;
        if (mu >= 0.0 || disc <= 0.0) {
            return vec3(1.0);
        }
        float d = -r * mu - sqrt(disc);
        mu = clamp((r * mu + d) / top, -1.0, 1.0);
        r = top;
    }
    r = max(r, bottom);
    float big_h = sqrt(top * top - bottom * bottom);
    float rho = sqrt(max(r * r - bottom * bottom, 0.0));
    float d = distance_to_top(r, mu);
    float d_min = top - r;
    float d_max = rho + big_h;
    vec2 x = vec2((d - d_min) / max(d_max - d_min, 1e-3), rho / big_h);
    return textureLod(sampler2D(u_transmittance, u_linear), lut_uv(x, vec2(256.0, 64.0)), 0.0).rgb;
}

// Sunlight transmittance at radius r for sun zenith cosine mu_s, including the planet's
// shadow (softened over the sun's disk and the refraction we don't model).
vec3 sun_transmittance(float r, float mu_s) {
    float h = horizon_mu(r);
    float visible = smoothstep(h - 0.01, h + 0.01, mu_s);
    return transmittance_lut(r, max(mu_s, h)) * visible;
}

vec2 sun_coords(float r, float mu_s) {
    float h = (r - frame.radii.x) / (frame.radii.y - frame.radii.x);
    return vec2(mu_s * 0.5 + 0.5, clamp(h, 0.0, 1.0));
}

// Multiple scattering (all orders >= 2), per unit sun illuminance.
vec3 multiscattering(float r, float mu_s) {
    vec2 uv = lut_uv(sun_coords(r, mu_s), vec2(32.0, 32.0));
    return textureLod(sampler2D(u_multiscattering, u_linear), uv, 0.0).rgb;
}

// Sky light on an up-facing surface, per unit sun illuminance.
vec3 sky_irradiance(float r, float mu_s) {
    vec2 uv = lut_uv(sun_coords(r, mu_s), vec2(64.0, 16.0));
    return textureLod(sampler2D(u_irradiance, u_linear), uv, 0.0).rgb;
}
