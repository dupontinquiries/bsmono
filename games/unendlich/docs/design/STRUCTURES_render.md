# Structure Rendering — Design

How **large and numerous static structures** (walls, towers, ruins, settlements)
render efficiently. The goal is to draw thousands of structural parts across a
wide view distance without drowning the GPU in draw calls, vertices, or shadow
work. This builds on the pure render-helper cores already landed; it does not
introduce new GPU bindings — wiring is the integration pass's job.

## Instancing repeated parts

Structures are overwhelmingly repetition: the same wall segment, pillar, arch, or
crenellation appears hundreds of times. Each unique part-mesh is uploaded once and
drawn with per-instance transforms (and a per-instance material/tint index), so a
1,000-pillar keep is a handful of instanced draws, not 1,000. Instance buffers are
grouped per `(part-mesh, material)` so a draw never mixes pipelines. The
`entity_cost_model` (R8) style accounting applies directly: cost scales with
instance count × parts-per-LOD, which is what tells us when a structure cluster
needs to drop to an impostor.

## Impostor / LOD for distant structures

Distant structures collapse through the same machinery as creatures. `lod_band`
(R8) picks a band from size-normalized distance — a large keep keeps real geometry
much farther out than a fence post. Past the far band, `silhouette_lod` (R9)
decides when a structure stops drawing articulated parts and becomes a billboard
impostor, and `impostor_card` (R10) chooses which captured angle to show (using
horizontal-flip symmetry to halve the captures). The new `impostor_uv` core maps
that angle index to its atlas cell rectangle, so the impostor pass samples the
right sub-image. Structures are ideal impostor candidates: they're static, so a
capture stays valid until the camera orbits, and one atlas serves every instance
of that structure type.

## Material table via `pipeline_key`

Structure surfaces (stone, timber, plaster, metal fittings) are encoded through
`pipeline_key` (R13), which packs `(lod, pass, material, double_sided)` into a
checked `u64`. This makes the LOD×pass×material table encode-checked rather than
ad-hoc: an out-of-range pass or LOD fails at encode time, and the packed key
doubles as the pipeline-bucket identity for batching. Most structure surfaces are
single-sided opaque, which keeps them in the cheap early-Z passes.

## Draw batching + `draw_key` sorting

Per frame, every visible structure part becomes a `(depth, pipeline_key)` pair fed
to `draw_key` (R13) — depth bucket in the high bits, pipeline in the low bits — so
an ascending sort orders draws front-to-back (for early-Z rejection) while
grouping equal-depth draws by pipeline state (minimizing state changes). With
thousands of parts, key generation itself is hot, so `batch_draw_key` (R15)
provides a bit-parity batched generator that amortizes the per-frame near/far
setup across the whole list.

## Shadow cost for big static geometry

Large static structures dominate the shadow pass: they cast into many cascades and
their geometry is mostly invisible to the main view but still rasterized for
shadows. Three levers: (1) shadow-only LOD — cast shadows from a coarser mesh than
the visible one, since penumbra hides the difference; (2) cascade assignment via
`shadow_cascade` (R11), so a structure only renders into the cascades its depth
range touches rather than all of them; (3) static shadow caching — a structure
that hasn't moved and whose light hasn't moved can reuse last frame's shadow
contribution, re-rendering only on light/structure change.

## Open questions

- **Impostor capture budget.** How many angles per structure, and do we capture
  elevation tiers (looking down from a hill) or only yaw? `impostor_uv` assumes a
  flat row-major atlas; a 2D (yaw × pitch) atlas would need a different mapping.
- **Impostor transition popping.** Cross-fade between geometry and impostor (and
  between adjacent captured angles) vs. a hard swap — where's the cost/quality
  line?
- **Cluster-level impostors.** Should a distant *settlement* become one impostor
  rather than per-building impostors? That needs a clustering pass upstream of
  `lod_band`.
- **Shadow caching invalidation.** What's the cheapest correct test for "this
  static structure's shadow is still valid" under a moving sun (time-of-day)?
- **Instance culling granularity.** Per-instance frustum/occlusion culling vs.
  per-cluster — at what instance count does GPU culling beat CPU?
