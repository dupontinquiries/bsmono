# Codemap / document outline (v0.1.695)

User idea: "add a codemap to the document view maybe as an extra? maybe it
pops open for voice only to help with jumping around?"

## Core (src/doc_outline.rs, 7 tests)
- outline(doc) → [OutlineItem{level, title, line}]: ATX (#..######) +
  setext (===/---) headings; fenced-code blocks as level-0 "‹lang›"
  landmarks; ATX closing hashes stripped; headings inside fences ignored.
- jump_match(query, items) → best index by whole-phrase / word-overlap /
  subsequence (content match REQUIRED — no spurious jumps). For voice
  "jump to <section>".
- outline_to_wire. qt_methods note_outline(doc), outline_jump(doc, query).

## UI (editor, v0.1.695)
- 🗺 header toggle opens a left-edge panel listing the outline, indented by
  level, click to _jumpToLine (re-segments at that line, reframes if
  typewriter on).
- VOICE: when the editor is open, "jump/go/scroll/take me to <section>"
  routes through outline_jump → jump + opens the codemap. (The doc-view —
  read-only markdown — can reuse note_outline the same way; left as a
  follow-up since the editor is where the nav infra lives.)
