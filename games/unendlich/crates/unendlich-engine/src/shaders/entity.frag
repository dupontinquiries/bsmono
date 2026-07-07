#version 450
//
// entity.frag — v0.24.63-entity-fog (§5a: mobs in the fog pass)
//
// Entities now composite the volumetric froxel fog so mobs and dropped
// items sit *in* the atmosphere instead of popping flat against fogged
// terrain. This mirrors chunk.frag's froxel CONSUMPTION exactly so an
// entity fragment and the terrain behind it integrate through the same
// volume slice and match seamlessly.
//
// Why this is shader-only (no Rust/descriptor change): the entity
// pipeline reuses the chunk pipeline's set-0 layout (camera UBO @0,
// shadow @1, froxel @2), and the renderer writes the froxel image into
// binding 2 of the very descriptor set the entity draw binds. The
// volume was already bound here — entity.frag simply never declared or
// sampled it.
//
// Fallback: if the volume is unfilled this frame (sentinel alpha==0,
// e.g. bins that don't run the froxel-fill compute pass), we render the
// entity un-fogged — its prior behavior, and strictly better than
// sampling garbage. chunk.frag's heavier per-fragment march fallback is
// intentionally NOT duplicated here (entities are few and small; a
// missing-volume frame just shows them crisp for that frame).
//
// Scope note: this composites FOG only, per the handoff §5a spec
// ("sample transmittance/in-scatter, composite identically, ~15 lines").
// Tonemap/contrast/vignette parity with chunk.frag is a separate item.

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec3 v_world_pos;
layout(location = 2) in float v_alpha;

layout(location = 0) out vec4 out_color;

// Camera + atmosphere UBO. Identical block to chunk.frag / entity.vert;
// the entity pipeline binds the same descriptor set, so camera_pos and
// viewport_rect are the same values the terrain uses. Do NOT reorder —
// must match CameraUbo everywhere (chunk.vert is canonical).
layout(set = 0, binding = 0) uniform CameraUbo {
    mat4 view_proj;
    vec4 camera_pos;       // xyz = eye position
    vec4 sun_dir;
    vec4 sun_color;
    vec4 sky_color;
    vec4 ambient_color;
    vec4 atmos_params;
    vec4 wind_params;
    vec4 viewport_rect;    // xy = viewport size px, zw = viewport offset px
    mat4 light_view_proj;
} ubo;

// 3D froxel scattering volume — binding 2 of the shared set (same image
// chunk.frag samples). Each texel stores cumulative (rgb_scattered,
// transmittance) from the camera through that froxel's far edge.
layout(set = 0, binding = 2) uniform sampler3D froxel_volume;

// Must match chunk.frag's froxel extents so the W mapping is identical.
const float FROXEL_NEAR = 0.5;
const float FROXEL_FAR  = 200.0;

#ifndef ENTITY_RIM_LIGHT
#define ENTITY_RIM_LIGHT 1   // r473: sky/sun rim light on creatures for silhouette form (0 = flat)
#endif

void main() {
    vec3 color = v_color;

#if ENTITY_RIM_LIGHT
    // r473: the entity mesh carries no normal attribute, so reconstruct a flat geometric normal
    // from the world-position screen-space gradient. Add a soft Fresnel-style rim (sky-cool on the
    // shaded side, warming toward the sun on the lit side) so creatures separate from the terrain
    // behind them. PURELY ADDITIVE — the base vertex colour is never darkened.
    vec3  rN   = normalize(cross(dFdx(v_world_pos), dFdy(v_world_pos)));
    vec3  rV   = normalize(ubo.camera_pos.xyz - v_world_pos);
    float rim  = pow(1.0 - max(dot(rN, rV), 0.0), 3.2);
    float rndl = max(dot(rN, -ubo.sun_dir.xyz), 0.0);
    vec3  rcol = mix(ubo.ambient_color.rgb, ubo.sun_color.rgb, rndl * 0.6);
    // r550: edge-only and gentle. The previous 0.85 over a flat-shaded box body
    // washed dark colours (teal/near-black heroes) toward white; concentrate it at
    // grazing silhouette edges (higher power) at a fraction of the strength so the
    // base vertex colour reads true while creatures still get a soft rim.
    color += rcol * min(rim, 0.6) * 0.28;
#endif

    // Eye→fragment distance in world units (same basis as chunk.frag).
    float dist = length(ubo.camera_pos.xyz - v_world_pos);

    // Screen-space froxel column UV — identical to chunk.frag.
    vec2 frag_uv = gl_FragCoord.xy / ubo.viewport_rect.xy;

    // Sentinel probe: central texel alpha==0 ⇒ volume unfilled this frame.
    vec4 froxel_center_probe = texture(froxel_volume, vec3(0.5));
    bool froxel_filled = froxel_center_probe.a > 0.0001;

    if (froxel_filled) {
        // Surface depth → froxel W (log-distributed NEAR..FAR), matching
        // chunk.frag so entity and terrain composite identically.
        float ray_far = min(dist, FROXEL_FAR);
        float w = log(ray_far / FROXEL_NEAR) / log(FROXEL_FAR / FROXEL_NEAR);
        w = clamp(w, 0.0, 1.0);

        vec4 froxel = texture(froxel_volume, vec3(frag_uv, w));
        // Standard volumetric composite: dim surface by remaining
        // transmittance, then add in-scattered light.
        color = color * froxel.a + froxel.rgb;
    }

    // r535: real per-vertex transparency. Fog is composited into rgb above; the
    // alpha here drives the hardware blend (opaque entities pass v_alpha = 1.0).
    out_color = vec4(color, v_alpha);
}
