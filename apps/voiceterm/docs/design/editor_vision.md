# GUI editor vision — Mark Text / Typora WYSIWYG + Pulsar code-editor shape

Status: **DIRECTION (v0.1.673), partially built.** Captures user feedback
after seeing the read-only markdown render (SR Latch.md): the *render* works,
but the EDIT-with-live-preview mode (Mark Text / Typora style) is the missing
component, and the GUI editor should be FIRST-CLASS — not a fallback to the
terminal editors.

## Two first-class editing worlds, not a hierarchy
- **Terminal**: helix (first-class), vim, and remote debug sessions over ssh
  — already strong (editor_open.rs routes `open X in helix on host`).
- **GUI**: the in-app editor is ALSO first-class — a Mark Text/Typora-style
  WYSIWYG-blend for notes/markdown AND a Pulsar-shaped code editor with
  syntax highlighting + language support. The newer GUI surface has room for
  tools the terminal can't show (live preview, inline images, the ZBracket
  graph, voice-driven completion).
Neither replaces the other; the user moves fluidly between them.

## What "WYSIWYG blend" means here (Mark Text / Typora)
Not a split source/preview pane. A SINGLE surface where:
- The block/line under the caret shows raw markdown source (edit `#`, `**`,
  `[[name]]` directly).
- Every other block renders: headings big, bold/italic styled, code in a
  mono box, images inline, and ZBrackets as live links.
- Caret leaves a block → it renders; caret enters → it reveals source.
This is the live-preview blend; note_edit.rs has the active-line split, and
render_note_zbrackets renders the inactive ones. STILL TO BUILD in QML: the
per-block rendered/raw ListView (current pass is one editable TextEdit).

## Pulsar code-editor shape (syntax highlighting + language support)
For code (and for markdown's own syntax), the editor needs token-level
highlighting:
- A language-parameterized lexer → typed spans (keyword / string / comment /
  number / type / ident / punct). Per-language specs (rust, python, c, js,
  shell, markdown, toml, …) so "language support" = add a spec.
- Markdown's own highlighting: headings, emphasis, code fences, links, and
  ZBrackets get distinct styling in the source view.
- Built syntax_hl.rs (pure, tested) as this foundation. QML renders the spans
  as colored runs.
Future Pulsar-shaped tools (build-gated): bracket matching, indent guides,
folding (the text_syntax_tree.md chunk tree feeds this), multi-cursor,
minimap, LSP/diagnostics hookup. Catalogued here, not all built.

## Focus / key handling (concrete, requested)
- **Search bar**: ESC unfocuses it AND hides the suggestion dropdown.
- **Editor ZBracket autocomplete**: ESC dismisses the completion dropdown
  (already wired in noteEditor); the editor also needs proper focus handling
  so ESC there doesn't bubble to close the whole pane unless the dropdown is
  already closed.

## Observed bug — FIXED v0.1.674
(was: from the screenshot, logged not yet fixed)
The rendered note showed an image as a camera placeholder + the raw relative
path (`SR Latch.assets/...png`) instead of the image. Relative image paths in
a `file://` markdown render aren't being resolved against the note's dir.
FIXED: manpages::resolve_one_img_src resolves a relative img src against the
note's directory into file://, applied in to_qml_with_source's block-image
emit (the live render path). Absolute/http/data/web2qml srcs pass through.
The screenshot's `SR Latch.assets/...png` now resolves. (Whether the image
then *displays* depends on the QML Image loader + file perms — runtime.)

## Build order
1. syntax_hl.rs highlighter core + qt_method (DONE v0.1.673).
2. ESC focus handling: search bar + editor (DONE v0.1.673, QML — build-gated).
3. Per-block WYSIWYG render blend in QML (next; backend ready).
4. Wire highlighting spans into the editor render (next; core ready).
5. Relative-image-path resolution in the md render (DONE v0.1.674).


## OPEN-HOOK WIRED v0.1.677 (was: STILL NOT WIRED v0.1.675)
The noteEditor pane + all its backend (zb_editing_context, accept_zb_
completion, note_completions, render_note_zbrackets, classify_note_link,
highlight_syntax, blend_lines) exist and are tested, but there is NO host
hook that calls noteEditor.present(...) — no button, no voice intent, no
keybinding — so it's unreachable in the running app. This is the single
blocker to the editor being usable. NEXT: (1) an "edit" affordance on a
rendered .md page / notes list that calls present() with the file text +
path + the known-notes/paths/history for completions; (2) a voice intent
("edit this note"); (3) the per-block render blend so it's WYSIWYG not raw.
WIRED v0.1.677: a pencil ToolBtn (and any caller) → backend.open_note_editor()
reads the current file:// .md, gathers known note paths/names for completions,
and emits open_note_editor_ready → QML Connections → noteEditor.present(). So
the editor is now REACHABLE on a markdown page. Compile-gate + lint clean;
needs the build to confirm it opens + renders. STILL TO DO: a voice intent
("edit this note"); the per-block render blend (currently one editable
TextEdit); wiring highlight_syntax spans + classify_note_link clicks; scroll/
caret-tracking. The door exists now; the room is still being furnished.