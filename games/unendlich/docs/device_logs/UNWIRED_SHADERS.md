# Unwired / superseded shaders (audit r466)

Shaders are compiled to `assets/shaders/*.spv` by build.rs and loaded into pipelines at runtime.
These compile but are NOT instantiated into any pipeline on default runs:

- **sky.vert + sky.frag** — a dedicated analytic atmospheric SKY pass ("fullscreen triangle on the
  far plane", build.rs r68-07). Compiled to .spv, but no pipeline loads sky.{vert,frag}.spv. The sky
  is currently produced by the fog/sky_color path inside chunk.frag (analytic, no separate pass), so
  this dedicated pass is built-but-dormant. WIRING IT is a real render feature (new pipeline + render
  pass slot before chunk draw) — not a flag flip; deferred (adds pipeline + crash surface, relevant
  to the Fairphone bring-up). Candidate for a future "better sky" pass.
- **clouds.frag** — superseded by cloud-as-fog in chunk.frag; NOT in build.rs, no .spv, not loaded.
  Effectively dead. Keep as reference or delete.
- **neural_feat_lean.comp + neural_feat_octree.comp** — alternate neural feature-extractor variants;
  compiled to .spv but neural_feat_pipeline.rs only loads the `gbuf` variant. The lean/octree variants
  are dormant alternates (selectable if the pipeline learns to pick a variant). Not wired by default.

All other shaders in src/shaders are wired (chunk, entity, present, sky-as-fog, the mesh/cull/gen
compute family, bloom, neural_shade/gbuf, froxel, cascade, ui, egui, octree_probe).

Note: octree_sample.glsl / octree_types.glsl / tonemap.glsl / water_waves.glsl are #include files
(pulled into other shaders), correctly not loaded directly.
