# Selection highlight off-by-one in helix

User reports: when selecting lines in helix and pressing `d` to delete,
the selection highlight extends one line BEYOND the actual deleted range.
The deleted text is correct; only the highlight rendering is wrong.

## Hypothesis

Helix in alt-screen mode renders its own selection inversion as part of
the screen content. We send the bytes through, vte parses them, the grid
renders them. The selection extends one row too many — most likely
because:

1. Our render_html in `term_grid.rs` emits one extra blank row at the
   bottom of the highlighted region, OR
2. Our row-count computation (`r = floor((height - 4 - charH) / charH)`)
   is including a partial row that shouldn't be highlighted, OR
3. The ANSI selection inversion sequence (CSI 7 m) isn't being terminated
   correctly when helix sends the end-of-selection marker.

## Why I'm not fixing it this session

Rendering bugs in terminal emulators are notoriously easy to make worse
without interactive testing. Our test harness can't see "the highlight
shows one extra line" — it would only catch a wrong byte stream. The
fix needs to be tested with an actual helix running; that requires
device/screen access I don't have.

## What to do next session

1. Reproduce in a controlled way: open a file in helix, select 3 lines
   with `V`, observe the selection. Should highlight exactly 3 lines,
   not 4.
2. Capture the exact byte sequence helix sends for the selection. Either
   via `script(1)` or by enabling our PTY tap dump.
3. Compare against what we render. Look for:
   - Trailing CSI 7 m without a CSI 27 m terminator
   - Extra empty row in render_html output
   - Off-by-one in the grid coordinate range
4. Verify Konsole renders the same byte sequence correctly (sanity check
   that the bytes are well-formed and our renderer is the bug).

## Workaround for now

User can press `d` and ignore the cosmetic highlight extension — the
delete itself is correct.

## v0.1.565 update — capture facility now exists + parser ruled out

The tap dump the "what to do next" section asked for is now BUILT. To
get the evidence:
  WEB2QML_TAP=1 cargo r --release --features "voice claude llamacpp"
then select 3 lines in helix with V and read /tmp/web2qml-tap.log. Each
raw burst is logged as escaped ASCII (ESC shows as \e, control bytes as
\xNN). Find the `\e[7m` (reverse on) ... `\e[27m` (reverse off) span and
count the rows it covers.

IMPORTANT narrowing (checked this session): the SGR parser in
pty_term.rs handles 7 (reverse on, ~line 1381) and 27 (reverse off,
~line 1386) CORRECTLY, and SGR 0 resets attrs. So hypothesis 3 (mis-
terminated inversion) is RULED OUT at the parser level. The off-by-one
is almost certainly render geometry — hypothesis 1 (render_html emits an
extra blank row at the bottom of the highlighted region) or hypothesis 2
(row-count rounding includes a partial row). Once the tap confirms the
byte span is N rows, compare to how many rows render_html inverts.

## v0.1.571 — user evidence + likely root cause + fix shipped

User reported (notes.txt): "the copy selection box is shifted down a
line when the cursor offset is a line too far down... this might
reveal more about the bug". That's the breakthrough piece — the
*cursor* is also off by one. That means both render at the wrong row
because they share the same upstream row value.

### Hypothesis (now backed by code-reading + diagnostic)
At line ~14986 (pre-fix), block tracker captured `cursor_row` AFTER
~100 lines of work inside `track_pty_command`. By that point the
reader thread can have processed the echoed `\r\n` from the Enter we
just wrote — advancing the live cursor to the row BELOW the command.
The block tracker anchored at that advanced row, and the selection
box rendered there. Hence "selection shifted DOWN by one."

### Fix (shipped this version)
Snapshot `cursor_row` at the very top of `track_pty_command` when data
== "\r" (Enter pressed — before any further grid access could race
with the echo). At submit_command time, prefer the early snapshot over
re-reading. The fix also logs `[track] cursor-race observed: early_row=N
row_now=M (delta=K)` whenever the race ACTUALLY happens, so we get
direct confirmation in dogfooding that this hypothesis was right —
absence of those logs means the bug was elsewhere and we may need to
revisit.

## v0.1.611 — selection range math extracted to a pure, TESTED module
The range logic that was inline in term_selection_text (untestable —
tangled with pty.render_row) is now `src/selection.rs`:
- `extract_selection(rows, r1,c1,r2,c2)` — the normalize/clamp/first-
  last-middle-row/inclusive-end logic, operating on rendered row strings.
  term_selection_text now delegates to it (dedup + testable).
- `highlight_row_span(r1,r2,total_rows) -> Option<(usize,usize)>` — the
  geometry the off-by-one is ABOUT: the inclusive row span a selection
  should highlight, CLAMPED to the last real row (total_rows-1), not
  beyond. Test `highlight_span_clamps_end_to_last_row_not_beyond` pins
  exactly hypothesis 2 (a partial/extra row at the bottom): r2 past the
  screen → clamps to last row, never last+1.
- 13 tests incl. "select 3 lines yields exactly 3 lines, not 4" and the
  clamp cases.

### What this de-risks
The arc's blocker was "the harness can't see the highlight shows one
extra line." It still can't see PIXELS — but the RANGE MATH is now
testable, so when wiring the highlight renderer (build-gated), it should
consume highlight_row_span (which is proven to clamp correctly) rather
than recompute the span inline. If the QML highlight still shows an extra
row after using highlight_row_span, the bug is in the QML row-geometry
(charH rounding), not the range math — which narrows it a lot.

### Still needs the running build
The actual highlight RENDERING (QML selection-overlay rectangles, ~4975)
+ confirming via WEB2QML_TAP whether helix's own inversion or our
geometry is the visible extra row. But the range it should use is now a
tested function.

## v0.1.631 — probe-found + fixed: extract_selection overshoot trailing newlines
Adversarial probe of extract_selection (the COPY-text path, wired via
term_selection_text) found two real defects:
- end row past the actual grid (e.g. drag past the last line, r2=9 on a
  3-row grid) iterated sr..=er over nonexistent rows, appending an empty
  string + '\n' each → a pile of trailing newlines in the copied text.
- an empty rows slice returned "\n".
Fix: clamp the row range to the rows that actually exist (rows.is_empty
→ "", sr past last → "", er past last → last_row), mirroring the clamp
highlight_row_span already does for the HIGHLIGHT. Now the copied TEXT and
the highlight GEOMETRY agree at the bottom edge. 3 tests added (overshoot
no-trailing-newline, empty-slice, start-past-grid). 935 pure tests green.
Note: this is the copied-text correctness; the visible HIGHLIGHT extra-row
rendering is still build-gated QML (selection-overlay rectangles ~4975).
