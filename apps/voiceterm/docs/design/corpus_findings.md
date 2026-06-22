# Real-speech corpus findings (Asahi kernel-dev transcripts) — v0.1.657

## Corpus
Mined from 3 Asahi kernel-dev livestream transcripts (~43k words) into two
fixtures (tests/corpus/):
- asahi_commands.txt — 153 command-like utterances (imperatives, "let's X").
- asahi_narration.txt — 3757 thinking-aloud utterances (declarative).
Real spontaneous power-user speech — the best test fuel available. Run via
scripts/sim-corpus.sh (stages voice_intent, no model/audio needed).

## Headline metric: false-positive rate on NARRATION
A kernel dev thinking aloud must NOT trigger actions. Result: **0.4%**
(15 / 3758) of narration routes to an actiony intent. Low — the parser
mostly leaves narration as Freeform. The harness asserts a ≤1.0% budget so a
regression that pushes natural speech into actions trips CI.

## Real bugs found (the 15 narration false-positives cluster)
1. FIXED v0.1.657 — FileAppend path hallucination. "add those to the wrong
   place" → FileAppend("~/the_wrong_place.txt", "those"). The fallback
   turned ANY "add X to Y" into a write. Now only known targets (notes/todo/
   journal), explicit paths, or "<name> file/.txt" append; else fall
   through. (A destructive misfire — highest priority, fixed + regression
   test.)

## Known remaining false-positives (TODO — lower stakes, logged not fixed)
2. "run X" / "type X" over-capture: "just run this on the host, effectively"
   → TerminalCommand("this on the host, effectively"). The run/type arms
   grab the whole tail as a command even mid-narration. Should require the
   utterance to START with run/type (not contain it) or look like a real
   command token. ~6 of the 15.
3. "shut down" → Quit: narration about shutting something down quits the
   app. Should require a more app-directed phrasing ("shut down the app" /
   "quit"). 1 occurrence.
4. "let me ... open X" → SearchWeb(X): "let me open this up in ghidorah" →
   SearchWeb. Mid-sentence "open" inside a "let me" clause shouldn't be a
   web search. Tie-in to the editor-open work (could be OpenInEditor with
   the right markers).

## Command-side findings (asahi_commands.txt)
Only 11% (17/154) of "command-like" utterances route to an action — because
most are vague spoken intent ("let's see", "let's do that", "let's go with
that") with no concrete object, which SHOULD stay Freeform. Of those that
route, TerminalCommand parsing is sloppy: "run it again, and continue" →
command "it again, and continue" (should detect "again" → RerunLast). Logged
for the R4 spoken-variation work (TODO_slowness_and_variation.md).

## Why this matters
This is the R4 "spoken variation" concern made measurable. The corpus gives
a regression budget AND a prioritized real-bug list, from real power-user
speech rather than invented examples. Re-runnable any time via
scripts/sim-corpus.sh.

## v0.1.658 — corpus-driven UX refinement: 0.4% → 0.11% false positives
Worked the logged false-positives, measuring the narration FP rate before/
after each fix (the corpus as the objective). 15 → 4 misfires (and one of
the remaining 4 is actually CORRECT — " let's run it again" → RerunLast).

Fixes (all with regression assertions + verified legit cases still route):
- **run/type prose guard** (biggest cluster, ~8 → ~1): added
  looks_like_command() — a "run X"/"type X" tail must be short (≤5 words),
  not start with a prose word, contain no prose markers, and have no
  comma-clause. So "basically run this on the host, effectively" (after the
  hedge-stripper makes it "run this on the host, effectively") no longer
  routes to a literal command, while "run cargo test"/"run git status"
  still do.
- **run it again → RerunLast** (true-positive fix too): "run/let's run it
  again" now means rerun, not a literal command "it again".
- **bare number guard**: "type 2" (narration) no longer a command.
- **Quit narrowed**: dropped "shut down"/"shutdown"/"leave"/"close" — a
  kernel dev says "shut down" about a machine constantly. Requires app-
  directed phrasing ("quit", "close the app", "shut down the app").
- **recall demonstrative guard**: "what did i do with this one" (pronoun-
  only query) no longer a recall search; a real query names a topic.

Budget tightened in sim-corpus.sh: narration ≤0.3% (was 1.0%), locking the
gain so it can't silently regress.

### Remaining (4 FPs, deliberately NOT chased — over-fit risk)
"save as python file" → WorkspaceSaveAs, "let me ... open ... ghidorah" →
SearchWeb, "i need to write a function" → Dictate, " let's run it again" →
RerunLast (correct). Fixing the first three risks breaking legit phrasings
on a single corpus; better to wait for a second user's data before tuning
further. Honest stopping point.
