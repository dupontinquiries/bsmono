# Shader / Renderer Roadmap

This document captures the user-provided SOTA non-RT forest renderer
plan, in priority order. It's a roadmap to draw against, not a
spec — increments will pick from here as the simulation/perf base
allows. Originally received during the v0.11.1 turn.

DESIGN_NOTES has a shorter summary of this in the "Increment 4.1"
section; this file is the long-form reference with the specific
techniques, parameters, and pipeline pass breakdown.

---

## The core insight

> **In a forest game, fog IS the lighting system, not an add-on.**

A forest with mediocre geometry and amazing atmosphere looks AAA. A
forest with perfect geometry and weak atmosphere looks old. Fog
softens aliasing, hides shadow cascade transitions, creates depth,
creates god rays, creates scale, creates weather, creates mood, and
unifies distant foliage. Every later phase below ladders into making
fog (and the lighting that drives it) better.

The wrong shape:

```
Surface shading + cheap fog overlay
```

The right shape:

```
Sunlight
   ↓
Shadow Cascades
   ↓
Volumetric Light Injection
   ↓
Temporal Fog Accumulation
   ↓
Final Surface Shading
```

---

## Priority order (highest visual gain first)

1. **Shadowed volumetric fog** — biggest cinematic gain.
2. **Temporal stability** — makes everything feel expensive.
3. **Cascaded shadows** — grounds the world.
4. **Height-layered fog** — creates scale.
5. **Animated wind-driven density** — creates life.
6. **Contact shadows** — fixes floating foliage look.
7. **Better sky atmosphere** — improves whole-frame coherence.

Items 1, 2, 3 build on each other (shadows feed into volumetric
fog's light injection; temporal reprojection makes the cost
affordable). Items 4, 5 are already partly in place via the v0.11.1
fragment shader's height-fog + wind-advection layered fog; they
become real once we have a froxel grid (item 1) to put them in.

---

## What NOT to spend performance on

These are explicitly *not* the path. They scale poorly in giant
forests:

- Full raytraced GI
- Path traced fog
- Brute force volumetric raymarching (every-pixel-every-frame)
- Voxel cone tracing everywhere
- Super-high-resolution shadow maps

---

## The full stack (priority-ranked)

| System                          | Importance                |
| ------------------------------- | ------------------------- |
| Cascaded shadow maps            | Mandatory                 |
| Volumetric froxel fog           | Mandatory                 |
| Temporal reprojection           | Mandatory                 |
| Clustered forward/deferred      | Very important            |
| Contact shadows                 | Very important            |
| Screen-space GI                 | Optional                  |
| Probe GI                        | Optional                  |
| SSR                             | Nice                      |
| RTGI                            | Optional luxury           |

---

## Phase 1 — foundation upgrades

### Cascaded shadow maps (the #1 thing)

Without cascaded shadows:

- Forests never ground correctly.
- Trunks float visually.
- Fog has no shape.
- Atmosphere feels fake.

Forests contain extremely high shadow frequency, vertical geometry,
lots of partial occlusion, and huge depth ranges. A single shadow
map fails horribly in this case.

Use **4 cascades** with **logarithmic split distribution**. Typical
splits:

- Cascade 0: 0–20m
- Cascade 1: 20–60m
- Cascade 2: 60–180m
- Cascade 3: 180–600m

This is the largest single visual win available and is the foundation
for both grounded-shadow forests and Phase 2's shadowed volumetrics
(the fog needs the shadow maps to inject light correctly).

### Temporal reprojection (mandatory)

This is how modern volumetrics become affordable. Without it,
volumetrics are noisy, low-res, unstable. With it, they're smooth,
cinematic, stable, dense.

The trick: render volumetrics at low resolution and accumulate across
frames. Massive performance win — typically 4–8× cost reduction at
visually equivalent quality.

### Improved fog stepping

**Already done in v0.11.1**: logarithmic depth spacing + per-pixel
hash jitter in the volumetric fog march. This was the first item
that didn't need new infrastructure.

