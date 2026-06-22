# Note editor — WYSIWYG blend (v0.1.672)

The surface the notes thread kept pointing at: where ZBrackets come alive
while editing.

## The blend (live preview)
The line the cursor is on shows RAW markdown source (so you can edit
`[[name]]` syntax); every other line RENDERS with ZBrackets as live links.
Move off a line → it re-renders. (Line-level, like Typora/Obsidian's default;
token-level reveal — only the [[token]] under the caret shows source — is a
future refinement.)

## Typing flow (ZBrackets come alive)
1. Type `[[` → backend.zb_editing_context reports in_link + the partial.
2. A completion dropdown shows context-ranked suggestions
   (note_completions): local files + GRAPH NEIGHBORS of the current note +
   history, blended (note_graph.rs).
3. Enter/Tab/click accepts → backend.accept_zb_completion inserts `[[name]]`
   (no doubled brackets if a `]]` already follows) and moves the cursor past
   it. The new link is a new graph edge → informs future adjacency.
4. Esc dismisses the dropdown.

## Clicking a rendered link
classify_note_link(href) routes it: a note → open/new tab, a pdf/epub → doc
pane (ereader path), a url → browser, #anchor → in-page. (No longer falls
through to the teleprompter — the v671 fix.)

## Pure core (src/note_edit.rs, 11 tests)
- zb_cursor_context(text, cursor) → EditContext {in_link, partial,
  link_start}; char-boundary safe; a newline or a closing `]]` breaks the
  context.
- accept_completion(text, cursor, ctx, chosen) → (new_text, new_cursor);
  doesn't double an existing `]]`; preserves surrounding text.
- cursor_line / blend_lines → active-line split for the live preview.

## qt_methods
zb_editing_context, accept_zb_completion (this module) + note_completions,
render_note_zbrackets, classify_note_link (the supporting engines).

## QML (qml/main.qml: noteEditor) — needs the build to verify
A full-screen overlay (noteEditor.open) with a TextEdit holding the raw
source, a completion dropdown bound to the backend context, and done-to-close.
Runtime behavior (rendering, focus, dropdown placement near the caret) is
UI-gated. Current pass is a single editable TextEdit + dropdown; the
per-line rendered/raw blend (ListView of lines, inactive → RichText via
render_note_zbrackets) is the next QML iteration — the backend (blend_lines,
render_note_zbrackets) already supports it.

## Honest status
Backend logic: unit-tested + compile-verified. The QML editor exists and
qml_lint-passes but its rendering/interaction needs the aarch64 build; the
true per-line render blend is staged in the backend but the QML currently
shows raw source in one TextEdit with live autocomplete (the "come alive"
typing half works; the "rendered inactive lines" half is the next QML step).


