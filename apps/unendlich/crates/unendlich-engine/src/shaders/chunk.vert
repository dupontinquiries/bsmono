#version 450
//
// chunk.vert — render meshed chunks with a view-projection transform.
//
// v0.24.20-vertex-pack: vertex inputs are now 8-byte packed
// (`mesher::PackedVertex`):
//
//   location 0: A2B10G10R10_UNORM_PACK32 → vec4 in [0,1]. xyz is the
//               vertex's chunk-LOCAL position, scaled so [0,1] maps to
//               [0,32] blocks. w (2-bit alpha) is currently reserved.
//   location 1: R8G8B8A8_UNORM           → vec4 in [0,1]. Flat-shaded
//               color in rgb (face brightness baked in by the mesher);
//               a is currently reserved.
//
// To reconstruct world position the shader needs the chunk's world
// origin. It reads that from the chunks-metadata SSBO at binding 3,
// indexed by `gl_InstanceIndex`. The cull shader (chunk_cull.comp)
// writes each visible chunk's slot index into the indirect command's
// `firstInstance` field, so `gl_InstanceIndex` = chunk slot.
//
//   chunk_origin.xz = chunks[slot].aabb_min.xz  (existing — these are
//                     the chunk's XZ origin since the AABB is built
//                     from `coord.block_origin()`).
//   chunk_origin.y  = chunks[slot].aabb_min.w   (NEW — what was a
//                     padding slot now carries `coord.y * 32`. The
//                     cull shader still uses `aabb_min.y` (tight Y
//                     bound) for frustum testing, unchanged).
//
// Vertex outputs:
//   - v_color: rgb of the packed color attribute (same semantics as
//     the pre-pack path).
//   - v_world_pos: reconstructed world-space position for the fragment
//     shader's distance fog, view-direction lighting, etc.

layout(location = 0) in uint in_pos_packed; // r349: R32_UINT, manual unpack (Asahi A2B10G10R10 UNORM quirk)
layout(location = 1) in vec4 in_color_unorm;

// Camera + atmosphere UBO. Visible to both stages (see chunk_pipeline.rs).
// The vertex shader only needs view_proj from this; the fragment shader
// uses everything else. Sharing the binding keeps the descriptor set
// minimal — one buffer, one binding, both stages.
layout(set = 0, binding = 0) uniform CameraUbo {
    mat4 view_proj;
    vec4 camera_pos;       // xyz = world-space eye, w = unused
    vec4 sun_dir;          // xyz = direction sunlight TRAVELS (sun→ground), w = directional intensity
    vec4 sun_color;        // rgb = sun radiance, a = unused
    vec4 sky_color;        // rgb = sky/fog color, a = unused
    vec4 ambient_color;    // rgb = ambient ground-bounce term, a = unused
    vec4 atmos_params;     // x = fog_density, y = humidity, z = temperature, w = time_of_day
    vec4 wind_params;      // xy = wind vector at player, z = wind magnitude, w = unused
    vec4 viewport_rect;    // xy = viewport size px, zw = viewport offset px
    mat4 light_view_proj;  // light-space VP for shadow-map sampling (Phase 2 will read this)
    // r597: the block must declare the FULL CameraUbo layout up to `disturbers`
    // (mirrors chunk.frag lines 52-68) so std140 offsets line up. Fields between
    // here and `disturbers` are unused by this shader but must be present.
    vec4 flow_coverage;        // r419
    vec4 flow_grid[128];       // r419
    vec4 fx_params;            // r427
    vec4 water_depth_cov;      // r431
    vec4 water_depth_grid[64]; // r431
    vec4 disturbers[8];        // r597: xyz = world pos, w = strength (w<=0 = empty)
} ubo;

// v0.24.20-vertex-pack: chunks-metadata SSBO. Mirrors the layout of
// `chunk_indirect::ChunkDraw` and `chunk_cull.comp::ChunkDraw`. The
// vertex shader only reads `aabb_min`, but the whole struct must be
// declared for std430 layout to match the buffer the renderer binds.
struct ChunkDraw {
    uint  index_count;
    uint  first_index;
    int   vertex_offset;
    uint  flags;
    vec4  aabb_min;        // xyz = chunk_origin.x, tight_min_y, chunk_origin.z
                           // w   = chunk_origin.y  (was padding pre-pack)
    vec4  aabb_max;
};

layout(std430, set = 0, binding = 3) readonly buffer Chunks {
    ChunkDraw chunks[];
};

layout(location = 0) out vec3 v_color;
layout(location = 1) out vec3 v_world_pos;

const float CHUNK_EXTENT = 32.0;

