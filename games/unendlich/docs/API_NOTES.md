# API gotchas (session-learned; read FIRST, saves re-discovery)
- TickContext field: tick_dt (NOT dt); TickEdits.handler_timings is
  Vec<(&'static str, u64)> (NOT handler_us).
- BlockInstanceState { age_ticks: u8, flags: u8 }; advance API:
  plant_lifecycle::{advance_one_tick, advance_n_ticks} (pub).
- ChunkCoord::new(x,y,z); CHUNK_SIZE in constants; Chunk::block_index.
- WorldGenerator::new(u32 seed); generate_chunk(coord);
  surface_y_at(x,z,ymin,ymax) -> Option<i32>; biome_at_3d(..).biome;
  sea_level_at(x,z) -> f64 (VARYING, r188+ fills follow it).
- set_instance_state path clones chunk + bumps version (= REMESH);
  r191: age-only batches skip the bump (chunk_manager apply).
- egui 0.34: Rounding::same(u8), Margin::same(i8) (INTS);
  Event::Key{ physical_key: None, .. }; animate_bool for hover/open;
  load_texture only inside prepare closure (capture-locals+write-back).
- glfw: set_*_polling(true) PER EVENT TYPE (Char needed it — r183).
- InputCache: derive(Default) (no ctor); presses_this_frame(key)->u8.
- Menu picks: 1 SP, 2 Host, 3 Join, 4 Quit, 5 Confirm, 6 Back.
  Wizard actions: 1 Back 2 Next 3 Confirm 4 Cancel 5 Mode 6 SeedCycle
  7 SetName 8 SeedText 9 CreateSpree. AppStep::{Menu,Launch(plan),Quit}.
- SeedChoice lives in sim (new_world_config), not app.
- Test tier: --profile sandboxtest (debug=false beats 4GB OOM);
  setsid for background builds; probe-as-example dodges cfg(test).
- Pre-existing upstream test failures (NOT ours): loot-eye, mob-eyes,
  smoke-decay. Spirv freshness fails in-sandbox after .comp edits by
  design (operator build.rs refreshes).
