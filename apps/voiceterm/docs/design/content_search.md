# Content (full-text) search — the "cloak" gap (v0.1.675)

## Problem
Searching "cloak" in the bar returned only "Search the web for cloak" while
`rg cloak | wc` found 159 lines. Root cause: search_suggestions searched
history URLs/titles, bookmarks, and FILENAMES (path_index fuzzy) — never file
CONTENTS.

## Fix (src/note_search.rs, 9 tests + wiring)
- note_search::search(query, docs, max): case-insensitive; multi-word = AND on
  a line; ranks by hit count (diminishing) × whole-query-on-one-line boost ×
  filename-match boost × shallow-path tiebreak; extracts a hit-centered
  snippet. note_name() for display.
- PathIndex::text_file_paths(cap): enumerate indexed md/txt/org/rst/etc.
- qt_method note_content_search: reads a bounded set of indexed text files
  (≤400 files, ≤512KB each), ranks, returns "path\tline\tsnippet". Bounded IO;
  QML debounces (120ms, shared with the suggestion timer).
- Wired into urlSuggestPopup._refresh: content matches (tag "c", magnifier
  icon) appear below quick suggestions; activating one navigates to the file
  (file://path). Line number carried for a future scroll-to-line.

## Limits / next
- Substring match (no stemming/fuzzy yet); fine for "cloak" exact-ish.
- Reads files synchronously on the qt_method call — bounded, but a very large
  notes tree could be slow; an rg pre-filter or background index is the
  follow-up if needed.
- Scroll-to-line on open (the line is captured but the md render doesn't jump
  there yet).
- Voice: the same path applies — a voiced "search my notes for cloak" can
  call note_content_search; intent wiring is the follow-up.
