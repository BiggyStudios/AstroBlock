// Shared weather access: the planet-wide weather maps and the cloud noise. Include after
// atmosphere.glsl.
//
// The maps are cube maps in home's body frame (one texel = one cell of src/weather/sim.rs):
//   A: r cover, g cloud top (fraction of the cloud shell), b precipitation, a storm
//   B: r dust, g snow fraction, b fog, a volcanic ash

layout(set = 0, binding = 13) uniform textureCube u_weather_a;
layout(set = 0, binding = 14) uniform textureCube u_weather_b;
layout(set = 0, binding = 15) uniform texture3D u_cloud_shape;
layout(set = 0, binding = 16) uniform texture3D u_cloud_detail;
layout(set = 0, binding = 20) uniform sampler u_repeat;

// One tile of each noise (m). The shape's must be a multiple of the detail's, and both are
// repeated by `frame.cloud_offset`, which wraps at the shape's tile (src/app/weather.rs).
const float SHAPE_TILE = 4800.0;
const float DETAIL_TILE = 600.0;

// A home-relative point (active frame axes) in home's body frame.
vec3 to_home(vec3 p) {
    return mat3(frame.weather_rotation) * p;
}

vec4 weather_a(vec3 q) {
    return textureLod(samplerCube(u_weather_a, u_linear), q, 0.0);
}

vec4 weather_b(vec3 q) {
    return textureLod(samplerCube(u_weather_b, u_linear), q, 0.0);
}

float cloud_shape(vec3 x, float lod) {
    return textureLod(sampler3D(u_cloud_shape, u_repeat), x / SHAPE_TILE, lod).r;
}

float cloud_detail(vec3 x, float lod) {
    return textureLod(sampler3D(u_cloud_detail, u_repeat), x / DETAIL_TILE, lod).r;
}

// How the clouds light a surface at home-relative `p`:
// x: fraction of direct sunlight getting through, y: fraction of the sky light.
vec2 cloud_light(vec3 p, vec3 sun) {
    float r = length(p);
    // (Only home's own surface has home's clouds above it.)
    if (r > frame.radii.y) {
        return vec2(1.0);
    }
    float height = frame.clouds.x + frame.clouds.y * 0.2;
    float mu = max(dot(p, sun) / r, 0.2);
    vec3 s = p + sun * (max(frame.radii.x + height - r, 0.0) / mu);
    vec3 q = to_home(s);
    vec4 a = weather_a(q);
    vec4 b = weather_b(q);
    float cover = max(a.r, b.a);
    float noise = cloud_shape(q + frame.cloud_offset.xyz, 2.0);
    float blocked = clamp((noise - (1.0 - cover)) / max(cover, 0.1) * 2.5, 0.0, 1.0);
    blocked = blocked * min(1.0, cover * 2.0);
    float dark = a.a * 0.5 + a.b * 0.3;
    return vec2(
        clamp(1.0 - blocked * (0.82 + 0.15 * dark), 0.0, 1.0),
        clamp(1.0 - cover * (0.3 + 0.5 * dark), 0.0, 1.0)
    );
}
