# Deferred ideas from v0.1.56-v0.1.57 sessions

These are real ideas, each substantial enough to warrant its own session.
Listed roughly in order of value-per-effort.

## 0. (RESOLVED v0.1.465) Two pre-existing graph_layout test failures

Both fixed in v0.1.465 — and they turned out to be different
kinds of problem, handled differently:

- `disconnected_cards_dont_overlap` — was a REAL latent code bug.
  Coincident nodes (dx=dy=0) produced a zero repulsion UNIT vector
  (`dx/dist = 0/1`), so the push had magnitude but no direction —
  stacked nodes never separated. Could have bitten real graphs if
  a seed ever placed two nodes at identical coords. Fixed in the
  repulsion loop with a deterministic, per-pair-index symmetry
  break (angle from `(i*7 + j*13) % 360`) so coincident clusters
  fan out evenly and reproducibly.

- `single_card_unchanged_by_layout` — was a TEST-expectation bug,
  not a code bug. The 50px-arrival tolerance was unsatisfiable
  given the deliberately-gentle center-gravity (geometric decay:
  a card 300px out reaches ~57%, x≈272, in the default 200 iters —
  verified the math reproduces the exact observed value). A
  stronger pull would crush dense graphs, which is precisely what
  adapt_to_graph guards against. Fixed the test to assert the real
  contract (substantial movement toward center + no overshoot) and
  documented why inline so it's not re-tightened.

All 29 graph_layout tests now pass.

## 1. Process metric overlay on the prompt line

### v0.1.602 UPDATE: pure core extracted + tested
The overlay was ALREADY implemented (system_info::resource_overlay +
memory_usage). v0.1.602 extracted its parse + decision logic into the
pure, tested sysmon.rs module (parse_meminfo_pct/used_total,
parse_loadavg_1min, format_mem_usage, overlay_decision) — 11 tests
covering the bug-prone threshold-crossing logic (fires-once, alert
priority, 5-point jump, first-sample-quiet). Dedupes the meminfo parse
that was copied in two IO fns. Behavior preserved exactly. The QML
overlay rendering (floating rect near cursor, fade) is the remaining
build-gated UI piece.

User wants: a small floating rectangle near the cursor/prompt line showing
condensed CPU/MEM use. Could replace the cwd display every few seconds with
a fade transition.

Architecture sketch:
- Backend qt_method `system_resource_summary()` returns "cpu:42% mem:1.2G/8G"
- Read from /proc/loadavg, /proc/meminfo (or sysinfo crate)
- Poll every 2s on a Timer in QML
- Render as a Rectangle pinned to cursor row (use existing termCursor x/y)
- Fade between cwd label and resource label every 4s on cwd-line shells
- Critical: don't render in alt-screen mode (would overlap helix UI)

Estimated size: ~80 lines Rust + ~50 lines QML. Half session.

## 2. Auto package install card

When voice references a tool that's not on PATH, surface an install card
with one-tap action. Backends: cargo / apk / dnf / brew / pacman. Detect
which is the right one for the system from /etc/os-release and presence
of the tool binaries.

UI: a card with red border, "Installing X..." text, ::after-style overlay
that fades when the install completes. The actual install runs in PTY
(visible to user) but with the voice-supplied label.

Source-of-truth for "is X installed":
- which X → quick path check (0ms, cheap)
- if absent, search via the relevant pkg manager:
   - cargo search X --limit 1
   - apk search -e X
   - dnf provides X
   - brew search X
   - pacman -Ss X

Track whether voice-driven so commands appended to history are tagged
(user wanted: "comment that voice put this in somehow"). Simplest: add a
`# voice: <utterance>` comment line before the install command.

The user noted the dnf bug where you can't rely on auto-handling for
history-included installs — solution is to materialize the install as
a real pty command, not a shell-of-shell layer.

Estimated size: ~250 lines + UI. One session.

## 3. Voice-driven gdb stepping  (CORE DONE v0.1.477)

