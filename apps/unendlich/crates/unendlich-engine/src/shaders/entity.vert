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
    // r599: declared through `disturbers` to match chunk.vert byte-for-byte
    // (std140 offsets must line up — same buffer, same binding, no rebind).
    vec4 flow_coverage;
    vec4 flow_grid[128];
    vec4 fx_params;
    vec4 water_depth_cov;
    vec4 water_depth_grid[64];
    vec4 disturbers[8];        // xyz = world pos, w = strength (w<=0 = empty)
} ubo;

layout(location = 3) in float in_sway;

layout(location = 0) out vec3 v_color;
layout(location = 1) out vec3 v_world_pos;
layout(location = 2) out float v_alpha;

void main() {
    vec3 world_pos = in_position;

    // r599: wind + disturber bending for capes/scarves/cloth — the same model
    // that bows foliage (chunk.vert), scaled by this vertex's `in_sway` weight
    // (0 = rigid body, up to 1 = free tip). Entities are baked world-space
    // per-frame (no chunk-local repack needed), so we can displace directly.
    if (in_sway > 0.001) {
        float wmag = ubo.wind_params.z;
        vec2  wd   = ubo.wind_params.xy;
        float wlen = length(wd);
        wd = wlen > 0.001 ? wd / wlen : vec2(1.0, 0.0);
        float t = ubo.atmos_params.w;
        float pressure = wmag * wmag;
        float phase   = world_pos.x * 0.5 + world_pos.z * 0.4 + t * 1.6;
        float flutter = world_pos.x * 1.9 + world_pos.z * 1.5 + t * 3.4;
        float bend = 0.12 * pressure * in_sway;
        float osc  = (0.6 * sin(phase) + 0.4 * sin(flutter)) * in_sway;
        float amp  = 0.02 + 0.06 * pressure;
        world_pos.xz += wd * (bend + osc * amp);
        world_pos.y  -= bend * 0.25;

        // mob/dragon disturber wake — same falloff shape as chunk.vert.
        for (int di = 0; di < 8; di++) {
            vec4 dstb = ubo.disturbers[di];
            if (dstb.w <= 0.001) { continue; }
            vec2  m_xz  = world_pos.xz - dstb.xz;
            float m_len = length(m_xz);
            float m_reach = 2.0 + pow(dstb.w, 0.333) * 2.2;
            if (m_len > 0.001 && m_len < m_reach) {
                float mf = 1.0 - m_len / m_reach;
                mf *= mf;
                float push = min(mf * 0.18 * dstb.w, 1.5) * in_sway;
                world_pos.xz += (m_xz / m_len) * push;
                world_pos.y  -= min(mf * 0.04 * dstb.w, 0.4) * in_sway;
            }
        }
    }

    gl_Position = ubo.view_proj * vec4(world_pos, 1.0);
    v_color = in_color;
    v_world_pos = world_pos;
    v_alpha = in_alpha;
}
