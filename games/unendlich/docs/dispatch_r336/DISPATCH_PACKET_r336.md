# DISPATCH PACKET — r336 (8 threads)

Eight threads from the operator's idea list. All read `docs/dispatch_r336/SHARED_RULES.md` FIRST
(pure cores, no bin/glfw, in-sandbox-validatable, ≥6 tests, no unwrap/expect/panic/IO/RNG/clock,
return changed `.rs` + MANIFEST + any PNG/shader proof). Base = `unendlich-r336-merged-r335-CHECKED.zip`.
Each carries a FILE ALLOW-LIST; all eight edit disjoint files. `util::mathx`/`vecmath`/`fastmath`
(r332) exist — reuse clamp/lerp/smoothstep, don't re-define.

**Merge order:** `simvectors → dragon-size → plants → textures → weather → biomes → poi-structure → neural`.
**Two coordination notes:** (1) biomes adds `Biome` variants → coordinator sweeps exhaustive matches +
reconciles `rare_structure` biome masks (poi-structure references `Biome`); (2) dragon-size edits
`dragon_forge`/`dragon_archetype` — must not regress the r332 anim-balloon clamp.

---

## T-simvectors — batched (autovectorizable) vector math
**WID:** `r336-simvectors`
**Why:** `util::vecmath` is scalar per-vector (`len3`/`norm3`/`lerp3`); hot loops (mesher, particles,
flow fields) process arrays of vectors one at a time. Add a **batched** layer the compiler can
autovectorize (structure-of-arrays / slice-wise ops), complementing `util::batch_kernel`'s `map_into`.
**Build:** new `util::simvec` (`crates/unendlich-engine/src/util/simvec.rs`):
- slice ops over `&[[f32;3]]` / SoA `(&[f32],&[f32],&[f32])`: `add_into`, `scale_into`, `dot_into`,
  `len_into`, `normalize_into`, `lerp_into` — write results into a caller `&mut [..]` (no alloc in
  the hot path), straight-line `for` loops (no early-exit) so LLVM vectorizes.
- a `Vec3Soa` helper (`from_aos`/`to_aos`) for callers that want SoA.
**Anchors:** `util/vecmath.rs` (scalar reference for parity), `util/fastmath.rs` (`norm3_fast`),
`util/batch_kernel.rs` (`map_into` style + the r332 batch refactor), `util/mathx.rs`.
**Verify:** ≥6 tests (each batched op matches the scalar `vecmath` op elementwise within 1e-6;
empty slice → no-op; AoS↔SoA round-trips; normalize of zero → zero (no NaN); deterministic).
Self-contained → `rustc --test`.
**Watch:** numerical parity with `vecmath` is the contract; don't change results, just shape the loops.

---

## T-dragon-size — restore the small→huge size range
**WID:** `r336-dragon-size`
**Why (operator):** dragons should span **small → huge** again; the size range feels compressed.
`dragon_archetype::size_class_of(g) = round(g01(g.bulk) * SIZE_CLASS_MAX)` buckets the genome, but the
`size_class → body-scale` mapping that `dragon_forge` applies needs to cover a wide range, and must
**not** fight the r332 anim-balloon clamp (`clamp_extent`, rest·1.15).
**Build (in `sim::dragon_archetype` + the size-scale path in `sim::dragon_forge`, additive under
`// r336-dragon-size`):**
- a `size_scale(size_class) -> f32` ramp spanning a documented small→huge range (e.g. ~0.45×…~3.0×),
  applied to the forge's **rest** dimensions (so the whole body scales, before the anim clamp which
  bounds per-axis *animation* growth, not base size).
- verify the anim clamp is relative (rest·1.15) so a huge dragon still animates correctly — the clamp
  scales with rest size, it doesn't cap absolute size.
**Anchors:** `sim/dragon_archetype.rs` (`size_class_of`@215, `size_class`@175, `SIZE_CLASS_MAX`,
`NUM_BUILDS`), `sim/dragon_forge.rs` (`skin`, the rest-dimension build + the r332 `clamp_extent`),
`docs/dispatch_r332/dragon_balloon_fix.png` (the clamp it must respect).
**Verify:** ≥6 tests (size_class 0 → smallest, SIZE_CLASS_MAX → largest, monotone; smallest/largest
ratio ≥ ~5×; a huge dragon's animated extents stay within rest·1.15 of ITS rest, i.e. the clamp
scales; rest pose unchanged at mid size_class; deterministic). **PNG:** a silhouette row from
smallest→largest size_class (CPU raster). `cargo test --features "render-core egui" --lib` or the
r332 dragon `#[path]` harness pattern.
**Watch:** don't touch the arc-follow or the balloon clamp logic — only the base size ramp.

