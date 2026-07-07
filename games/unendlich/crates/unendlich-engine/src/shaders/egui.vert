#version 450
//
// egui.vert — overlay vertex shader. egui emits vertices in screen-space
// *points* (origin top-left, y down); this maps them straight to Vulkan NDC
// (also y-down, so no flip). Push constant carries the screen size in points.
//
// Vertex layout matches epaint::Vertex: pos (vec2), uv (vec2), color (rgba8).
// Color is passed through as UNORM 0..1 (no sRGB math in this first cut — that's
// a screenshot-tuning item, see docs/EGUI-OVERLAY.md).

layout(location = 0) in vec2 a_pos;   // points
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_color; // UNORM 0..1

layout(push_constant) uniform PushConstants {
    vec2 screen_size; // in points
} pc;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

void main() {
    v_uv = a_uv;
    v_color = a_color;
    gl_Position = vec4(
        2.0 * a_pos.x / pc.screen_size.x - 1.0,
        2.0 * a_pos.y / pc.screen_size.y - 1.0,
        0.0,
        1.0
    );
}
