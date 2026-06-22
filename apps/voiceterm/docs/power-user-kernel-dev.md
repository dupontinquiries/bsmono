# Kernel-engineer workflow patterns (Asahi reverse-engineering transcripts)

Source: 3 livestream transcripts (~43k words) of low-level Linux kernel
reverse-engineering on Apple Silicon — DCP display-coprocessor command
interface + ASC mailbox reversing, and installer implementation. This is a
think-aloud record: the engineer narrates intent, friction, and tool use
continuously. Below are the patterns that matter for designing power-user
features, separated into his two modes.

## Who this user is (from the evidence)
A systems/kernel engineer doing register-level reverse engineering: tracing
coprocessor mailboxes, diffing memory/register dumps, writing Python
tracer/harness code, rebooting hardware to test, reading hex/addresses, and
bouncing between his tracer, source, logs, and docs. Moves fast, lives at a
low level, tolerates raw tools but is constantly fighting two things:
**slow feedback loops** and **losing context**.

## MODE A — "educational / streaming" (explaining while doing)
When narrating for an audience he is simultaneously operating and
teaching. The observable patterns:

1. **Constant verbalized intent.** He says what he's about to do before
   doing it ("let's see what happens if…", "I need to…", "I'm going to…").
   The narration IS a command stream — he's already speaking his actions
   aloud. (Hundreds of "let's / I need to / I'm going to" openers.)
2. **Show-and-tell context switches.** He pulls up a thing, explains it,
   moves on — tracer → source → log → register view — narrating each jump.
   The switch itself is a spoken beat ("let me look at…", "let me see…").
3. **Re-orientation out loud.** "Where was I?", "Oh, I think I sent the
   wrong trace.", "what happened here I think is…" — he reconstructs lost
   context verbally, for himself as much as the audience.
4. **Recall of earlier state.** "Let's see if it was the same in 9.4",
   references to "previous data in the log" — he reaches back to compare
   now vs a few minutes/versions ago.

## MODE B — "hardcore development" (heads-down RE loop)
The core inner loop, by frequency:

1. **The reboot/reload tax (dominant pain).** reboot ×8, reload ×6, reset
   ×4, crash ×5, panic ×2. Direct quotes: "It's really annoying to have to
   reboot everything.", "I'm going to reboot and start over again", "No,
   it's still doing reboots." Every hardware test risks a multi-minute
   reboot. This is THE latency he fights.
2. **The trace → diff → observe loop.** His own tooling: a tracer with
   per-endpoint classes, a "register monitor / memory diffing tool" that
   "dumps it all the time… and tells you if anything changed". He toggles
   it: "I can turn off the register monitor for this and that'll speed
   things up a lot" — i.e. he manually trades observability for speed.
3. **Hunting through output.** "Where is that coming from, though?", "Where
   is that sort thing coming from?", "How do I find the index now?" — he
   repeatedly searches logs/dumps for the origin of a value. Finding-where-
   a-value-came-from is a recurring, unsupported need.
4. **Staring at raw hex/addresses.** "This is the address, and there's no
   obvious size.", "the address before that is 0902 CAFE…", offset
   reasoning. He manually correlates addresses, offsets, sizes across
   dumps.
5. **Slowness he can't explain.** "Why is it so slow? I don't know why when
   I use curl…" — unexplained latency mid-task that he has no quick way to
   diagnose.
6. **Tool-identity confusion.** "I feel like I have two tracers here", "dcp
   tracer state, nah, dot ep…" — he loses track of which instance/file/
   buffer he's in.

## Cross-cutting friction (both modes)
- **Feedback latency dominates** (reboot tax + unexplained slow ops). Every
  minute waiting on a reboot is a minute of lost flow + lost context.
- **Context loss is constant** ("Where was I?"). Switching between tracer/
  source/log/docs drops his mental stack; he rebuilds it by hand.
- **Provenance hunting** ("where is that coming from?"). He spends real
  time tracing a value/log line back to its source.
- **Raw-data correlation** (hex, offsets, registers) is manual and
  error-prone.
- **Observability vs speed is a manual toggle** he flips constantly.

## What he reaches for (toolchain evidence)
python (51 — his tracers/harnesses are Python), tracer (15), register/
register-monitor (12), trace (9), dump (8), hex (8), less (5), hypervisor/
m1n1-proxy (5), mailbox (4), diff (2), vim (2), grep (1), mmio (1). Low-
level, scriptable, text-first. He builds his own observability tools.

