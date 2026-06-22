# Wiring audit (v2) — built-but-unsurfaced backend features

Status: **audit findings (v0.1.514), doc-only.** A follow-on to the
v0.1.500–502 wiring audit. That one found method-level dead wiring (a
button calling the wrong name). This one is feature-level: backend
capability that's fully built + exposed as qt_methods but reachable
from NO QML — whole features sitting dark.

## Method

- Extracted all 317 qt_method names; diffed against the 296 distinct
  `backend.X` references across ALL qml/ files.
- Filtered to methods with exactly 3 Rust references (declaration +
  Default::default() init + fn definition = ZERO actual callers) AND
  absent from every QML file.
- Result: **43 truly-unwired methods**, clustering into coherent
  features.

## Findings, ranked by "is this an intended-but-missing UI?"

### A. Chat feature — ENTIRELY unwired (8 methods, src/chat.rs 493 LOC)
chat_new / chat_send / chat_list / chat_messages / chat_select /
chat_export_md / chat_has_claude / chat_has_openai. A complete chat
backend: conversations, message send via ureq, markdown/JSON export,
Claude+OpenAI provider availability. NO QML surface at all. This is
the headline finding — a real feature with zero UI.
  - Action (on device): build a chat drawer/panel binding to these.
    The backend is sound; this is pure live-surface work (layout +
    the running build to verify). Not done blind here.

### B. Filler handling — unwired, and it OVERLAPS dead_air's "Uh-ware"
voice_is_filler / voice_mark_filler / voice_unmark_filler /
voice_strip_fillers (+ filler_words.rs corpus-learned filter +
token_stats::strip_fillers). IMPORTANT CROSS-REFERENCE: the v0.1.512
dead_air design doc listed text-level filler removal ("Uh-ware") as a
FUTURE follow-up — but a corpus-LEARNED filler filter already exists,
fully built, just unwired. Do NOT rebuild it in dead_air; wire THIS.
  - Action: surface filler mark/unmark in the corpus/voice UI; feed
    voice_strip_fillers into the transcript path. dead_air handles the
    ACOUSTIC dead air; this handles the TEXT fillers — complementary.

### C. Live voice-grammar display — unwired (the grammar-label payoff)
voice_commands_available + current_grammar_label. The v0.1.510 grammar
roadmap item wanted "a SMALL UI affordance showing the live grammar
near the voice button." current_grammar_label() is built (and now
stamps recordings) but nothing DISPLAYS it. voice_commands_available
returns the available commands for the active grammar — also undisplayed.
  - Action: a subtle label near the voice button bound to
    current_grammar_label() (kept subtle per the user).

### D. Smaller unwired clusters (likely intended, lower priority)
- palette_* (create/add/save, 3): a saved-command-palette feature.
- niftyplot_* (suggest/scope_at, 2): plot suggestion — may be staged.
- workflow_* (create_overwrite/is_recording/list): the saved-sessions
  roadmap item 3 has partial backend already.
- speaker_* (is_allowed/is_deep_muted/remove, 3): speaker management —
  likely partially surfaced elsewhere via different method names.
- island_* (kind/src), file_* (is_image/resolve), term_branch_tree,
  manpage_backlinks, model_bench_comparison_json, etc.: mostly
  latent helper APIs, not necessarily missing UI.

### E. Deliberately staged (NOT findings — correctly unwired for now)
bottom_bar_model (v0.1.508 placeholder, awaiting the Row),
corpus_start_dual / voice_listen_dual_running (dual-capture, staged).

## Why doc-only

Every action above is live-surface QML work (a drawer, a label, a
panel) that needs the running build to lay out + verify — the same
discipline held all session. The VALUE here is the map: it tells the
user which backends are ready to surface (chat is the big one) and,
crucially, stops duplicate work — dead_air was about to grow a filler
stripper that already exists (finding B).

## Recommended order when on-device
1. Filler wiring (B) — small, complements the just-shipped dead_air,
   reuses an existing learned filter.
2. Live grammar label (C) — tiny, completes the v0.1.510 item.
3. Chat panel (A) — biggest payoff, biggest surface.
