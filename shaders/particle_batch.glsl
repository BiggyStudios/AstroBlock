// The compute shaders' per-frame input. Must match `BatchHeader` and `GpuEmitter` in
// src/render/particles.rs. Include after particle.glsl.

// One spawn request: `count` particles in a box, with random ranges.
struct Emitter {
    vec4 origin;   // xyz: box centre relative to the (new) anchor, w: burst speed (outwards)
    vec4 extent;   // xyz: box half size (m), w: random speed in any direction
    vec4 velocity; // xyz: base velocity (m/s), w: unused
    vec4 ranges;   // x, y: lifetime range (s), z, w: size range (m)
    vec4 phys;     // x: gravity scale, y: drag, z: bounciness, w: texture piece size (texels, 0 = whole rect)
    uvec4 look;    // x, y, z as in Particle; w: index of its first particle in this frame's batch
    uvec4 count;   // x: particles to spawn
};

// Per frame: the simulation parameters, then this frame's emitters.
layout(std430, set = 0, binding = 12) readonly buffer Batch {
    mat4 b_view_proj;     // camera-relative world -> clip (this frame)
    mat4 b_inv_view_proj; // and back
    vec4 b_anchor;        // xyz: the (new) anchor relative to the camera, w: time step (s)
    vec4 b_planet;        // xyz: planet centre relative to the anchor, w: gravity (m/s^2)
    vec4 b_wind;          // xyz: wind velocity (m/s), w: camera near plane (m)
    vec4 b_shift;         // xyz: new anchor - old anchor (re-base), w: unused
    uvec4 b_info;         // x: new particles, y: emitters, z: ring index of the first new one, w: capacity
    uvec4 b_sim;          // x: first ring index to simulate, y: how many, z: random seed, w: unused
    vec4 b_vortex[3];     // tornadoes: xyz relative to the anchor, w: core radius (0 = none)
    vec4 b_roof_center;   // xyz: the roof map's centre relative to the anchor, w: 1 = roof map in use
    vec4 b_roof_u;        // xyz: its first axis (unit), w: cell size (m)
    vec4 b_roof_v;        // xyz: its second axis (unit)
    float b_roof[2304];   // 48 x 48 heights of the highest block, along "up" from the centre
    Emitter b_emitters[];
};
