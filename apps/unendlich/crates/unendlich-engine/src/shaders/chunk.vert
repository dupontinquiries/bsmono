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
            float t     = ubo.atmos_params.w * 1.4;
            float phase = world_pos.x * 0.6 + world_pos.z * 0.45 + t;
            float amp   = 0.02 + 0.04 * wmag;
            world_pos.xz += wd * (sin(phase) * amp);
            world_pos.y  += sin(phase * 1.7) * 0.012 * (0.3 + wmag);
        }
        // sway_tag == 2u (Anchor) deliberately falls through: the base vertex
        // stays put so the plant pivots about its root.
    }

    gl_Position = ubo.view_proj * vec4(world_pos, 1.0);
    v_color     = in_color_unorm.rgb;
    v_world_pos = world_pos;
}
