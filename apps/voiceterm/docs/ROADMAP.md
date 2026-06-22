# Feature exploration roadmap (current — as of v0.1.485)

Strategy (user, this session): **feature exploration + hard testing
now; unification LATER.** The unified libraries will emerge from
seeing patterns across MULTIPLE apps, so abstracting prematurely
inside web2qml would destroy that signal. AI coding removes the
redundant build-system tax Pulsar suffered, letting the CORE get
integrated fast. So: explore broadly, test what's testable, keep new
integration points COHESIVE with existing ones (matching slugify,
reusing detection) — but don't force unification.

Guiding thesis (from kss → reVos → web2qml): **compress the dead
time between intent and result so the developer stays on content,
works on the move / on a phone, sustainably.** See
`design/voice_editing_and_unification.md` for the full lineage.

Surface rule: voice → one unified popup; spatial tasks (find/replace)
→ sidebar/overlay; both fine if context-driven with one config
model. Multiple SURFACES ok; multiple CONFIG MODELS = the Pulsar trap.

---

## DONE this session (471–485)

- Model selection: curated fast picks, RAM-aware recommendation
  (reserve-aware, dual-model-ready), auto-activate on download,
  setup-assistant pill wired at web-flow/history entry points.
- Voice debugging: repl_voice (gdb/lldb/python/sqlite/redis), wired
  into voice_parse_intent. Strict detection, injection-safe.
- Voice file editing: editor_voice (helix/vim), wired the same way.
- Self-surfacing voice grammars: voice_commands_available +
  truthful-by-test command lists (the man-pages principle).
- Knowledge-web: link_graph (backlinks + dangling targets),
  manpage_backlinks.
- Hardening passes on repl_voice / model_bench.
- model_bench: per-model benchmark store (playground backend).

---

## NEXT — ordered by value-per-effort given the thesis

### 0a. (ARCHITECTURE) Layered corpus model — vocabulary vs identity vs provisional
**Why it matters:** the user identified a real VOID — the corpus
conflates shared VOCABULARY with speaker IDENTITY, and has no
provisional/temporary-user layer or corpus-wide refinement pass. This
is also the proper HOME for the v0.1.496 fingerprint-identify fix
(accumulate-then-promote instead of greedy per-utterance attribution).
Full design in design/layered_corpus_model.md.
**Why NOT rushed:** stage 1 (an `owner` scope discriminant on
Fingerprint) touches the VERSIONED ON-DISK FORMAT (FP01…FP05). A
serialized representation is the hardest thing to change later, so
committing it before the open questions (general-aggregation policy,
promotion bar, decay TTL) are answered would calcify a wrong model.
**Blocked on:** user answers to the 3 open questions in the design
doc. Then stage 1 (FP06 additive field, old versions default scope)
is safe.

### 0. (PERF, next perf arc) Dirty-rectangle / per-row rendering
**Why:** v0.1.488–491 made the IDLE case free and DECOUPLED render
from the UI thread (reader renders off-thread, UI reads a snapshot,
shared-atomic generation, no grid-lock wait — race proven closed
under concurrency). But when something DOES change, the reader still
re-serializes the WHOLE screen via render_html. The next ceiling is
to re-serialize only the CHANGED rows.

**Why it's a real arc, not a quick win (don't half-build it):**
  - render_html currently returns one HTML string for the whole
    screen; per-row rendering changes its contract (emit per-row, or
    a {row_idx → html} delta).
  - The QML side must PATCH individual rows rather than replace the
    whole RichText document — that's the hard part and where the
    actual paint savings are. Co-design needed with the QML terminal
    view.
  - Row-dirty tracking in the grid is the pure/testable foundation:
    a `Vec<bool>` (or a dirty-row range) marked at every cell-write
    site (print, J, K, X, P, @, L, M, scroll…). Many scattered sites
    → invasive; do it carefully in one pass with tests per handler,
    not piecemeal.
  - Scroll is the tricky case: a scroll shifts ALL rows' content up
    by one, so naively every row is "dirty" — but visually it's a
    blit. A good impl special-cases scroll (shift the row→html cache)
    rather than re-rendering every row.

**Sequencing:** (a) grid row-dirty tracking + tests; (b) render_row
delta API; (c) QML row-patch. Each independently testable; (a) is
pure logic and safe to land first. Holds the "as fast as Konsole"
goal — this is where a helix redraw / fast scroll would actually
beat the current whole-screen re-serialize.

