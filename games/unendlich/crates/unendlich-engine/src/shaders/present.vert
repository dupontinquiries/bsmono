#version 450

// Fullscreen-triangle present/post-process vertex shader.
//
// Emits a single oversized triangle covering the whole screen with NO vertex
// buffer (positions derived from gl_VertexIndex), so the present pass is a
// 3-vertex draw with no bindings. Used by present.frag to sample the offscreen
// HDR scene color, tonemap/grade it, and write the swapchain. Part of the
// gated post-FX pipeline (UNENDLICH_POSTFX) that bloom + SSR build on; see
// toolkit/harness/RULESET.md §ssr.
//
//   vertex 0 -> uv (0,0)  pos (-1,-1)
//   vertex 1 -> uv (2,0)  pos ( 3,-1)
//   vertex 2 -> uv (0,2)  pos (-1, 3)
// The triangle's interior exactly covers [-1,1]^2; uv interpolates to [0,1]
// across the visible region.

layout(location = 0) out vec2 v_uv;

void main() {
    v_uv = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    gl_Position = vec4(v_uv * 2.0 - 1.0, 0.0, 1.0);
}
