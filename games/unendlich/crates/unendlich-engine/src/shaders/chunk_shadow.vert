#version 450
//
// chunk_shadow.vert — depth-only render into one shadow cascade.
//
// Used by the shadow pre-pass that runs BEFORE the main color pass
// each frame. v0.24.2c: invoked once per cascade (4× per frame).
// Each invocation rasterizes loaded chunk meshes into one layer of
// the shadow-map array image, using that cascade's light-space view-
// projection supplied via push constant.
//
// v0.24.20-vertex-pack: vertex inputs are now 8-byte packed
// (`mesher::PackedVertex`). Same format as chunk.vert — see that
// shader for the full layout description. We declare in_color_unorm
// even though shadow rendering doesn't use color, because the vertex
// buffer binding is shared with the main pipeline and Vulkan requires
// the attribute layout to match the pipeline declaration.
//
// World-position reconstruction is identical to chunk.vert: read the
// chunk's origin from the chunks SSBO via gl_InstanceIndex (populated
// by chunk_cull.comp via firstInstance), then world_pos =
// chunk_origin + chunk_local * 32.

layout(location = 0) in uint in_pos_packed; // r349: R32_UINT, manual unpack (Asahi A2B10G10R10 UNORM quirk)
layout(location = 1) in vec4 in_color_unorm;

// Same CameraUbo binding the main pass uses. Pre-v0.24.2c the shadow
// pass read `light_view_proj` from here; v0.24.2c switched to a push
// constant per cascade. We keep the binding declaration for layout
// compatibility with the main pipeline's descriptor set, but no
// field is read from it.
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

// v0.24.20-vertex-pack: chunks-metadata SSBO. Same struct as chunk.vert.
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

// v0.24.2c: per-cascade view-projection supplied per render pass.
// Caller pushes `cascade.view_proj` for whichever cascade is being
// rasterized into the corresponding shadow-map array layer.
layout(push_constant) uniform PC {
    mat4 cascade_view_proj;
} pc;

const float CHUNK_EXTENT = 32.0;

void main() {
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

    // No-op contribution from in_color_unorm so the attribute is not
    // dead-coded away — some drivers reject pipelines whose vertex
    // layout doesn't match the shader's declared inputs.
    vec4 used = in_color_unorm;
    gl_Position = pc.cascade_view_proj * vec4(world_pos, 1.0);
    gl_Position.x += used.x * 0.0;
}
