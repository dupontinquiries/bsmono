# Dogfood perf fixes (v0.1.709)

Two daily-use blockers, tracked down by code inspection (no GUI in sandbox).

## 1. Laggy scrolling — reader codemap live-follow
ROOT CAUSE: _readerActiveSection() (added v0.1.701) ran on every scroll tick
(~8/sec while the codemap was open). It called backend.current_doc_outline()
(an IPC re-parse) AND, for EACH heading, _findMdHeadingY — a full recursive
mapToItem tree walk of the rendered doc. So N headings × full-tree-walk + 1 IPC,
per scroll tick, on a long document. That's the lag.

FIX: build an anchor-y cache ONCE per page load (_buildAnchorCache via a single
_collectMdhdgYs tree walk that gathers every mdhdg:<n> y in one pass), sorted by
y. _readerActiveSection now just scans that small array (O(anchors), no tree
walk, no IPC). Cache invalidated in pageLoader.onLoaded. The click-jump path
still uses the targeted _findMdHeadingY (not hot).

## 2. Helix launches sluggishly vs konsole — query-response poll latency
ROOT CAUSE: a TUI's startup query round-trips (DSR cursor-position, DA1 device
attributes, sync) were answered by drain_responses(), which only ran on the
50ms UI poll. The reader thread parsed the query and queued the reply in
grid.response_buf, but the reply wasn't WRITTEN BACK until the next UI tick —
up to ~50ms per round-trip. Helix blocks on these, so several round-trips
stacked into visible launch sluggishness. konsole answers in its read loop
instantly.

FIX: the reader thread now flushes grid.response_buf to the PTY writer
IMMEDIATELY after parsing each burst (writer_arc cloned into the thread), so
query replies go out with zero poll latency. drain_responses() on the UI poll
stays as a harmless backstop. enter_alt was already cheap (profiled); the cost
was the reply latency, not the screen switch.

## Verified
Compile gate green; qml_lint clean; harness 1311 + 28 (logic fixes in QML +
reader-thread paths — not unit-testable in-sandbox; verified by compile +
no regression). RUNTIME confirmation (scroll smoothness, helix launch snappiness)
is the user's dogfood test — these are the two specific mechanisms found.