The next step (deferred) is adding temporal accumulation to the fog
march itself — currently re-marches from scratch every frame.

---

## Phase 2 — volumetric atmosphere

### Volumetric froxel fog (the big one)

Replaces

```glsl
fog = exp(-distance)
```

with a camera-aligned 3D grid (typical resolution **160 × 90 × 64
froxels**), storing per-froxel:

- density
- scattering
- extinction
- sunlight energy (injected from shadow maps)
- emissive media

Then integrate through the grid along view rays.

Why froxels are the SOTA sweet spot:

- vs raytracing: vastly cheaper, temporally stable, scalable, works
  on huge worlds, works with weather.
- vs simple fog: actually volumetric — gives shafts, light cones,
  layered mist, shadowed fog, localized plumes.

This is the single technique that gets us from "looks like a voxel
game" to "looks like an AAA forest at dusk".

### Clustered forward+ shading

Better than deferred for a voxel forest with alpha foliage. Voxel
forests contain alpha foliage, volumetrics, particles, transparent
water, and many small lights — all of which make deferred painful.

### Contact shadows

Very important in forests. Either:

- screen-space contact shadows, or
- short raymarch from depth

Cheap. Grounds grass, roots, rocks, trunks. Fixes the
floating-foliage look that even good cascaded shadows can leave.

### Volumetric lighting injection

Inject sunlight INTO the fog volume. The fog receives light, the fog
casts shape, shafts emerge. Without this, fog looks flat.

Forest god rays come from: shadow maps + froxel scattering + temporal
reprojection. **Not raytracing.**

---

## Phase 3 — temporal coherence

### Temporal volumetric accumulation

Volumetrics become smooth + cinematic instead of noisy. Multi-frame
accumulation through reprojected froxel state.

### Atmosphere scattering / aerial perspective

Replaces simple sky gradients. Even approximate models (Bruneton,
Hillaire) massively improve forests. Required for:

- correct sun-phase color
- atmosphere extinction at distance
- horizon haze that's actually atmosphere, not desaturate-toward-grey
  (the v0.11.1 placeholder)

### Cloud layers

Signed-distance bounded volumetrics for mega fog banks. Distinct
from the per-block cloud cubes — these are continuous volume
shapes, the "approaching storm front" kind of cloud.

---

## Phase 4 — refinement

- **SSR**: water reflections, wet surfaces.
- **Probe GI / SSGI**: bouncing light into shadowed canopy interior.
- **Wetness system + rain interaction**: surfaces darken/glisten,
  particles for rain, fog density bumps with precipitation.

---

## Phase 5 — optional luxury

Hardware RT augmentation, only if it pulls weight against the
scalability we'd be giving up. Treat as augmentation to the
established Phase 1–3 stack, never as foundation.

---

## Recommended pipeline (once Phase 2 lands)

```
Pass 1 — Depth prepass
Pass 2 — Shadow cascades
Pass 3 — Froxel fog build
            inject: sunlight, density, shadowing
Pass 4 — Main clustered forward shading
            consume: shadows, fog lookup, atmosphere
Pass 5 — Temporal accumulation
Pass 6 — Post FX (bloom, exposure, TAA, color grading)
```

This is what modern AAA forest renderers actually do.

---

## Why height + humidity + wind fog matter

The fog density formula should not be uniform. Instead:

```
density = humidity
        * valley_factor
        * temperature_factor
        * noise_field
        * wind_transport
        * biome_factor
```

This gives:

- **Valley fog**: settles in low terrain.
- **Morning mist**: low-angle sunlight catches shallow fog layers.
- **Forest pockets**: dense fog under tree canopy.
- **Dynamic weather fronts**: fog banks move through terrain.
- **Wet biome atmosphere**: swamps feel heavy.

The v0.11.1 fragment shader already approximates several of these in
the per-fragment march (height fog, wind advection, temperature
turbulence). When the froxel grid lands, they move into the volume
representation and become coherent across the whole frame instead of
being reconstructed per-fragment.

