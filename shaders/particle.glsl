// Shared by the particle shaders. Must match `GpuParticle`, `GpuEmitter` and `BatchHeader`
// in src/render/particles.rs.
//
// Positions are relative to the particle *anchor*, a point near the camera that the CPU moves
// (re-bases) when the camera gets far from it, so f32 stays precise anywhere in the world.

struct Particle {
    vec4 pos_age;  // xyz: position relative to the anchor (m), w: age (s)
    vec4 vel_life; // xyz: velocity (m/s, world frame), w: lifetime (s); dead once age >= life
    vec4 phys;     // x: gravity scale, y: drag (1/s, towards the wind), z: size (m), w: bounciness
    // x: texture layer (bits 0-7), flags (8-15), block light (16-19), sky light (20-23)
    // y: texture rect in texels (u0, v0, u1, v1, one byte each)
    // z: tint (RGBA8, linear)
    // w: random seed
    uvec4 look;
};

// Flags (bits 8-15 of look.x).
const uint P_BOUNCE = 1u;      // collides with the depth buffer and bounces / settles
const uint P_DIE_ON_HIT = 2u;  // collides and disappears (rain)
const uint P_STRETCH = 4u;     // a streak along the velocity instead of a square
const uint P_BLEND = 8u;       // translucent (blended pass, no depth write)
const uint P_ROUND = 16u;      // a soft disc instead of a texture
const uint P_TINT_MASK = 32u;  // block texture: alpha is the tint mask, not transparency
const uint P_GLOW = 64u;       // emits its own light (flames, plasma): not lit by the sun
const uint P_RESTING = 128u;   // (internal) settled on a surface

