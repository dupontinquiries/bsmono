# HANDOFF — hh-r332-t2 record-cost wiring (PROPOSE, do not apply in-sandbox)

**WID:** `hh-r332-t2` · **Pure deliverables:** `sim::record_zones`, `sim::draw_batch`
Target: the steady `record` pass cost (r331 M2: p50 4.0 / p90 8.3 ms, 100 % of frames > 1 ms).
Per SHARED_RULES §2 the worker does not touch `render/world_renderer.rs` (glfw/egui path); the
coordinator applies + validates on-device. Anchors are file:line in the r330b tree the worker
built against — re-confirm against the r332 base on merge (see MANIFEST honesty).

---

## Piece 1 — `sim::record_zones`: attribute the 5 ms (instrument only)

Goal: the next M2 run shows **which** slice of `record` owns the time
(`record_chunks / record_entities / record_overlay / record_ui`). This is pure instrumentation
— no behaviour change.

`RecordZoneFrame` is a `Copy`, alloc-free per-frame accumulator. The bin times each slice
(reusing the existing `CpuTimer`/`Instant` pattern), `add`s the elapsed ms, then forwards the
four `zone_samples()` into the existing `crate::profiler::Profiler` (lib.rs:90) so its
histogram/percentile/`dump()` machinery works per sub-zone — `dump()` already orders zones by
p99 desc, so the worst record slice surfaces first.

Proposed insertion points in the frame record loop (`render/world_renderer.rs`):

```rust
// once per frame, before recording draws:
let mut rec = sim::record_zones::RecordZoneFrame::new();

// (a) chunk draws — around the indirect-count path + per-chunk fallback
//     (~L4830 indirect setup, the cmd_draw_indexed_indirect_count at ~4964/5372/5442,
//      and the per-chunk cmd_draw_indexed fallback at ~5016/5494):
let t = std::time::Instant::now();
/* ...existing chunk record... */
rec.add(RecordZone::Chunks, t.elapsed().as_secs_f64() * 1e3);

// (b) entity instance draws (mobs/dragons/dropped items) — wrap that loop:
rec.add(RecordZone::Entities, /* elapsed ms */);

// (c) foliage/decoration/particle overlay instances:
rec.add(RecordZone::Overlay, /* elapsed ms */);

// (d) egui / HUD / debug-overlay record (ui-overlay path):
rec.add(RecordZone::Ui, /* elapsed ms */);

// end of frame — feed the existing profiler:
for (name, ms) in rec.zone_samples() {
    self.profiler.record(name, ms); // zone names: record_chunks/entities/overlay/ui
}
```

`rec.dominant()` / `rec.fraction(zone)` give a cheap per-frame read for a live HUD line if
wanted. Samples are NaN/negative-sanitized, so a missing timer can never poison the totals.

## Piece 2 — `sim::draw_batch`: collapse per-instance record calls

Goal: stop issuing one `cmd_draw_indexed(.., instance_count = 1, ..)` per instance. The chunk
path is already GPU-driven indirect (`cmd_draw_indexed_indirect_count`, ~L4964) — **leave it**.
This targets the **per-instance** record loops (the `draw1` fallback at ~L5011-5016 and the
draw loop at ~L5494, plus any mob/foliage/overlay instance loops), which is where the
per-instance CPU tax lives.

Wiring:
1. Build `Vec<DrawItem>` for the frame's instanced draws — `pipeline` = the
   `crate::pipeline_key::encode(..)` u64 for that instance's state (already the `pipeline: u64`
   that `draw_key` consumes), `mesh_id` = its mesh slot, `instance` = its instance-buffer slot.
   If the caller already sorted front-to-back via `draw_key`, push in that order — grouping is
   stable and preserves it.
2. `let groups = sim::draw_batch::group(&items);`
3. Gather the instance buffer so each group's `instances` are **contiguous**, then emit one
   call per group:
   ```rust
   for g in &groups {
       ctx.device.cmd_draw_indexed(
           cmd,
           mesh_index_count(g.mesh_id),
           g.instance_count(),          // was 1 per instance
           first_index(g.mesh_id),
           vert_base(g.mesh_id),
           first_instance_of(g),        // base slot of this group's contiguous run
       );
   }
   ```
4. `sim::draw_batch::savings(&items)` gives before/after call counts for a log line / the
   `record_*` profiler zones, to confirm the win on the next run.

**Render-equivalence:** collapsing same-`(pipeline, mesh_id)` instances into one instanced call
is identical output for opaque instanced geometry — same triangles, fewer record calls. No
shader change (the off-limits `chunk.frag`/`froxel_fill.comp` are untouched), no change to draw
order across distinct pipeline states (groups stay in first-appearance / front-to-back order).

**Interactions:** complements the coordinator's `frame_correlation` attribution; if T1's mesh
budget lands, the two are independent (T1 = build admission, T2 = record-call count).
