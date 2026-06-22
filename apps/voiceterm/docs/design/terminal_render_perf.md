# Terminal render perf — helix slower than konsole

Status: **DESIGN / PROFILING PLAN (filed post-v0.1.519).** F5: loading
helix is slower than konsole, and scrolling inside the helix panel
lags vs konsole. The v0.1.490–494 arc decoupled rendering off the UI
thread and added a conservative dirty-rect pass, but explicitly
DEFERRED stage-c (row cache + QML patch). This complaint now justifies
it. Needs on-device profiling to confirm the lever before building.

## What we already did (v0.1.490–494)
- Off-thread render: the reader thread renders HTML into a snapshot;
  the UI Arc::clones it, never locks the grid to render.
- Dirty-rect: precise row range in `print`, CONSERVATIVE (mark-all)
  for scroll/resize/scrollback and CSI clear ops.
- DEFERRED: stage-c = a per-row render cache + patching only changed
  rows into the QML view (vs replacing the whole HTML string).

## Why helix specifically is slow (hypotheses, ranked)
1. **Alt-screen apps mark ALL rows dirty every frame.** helix repaints
   its whole screen on cursor move / scroll, so the conservative
   dirty-rect path gives ZERO win there — every frame is a full-grid
   render + a full QML HTML replace. This is the most likely dominant
   cost and exactly the case stage-c targets.
2. **QML Text/RichText reflow.** Replacing the entire terminal HTML
   string each snapshot forces QML to re-lay-out the whole block;
   konsole uses a GPU glyph atlas and only redraws changed cells. Even
   off-thread rendering doesn't help if the UI-side text reflow is the
   bottleneck.
3. **Snapshot string size.** A full-screen helix (e.g. 50×200) HTML
   string is large; building + marshalling it Rust→QML each frame
   costs, even decoupled.

## Profiling plan (on device, in order)
1. Set WEB2QML_PROFILE=1 (the existing profiler hook) and capture
   frame times while scrolling in helix vs a plain shell vs konsole.
2. Identify whether the cost is (a) the off-thread RENDER (CPU on the
   reader thread — would show as lag in snapshot freshness), or (b)
   the QML-side TEXT REFLOW (UI thread busy laying out the HTML).
3. If (b) — the likely case — implement dirty-rect STAGE-C:
   - keep a per-row rendered cache (render_row_into already exists
     from v0.1.493);
   - on a new snapshot, diff rows vs the cache;
   - patch ONLY changed rows into the QML view (a row-keyed model /
     Repeater of row items) instead of replacing one big HTML string.
   This turns helix's "all rows dirty" into "only the rows that
   actually changed text" at the QML layer.
4. If (a) — optimize the off-thread renderer (it's already CPU-
   threaded; consider skipping unchanged rows in render_row_into via
   the dirty range even in the conservative path).

## Why not just guess + build stage-c now
Stage-c is real work (a QML architecture change: row-keyed model
instead of a single HTML string) and touches the live terminal
surface — the v0.1.490 notes flagged it as needing the running build
to verify it doesn't regress the decouple. Profiling FIRST confirms
the bottleneck is the QML reflow (hypothesis 2/1) and not something
cheaper, so the big change is justified by data, not assumption.

## Reuse / alignment
- render_row_into(out, row, fg, bg) (v0.1.493) is already the per-row
  renderer stage-c needs.
- take_dirty_rows() (v0.1.494) already reports the changed range for
  the non-alt-screen path; stage-c extends the idea to a per-row text
  diff so even "mark-all" frames patch minimally.
- The off-thread snapshot (v0.1.490) stays; stage-c changes only how
  the UI APPLIES a snapshot (patch rows vs replace string).

## Open question
- Is the helix slowness mostly LOAD (first paint) or SCROLL (steady
  state)? User reports both. Load-time may be a separate cost (initial
  full render + QML construct); scroll is the per-frame reflow. Profile
  separates them.

---

## v0.1.574 — benchmarked the lever + built the pure stage-c core

