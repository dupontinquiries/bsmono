# Worker thread T04 (temporal) — temporal froxel accumulation CPU model

Stream: **temporal**. Doc: `docs/SHADER_ROADMAP.md` → "Reference ↔ engine
(v0.24.x)" → "Next-increment analysis — temporal froxel accumulation". That
analysis says: validate the reproject + EMA math with a CPU model **before** any
shader change. That model is your task.

## Task
Write `handoff/froxel_temporal_model.c` (gcc/clang, `float` to match the GPU,
same spirit as `handoff/froxel_skip_model.c`). Model one froxel column's value
across frames:
- ground-truth per-frame value (reuse the `cell_density`-style signal — a simple
  fbm is fine; the point is the temporal behaviour, not the exact density);
- **reproject** previous frame into current (simulate a camera pan as a UVW shift
  + small resample error) and **EMA blend** `mix(history, current, α)`;
- a **neighbourhood clamp** (clamp history to min/max of current's neighbours);
- a **disocclusion event** mid-run (hard cut) to measure smearing.

Report, for α ∈ {0.05, 0.1, 0.2}: (a) steady-state convergence error vs ground
truth, (b) warmup frames to reach <1 LSB, (c) ghosting magnitude after the cut
with vs without the clamp. End with a one-line **recommended α** and whether the
clamp is needed. Print a small table; no shader edits, no production code edits.

## Validate / return
`gcc -O2 -std=gnu11 handoff/froxel_temporal_model.c -o /tmp/m -lm && /tmp/m`.
Then `bash RETURN_KIT.sh start` (before) / `finish` (after, HARNESS GREEN) →
`CHANGES-T04-R26.patch`. Keep your reply to the present_files call + MANIFEST.
