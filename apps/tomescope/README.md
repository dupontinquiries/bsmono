# tomescope

The overkill ebook / PDF / textbook reader for the egui-kit fleet: EPUB, PDF,
CBZ/CBT, FB2, MD, HTML, TXT; library shelves + covers; a pagination engine
with typography controls; highlights, margin notes, flashcards (SRS),
citations, per-book glossary, reading stats + a heatmap, OPDS import, a TTS
hook — all on `uikit-egui`, the same shared layer the rest of the portfolio
ports onto.

## Verification (2026-08-19)

This crate had no build-status record anywhere. A session with a real rustc
1.96 (clears egui 0.34's MSRV) ran it for the first time: `cargo test` is
45/45, spanning every module the description above names — pagination
(`layout`), format parsing (`formats`, `formats_cbz`), the library view and
smart-rule partitioning, dictionary/glossary lookup, OPDS feed parsing, the
reader's tap-zone and RTL handling, spaced-repetition scheduling (SM-2) and
citation formatting, the reading-stats heatmap and streak counting, TTS
sentence splitting, and theme contrast checks.

Compiles clean — the only warnings are two unused imports in `reader.rs`
(`palette_colors`, one more alongside it), not touched here. Not run on a
real display (headless sandbox): compiling and the pure logic are confirmed,
the on-screen reading experience is not.