**BONUS (operator) — brain or genetic work:** alongside the size ramp, extend dragon genetics or brain. EITHER add genome traits that drive size/proportion variety (`dragon_archetype::Genome` → wider small→huge spread, new heritable proportions) OR extend `sim::dragon_brain` (`BrainState`) behavior (size-aware flight/aggression). Keep it pure + tested; don't regress `size_class_of` or the r332 clamp. Anchors: `sim/dragon_archetype.rs` (`Genome`), `sim/dragon_brain.rs` (`BrainState::new(&Genome)`).

---

## T-plants — more plant species / blueprints
**WID:** `r336-plants`
**Why:** the plant system is rich (`plant_growth`, `plant_propagation`, `plant_root_system`,
`plant_tree_genetics`, `plant_gene_express`, `grass_field`, `world::plant_blueprint`) but the operator
wants **more plants**. Add species blueprints + their growth/gene params.
**Build:** extend `world::plant_blueprint` (and `sim::plant_gene_express` if a trait is needed),
additive under `// r336-plants`:
- a curated set of new `PlantBlueprint`s (e.g. ferns, reeds, vines, shrubs, wildflowers, mushrooms)
  with their morphology + biome affinity + growth-rate params, mirroring the existing blueprint shape.
- a `blueprints() -> &[PlantBlueprint]` (or extend the existing catalog) the worldgen scatter consumes.
**Anchors:** `world/plant_blueprint.rs`, `sim/plant_growth.rs`, `sim/plant_propagation.rs`,
`sim/plant_gene_express.rs`, `sim/grass_field.rs`, `world/leaf_decay.rs`.
**Verify:** ≥6 tests (each new blueprint is well-formed: non-empty morphology, valid biome affinity,
growth params in range; catalog non-empty + no dup ids; deterministic; a blueprint grows under
`plant_growth` without NaN). Self-contained or flatten with the blueprint type.
**Watch:** match the existing blueprint contract exactly so the scatter/mesher consume them unchanged.

---

## T-textures — more block textures / atlas coverage
**WID:** `r336-textures`
**Why:** `texture_atlas` + `moisture_texture` (crate root) drive block appearance. Operator wants
**more textures** — broaden atlas coverage / variation.
**Build:** extend `texture_atlas` (additive, `// r336-textures`):
- atlas-slot/UV definitions for additional block faces + variants (the pure UV-rect + slot-id math).
- per-block texture variation (deterministic pick from a small variant set by block position) so
  surfaces don't tile obviously — a pure `variant_for(block, pos) -> u8` + its UV resolution.
**Anchors:** `texture_atlas.rs`, `moisture_texture.rs`, the `BlockId` set, `render/mesher.rs`
(how UVs are consumed — READ ONLY; propose any new-slot wiring).
**Verify:** ≥6 tests (every defined slot has in-bounds UVs; variant pick deterministic + uniform-ish
over positions; no two slots overlap; round-trip slot↔UV; deterministic). Self-contained → `rustc --test`.
**Watch:** if new atlas slots need texture image bytes, those are an asset/bin step — propose in a
handoff; ship the pure slot/UV/variation logic only.

**BONUS (operator) — remove the leaf/bush placeholder:** leaves and bushes currently render a placeholder/fallback texture. Give them real atlas slots + UVs (find the fallback the mesher uses for leaf/bush BlockIds — likely a default slot or a `// TODO` UV — and replace it with proper foliage slots, with the alpha-test/cutout flag the leaf pass expects). Add a test that no leaf/bush BlockId resolves to the placeholder slot. Texture image bytes remain an asset step (handoff); ship the slot/UV logic.

---

