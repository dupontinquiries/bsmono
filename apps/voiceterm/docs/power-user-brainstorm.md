# Power-user avenues — brainstorm (v0.1.637)

A fresh slate of power-user directions, grounded in what the architecture
ACTUALLY supports now (not wishlist features that fight the design). Each
is tagged by how much rides on voice/multi-speaker/dialect work that
already landed this arc, and whether the first slice is PURE (sandbox-
verifiable) or needs the build.

Anchored to the vision: voice-first, local, ADAPTS TO THE USER, terminal+
web as peers, "thinking surface not workshop." A power-user feature earns
its place if it makes the system more of a thinking surface OR leans into
the adaptive-to-you angle that a generic terminal can't copy.

---

## A. Leverage what JUST got built (low new surface, high payoff)

### A1. zellij assisted layer — "focus the build pane" (★ best near-term)
The zellij CLI returns structured JSON (`list-panes --json`). Parse it into
a model and you unlock things KEYS CANNOT DO: "focus the pane running the
tests", "what's in pane 3", "close the dead panes" (panes whose process
exited — list-panes reports exited status). This is the differentiated
win — a keys-first power user still reaches for voice for QUERIES over
their layout. First slice: pure parser for list-panes JSON → PaneModel +
a phrase→pane resolver (reuse the ui_landmarks scorer!). Builds on v634.

### A2. nu lexicon + dialect routing — speak structured pipelines
shell_dialect (v636) detects nu. The payoff: "show rows where status is
failed" → `where status == "failed"` in nu vs a grep pipe in bash. Voicing
a STRUCTURED query is where nu+voice beats typing — you don't remember the
column syntax, you say the intent. Pure: nu_lexicon templates + route
table_nlp by dialect. Already scoped in TODO_nushell_overlap.md.

### A3. tiered speaker actions — "pair-programming with guardrails"
sudo_shim (v633) has Owner/Trusted/Guest/Observer tiers. Power-user
framing: invite a colleague's voice as Guest — they can navigate, search,
run read-only commands, but elevation/destructive ops are denied + logged.
The audit log already attributes per-command. This makes the multi-speaker
story a REAL collaboration tool, not a demo. First slice: the Tier field
on Speaker (pure) + the gate wiring (build-gated, careful — privileged).

---

## B. The adaptive-to-you angle (the moat a generic terminal can't copy)

### B1. Personal command lexicon / voice macros
"deploy staging" → a multi-step sequence the USER defined by voice once.
The workflow recorder exists; the gap is naming a recorded sequence with a
spoken trigger and fuzzy-matching it later (phonetic DB already does this
for commands). Power users live on aliases — this is voice-native aliases
that survive across shells (bash alias doesn't help you in nu; this would).
Pure: a trigger→sequence registry + phonetic match; reuse decision_cache.

### B2. "What did I do last time" — session memory as a verb
Conversation history + workflow recordings + block history already persist.
Surface them as a query: "how did I fix this last week", "what was that
docker command", "show me the session where I set up the nginx config".
This is the thinking-surface vision realized — the terminal REMEMBERS with
you. First slice: pure ranker over the existing archive.db + recordings
(keyword + recency), like search_suggest. Big payoff, mostly pure.

### B3. Correction-learning surfaced — "you keep saying X, did you mean Y?"
The confusion table + token stats learn corrections silently. Power-user
surfacing: a tiny "voice health" panel — your most-corrected phrases, your
recognition rate trend, suggested re-enrollments. Turns the adaptive black
box into something a power user can TUNE. Pure: stats aggregation over the
existing token_log + confusion table.

---

## C. Thinking-surface deepening (web+terminal as peers)

### C1. Output → web as a verb
Terminal output is already interactive (smart tokens). Next: "search this
error", "open the docs for this command", "what's this CVE" — pipe a
selected block to a web search / man page / the AI summary, in the peer
browser tab. The click-search planner (v626) is the selection→action brain;
this is the terminal-side mirror. Pure: an intent classifier (error vs
command vs url vs identifier) → target verb.

### C2. Man-page / --help as a first-class panel
phoneme-db already loads man pages. Power move: "explain these flags" on a
command you're typing → inline flag glossary from the man page, voiced or
shown. Helps the "learning an unfamiliar tool" use case in the vision.
Pure: man-page flag parser + a phrase→flag matcher.

### C3. The graph view as a command/knowledge map
The cards/graph view exists (pages, files, chats, workflows as nodes).
Extend to terminal: recent commands, error→fix pairs, and project files as
nodes you can voice-navigate ("show me everything about the nginx setup").
The thinking-surface made spatial. Larger effort; the node model + edge
canvas already exist to build on.

---

## D. Ambient / live-detection avenues (lean into always-listening)

### D1. Proactive long-command notify + voice followup
Long-running commands already get completion notifications. Add: when one
finishes, a voiced "build done, 0 errors" or "tests failed, 3 failures" —
parsed from the output, spoken back. Hands-free monitoring while you do
something else. Builds on the block/notify infra + a cheap output classifier.

### D2. "Narration mode" for teaching/demos
The vision names demos/teaching explicitly. A mode where every command +
its plain-language intent is captured as a clean transcript/asciinema-like
artifact — exportable. Leans on the workflow recorder + block history.
Export-over-lock-in (vision principle 3). Mostly assembly of existing parts.

---

## Honest filter (what NOT to chase)
- Plugin/extension model, full tmux parity, SSH session mgmt: large, and
  they pull toward "workshop" (execution) not "thinking surface". The doc
  already lists these lower-priority; keep them there.
- Anything that needs cloud: violates local-first.
- Generic LLM-agent features (autonomous multi-step): the vision is
  voice-STEERED, user-in-the-loop. Don't drift into agent-land.

## Suggested order (my read)
1. A1 zellij assisted parser (pure, differentiated, builds on fresh work).
2. B2 "what did I do last time" ranker (pure, big thinking-surface payoff).
3. A2 nu lexicon (pure, already scoped).
4. B1 voice macros (pure-ish, power users love aliases).
Then the build-gated wiring for whichever resonates.

These are SUGGESTIONS — pick the ones that match where you want the product
to go; I'll deepen any into a real TODO + start the pure slice.
