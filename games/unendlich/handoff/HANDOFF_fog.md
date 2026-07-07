# HANDOFF — fog wiring + the "absent fog box" diagnosis (r335-fog)

PROPOSAL ONLY — no bin/glfw/shader edits (SHARED_RULES §2). All new math is pure and
gated. `shaders/chunk.frag` and `shaders/froxel_fill.comp` are coordinator-owned and were
read-only here.

## Part A — wiring the new fog math (propose, don't wire)

New pure fns (all additive, `// r334-fog` headers):
- `height_fog::FogParams` (+ `Default`, `sane(..)`) — the one tuning table.
- `height_fog::height_factor(world_y, params)` — height fog with the ambient floor.
- `fog_blend::distance_fog(dist, density, floor)` — exp2 distance fog with the floor.
- `fog_blend::combine_height_distance(height01, distance01, floor)` — final amount.
- `fog_blend::froxel_blend_factor(view_dist, near, far, band)` and
  `fog_blend::froxel_seam_blend(march, froxel, view_dist, near, far, band)` — the seam.
- `scatter_inscatter::inscatter_tinted(transmittance, sun_light, tint_rgb, sun_elev_sin, gain)`.
- `atmosphere_scatter::sun_inscatter_color(cos_theta, turbidity, sun_elev_sin)` — the tint.
- `godray_math::godray_sun_weight(view_dot_sun, sun_elev_sin, sharpness)`.

Suggested per-fragment composition (in the march fallback / the uniform feed):
```
let h = height_factor(world_y, P);
let d = distance_fog(view_dist, P.distance_density, P.fog_floor);
let march_fog = combine_height_distance(h, d, P.fog_floor);
// when sampling the froxel volume, reconcile the two at the boundary:
let fog = froxel_seam_blend(march_fog, froxel_fog, view_dist,
                            FROXEL_NEAR_M, FROXEL_FAR_M, /*band*/ 4.0);
let tint = sun_inscatter_color(dot(view,sun), turbidity, sun.y);
let inscatter = inscatter_tinted(transmittance, sun_light, tint, sun.y, P.sun_inscatter_gain);
let godray = godray(sun_uv, frag_uv, gd, gdec, samples) * godray_sun_weight(dot(view,sun), sun.y, 4.0);
```
`FogParams` is plain data → mirror it into a `std140` UBO and feed the existing fog/uniform
path. No new pipeline.

## Part B — the "absent fog box under the character, offset to one side, fixed position"

**Reading of the symptom.** The froxel volume (`render/froxel.rs`) is camera-frustum-aligned
— screen-tile XY (160×90), logarithmic-depth Z (64), range **[0.5 m, 200 m]**. So anything
"fixed in position, offset to one side, under the avatar" is fixed in **screen space**, which
is exactly the froxel grid's frame (a 3rd-person avatar is itself a fixed screen feature). A
rectangular "box" where fog is *absent* = a froxel XY-tile region (at some depth) that the
fragment reads as the **unfilled sentinel** (`transmittance == 0` → fall back to the march),
where the march then produces ~0 fog → a hard clear rectangle against the foggy surround.

**Ranked root-cause candidates (the fill + sample transform are in the off-limits shaders, so
these are checks for the coordinator, with line anchors I could see):**

1. **A froxel region genuinely left unfilled by the fill pass.** The volume is cleared to the
   sentinel `vec4(0,0,0,0)` (`froxel.rs:191`, `clear_to_sentinel`); the fill must overwrite
   every froxel with `a>0` or the sampler's `CLAMP_TO_EDGE` + sentinel boundary shows through.
   Check in `froxel_fill.comp`: (a) the front-to-back integration's **first Z slice init** —
   an off-by-one that leaves slice 0 (the nearest, ~0.5 m, right under the avatar) at sentinel;
   (b) the dispatch covers X∈[0,160), Y∈[0,96) for a 90-tall volume (`world_renderer.rs:5174`)
   — over-covers Y (fine) but confirm no early-out leaves a tile band unwritten.

2. **Near-plane seam at 0.5 m.** Closer than `FROXEL_NEAR_M` the fragment uses the march; if
   march-fog ≠ froxel-fog at 0.5 m there's a hard fog step on the 0.5 m shell — boxy where the
   avatar occludes part of it. **This is the one Part A fixes:** `froxel_seam_blend` crossfades
   across the boundary (continuity proven in `fog_blend::r334_fog_tests::seam_blend_is_continuous_within_tolerance`),
   and `FogParams::fog_floor` keeps the near/fallback region from reading as a hard clear box.

3. **`inv_view_proj` mismatch (`world_renderer.rs:5144`).** The fill reconstructs world pos from
   `vp.try_inverse()`. If `vp` here differs from the matrix `chunk.frag` uses to compute the
   froxel UVW (e.g. TAA jitter applied to one and not the other, or an aspect/offset mismatch:
   `FROXEL_X:FROXEL_Y = 160:90 = 16:9` assumes a 16:9 target), the froxel grid is registered at
   a constant screen offset → a fixed offset region of wrong/absent fog. Confirm both passes use
   the **same** (un-jittered) view-proj and that the screen→tile mapping uses the real
   framebuffer aspect, not a hard-coded 16:9.

**Recommended order:** apply Part B-2 (my pure mitigation — `froxel_seam_blend` + `fog_floor`)
first; it removes the *symptom class* cheaply and is already gated. Then instrument candidate 1
(dump the froxel volume's alpha channel — any tile reading `a==0` after fill is the box) and 3
(log both passes' view-proj). The box's tile coordinates, mapped back through the froxel UVW,
will point straight at the offending slice/tile.

**On-device repro the coordinator can run:** sweep a test fragment's `view_dist` across 0.5 m
with the froxel sample forced to a value that differs from the march; without `froxel_seam_blend`
there's a step at 0.5 m, with it the transition is continuous (the CPU test already demonstrates
this numerically).

## Base / parity notes
- Built on the **r330b** tree (the r335 base wasn't in this bundle). The fog modules live at the
  **crate root** here (`crate::height_fog`, …); on r335 they're under `sim::` — reconcile paths.
- `mathx` (r332) is absent on this base, so `fog_blend` carries a local `smoothstep01` and the
  in-scatter/godray gates inline the dawn ramp. **Swap these for `mathx::{smoothstep,clamp,lerp}`
  on merge** so there's one definition.
- **Froxel parity constant:** the seam `band` (4 m) and the exact march-vs-froxel agreement
  depend on `froxel_fill.comp`'s density curve, which I can't see. Confirm the march
  `distance_density`/`height` constants reproduce the froxel's look at the seam on-device; the
  blend guarantees *continuity*, not that the two curves are *identical* in slope.
