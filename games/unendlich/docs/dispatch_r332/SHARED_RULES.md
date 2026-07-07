# SHARED RULES — all r330 threads (read first)

You are ONE worker thread building a slice of **unendlich** (Rust voxel/dragon Vulkan
engine, Linux/Asahi, Apple M2, FP32). A coordinator merges your return, gates it on
real hardware via the operator. Follow these exactly.

1. **PURE CORE, OR STOP.** Deliver pure, deterministic, panic-free engine code under
   `crates/unendlich-engine/src/...`. **No `unwrap`/`expect`/`panic!`/IO/RNG/clock** in
   non-test code (use `Option`/`Result`, saturating arithmetic, passed-in time/seeds). If
   your task can't be done as pure core + a headless/numeric proof, STOP and write down
   what's blocking — don't half-wire the bin.

2. **NEVER edit the bin or the glfw/window/ui-overlay path.** `unendlich_fullgame.rs`,
   `crates/unendlich-ui`, and anything behind `window`/`ui-overlay` CANNOT compile in your
   sandbox (glfw-sys needs cmake + X11/GL, not allow-listed). The coordinator does bin
   wiring on-device. You may **PROPOSE** bin wiring in a handoff `.md` — not apply it.

3. **VERIFY IN-SANDBOX — this is the merge bar:**
   - Toolchain is Rust **1.96** (already at `/home/claude/.local/bin` or `/usr/local/bin`;
     if missing, install from the offline bundle). `Cargo.lock` is v4 — no lock-dance.
     Always `CARGO_INCREMENTAL=0`.
   - Fast pure-module gate (no glfw link): copy your self-contained module to `/tmp` and
     `rustc --edition 2021 --test mod.rs -o /tmp/t && /tmp/t`. This is the accepted floor
     for a dependency-free `sim::` module.
   - If your module references `crate::`, gate with
     `cargo test -p unendlich-engine --features "render-core egui" --lib <your_module>`.
   - **Prove correctness with assertions or pictures.** Numeric tests for logic; for any
     visual/mesh claim, emit a PNG via a headless example (copy `examples/sim_collage.rs`
     pattern) and ship it.
   - GLSL you author MUST pass `glslangValidator --target-env vulkan1.2 -V` **and**
     `spirv-val`. After editing a shader, regenerate the committed `.spv`+`.hash`.

4. **STAY IN YOUR FILES.** Add new `sim::<x>` modules as new files. Touch `sim/mod.rs`
   with **ONE** `pub mod` line only (coordinator reconciles). Don't restructure shared
   scaffolding; if you must extend a shared file, add your own fns/types under a header
   comment naming your WID.

5. **Don't add dependencies** without flagging (breaks the offline/allow-listed build).

6. **RETURN FORMAT:** a `CHANGES-<WID>.patch` (`git diff` / `diff -u`, applies `-p1`), the
   raw new `.rs` files at their repo paths (this is the accepted floor — patch is welcome
   but optional), any PNG proofs, and a short `MANIFEST` block: files touched, API added,
   the exact verify commands you ran + their results, and handoff notes for bin wiring.
   **Do not block on a full `cargo build`** — the coordinator runs that on merge.

PRE-EXISTING FAILURES (don't fix, don't worry): loot `one_destroyed_eye_halves_the_amber`,
mob `losing_both_eyes_blinds_it`, smoke_gpu `pure_decay_monotonic_decrease`.

OFF-LIMITS to all workers (coordinator owns): `shaders/froxel_fill.comp`, `shaders/chunk.frag`.