## v0.1.679: per-block render engine ready
src/note_render.rs (9 tests): render_block(line, current_path, paths) → styled
HTML for ONE inactive line — heading sizing, inline bold/italic/code, bullets,
and ZBracket links (delegated to zbrackets::rewrite so links match the
read-only render). Critically, emphasis styling skips inside <a> tags so
ZBracket anchors aren't mangled. qt_method render_note_block. This is the
engine the WYSIWYG blend needs for inactive lines; the QML ListView (active
line raw TextField, others render_note_block → RichText Text) is the next QML
step — backend now fully ready (blend_lines + render_note_block).
## v0.1.683: blend rules (tested) + [[ candidates fix + indented-code render
Per user feedback (editor opens; wants integrated buffer with live
replacement + subrendering, "solid rules", tests; [[ not firing; md inline
code / off-page-left).
- src/note_blend.rs (9 tests): segment(text, cursor) → blocks
  (Paragraph/Heading/FencedCode/ListItem/Blank), marks the cursor's block
  active (raw). KEY RULE: multi-line constructs (fenced code, grouped list
  items) are ONE block, so editing inside a fence keeps the whole fence raw
  (no per-line flicker). qt_method note_blend_blocks → "start\tend\tkind\t
  active\traw". This replaces the per-line blend_lines for the live render.
- [[ completion candidates: open_note_editor now ALSO reads the current
  note's directory for sibling .md/txt files (merged with the path index),
  so [[ always has local candidates even if the global index is empty/stale
  (the likely cause of "[[ shows nothing").
- Indented code blocks (4-space/tab) now render as code (manpages
  is_indented_code_line + detection in to_qml_with_source) — was rendering
  as mangled, off-page paragraphs for big pasted exports.

### Next QML step (the actual integrated buffer)
A ListView over note_blend_blocks: active block → editable TextEdit (raw);
inactive blocks → render_note_block → RichText. On cursor move, re-segment
and swap which block is raw. Backend is now complete + tested; this is the
remaining QML assembly.


## v0.1.684: block-level renderer pairs with the blend
note_render::render_full_block(raw, kind, current_path, paths) renders a WHOLE
note_blend block by kind: "code" (fences stripped, HTML-escaped, monospace),
"li" (one bullet per item, inline-styled), "h" (heading), "p" (soft lines
joined + inline). qt_method render_blend_block. Now the blend has BOTH halves
tested: note_blend_blocks segments + marks active; render_blend_block renders
each inactive block. The QML ListView delegate: active block → editable
TextEdit (block.raw); inactive → render_blend_block(raw, kind,...) → RichText.
Re-call note_blend_blocks on cursor move to re-mark active. Backend complete.
6 new tests (fence-strip, html-escape, bullets w/ inline, paragraph join,
heading, blank).

## v0.1.685: editor-assembly glue (backend now COMPLETE)
note_blend::line_offset(text, line) and splice_block(text, start, end,
new_block) (4 tests incl. a segment→edit→splice round-trip). qt_methods
editor_line_offset + editor_splice_block. The QML ListView now has every call
it needs:
  1. note_blend_blocks(text, cursor) → blocks (active marked)
  2. render_blend_block(raw, kind, ...) → inactive block HTML
  3. on edit of the active block's TextEdit: editor_splice_block(...) → new
     docText; editor_line_offset(...) + local cursor → global cursor →
     note_blend_blocks again to re-mark active.
No backend pieces remain for the integrated buffer — only the QML delegate
assembly + styling.

## v0.1.689: live per-block blend WIRED in QML (test tomorrow)
The editor body was a single plain TextEdit with no live render ("edit works
but not updating live"). Replaced with a Column+Repeater over _blocks:
- active block (cursor's) → editable TextEdit (raw markdown), focus forced,
  caret restored to _activeLocalCursor.
- every other block → Text(RichText) via render_blend_block, updates live;
  click activates it.
QML functions on noteEditor: _resegment(globalCursor) (note_blend_blocks →
_blocks, marks active), _blockBaseOffset (editor_line_offset),
_renderBlock (render_blend_block), _activateBlock(idx), _spliceActive(blk,
newText, localCursor) (editor_splice_block + updates blk.end by new line
count so successive splices stay correct; re-segment only on block CHANGE,
not per keystroke, to avoid focus theft).
VERIFICATION: backend calls all unit-tested (1198+28); qml_lint clean; but
QML runtime (focus across block switch, caret restore, re-segment timing) is
UNTESTED by me — this is the test-tomorrow item. Known risks to watch:
focus loss when the active block changes; caret jump on splice; blank-block
click target height.

## v0.1.690: editor features 1-4 (caret continuity, accent, highlight, images)
1. Caret continuity: Up on the active block's first line activates the prev
   block at its END; Down on the last line activates the next block at its
   START (_activateAdjacent). Movement flows across block boundaries.
2. Active-block affordance: the editable block sits in a tinted Rectangle
   with a 2px left accent bar (theme ac), so the hot block is obvious.
3. Syntax-highlight overlay (the real technique): the active block stacks a
   colored Text(RichText) BEHIND a TextEdit whose glyphs are transparent —
   same monospace font/size/wrap so they align char-for-char. The TextEdit
   handles all editing/caret/selection; render_active_highlight (Rust,
   highlight_markdown_html — preserves EVERY char) feeds the overlay, updated
   on every textChanged. Colors via HlColors.
4. Inline images + mermaid in rendered blocks: note_render::inline now
   converts ![alt](src) → <img src=file://resolved> (relative srcs resolve
   under the note dir; Qt RichText shows file:// images inline). A ```mermaid
   fence renders a labeled "▦ mermaid diagram" box + the source (RichText
   can't run mermaid; the read-only view still does the real JS render).

### Verified
note_render 21 tests (image/highlight/mermaid: +6), staged w/ zbrackets +
syntax_hl = 46; qt_struct clean (467); compile gate green; qml_lint 8 clean;
harness 1204 + 28. QML RUNTIME still untested by me — watch overlay alignment
(transparent TextEdit vs colored Text), caret continuity edge cases, image
load from file://, accent-rectangle height vs rendered-block rhythm.

## v0.1.691: editor features 1,2,3,5 + niftyplot images
User picked 1,2,3 (+smart sizing +niftyplot images),5 from the v690 menu.
1. Overlay alignment hardening: colored Text + transparent TextEdit share
   exact box model (font, lineHeight:1.0, zeroed top/leftPadding) so glyphs
   register 1:1; noteEditor.overlay toggle falls back to plain-colored
   editing if alignment ever drifts.
2. Block-aware Enter: note_blend::enter_action (4 tests) — continue unordered
   lists, increment ordered markers (3.->4., 1)->2)), preserve indent, empty
   item exits the list (clear_marker). qt_method editor_enter_action; wired in
   the active editor Keys.onPressed (Shift+Enter = plain newline).
3. Smart image sizing: note_render::image_size(total_chars, img_count, dims)
   -> Thumb/Medium/Large/Collage (4 tests): >=4 imgs collage; lone hi-res w/
   little text large; text-heavy medium/thumb. image_size_px maps to width;
   render_full_block_sized/inline_sized/render_image_links_sized thread the
   cap into <img width>. render_blend_block now takes doc_chars + img_count.
   NIFTYPLOT: Node gained image:Option<String>, parsed from
   `A image("p.png"): "label"` (3 tests).
5. Typewriter mode: reuses follow_camera (the clean-room impakt-cam
   critically-damped spring). qt_methods typewriter_target(line,line_h,vh,dt)
   -> scroll-top px (-1=hold) + typewriter_reset. ~60fps QML Timer drives
   _editFlick.contentY; ⌨ header toggle; reset on present (snap not dolly-in).

### Verified
niftyplot 34, note_blend 17, note_render 25 (staged w/ zbrackets+syntax_hl =
50); qt_struct clean (470); compile gate green; qml_lint 8 clean; harness
1213 + 28. QML RUNTIME untested by me — watch: overlay caret alignment,
list-continuation Enter, image render sizes, typewriter line-height (used
~19px for monospace-14; tune if it drifts).