### 1. Tune voice verb tables to the user's real muscle memory
**Why first:** highest value, lowest effort, and it's the difference
between voice editing/debugging being a demo vs. something the user
actually dogfoods on their phone. The verb tables (editor_voice,
repl_voice) are reasonable starter guesses; the RIGHT tables are the
15-ish edits the user reaches for one-handed on the move. Pure,
testable, fast to iterate. **Blocked on: the user's actual list.**
Ask for it; until then the tables are placeholders.

### 2. Grammar-context labeling of voice recordings  ✅ DONE (v0.1.510 write, v0.1.511 read)
**Why second:** the user's own idea, and it's on-thesis (instrument
the corpus so the data self-labels — kss/reVos for your own voice).
Stamp each recording with the active grammar (shell/gdb/helix/web)
so the corpus becomes queryable + cross-referenceable ("show my
helix voice commands"). Infra mostly exists: CorpusEntry already
carries metadata (v0.1.325 stamps page-context); add a
grammar-context field on the record path + surface in list_recordings.
Also: a SMALL UI affordance showing the live grammar (the
voice_commands_available data) near the voice button — kept subtle
per the user. Touches the recording WRITE path (not pure) — scope
carefully, test the label-derivation logic standalone.

### 3. Saved / replayable workflow sessions (traveling coders)
**Why third:** directly serves the mobility thesis (social + roaming
coders, the user's words) and synergizes with everything above — a
"session" is a sequence of voice intents + context. Substantial:
needs a session record format, capture, replay, and a sharing story.
The capture/replay FORMAT + the intent-sequence model are testable
in isolation; the replay execution touches the dispatch. Big enough
to be its own arc.

### 4. reVos-style dead-air / filler-word cleaving  ⏳ CORE DONE (v0.1.512); pipeline integration deferred
**Why fourth (not lower — it's the most on-thesis, but heaviest):**
this is literally the reVos idea ported to the dev loop — compress
silence/filler out of voice transcripts. The SILENCE-DETECTION and
filler-classification logic is pure and testable (RMS curves, filler
dictionaries — kss did this in Python with pydub). But it's the
deepest integration (audio pipeline + whisper timing) and the
payoff overlaps with what whisper already does. Worth doing as the
"signature" reVos-lineage feature, but it's a real arc, and the
audio-pipeline parts can't be sandbox-verified.

---

## Standing / blocked (need the user or a real build)

- Selection off-by-one in helix (TODO_selection_off_by_one.md) —
  needs the user's captured selection escape sequences.
- Playground UI + run-test timing (model_bench backend is done) —
  needs the LLM worker timing path + blind QML.
- Wire voice_commands_available into a QML hint panel.
- Wire the install-card voice trigger (installCard.offer seam).
- End-to-end build+test of the llama path with a downloaded model.

## Deferred (from the old list, still valid)

- Dual-model parallel whisper inference (recommendation hook ready,
  v0.1.478).
- Streaming whisper.
- Voice-on-web, recent-fact lookup, workflow recorder (older notes).

## User testing pass — post-v0.1.519 (see docs/findings_post_519.md)

Filed from on-device testing. Triaged with per-item design docs:
- F1 [quick] homepage action missing in Page Commands → Actions.
- F2 [quick] bottom-bar Row never built (bottom_bar_model unwired).
- F3 [quick] model-setup pill CTA does nothing (wiring gap, F-class
  like the v0.1.502 method-name bug).
- F4 welcome-page (man/welcome) text CLUTTER/overlap — localize the
  layer first (manpage gen vs table→QML vs paragraph layout).
- F5 [perf] helix load+scroll slower than konsole →
  docs/design/terminal_render_perf.md (profile → dirty-rect stage-c).
- F6 [arch] big text files lag → docs/design/text_syntax_tree.md
  (chunk tree + lazy render; generalize note_cards splitter).
- F7 [bug] folder-picker cd injects into helix buffer; needs new-tab
  prompt + full tab model → docs/design/term_tabs_and_cd_flow.md
  (convergence of bottom_bar + GestureHost + voice hints).
- F8 [arch, BIG] multi-mic processing + dynamic pipeline →
  docs/design/multi_mic_pipeline.md (classify→tree→scheduler→VAD FSM→
  Kalman isolation; staged, mostly pure-testable cores).
- F9 [arch] voice-hinted on-screen buttons →
  docs/design/voice_hinted_buttons.md (reuses editor/repl matchers).
Suggested order in findings doc: F1, F3, F4, then F2+F7 (shared
gesture/live-surface), F9, F5, F6, F8.
