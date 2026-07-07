# Helper-library metrics scratchpad (guide §7b format; update in place)

── HELPERS ─────────────────────────────
fn             | mod        | call_sites | p_id
mix64          | hash_util  | 6          | 4.1  (design,terrain_warp,cave_moss,plant_exotics,batch_ridge_noise,+viewer-via-unit)
mix64_unit     | hash_util  | 5          | 4.1  (+spark_burst,plant_gene_traits salt-fold)
mix64_2        | hash_util  | 1          | 4.1
axis_bounds    | bbox       | 5          | 4.2
norm3          | vecmath    | 1          | 4.3
lerp3          | vecmath    | 0          | 4.3
scale_clamp01  | vecmath    | 0          | 4.3
len3           | vecmath    | 1          | 4.3

── FILE TALLY ──────────────────────────
file               | p_ids    | loc- | helpers_called
creature_design.rs | 4.1,4.2,4.3 | 24 | skin_mix->mix64(1) skin_unit->mix64_unit(1) axis_bounds(2) norm3(1)
creature_viewer.rs | 4.1,4.2  | 16   | hash_unit->mix64_unit(1) axis_bounds(3)
world/reef_seeder.rs | 4.1    |  4   | mix64_2(1)

── SESSION TOTALS ──────────────────────
files_processed:  9 / ~1030
loc_removed:      80
call_sites_added: 19
reuse_ratio:      19 / 7 = 2.7  (target > 3.0 at 50 files; mix64 now 8 sites)
perf_gains:       0 (parity-preserving session; const-fn mix64 enables
                  future const seeding)
new_patterns:     0
notes:            reef h2 -> mix64_2 reshuffles coral layouts once
                  (pre-release cosmetic; determinism per inputs intact).

── GREP_RESULTS (session checkpoint; §3b.1 run 2026-06-12) ──
P4.1 splitmix: audio spark_burst creature_viewer creature_design
  world/{terrain_warp,plant_exotics,stem,batch_ridge_noise,cave_moss,
  plant_gene_traits,flora_placement} sim/mob_spawn_director [+more]
P4.2 bounds-fold: water_depth_color sun_disk bloom_math wing_membrane
  creature_viewer creature_design spawn world/{karst_sink,
  sound_attenuation,altitude_temp,biome_blend_weight} [+more]
P4.5 rem_euclid ring: (run next session — list not yet captured)
priority head: mob_spawn_director(3) creature_design(3) entity(2)
  chunk_manager(2) spark_burst(2) wind_field(2) seed_dispersal(2)
NEXT SESSION: 4.1 splitmix cluster DONE (9 files: creature_design,
  creature_viewer, reef_seeder, spark_burst, terrain_warp, cave_moss,
  plant_exotics, batch_ridge_noise, plant_gene_traits). SKIPPED (not
  true splitmix): world/stem.rs (XOR-accumulate), world/flora_placement
  (salt^GOLDEN init -> different output). NEXT: pattern 4.2 bbox cluster
  (water_depth_color, sun_disk, bloom_math, wing_membrane, spawn,
  world/karst_sink...) then 4.5 ring (capture rem_euclid grep first).
