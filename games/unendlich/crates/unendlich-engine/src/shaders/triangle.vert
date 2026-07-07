#version 450
//
// triangle.vert — hardcoded colored triangle
//
// No vertex buffer. The three vertex positions and colors are baked in by
// gl_VertexIndex. This is the absolute minimum graphics pipeline that still
// proves the swapchain + render pass + pipeline + descriptor wiring works.

layout(location = 0) out vec3 v_color;

vec2 positions[3] = vec2[](
    vec2( 0.0, -0.5),
    vec2( 0.5,  0.5),
    vec2(-0.5,  0.5)
);

vec3 colors[3] = vec3[](
    vec3(1.0, 0.0, 0.0),
    vec3(0.0, 1.0, 0.0),
    vec3(0.0, 0.0, 1.0)
);

void main() {
    gl_Position = vec4(positions[gl_VertexIndex], 0.0, 1.0);
    v_color     = colors[gl_VertexIndex];
}
