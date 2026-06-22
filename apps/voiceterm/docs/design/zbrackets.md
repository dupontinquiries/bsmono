# ZBrackets — Zettelkasten note linking (v0.1.667)

From a precursor project. Write `[[name]]` in a note to link another note.

## Semantics (src/zbrackets.rs, 9 tests)
- `[[Name]]` → `<a href="REL/Name.md">Name</a>` — .md default, extension
  hidden in display, href resolved RELATIVE to the current note.
- `[[Name.pdf]]` → links the PDF, keeps the extension shown (ereader /
  old-device compat — the requested feature).
- `[[.sub/Deep]]` → smart: resolve, show only the basename ("Deep").
- `[[,sub/Name]]` → show the folder in the display text.
- `[[-Name]]` → escaped: passthrough literal `[[Name]]` (no link).
- `[[12]]` → numeric & short → citation, passthrough `[[12]]`.
- bare/unresolved `Name` → links to `Name.md` under the current dir (so a
  new note can be created on click later).

## Path resolution
`path_difference(from, to)` ports the precursor's algorithm; validated
against the exact fixtures from its source comments (e.g.
.../impakt/reVos.md → .../Go.md = "../Go.md").

## Wiring (v0.1.667)
- qt_method `render_note_zbrackets(markdown, current_path, paths)` builds the
  filename→path index from the notes layer's known paths ('\n'-joined) and
  returns the markdown with links resolved. Compile-verified.
- The rendered surface (a notes/doc pane using TextEdit RichText) calls this
  and routes `onLinkActivated(href)` to open the target (md → note view, pdf
  → doc pane). That render surface isn't built yet — the resolver +
  qt_method are ready for it. Honest: no QML notes-pane exists to host this
  yet, so the end-to-end click flow is build/UI-gated; the resolver is fully
  tested and the entry point is live.

## Future (noted in the precursor)
Fenced-code-block detection (don't rewrite `[[x]]` inside ``` ```), merged-
document export with intra-page anchors, kzn:: intrinsic-file prefix.
