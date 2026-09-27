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
    // Declared only to reach the light fields after them (chunk.frag's block).
    vec4 flow_coverage;
    vec4 flow_grid[128];
    vec4 fx_params;
    vec4 water_depth_cov;
    vec4 water_depth_grid[64];
    vec4 disturbers[8];
    vec4 disturber_meta;
    vec4 light_volume;     // world::light_volume: xyz = window corner, w = enable
    vec4 point_pos_radius[8];  // render::dynamic_lights: xyz = pos, w = radius
    vec4 point_color_int[8];   // rgb = colour, a = intensity
    vec4 point_meta;           // x = live count
} ubo;

// world::light_volume (binding 4, the image chunk.frag samples): creatures and
// held items in a dark cave are as dark as the cave, and lit by torches.
layout(set = 0, binding = 4) uniform sampler3D light_volume_tex;
const vec3 LV_SIZE = vec3(160.0, 96.0, 160.0);

// render::dynamic_lights, wired: up to POINT_LIGHTS point lights (a held torch,
// a burning weapon) from the camera UBO. The attenuation lines are the ones
// `dynamic_lights_frag_glsl` emits and `apply_lights` mirrors on the CPU (a
// test keeps them identical); a Lambert term against `n` is added on top.
// No occlusion: a point light shines through a thin wall.
const int POINT_LIGHTS = 8;
vec3 point_lights_at(vec3 P, vec3 n) {
    int count = int(ubo.point_meta.x + 0.5);
    vec3 acc = vec3(0.0);
    for (int i = 0; i < POINT_LIGHTS; i++) {
        if (i >= count) { break; }
        vec3 d = ubo.point_pos_radius[i].xyz - P;
        float r = max(ubo.point_pos_radius[i].w, 0.000100);
        float d2 = dot(d, d);
        float r2 = r * r;
        float window = clamp(1.0 - d2 / r2, 0.0, 1.0);
        window = window * window;
        float a = (1.0 / (1.0 + d2)) * window * max(ubo.point_color_int[i].a, 0.0);
        // n = 0 means "no surface" (the item in hand): no Lambert term.
        float lambert = (d2 > 1e-6 && dot(n, n) > 0.25) ? max(dot(n, d * inversesqrt(d2)), 0.0) : 1.0;
        acc += max(ubo.point_color_int[i].rgb, vec3(0.0)) * a * lambert;
    }
    return clamp(acc, 0.0, 1.0);
}


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

    // Scene light (world::light_volume + dynamic point lights), as on the
    // terrain: a creature deep in a cave is dark unless a torch is near, and the
    // item in hand takes the light at the eye. Built from the cross product
    // without normalize() so nothing here can be NaN; oriented to the camera.
    bool at_eye_l = v_alpha > 1.5;
    vec3 ecr = cross(dFdx(v_world_pos), dFdy(v_world_pos));
    float ecr2 = dot(ecr, ecr);
    vec3 en = ecr2 > 1e-20 ? ecr * inversesqrt(ecr2) : vec3(0.0, 1.0, 0.0);
    if (dot(en, ubo.camera_pos.xyz - v_world_pos) < 0.0) en = -en;
    if (ubo.light_volume.w > 0.5) {
        vec3 lp = at_eye_l ? ubo.camera_pos.xyz : v_world_pos + en * 0.4;
        vec3 rel = lp - ubo.light_volume.xyz;
        float e_sky = 1.0;
        vec3  e_blk = vec3(0.0);
        if (all(greaterThanEqual(rel, vec3(0.5))) && all(lessThan(rel, LV_SIZE - 0.5))) {
            vec4 lv = texture(light_volume_tex, lp / LV_SIZE);
            e_sky = lv.r;
            e_blk = lv.gba;
        }
        float e_bi = max(max(e_blk.r, e_blk.g), e_blk.b);
        color = color * mix(0.03, 1.0, e_sky * e_sky) + v_color * e_blk * e_bi * 1.2;
    }
    color += v_color * point_lights_at(v_world_pos, at_eye_l ? vec3(0.0) : en) * 1.2;

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
