# Autocomplete UX flow — search bar + ZBracket linking (v0.1.670)

## The unifying idea
The search bar and ZBracket (`[[link]]`) typing want the SAME thing: a
context-aware dropdown that completes from local files, the note graph, and
your history. So there is ONE ranking engine (note_graph::rank) with TWO
consumers. Web/graph search results merge in on top asynchronously.

## What feeds a suggestion (ranked blend)
For a partial string, each candidate gets:
- **name match** (gates everything): exact > prefix > word-boundary >
  substring > fuzzy-subsequence. Empty partial = neutral, so context decides.
- **graph adjacency** (×0.8): is this note linked to/from the current note?
  Direct neighbor = full, two-hop = partial. This is the Zettelkasten "mind
  palace" — when editing note A, the notes A already connects to surface
  first. Backlinks count (a note that links TO you is adjacent).
- **folder proximity** (×0.5): files in the same / nearby directory rank
  higher (path-distance decay). "Adjacent files" in the literal on-disk sense.
- **history** (×0.6): terms you've typed before, recency × frequency decay
  (~3-day half-life). Historical terms resurface.
Dedup across sources keeps the best score; the label shows the strongest
source (note / file / hist).

## Flow A — ZBracket linking (in a note)
1. User types `[[`. With an empty partial, the dropdown shows the current
   note's graph neighbors first (likely links), then nearby files, then
   recent link targets. (This is the "what do I usually connect here" state.)
2. As they type `[[Da…`, name-match narrows it; adjacency/proximity/history
   still re-rank within the matches — so a linked "Database" beats an
   unrelated "Data dump" even at equal prefix.
3. Enter/click inserts the name and closes `]]`. The new link is also a new
   graph edge → it informs future adjacency immediately.
4. A no-match partial still offers "create [[NewName]]" (a new note), since
   the resolver links unresolved names under the current dir.

## Flow B — search bar
1. Empty/short query: recents + bookmarks (existing search_suggest) PLUS, if
   a note is open, its graph neighbors and nearby files (note_graph). So
   search is seeded by where you are.
2. Typing ranks the blend; web/graph results stream in async and merge below
   the instant local/graph/history rows (so the bar is responsive — local is
   synchronous, web fills in).
3. Selecting a note → open it; a URL → navigate; a file → doc pane.

## Async web/graph merge
note_graph::rank is synchronous (local + graph + history) so the dropdown is
instant. Web suggestions (and any remote graph search) arrive later and are
appended/merged by the UI — never block the instant rows. (Wiring: the
existing search_web / search_suggestions path provides the web rows.)

## Wiring status (v0.1.670)
- DONE (pure + tested): note_graph::{NoteGraph, rank, proximity, adjacency,
  to_wire}; zbrackets::link_targets (builds the graph). qt_method
  note_completions(partial, current_note, current_path, files, notes,
  history) → ranked "text\tkind\tscore" wire. Compile-verified.
- UI-GATED (needs the build): the dropdown widget itself (shared by the
  search bar + the note editor), the async web-merge, and the keyboard/voice
  selection flow. The engine + entry point are ready; there's no note-editor
  surface yet (see TODO_zbrackets.md) so Flow A is gated on that pane.

## Honest notes
- Adjacency uses 1- and 2-hop; deeper graph walks (PageRank-ish centrality)
  could improve "important neighbor" ranking later — not done, measure first.
- The graph is rebuilt from the supplied notes each call; for large
  vaults a cached/incremental graph would be better (the caller can cache).
- History terms are supplied by the caller (search history / past links);
  recall.rs persistence could feed this.
