# entity_sanitize integration — the exploding-triangles guard (NOT APPLIED)

`world::entity_sanitize` (landed this turn) is the missing NaN/Inf guard for the
creature/entity render path. Built and tested standalone — 7/7 tests, including one
that caught a wrong assumption in my own first test draft (a quad-fan's two
triangles don't both touch every vertex; the sanitizer is precise enough to drop
only the actually-affected triangle, which the test initially got wrong and the
real logic corrected it against).

## The finding, restated precisely

Two structurally different render paths exist in this engine:

- **Chunk mesh** (`render::mesher::PackedVertex`): every position is `.clamp(0.0,
  1.0)`-ed before packing into 10-bit fixed-point, and Rust's `as u32` cast
  saturates NaN to 0 rather than producing an undefined bit pattern. PLUS explicit
  `is_finite`/`is_nan` guards on top (2 hits in `mesher.rs`). This path is
  structurally close to immune to the "exploding triangles" bug class.
- **Entity/creature mesh** (`world::entity::forge_to_vertices`/`creature_to_vertices`
  → `render::entity_pipeline`): full-precision `EntityVertex { position: [f32; 3],
  .. }`, taken directly from `creature.body.position`/`.velocity`, `.yaw`, `.pitch`,
  `.roll` — physics-sim f32 state with no invariant enforced anywhere. Grep confirms
  **zero** `is_finite`/`is_nan` hits across `physics.rs`, `creature.rs`, and
  `entity_pipeline.rs`.

Any one of the many systems that write creature transform state — combat
knockback, bending-ability launches (`sim::player_bending`), dragon
breath/flight impulses, teleport-on-respawn, and others accumulated across this
project's ~250-entry development log — producing one NaN or a runaway velocity is
enough. It reaches the GPU vertex buffer with nothing in between to catch it. A
NaN in clip-space position after the MVP transform is the textbook cause of the
"triangles filling the screen" artifact (rasterizer edge functions are undefined
for NaN inputs).

## What's built, and verified

`entity_sanitize::sanitize<V: HasPosition>(verts, indices) -> Option<(Vec<V>,
Vec<u32>, SanitizeReport)>`:
- Fast path: one linear scan, no allocation, returns `None` when everything's
  already finite — the overwhelmingly common case, so steady-state cost is a few
  ns per vertex, nothing more.
- Slow path (only when something's actually broken): drops per-TRIANGLE, not
  per-creature. A creature's boxes are independent (`push_box_oriented` — each
  box's quad is a closed 4-vertex/2-triangle set nothing else references), so a
  single bad wing-tip vertex loses that wing-tip's two triangles, not the whole
  dragon.
- Defends against out-of-range indices too, not just bad positions — a garbage
  index reaching this far is the same failure shape.
- `EntityVertex` already implements the `HasPosition` trait this turn (in
  `world/entity.rs`), so it's ready to call — `entity_sanitize::sanitize(&everts,
  &eindices)` type-checks against the real vertex type today.

## Why the actual wiring isn't in this patch

The call site is inside the same ~600-line hot creature-mesh block in
`unendlich_fullgame.rs` (~line 11979-12581) flagged in `PERF_FINDING_creature_mesh.md`
and `mesh_dirty_integration_sketch.md` — same reasoning as both of those: no
GPU/display here to confirm a change to this block doesn't regress rendering, and
this project's own log shows repeated instances of blind render-loop edits landing
wrong (r175b, r182b, r234, r245's parallax regression).

This integration is lower-risk than the mesh-dirty or segmented-allocator ones,
though — it's a single call wrapping existing data, not new state or draw-order
surgery:

```rust
let (mut everts, mut eindices) = snapshot_to_vertices(&snap, default_color_for_kind);
for c in &test_creatures {
    let (cv, ci) = creature_to_vertices(c);
    let base = everts.len() as u32;
    everts.extend(cv);
    eindices.extend(ci.iter().map(|i| i + base));
}
// ... (remote avatars, self-avatar, appended the same way) ...

// NEW: one guard right before the buffer goes to the GPU.
if let Some((clean_v, clean_i, report)) =
    unendlich_engine::world::entity_sanitize::sanitize(&everts, &eindices)
{
    if report.dropped_triangles > 0 {
        log::warn!(
            "[entity_sanitize] dropped {} triangle(s) / {} vertex/vertices with \
             non-finite position this frame — a creature transform produced NaN/Inf",
            report.dropped_triangles, report.dropped_vertices,
        );
    }
    everts = clean_v;
    eindices = clean_i;
}
renderer.upload_entities(&ctx, &everts, &eindices);
```

That's it — no new state, no changed draw calls, no changed buffer layout. The
risk is purely "does this silently mask a real bug instead of surfacing it,"
which the `log::warn!` on `dropped_triangles > 0` addresses: if this ever fires,
that log line is the smoking gun pointing at exactly which frame and how much
geometry got caught, instead of a silent visual glitch with no trail. I'd
recommend landing this warn-on-catch behavior FIRST (even before deciding
whether to also chase down the specific upstream writer that produced the bad
value) — it turns "creature explodes sometimes, no idea why" into "the log says
which frame, and `report.dropped_vertices` tells you how much was corrupted,"
which is the actual diagnostic step needed to find the root cause upstream.

## What I'd verify on hardware before landing

1. `cargo check` the whole tree (needs rustc ≥1.78, unavailable here).
2. Confirm the fast path really does cost nothing measurable — profile with
   `UNENDLICH_FRAME_PROFILE=1`, compare `creature_mesh` zone timing before/after
   with a normal (non-corrupted) creature population. Should be within noise.
3. Deliberately inject a NaN (a debug console command or a temporary test hook
   setting one creature's `body.position[1] = f32::NAN`) and confirm: (a) the
   warn log fires with a sane triangle/vertex count, (b) the rest of that
   creature and every other creature render normally, (c) no crash/hang.
4. Leave it running for a while in a normal session and see whether the warn
   log ever fires at all — if it does, that's the actual "exploding triangles"
   root cause, caught red-handed with a frame number and a magnitude, instead of
   a symptom report with no lead.
