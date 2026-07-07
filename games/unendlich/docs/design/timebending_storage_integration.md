# Timebending ↔ World Storage & History Loading: The Integration

> Status: design / integration (r277). Answers the operator's question directly:
> *does the sim-vector machinery plug into world history loading? can it be part
> of world storage? timebenders can access all of the world and load any time
> from the world menu.* Grounds `statistical_timebending_and_storage.md` §4 onto
> the REAL files (`storage.rs`, `sim/save_slots.rs`) and the new machinery
> (`sim::{convergence, sim_scheme, region_digest, sim_vector}`). No storage format
> changed yet — this is the plan + where each piece attaches.

---

## 0. The short answer

**Yes to both — and the current storage design is *most* of the way there
already, because it's reconstructive, not snapshot-based.** Today a world is
`name_hash → seed` (`save_slots.rs`) + per-chunk procedural base + a flat
edit-overlay (`storage.rs`). That is exactly the right substrate for time-loading:
the world is *computed*, not *stored frame-by-frame*. The one thing it lacks is a
**time axis** — edits are "the state now," not "the state at tick T." Adding deep
time = adding that axis, and the sim-vector machinery IS the thing that computes a
requested tick from the reconstructive base.

So "load any time from the world menu" becomes: **pick a target tick → drive a
sim-vector chain from the nearest checkpoint to that tick → sim-prefill the
landing → present the materialised world.** It reuses the load path; it doesn't
replace it.

---

## 1. What storage looks like today (the substrate)

- **`save_slots.rs`**: a world is a `name_hash` → `WorldSeed { master, terrain,
  caves, flora, dragons }`. Fully reconstructable from the name — *nothing else
  is needed to regenerate the procedural world*. Seeds are derived, not stored RNG
  state.
- **`storage.rs`**: each chunk is *born procedural* from `(coord, seed,
  generator_version)`; only **explicit block edits** are persisted (`chunk.mks`
  files = a sparse `(index, block)` overlay). `load_chunk_edits` regenerates the
  procedural base then overlays the edits. World metadata (`world.mkw`) is seed +
  generator_version + last-play-time.

The load path is therefore already: **regenerate base → apply persisted deltas →
present.** This is the "regenerate-on-load" model the timebending doc wanted; it's
the same shape as "compute forward from a checkpoint."

**The gap:** the edit overlay is *timeless*. It's the current cumulative state, not
a history. There is no tick dimension, no checkpoint-at-T, no way to ask "what did
this region look like 10,000 ticks ago / forward." Dynamic-world sim (water,
populations, growth) *changes* state over time with no edit event, which the flat
overlay can't represent at all (it only stores discrete block placements).

---

## 2. The integration: add a time axis to the reconstructive model

The whole point of `statistical_timebending_and_storage.md` §4 is that you do NOT
store every tick. You store **sparse checkpoints + the reconstructive rule**, and
*compute* any requested time. The pieces map onto storage like this:

### 2a. World metadata gains a world-tick + checkpoint index
`world.mkw` (today: seed + gen_version + last-play) gains a **current world-tick**
and a small **checkpoint catalog** (a handful of `(tick, digest)` entries per
region-group — the committed checkpoints a chain can start from). This is tiny:
digests are `u64` (`region_digest`), not state.

