# Terminal render speed — the remaining lever (per-row HTML cache)

Status after a thorough v0.1.594 investigation of the render path.

## What's ALREADY optimized (don't redo — verified by reading the code)

The terminal render is far more optimized than a naive Qt+Rust app:

1. **Off-thread render (v0.1.490, "Option B").** The READER thread
   renders HTML and publishes a finished snapshot; the UI thread reads
   it with a cheap `Arc::clone` and a monotonic generation counter —
   it NEVER locks the grid to render, and never diffs the big HTML
   string. A parsing burst (helix redraw, build log) cannot stall the
   UI tick.
2. **Adaptive poll (ptyPoll, main.qml ~715).** 50ms (20fps) only when
   the terminal tab is visible; 500ms when browsing. Idle browsing
   doesn't pay for terminal renders.
3. **Cheap idle path.** pty_read_screen drains bytes, does a cheap
   `is_dirty()` bool check under the lock (NOT a render), and only
   republishes on an idle main-thread mutation. Output bursts render
   off-thread.
4. **Dirty tracking (v0.1.488/494).** `dirty: bool` and
   `dirty_rows: Option<(usize,usize)>` (an inclusive changed-row range)
   are tracked, with `mark_row_dirty` on the precise print hot path.

## The ONE remaining lever: render_html ignores dirty_rows