---

## Why this engine is well-positioned

Most engines fake humidity, temperature, wind, and climate. This
engine *simulates* them — the fields are real, queryable per-block,
already on the UBO. That means once the froxel grid lands, fog
density can be **simulation-driven, biome-aware, terrain-aware,
weather-aware** rather than aesthetically faked.

Other things already in place that the SOTA stack will consume:

- world-space shaders (clean separation)
- physics-aware cameras (clean per-RenderView UBO design,
  partly built in v0.11.1 with `viewport_rect`)
- deterministic views (means temporal reprojection has a clean truth
  to reproject from)
- atmosphere UBOs (ready to feed the froxel pass once it exists)

---

## High-ROI specific techniques to investigate

- **ReSTIR-like light reuse** (probably later, but the lit-points
  in a forest are perfectly suited to this).
- **Checkerboard volumetrics** — huge performance win, easy retrofit.
- **Blue-noise volumetric jitter** — mandatory, supersedes the
  hash-based jitter in the current v0.11.1 fragment shader.
- **Temporal blue-noise rotation** — keeps temporal accumulation
  free of pattern artifacts.
- **Signed-distance cloud layers** — for mega fog banks.
- **Hierarchical Z for volumetric skipping** — excellent
  optimization, lets us skip empty froxels. *(v1 shipped: a per-cell
  optical-depth early-out in `froxel_fill.comp` skips the 9-tap shadow
  PCF on negligible cells — ~5–19% of the ~8.3M shadow taps/frame,
  sub-LSB error, validated by `handoff/froxel_skip_model.c`. A full HZB
  would extend this to whole-tile skipping.)*
- **Deep shadow maps for foliage** — very strong specifically for
  forests with alpha foliage.
- **Epipolar volumetric sampling** — advanced but excellent for
  shaft-heavy scenes.

---

## What the biggest "AAA forest" actually looks like

It is NOT polygon count. It is:

- depth layering
- light shafts
- atmospheric extinction
- moving fog
- shadow softness
- temporal coherence

That is what creates scale, realism, cinematic feel. Even
medium-detail geometry looks incredible once those systems exist.

---

## Status as of v0.11.1

**Done**:
- Logarithmic-depth fog stepping (Phase 1, "improved fog stepping")
- Per-pixel hash jitter for fog march (Phase 1, jitter component)
- Hash-based screen-space dither / film grain
- viewport_rect on the camera UBO (RenderView seam built)
- Layered per-fragment fog (height, distance, wind, temperature)

**Recommended next big increment**: cascaded shadow maps. Phase 1
foundation, largest single visual win, prerequisite for the
volumetric light injection in Phase 2.

**Filed (not yet started)**: everything else above.

---

## Reference ↔ engine: current-state analysis (v0.24.x)

The "Status as of v0.11.1" above is historical. Read from the live shaders
(`froxel_fill.comp`, `chunk.frag`) and the perf/handoff docs, here is where the
reference stack actually stands today — and where the next gain is.

### Technique status (reference → engine)

