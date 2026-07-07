#version 450
//
// entity.vert — v0.24.62-entity-shaders
//
// Minimal entity vertex shader. Reads per-vertex world-space position
// and color, transforms to clip space, passes color to fragment.
//
// **Design choice: world-space vertices, not instanced.** Each entity
// is small (a single cube — 8 vertices, 36 indices) and the entity
// count stays low (<100 expected). The CPU-side renderer is expected
// to bake each entity's transformed cube into one batched vertex
// buffer per frame via `entity::snapshot_to_vertices`. Simpler than
// instancing for first-pass; can switch to instancing later if the
// per-frame vertex bake shows up in profiling.
//
// **UBO layout match:** the CameraUbo block here mirrors chunk.vert
// exactly so the entity pipeline can reuse the existing descriptor
// set without rebinding. We only sample `view_proj`; the other fields
// are reserved for future shader work (fog, shadow sampling,
// atmosphere) that this shader doesn't perform yet.
//
// **No fog, no shadows, no lighting** in this scaffold. Future
// iterations should:
// - Apply fog via atmos_params.x against v_world_pos distance.
// - Sample the cascaded shadow map (light_view_proj) to darken
//   entities under tree canopies.
// - Add per-face dim factor matching the chunk mesher's FACE_LIGHT
//   constants so entities visually match the world's lighting style.

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_color;
layout(location = 2) in float in_alpha;

// CameraUbo layout MUST match the one declared in chunk.vert. The
// renderer binds the same buffer to both pipelines; field offset
// mismatch would silently corrupt entity rendering. Whenever
// chunk.vert's UBO changes, this block must change in lock-step.
layout(set = 0, binding = 0) uniform CameraUbo {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 sun_dir;
    vec4 sun_color;
    vec4 sky_color;
    vec4 ambient_color;
    vec4 atmos_params;
    vec4 wind_params;
    vec4 viewport_rect;
    mat4 light_view_proj;
} ubo;

layout(location = 0) out vec3 v_color;
layout(location = 1) out vec3 v_world_pos;
layout(location = 2) out float v_alpha;

void main() {
    gl_Position = ubo.view_proj * vec4(in_position, 1.0);
    v_color = in_color;
    v_world_pos = in_position;
    v_alpha = in_alpha;
}
