# Operation-gated parallelism (r196) — gate on WORK SHAPE, not area

## The shift the operator asked for
The r191 plan gated parallel work on *spatial regions* (2x2x2
checkerboard) to avoid neighbor-read races. That works but is fragile:
it assumes you can always partition space cleanly, and a single handler
that reads a diagonal neighbor breaks the guarantee silently.

New model: gate on the **shape of the work**, not where it happens.
Classify every parallelizable task into one of three access classes,
and let the scheduler run only the classes that are provably safe to run
concurrently. Breakage is prevented by the CLASS, not by hoping the
spatial split lined up.

## The three classes (THREE worker threads, one per safe lane)

### Class P — PURE MAP (read-only in, owned-value out)
A function `Fn(&In) -> Out` where `In` is shared `&` and `Out` is a fresh
owned value. No mutation of shared state, no interior mutability touched.
  Examples: worldgen column -> Vec<solid idx> (r191 already does this);
  mesh build (Chunk -> ChunkMesh); coral layout (seed -> blocks);
  per-entity AI decision (snapshot -> intent).
SAFETY: trivially data-race-free — inputs are immutable, outputs are
disjoint owned values merged serially after. rayon `par_iter().map()`.
This is the LARGE lane; most expensive work is here.

### Class C — CLONE-THEN-MUTATE (copy out, mutate the copy, swap back)
A task that needs to mutate, but does it on a CLONE: read shared state,
clone the piece it owns, mutate the clone in parallel, then the MAIN
thread swaps the clones in serially. The chunk-manager COW apply path is
already exactly this shape (clone_for_edit -> mutate -> store).
SAFETY: the parallel phase touches only thread-local clones; the only
shared write is the serial swap-back on main. No two threads write the
same datum. The clone cost is the price of the safety.

### Class S — SERIAL ONLY (in-place mutation of shared state)
Anything that mutates shared state in place, or whose output order
matters, or that touches a non-Send/non-Sync resource (GPU queue, the
egui context, file handles). These NEVER parallelize; they run on main.
  Examples: applying edits into the live DashMap; version bumps; GPU
  uploads; audio sink writes; RNG streams whose order is a butterfly.
SAFETY: by exclusion — if a task can't be proven P or C, it is S.

## The classifier is the safety boundary
A task is admitted to a parallel lane ONLY if it declares its class, and
the type system enforces the declaration:
  - Class P closures must be `Fn(&In) -> Out + Send + Sync` and capture
    nothing mutable (no `&mut`, no `Cell`/`RefCell` in scope).
  - Class C tasks produce an owned `Patch` value; the apply is a separate
    serial step taking `&mut Target`.
  - Anything that won't fit those signatures is S and the API won't let
    you submit it to a worker — it has no parallel entry point.
This is why gating on shape beats gating on area: a diagonal-neighbor
read makes a task fail the Class-P signature (it'd need a wider `&In`),
so it's caught at compile time, not as a runtime race.

## Three workers, mapped to classes
  worker 0,1: Class-P pool (rayon work-stealing over par_iter) — the
    bulk lane (mesh, worldgen columns, AI decisions).
  worker 2: Class-C lane — runs clone+mutate tasks producing Patches,
    fed to a serial apply on main between frames.
  main thread: Class-S — apply, uploads, version bumps, audio.
The split is 2+1 rather than 3 equal because Class P dominates wall time.

## Determinism (preserved — the harness is the gate)
- Class P merge is order-independent (disjoint outputs) OR merged in a
  FIXED key order (sorted coords) when the consumer cares.
- Class C swap-back happens in a FIXED order on main.
- Class S is already serial.
Every conversion lands behind examples/headless_run.rs: the per-tick
digests must be byte-identical before/after. A digest change = a
mis-classified task (something declared P/C that actually had a shared
write) — the harness catches the exact tick it diverges.

## Migration order (lowest risk first)
1. Class P, already-pure work: worldgen columns (DONE r191), mesh build
   (already on a worker pool), coral layout. Verify digests unchanged.
2. Class P, AI decisions: per-entity intent from a world snapshot —
   currently serial; convert to par_iter over a frozen snapshot.
3. Class C: the plant/leaf lifecycle catch-up — already produces a
   Patch-like TickEdits; move the COMPUTE to the Class-C worker, keep
   the apply (set_instance_state) on main (it's Class S).
4. NEON/SIMD (separate axis): only inside Class-P kernels, where a lane
   is a pure map — the SoA noise path is the template. (Note: r196
   bench shows target-cpu=native gives ~0% on the current worldgen
   path; SIMD only helps kernels that are actually arithmetic-bound,
   measure per-kernel before converting.)

## What this is NOT
- Not a replacement for the spatial split where it already works
  (worldgen columns are both P AND spatially disjoint — belt and braces).
- Not a license to parallelize Class-S work by adding locks. If it needs
  a lock, it stays on main. Locks are how the breakage the operator
  wants to avoid sneaks back in.