| Reference technique (priority) | Status | Where it lives now |
|---|---|---|
| Improved fog stepping (log-Z) | **Done** | `view_z_from_slice` log distribution in `froxel_fill.comp`; fallback march in `chunk.frag` |
| Volumetric froxel fog (Phase 2, "the big one") | **Done** | `froxel_fill.comp` fills a 160×90×64 volume every frame, cumulative front-to-back; `chunk.frag` samples once at surface depth |
| Volumetric light injection (god rays) | **Done (single-scatter)** | per-froxel shadowed sun in-scatter — `cell_inscatter`: 9-tap PCF × HG phase (g=0.6) + ambient sky |
| Height-layered fog | **Done** | `height_fog = exp(-alt·0.05)`, `altitude_rolloff`, `altitude_moisture_bias` |
| Animated wind-driven density | **Done (enhanced)** | wind-advected fbm, now blobs→streaks (shear-stretch along wind); identical in froxel fill + fallback |
| Empty-froxel skipping (HZB family) | **v1 done** | per-cell optical-depth early-out (`OD_EPS=7e-5`) skips the shadow PCF on negligible cells |
| Cascaded shadow maps | **Partial** | a shadow map is sampled (`light_view_proj`, `shadow_map`); *multiview* CSM is blocked on moving frusta out of push constants (see DESIGN_NOTES) |
| Temporal reprojection / volumetric accumulation (priority #2) | **Not started** | no froxel history — every froxel is recomputed from scratch each frame |
| Blue-noise volumetric jitter | **Not started** | froxel fill samples cell centers; the fallback march still uses hash jitter |
| Checkerboard volumetrics | **Not started** | the fill dispatches all 160×90 columns every frame |
| Aerial perspective / sky multiple-scatter LUT | **Implicit only** | froxel extinction already reads as aerial perspective; no dedicated sky LUT |
| Clustered forward+ · contact shadows · SDF clouds | **Not started** | — |

Headline: the **spatial** half of the reference stack (Phase 2 volumetrics) is
substantially built. The **temporal** half — priority #2, "makes everything
feel expensive" — is the largest unrealized gain, and it is also the key that
unlocks a big *perf* cut to the froxel pass.

### Next-increment analysis — temporal froxel accumulation (+ blue-noise jitter, + cheaper PCF)

These three reference items compose into one step, and together they raise
quality while *reducing* cost — which is exactly why the reference ranks
temporal stability second.

Today `froxel_fill.comp` pays a 9-tap (3×3) shadow PCF per surviving cell to
soften the hard on/off shadow boundary at coarse froxel-Z. That spatial PCF is
a stand-in for temporal denoising. The reference path replaces it:

1. **History volume.** Ping-pong a second `rgba16f` 160×90×64 image against the
   current `froxel_out` (~7 MB at fp16). One image + view; no new pass.
2. **Reproject.** Transform each froxel's world-space center into the *previous*
   frame's froxel UVW via a `prev_view_proj`. We already carry `view_proj`,
   `inv_view_proj`, and `light_view_proj`, so this is one more mat4 on the
   UBO/push — then a trilinear sample of the history volume.
3. **Blend + reject.** Exponential moving average `mix(history, current, α)`,
   α ≈ 0.05–0.1, with a neighborhood clamp (clamp the history sample to the
   min/max of the current cell's neighbours) and a frustum reject (reprojected
   UVW out of [0,1] → fall back to current). This kills smearing on moving fog
   and camera cuts.
4. **Collapse the PCF to 1 jittered tap.** With temporal accumulation doing the
   denoising, `cell_inscatter`'s 3×3 PCF becomes a single shadow tap whose
   offset is driven by a **blue-noise / Halton** sequence rotated per frame
   (the reference's "blue-noise volumetric jitter" + "temporal blue-noise
   rotation"). One tap that converges over ~8–16 frames reads *softer* than 9
   static taps.

Net: ~9× off the dominant per-cell shadow cost, composing multiplicatively with
the ~5–19% cell-skip already shipped, plus temporally stable shafts. Cost: one
fp16 volume, one `prev_view_proj`, a trilinear history fetch, a small clamp. The
engine is well-positioned — the deterministic per-RenderView UBOs noted above
give "a clean truth to reproject from."

Risks/validation: ghosting on fast motion (mitigated by the clamp + reject) and
a few-frame warmup. Testable the same way the empty-skip was: a `float`-accurate
CPU model of the reproject + EMA can bound steady-state error and warmup length
before any GPU work, and `vk-harness` gates the shader once it lands.

### Cheap complementary win — checkerboard volumetrics

Orthogonal to temporal: fill only half the columns each frame (X/Y
checkerboard), reuse the history volume for the other half, swap the pattern
next frame. Roughly halves the fill dispatch for a 1-frame latency on
alternating columns — and it *requires* the history volume above, so it falls
out for free once temporal accumulation lands. With the empty-skip on top, the
froxel pass would do well under half its current shadow work.
