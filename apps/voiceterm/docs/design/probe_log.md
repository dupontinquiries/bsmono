
## graph_layout adversarial probe (v0.1.606, no code change)
Probed the Fruchterman-Reingold force math (live web graph-view path)
for NaN/divide-by-zero/panic under degenerate input:
- 30 coincident nodes at (0,0): finite ✓ (the dist2<1e-6 index-derived
  jitter at ~435 works)
- dangling edge (references nonexistent node): safely ignored ✓
- 200-node chain: finite ✓
- coincident pinned + free node: finite ✓
- NaN/inf SEEDED into an input position: stays non-finite but does NOT
  panic (correct GIGO — layout shouldn't launder caller-produced NaN)
Conclusion: sound, no fix. Guards in place: coincident jitter,
dist2.max(1.0), eff floor, attraction .max(0.001), velocity normalize
only inside `if mag > max_step`. Recorded so it isn't re-probed.

## v0.1.613 — harness dormant-test audit + decision_cache probe (clean)
Audited which test-bearing modules the harness actually RUNS: 65 of ~111
test-bearing modules aren't in the harness MODS list. Most genuinely
can't compile in-sandbox (crate::sibling chains, data files, clang/Vulkan
deps), but some are tier-2-stageable. Brought decision_cache (fingerprint
arc item 3) into tier-2 (+6 tests, 42→48). Probed it: normalize is
consistent across casing/punctuation/whitespace (lookup always hits what
record wrote), count increments on repeat, conflicting command replaces +
resets count. CLEAN — no fix. The remaining ~64 dormant-test modules are a
known backlog; adding the tier-2-stageable ones is incremental future
work (each needs its crate::sibling deps staged + sometimes a clog! stub).

## v0.1.616 — second dormant-test batch activated (tier-2), clean
Staged 6 more dormant modules into tier-2 (they need clog!+util, some
libc): llm_cache, missing_packages, user_tags, workspaces, async_jobs,
cpu_efficiency. +55 tests (tier-2 48→103). All passed — no bugs this
batch (unlike the unhide batch). Reported clean, no manufactured fix.
command_index was a candidate but pulls ureq (network); deferred until
that crate is worth staging. Remaining dormant modules mostly need
heavier deps (scraper, portable_pty, data files) or deep sibling chains
(term_blocks → 7 siblings); those are progressively less worth the
staging effort.

## v0.1.621 — notebook::classify probe found a real false-positive
Probed structured-output detection with hostile inputs (prose w/ commas,
colons, code, tracebacks, URLs). Most resisted (CSV branch well-guarded).
ONE false positive: prose log lines "Warning: disk low" / "Error: ..."
classified as MAP — parse_kv_map's ':' branch only required key + space-
after-colon, so any "Word: text" prose matched. Fix: value-compactness
guard — require a majority of values to be single-token (real kv values
are compact: localhost/5432/admin; prose values are multi-word). Keeps
multi-word-KEY maps ("Last Modified: 2pm") since it constrains the value
column. 4 regression tests. The CSV/TSV/whitespace/markdown branches
probed clean — no other false positives.

## v0.1.622 — motion_history::keys_to_reach probe (mostly clean)
Probed the helix-keystroke generator across edge cases (line 1, col 0,
large line/col, col→Nl conversion). Core logic CORRECT for all valid
inputs: 1-based line, 0-based col→<count>l, col-0 skip all right. Only
finding: a defensive gap — an out-of-contract line=0 emitted invalid
"0G". Added a .max(1) clamp + test so it emits valid "1G". Not a
logic bug; robustness hardening. (keys_to_reach is still build-gated/
unwired — the status-line parser that feeds it is the gated part.)

## v0.1.622 (follow-up) — search_suggest::suggest_commands probe: CLEAN
Probed the command/manpage suggester: ranking tiers (exact 120 > prefix
100 > name-substring 70 > summary-word 40), dedup, cap, case/whitespace
handling. All correct:
- exact/prefix/substring ordering right; duplicate command names deduped
  (kept the highest-scored); "  TAR  " trims+lowercases to match "tar".
- 1-char queries correctly rejected (too noisy).
- score ties hold stable insertion order.
Two behaviors are documented DESIGN CHOICES, not bugs, so left alone:
- multi-word query ("cat clone") matches nothing — summary matching is
  whole-word, and the query has no name prefix/substring.
- "concat" doesn't find cat ("concatenate" != "concat") — whole-word
  summary matching is deliberate to avoid substring noise. Loosening it
  would re-introduce the noise the design avoids.
No fix shipped — clean probe reported as clean.

## v0.1.623 (follow-up) — editor_voice probe: CLEAN (wired live code)
Probed the helix/vim voice-command translator (WIRED, 4 call sites) +
detect_editor. All correct:
- detect_editor takes /proc/<pid>/comm (bare process name, not argv), so
  "hx file.txt" never reaches it — matching bare "hx"/"helix" is right.
- translate maps keystrokes to each editor's MODEL correctly: Helix
  selection-first (delete line "xd" = select-line-then-delete; copy line
  "xy"), Vim line-ops ("yy", "dd"). The two "copy line" arms are in the
  CORRECT editor blocks (422 helix xy, 499 vim yy) — not swapped.
- destructive quit-without-saving flagged confirm=true.
- strip_filler loops to remove stacked fillers ("please um save"→":w")
  without eating real words; deliberately does NOT strip "go " (protects
  "go to line/top").
- ambiguous/invalid inputs decline rather than guess: "save and quit
  without saving"→None, "go to line abc"→None, bare "go"→None.
Minor non-bugs (left alone): bare "copy" has no mapping (only "copy line"
does); "go to end" unmapped while "go to bottom"→ge. Synonym gaps, not
correctness bugs — adding them is feature work, not a fix.
No fix shipped — clean probe reported as clean.