Did the measurement the "profiling plan" called for, but in a sandbox
microbench (rustc -O, std::time) since the GUI isn't available here.
scripts/bench_row_cache.rs models the helix hot path: 50x200 grid, 1
row changes per frame, 10k frames.

RESULT:
  OLD full-grid serialize : 18.05 us/frame
  NEW serialize dirty rows: 0.34 us/frame
  → ~53x for the common case (typing / cursor move: 1-2 rows change)

IMPORTANT correction this produced: my first attempt HASHED every row
each frame and serialized the ones whose hash changed. That was 3x
SLOWER than full-serialize — hashing the whole grid costs more than
serializing it. The win requires never TOUCHING untouched rows, which
means the PARSER must report dirty row indices (it already knows them
in print/scroll/clear). So stage-c is parser-driven, not hash-driven.

BUILT (pure, tested): src/row_cache.rs. The grid feeds it dirty row
indices (mark_row / mark_range / mark_all), and flush() returns a
Repaint:: None | Rows(indices) | Full, with a churn threshold that
falls back to Full when patching almost-everything isn't worth it. 11
tests, in the pure-check set.

STILL PENDING (live surface — needs the running build):
1. In pty_term: replace the single `dirty: bool` (v0.1.488) with a
   RowCache; have print() mark_row(cursor_row), scroll/clear mark_all
   or mark_range, resize set_rows.
2. render layer: on flush()==Rows(idx), serialize only those rows
   (render_row_into exists) and emit a row-keyed snapshot.
3. QML: replace the single-HTML-string terminal with a row-keyed
   Repeater/model so only changed rows re-lay-out (this is the part
   the design doc flagged as needing on-device verification — the
   53x bench now justifies the work).

The risky pure logic (what to repaint, when to fall back to full) is
now proven by bench + tests; only the Qt-side row-patch wiring remains,
and that's where on-device profiling should confirm the QML reflow was
the bottleneck (hypothesis 2) before finalizing the Repeater design.

---

## v0.1.576 — helix-open blank-screen 1-2s + delayed scroll (user re-report)

User on v0.1.575: "opening hx still slow (blank screen for 1-2 seconds
after executing command) and the scrolling feels delayed maybe there is
a spawning thing in the UI that needs to be changed."

### Ruled OUT (checked the code, not the render)
- The `web2qml_pv_*.qml` runtime QML temp files in the log are the WEB
  PAGE view (pv = page view), NOT the terminal. They fire on web nav
  (Apple.com in the log), not on helix open.
- `repl_voice::detect_repl` (the `[repl-glow] REPL detected: helix`
  line) is pure string matching, called on-demand for voice-command
  context — NOT per-render-tick. Not the culprit.

### The capture recipe (instrumentation already exists, just needs the flag)
enter_alt() already has WEB2QML_PROFILE timing (pty_term.rs ~343), and
the read-loop burst timing too (~1735). Run helix WITH the flag:
    WEB2QML_PROFILE=1 cargo r --release --features "voice claude llamacpp"
then open hx and read stderr for:
- `[profile] altscreen:enter NxN took ...` — if this is >100ms, the
  grid clone/alloc is the cost (hypothesis 3, unlikely).
- read-burst timings — if these are large during the blank period, the
  cost is the PARSE FLOOD (helix paints its whole screen on open; we
  parse every byte). 
- If BOTH are small but the screen stays blank 1-2s, the cost is the
  QML-side TEXT REFLOW of the first full-screen snapshot (hypothesis 2)
  — which is exactly what row_cache.rs / stage-c targets.

The 1-2s BLANK (not just slow, but EMPTY) specifically suggests the
first alt-screen frame isn't reaching QML until something synchronous
completes. Next session with the profile capture will localize it. Do
NOT blind-change the render path — the row_cache hashing misstep this
session (3x slower) is the cautionary tale; measure first.

### "delayed scrolling" = the per-frame full-grid cost (stage-c)
This is the case row_cache.rs (v0.1.574) was built for and benchmarked
at 53x. The wiring (parser-driven dirty rows → serialize only changed
rows → QML row-keyed patch) remains the pending live work; the blank-
on-open is a separate startup/first-frame question the profile capture
will separate from the steady-state scroll cost.
