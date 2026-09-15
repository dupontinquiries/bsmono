# TODO — bscale

Monorepo convention: every item marked **verified** (ran it and saw the
result), **read** (read the code, didn't run it), or **assumed** (neither —
treat as a guess).

## GUI: ring sizing was never applied on the single-job path — fixed

**verified** (read `pipeline.rs`, `job.rs`, `budget.rs`, `app.rs`,
`main.rs`; builds clean; 152+15 existing d2x-core/d2x-job lib tests still
pass): the GUI's single-pipeline path (`run_job` in `app.rs`, the normal
"press start" flow, NOT `--jobs > 1`) called `d2x_core::pipeline::run_tiered`
directly with the raw, un-adjusted `job.pipeline` — `mem_plan`/`apply_plan`
(the RAM-aware ring sizing) was never called on this path at all. The
refs=3/shot-refs=4/batch=4 shown in the settings panel were not a display
artifact for this path — they were exactly what ran, regardless of
available RAM. (The `--jobs > 1` path and the CLI's `job::run`/
`run_segmented` DID already apply it correctly, on an internal copy of the
config the GUI's sliders were never bound to — that half of the original
diagnosis was right, the single-job GUI half was wrong until this fix.)

Fixed:
- `run_job` now calls `mem_plan`/`apply_plan` before opening the sink, same
  as every other path.
- A new structured `d2x_job::Event::MemoryPlan(MemPlan)` variant carries the
  real numbers out (previously only a formatted `Event::Stage` string
  existed, fine for a log line, useless for a UI to read as numbers).
  Threaded through `Msg::MemoryPlan` to `Shell::running_plan`, displayed as
  "last run actually used: ..." next to the override sliders — replacing a
  caption that asserted "ring depths ... follow the memory budget ... unless
  set on the CLI", which was aspirational, not true, for this exact path.
- Downstream consumers of `d2x_job::Event` needed the new match arm too:
  `main.rs`'s CLI printer and **`apps/studio/crates/studio-app/src/
  upscale.rs`** (studio's own beatspace-style upscale integration) both had
  non-exhaustive matches once `MemoryPlan` was added, deliberately NOT
  wildcarded in studio's case (see that file's own comment on why) — both
  fixed, both rebuilt clean.

- [ ] **Not verified**: I have not watched this run live against real
      footage or a real GPU in this environment (no display, no GPU here).
      Code-correct by direct reading and a clean build; not eyeballed.

## GUI: model-folder infinite rescan loop — fixed

**verified** (read `app.rs`; builds clean): the auto-rescan trigger was
`self.catalogue.is_empty() && !self.scanning && self.scan_rx.is_none()`,
checked every UI frame. `drain_scan` sets `self.catalogue = found`
unconditionally, including when `found` is an empty `Vec` (empty, missing,
or briefly-inaccessible models folder). So: scan completes with zero
results -> catalogue is empty again -> next frame's check is true again ->
new scan thread spawned -> repeat, roughly once per frame (~60/sec) --
matches the reported "rescanning really fast and spamming errors on
startup" exactly.

Fixed: added `scanned_once: bool`, set `true` in `drain_scan` after ANY
result (including empty), and changed the auto-trigger to key off
`!scanned_once` instead of `catalogue.is_empty()`. The manual "Scan" button
is unaffected (it always called `rescan_models()` directly, never gated by
this condition).

- [ ] **Not verified live**: same caveat as above — reasoned through
      carefully and compiles, but the actual infinite-loop-stopping has not
      been watched happen in a running GUI in this environment.

## Reuse / segmentation findings from a real diagnostic session

Real footage: shaky handheld 4K with focus racking, AND a SMITE (game)
capture through a preshow -> arena-load -> gameplay sequence. Both run
through `analyze`/real jobs by the user, not synthetic test clips — the
first genuinely field-tested read on this pipeline's behavior outside its
own synthetic test clips and the two clips (`Lone Ranger`, `SMITE`) already
in `docs/REUSE_FINDINGS.md`.

- **verified (by the user's own screenshots + code reading, not by us
  running their footage)**: a hard cut (map load) is a wall of full-network
  redraw, exactly as expected — the reference ring has never seen the new
  content, there is nothing to reuse yet. Recovers over the following
  frames as shot-start references accumulate. A HUD/scoreboard fixed in
  screen position still gets partial block-level reuse even while the 3D
  background behind it doesn't — the per-block granularity earning its
  keep, not an all-or-nothing frame decision.
- **read, grounded in `docs/REUSE_FINDINGS.md`'s existing SMITE numbers,
  NOT re-verified on the user's specific clip**: raising reference-ring
  depth is very unlikely to be the right lever for a continuous 3D camera
  (SMITE's own documented finding: "SMITE unchanged -- a 3D camera repeats
  nothing exactly"). What already-measured, documented flags DID move
  SMITE's redraw fraction a lot: `--halfpel --quarterpel` (95.9% -> 54.5%)
  plus `--anchor-max-age 5` (-> 31.4%). This is the concrete next test on
  the user's own SMITE capture, not yet run: **not done**.
- [ ] Run `analyze --halfpel --quarterpel --anchor-max-age 5` on the user's
      actual arena/map-load segment and compare against their plain
      `analyze` baseline. Nothing here is verified until that number
      exists.
- **On noise/texture/effects segmentation** (user's idea, explored and
  set aside for now, not rejected outright): genuinely harder than it
  looks, for a specific, non-hand-wavy reason -- real high-frequency
  texture and sensor/compression noise are spectrally similar; the actual
  distinguishing signal is TEMPORAL coherence (real texture moves
  consistently with motion compensation, noise doesn't), which needs
  motion-compensated analysis, not a per-frame spatial filter. The existing
  difficulty cascade (luma std-dev: flat/smooth/textured) is already a
  cruder version of the same idea, and its own known cost is directly
  relevant: cross-tier seams (`seams.rs`'s documented "fault 2" -- a level
  mismatch between two different networks' output statistics, which
  feathering smooths into a soft band but does not actually correct). The
  README's own fix for this is threshold tuning (`--smooth-std`), not a
  structural cure. Any finer-grained content segmentation multiplies the
  number of tier boundaries this same unfixed fault can appear at.
  **Recommended before building anything new**: run `analyze --cascade
  default --flat-bypass` on the problem segment and compare
  `heavy_pixel_fraction` (main-network share, already reported, already
  named in the README) against the plain redraw fraction -- tells you
  empirically whether the existing blunt segmentation already captures
  most of the win, before scoping a finer one. **Not done.**
