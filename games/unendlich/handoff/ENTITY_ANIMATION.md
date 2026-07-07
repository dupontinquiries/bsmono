# ENTITY ANIMATION — dragon (and creature) motion + material + effects
Layers on the gait + segment pose (ENTITY_BODY_SEGMENTATION.md). All pure cores; the vertex-pass /
shader application is M2 (render thread). Genome drives the parameters (DRAGON_GENOME.md).

## Motion (SIM cores)
- **breathe** — chest/torso swell oscillation. `breathe(phase,depth,rate,dt)->torso_scale`. r14.
- **vocal_pose** — sound amplitude → jaw-open + neck-raise; the dragon "speaks/roars" with its whole
  neck. `vocal_pose(sound_amp,expressiveness)->{jaw_open,neck_raise}`. r14.
- **claw_curl** — toes/claws curl on grip and through the gait. `claw_curl(grip,gait_phase)->curl[]`. r15.
- **appendage_ripple** — secondary motion: slight ripples through wings & legs from velocity/accel.
  `ripple(vel,stiffness,t)->offset[]`. r15.
- **blink** — eyelid state from a per-creature blink timer + rate (deterministic, occasional). r15.

## Material / shader (RENDER cores)
- **armor_reflect** — armor gene → {reflectivity, specular_power}; armored dragons read harder/glossier
  under the existing lighting + bloom. r14.
- **wing membrane** — wings are flatter/thinner **voxel sheets** webbing between the wing-skeleton
  bones; `wing_opacity` gene → membrane alpha (glassy translucent ↔ opaque leathery). Core
  `wing_opacity(gene)->alpha`; the thin-sheet webbing geometry goes in the viewer/mesh builder. r14.
- emissive eyes/neck/spine (r7) + bloom already carry the element glow.

## Effects (WORLD/fx cores) — "smoke and other effects like in the docs"
- **smoke_field** — smoke puff density + dissipation/advection over time. r14.
- **ember_field** — rising embers (ties to fire/weather — and the runaway `fires` count to fix). r14.
- **dust_motes** — ambient floating-particle density by biome/light. r14.
These feed the emission/particle path (docs/SHADER_ROADMAP "fog IS the lighting system").

## Animation LOD — downsample lerps by motion speed (GPU-load reducer, user)
Slow motions don't need per-frame updates: recompute the pose every N frames and interpolate; fast
motions run full-rate. Cuts per-frame animation + vertex work, most for distant/idle creatures.
- **anim_cadence** — `anim_cadence(motion_speed,dist)->update_every_n` + a `downsampled_lerp` that
  holds & interpolates between sparse keyframes. r14. Pairs with `lod_band`/`silhouette_lod`.