`render_html()` rebuilds the ENTIRE grid into one HTML string every
call, via a per-row loop over `render_row_into` (already row-addressable
since v0.1.493 — the comment literally says "foundation for per-row
caching"). But `dirty_rows` is tracked and EXPOSED (`take_dirty_rows`)
and has ZERO consumers. So even though we know exactly which rows
changed, we re-serialize all of them.

Benchmark from v0.1.574 (scripts/bench_row_cache.rs): serialize-only-
dirty-rows vs full-grid was ~53x for the 1-row-change (typing) case.

## The plan (needs the running build — pty_term has portable_pty/vte/
## unicode_width deps, so it can't be sandbox-compiled or tested here)

### Option 1 — per-row HTML cache INSIDE render_html (low QML risk)
Keep the QML contract identical (still emit one full HTML string), but
cache each row's rendered `<span>...` string and only re-render rows in
`dirty_rows`:

```rust
// in the grid struct:
row_html_cache: Vec<String>,   // one per visible row, lazily filled

pub fn render_html(&mut self) -> String {     // note: &mut now
    let dirty = self.dirty_rows.take();        // (lo,hi) or None
    // ensure cache sized to view_rows
    for (i, row) in view_rows.iter().enumerate() {
        let restale = match dirty { Some((lo,hi)) => i>=lo && i<=hi, None => cache_empty };
        if restale { self.row_html_cache[i].clear();
                     render_row_into(&mut self.row_html_cache[i], row, ...); }
    }
    // concat cache[..] with \n separators into the output string
}
```
- Win: on a 1-row change, 1 row re-serialized instead of `rows`.
- Cost: still concatenates all cached rows into one string (the concat
  is cheap vs re-serializing; the v0.1.574 bench measured the
  serialize, not the concat, as the cost).
- Risk: LOW — QML still gets one identical HTML string. The only
  subtlety is `render_html` becomes `&mut self`; check the off-thread
  caller (reader thread) still has a mutable grid handle there (it owns
  the render, so it should).
- TESTABLE caveat: pty_term can't compile in the sandbox. But the cache
  logic could be extracted into a tiny pure helper
  (`fn assemble_rows(cache: &[String]) -> String` + the staleness
  decision) and unit-tested there.

#### v0.1.595 update — the pure building blocks now exist + are tested
Built in `row_cache.rs` (which IS sandbox-testable), ready to drop into
pty_term's render_html:
- `assemble_rows(&[String]) -> String` — joins cached per-row HTML with
  '\n', NO trailing newline (matches render_html ~534 exactly; verified
  by test incl. empty-row-preservation + single-row + capacity).
- `rows_needing_rerender(dirty, total, cache_len) -> Vec<usize>` — the
  staleness decision: resize (cache_len != total) → all rows; else the
  clamped dirty range. Tested incl. the adversarial stale-range-outran-
  a-shrink case (clamps instead of indexing OOB) and resize-forces-full.

So the per-row cache wiring in pty_term is now just: hold a
`Vec<String>` row cache, call `rows_needing_rerender` to know which to
re-fill via the existing `render_row_into`, then `assemble_rows` to
produce the (identical) output string. The decision + assembly logic is
already verified; only the grid plumbing (the &mut self change + the
Vec<String> field) is left, and that's the part that needs the build.

### Option 2 — true row-keyed QML model (bigger win, bigger risk)
Replace the single-HTML-string TextEdit with a row-keyed Repeater/
ListView so QML patches only changed rows (no full re-layout of one big
string). This is the larger frame-drop win for SCROLLING (where many
rows change but the QML reflow of one giant string is the cost). It's a
real QML architecture change — do it ONLY after Option 1, and only with
a profile capture (WEB2QML_PROFILE=1) confirming the reflow (not the
serialize) is the scrolling bottleneck.

## What to capture to choose between them
Run `WEB2QML_PROFILE=1`, open helix, scroll. The existing instrumentation
logs altscreen:enter timing + read-burst timing. If the BLANK on open is
the serialize → Option 1. If scrolling jank persists with small
serialize times → it's the QML reflow → Option 2.

## Animations — investigated, found already careful
359 animation declarations in main.qml. Spot-checked the infinite/
always-running ones: the breathing/recording animations are properly
gated on `running:` conditions, and ptyPoll already adapts its interval.
No obvious always-running-while-hidden jank found. If a specific
animation feels janky, capture which one + when, and it can be gated.

---

## v0.1.607+ CONCRETE FINDING — the hidden full-document RichText layer

Reading the QML (not theorizing) surfaced a specific, addressable cost
that the Option 1/2 framing above didn't name:

qml/main.qml ~4875-4896 has a HIDDEN `TextEdit { id: termText; text:
backend.term_output; textFormat: TextEdit.RichText; opacity: 0.001 }`.
It is NOT the visible terminal — the visible render is the per-row
Repeater over `term_row_html(index)` (~4927). termText exists ONLY as a
selection-source layer. But its `text:` binds to the FULL term_output
HTML, so Qt re-parses the ENTIRE screen through the expensive RichText
engine on EVERY update, for an invisible element. Pure waste each frame
of scrolling / helix repaint — a strong suspect for the residual lag
after v0.1.607's render coalescing.

### Likely safe to cut — but verify (selection is mostly grid-direct now)
Primary selection already goes through `backend.term_selection_text(
r1,c1,r2,c2)` (grid-direct) at main.qml 1325, 5395, 7880, 12903.
`termText.selectedText` is only a FALLBACK (`else if` at ~7884, ~7919,
~1331). So the layer is plausibly vestigial.

### Staged change, LOWEST risk first (do AFTER confirming v0.1.607)
Step 1 — stop the live re-parse without removing the element:
  `text: (backend && termScreen.selActive) ? backend.term_output : termText.text`
  (selActive ~4292). Not selecting → keeps last text, no re-parse.
  Also set `termText.text = backend.term_output` in the selection-start
  handlers (_selecting=true ~5490, ~6218) so the first selection after a
  screen change isn't stale.
Step 2 — if Step 1 confirms the win: remove termText entirely + drop the
  selectedText fallback branches (after grepping that every fallback has
  a term_selection_text branch that fires).

### DISCIPLINE
This is a SECOND unverifiable hot-path change. Do NOT stack it on
v0.1.607 untested — build + confirm coalescing first, THEN apply Step 1,
so any regression isolates to one change. Documented here rather than
blind-edited for exactly that reason.

## v0.1.628 — MEASURED: the cost was needless RichText re-parse, not render
WEB2QML_PROFILE data (helix + pager session) settled the question:
- render:html = 515–785µs even for a full 99×32 screen. Render is FAST.
- altscreen:enter = 9–26µs. Trivial.
- pty:parse = ~2ms for a 105-byte burst (one logged). Fine.
- republish = the smoking gun. On a STATIC helix screen the generation
  counter bumped ~90× in a row over an IDENTICAL 853B payload (cursor
  blink / status redraws set grid.dirty → republish() bumps gen). Each
  bump fired term_output_changed → hidden termText (RichText) re-parsed
  the WHOLE screen for zero visible change. Same at gen 1606–1811 (76×
  identical 2276B).

Fix: pty_read_screen now compares snap.html to the current term_output
and only re-assigns + fires term_output_changed when the HTML actually
CHANGED (a cheap QString compare vs a full RichText relayout). Cursor/
scrollback fields still update unconditionally (separate overlay). Safe
because identical HTML ⇒ identical term_row_html(i) ⇒ visible rows have
nothing to update, and scroll/new-output always change the HTML so the
signal still fires for real changes.

This is the highest-leverage render change available and it's backed by
measurement, not the earlier hypothesis. The "Lever A" (gate termText on
selActive) is now LESS urgent — this guard already stops the re-parse on
static screens; selActive gating would further help only during active
output where the HTML legitimately changes each frame. Defer Lever A
until a post-fix profile shows it's still needed.

## v0.1.629 — burst-coalesce the RichText re-parse for load time
v0.1.628's guard confirmed: static-screen over-fire is GONE (profile shows
gen jumping 12-35 between republishes, no identical-payload runs). But the
new profile exposed the LOAD cost: helix open / pager page = ~20 distinct
large republishes (7-20KB, genuinely changing as syntax highlighting
arrives in waves), each a full RichText re-parse, over ~1s. render:html
spiked to 2.28ms on the open frame too.

Fix: coalesce on the QML poll. gen_delta = how many finished frames since
last poll; >1 = a burst (multiple frames in one 50ms window). While
bursting, defer the re-parse (don't advance term_render_generation, keep
cursor tracking) up to MAX_DEFER=3 polls (~150ms), then force a render so
a continuous stream never starves. Typing = delta 1 = never deferred =
instant. This collapses the ~20 open re-parses into a handful.

UNVERIFIABLE in sandbox (hot path): needs the build to confirm helix opens
faster AND typing stays snappy. Re-run WEB2QML_PROFILE: during a helix
open you should see FEWER republish lines (deferred frames collapsed),
with gen jumps showing the coalescing. If typing feels laggy, lower
MAX_DEFER or require a larger gen_delta to trigger deferral.
