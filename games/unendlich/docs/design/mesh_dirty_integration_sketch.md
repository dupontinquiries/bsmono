# mesh_dirty integration sketch — completes the r247 fix (NOT APPLIED)

`util::mesh_dirty::MeshDirtyTracker` (landed this turn, 9/9 tests green,
compiled standalone) is the safe, pure half of the r247 finding: a
per-creature "did anything that affects its mesh actually change" decision,
quantized so physics/anim float noise doesn't force a rebuild every frame.

This doc is the integration sketch for the other half — hooking it into
`unendlich_fullgame.rs`'s creature-mesh block (~line 11979-12581) — written
out but **not applied**. That block is a ~600-line hot render path mixing
roamer snapshots, test creatures, LAN remote avatars, and the player's own
character, with no GPU/display available here to confirm it doesn't
regress rendering. This project's own log has three prior instances of
blind edits to exactly this class of code landing wrong (r175b, r182b,
r234, r245's parallax regression) — so this stays a sketch for the next
hardware session rather than a fifth entry in that list.

## What changes, conceptually

Today (`snapshot_to_vertices` + the `creature_to_vertices` loop):
every creature, every frame → build fresh `Vec<Vertex>`/`Vec<u32>` → concat
into one big buffer → `renderer.upload_entities(&ctx, &everts, &eindices)`.

With the tracker: keep a `MeshDirtyTracker<CreatureId>` (one instance
outside the frame loop, alongside `entity_store`/`test_creatures`). Each
frame, for each creature, build a `PoseKey` from its current
position/yaw/pitch/roll/gait_phase/torso_bend/anim_time/tint and ask
`should_rebuild`. Only creatures that return `true` get their per-creature
vertex slice rebuilt; everyone else reuses last frame's slice. Concat as
before. Skip the whole `upload_entities` call on a frame where nothing
changed (`tracker.tracked_count() > 0 && zero rebuilds this frame`).

## Why this is more than "wrap it in an if"

The real win isn't just skipping `creature_to_vertices` for a stationary
creature — the current code also **reallocates the whole shared buffer**
(`everts`/`eindices`) every frame regardless. A faithful fix needs a
per-creature vertex-range cache (id → (offset, len) into a persistent
buffer) so an unchanged creature's bytes are neither recomputed nor
re-copied, only a changed creature's range gets replaced and the rest
copied forward (or, better, a buffer laid out so unchanged ranges don't
move at all — append-mostly with a free-list for despawns). That's a
real data-structure decision (contiguous-with-holes vs. per-creature GPU
sub-allocations vs. a generation-tagged upload diff), not a one-line
guard, which is the other reason this wants a build-and-profile loop
before landing: the *shape* of the cache affects whether it actually
helps once fragmentation from spawns/despawns sets in.

## Sketch (pseudocode, not compiled against the real block)

```rust
// Outside the frame loop, alongside entity_store/test_creatures:
let mut mesh_dirty: unendlich_engine::util::mesh_dirty::MeshDirtyTracker<u64> =
    unendlich_engine::util::mesh_dirty::MeshDirtyTracker::new();
// cached per-creature vertex/index ranges from last frame, keyed the same way
let mut mesh_cache: std::collections::HashMap<u64, (Vec<Vertex>, Vec<u32>)> = Default::default();

// Inside the per-frame block, per creature:
let key = PoseKey::new(c.body.position, c.yaw, c.pitch, c.roll,
                        c.gait_phase, c.torso_bend, c.anim_clock, c.tint);
if mesh_dirty.should_rebuild(c.id, key) {
    let (cv, ci) = creature_to_vertices(c);
    mesh_cache.insert(c.id, (cv, ci));
}
// then concat mesh_cache values in stable id order (same order every
// frame, or the index rebasing breaks) into everts/eindices as today.

// On despawn/death (wherever that's already handled):
mesh_dirty.forget(dead_id);
mesh_cache.remove(&dead_id);
```

`invalidate_all()` should be called wherever a debug-render toggle that
isn't part of `PoseKey` changes (e.g. a future wireframe/hitbox overlay
toggle), so a setting flip doesn't silently show stale geometry.

## What the next session needs to actually land this

1. Decide the buffer layout (append-mostly + free-list is simplest to get
   right first; a true sub-allocated GPU buffer is the fuller win but
   bigger surface area).
2. Wire the sketch above into the real block.
3. Build with `UNENDLICH_FRAME_PROFILE=1`, compare `creature_mesh` p50/p95
   before/after with the same population (~200 creatures per the r247
   finding).
4. Visually confirm nothing freezes mid-animation (a stale cache entry
   would look like a creature stuck mid-stride) — this is the check that
   genuinely needs a screen and can't be done here.
