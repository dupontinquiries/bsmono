# Tick Authority, Shared Ownership & Rollback-Until-Ready

> See also `dynamic_world_architecture.md` — this regional async-tick + render-last-state machinery is the dirty-propagation half of the compute-vs-vision split (§5).
(captured from operator design notes, r184 — wiring-faithful mapping)

## 1. Perf levers are settings, minimized by authority
All tick/perf knobs (strides, apply budget, render distance, mesh batch)
live in the settings Performance tab with an FPS-target autotuner
(WIRED r184, single-machine). In multiplayer the EFFECTIVE value of each
lever is the MINIMUM across AUTHORITATIVE devices only — the least
powerful *server* sets the floor; clients never count. Rationale: a weak
client must not degrade the shared world's simulation fidelity; a weak
server must, because it owns the truth.

## 2. Shared vs unshared world ownership
Worlds can be owned solo, or ownership SHARED across machines. Shared
ownership buys: (a) parallel tick speedup — regions tick on their owning
machine; (b) network rollback — the rollback scopes (world/rollback_scope
.rs + sim/rollback.rs, ROLLBACK.md) become the reconciliation primitive
when authoritative results arrive late or conflict.

## 3. Blended authority regions ("don't turn off my fast tick")
Authority is REGIONAL and blended, not global: when a visitor steps into
my world but hasn't reached my base, my machine keeps fast-ticking my
base region at MY cadence; their presence only transfers (or blends)
authority for the regions they actually occupy. Region grain: chunk
groups (reuse ChunkManager ownership + per-region stride atomics — the
r184 strided_dyn atomics generalize to a per-region table keyed like the
heat-source centroids).

## 4. Rollback-until-ready (async ticks, pop-free)
Tick compute decouples from the frame: handlers run against a snapshot
on worker threads; the world RENDERS THE ROLLBACK LAST-STATE until the
new state is ready, then results are pulled in and DEFERRED INTO THE
RENDERER (the existing apply-budget machinery is already the deferral
half). Rule: anything with a loading time simply isn't shown until
ready — no half-applied states, no pops. Maps onto: rollback_scope
(snapshot), TickRegistry handlers (pure against ctx), block_edits/
state_changes queues (result transport), apply budget (paced apply).

## Wiring order proposed
1. r184 (done): live levers + autotune, single machine.
2. Async tick worker w/ rollback snapshot (single machine) — removes
   tick teeth entirely; apply stays budgeted.
3. Per-region stride table (single machine; regions = chunk radius
   around bases/POIs).
4. Networked authority: region ownership + min-rule + rollback
   reconciliation (needs the open-world-to-players menu + transport).

## 5. Audio streams ride the rollback (operator note, r187)
Audio streams are HELD while a rollback point is held — decoded stems and
their DSP state stay primed so a restore never cold-starts the sound.
Reverb tails can be REPLAYED AND CACHED for repeat sounds in
psychoacoustically identical environments (same room signature -> reuse
the tail instead of re-convolving). Maps onto: StemGroup (decode-once
loops already persist), the Schroeder state (comb/allpass buffers ARE the
tail — snapshot them with the rollback point), and a future
environment-signature key (biome + enclosure -> reverb preset id).
