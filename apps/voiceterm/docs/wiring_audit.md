# Wiring audit — unwired capabilities ("free upgrades sitting in the code")

User's filing: capabilities built for one feature that were never
ROUTED to other features that could use them. Method: scan all
`pub fn` (695 total) for those with ≤1 caller across src/, then
distinguish GENUINE clean drop-ins (low-risk, worth wiring now) from
APPARENT ones that actually need deferred architecture.

## WIRED THIS PASS (v0.1.500)

### foreground_is_pager — DONE ✅ (the clearest instance of the pattern)
`pty_term::foreground_is_pager()` (checks man/less/more/view/most/pg)
had ZERO callers, while the pager-name list was duplicated VERBATIM
in QML **four** times (main.qml ~6067, ~6133, ~7443, ~16364), each
re-implementing the membership check. One QML comment even referenced
"the backend's foreground_is_pager logic" while still hand-rolling
the list beside it.
**Fix:** exposed `term_foreground_is_pager()` qt_method routing
through the single source of truth; replaced all four QML duplicates.
Pager-name list now lives in ONE place. Adding a pager benefits every
site. (qt_* 427 → 428.)

## CANDIDATES — APPARENT free upgrades that need architecture (do NOT rush)

### phonetic::best_match — NOT a clean drop-in
0 callers; looks like it should feed speaker `identify()`. But it
returns COMMAND-LABEL matches (vocabulary the FP_DB was trained on),
not speaker identity — answers "what was said," not "who said it."
Wiring it into identity needs the v0.1.498 embedding work + the
v0.1.497 layered-corpus architecture. Tracked there, not a quick win.

### phoneme_db::lookup_by_text — RESOLVED: latent precise API, NOT a misrouting
0 callers. Sibling `lookup_any(text)` (scans ALL manpages) IS used
(main.rs:5351, in paragraph_selected). Initially looked like the
caller should use the precise `lookup_by_text(manpage, text)` instead
— but checked the call site: paragraph_selected does NOT have the
manpage name (only id/text/content_type/raw_content), and the code
deliberately uses lookup_any precisely to DISCOVER which manpage a
paragraph came from. So lookup_any is correctly chosen; lookup_by_text
is latent precise API for a manpage-aware caller that doesn't exist
yet. NOT a free upgrade — leave it. (Finding: the distinction between
"weaker companion wrongly used" and "precise variant with no current
caller" matters — don't wire the latter.)

## CANDIDATES — GENUINE unwired but NOT pure free upgrades (need empirical tuning)

### phonetic style_id same-style match weighting — genuine, but changes recognition
classify_style() runs at fingerprint creation and style_id IS read —
but only for DB CAPPING (MAX_PER_STYLE per label+style, phonetic.rs
~215). The self-confessed TODO (phonetic.rs:197 "matcher can use it
later to weight same-style fingerprints higher") is genuinely
unwired: the DTW SCORING loop does not use style_id. This IS a real
unwired capability — BUT wiring it changes match SCORES (which command
wins), a behavior change to recognition. classify_style is
intentionally conservative (most utterances = style 1/normal), so a
same-style boost is near-no-op for normal speech and only nudges
whispered/loud/fragmented cases. Plausibly safe as a small tiebreaker,
risky as a strong weight — needs REAL-AUDIO tuning to set the weight,
so it's NOT a pure drop-in like the pager list. Leave until there's a
way to validate the weight on real utterances. (Contrast with
foreground_is_pager: that was behavior-PRESERVING centralization;
this would be a behavior CHANGE.)

## CANDIDATES — still to investigate (0-caller pub fns from the scan)

get_screen_text, render_markdown, render_workspaces_page,
multilingual_model_available, parallel_llm_count, is_built (acoustic
index — likely a status reader for QML that QML doesn't call yet),
take_dirty_rows (intentionally additive, v0.1.494 — NOT unwired, its
consumer is deferred dirty-rect stage c), is_playing_back,
should_drain, force_reload.

NOTE several 0-caller results are INTENTIONAL (additive groundwork
whose consumer is deliberately deferred — e.g. take_dirty_rows,
speaker_embed::batch_cosine_distance). The audit must not "wire" those
prematurely; that would defeat the staging discipline. Only wire when
there's an EXISTING weaker/duplicated consumer that should route
through the capability (the foreground_is_pager case).

## Method for future audit slices

1. 0-1 caller `pub fn` → candidate.
2. Is there an EXISTING consumer doing the same thing more weakly or
   by duplication? → GENUINE drop-in, wire it (centralize the source
   of truth).
3. Does wiring require new architecture / a format change / a
   not-yet-built consumer? → APPARENT; leave it, track in the
   relevant design doc.
4. Is it deliberate additive groundwork (staged)? → leave it.
