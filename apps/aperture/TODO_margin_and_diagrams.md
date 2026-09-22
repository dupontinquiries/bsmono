# TODO -- margin decorations and inline diagrams (C20)

Investigated, not built. Source-checked findings are in
`docs/PULSAR_RECONCILIATION.md`. Order:

1. **Colour swatches in the right margin** beside colour literals. This proves
   the margin mechanism with no sizing question: the lexer already emits spans,
   and a swatch is a filled rect one row tall.
2. **SVG painter** over `egui::Painter` for the subset `nifty-core::svg` emits.
   This is the real blocker for (3).
3. **Inline diagrams from comments.** The size comes from the comment block
   itself. They are painted into the margin with row spans unchanged, and the
   render is cached on `(source_hash, font_px)`, so nothing reflows.
4. **Image paste** -- small and independent; shared with voiceterm.
