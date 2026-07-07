# DEVICE LOG TRIAGE — log-r462 (Asahi/M2 GPU run, ~7.5 min session)

Source: `docs/device_logs/log-r462.log` (8988 lines, single-player "New World", LAN announced).
This is a **living triage doc** — revisit it across sessions. Update status as items are fixed.
No ERROR/PANIC in the log; 69 WARN (68 LONG FRAME + 1 raycast normal). FPS min=2, max=138.

Status legend: 🔴 open · 🟡 investigating/partial · 🟢 fixed (rNNN) · ⚪ wontfix/tuning-only

---

## T1 🟡 [PERF/HITCH] Near-chunk meshing spikes to 88–889 ms — the hitch root cause
Evidence (lifetime mesher summary):
`near n=32131 mean=3.56ms ±21.68 p50=0.16 p90=3.23 p99=88.15 max=889.29`
The 68 "LONG FRAME" WARNs (dt 113–664 ms) and FPS dips to 2 line up with this: a *single near
chunk* occasionally takes 88 ms (p99) up to 889 ms to mesh. 146 frame samples had p99>100ms.
- The per-frame `cpu: … draw=883ms` lines correlate in magnitude with `near max=889ms` → the giant
  "draw" number is almost certainly a meshing spike bleeding into the draw window, not real GPU draw
  (GPU `main=7.4ms`, `total=7.75ms`). So **the draw timer is misattributing a mesh stall** (see T6).
- Hypothesis: a pathological near chunk (huge vert count? greedy-mesh blowup? a lock/contention in
  the octree mesher?) — `max_verts=51175`, `avg_verts=28309` seen in octree-mesher batches.
- INVESTIGATE: `render/meshing_pool.rs` + `render/gpu_meshing_octree.rs` near path; is the 889ms a
  CPU pre-pass (greedy/index build) or a GPU-dispatch wait? Add per-chunk vert/time logging to catch
  the offending chunk. Device-only-verifiable (GPU mesher).
- r463: the 889ms is `dispatch_us / batch.len()` in the octree GPU dispatch — i.e. a GPU DISPATCH STALL
  (whole-batch wall-clock spike), not CPU greedy meshing. Added a `[mesh-stall] octree dispatch Nms for K
  chunks` WARN (meshing_pool.rs, both near-dispatch sites, threshold 100ms) so the NEXT device run names
  the stalling batch + size. Likely an Asahi fence/queue wait or an oversized batch — confirm from the
  new warn whether it's batch size (lower UNENDLICH_GPU_GEN_BATCH) or a per-dispatch driver stall.

## T2 🟢 [ECOLOGY] Runaway population → constant mass-starvation churn — FIXED r463
Evidence: 117 `[popdyn] … starved (herd N > capacity 8)` events; herds reach 21,24,25,26,27,31,34,40
vs **capacity 8**. Affects Fauna, Spaeher, Lauscher. Spawns vastly outrun carrying capacity, so
creatures spawn then immediately starve — wasted spawns, entity churn, log spam, and feeds T1 (more
creatures = more sim/mesh load). The r462 fauna-roster spawn wiring pours INTO this loop.
- Carrying capacity floor is `fg_colony.population().max(8)`; cull triggers only at `len > carrying+12`.
- FIX DIRECTION: gate spawning on capacity (don't spawn when herd ≥ capacity), and/or raise capacity to
  a sane band, and/or slow spawn cadence. CPU-side, tractable. (Started — see below.)

## T3 ⚪→🟡 [LOD] mid/far/vfar LOD never engages — everything renders at near
Evidence: `lod_near=163 lod_mid=0 lod_far=0 lod_vfar=0`; mesher `mid n=699 (all 0ms/empty) far/vfar
(no chunks)`. Config: `UNENDLICH_RENDER_DISTANCE=4`, `UNENDLICH_LOD_BANDS=4,6,8`. The first band
boundary (4) ≥ render distance (4), so no chunk is ever far enough to demote → LOD is effectively
inert and every visible chunk meshes at full near resolution (amplifies T1).
- Likely TUNING not a logic bug: with RD=4 there is nothing beyond the near band. But if LOD is meant
  to relieve load, bands must sit INSIDE render distance (e.g., RD=8, bands 3,5,7) — or near should
  cap lower. CONFIRM whether the band/RD relationship is intended; if LOD should help, reconcile.

## T4 🔴 [RENDER] octree raycast returned non-axis normal
Evidence: 1× `WARN octree raycast returned non-axis normal (x,y,z)`. A voxel raycast should return a
cardinal face normal; a non-axis normal can mis-orient block placement/highlight or lighting at that
hit. Rare (1 in 7.5 min) but a correctness smell in the octree raycast. INVESTIGATE the octree
raycast normal derivation (off-by-one / diagonal-step case).

## T5 🟡 [MP] LAN re-announces world every ~2 s (212×)
Evidence: 212 identical `[lan-mp] announced world 'New World' seed …`, ~28/min for the whole session.
Probably an intended discovery heartbeat, but 2 s cadence is noisy and may be more frequent than
needed. CONFIRM intended interval; consider backing off to ~5–10 s and/or only logging on change.

## T6 🟡 [INSTRUMENTATION] Frame-section timing is unreliable
The `cpu: … draw=883ms` per-window line vs the LONG-FRAME `recent 1s totals: draw=43ms` disagree by
20×, and "draw" magnitude tracks T1's mesh spike. The section timers (esp. `draw`, and the
"sections may have already accumulated some of this" caveat) don't cleanly attribute the hitch.
Fixing attribution (separate mesh-stall time from draw; make the windowed totals consistent) is
prerequisite to trusting any frame-timing triage. Tie to T1.

## OK / no signal this log
- Water physics: runs; `depth@player` varies 0/5/36, `flow@player` ~0 (standing water), `moved` 0–8,
  `active` toggles. No anomaly visible here (watch in a flowing-river capture).
- Uploads: `upload_fails: 0` across 418 samples; `gpu_oom_fallback=0`. Upload path healthy.
- Multi-world: only one world this session (no menu switch), so multi-world bugs aren't exercised here.
  Need a capture that switches worlds via the menu to triage those.

## Suggested order
T2 (tractable, CPU, started) → T1+T6 (biggest user-facing hitch; needs device instrumentation) →
T3 (tuning decision) → T4 (raycast correctness) → T5 (MP noise).
