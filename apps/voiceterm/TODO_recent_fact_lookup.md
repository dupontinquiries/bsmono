# TODO: Recent-fact lookup voice commands

User's framing:
  "scroll back up to {keyword}"
  "insert that last IP address"
  "insert the IP address for {keyword}"
  "insert name of last file opened" (mid-command, e.g. for git commit -m)

These are different voice patterns from what we have today, but they all
share an underlying capability we don't yet implement:

> **Index of typed-and-extractable facts from recent terminal output,
> queryable by voice during command composition or pager scrolling.**

## Three sub-features

### 1. Mid-command insertion ("insert that last IP")

User scenario: user is typing a curl command, gets to the URL part, can't
remember the IP. Says "insert that last IP address". Voice should:

  a. Scan recent BlockTracker output for IP-shaped tokens (or whatever
     "type of fact" the user named: "IP", "MAC", "hash", "PID", "port",
     "filename")
  b. Pick the most recent matching token
  c. Inject it as PTY input AT THE CURRENT CURSOR POSITION (not as a new
     command)

Critical UX detail: the user's typed-buffer is half-composed. We can't
just send a new command line — we need to write the IP literally into the
PTY at the cursor. That's `pty.write_str("192.160.2.170")` with no Enter,
which the user then continues typing after.

### 2. With-keyword variant ("insert the IP for kwin")

Same as above but with a filter. "Insert the IP for kwin" means: among
recent IP-shaped tokens, find the one that appeared in or near a block
mentioning "kwin". Requires an output-context score: token-found-in-block
score = how well the block's command + nearby output matches the keyword.

### 3. Pager / scrollback navigation ("scroll back up to {keyword}")

Different from 1 & 2 — this is about scrolling the visible terminal grid
to a specific historical row. Implementation:

  a. Search the scrollback buffer for the keyword
  b. Scroll the grid so that line is visible at top
  c. Optional: highlight/flash the line briefly

Our `pty_term::TermGrid` has scrollback. Need a `find_in_scrollback(keyword)
-> Option<row_idx>` method, then scroll to it.

## Why this is its own session

It needs:

- A "fact extractor" pass distinct from per-tool entity extractors. Not
  bluetoothctl-specific or git-specific — it scans ANY block's output for
  generic shapes (IPs, MACs, hashes, PIDs, ports, file paths). New module
  `fact_extract.rs`.

- A `pty_write_at_cursor` PTY write path that doesn't trigger
  track_pty_command's submit-on-Enter logic (because we're inserting
  mid-command, not submitting). Needs careful state handling.

- New voice patterns in voice_parse_candidates: "insert ...", "scroll up
  to ...", "find ... above". These need to be recognized BEFORE generic
  intent parsing because they're context-sensitive (the cursor position
  matters, the typed buffer matters, scrollback state matters).

- New intent variants: `InsertAtCursor(String)`, `ScrollbackTo(String)`.

Estimated scope: ~400 lines new code split across fact_extract.rs (200),
voice_intent.rs (50), main.rs (100), qml/main.qml (50). Not for one
turn-and-test cycle — needs its own session with focused testing.

## Storage thoughts (out of scope for v1)

For "insert IP for kwin", a one-shot scan of recent BlockTracker output
works for the latest session. For "the IP I used last week", we'd need to
persist fact extractions across sessions. That's a real database. Not
needed for v1 — start with in-session, decide on persistence after seeing
how the feature gets used.

## Whisper finetuning

User raised: "maybe there is a way to finetune the whisper models or just
more evidence for our own".

Honest answer: finetuning whisper is feasible but substantial. You'd need:
- Hours of audio of you speaking your own filenames + commands (200+
  utterances minimum, ideally 1000+)
- Time-aligned transcripts (forced-alignment tooling)
- A GPU for finetuning (a few hours on a 16GB+ card)
- Model deployment (replace `ggml-base.en.bin` with your finetuned ggml)

The lighter-weight thing we've been pursuing — post-correct whisper
output against a lexicon (zsh history + filenames in recent output +
shell function definitions) — gets most of the benefit without the
training overhead. The custom-lexicon approach also adapts faster:
finetuning has to be redone when your projects change; lexicon scanning
is automatic.

Recommendation: do lexicon-based post-correction first (own session).
Re-evaluate finetuning after we've seen how good lexicon correction
gets us. My guess is lexicon alone fixes 80% of the filename-mishearing
problems, and finetuning is for the long tail.

---