## Design implications (seed for the brainstorm — not yet features)
These are the leverage points the transcripts expose:
1. **Kill the reboot-tax dead time.** When a reboot/reflash is unavoidable,
   the dead minutes are when context is lost. A feature that *holds and
   re-presents* "what you were doing / last command / last diff / where you
   were" across a reboot would directly attack his #1 pain. (Connects to
   recall + session-walk, but for the dev loop, not browsing.)
2. **Provenance / "where did this come from".** He repeatedly asks where a
   value or log line originated. A "trace this value/line back" — jump from
   a log line to the command/source that produced it — fits his hunting.
   (Connects to recall over command+output, and clippings-with-provenance.)
3. **Diff-aware log/dump views.** His own killer tool is "dump it and tell
   me what changed." First-class "what changed since last run / last
   reboot" in the log/output surface would match how he already thinks.
4. **Voice for the narrated loop.** He ALREADY speaks his intent
   continuously ("let's see what happens if…"). In Mode A especially, a
   voice layer that turns narrated intent into actions (run, diff, open the
   source for this symbol, re-run last) fits a workflow that's already
   verbal — without breaking his hands-on-keyboard flow.
5. **Buffer/instance disambiguation.** "I have two tracers here" — when
   multiple similar windows/instances exist, naming/labeling them (and
   voice-addressing them: "the DCP tracer") cuts the which-one-am-I-in tax.
6. **Diagnose-the-slowness affordance.** "Why is it so slow?" — a fast
   "what is this process waiting on" probe (the dev-loop analog of the
   log-summary worker) would catch the latency he can't currently explain.
7. **Observability presets, not manual toggles.** He hand-trades register-
   monitor on/off for speed. A one-gesture "fast vs full-trace" mode would
   remove that fiddling.

## Note on the data
These are spoken-narration captions, not terminal logs, so command syntax
isn't literal — the value is in the INTENT, FRICTION, and CADENCE, which
narration captures better than a shell history would. "Let me open stuff."
recurs as a caption artifact and was excluded from analysis.

---

# Addendum — patterns from READING full passages (not just grep sampling)

## Honest correction
The first pass (7 patterns) came from grep/frequency sampling over the
stripped text, NOT a full read. The user rightly pushed back that long-form
content should yield more. Reading continuous stretches surfaced patterns
the sampling flattened. Documenting those here, plus the user's own
additions.

