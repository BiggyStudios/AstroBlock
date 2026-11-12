#version 450

layout(set = 0, binding = 0) uniform texture2DArray u_textures;
layout(set = 0, binding = 1) uniform sampler u_sampler;
layout(set = 0, binding = 10) uniform texture2D u_font;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) flat in uint v_layer;

layout(location = 0) out vec4 out_color;

// v_layer: 0xffffffff = font atlas. Otherwise bits 0-11 texture layer (past the blocks' and items'
// textures: the interface's pictures), bits 16-23 brightness (0 = full), bit 31 sprite (see `ui::Ui`).
void main() {
    if (v_layer == 0xffffffffu) {
        float a = texture(sampler2D(u_font, u_sampler), v_uv).a;
        out_color = vec4(v_color.rgb, v_color.a * a);
        return;
    }
    uint layer = v_layer & 4095u;
    uint b = (v_layer >> 16u) & 255u;
    float brightness = b == 0u ? 1.0 : float(b) / 255.0;
    vec4 t = texture(sampler2DArray(u_textures, u_sampler), vec3(v_uv, float(layer)));
    if ((v_layer & 0x80000000u) != 0u) {
        // Item sprite: alpha is transparency; the colour multiplies.
        out_color = vec4(t.rgb * v_color.rgb * brightness, t.a * v_color.a);
    } else {
        // Block icon: the colour is the biome tint, applied where the texture's alpha says.
        out_color = vec4(t.rgb * mix(vec3(1.0), v_color.rgb, t.a) * brightness, v_color.a);
    }
}
