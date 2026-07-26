# tomescope — partial drop, does not build yet

This is the format/data layer of the tomescope reader, landed exactly as
received. **The crate cannot compile in this state**, and that is a property of
the drop, not of the merge — three files it references were not included.

## What landed (7 files, verbatim)

| File | What it is |
| --- | --- |
| `Cargo.toml` | package + deps + two `[[bin]]` targets + 2 features |
| `src/content.rs` | the neutral book model: `Block`/`Span`/`Locus`, search, `word_at` |
| `src/formats.rs` | sniffing + dispatch, TXT/Markdown parsers, the mini-HTML walker |
| `src/formats_epub.rs` | EPUB 2/3: container → OPF → spine → nav doc / NCX TOC |
| `src/formats_pdf.rs` | PDF via lopdf: text lane, page labels, outline, optional raster |
| `src/formats_cbz.rs` | CBZ (natural sort, ComicInfo, manga RTL) + FB2 |
| `src/library.rs` | records, shelves (manual + smart), sort, fuzzy view, texture cache |

Ten `#[cfg(test)]` unit tests came with it, across `content`, `formats`,
`formats_cbz` and `library`.

## What is missing

| Missing file | Why it is required |
| --- | --- |
| `src/main.rs` | Declared as `[[bin]] name = "tomescope"`. **Also the crate root** — with no `lib.rs`, this is where `mod content; mod formats; mod formats_cbz; mod formats_epub; mod formats_pdf; mod library; mod util;` must live. Without it *none* of the six `crate::…` paths resolve and nothing compiles. |
| `src/util.rs` | `library.rs` calls `crate::util::now_unix()` when stamping `added_unix` on a new `BookRecord`. |
| `src/bin/shelfctl.rs` | Declared as `[[bin]] name = "shelfctl"`. |

Verified mechanically: every `crate::` path in the landed sources resolves to
one of `content`, `formats`, `formats_cbz`, `formats_epub`, `formats_pdf`,
`util` — and `util` has no file.

Nothing was fabricated to paper over this. A stub `main.rs` would make the
crate build while silently replacing the app the description promises (the
Cargo description lists highlights, margin notes, flashcards/SRS, citations,
per-book glossary, reading stats, OPDS import and a TTS hook — none of which
are in the landed files, and all of which are UI-layer work). Better to have
the gap visible than hidden behind a placeholder.

## What was checked and is fine

- All three path deps resolve from `apps/tomescope`: `shared/uikit/crates/uikit-egui`,
  `shared/uikit/crates/snapclone-theme`, `shared/settings-kit`.
- egui/eframe pinned at `0.34`, which **matches** `uikit-egui`'s `0.34`. (Note
  `games/blackstar/rust/assetkit` is still on 0.29 — see `3bf7f21`. tomescope
  does not inherit that problem.)
- `uikit_egui::fuzzy::score(needle, hay) -> Option<i32>` exists and is publicly
  exported at `src/fuzzy.rs`; `library.rs`'s call site matches that signature.
- The crate is standalone — not a member of any workspace and not registered in
  `manifest/projects.toml` — so an incomplete crate here cannot break siblings.
  It is deliberately **not** registered with the orchestrator until it builds.

## Not verified

No `cargo check` was possible: this host has cargo 1.75 and the fleet's egui
0.34 stack needs ≥1.85 (see `maintenance/MERGE_C4_245.md`). So even once the
three files arrive, the first real compile is a hardware step.
