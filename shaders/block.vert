#version 450

// xyz: mesh-local position in lattice units (times pc.params.x = meters).
// w: biome tint (grass / foliage / water) as RGB565 of sqrt(linear colour).
layout(location = 0) in ivec4 in_position;
// Packed: bits 0-7 texture layer, 8-9 quad corner (uv), 10-11 ambient occlusion,
// 12-15 sky light, 16-20 / 21-25 quad size in blocks minus one (texture repeats per block).
// Bit 26 (models, see `models.rs`): bits 16-20 / 21-25 are this vertex's uv in texels.
// Bit 27: cutout sprite (alpha = transparency). Bits 28-31: block light (torches), 0-15.
layout(location = 1) in uint in_data;

layout(push_constant) uniform Push {
    mat4 mvp;     // projection * view-rotation * translate(mesh origin - camera)
    vec4 offset;  // xyz: mesh origin relative to camera, w: inner discard radius
    vec4 params;  // x: meters per position unit, y: outer discard radius, zw: fade
} pc;

layout(location = 0) out vec3 v_rel;    // camera-relative position
layout(location = 1) out vec2 v_uv;
// flat: an interpolated float layer can land on N-1. Bits 0-7 layer, bit 8 cutout.
layout(location = 4) flat out uint v_layer;
layout(location = 2) out vec4 v_tint;
layout(location = 3) out vec3 v_light;  // x: ambient occlusion, y: sky light, z: block light

void main() {
    vec3 position = vec3(in_position.xyz) * pc.params.x;
    gl_Position = pc.mvp * vec4(position, 1.0);
    v_rel = position + pc.offset.xyz;

    uint layer = in_data & 255u;
    uint corner = (in_data >> 8u) & 3u;
    uint ao = (in_data >> 10u) & 3u;
    uint light = (in_data >> 12u) & 15u;

    uint bits_u = (in_data >> 16u) & 31u;
    uint bits_v = (in_data >> 21u) & 31u;
    if ((in_data & (1u << 26u)) != 0u) {
        v_uv = vec2(float(bits_u), float(bits_v)) / 16.0;
    } else {
        v_uv = vec2(float((corner & 1u) * (bits_u + 1u)), float((corner >> 1u) * (bits_v + 1u)));
    }
    v_layer = layer | (((in_data >> 27u) & 1u) << 8u);
    uint t = uint(in_position.w) & 65535u;
    vec3 tint = vec3(float(t >> 11u) / 31.0, float((t >> 5u) & 63u) / 63.0, float(t & 31u) / 31.0);
    v_tint = vec4(tint * tint, 1.0);
    uint block_light = in_data >> 28u;
    v_light = vec3(0.45 + 0.55 * float(ao) / 3.0, float(light) / 15.0, float(block_light) / 15.0);
}