void main() {
    // Reconstruct world position from chunk-local quantized vertex.
    //
    // Note: gl_InstanceIndex in Vulkan = gl_InstanceID + firstInstance.
    // Each draw command has instance_count=1, so gl_InstanceID is 0
    // and gl_InstanceIndex = firstInstance = chunk_slot_index (set by
    // chunk_cull.comp).
    ChunkDraw c = chunks[gl_InstanceIndex];
    vec3 chunk_origin = vec3(c.aabb_min.x, c.aabb_min.w, c.aabb_min.z);
    // r349: unpack A2B10G10R10 by hand from the raw uint, bypassing the
    // driver's UNORM decode (Asahi Mesa was scaling these wrong → giant
    // geometry). Bit layout mirrors PackedVertex::pack_pos exactly:
    // R=0..10, G=10..20, B=20..30 (each /1023), A2=30..32 = sway tag.
    float _ux = float(in_pos_packed & 0x3ffu) / 1023.0;
    float _uy = float((in_pos_packed >> 10) & 0x3ffu) / 1023.0;
    float _uz = float((in_pos_packed >> 20) & 0x3ffu) / 1023.0;
    uint _sway_bits = (in_pos_packed >> 30) & 0x3u;
    vec3 local_pos = vec3(_ux, _uy, _uz) * CHUNK_EXTENT;
    vec3 world_pos = chunk_origin + local_pos;

    // r330-gpu: foliage sway is gated by an EXPLICIT per-vertex tag set by the
    // mesher (sim::foliage_sway), not by green vertex color. The old color test
    // (vc.g dominant) also matched the solid green TOP face of grass blocks, so
    // they rippled like loose foliage (§4b), and it had no notion of a plant's
    // footing so bases tore out of the ground.
    //
    // The tag rides the 2 reserved alpha bits of the packed position
    // (A2B10G10R10_UNORM_PACK32); the hardware presents them here as
    // in_pos_unorm.w in {0, 1/3, 2/3, 1}:
    //   0 (None)   : solid geometry incl. green grass-block tops — NO sway
    //   1 (Sway)   : free foliage vertex (leaf cube / plant tip) — full sway
    //   2 (Anchor) : planted plant base — eligible foliage, but held still
    // r323 timing note: atmos_params.w is time_of_day IN SECONDS (advances
    // ~1/s), so a small factor gives a gentle ~0.2-0.3 Hz sway.
    {
        uint sway_tag = _sway_bits;
        if (sway_tag == 1u) {
            float wmag = ubo.wind_params.z;
            vec2  wd   = ubo.wind_params.xy;
            float wlen = length(wd);
            wd = wlen > 0.001 ? wd / wlen : vec2(1.0, 0.0);
            float t = ubo.atmos_params.w; // seconds (r323)
            // Physics-based sway: aerodynamic drag rises with the SQUARE of wind
            // speed, so foliage takes on a steady DOWNWIND lean (∝ wmag²) and then a
            // slow sway + faster flutter oscillate about that lean — rather than a
            // symmetric wobble about vertical. At zero wind there's only a whisper of
            // idle motion. (Per-tree stiffness from trunk-thickness / flexibility genes
            // — sim::plant_survival::sway_offset — is a mesher-encoded follow-up.)
            float pressure = wmag * wmag;
            float phase   = world_pos.x * 0.6 + world_pos.z * 0.45 + t * 1.4;
            float flutter = world_pos.x * 1.7 + world_pos.z * 1.3 + t * 3.1;
            float bend = 0.10 * pressure;                        // steady downwind lean
            float osc  = 0.6 * sin(phase) + 0.4 * sin(flutter);  // sway + flutter
            float amp  = 0.015 + 0.05 * pressure;                // idle whisper + gust
            world_pos.xz += wd * (bend + osc * amp);
            world_pos.y  -= bend * 0.2;                          // tip dips as it leans

            // r591/r592: DISTURBER — foliage shoved aside by nearby heavy things. The
            // camera (≈ the player) is the first disturber (mobs become extra entries in
            // a disturber array later). Strength comes live from the F3 slider via
            // ubo.sun_color.a (1.0 = default, 0 = off); higher also widens the reach so
            // cranking it is a decisive test. Radial push away, fading to 0 at `reach`,
            // roots (tag 2) stay so plants pivot at the base.
            float dstr = ubo.sun_color.a;
            if (dstr > 0.001) {
                vec2  d_xz  = world_pos.xz - ubo.camera_pos.xz;
                float d_len = length(d_xz);
                float reach = 3.0 + dstr * 0.3;                  // widens as you crank
                if (d_len > 0.001 && d_len < reach) {
                    float fall = 1.0 - d_len / reach;
                    fall *= fall;
                    world_pos.xz += (d_xz / d_len) * fall * 0.35 * dstr;
                    world_pos.y  -= fall * 0.05 * dstr;
                }
            }

            // r597: MOB/DRAGON disturbers — nearby heavy bodies shove foliage aside
            // (the bow-wave). Each slot is xyz = world pos, w = strength (heft from
            // stature). Radial push away, reach and push both scale with strength so
            // a dragon parts a wide swath and a wolf barely a ripple; capped so tips
            // bend hard without stretching. Empty slots (w<=0) cost one branch.
            for (int di = 0; di < 8; di++) {
                vec4 dstb = ubo.disturbers[di];
                if (dstb.w <= 0.001) { continue; }
                vec2  m_xz  = world_pos.xz - dstb.xz;
                float m_len = length(m_xz);
                float m_reach = 2.0 + pow(dstb.w, 0.333) * 2.2;  // ~8.5 blocks at str 35
                if (m_len > 0.001 && m_len < m_reach) {
                    float mf = 1.0 - m_len / m_reach;
                    mf *= mf;
                    float push = min(mf * 0.18 * dstb.w, 1.5);   // capped bend
                    world_pos.xz += (m_xz / m_len) * push;
                    world_pos.y  -= min(mf * 0.04 * dstb.w, 0.4);
                }
            }
        }
        // sway_tag == 2u (Anchor) deliberately falls through: the base vertex
        // stays put so the plant pivots about its root.
    }

    gl_Position = ubo.view_proj * vec4(world_pos, 1.0);
    v_color     = in_color_unorm.rgb;
    v_world_pos = world_pos;
}
