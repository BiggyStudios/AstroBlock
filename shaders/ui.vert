#version 450

// Screen-space overlay (console, HUD, hotbar). Positions in pixels, top-left origin.
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color; // sRGB + alpha
layout(location = 3) in uint in_layer; // block texture layer, or 0xffffffff for the font

layout(push_constant) uniform Push {
    vec4 screen; // xy: screen size in pixels
} pc;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;
layout(location = 2) flat out uint v_layer;

void main() {
    gl_Position = vec4(in_pos / pc.screen.xy * 2.0 - 1.0, 0.0, 1.0);
    v_uv = in_uv;
    // The swapchain is sRGB: blend in linear space.
    v_color = vec4(pow(in_color.rgb, vec3(2.2)), in_color.a);
    v_layer = in_layer;
}