## T-weather — global weather system
**WID:** `r336-weather`
**Why:** `world::weather` has `WeatherKind`/`WeatherState` (`liquid_rain`/`snow_amount`/`kind`), but the
operator wants **global weather** — a world-wide weather cycle that transitions clear↔rain↔storm↔snow
over time, biome/temperature-aware.
**Build:** extend `world::weather` (additive, `// r336-weather`):
- `GlobalWeather { current: WeatherKind, intensity01, ticks_remaining }` + `step(dt, seed, temp01,
  humidity01) -> GlobalWeather` — a deterministic (seeded) weather state machine: transition
  probabilities gated by temperature (snow only when cold) + humidity (rain needs moisture), with
  min/max durations so it doesn't flicker.
- `precip_for(&GlobalWeather, temp01) -> Precip { rain01, snow01 }` reusing the existing
  `liquid_rain`/`snow_amount` semantics.
**Anchors:** `world/weather.rs` (`WeatherKind`@119, `WeatherState`@130, `liquid_rain`@165,
`snow_amount`@171, `weather_enabled`@189), `world/weathering.rs`, `world/biome_temp_map.rs`.
**Verify:** ≥6 tests (deterministic for a seed; snow only below a temp threshold; rain needs humidity;
durations respected (no 1-tick flips); intensity bounded; clear↔storm reachable). Self-contained →
`rustc --test`. Propose the bin tick + sky/particle hookup in `HANDOFF_weather.md`.

**BONUS (operator) — cloud shaders:** `world::cloud_coverage`/`batch_cloud_coverage`/`precip_rate` already give cloud density math — wire it to a cloud *shader*. Author a NON-off-limits shader (e.g. `shaders/clouds.frag`, NOT `froxel_fill.comp`/`chunk.frag`), passing `glslangValidator -V --target-env vulkan1.2` + `spirv-val` with `.spv`/`.hash` committed, plus the pure coverage→shader uniform mapping (coverage, density, sun dir, wind). Ground in `world/cloud_coverage.rs`; keep the GlobalWeather→coverage feed pure.

---

## T-biomes — more biome types
**WID:** `r336-biomes`
**Why:** `world::biomes::Biome` has ~16 variants; operator wants **more biomes**. Add biome types +
their climate/color/terrain params.
**Build:** extend `world::biomes` + the climate maps (additive, `// r336-biomes`):
- new `Biome` variants (e.g. Savanna, Jungle, Taiga, Mesa, Mangrove, VolcanicWastes, FrozenOcean) with
  their `biome_temp_map` temperature/humidity cells, `biome_blend` palette/weights, and surface
  block choices — mirroring an existing variant end-to-end so each new biome is fully parameterized.
- keep `Biome as u8` ids **append-only** (new variants get new high ids — don't renumber, the wire/
  save formats depend on the discriminants).
**Anchors:** `world/biomes.rs` (`Biome`@8, append after id 15), `world/biome_temp_map.rs`,
`world/biome_blend.rs`, `world/biome_blend_weight.rs`, `world/biome_transition.rs`, `world/biome_loader.rs`.
**Verify:** ≥6 tests (each new biome has a temp/humidity cell + palette + surface block; ids are
append-only + unique; blend weights sum sanely; a new biome round-trips through the loader;
deterministic). `cargo test --features "render-core egui" --lib` (touches the shared `Biome`) or flatten.
**Watch:** `Biome` is matched exhaustively in many places — the **coordinator sweeps exhaustive matches
on merge** (like `ItemKind`). List every new variant in the MANIFEST so the sweep is complete.

**BONUS (operator) — fix ocean grass/trees:** despite the gating at `world/flora_placement.rs:173` ("Ocean + all cave biomes: no surface flora") and `biomes.rs:84/199` (no trees in oceans), grass/trees still appear in/over ocean. Find the LEAK — likely `grass_field` scattering independently of `flora_placement`, or biome-blend at ocean shorelines bleeding land flora below the water line — and gate it by water level + Ocean/FrozenOcean biome. Add a test: no flora placed where surface_y < water_level or biome is Ocean. This is a bugfix; keep it surgical.

---

## T-poi-structure — more POI + structure types
**WID:** `r336-poi-structure`
**Why:** `world::rare_structure` (`RareKind{Obelisk,Shrine,Ruin,Monument}`, just merged) +
`structure_grammar`/`structure_template`/`structure_place` exist; operator wants **more POI and
structure**. Add rare-structure kinds + their grammars + (optionally) ambient POIs.
**Build:** extend `world::rare_structure` + `world::structure_grammar` (additive, `// r336-poi`):
- new `RareKind` variants (e.g. Tower, Henge, Statue, Bridge, Camp, Well) + their `axiom`/biome
  affinity/rarity entries, extending `RareKind::ALL`.