### 2b. A region checkpoint = committed coarse state at a tick (sparse)
Per the sim-vector model, you persist a region's **coarse state** (the quantized
field vector that `region_digest` hashes) at **sparse checkpoint ticks**, not
every tick. Between checkpoints, state is *recomputed* by driving sim-vectors. So
storage grows with **number of checkpoints**, not with time — and convergent
regions need almost none (they're regenerable / Class B).

### 2c. Edits become tick-stamped (the edit-LOG, not edit-overlay)
The flat `(index, block)` overlay becomes a **tick-stamped edit log**:
`(tick, index, block)`. This is the single change that unlocks history for the
*discrete* layer — to load tick T you apply only edits with `edit_tick ≤ T`. It
also subsumes the structure/event commitments (`sim_vector` `EventCommitment`): a
dragon's build increments are tick-stamped committed edits, replayed up to T. The
overlay→log change is backward-compatible (an un-stamped edit = tick 0).

### 2d. Loading a tick = the sim-vector chain (the machinery already built)
"Load any time T" in the world menu runs:
1. Find the nearest checkpoint `≤ T` in the catalog (2a).
2. `choose_scheme(T - checkpoint_tick, classes, …)` (`sim_scheme`) → SimIt /
   SingleVector / DriveThenPrefill.
3. If vectoring: `SimVector::plan(...)` per link (`sim_vector`), chained under
   `VECTOR_MAX_SPAN`, convergent links merged by `region_digest` equality.
4. Sim-prefill the final stretch so building/growth is materialised at T.
5. Apply tick-stamped edits with `tick ≤ T` (2c) over the computed coarse state.
6. Present. (Forward OR backward in time — both are "compute T from the nearest
   checkpoint + the reconstructive rule.")

So the machinery built in r274–r277 is *exactly* the load-a-tick engine; this doc
is the storage shape it reads/writes.

---

## 3. Timebenders accessing "all of the world, any time"

The operator's feature — a timebender can load any time from the world menu — has
two scope dimensions, both handled by the model:

- **All of the world (space):** a load doesn't need every region — it materialises
  the regions the player will observe (the tiered-mesher / vision consumers from
  `dynamic_world_architecture.md`), computing each from its nearest checkpoint.
  Distant regions stay coarse (Class B / aggregate) until approached. "Access all
  of the world" = *can* compute any region at T, not *must* compute all at once.
- **Any time (time):** bounded by checkpoints + convergence. Forward jumps drive
  vectors; "backward" loads are the same (find a checkpoint ≤ T, compute up to T) —
  the world is reconstructive, so there's no "undo," just "recompute T." Path-
  dependent channels (genome/habit) are the cost; convergent ones are nearly free
  (regenerated). A deep jump (1M yr) chains vectors + merges quiescent eras
  (`vector_chaining_and_timewarp.md`), presented as the timestep animation.

**Determinism makes this safe for a shared/multiplayer world:** because a tick is
*computed* from `(seed, gen_version, tick-stamped edit log, sparse checkpoints,
committed sim-vector plans)` — all deterministic — every client that loads time T
gets the bit-identical world (`deterministic_cross_region_sim.md`). The world menu
isn't loading a saved snapshot that could diverge; it's running a reproducible
computation. That's why the sim-vector *plan* is committed (replay re-fills, never
re-searches).

---

## 4. What actually has to change in storage (the concrete diff, when built)

Minimal, additive, backward-compatible:
1. **`world.mkw`**: add `world_tick: u64` + a sparse checkpoint catalog
   `[(region_group, tick, digest)]`. (Small; digests not state.)
2. **chunk/region files**: keep the procedural-base + delta model, but
   - tick-stamp edits: `(index, block)` → `(tick, index, block)` (un-stamped =
     tick 0, so old saves load);
   - add an optional **coarse-state checkpoint** blob per region at catalog ticks
     (the quantized field vector `region_digest` hashes).
3. **load path** (`load_chunk_edits` + a new region-state loader): take a target
   tick; regenerate base; if a checkpoint exists, compute forward via the sim-
   vector chain to T; apply edits with `tick ≤ T`.
4. **bump `FORMAT_VERSION`** (storage already has the version-gate machinery for
   exactly this).

Everything else (the classify/traverse/identity/vector logic) is the already-built
`sim::{convergence, sim_scheme, region_digest, sim_vector}` — it plugs in at step 3.

---

## 5. Why this fits the existing philosophy (not a bolt-on)

`storage.rs`'s own header says chunks are born procedural and only deltas are
saved. Deep-time loading is the **same principle extended one axis**: the world is
born procedural *and evolves by a reconstructive rule*, and only **sparse
checkpoints + tick-stamped deltas + committed plans** are saved. You still never
store the bulk; you compute it. The sim-vector machinery is the reconstructive
rule for the *dynamic* layer, exactly as worldgen is for the *procedural* layer.

So: it plugs into world history loading (it *is* the load-a-tick engine), it
becomes part of world storage (as sparse checkpoints + a tick-stamped edit log +
committed plans, not snapshots), and "timebenders load any time from the menu" is
a deterministic recompute of T — reusing the load path, bounded by convergence,
presented as the scrub animation.

---

## 6. Status + sequence

- **Built + callable now:** the load-a-tick engine's decision/data layer
  (`convergence`, `sim_scheme`, `region_digest`, `sim_vector`).
- **Gated on Step 0 (dynamic systems running):** the R1 fill that steps a vector,
  and the coarse-state checkpoints (you can't checkpoint dynamic state that isn't
  simulated yet).
- **The storage diff (§4) is the bridge** — small, additive, version-gated; do it
  alongside Step 1 (the shared coarse field, which is also the far-mesh fix and the
  natural place coarse-state checkpoints come from).
- **Invariant:** the world menu loads a *computation of tick T*, never a stored
  snapshot — so it stays deterministic, reconstructable, and small on disk. Add the
  time axis (tick-stamped edits + sparse checkpoints); never the time *bulk*.
