# TODO: "why so slow" process bubbles (R3) + spoken-word variation (R4)

## R3 — process-status / "why so slow" bubbles
### Priority: REAL but DEPENDENT on the assist-bubble primitive (TODO_assist_bubble.md)
Answers his literal complaint ("Why is it so slow? I don't know why").
When an op is slow or a process is stuck, surface (transiently) WHAT IT'S
WAITING ON: syscall / IO / lock / network / child process. The dev-loop
analog of the log-summary worker.
- Detection: sample the foreground/child process wait-state (e.g.
  /proc/<pid>/wchan, status, fd activity) when an op exceeds a latency
  threshold.
- Surface: an assist bubble ("waiting on network read, 3.2s") — TRANSIENT,
  triggered by detected slowness, never ambient.
- Pure slice: a classifier (wchan/status → human cause string) is pure +
  testable. The /proc sampling + bubble are wiring.
### Status: build the assist-bubble primitive first, then the wchan classifier (pure), then sampling+surface.

## R4 — spoken-word VARIATION command calling
### Priority: FRAMING of existing work (modest novelty, low risk)
Embrace that the same intent is said many ways ("run the tests" / "kick off
the tests" / "let's test this" / "cargo test it") → one action; and LEARN
his idiomatic phrasings over time.
- Leans on existing: voice_intent parser + decision_cache + confusion_table.
- New angle: a "variation learning" path — when a novel phrasing is
  corrected/confirmed to an action, store the mapping so next time it's
  direct. (decision_cache already learns; frame + extend it for phrasing
  variation specifically.)
- Pure slice: a normalizer that collapses known variation patterns
  (synonymous verbs, filler) before intent matching — testable.
### Status: not started. Lowest-novelty; do after the higher-value threads.

## Cross-ref

### DONE v0.1.663 — wait classifier (pure core, validated on real /proc)
src/slowness_probe.rs (11 tests): classify_wait(wchan, state) → WaitCause
(DiskIo/Network/Lock/Pipe/ChildProcess/IdleWaiting/RunningCpu/NotBlocked/
UnknownBlocked); cause_label (no kernel jargon); is_stuck(cause, elapsed)
(idle/normal never surface; CPU only on long burn; real blocks after 3s);
stuck_message + bubble_kind (dedups by cause for the assist-bubble).
- Validated on REAL processes in /proc: caught that `sleep`'s
  hrtimer_nanosleep is an INTENTIONAL wait (→ IdleWaiting, not "stuck") and
  that the modern pipe symbol is anon_pipe_read — both fixed + regression
  tests. Unit tests alone wouldn't have found these.
- Consumes the assist_bubble scheduler (v662): a stall offers bubble_kind;
  the scheduler's cooldowns keep it from nagging.

### STILL TO WIRE (build-gated)
1. Sample the foreground/child pid's /proc/<pid>/wchan + stat on a slow op
   (Linux I/O); classify; if is_stuck → assist_bubble.offer(bubble_kind).
2. The bubble shows stuck_message ("waiting on the network · 4s") + an
   optional action (e.g. show the blocked syscall / the process tree).
Both connect to his already-verbal narration (P8/P12) and the round-2
surfacing principle (R3 must not be furniture).
EOF

## R4 addendum — corpus-found false-positives (v0.1.657, from sim-corpus.sh)
Real Asahi narration surfaced concrete R4 targets (see corpus_findings.md):
- "run X"/"type X" over-capture mid-narration (~6/15 false positives):
  require utterance to START with run/type, or validate the tail looks like
  a real command. The single biggest cluster.
- "shut down" → Quit on narration: require app-directed phrasing.
- "let me ... open X" → SearchWeb: mid-clause "open" shouldn't web-search;
  route via OpenInEditor markers or leave as Freeform.
- TerminalCommand "again" handling: "run it again" → RerunLast not a literal
  command "it again".
Regression budget (narration ≤1.0% actiony) is enforced by sim-corpus.sh;
fixing the above should DROP the rate further — measure before/after.

### WIRED v0.1.668 — slowness probe → assist-bubble (2nd live consumer)
- pty_term.foreground_wait_state() → (wchan, state) for the foreground pgid
  via tcgetpgrp + /proc/<pgid>/{wchan,stat}; returns None when the foreground
  IS the shell (idle prompt never "slow").
- pty_read_screen: throttled ~once/sec, only when the last block is still
  running AND has been ≥3s, captures the wait-state (inside the &pty borrow)
  and defers classification to after the borrow (probe_slowness, &mut self).
- probe_slowness: classify_wait → is_stuck(elapsed) → bubble_kind through
  scheduler.offer() → assist_bubble_ready with stuck_message ("waiting on the
  network · 4s"), no action options. QML present() already handles the empty-
  options case (title + dismiss/snooze only).
- Compile-gate green; qt_struct clean. RUNTIME (does it sample the right
  pgid, fire at the right moment) needs the user's build.
Future: action options ("show blocked syscall", "process tree").
