# Vision-Driven Mesh Scheduling (experimental)

> See also `dynamic_world_architecture.md` — the three vision consumers (player / mob / sim) and the tiered mesher this scheduling generalises into.

> Part of the perf work — see `PERF_PROFILING.md`, `grass_field_batch.md`.

The biggest meshing waste is meshing geometry **nothing can see**. The engine
already GPU-frustum-culls at *draw* time (`render::chunk_indirect` runs
`chunk_cull.comp`), but that's after the expensive part — building the vertex
data — already ran. A chunk gets fully meshed when it loads or changes, blind
to visibility. Dense **cave** chunks are the pathological case: a cube of
twisty rock has enormous interior surface area, almost all occluded, and we
mesh every face though the player sees only the corridor they walk.

This is a **meshing-time visibility gate**: mesh what view cones (player +
nearby mobs) actually need now/soon, and defer the rest. It generalises to most
blocks — any chunk no cone needs is deferred, so meshing cost tracks *what's
about to be looked at*, not the whole loaded set.

Status: the **decision model** (`crates/unendlich-engine/src/sim/mesh_vision.rs`)
is pure, always-built, unit-tested (9 tests). A **first integration pass** is
wired behind the `mesh-vision-sched` cargo feature (off by default), r240:
the chunk-manager shell-walk now builds the player view cone and DEFERS chunks
no cone needs (behind the player / out of range) from meshing. **Occlusion is
still OFF** in this pass (conservative — never defers an in-view chunk, so no
holes in front of you); the occlusion-backed cave win is the next step. NEEDS
A RUN to validate (compile-verified only — no GPU in the build sandbox).

### Wired (r240, conservative pass)
- `ChunkManager` gained a gated `player_view_yaw` + `set_player_view_yaw`,
  fed each frame from `local_player.view.yaw` in the bin.
- In the shell-walk (`request_visible`), each candidate chunk gets
  `urgency_for_cones(&[player_cone], chunk_center, |_,_| false)`; on `Defer`
  the chunk is skipped for meshing this pass (and its budget slot refunded).
  Voxels still load; the mesh is rebuilt when the player turns toward it
  (the walk runs every update).
- Occlusion predicate is hardcoded `false` for now → only clearly-unseen
  (behind / out-of-range) chunks defer. Wiring a real LOS/occlusion source
  (raycast → GPU occlusion query) is the follow-up that unlocks the
  dense-cave-interior win.

## The model

A `ViewCone { pos, yaw, half_fov, range, turn_margin }` is an eye. The player
is one; nearby mobs can contribute theirs so their immediate surroundings stay
detailed even slightly off the player's axis.

`urgency_for_cone(cone, chunk_center, occluded) -> MeshUrgency`:
- **`Now`** — in fov, within the near half of range, not occluded → mesh this
  frame.
- **`Soon`** — in fov but far; OR just outside fov within `turn_margin` (about
  to rotate in → prefetch); OR in-fov-but-occluded yet near (a step could
  expose it) → mesh at low priority.
- **`Defer`** — behind the actor, out of range, or far+occluded → keep the
  voxels, skip meshing until a cone turns toward it. **The cave saving lives
  here.**

`urgency_for_cones(cones, chunk_center, occluded_for)` takes the **max** across
all cones — if the player can't see a chunk but a nearby mob can, it still gets
meshed (so the world stays correct from every POV that matters).

Cone math mirrors `sim::perception` (`horiz_angle` / the `being_watched` fov
test) so mesh-vision and AI-vision agree on "in view".

## Occlusion is injected, not hardcoded

`occluded` / `occluded_for` are predicates the *caller* supplies. The model
stays pure and testable; the real occlusion source plugs in at the call site.
Options, cheapest first:
- **Chunk-level LOS**: march the existing `raycast` from the eye to the chunk
  center; if it hits solid before arriving, occluded. Cheap, coarse, good
  enough to defer fully-buried cave chunks.
- **GPU occlusion queries / a depth pre-pass**: the precise version — last
  frame's depth tells you which chunks contributed no visible fragments; those
  are occluded this frame. Reuses the draw pipeline.
- **Portal/visibility between chunk faces** for caves specifically (which chunk
  faces connect through open air) — the most accurate for interiors, the most
  work.

## Integration recipe (the `mesh-vision-sched`-gated part)

1. **Build the cones each frame.** Player cone from the camera (pos, yaw, fov,
   `UNENDLICH_RENDER_DISTANCE`). Optionally add cones for the N nearest active
   mobs (reuse their `sim::perception` facing) so their surroundings mesh.
2. **Gate the mesh queue.** Where `chunk_manager` enqueues a chunk for meshing
   (the shell-walk / remesh path), compute `urgency_for_cones(cones,
   chunk_center, occluded_for)` and:
   - `Now` → enqueue at high priority (the existing `UPLOAD_PRIORITY` path).
   - `Soon` → enqueue at low priority / into the prefetch budget.
   - `Defer` → **don't enqueue**; mark the chunk "mesh pending" so that when a
     cone later turns toward it (urgency rises), it enqueues then.
   A chunk that's edited while deferred stays correct: its voxels are current;
   only the *mesh* is stale, and it's rebuilt on first need.
3. **Re-evaluate on movement/rotation.** When the player turns or moves, some
   `Defer` chunks become `Soon`/`Now`. Re-scan the loaded set (cheaply — it's
   just the urgency function per chunk) and enqueue the newly-needed ones. The
   `turn_margin` band means rotation has already prefetched the about-to-show
   geometry, so turning shouldn't hitch.
4. **Keep the draw-time frustum cull.** This gate decides what to *mesh*; the
   existing GPU cull still decides what to *draw*. They compose: mesh the
   soon-visible, draw the visible.
5. **Gate it all** under `#[cfg(feature = "mesh-vision-sched")]`; the
   mesh-on-load path stays default until this proves out.

## How to test (needs a build + the feature)

```
cargo run --release --bin unendlich-fullgame \
  --features "render,hot-shaders,window,gpu,ui-overlay,audio,mesh-vision-sched"
```
- Walk into a dense cave system and profile with `UNENDLICH_FRAME_PROFILE=1`:
  `manager_update` p99/max should drop sharply versus the default build
  (far/buried cave chunks no longer mesh).
- Spin in place: should NOT hitch (the `turn_margin` prefetch covers rotation).
  If it hitches on turning, widen `turn_margin` or the `Soon` budget.
- Look for holes/late geometry: if chunks pop in *after* you're already looking
  at them, the occlusion predicate is too aggressive (marking visible chunks
  occluded) or step 3's re-scan is too slow — loosen occlusion / re-scan more
  often.

## Risks / honesty
- **Correctness over speed**: a wrong "defer" shows the player an unmeshed hole.
  The occlusion predicate must be *conservative* (when unsure, don't defer).
  The pure model already biases this way (occluded+near → Soon, not Defer).
- This is a real pipeline change touching the mesh queue + needs the occlusion
  source wired; only the decision model is done here. Nothing is on the default
  path, so the engine is unaffected until the gated integration is built and
  validated in a running session.
