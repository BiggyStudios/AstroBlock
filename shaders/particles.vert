#version 450

// Particles as camera-facing quads (or streaks along their velocity), pulled straight from
// the particle buffer: 6 vertices per particle, no vertex buffer. Dead particles and those of
// the other pass (cut-out vs blended) collapse outside the clip volume.

#include "particle.glsl"

layout(std430, set = 0, binding = 11) readonly buffer Particles {
    Particle particles[];
};

layout(push_constant) uniform Push {
    mat4 view_proj; // camera-relative world -> clip
    vec4 anchor;    // xyz: particle anchor relative to the camera, w: 1 = blended pass
    vec4 right;     // xyz: camera right, w: first ring index drawn
    vec4 up;        // xyz: camera up, w: ring capacity
} pc;

layout(location = 0) out vec3 v_rel;
layout(location = 1) out vec2 v_uv;
layout(location = 2) flat out uint v_look;
layout(location = 3) out vec4 v_tint;
layout(location = 4) out vec2 v_corner;
layout(location = 5) out float v_alpha;

void main() {
    uint index = (uint(pc.right.w) + uint(gl_VertexIndex) / 6u) % uint(pc.up.w);
    // Two triangles: corners 0 1 2, 0 2 3 of (-1,-1) (1,-1) (1,1) (-1,1).
    uint k = uint(gl_VertexIndex) % 6u;
    uint id = k < 3u ? k : (k == 3u ? 0u : k - 2u);
    vec2 c = vec2(id == 1u || id == 2u ? 1.0 : -1.0, id >= 2u ? 1.0 : -1.0);

    Particle p = particles[index];
    uint flags = (p.look.x >> 8u) & 255u;
    bool blended = (flags & P_BLEND) != 0u;
    float left = p.vel_life.w - p.pos_age.w;
    if (left <= 0.0 || blended != (pc.anchor.w > 0.5)) {
        gl_Position = vec4(0.0, 0.0, -2.0, 1.0);
        return;
    }

    vec3 center = p.pos_age.xyz + pc.anchor.xyz;
    float size = p.phys.z;
    // Nothing may cover the view: particles shrink away within a metre of the eye.
    size *= clamp((length(center) - 0.3) / 0.7, 0.0, 1.0);
    // Cut-out particles shrink away at the end; blended ones fade in and out.
    if (!blended) {
        size *= clamp(left / 0.25, 0.0, 1.0);
    }
    v_alpha = blended ? clamp(left / (0.3 * p.vel_life.w), 0.0, 1.0) * clamp(p.pos_age.w / 0.1, 0.0, 1.0) : 1.0;
    // Big soft puffs fade out as they reach the eye, instead of a disc across the view.
    if (blended && size > 1.0) {
        v_alpha *= clamp((length(center) - size * 0.5) / (size * 1.5), 0.0, 1.0);
    }
    // Streaks right next to the eye would be huge blades across the view.
    if ((flags & P_STRETCH) != 0u) {
        v_alpha *= clamp((length(center) - 1.5) / 4.0, 0.0, 1.0);
    }

    vec3 right = pc.right.xyz * (size * 0.5);
    vec3 up = pc.up.xyz * (size * 0.5);
    if ((flags & P_STRETCH) != 0u) {
        vec3 v = p.vel_life.xyz;
        float speed = length(v);
        vec3 side = cross(v, center);
        if (speed > 0.1 && dot(side, side) > 1e-8) {
            up = v / speed * max(size * 0.5, speed * 0.02);
            right = normalize(side) * (size * 0.5);
        }
    }
    vec3 pos = center + right * c.x + up * c.y;
    gl_Position = pc.view_proj * vec4(pos, 1.0);

    v_rel = pos;
    uint rect = p.look.y;
    vec2 uv0 = vec2(float(rect & 255u), float((rect >> 8u) & 255u));
    vec2 uv1 = vec2(float((rect >> 16u) & 255u), float(rect >> 24u));
    vec2 t = c * 0.5 + 0.5;
    v_uv = vec2(mix(uv0.x, uv1.x, t.x), mix(uv1.y, uv0.y, t.y)) / 16.0;
    v_look = p.look.x;
    v_tint = unpackUnorm4x8(p.look.z);
    v_corner = c;
}
