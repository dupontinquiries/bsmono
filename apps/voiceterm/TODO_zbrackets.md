# TODO: ZBrackets note linking

## DONE v0.1.667 — pure resolver + qt_method entry point
src/zbrackets.rs (9 tests): build_index, path_difference (validated on the
precursor's own fixtures), resolve_token (citation/escape/smart/folder/pdf/
standard), rewrite. qt_method render_note_zbrackets(markdown, current_path,
paths) wired + compile-verified.

## TO WIRE (UI-gated)
1. A notes/doc render surface (TextEdit RichText) that calls
   render_note_zbrackets and shows the linked markdown.
2. onLinkActivated(href): .md → open that note (re-render its ZBrackets),
   .pdf → open in the doc pane (ereader/old-device path).
3. Supply the paths index from the notes layer (note_cards/note_clip dir
   scan) — currently the caller passes it; could read ~/.config/web2qml/notes.
4. Fenced-code-block detection (skip [[x]] inside ``` ```), per the
   precursor's note. Needs the Tree-sitter/code-fence pass.
5. Merged-export mode (single doc, intra-page anchors) — precursor feature 1.

## v0.1.668 — reasoned audit of the ported logic (old code was NOT battle-tested)
Treated the precursor as ideas, not a spec (its two versions + prose doc
contradict — e.g. code says `-`=ignore, prose says `[[-x.pdf]]` LINKS a pdf;
the prose's "(without the -)" reads like the dash is a doc artifact). Actions:
- RE-DERIVED all 5 path_difference fixtures from first principles (link
  resolved against FROM's directory) — all correct, so the tests are sound,
  not inherited-on-faith.
- FIXED extension over-detection: precursor would treat any trailing ".word"
  as an extension; "report.final"/"meeting.notes" are dotted TITLES. Now only
  ≤4-char alphanumeric trailing tokens are extensions (md/pdf/txt/png/html/
  jpeg); else default .md.
- FIXED whitespace: "[[ Go ]]" now trims to "Go" (was seeking a file named
  " Go ").
- IMPLEMENTED the code-block guard the precursor only NOTED as needed:
  fenced ``` / ~~~ blocks (incl. language tags) emitted verbatim, and
  single-backtick inline-code spans skipped, so `[[x]]` in code isn't
  rewritten. Line-by-line (links don't span lines) — safer than the
  precursor's whole-doc split.
Confidence: standard link / PDF / numeric-citation = well-supported + tested.
`-`-escape = kept (low-risk, useful). `.`/`,` folder-display prefixes = MY
design choice (the precursor's were contradictory/untested), pinned by tests
so at least the behavior is defined + stable. Double-backtick inline spans
not specially handled (rare in prose) — noted.

## v0.1.670 — context-aware autocomplete engine (search bar + ZBracket linking)
The UX-flow ask: make both auto-complete friendly with historical terms +
adjacent files, blending local dir + web/graph. Built ONE engine, two
consumers (see docs/design/autocomplete_ux.md).
- zbrackets::link_targets — extract a note's resolvable link targets (code-
  fence aware) to build the graph.
- note_graph.rs (10 tests): NoteGraph (out + back links, build from notes),
  adjacency (1-hop=1.0, 2-hop=0.4, backlinks count), match_score (tiered),
  proximity (folder distance up+down), history_boost (recency×freq), rank
  (blended, deduped), to_wire.
- qt_method note_completions(partial, current_note, current_path, files,
  notes, history) → ranked wire. Compile-verified.

### UI-gated (needs the build + a note-editor pane)
- The shared dropdown widget (search bar + note editor), async web-merge
  below instant rows, keyboard/voice selection.
- A note-editor surface to host Flow A (also the ZBracket render gap).
- Possible: cache/incremental graph for large vaults; feed history from
  recall.rs persistence; deeper-than-2-hop centrality (measure first).

## v0.1.672 — note-editor WYSIWYG-blend (the missing surface)
The edit mode where ZBrackets come alive (see docs/design/note_editor.md).
- note_edit.rs (11 tests): zb_cursor_context (open-[[ detection + partial,
  char-boundary safe), accept_completion (insert [[name]], no doubled ]],
  preserves text), cursor_line/blend_lines (active-line split).
- qt_methods: zb_editing_context, accept_zb_completion. (+ the existing
  note_completions / render_note_zbrackets / classify_note_link.)
- QML noteEditor pane: TextEdit raw source + context-aware completion
  dropdown (typing `[[` → ranked suggestions → accept inserts the link).

### Still to do (QML iteration, build-gated)
- The per-LINE render blend: inactive lines as RichText via
  render_note_zbrackets, active line raw. Backend (blend_lines) ready;
  current QML shows one editable TextEdit (typing/autocomplete half works,
  the rendered-inactive-lines half is next).
- Caret-tracking dropdown placement (currently bottom-left).
- onLinkActivated → classify_note_link routing in the rendered lines, then
  open note/doc/url. Host wiring to OPEN the editor (present(...)) from the
  notes list / a voice intent.
- Token-level reveal (only the [[token]] under the caret shows source).

## v0.1.673 — syntax highlighting core + ESC focus handling + editor vision doc
Per user feedback on the rendered SR-Latch.md (render works; WYSIWYG EDIT
mode is the missing piece; ESC + highlighting + language support needed).
- docs/design/editor_vision.md (NEW): GUI editor first-class (Mark Text/
  Typora WYSIWYG + Pulsar shape) alongside terminal helix/vim/remote-debug;
  highlighting + language support; observed image-path bug logged.
- syntax_hl.rs (9 tests): language-parameterized lexer → typed spans
  (kw/ty/str/com/num/ident/punct + md heading/emphasis/code/link). Specs for
  rust/python/c/js/sh/toml; markdown highlighter (fence-aware). qt_method
  highlight_syntax.
- ESC fix: urlBar ESC now hides urlSuggestPopup first, then unfocuses (was
  leaving suggestions up). Editor dropdown already ESC-dismisses (v672).

### STILL TO DO
- Per-block WYSIWYG render blend in QML (the actual Mark Text/Typora view):
  inactive blocks render (headings big, bold/italic, code box, images inline,
  ZBracket links), active block raw. Backend ready (blend_lines, render_note_
  zbrackets, highlight_syntax). This is the core "missing component".
- Wire highlight_syntax spans into the editor's source view as colored runs.
- Relative image-path resolution in the md render (camera-placeholder bug:
  `SR Latch.assets/...png` shown as raw path, not resolved to the note dir).
- Pulsar tools (build-gated): bracket match, indent guides, folding (feeds
  text_syntax_tree.md), minimap, LSP/diagnostics.
