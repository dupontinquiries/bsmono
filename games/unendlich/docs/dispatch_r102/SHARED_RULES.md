# SHARED RULES — all r102 threads (read first)

You are ONE worker thread building a slice of "unendlich" (Rust voxel/dragon Vulkan
engine, Linux/Asahi, Apple M2 Max, FP32). A coordinator merges your return, gates it,
and the operator runs the fullgame bin on real hardware. Follow these exactly:

1. PURE CORE, OR STOP. Deliver pure, deterministic, panic-free engine code
   (`crates/unendlich-engine/src/...`). NO `unwrap`/`expect`/`panic!`/IO/RNG in
   non-test code. If your task can't be done as pure core + a headless proof, STOP
   and write down what's blocking instead of half-wiring the bin.

2. NEVER edit the bin or the glfw/window/ui-overlay path. `unendlich_fullgame.rs`,
   `crates/unendlich-ui`, and anything behind `window`/`ui-overlay` CANNOT compile in
   your sandbox (needs glfw-sys: cmake + X11/GL, not allow-listed). The coordinator
   does bin wiring on-device. You may PROPOSE bin wiring in a handoff `.md`, not apply it.

3. VERIFY IN-SANDBOX — this is the bar for merge:
   - `cargo check --release -p unendlich-engine --features render` must be green.
   - Your module's tests pass: `cargo test -p unendlich-engine --features gpu --lib <your_module>`
     (use `--features render,egui` if you touch egui/menu/peeper code).
   - "Failures as pictures": the bin can't run here, so PROVE correctness with either a
     headless example that emits a PNG collage (dependency-free CPU rasteriser + PNG
     encoder — copy the pattern from `examples/sim_collage.rs` or `creature_surface_collage.rs`)
     OR numeric assertions in tests. Ship the PNG in your return.
   - GLSL you emit/author MUST pass `glslangValidator -V` AND `spirv-val` (both at /usr/bin).

4. Rust 1.96 in-sandbox reads `Cargo.lock` v4 natively — no lock-dance needed. Always
   `CARGO_INCREMENTAL=0`. Don't add dependencies without flagging it (new deps can break
   the offline/allow-listed build).

5. STAY IN YOUR FILES. If your module shares a file with another thread (e.g. a new
   `sim::<x>` everyone extends), ADD your own functions/types with a header comment naming
   your WID; don't restructure shared scaffolding. Touch `sim/mod.rs` with ONE `pub mod`
   line only — expect the coordinator to reconcile it.

6. RETURN FORMAT: a `CHANGES-<WID>.patch` (`git diff` / `diff -u`, `-p1` apply), the raw
   new `.rs` files, any PNG proofs, and a short `MANIFEST` block: files touched, API added,
   exact verification commands you ran + their results, and any handoff notes for bin wiring.

PRE-EXISTING TEST FAILURES (don't worry about these 3): loot
`one_destroyed_eye_halves_the_amber`, mob `losing_both_eyes_blinds_it`, smoke_gpu
`pure_decay_monotonic_decrease`. A clean run is 6897 pass / 3 fail.
