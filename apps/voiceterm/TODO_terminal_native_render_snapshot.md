# Terminal perf: make term_screen_runs snapshot-backed (v0.1.902)

## Orientation (what I checked before touching anything)

The old `render_speed_remaining.md` / `terminal_render_perf.md` design
docs (v0.1.574-629) were about optimizing the single-HTML-string QML
`TextEdit` terminal — full-grid re-serialize, RichText re-parse, the
hidden `termText` selection layer. That entire path was **superseded**,
not finished:

- v0.1.786: opt-in native Canvas renderer (packed styled runs).
- v0.1.788: native is the DEFAULT.
- v0.1.809: the HTML terminal layers were deleted outright.
- v0.1.892: the native path got its own per-row cache
  (`row_pack_cache` / `row_runs_packed_cached`, "dirty-rect arc stage
  c") — so the "patch only changed rows" goal those old docs were
  aiming for is achieved, just via the native Canvas + packed-run
  cache instead of a QML row-keyed model.

So the documented "one remaining lever" (per-row HTML cache) is moot —
that code path doesn't run by default anymore. I went looking for what
the CURRENT default path (native Canvas) still had open, and found one
concrete thing, flagged in two places in the code itself since v0.1.786
and never done:

> "FIRST CUT reads the grid directly under try_lock; once native is the
> default this should be snapshot-backed like render_html so the UI
> thread never touches the grid."

## The bug this left in place

`term_screen_runs()` (in `main.rs`) is called on **every UI poll tick**
— up to 20/sec while a terminal tab is visible (`ptyPoll`, 50ms). It
was `try_lock`ing the grid directly on the UI thread to build the
packed-runs payload:
- Contends with the reader thread for the *same* lock the reader holds
  while parsing an output burst — exactly the moment a fresh frame
  matters most (helix redraw, a fast build log).
- On contention it fell back to the "\u{1c}" sentinel (keep last
  frame) — safe, but means bursts were the most likely time to show a
  **stale** frame, the opposite of what you want.
- Meanwhile `html`/cursor/title have used a proper off-thread
  publish-and-Arc::clone snapshot (`RenderSnapshot` / `latest_snapshot()`)
  since v0.1.490 — the packed-runs payload just never got folded into
  that mechanism when native replaced HTML as the default.

## The fix

1. **`TermGrid::screen_runs_packed(&mut self) -> String`** (new, in
   `pty_term.rs`) — extracted the exact whole-screen assembly (2-char
   header + `\u{1c}` + rows joined by `\u{1d}`, each row via
   `row_runs_packed_cached`) that used to be inlined in `main.rs`'s
   `term_screen_runs`. Single source of truth now.
2. **`RenderSnapshot` gained a `runs: String` field**, built and
   published alongside `html` in both places a snapshot gets built:
   - the reader thread's per-burst render (mirrors the existing
     `html`/native gate: build `runs` when native is on, empty
     otherwise — inverse of the `html` gate).
   - `PtyTerminal::republish()` (used by the main thread after a
     scroll/resize — grid mutations outside the reader's byte path).
     This path was previously **unconditionally** building `html` even
     in native mode (dead work) and never built `runs` at all, so a
     scroll/resize left the native snapshot stale until the next PTY
     output. Fixed to gate the same way as the reader thread.
3. **`term_screen_runs()`** now just reads `pty.latest_snapshot().runs`
   — a cheap `Arc::clone`, no lock. Empty `runs` (no snapshot published
   yet) still returns the `"\u{1c}"` sentinel so the Canvas keeps
   waiting instead of painting blank on the very first frame.

## Verified (sandbox)

No Qt toolchain here, but `pty_term.rs`'s real dependencies
(`portable-pty`, `vte`, `unicode-width`) ARE on crates.io (an allowed
network domain), so unlike the mail-module session I could typecheck
this file for real, deps and all:
- `cargo check` on the actual file: **0 errors** (only expected
  "never constructed/used" warnings from checking it as an isolated
  lib with nothing calling in).
- `cargo test --lib`: **19/19 pass**, including 2 new tests pinning
  `screen_runs_packed`'s wire format against the exact format the old
  inline code produced (header byte count, cursor-visible/alt-screen
  bits, row separator, per-row content) — so the extraction can't have
  silently changed what QML receives.
- `main.rs`'s `term_screen_runs` itself isn't compilable here (needs
  qmetaobject/Qt), but its new body is now ~10 lines calling only
  already-existing, already-typed methods (`latest_snapshot()` existed
  before this change; only the `.runs` field access is new), so the
  risk surface there is small.

## Still open / not attempted

- The `WEB2QML_HTML_TERM=1` fallback path (`render_html`/`term_output`)
  is untouched — it's explicitly a legacy escape hatch, not the
  default, and out of scope for a native-path perf fix.
- Haven't profiled on real hardware (can't — no Qt/GPU here). The
  project's own convention (`BUILD_REPORT.md`, `WEB2QML_PROFILE=1`) is
  to confirm hot-path changes like this on device. Suggested check:
  `WEB2QML_PROFILE=1`, open helix, scroll — the lock-contention
  sentinel case should now never fire during a burst (search stderr /
  add a temporary counter if you want a hard number), and the Canvas
  should never show a stale frame while output is actively streaming.
