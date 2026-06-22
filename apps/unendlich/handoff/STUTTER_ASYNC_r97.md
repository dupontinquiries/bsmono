# r97 — stutter ("chug") analysis + the menu fix

## Finding: the async decoupling is ALREADY in place
Tracing the streaming pipeline:
- GENERATION runs on worker threads (chunk_loader: game thread → request channel →
  workers → chunk channel; octree built on the worker).
- MESHING runs on its own worker pool + GPU dispatcher threads (meshing_pool,
  crossbeam channels).
- UPLOADS are time- + count- + bytes-budgeted per frame (UNENDLICH_UPLOAD_BUDGET_MS,
  MAX_UPLOADS_PER_FRAME, UPLOAD_BYTES_BUDGET).
- The chunk_manager is phased (drain → evict → request_visible) with an adaptive
  drain-to-budget.
So "decouple via async" upstream is done — adding more threads there won't help.

## The menu "chug" was self-inflicted by the flythrough
The cinematic camera (r90) orbits the spawn while the menu is up, and the manager's
streaming interest point followed `local_player.view.position` — i.e. the ORBITING
eye. So every menu frame re-walked request_visible and churned gen/upload around a
moving center, even though the spawn is already loaded. That matches the log: during
menu navigation FPS fell 90→27–43 and `loaded` climbed 31→54 with the camera moving.

FIX (r97, bin, compile-on-end): pin the streaming center to `spawn_pos` while
`menu_open || wizard_open`. The camera still orbits visually; nothing new streams.
Expected: the menu-time FPS drop + churn disappear; the orbit runs at full FPS.

## The one-time 2896ms p99 spike — needs on-device profiling
That single 2.9s frame is not steady-state streaming (which is budgeted). It's a
one-time stall (worker warmup / a large allocation / a GPU fence or pipeline stall on
the first big burst). To pinpoint, capture the `cpu_zones` + `mgr_update breakdown`
lines for THAT frame on-device — they attribute the time to wait_fences / record /
request_visible / upload. Fix follows the attribution (e.g. pre-warm workers, cap the
first burst, or pre-allocate). Can't be diagnosed in-sandbox.

## If the menu still chugs after the anchor fix
Next lever: while `menu_open`, also cap the render distance (a smaller sphere is
enough for an orbit of the spawn), or skip request_visible re-walks entirely (freeze
streaming) since the loaded spawn is all the orbit needs.