**Status: the pure core is built + tested (src/repl_voice.rs, 14/14),
exposed via qt_methods repl_voice_translate(last_line, phrase) and
repl_voice_active(last_line).** Covers gdb + lldb + python + sqlite +
redis (generalized to the "REPL voice mode" framework the note below
envisioned — each REPL is a (prompt-matcher, verb-table) pair).
Includes shell-prompt discrimination (bare `$`/`>` don't match) and
symbol-injection rejection (a transcript like "print x; quit" is
refused rather than injected into the debugger PTY).

REMAINING INTEGRATION (needs the live voice dispatch + a real build):
At the point voice produces a transcript, BEFORE normal intent
routing: sample the last PTY output line, call repl_voice_translate
(last_line, transcript); if it returns non-empty, write "<cmd>\r" to
the PTY and skip normal routing. Also call repl_voice_active to drive
a "voice → gdb" mode indicator in the UI. Left unwired because the
voice→intent dispatch is deep, un-runnable-in-sandbox code; the
translation seam is intentionally pure so this hook is a few lines.

Original note follows:


Voice commands: "step into" / "step over" / "next line" / "continue"
/ "print X" / "break at line 42" / "list" / "quit gdb". Detect when
the focused PTY app is gdb (look for `(gdb) ` prompt in last output line)
and route voice through to gdb commands instead of shell commands.

Architecture:
- Detect gdb by prompt pattern in TermGrid
- New voice intent kind: GdbCommand
- Verb dictionary: step→s, next→n, continue→c, print→p, list→l, etc.
- Compose: `<gdb_cmd>\r` directly to PTY when in gdb context

Same pattern would work for python REPL, lldb, sqlite cli, redis-cli,
etc. Generalizable to "REPL voice mode" where each REPL has a verb table.

Estimated size: ~200 lines, generalizable framework. One session.

## 4. Whisper Large V3 Turbo on desktop

Desktop has way more compute than mobile. We're already adaptively
choosing tiny on slow devices. Symmetrically, on fast desktops we could
opt INTO Large V3 Turbo for better accuracy.

Detection: average transcription speed << threshold (say ratio < 0.05x
realtime over 5 utterances) AND we're on base.en → suggest upgrade.
Auto-download? Probably not — Large V3 Turbo is ~800MB. Better: log a
suggestion with the URL and let user opt in.

Mind that V3 Turbo is a Whisper-Large variant (32→4 decoder layers); it
takes the place of Large V3, not base. So the comparison is base vs
Large-Turbo. Large-Turbo at 800MB will likely BE slower than base on a
typical desktop CPU, despite being labeled "turbo". The label is
relative to Large V3 (~6x faster than that), not faster than base.

So: Large V3 Turbo isn't actually a desktop upgrade for our use case
unless the desktop has serious compute. Check: an M2 desktop transcribes
base.en in ~50ms; Large-Turbo would still be 200-300ms because parameter
count dominates over decoder depth at those sizes.

**Verdict: Large V3 Turbo doesn't actually help us. Better path is
quantized base.en (Q5_1 at 57MB, ~2x faster than fp16 base, no WER
loss).** Already documented in v0.1.56's research notes.

## 5. Dual-model parallel inference

User said: "we could always run tiny in background on desktop to do
matching". Idea: while base.en is transcribing, ALSO run tiny.en on the
same buffer. Use tiny's faster output for immediate hint-sidebar
updates, keep base for the canonical transcription that drives intent.

Architecture:
- Two whisper contexts loaded simultaneously
- On audio capture, dispatch to BOTH via std::thread
- tiny finishes first (~150ms vs ~800ms for base on desktop)
- Use tiny's output to populate live hints sidebar immediately
- When base finishes, replace tiny's text with base's authoritative version

Memory cost: ~220MB for both models loaded. Acceptable on desktop.
Should be DISABLED on mobile where one model is already strain.

**Recommendation interaction (handled v0.1.478):** dual-model means
TWO whisper models resident, which shrinks the RAM budget available
to the LLM. The model recommendation now accounts for this:
hf_hub::whisper_reserve_gb(dual) returns the non-LLM reserve (single
vs dual), and recommend_curated_with_reserve subtracts it. When this
feature lands, set `dual_whisper = <enabled>` in
setup_recommendation_json (main.rs) and the recommendation budget
updates automatically — no recommendation-logic change needed.

Estimated size: ~150 lines, mostly thread coordination. One session.

## 6. Streaming whisper

Still pending. The right architecture for the live sidebar to update
DURING utterance, not just after. Substantial work because whisper is
not designed for streaming — needs chunked overlap-and-add or use of
a dedicated streaming variant (Voxtral Realtime came out Feb 2026).

Stays deferred.

## 7. Selection off-by-one in helix

Still deferred. Needs interactive testing with helix's exact selection
escape sequences captured.

## 8. Workflow recorder

Still deferred. Documented in TODO_workflow_recorder.md.

## 9. Voice-on-web

Still deferred. Documented in TODO_voice_web.md.

## 10. Recent-fact lookup

Still deferred. Documented in TODO_recent_fact_lookup.md.
