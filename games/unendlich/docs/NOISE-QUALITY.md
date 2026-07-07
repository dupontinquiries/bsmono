# Noise quality: why terrain/caves look jagged, and what to change

Audit prompted by three reported symptoms: jagged cliff edges, "on/off" froxels,
and underground terrain stripes. Findings are from the actual code, not guesses.

## What we are NOT missing
Multilayer noise *exists*: `noise.rs` has `fbm_2d`/`fbm_3d` (proper FBM —
lacunarity 2, gain 0.5), and the terrain surface uses 3-octave FBM. So "no FBM at
all" is not the problem. **Poisson / blue-noise sampling** is a red herring here:
that controls *point distribution* (tree/feature scatter), not the smoothness of a
density field — it wouldn't fix jagged terrain or banded caves.

## What IS the problem

**1. It's VALUE noise, not gradient (Perlin/Simplex) noise.** `noise_2d`/`noise_3d`
hash the cell-corner *values* and interpolate them. Value noise is inherently
grid-aligned — its features line up with the integer lattice and read as blocky,
especially on steep gradients. Gradient noise (Perlin) interpolates corner
*gradients* (zero at lattice points) and is markedly smoother; Simplex avoids the
grid entirely. The cliffs look jagged largely because the 2D continental/hills
field that shapes them is value noise.

**2. The interpolation curve is cubic, not quintic.** `smoothstep(t) = t²(3−2t)`
(in `noise.rs`) has a *discontinuous second derivative* at every integer lattice
point. That creates faint grid-aligned creasing — exactly the artifact Perlin
switched to the quintic `6t⁵−15t⁴+10t³` to remove. One-line change per
implementation, meaningful smoothing, no shape change to the broad terrain.

**3. The caves use SINGLE-octave value noise — this is the underground stripes.**
`is_cave` builds its density from three raw `noise_3d` lookups (worm 0.05, chamber
0.012, seepage 0.15) — *not* `fbm_3d`. Single-octave value noise is the most
grid-aligned of all; thresholding it (`density > cave_threshold`) traces the
lattice and produces blocky, banded cave walls. **This is the clearest, highest-
value fix**: make worm/chamber/seepage use `fbm_3d` (3–4 octaves) so cave walls
become organic instead of grid-traced. Your "missing multilayer noise" hunch is
correct *specifically for the caves*.

**4. Terrain 3D detail is intentionally weak.** `density()` adds `detail * 0.15`
(3-octave `fbm_3d`). The 0.15 is the earlier worldgen-stripe fix — stronger detail
re-introduced horizontal terracing because the detail's vertical slope rivaled the
density gradient. So the surface is mostly 2D-driven; raising detail needs care
(it's the terracing tradeoff, documented in `STRIPES-STATUS.md`). Domain warping
(perturb sample coords with a second low-freq noise) breaks grid alignment without
raising amplitude — a safer way to add apparent detail.

**5. Froxels "on/off" is a separate system.** `froxel_fill.comp` builds fog from a
humidity × 3D value-noise density and a shadow-map sample, then the fragment shader
trilinearly samples the volume. The binary look is most likely a *hard* shadow term
(in/out of shadow with no softening) and/or low froxel grid resolution — not the
terrain noise. Needs its own pass (PCF-soften the froxel shadow sample; consider
more froxel Z-slices). Listed here so it isn't conflated with the worldgen fixes.

## The hard constraint on any fix
The CPU generator (`world_model.rs`/`noise.rs`) and the GPU worldgen shader
(`shaders/chunk_generation.comp`) must stay bit-parity (the `OctreeGpuMesher`
verify path + `tools/density_parity_lint.py` enforce it). So every noise change is
a *coordinated* edit in three places — Rust, GLSL, and the Python lint (which
re-implements the formula). These are careful, tested changes, not one-liners.

## Ranked fixes (value × safety)
1. **Caves → `fbm_3d`** (worm/chamber/seepage multi-octave). Directly fixes the
   underground stripes. Touches `world_model.rs` + `chunk_generation.comp` + the
   lint. Highest value.
2. **Cubic → quintic smoothstep** in `noise.rs` + the shader + the lint. Cheap,
   global smoothing, smallest blast radius.
3. **Domain warping** on the terrain sample coords — breaks grid alignment, adds
   apparent detail without the terracing risk of raising detail amplitude.
4. **Value → gradient/Simplex noise** — the biggest quality jump and the biggest
   rewrite (new primitive in all three places). Do last, on its own.
5. **Froxel** (separate track): soften the shadow term + raise Z-resolution.

Recommended order: (1) caves first — it's the symptom you named and the clearest
win — validated by re-running `density_parity_lint.py` (CPU↔GPU still match) and a
visual check, then (2) quintic, then revisit the froxel.