## User's added suggestions (verified against the transcript)
- **U1. Detect debug mode → offer to spawn/attach gdb + monitor, via a
  fuzzy voice picker.** Strongly supported: he manually stands up tracers/
  monitors and toggles them. An assistant that NOTICES "you're debugging
  this" and offers "want me to attach gdb / watch these registers?" with a
  voice-driven fuzzy picker for WHICH thing to monitor matches exactly how
  he already works (he's choosing what to trace constantly).
- **U2. Stuck-cadence popups with relevant context (the cafe-quote idea,
  applied to dev).** When typing/among cadence stalls, surface a small
  contextual nudge — relevant files, a web/doc snippet, the last related
  command. The transcript shows long "uh… where… hmm… one second" stalls
  (see U-patterns below) where a context nudge would land.

## Patterns only visible on a full read

- **P8. He writes CODE live, reasoning structure aloud.** Long stretches
  are him designing a data structure in Python in real time ("let's make it
  a list because I can mutate it… if it's an int return an accessor,
  otherwise assume it's a list… it should be an ndRange at that point").
  This isn't command issuing — it's THINKING IN CODE out loud. A voice
  layer that only does "run/open/switch" misses most of his actual talking.
  Implication: the high-value voice surface for him is NOT command
  dispatch — it's capture/scaffolding ("jot that as a note on this
  function", "remind me ndRange needs the in-range lookup too").

- **P9. Heavy self-correction / branch-and-backtrack.** "Actually it's
  probably easier to just do… you know what, it's easier to just do it like
  this." "Oh no, it should be an ndRange." He constantly proposes, rejects,
  re-decides. He'd benefit from a lightweight "decisions log" — capturing
  the choice + the reason — because he revisits these ("where was I /
  why did I…"). Connects to recall, but for DECISIONS not commands.

- **P10. Hypothesis → check → confirm loop on RAW DATA.** The mailbox/ring-
  buffer reversing is pure hypothesis testing: "this looks like a ring
  buffer… this is the write pointer… two ring buffers, one for commands one
  for responses… is there anything larger than E8? let's look further
  down." He forms a structural guess, scans the dump to confirm/deny,
  revises. Tooling that lets him ANNOTATE a hypothesis onto a memory region
  and auto-recheck it against new dumps would be huge (it's his diff tool +
  a labeled hypothesis).

- **P11. Counting/arithmetic on addresses, out loud, repeatedly.** "I count
  back 64 bytes. And I count back 64 bytes. And I count back 64 bytes." "It
  adds up to 4K." "padded to a cache line." He does manual offset/size
  arithmetic constantly and it's error-prone. A quick "what's at this
  address minus 0x40 / is this cache-line aligned / how big is this region"
  helper (voice or inline) targets real, repeated manual math.

- **P12. Long stalls with verbal filler = the real "stuck" signal.** "One
  second." "Uh…" "Hmm." "Hold on." "I don't know. I don't know." Repeated
  "Cool. Cool. Cool." (thinking filler). These ARE the cadence-stall moments
  U2 wants to detect — and notably they're often AUDIBLE (he's narrating),
  so a voice-first tool can detect "stuck" from speech disfluency, not just
  typing pauses.

- **P13. Interruptions / life happens mid-flow.** "I actually need to use
  the toilet, sorry." "So many spam accounts lately." He context-switches
  AWAY involuntarily, then has to re-enter. Re-entry support ("here's where
  you were: editing ndRange.contains, last dump showed E8 at…") attacks the
  same context-loss but for human interruptions, not reboots.

- **P14. "Does this still work?" regression anxiety.** "Hopefully I didn't
  break anything." "Let's see if that works." "That's broken int and range.
  Why?" After each change he's unsure what he broke and re-checks manually.
  A "what changed and what might it have broken" cue (diff-aware, P3-related)
  fits.

- **P15. Abandoning dead ends explicitly.** "I give up on detrust." "I don't
  think it's worth it for the kind of stuff we're doing." He makes explicit
  give-up decisions — worth capturing so he (or a teammate/viewer) knows a
  path was tried and rejected and WHY. (Decisions-log again, P9.)

## Revised design themes (superseding the first 7's framing)
Reading changes the emphasis. The biggest correction: **his dominant
verbal activity is thinking-in-code and hypothesis-testing on raw data —
NOT issuing commands.** So the highest-leverage voice/assist features are
CAPTURE and CONTEXT, not dispatch:
  A. Capture: voice notes/decisions/hypotheses attached to code regions,
     memory addresses, or commands (P8/P9/P10/P15). The clippings-with-
     provenance core already fits — point it at code+addresses+decisions.
  B. Context preservation across BOTH reboots (P-reboot) and human
     interruptions (P13) — "where you were" re-entry.
  C. Active assists offered on detected state: debug-mode → gdb/monitor
     picker (U1); stuck-cadence (now detectable from speech disfluency,
     P12) → contextual nudge (U2); post-change → regression cue (P14).
  D. Raw-data helpers: address arithmetic (P11), hypothesis-annotate-and-
     recheck on dumps (P10), diff-aware "what changed" (P3).
The command-dispatch framing from the first pass is the SMALLEST part of
his real workflow.

---

# Round 2 — user reactions + new directions (steer the brainstorm)

## Validated (keep / prioritize)
- **Command-history PAGE + better summaries** — confirmed good. The recall
  surface (v649) + log-summary improvements are on the right track. Lean in.

## CRITIQUE to take seriously (a design principle, not a one-off)
- **The sidebar + "missing packages" feel UNCONSIDERED — "thrown in because
  there was space."** This is the important note: features placed by
  available-real-estate rather than by workflow intent read as clutter.
  PRINCIPLE going forward: a surface must earn its place by a workflow
  moment ("you're about to need X"), not by filling empty UI. Audit the
  sidebar + missing-packages affordance against "what task is the user in
  when this should appear?" — if there's no answer, it shouldn't be ambient.
  Applies to ALL the active-assist ideas below: they must be triggered by
  detected state, surfaced transiently, and dismissable — never permanent
  furniture.

## New / sharpened directions

- **R1. Editor-open with ADVANCED CONTEXT (helix AND vim).** He uses vim;
  helix matters for "the next wave of devs" — support both. The leverage:
  open an item (file/symbol/log line) directly into the editor WITH
  context already resolved — a specific deep file path (via our
  path_fuzzy/entity_extract extractors), and crucially ACROSS SSH MACHINES
  (open path X on host Y in $EDITOR). "Open the dcp tracer on the m1n1 box
  at line 400" → resolves host + fuzzy path + editor + line. Ties the
  file-path extractors to editor + remote target. This is the concrete,
  high-value version of the editor work.

- **R2. Auto-named sessions with ACCURATE info + a mental map for short-
  burst memory.** Auto-generate session names from real signal (the repo/
  branch, the files touched, the dominant command, the host). Then a
  MENTAL-MAPPING layer to aid short-term recall — could be:
    * geometric (spatial positions, like the cards canvas),
    * docker-name style (memorable random adjective-noun handles),
    * or other learned patterns.
  Goal: when he bounces away and back (reboot/interruption/window-switch),
  the session has a name + a mental hook he can re-grab fast. Directly
  serves the context-loss + re-entry theme (B). Pairs with recall.

- **R3. Process-status / "why so slow" bubbles.** Expand the performance-
  diagnosis idea into transient popups/bubbles: when an op is slow or a
  process is stuck, surface what it's WAITING ON (syscall/IO/lock/network),
  as a transient bubble — the dev-loop analog of the log-summary worker.
  Must obey the principle above (triggered by detected slowness, transient,
  not ambient). Answers his literal "Why is it so slow? I don't know why."

- **R4. Spoken-word VARIATION command calling.** Play on natural speech
  variation: the same intent said many ways ("run the tests" / "kick off
  the tests" / "let's test this" / "cargo test it") should map to one
  action — and conversely, learn HIS idiomatic phrasings over time. Leans
  on the existing intent parser + decision_cache + confusion table, but
  framed as embracing spoken variation rather than fighting it. Connects to
  his already-verbal narration (P8/P12).

- **R5. Command + session CLUSTERING of voice data in the git repo (kmeans
  → flat labels → abstract to branches).** The most novel/ambitious: treat
  the captured voice/command/session data living in the repo as a dataset;
  run k-means (flat labels) over it; abstract the clusters up to BRANCH-
  level groupings. I.e. discover the natural "task clusters" in how he
  works and map them onto git branches — auto-grouping sessions/commands by
  what they were really about, surfaced as branch-aligned labels. Pairs
  with R2 (auto-naming) — the cluster labels could NAME the sessions/
  branches. Pure-ish ML (k-means on feature vectors of command/voice data),
  candle or plain Rust; verifiable in-sandbox.

## How these slot onto existing cores (so they're not "thrown in")
- R1 → path_fuzzy + entity_extract + editor_voice + an SSH-target resolver.
- R2 → recall + session metadata; mental-map could reuse the cards canvas
  geometry or a docker-style name generator (pure, testable).
- R3 → the "diagnose slowness" probe (process wait-state) + transient
  bubble UI (NOT sidebar furniture).
- R4 → voice_intent + decision_cache + confusion_table (variation learning).
- R5 → a pure k-means over command/session feature vectors; labels feed R2.

## Open question for the next pass
The UI-clutter critique means the ACTIVE-ASSIST features (R3, debug-picker,
stuck-nudge) need a shared, disciplined surfacing model: one transient
"assist bubble" channel, triggered by detected state, never permanent. Worth
designing that ONE surfacing primitive before building the individual
assists, so they don't each get "thrown in where there was space."

---
## U1 status — DONE (pure core) v0.1.664
src/debug_detect.rs (11 tests): detect_debug_signal(line) → DebugSignal
(Crash/Panic/KernelFault/CoreDump/DebuggerStarted) from real crash/panic/
oops/heap-error/debugger-launch output; monitor_options(sig, last_cmd) →
pickable next-actions (re-run under gdb, open core, RUST_BACKTRACE, dmesg);
pick_option(voiced, options) fuzzy voice picker; bubble_kind dedups.
Validated against well-known output formats; ignores normal build output.
WIRING (build-gated): scan pty output lines through detect_debug_signal; on
a hit, route bubble_kind through assist_bubble.offer(); show the options as
a voice-pickable bubble; on pick, pty_write the (non-auto-run) command.
