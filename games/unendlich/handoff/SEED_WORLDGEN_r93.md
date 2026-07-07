# r93 — seed→worldgen: verification landing pad ready, lifecycle wiring remains

VERIFIED IN-SANDBOX: `examples/heightmap_collage.rs` renders the world surface for
6 seeds (via `WorldGenerator::new(seed)` + `surface_y_at`) into world_heightmaps.png
+ prints per-seed min/max/mean surface y. Seeds produce visibly different terrain
(mean y 55.5..99.3). So the generator already responds to seed — the worldgen itself
is correct. What's missing is the LIFECYCLE: feeding the launch seed into a fresh
generator at "Create" time.

## The wiring that remains (bin/render, compile-on-end)
Today the bin builds `TerrainGenerator::new(WORLD_SEED_DEFAULT, ...)` ONCE at startup
(~line 315) before the menu, so `Confirm -> Launch(StartLocal{seed})` enters that
default world (the "seed->worldgen TODO" log line). To honor the seed:
1. `App::enter_game` (or the Launch handler) already has `WorldInit.seed` — carry it out.
2. On launch, rebuild the generator from that seed: `TerrainGenerator::new(launch_seed,
   backend, batch)` and reset the chunk manager so streamed chunks regenerate from the
   new seed (drop loaded/meshed chunks; re-request around spawn). The generator must be
   re-creatable post-menu — it can't stay a startup-once singleton.
3. Pick a spawn position from the new surface (`surface_y_at(spawn_x, spawn_z, ...)`).

## How to verify the wiring landed
Re-run `cargo run --example heightmap_collage` is the worldgen check. For the bin
itself: launch with two different fixed seeds (the wizard's Fixed(N)) and confirm the
spawn terrain differs + the logged seed matches. The harness is the ground truth for
"does seed change the world"; the bin step is "does Create use the launch seed."
