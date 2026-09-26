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

    // r-fp-nearclip: dithered "screen door" cutout for very-close entity
    // geometry. Specifically for the player's own first-person avatar
    // (`fp_body`) — its head/torso otherwise fills most of the screen
    // while bobbing right in front of the camera, since nothing else in
    // the game gets this close to the eye in first person. Mirrors
    // chunk.frag's water dithered-transparency technique (ordered 4x4
    // Bayer pattern, r448) rather than a smooth alpha fade: discard is
    // order-independent for a solid mesh with overlapping body parts,
    // where blending could show seams or wrong-order artifacts depending
    // on draw order within the avatar itself.
    //
    // Fades in smoothly from NEAR_CUTOUT_START (still solid) to fully
    // discarded at NEAR_CUTOUT_FULL, so the avatar thins out and vanishes
    // as it approaches the eye instead of hard-popping at one distance.
    const float NEAR_CUTOUT_START = 1.35; // world units; solid outside this
    const float NEAR_CUTOUT_FULL  = 0.30; // world units; fully gone inside this
    // A vertex alpha above 1 marks geometry that is MEANT to sit at the eye:
    // the first-person viewmodel (the item in hand, 0.3-0.8 m away). It is
    // opaque and exempt from the cutout, which otherwise discarded it whole.
    bool at_eye = v_alpha > 1.5;
    if (dist < NEAR_CUTOUT_START && !at_eye) {
        float cutout = clamp(
            (NEAR_CUTOUT_START - dist) / (NEAR_CUTOUT_START - NEAR_CUTOUT_FULL),
            0.0, 1.0);
        int ndx = int(gl_FragCoord.x) & 3;
        int ndy = int(gl_FragCoord.y) & 3;
        float nbayer[16] = float[16](
             0.0/16.0,  8.0/16.0,  2.0/16.0, 10.0/16.0,
            12.0/16.0,  4.0/16.0, 14.0/16.0,  6.0/16.0,
             3.0/16.0, 11.0/16.0,  1.0/16.0,  9.0/16.0,
            15.0/16.0,  7.0/16.0, 13.0/16.0,  5.0/16.0);
        if (cutout > nbayer[ndy * 4 + ndx]) {
            discard;
        }
    }

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
    // Near-clear fragments (soft rims of smoke wisps) must not write depth:
    // this pipeline writes depth for everything it draws, so an invisible rim
    // would still hide whatever is drawn behind it and turn soft wisps into
    // hard-edged panes. (A separate no-depth-write effects pass is the full fix.)
    if (v_alpha < 0.03) {
        discard;
    }
    out_color = vec4(color, at_eye ? 1.0 : v_alpha);
}
