# BUILD SESSION CHECKLIST — pending wiring (as of v0.1.596)

Everything here has a **verified pure core already in the tree** but
needs the running build to wire into the live surface (Backend FFI / QML
/ PTY), which can't be compiled or tested in the dev sandbox. This is
the one-sitting punch list: each item is small, has file:line anchors,
and a "done when" check. Ordered by isolation + risk (safest, most
self-contained first).

After EACH item: `cargo build --release` should still succeed before
moving on. If one fails to compile, the error names exactly what's off —
fix or revert that item alone; they're independent.

---

## 0. First, confirm the prior pure-core fixes compile (no wiring)
These already shipped as source; just verify they build clean on real
hardware, since the sandbox couldn't:
- [ ] **workflow stateful API** (v0.1.590) — `mod workflow` is declared
      once; the 9 fns (start_recording … create_from_commands) resolve.
- [ ] **KEYS sentinel + _narrow fixes** (v0.1.592) — app loads, open a
      large web page (e.g. a long article); confirm NO "_narrow is not
      defined" flood in stderr, and a voice "search for X" in helix
      sends the search (not a literal `KEYSX` command).
*Done when:* release build succeeds and those two manual checks pass.

---

## 1. click_positional execution (LOW risk — mirrors existing click_link)
Core: `Intent::ClickPositional{ordinal,kind}` + parser DONE/tested
(v0.1.596). Recognizer works; just needs dispatch.
- [ ] **QML** (qml/main.qml ~16671, beside `case "click_link":`): add
      `case "click_positional":` — param holds `kind`, the 3rd tuple
      field holds `ordinal` (as string; parseInt it).
- [ ] **Backend**: resolve (ordinal, kind). For kind=="link",
      `page_links()` (main.rs ~6937) already returns the list — pick
      index `ordinal-1`, or last when ordinal==-1, then follow it
      (reuse whatever click_link's follow does).
- [ ] Out-of-range guard: ordinal > count → toast "only N links on this
      page", don't silently no-op.
- [ ] (Optional) kind=button/image/field needs element lists beyond
      links — defer unless page model exposes them.
*Done when:* "click the third link" on a link-dense page follows link 3.

## 2. file-open voice intent (LOW–MED — pure resolver DONE + hardened)
Core: `completion::resolve_file_for_voice` + `entity_extract::
resolve_reference` (exact-match boost, v0.1.581) DONE/tested.
- [ ] Add a voice intent / parse path for "open the <file>" / "edit
      <file>" that calls `resolve_file_for_voice(shell_cwd, text, top_n)`.
- [ ] On the terminal/editor surface, act on the top result: shell →
      type the path at cursor; helix → `:open <path><ret>`.
- [ ] Confidence gate: if top score is low or top-2 are close, show the
      candidate picker instead of auto-acting (scores already returned).
*Done when:* "open the readme" opens README.md; "open main" → main.rs.

## 3. recent-fact insertion (MED — needs careful PTY cursor handling)
Core: `fact_extract` (IP/port/path/etc. extraction) DONE/tested.
- [ ] `pty_write_at_cursor(s)` on Backend — write text at the cursor
      WITHOUT triggering track_pty_command's submit-on-Enter (see
      main.rs track_pty_command for the submit-detection state to avoid).
- [ ] Voice "insert the last IP" / "paste that port" → look up the most
      recent fact of that kind → pty_write_at_cursor.
*Done when:* after a command prints an IP, "insert the last IP" types it
at the prompt without executing.

## 4. workflow recorder UI + replay loop (MED–HIGH — most moving parts)
Core: recording/playback/persistence API (v0.1.590) DONE; danger
detection (glob/force/recursive, v0.1.582) DONE/tested.
- [ ] **Record**: a QML record button → backend start/stop_recording;
      capture each BlockTracker submitted command between marks →
      record_command(cmd).
- [ ] **Replay loop**: pump `next_playback_command()` into the PTY, but
      AWAIT each command's completion before sending the next (don't
      blast them — race). Use the block-tracker's command-done signal.
- [ ] **Danger gate**: before replaying, run `dangerous_commands(&cmds)`;
      if non-empty, show a confirm dialog listing the flagged commands +
      reasons. Only proceed on explicit confirm.
*Done when:* record "cd build / cmake .. / make", replay runs all three
in order; a workflow containing "rm -rf x" prompts before replaying.

## 5. render per-row cache (PERF — the frame-drop lever; do LAST)
Core: `row_cache::assemble_rows` + `rows_needing_rerender` (v0.1.595)
DONE/tested. This is the ~53x-on-1-row-change lever (bench v0.1.574).
**Capture a profile FIRST** (`WEB2QML_PROFILE=1`, open helix, scroll) to
confirm the serialize (not QML reflow) is the cost — see
docs/design/render_speed_remaining.md "what to capture".
- [ ] In the grid struct (pty_term.rs): add `row_html_cache: Vec<String>`.
- [ ] `render_html` becomes `&mut self`: call
      `row_cache::rows_needing_rerender(self.take_dirty_rows(),
      view_rows.len(), self.row_html_cache.len())`, re-fill only those
      rows via the existing `render_row_into` (~549), then
      `row_cache::assemble_rows(&self.row_html_cache)` for the body.
- [ ] Verify the off-thread reader-thread caller (the one that owns
      render + publish) still has a `&mut` grid handle there.
- [ ] Keep the `<html><body>…<pre>` wrapper exactly as render_html emits
      it now — the cache only replaces the per-row loop, not the wrapper.
*Done when:* typing in a full-screen TUI no longer re-serializes the
whole grid (profile shows per-frame serialize time drops); output looks
byte-identical to before (assemble_rows is verified to match).
- [ ] Only if profile shows QML *reflow* (not serialize) is the cost:
      escalate to Option 2 (row-keyed QML model) — bigger change, see
      design doc. Don't do this speculatively.

---

## Notes
- Items 1–3 are independent and low-risk; do them in any order.
- Item 4 has the most surface; budget the most time + test the danger
  gate deliberately.
- Item 5 is the perf payoff but gated on a profile capture so you don't
  optimize the wrong layer.
- Every pure core referenced here is already unit-tested (577 + 28 in
  the cargo harness); these steps only add the live glue.
