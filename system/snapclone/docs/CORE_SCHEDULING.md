# Core Scheduling — keeping background work on efficiency cores

Status: **classification + policy shipped + tested** (`snapclone-coresched`);
the per-thread C++ application of the placement is specified here and deferred
(it needs the live threads + syscalls, untestable in this sandbox).

## Why

Heterogeneous CPUs — ARM big.LITTLE on the Fairphone 5, Intel/AMD hybrid on
laptops — have efficiency cores that draw far less power. Linux can steer work
onto them via `sched_setaffinity` (hard pin), `uclamp`/`sched_setattr`
(util-clamp hint so the Energy Aware Scheduler prefers E-cores + low freq), and
`setpriority`/nice. Today the app's background threads run wherever the
scheduler puts them, which on a phone often means waking a big core for a 55-min
DHT re-announce — wasteful. Pinning genuinely-background work to E-cores cuts
idle and near-idle power without hurting the interactive experience.

## Two layers (mirrors the user's "e-cores + schedulers + uclamp" framing)

1. **Classification** (`topology`): parse `/sys/devices/system/cpu/cpuN/
   cpu_capacity` (capacity normalized to 1024; lower = efficiency) with a
   cpufreq max-freq fallback, into an E-core / P-core split. The split uses
   **gap detection**, not a fixed tolerance from the max: it sorts the distinct
   capacities and splits at the largest RELATIVE gap (requiring ≥1.3×). This is
   load-bearing for the FP5, whose QCM6490 is THREE-tier — 4 silver A55 (~335),
   3 gold A78 (~910), 1 prime A78 (1024). A naive "within 5% of max" rule put
   the gold A78s in the EFFICIENCY bucket (910 < 0.95·1024), which would have
   hard-pinned background work onto real big cores AND starved the UI to one
   core. Gap detection splits at the A55→A78 jump (2.7×), not gold→prime (1.13×),
   so all four A78s are "performance" and only the A55s are efficiency. On a
   uniform CPU it reports "not heterogeneous" so callers skip pinning. Tested
   against the real FP5 three-tier layout, clean 4+4 big.LITTLE, Intel hybrid,
   near-uniform (no split on noise), uniform, and the freq fallback.

2. **Policy** (`policy`): map a work CLASS to a placement — mechanism (none /
   nice / uclamp / affinity), CPU set, nice delta, uclamp ceiling.

## Work classes and the app threads in each

| Class                | Placement                          | App threads (examples) |
|----------------------|------------------------------------|------------------------|
| Interactive          | none                               | UI / main thread |
| LatencySensitive     | none (stays on P-cores)            | CDAF focus sweep, VCM control, live-preview transforms — the viewfinder depends on these being fast |
| PeriodicBackground   | uclamp toward E (nice +5, util≤256)| DHT re-announce (55 min), network-change poll (2 min), WebSocket keepalive (30 s), pollinator/beacon discovery |
| BackgroundCompute    | hard-pin E-cores (nice +10)        | ThumbnailService pool (2 threads), AmbientPaletteDeriver, face detection on captured frames, video post-processing |
| BackgroundIo         | hard-pin E-cores (nice +15)        | FirewallPortManager ufw/nft shell-outs |

Rationale per mechanism:
  * **uclamp for periodic work** — it's bursty and infrequent; a low util ceiling
    lets EAS keep it on E-cores at low frequency, but it can still migrate if it
    ever needs to catch up. We don't hard-pin because hard-pinning a rarely-
    waking thread gains little and removes its escape hatch.
  * **Hard affinity for background compute** — thumbnailing/palette/face work is
    sustained CPU; hard-pinning keeps it off the P-cores the UI wants and forces
    the battery-favorable cores. Accepting it runs a bit slower is the whole
    point (the user isn't blocked on it).
  * **nice always** — orthogonal to placement, always safe, and the sole
    fallback on a uniform CPU or a sandbox where affinity/uclamp are denied.

## Graceful degradation

  * Uniform CPU (desktop/server): no E-cores → every background class degrades to
    nice-only; nothing is pinned to a nonexistent set. Interactive stays none.
  * Sandbox / permission denied: the C++ apply step treats EPERM as "skip this
    mechanism, keep nice"; placement is best-effort, never fatal.
  * Non-Linux: `read_from_sysfs` returns uniform → nice-only everywhere.

## What ships now vs deferred

**Now (tested):** `snapclone-coresched` — `topology` (gap-detection
classification incl the real FP5 three-tier QCM6490 layout, 8 tests),
`policy` (placement per class, 7 tests), C FFI (`sc_coresched_has_efficiency_
cores`, `sc_coresched_placement`, 2 tests). 17 tests total. The release
staticlib builds and `include/coresched/snapclone_coresched_ffi.h` passes
`gcc -fsyntax-only` as both C and C++. Top-level `SNAPCLONE_RUST_CORESCHED`
CMake option + build block in place.

**Deferred (needs live threads + syscalls, untestable here):**
  * The C++ `applyPlacement(tid, CPlacement, cpu_ids)` helper that calls
    `sched_setaffinity` / `sched_setattr(SCHED_FLAG_UTIL_CLAMP)` / `setpriority`,
    treating EPERM as a soft skip.
  * Tagging each background thread/pool at creation with its WorkClass and
    calling apply on its TID: ThumbnailService pool workers, the DHT
    reannounce/network-poll context, AmbientCameraDriver workers,
    FirewallPortManager threads. The CDAF sweep is explicitly LEFT on P-cores.
  * Linking `snapclone_rust_coresched_lib` into the consumer in the same change
    (no dead link before a caller exists), same convention as the group FFI.
  * Optional: a settings toggle to disable E-core demotion (for users who'd
    rather spend battery for snappier background sync).
