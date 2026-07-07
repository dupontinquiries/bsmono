#version 450
//
// sky.vert — fullscreen-triangle sky pass vertex shader.
//
// Emits one oversized triangle covering the screen with NO vertex buffer
// (positions from gl_VertexIndex), identical to present.vert's pattern, so the
// sky pass is a 3-vertex bindless draw. Passes clip-space NDC xy to the
// fragment shader, which rebuilds a world-space view ray via the inverse of the
// camera view_proj to evaluate the analytic sky in that direction.
//
//   vertex 0 -> ndc (-1,-1)
//   vertex 1 -> ndc ( 3,-1)
//   vertex 2 -> ndc (-1, 3)
// Interior exactly covers [-1,1]^2.

layout(location = 0) out vec2 v_ndc;

void main() {
    vec2 uv = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    vec2 ndc = uv * 2.0 - 1.0;
    v_ndc = ndc;
    // z = 1.0 (far plane) so the sky sits behind all geometry when drawn with
    // depth test LESS_OR_EQUAL and depth write off.
    gl_Position = vec4(ndc, 1.0, 1.0);
}
