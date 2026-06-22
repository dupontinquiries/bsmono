# TODO: "what did I do last time" — recall over command history

## Vision (brainstorm B2, user-endorsed)
Session memory as a verb: "how did I fix this", "what was that docker
command", "the command I ran in the nginx dir". The terminal remembers
WITH you — better than Ctrl-R because it ranks by meaning (command +
output + cwd) and prefers commands that SUCCEEDED.

## Done (pure, tested — v0.1.640)
src/recall.rs: recall(query, &[RecallItem], now, top_n) -> Vec<RecallHit>.
- RecallItem = lightweight view of a term_blocks::CommandBlock (command,
  output_snippet, cwd, started_at, exit_code) — no FFI dep.
- Scoring: command match strongest (whole-word > substring), then cwd
  (0.5), then output (0.4); coverage bonus for matching more query terms;
  success modifier (exit 0 → ×1.15, failed → ×0.9); recency nudge.
- Dedups repeated commands to the best/most-recent instance.
- Empty query → most recent (the "what did I just do" listing).
- 10 tests incl. output-match, cwd-context, succeeded-outranks-failed,
  coverage, recency tiebreak, dedup.

## Wiring (build-gated)
1. **Source**: BlockTracker holds the command blocks (term_blocks.rs
   ~207). Map its blocks → RecallItem (truncate output to ~200 chars for
   output_snippet). If blocks are capped in memory, this searches the
   live session; for cross-session recall, persist blocks (see below).
2. **Query entry**:
   - voice intent: "what did I ... ", "how did I ...", "the command
     that ...", "last time I ..." → strip the lead-in, pass the rest to
     recall(). Add to the voice intent parser.
   - a qt_method `recall_commands(query: QString, n) -> QString` returning
     the hits as a wire string (command \x1F cwd \x1F ts \x1F exit per
     line) for QML to render as a pickable list.
3. **Render + act**: a results overlay (reuse the search-suggestion list
   shape); tapping/saying a number re-runs the command (or drops it on the
   input line for edit-then-run). exit_code lets the UI mark
   succeeded/failed.
4. **Cross-session (bigger)**: persist command blocks to disk (like
   archive.db) so "last WEEK" works, not just this session. A capped
   append-only log keyed by started_at; recall() already takes `now` and
   weights recency, so it works unchanged over a longer history.

## Why it beats Ctrl-R (the pitch)
Ctrl-R: exact substring, newest-first, no output/cwd/success awareness,
single-session. recall(): ranks by meaning across command+output+cwd,
prefers what worked, dedups, and (with step 4) spans sessions. It's the
adaptive-to-you moat — a generic shell can't do this.

## Next pure slice (optional, before wiring)
A voice lead-in stripper: "how did I fix the build" → "fix the build" (or
better, "build" + a "fix" intent hint). Pure phrase normalization, like
doc_resolver::extract_query. Lets the voice path feed recall() cleanly.

## WIRED v0.1.649 (compile-verified end-to-end)
- backend.recall_commands(query, n) qt_method: maps block_tracker.blocks →
  recall::RecallItem (output truncated to 200 chars), runs the ranker,
  returns wire string (command \t cwd \t started_at \t exit_code per line).
  qt_struct clean (444 fields), compile gate green.
- voice intent RecallCommands(String): "what did I do last time", "how did
  I fix X" (lead-in stripped → query), "what was that command for X", etc.
  + bare forms → most recent. Compiles (verified via gate; voice_intent
  can't run standalone). 4 tests added.
- QML: recall_commands dispatch → recallModel + recallOverlay (list of
  hits, exit-0 shown ✓ / failures ✗, tap a row → switch to terminal +
  pty_write the command (no \r, user edits/confirms)). lint clean.

### Still build-gated (needs your run to confirm behavior/feel)
- The overlay renders + tap-to-load works (QML runtime, not sandbox-
  testable).
- Cross-session: blocks are in-memory (this session only). For "last WEEK"
  persist blocks to disk — recall already weights recency via `now`, so it
  works unchanged over longer history. Next slice.

## DONE v0.1.660 — cross-session persistence (recall survives reboots)
- recall.rs pure: serialize_history / parse_history (tab-delimited, escaped
  tabs+newlines) + merge_history (dedup by (command,started_at), sort by
  recency, ring-cap MAX_PERSISTED=2000, drop oldest). 5 tests.
- main.rs wiring: recall_commands now loads ~/.config/web2qml/
  recall_history.tsv, merges with live block_tracker blocks, ranks over the
  union, and writes the merged set back. So "what did I do last time / last
  week" works across reboots — the kernel-dev's #1 pain (post-reboot context
  loss). Compile-gate green, qt_struct clean.
- Feeds session_auto's ReentryAfterReboot proposal (the restore set can come
  from persisted recall).

### Note
Persistence is lazy (load+merge+save on each recall query) — simple, no
per-block-finalize hook needed. If recall is queried rarely, history still
accrues because live blocks are merged in at query time. A periodic/on-exit
flush could be added later if blocks are lost before the first query of a
session, but the common path (query during/after work) captures them.

## STATUS (v0.1.725 audit): COMPLETE — no work needed
Verified already fully wired: voice "what did i do / how did i fix X" →
Intent::RecallCommands → recall_commands qt_method → recallOverlay (tap a row to
drop the command on the terminal line; ✓/✗ exit-code coloring present). Nothing
to add.