- matching L-system axioms/rules in `structure_grammar` (or templates in `structure_template`) so
  `to_placements` lays a recognizable footprint for each.
- optional: a lightweight POI marker set (non-structure landmarks) if it fits the existing POI cache.
**Anchors:** `world/rare_structure.rs` (`RareKind`@67, `ALL`@79, `select_for_region`),
`world/structure_grammar.rs` (`expand`, `to_placements`), `world/structure_template.rs`,
`world/structure_place.rs` (`rank_sites`, `suitability`).
**Verify:** ≥6 tests (each new RareKind has an axiom + valid rarity entry; `ALL` covers every variant;
grammar expands each axiom to ≥1 placement; min-spacing still holds; deterministic). Self-contained →
`rustc --test`.
**Watch:** if T-biomes lands new `Biome` ids, the **coordinator reconciles** this thread's
`allowed_biomes` masks on merge — author masks over the current `Biome` ids and note it.

---

## T-neural — neural-rendering speedups (quantize / GEMM packing)
**WID:** `r336-neural`
**Why:** the neural shading path (`render::neural_shade_pipeline`/`neural_gemm_pipeline`/`q8`) bakes a
trained net via Q8_0 quantization. Operator wants **speedups**. The pure, testable lever is the
**CPU-side quantize + GEMM weight packing** that feeds the shader — faster/tighter packing, not the
shader itself.
**Build:** optimize `render::q8` + the pure packing in `neural_gemm_pipeline` (additive,
`// r336-neural`):
- a faster `quantize`/`quantize_layer` (block-wise, fewer passes; keep **byte-identical** output to
  the current validated quantizer — it must still match `oracle.quantize_q8`).
- improved weight **pre-packing / tiling** (the CPU layout the GEMM shader consumes) to cut bandwidth:
  a pure `pack_tiles(weights, tile) -> Vec<u32>` + its inverse, with a documented layout.
- optional: a `q4` experiment **only** behind a new fn (don't change q8 output).
**Anchors:** `render/q8.rs` (`quantize`@21, `quantize_layer`@49), `render/neural_gemm_pipeline.rs`,
`render/neural_shade_pipeline.rs` (READ — the shader contract the packing must match),
`render/neural_train.rs`. **Do NOT touch** `froxel_fill.comp`/`chunk.frag` or any `.comp`/`.frag`
shader (off-limits); this is CPU-side prep only.
**Verify:** ≥6 tests (quantize output byte-identical to the current impl over fuzzed weights — a
golden-vector parity test; dequant(quantize(w)) within Q8 error bound; pack→unpack round-trips;
faster path == slow path bit-for-bit; deterministic). Self-contained → `rustc --test`.
**Watch:** **parity is sacred** — any speedup must produce identical quantized bytes, or the baked net
diverges from training. Prove it with a golden-vector test vs the current quantizer.

**BONUS (operator) — 25% min slider + 4x upscale:** lower `settings_model::RS_MIN` from 0.5 to 0.25 (25%) and support 4x neural upscaling (quarter-res → full). Add the pure render-scale→upscale-factor mapping (0.25→4x, 0.5→2x, 1.0→1x) + the neural-upscale tile math for the 4x path, clamped so 0.25 is the floor. Anchors: `settings_model.rs:119` (RS_MIN), `render/neural_shade_pipeline.rs`/`neural_feat_pipeline.rs` (the upscale path). Keep quantize parity intact; the 4x path is additive.

---

### Coordinator checklist
- [ ] Cut `worker-<WID>-r336.zip` = base + `WORKER_ID` + brief + `SHARED_RULES.md`.
- [ ] On return merge in order; **sweep `Biome` exhaustive matches** (T-biomes) and reconcile
      `rare_structure` biome masks (T-poi-structure); verify T-dragon-size respects the r332 clamp;
      confirm T-neural quantize parity; roll `unendlich-r337-merged.zip`.
- [ ] Continue MP in-house (net_transport_select item 2 / world_presence item 3) + the DHT-node IO now
      that `world_management` Open-to-DHT + `world_advert` are in.
