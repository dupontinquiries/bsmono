# Performance reference log

`reference.jsonl` is the **persistent, append-only** record that pairs two kinds of data so a
lavapipe↔hardware correlation can be built empirically over time:

- `source:"hardware"` — **real M2 / Honeykrisp** numbers, mined from run logs (the engine's GPU/CPU/mesher
  lifetime summaries). These are the ground truth.
- `source:"lavapipe"` — the in-sandbox **software-Vulkan** bench kernels from `tests/bench.rs`. A *relative*
  proxy only (see `../BENCH.md`); absolute values are unrelated to real hardware.

One JSON object per line. A test (`tests/perf_reference.rs`) validates every row stays well-formed, so the
table can't silently rot as rows are added.

## Schema

Common: `source` (`"hardware"`|`"lavapipe"`), `date` (`YYYY-MM-DD`), `unit` (`"ms"`|`"us"`), `p50`, and
optional `p90`/`p99`/`max`/`min`/`n`/`note`.

- hardware rows add: `device`, `binary`, optional `run_id`, optional `mesa`, `flags` (object, e.g.
  `{"RENDER_DISTANCE":6,"GPU_MESHER":1}`), and `metric` — one of `gpu.{total,main,shadow,cull,froxel}`,
  `cpu.{record,wait_fences,...}`, `mesher.{near,mid,far,vfar}`.
- lavapipe rows add: `device`, `kernel` (e.g. `octree_sample_32cubed`), `iters`.

## How to add rows

**From your hardware logs** (append a `source:"hardware"` row per metric you care about — paste me the run
summary and I'll convert it, or hand-write following the schema). Keep `run_id`/`flags` so runs stay
comparable.

**From a lavapipe bench run** (Claude, after a kernel change):

```
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
VK_HARNESS_BENCH_LOG=/tmp/bench.jsonl \
  cargo test -p vk-harness --test bench -- --nocapture
# then curate the /tmp/bench.jsonl rows into reference.jsonl with source/date/device added
```

(The bench test writes to a scratch path, not directly into this committed file, so the tree stays clean
until rows are deliberately curated in.)

## What the pairing can and cannot tell you

It **can**, with enough paired points, show whether a lavapipe kernel delta tracks a hardware delta for the
*same* change — useful for catching algorithmic regressions before they reach the M2.

It **cannot** be reduced to a single conversion factor today: the real spikes here are CPU/scheduling
(`cpu.wait_fences` max 130ms, the CPU near-LOD re-mesh path at `mesher.near` p50 21ms with `GPU_MESHER=0`),
not GPU-kernel time. Note the one dramatic, *trustworthy* signal already in the table: turning the GPU
mesher on drops `mesher.near` p50 from **21.43ms → 0.11ms** — that's a real-hardware algorithmic win, the
exact kind of thing the lavapipe proxy is meant to predict. Accumulate paired rows; don't overfit to one.

## Seeded rows (at creation)

Hardware (Honeykrisp Asahi M2 Max):

| date | flags | metric | p50 | p99 | max | n |
|---|---|---|---|---|---|---|
| 2026-05-14 | RD6 mesher=cpu | gpu.total | 2.07 | 4.24 | 12.79 | 7866 |
| 2026-05-14 | RD6 mesher=cpu | gpu.shadow | 1.33 | 2.26 | 11.31 | 7866 |
| 2026-05-14 | RD6 mesher=cpu | mesher.near | 21.43 | 46.00 | 51.88 | 3302 |
| 2026-05-14 | mesher=gpu | gpu.total | 1.21 | 1.59 | 5.31 | 15095 |
| 2026-05-14 | mesher=gpu | mesher.near | 0.11 | 0.45 | 2.14 | 7250 |
| 2026-05-30 | RD2 mesher=gpu2 | gpu.total | 1.67 | 2.76 | 83.22 | — |
| 2026-05-30 | RD2 mesher=gpu2 | mesher.near | 0.06 | 3.69 | 25.49 | 5958 |
| 2026-05-01 | RD8 heavy | gpu.shadow | 113.9 | 224.8 | — | — |

lavapipe (llvmpipe LLVM 20.1.2, µs):

| date | kernel | min | p50 | p99 | max | iters |
|---|---|---|---|---|---|---|
| 2026-05-30 | octree_sample_32cubed | 413.5 | 438.6 | 494.2 | 946.1 | 64 |
| 2026-05-30 | double_u32x4096 | 11.4 | 11.9 | 30.3 | 30.7 | 64 |