## v0.1.573 PROGRESS — fact_extract.rs core BUILT + tested (the pure foundation)

The "fact extractor" pass the spec called for is DONE and verified:
`src/fact_extract.rs` (~500 lines, registered in main.rs + the pure-test
set). 19 tests, all passing via rustc --test against real execution.

### What it does (pure, dependency-free, no regex)
- `FactKind` enum: Ipv4, Ipv6, Mac, Hash, Pid, Port, Path.
- `FactKind::from_spoken(noun)` → maps spoken nouns ("ip address",
  "hash", "port", "filename"...) to a kind, for voice parsing.
- `extract_all(text) -> Vec<Fact>` — scans ANY text for all fact shapes,
  each Fact carrying {kind, value, offset}. Offset = byte position so
  callers rank by recency (terminal output is appended).
- `most_recent(text, kind)` — latest fact of a kind.
- `most_recent_near(text, kind, keyword)` — powers "insert the IP for
  kwin": prefers a fact on a line mentioning the keyword, falls back to
  most-recent if no keyword line has one.

### Verified edge cases (these caught real bugs during the build)
- Wrapped + punctuated tokens: "(192.168.0.1)." → "192.168.0.1".
- Relative paths preserved: "./build/out" is NOT mangled to "/build/out"
  (asymmetric trim: leading '.' kept, trailing '.' stripped).
- MAC vs IPv6 disambiguation (both use ':').
- Precision over recall for PIDs: only "pid N" / "pid=N" / "[pid=N]"
  forms, never a bare number.
- Ports range-checked (1..=65535); octets range-checked (0..=255).
- URLs ("://") excluded from Path.

### STILL PENDING (live surface — needs the running build, not testable here)
1. `pty_write_at_cursor(s)` — write a fact into the PTY at the cursor
   WITHOUT triggering track_pty_command's submit-on-Enter. Careful
   state handling (see main.rs track_pty_command).
2. Intent variants `InsertAtCursor(String)` + `ScrollbackTo(String)` in
   voice_intent.rs, recognized BEFORE generic parsing (context-sensitive).
3. Voice patterns in voice_parse_candidates: "insert that last <noun>",
   "insert the <noun> for <kw>", "scroll up to <kw>".
4. `find_in_scrollback(keyword) -> Option<row_idx>` on TermGrid + scroll.
5. Wire it: on InsertAtCursor, call fact_extract::most_recent[_near] over
   recent BlockTracker output, then pty_write_at_cursor the value.

The hard/uncertain parts (1, 4) are the PTY/grid state handling; the
extractor itself (the part most likely to have subtle parsing bugs) is
now done and locked behind tests.

---

## v0.1.603 — the phrase PARSE is now done (pure, tested)
Item 2-3's recognizer shipped: `fact_extract::parse_fact_request(phrase)
-> Option<FactRequest { action: Insert|ScrollTo, kind: FactKind,
keyword: Option<String> }>`. Handles "insert the last IP address", "type
the hash", "insert the ip for kwin" (keyword split happens BEFORE noun
lookup so "for the server" doesn't corrupt the noun), "scroll back to
the hash", "find the filename in scrollback", stacked filler words, and
rejects non-requests (no verb / unknown noun / no noun). 4 tests.

Remaining (build-gated, unchanged): wire parse_fact_request into
voice_parse_candidates / the intent layer; on Insert call most_recent[_
near] + pty_write_at_cursor; on ScrollTo call find_in_scrollback + scroll.
The two genuinely hard parts (pty_write_at_cursor without submit-on-Enter,
and find_in_scrollback on the grid) remain the build-gated core.

## WIRED v0.1.725
- recent_fact(phrase) qt_method: parse_fact_request → most_recent[_near] over
  the last ~40 command blocks' output. Insert → pty.write_str(value) at cursor
  (NO newline, so no submit); ScrollTo → returns "scroll\t<value>". Wire:
  "inserted\t<v>" / "scroll\t<v>" / "none".
- QML freeform dispatch: "insert/type/paste/scroll up to/jump to …" routes to
  recent_fact; inserted → toast; scroll → opens the terminal find bar pre-filled
  with the value (reuses the proven search+jump). Falls through to normal
  handling on "none".
- Verified end-to-end: "insert that last IP"→most recent, "insert the IP for
  kwin"→keyword-scoped, "scroll up to the hash"→ScrollTo.
RUNTIME-UNTESTED: PTY write-at-cursor + find-bar jump need the live app; the
parse + extraction + ranking are unit-tested (25 tests).
