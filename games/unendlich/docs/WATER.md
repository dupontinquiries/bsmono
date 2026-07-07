# Water: voxel layer → shader

The plan, bottom-up: the voxel sim computes a small set of smooth per-column
fields; the surface shader consumes only those. Everything below the waterline
is `fluid.rs` (CPU, deterministic-ish, unit-tested); everything at the surface
is the fragment shader (GPU, your `hot-shaders` build). This file is the
contract between them — the CPU side is implemented and tested; the GLSL here
is a **reference** to integrate against your pipeline's uniforms/bindings, not
drop-in compiled code (no `glslc` in the dev sandbox).

## What the voxel layer provides (`sim::fluid`, all verified)

| CPU function | Gives the shader | Use |
|---|---|---|
| `FluidField::surface_height(x,z) -> Option<f32>` | continuous waterline (sub-cell) | build/raymarch the surface; depth = surface − bed |
| `flow_field(f) -> FlowField` (`.get`, `.speed`) | per-column horizontal velocity | scroll the normal map *along the current*; orient foam |
| `surface_normal(f,x,z) -> [f32;3]` | macro surface tilt | base normal before micro-ripples |
| `foam(f, flow, x, z, is_solid) -> f32` | whitewater 0..1 | blend foam at rapids / spill edges / shore |
| `water_color(depth, turbidity) -> [f32;3]` | absorption tint | the colour light becomes travelling through water |
| `material::fresnel_schlick(cosθ, 0.02)` | reflect vs refract mix | water F0 ≈ 0.02 |

The CPU writes these into a small per-column texture/SSBO (surface height + flow
xz + foam, ~4 channels) sized to the loaded water region. The shader samples it.

## The four ingredients of "looks like water"

1. **Fresnel reflect/refract mix.** At grazing angles water is a mirror; head-on
   you see into it. `F = fresnel_schlick(dot(N, V), 0.02)`. Final colour =
   `mix(refracted, reflected, F)`.
2. **Depth-based absorption.** Refracted colour is the bed colour tinted by
   `water_color(depth, turbidity)` — shallow is clear, deep goes teal/dark. Depth
   from `surface_height − bed_height`.
3. **Animated normals scrolled by flow.** Two normal-map samples at different
   scales/speeds, both *advected by the flow velocity* so ripples travel
   downstream, summed and combined with `surface_normal`. Wind (from
   `air::air_velocity_at`) adds the ambient chop where flow is near zero.
4. **Foam.** Where `foam > 0`, lerp toward white and flatten the normal; scroll
   the foam texture along flow so whitewater streaks downstream.

## Reference fragment shader (integrate, don't paste)

```glsl
// Inputs (bind to your UBO/SSBO): camera V, sun, the per-column water texture,
// the opaque scene colour+depth behind the water (for refraction), time.
vec3 shade_water(vec3 world_pos, vec3 V, float depth, vec2 flow, float foam_amt,
                 vec3 macro_normal, vec3 scene_behind, vec3 sky_refl, float t) {
    // 3. Ripple normals advected along the current (two octaves).
    vec2 uv = world_pos.xz;
    vec3 n1 = sample_normal_map(uv * 0.5  - flow * t * 0.6);
    vec3 n2 = sample_normal_map(uv * 1.7  - flow * t * 1.1);
    vec3 N  = normalize(macro_normal + 0.5 * (n1 - vec3(0,0,1)) + 0.3 * (n2 - vec3(0,0,1)));

    // 2. Refraction: scene behind, tinted by absorption over `depth`.
    vec3 absorb = water_color(depth, /*turbidity*/ 0.15);
    vec3 refracted = scene_behind * absorb;

    // 1. Fresnel reflect/refract blend (water F0 = 0.02).
    float F = fresnel_schlick(max(dot(N, V), 0.0), 0.02);
    vec3 col = mix(refracted, sky_refl, F);

    // Sun glint (sharp specular off the rippled normal).
    vec3 H = normalize(V + sun_dir);
    col += sun_color * pow(max(dot(N, H), 0.0), 200.0);

    // 4. Foam: scrolled whitewater, fades the surface to matte white.
    float fm = clamp(foam_amt * sample_foam(uv * 1.2 - flow * t), 0.0, 1.0);
    col = mix(col, vec3(0.92), fm);
    return col;
}
```

`water_color` and `fresnel_schlick` already exist on the CPU; port them verbatim
to GLSL (they're tiny) so the surface tint matches what the sim computes.

## Performance notes (decide empirically, per the plan)

- The per-column water texture is *coarse* (one texel per column, not per voxel)
  — cheap to upload, and the shader interpolates. Flow/foam change slowly, so
  this can update at a fraction of the frame rate.
- Refraction needs the opaque scene already rendered (sample its colour/depth),
  so water draws in a later pass. Start with screen-space refraction (sample the
  framebuffer) before considering a true refraction pass.
- Reflection: start with the sky/cubemap (cheap) and a Fresnel mix; only add
  screen-space or planar reflections if the look demands it and the budget
  allows. The whole point of measuring first.
- `foam`/`flow` are derived each settle; if water is static, skip recompute and
  reuse the last texture.

## Status

- Voxel layer (`surface_height`, `flow_field`, `surface_normal`, `foam`,
  `water_color`, buoyancy): implemented + unit-tested.
- Shader: reference above; needs wiring to the real pipeline + a normal/foam
  texture, then iteration on a display. That's the next on-device step.
