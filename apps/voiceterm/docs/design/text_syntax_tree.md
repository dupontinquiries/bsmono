# Large-text syntax tree + chunked lazy rendering

Status: **DESIGN ONLY (filed post-v0.1.519).** F6: dragging a big
.txt/.md/book (12k lines / 99k words) into the browse tab renders
eagerly → severe lag. Fix = parse into a chunk/syntax tree, lazy-load
subsections, budget chunk size to processing power, and improve the
QML scroll view. Pure parsing core is testable; the QML lazy-render is
live-surface.

## The problem

The browse tab renders the WHOLE document up front. At 99k words the
QML text layout + the generate_safe_qml path choke. Need: don't build
or render what's off-screen.

## Design

### A grammar-per-format chunk tree (pure, testable)
Parse a document into a tree whose LEAVES are render-sized chunks,
using a per-format grammar:
  - **.txt**: trivial grammar — split on blank lines into paragraphs,
    then group paragraphs into chunks up to a size budget. (User: "txt
    has a grammar/syntax of splitting on spaces or newlines.")
  - **.md**: markdown structure — headings define subtree boundaries
    (a section owns its heading + body), fenced code stays whole,
    lists group; chunk within a section by the budget. (User: "for big
    .md files add the markdown syntax.")
  - **niftyplot / structured**: split by SUBGRAPHS, then chunk the
    data within each. (User's example.)
The tree node carries: byte/line span, a cheap summary/first-line (for
the scrollbar + outline), and lazily-built rendered content.

This is the SAME shape as note_cards.rs (split_into_cards) — generalize
that splitter into a format-dispatched tree builder. Reuse it.

### Processing-budgeted chunk sizing (pure)
Chunk size chosen to balance "quick to build" vs "good lazy scroll":
  chunk_budget(doc_len, assumed_cpu) -> target_chunk_chars
A basic assumption (not a live probe at first): cap chunks at ~N KB /
~M lines so each builds in a few ms and the lazy loader can stay ahead
of a fast scroll. Tunable; later could read real CPU count. (User:
"limited based on basic processing power assumptions.")

### Lazy/subsection loading (live surface — QML)
Only build + render the chunks near the viewport (a window of
±K chunks), build-on-demand as the user scrolls, evict far ones.
Maps onto a QML ListView/Flickable whose model is the chunk tree's
leaves (variable height, but now BOUNDED per chunk) with reuseItems +
cacheBuffer (the v0.1.508 lesson) and a real height estimate per chunk
so the scrollbar doesn't jump. (User: "qml scroll view might also need
some work" — yes; the v0.1.509 variable-height delegate finding
applies here too.)

## What's pure/testable vs live
- TESTABLE NOW: the tree builder per format (.txt/.md chunking),
  chunk-budget math, the leaf summary extraction, deviation/eviction
  policy (which chunks to keep loaded for a given viewport). Build on
  note_cards' splitter + tests.
- LIVE SURFACE: the QML lazy ListView binding, scroll-ahead build,
  height estimation, eviction wiring — needs the running build to tune
  the window size + verify scroll smoothness against a real big file.

## Staging
1. (pure) Generalize note_cards splitter → a `doc_tree` module: parse
   .txt and .md into a chunk tree with budgeted leaves + summaries.
   Tests with a synthetic 12k-line input.
2. (pure) chunk-budget + viewport-window/eviction policy functions.
3. (live) QML lazy renderer bound to the leaf model; tune on the
   actual book file the user dropped.
4. (later) niftyplot subgraph chunking; real-CPU-aware budget.

## Reuse / alignment
- note_cards.rs already splits captured content into structural cards
  (heading/prose/list/code) with tests — the .md grammar is basically
  that, and .txt is a simpler case. Generalize, don't reinvent.
- v0.1.508/509 sidebar-perf findings (reuseItems, cacheBuffer,
  variable-height cost) directly inform the QML scroll view here.
- diff_cards (note_cards v0.1.504) → re-render only changed chunks on
  edit, same as the helix-edit pipeline.

## Open questions
- Eager-parse the whole tree (cheap: just spans + summaries) but lazy-
  RENDER leaves? Likely yes — parsing spans is O(n) light; rendering
  is the cost. Confirm parse-all/render-near is fast enough at 99k
  words.
- Outline/minimap from the tree summaries — worth surfacing?
