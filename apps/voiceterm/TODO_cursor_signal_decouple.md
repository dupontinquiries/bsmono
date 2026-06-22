# TODO: decouple cursor-move notification from html-change (reader-render waste)

## Why (measured)
Profile (v0.1.629 log) on a near-STATIC editor: the generation counter
jumps ~90-135 between actual re-parses. The reader thread renders ~90
full-screen HTML snapshots/sec that get coalesced away — cursor blink /
status redraws mark grid.dirty, reader re-renders + bumps gen each time.
v0.1.628/629 stopped the wasted RE-PARSES (QML side); this is about the
wasted RENDERS (reader side) + the fact that a pure cursor move still
triggers a full RichText re-parse because the cursor is a SEPARATE overlay
(termCursor @qml 5600, positioned by term_cursor_col*charW) NOT in the
html, yet term_cursor_row/col NOTIFY on term_output_changed.

This is a BATTERY/CPU win (background thread), NOT load latency (already
fixed). Lower priority — do only when load feel is confirmed good.

## Done (pure, tested — v0.1.630)
row_cache::classify_change(prev:Option<(&str,i32,i32)>, html, row, col)
-> SnapshotChange { Nothing, CursorOnly, Html }.
- Html if content changed (takes precedence even if cursor also moved).
- CursorOnly if html identical but cursor row/col moved.
- Nothing if fully identical. First frame (prev=None) → Html.
5 tests incl. the cursor-blink case. 932 pure tests green.

## Wiring (build-gated — cursor overlay is HIGH-VISIBILITY, verify on device)
The trap: term_cursor_row/col (main.rs ~555) use `NOTIFY
term_output_changed`. If you skip term_output_changed on identical html,
the cursor FREEZES. Fix = give the cursor its own signal.

1. **New signal**: add `term_cursor_changed: qt_signal!()` to Backend.
   Re-point the two cursor properties:
   `term_cursor_row: qt_property!(i32; NOTIFY term_cursor_changed)` (and
   col). qt_struct_check will confirm the NOTIFY count.
2. **QML cursor overlay** (termCursor ~5600): its x/y bindings read
   backend.term_cursor_col/row — they will now re-evaluate on
   term_cursor_changed instead of term_output_changed. Confirm the
   bindings actually depend on those properties (they do via fallbackX
   ~5649/5703) so the new NOTIFY drives them.
3. **pty_read_screen** (main.rs ~4954): use classify_change against the
   previously-published (html,row,col):
   - Html → assign term_output + term_output_changed() (as today, keep the
     v0.1.628 html-compare + v0.1.629 coalescing) AND update cursor +
     term_cursor_changed().
   - CursorOnly → update term_cursor_row/col + fire ONLY
     term_cursor_changed() (cheap; no re-parse). This is the new win.
   - Nothing → fire neither.
4. **Reader side (optional, bigger win)**: in pty_term.rs ~1818, the
   reader could skip BUMPING generation when render_html() output equals
   the last published html — but it must still publish the new cursor.
   Cleanest: split generation into html_gen + cursor_gen, or publish
   cursor via a cheap separate atomic. Defer this until step 1-3 prove out
   — it's the deeper change and the QML poll already reads cursor each
   tick, so steps 1-3 alone capture most of the re-parse savings.

## Verify on device
Arrow-key around helix: cursor must track smoothly with NO full-screen
flicker. Type: text appears instantly (Html path). Idle: cursor blink (if
enabled) must not trigger re-parse — re-run WEB2QML_PROFILE and confirm
republish lines DON'T fire on pure cursor blink. If cursor lags or
freezes, the binding in step 2 isn't picking up term_cursor_changed.

## Risk note
Multi-point signal rebind on the high-visibility cursor. Do NOT stack on
unconfirmed 628/629. Land after load feel is verified, ideally alone so a
cursor regression is easy to bisect.
