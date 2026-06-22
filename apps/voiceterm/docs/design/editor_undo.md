# Editor undo/redo — design & reasoning (v0.1.692, pre-implementation)

User: "advanced undo/redo with voice and visual stepbacks and smart
separation is super important... do some research and thinking + reasoning
before implementing." This doc reasons it through; only the SMART-SEPARATION
core is implemented this round (tested), the rest awaits sign-off.

## The three sub-asks
1. SMART SEPARATION — undo shouldn't step one keystroke at a time, nor jump
   too far. It should land on meaningful boundaries.
2. VOICE stepbacks — "undo that", "undo the last paragraph", "redo".
3. VISUAL stepbacks — a timeline/history the user can scrub, seeing each
   prior state before committing to it.

## Model choice: snapshots vs operations vs command-pattern
- Native TextEdit undo is per-widget and our per-block editor SWAPS the
  editable widget as the caret moves between blocks — so native undo loses
  history across block switches (this is WHY we need our own).
- Operational (insert/delete deltas): compact, enables true OT/merge, but
  fiddly to invert correctly with our splice model and multi-cursor-free
  single-user case doesn't need it.
- **Snapshot of docText + cursor** per undo entry: dead simple, robust, and
  notes are small (KBs). A 200-entry ring of full-doc strings is trivial
  memory. Inverting = restore the snapshot. CHOSEN: snapshots, because
  robustness > compactness here and it composes with the block model (we
  already round-trip docText through segment/splice).
- Refinement: store (docText, cursor, label) where label is a human/voice-
  addressable description ("typed 'hello'", "deleted line", "pasted").

## Smart separation (the IMPLEMENTED core: undo_coalesce)
When does a new keystroke START a new undo entry vs extend the current one?
Rules (border = commit a boundary BEFORE applying the new change):
- TIME: a pause >~700ms since the last edit → boundary (you stopped, that's
  a thought unit).
- KIND CHANGE: switching between inserting word chars, inserting whitespace/
  newline, and deleting → boundary (typing then deleting are separate acts).
- NEWLINE / STRUCTURE: pressing Enter (new block/line) → boundary.
- WORD: a space/punct after word chars closes the word → boundary (so undo
  removes a word at a time, not a letter).
- SIZE: a single edit that changes > ~80 chars (paste) → its own boundary
  on both sides.
These give "undo a word / a line / a paste / a pause-delimited burst" — the
meaningful granularity. Pure fn: edit_kind(prev, next) + should_break(...).

## Voice stepbacks (design, not yet built)
- "undo" / "redo" → pop/repush the ring (Intent::Undo/Redo).
- "undo that" → same as undo (one boundary).
- "undo the last paragraph/line/word" → undo boundaries until the label kind
  matches (we stored labels), or until N structural boundaries.
- Implementation: a new Intent variant pair + dispatch to backend
  editor_undo()/editor_redo()/editor_undo_until(kind). Reuses the ring.

## Visual stepbacks (design, not yet built)
- A slim timeline rail (right edge of the editor) with a tick per boundary,
  newest at bottom. Hovering/scrubbing a tick shows that snapshot in the
  rendered (read-only) blend; releasing commits (truncates redo) or cancels.
- Because entries are full snapshots, preview = render that docText; no
  replay needed. Cheap + exact.
- Labels annotate ticks ("¶ paragraph", "✎ word", "⌦ delete", "📋 paste").

## Proposed backend surface (for sign-off)
- editor_history_push(doc, cursor, kind) — with internal coalescing.
- editor_undo() -> "doc\tcursor" | "" ; editor_redo() likewise.
- editor_undo_until(kind) -> "doc\tcursor".
- editor_history_ticks() -> wire of labels for the visual rail.
Stored in a backend ring (Vec + index), cap ~300 entries.

## Status (v0.1.693)
BUILT + WIRED: undo_coalesce.rs (boundaries) + undo_stack.rs — edit_kind + should_break (pure, tested).
This is the hard/subtle part and the foundation. Ring + qt_methods + voice
intents + the visual rail are designed above and await your go-ahead.


## v0.1.693 update — BUILT + WIRED + voice
undo_stack.rs History (snapshot ring on undo_coalesce, 9 tests): record() with
coalescing, record_voice() (the user's idea — a dictated phrase is ONE undo
unit, labeled "🎤 <utterance>"), undo()/redo() (truncate redo tail on new
edit, cap 300), ticks_wire() for the visual rail. qt_methods editor_undo_init/
_record/_record_voice/_undo/_redo/_undo_ticks. QML: init on present, record on
edit (detects the inserted char), Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y.
VOICE CUES: record_voice is the hook — when a dictation inserts into the
editor, call editor_undo_record_voice(doc, cursor, utterance) so it undoes as
one labeled unit. (Wiring the editor's voice-dictation path to call it, plus
"undo"/"redo" voice intents and the visual timeline rail, are the remaining
follow-ups — ticks_wire is ready for the rail.)
