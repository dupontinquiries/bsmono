# TODO: man-page navigation in search suggestions

## What shipped (pure, tested — v0.1.600)
- `SuggestKind::ManPage` (wire tag 'm') in search_suggest.rs.
- `suggest_commands(query, &[(name, summary)], cap) -> Vec<Suggestion>`:
  ranks command-help suggestions — exact name (120) > name prefix (100)
  > name substring (70) > summary word match (40). 2-char minimum
  (a 1-char query would bury real navigation suggestions). Empty summary
  → "<name> — man page" label; otherwise "<name> — <summary>".
- 5 tests (prefix/exact, summary-word match e.g. "compress"→gzip, tiny-
  query guard, cap, wire tag).

## Wiring (needs the running build — main.rs FFI)
In `search_suggestions` (main.rs ~7107), after the existing
`search_suggest::build(...)` (~7148) and before `to_wire` (~7153):

1. Build the command source as `Vec<(String,String)>`:
   - From `command_index::index()` — tldr-known commands WITH summaries
     (rich help). Map each CommandDoc → (doc.name, doc.summary).
   - PLUS, for COMPLETENESS, `path_binaries()` (main.rs ~15264) entries
     NOT already in the tldr index → (name, String::new()). This is the
     key completeness win: a command with a man page but no tldr entry
     still gets a "view man page" suggestion. suggest_commands already
     handles the empty-summary case.
   - Dedup by name (tldr entry wins, since it has a summary).
2. `let cmds = search_suggest::suggest_commands(&q, &command_source,
   cap_per_kind);`
3. Append `cmds` to the `suggestions` vec, then to_wire as today.
   (build()'s cross-source re-rank isn't applied to the appended set, so
   either re-sort the merged vec by score desc, or rely on QML grouping
   by kind — match the existing behavior.)

## QML (needs the running build)
In the suggestion dropdown delegate, handle wire tag 'm' (ManPage):
- Icon/label: show as a "📖 man" or "help" styled row.
- On tap: run `man <target>` in the terminal (or tldr <target> if the
  user prefers tldr — could check a setting). target is the bare command
  name. Reuse the existing term_cmd execution path.

## Completeness notes
- tldr index is ~10MB, curated, short. PATH binaries are just names (no
  text cost). Union gives breadth (every installed command) + depth
  (rich summary where tldr has one) without the ~100MB man-page indexing
  the command_index module deliberately avoids.
- man-page SECTION handling (foo.1 vs foo.3) is out of scope; `man
  <name>` picks the default section, which is right for the common case.

## WIRED v0.1.725
search_suggestions now appends man-page/command-help suggestions: source =
command_index::index() (tldr commands WITH summaries) + path_binaries() not
already indexed (name only), deduped (tldr wins). suggest_commands(q, source, 5)
→ merged into the suggestions vec, re-sorted by score desc, capped at 12.
Verified: "gz"→gzip (summary match), a PATH-only binary still gets a man-page
suggestion. RUNTIME-UNTESTED in the live suggestion dropdown; ranking unit-tested.
