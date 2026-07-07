#version 450
//
// egui.frag — overlay fragment shader. Tint (vertex color) × texture sample.
// The texture is either the font atlas (white RGB + coverage alpha) or a user
// texture (e.g. the frosted-blur image). Straight alpha blending is set on the
// pipeline. sRGB handling is intentionally omitted for the first cut and tuned
// from a screenshot if colors read wrong.

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(set = 0, binding = 0) uniform sampler2D u_tex;

layout(location = 0) out vec4 f_color;

void main() {
    f_color = v_color * texture(u_tex, v_uv);
}
