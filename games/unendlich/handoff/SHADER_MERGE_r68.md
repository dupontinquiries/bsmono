# r68 AAA shaders — merge record (r70 base)

All shader work is validated IN-SANDBOX (glslangValidator compile + spirv-val + descriptor_gate
+ build.rs .spv regeneration + HARNESS GREEN). VISUAL + on-device frame-time validation is owed
on the M2 — a compile-clean shader can still need tuning.

## MERGED (5 of 7)
- **r68-01 PBR terrain** (chunk.frag): Cook-Torrance GGX direct + hemispheric ambient + normal
  mapping (PBR_NORMAL_MAP) + A/B path (PBR_QUALITY). Uses existing ambient_term/sun_attenuation.
- **r68-03 volumetrics/god rays** (froxel_fill.comp): dual-lobe HG phase, per-froxel shadowed
  shafts (SHAFT_INTENSITY), valley/ground fog, temporal hook (default off). Descriptor-stable.
- **r68-04 water** (chunk.frag is_water branch + #include water_waves.glsl): Gerstner-ish surface
  normals, Schlick fresnel, planar-sky reflection + Beer-Lambert refraction, foam, caustics.
  Descriptor-stable (uses existing ubo.wind_params). True SSR deferred (needs scene color/depth).
- **r68-05 tonemap+bloom** (tonemap.glsl, present.frag, bloom_prefilter/blur.frag): ACES+AgX,
  Karis-averaged bloom, lens dirt/vignette/dither. Also ADDED these 4 + present.vert to build.rs
  SHADERS[] (they were hash-unmanaged → stale-.spv footgun); this FIXED the long-standing
  `shader_artifacts::build_list_tracks_every_source_shader` failure (4 known fails → 3).
- **r68-07 sky/atmosphere** (NEW sky.vert/sky.frag + chunk.frag aerial perspective): analytic
  Rayleigh+Mie sky, sun disk/glow, dusk warming; far terrain hazes toward sky tint (non-froxel
  branch only, no double-count). sky binds CameraUbo set0/0 (blessed in descriptor baseline).

chunk.frag is a 3-way merge of PBR+water+aerial (disjoint regions: lighting / is_water / distance
fog) — combined via git merge-file, zero conflicts, recompiles + spirv-val clean.

## OWED — operator/next round
- **Sky pipeline wiring (compile-on-end):** the sky shaders compile + validate but NOTHING calls
  them yet. Add the render-side sky pass: fullscreen 3-vertex draw on the far plane (depth LEQ, no
  write), CameraUbo set0/0, before terrain. Then the Rust DescriptorSetLayout for sky exists (the
  baseline already records its layout).
- **r68-02 soft shadows: NOT RETURNED** — re-dispatch (cascade_fit/chunk_shadow/chunk.frag PCF/PCSS).
- **r68-06 AO: deferred (findings in docs/shaders/AO_FINDINGS_r68.md).** Correct negative: a good AO
  term can't be a chunk.frag-only edit here (flat per-face normals → halos; no depth/normal G-buffer;
  no mesher AO attribute). Needs infrastructure first: either a per-vertex AO attribute from the
  mesher, or a depth+normal target for screen-space GTAO/SSAO. Dispatch that infra, then AO.
- **Visual + perf pass on M2** for all 5 merged shaders (uniform defaults + A/B toggles documented
  in each shader). Watch the froxel↔aerial interaction and PBR exposure vs the old look.
