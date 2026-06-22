# Findings & triage — user testing pass (post-v0.1.519)

A full dump of issues + feature requests from on-device testing. This
doc TRIAGES everything: what it is, whether it's verified, root cause
where known, fix size, and a pointer to a design doc for the big arcs.
NOTHING is implemented yet — filing first per the user's instruction.

Legend: [BUG] regression/defect · [GAP] built-but-unwired · [PERF] ·
[ARCH] new architecture arc (own design doc).

---

## QUICK WINS (small, verified, low-risk) — do these first

### F1 [GAP] No "homepage" action in Page Commands → Actions
DONE (v0.1.525). Added a "Go to homepage" row (url:_homepage) after
"Reload page" + a dispatch case calling showHomepage() (the same
function the home ToolBtn at qml ~8003 and the `x` shortcut use),
switching to the browse view + closing the drawer. Structurally
verified (lint + backend-check clean, showHomepage/swipeView/
browserDrawer all in scope); render/tap confirmation is on-device.

### F2 [GAP] Bottom bar unchanged — model exists, Row never built
VERIFIED. bottom_bar_model() (backend, v0.1.508) is referenced in ZERO
qml files. The dynamic model (bottom_bar.rs, 7/7) + placeholder feed
exist but the visible QML Row was always staged. "Looks the same"
because nothing renders it yet. FIX SIZE: medium — build the Row of
GestureHost buttons bound to bottom_bar_model(), reuse SlidePopup.
This is live-surface work (needs the running build to lay out). See
docs/design/unified_bottom_bar.md (already has the plan).

### F3 [BUG] Model-setup pill call-to-action does nothing on click
DONE (v0.1.534, needs device render confirm). Root cause found: the CTA
called `settingsPopup.open()` (gated on `typeof settingsPopup !==
"undefined"`) — dumping the user into settings to hunt for the
ModelPicker, not a focused flow; if settingsPopup wasn't resolvable
from scope it silently did nothing. FIX: built qml/SetupWizard.qml — a
dedicated dialog reading setup_recommendation_json, offering (1) one-tap
download of the ★ RAM-aware recommended model, (2) browse-all (wraps
the existing ModelPicker), (3) enable-Ollama escape hatch; polls
download_progress_json, activates via set_llama_model_path on
"done:<path>". The CTA now opens setupWizard. Caught 3 JSON field-name
mismatches against the real backend before ship (curated uses label/
blurb/repo_id/filename not name/note/repo/file; completion is
"done:<path>" in status, not a done/path field). qml_lint + backend-
check clean. Render/tap is the on-device confirmation.

### F4 [BUG/PERF] Clutter on web2qml://man/welcome (see screenshot)
INVESTIGATED + CANDIDATE FIX (v0.1.525). welcome.md HAS a markdown
table (the Action|How "Quick reference"), rendered via manpages::
to_qml → qml_gen::emit_adaptive_table (the adaptive wide/narrow table).
The overlap in the screenshot = the table's outer Item reporting ~0
height during the initial layout pass, so the page's host Column draws
the FOLLOWING prose on top of the table. Root: the non-Loader (small-
table) path bound the Item height to `_wideCol.height` ← `_wideInner
(ColumnLayout).height`, which can be 0/stale before the layout engine
runs (a binding-cycle window: Item sizes to the ColumnLayout which
sizes to available space inside the Item).
FIX APPLIED: bind to `_wideInner.implicitHeight` (computed eagerly by
the layout, like the known-good patterns at qml_gen.rs:1584/2724)
instead of `.height`. Matches existing code, can't regress.
HONEST CAVEAT: emit_adaptive_table is SHARED with the webpage table
path, which renders fine — so this zero-height window may not be the
ONLY cause, or the welcome-specific trigger could be the host Column's
stacking. NEEDS ON-DEVICE CONFIRMATION: does the welcome table still
overlap after this? If yes, the next suspect is the manpage host
Column (vs the webpage host), not emit_adaptive_table. Do NOT assume
fixed until seen.

---

## PERF (needs profiling on device, likely real work)

### F5 [PERF] Helix load + scroll slower than konsole
VERIFIED on device (user). Two parts: (a) initial helix launch
slower; (b) scrolling INSIDE the helix panel laggier than konsole.
This is the terminal render path (the v0.1.490–494 decouple +
dirty-rect arc targeted exactly this). Hypotheses to profile:
  - the off-thread render still produces a full-grid HTML string per
    frame for alt-screen apps (helix repaints fully); dirty-rect
    stage-c (row cache + QML patch) was DEFERRED — this is likely the
    biggest lever and is now justified by a concrete complaint;
  - QML Text/RichText reflow of the whole terminal HTML on each
    snapshot vs konsole's GPU glyph atlas;
  - alt-screen (helix) marks all rows dirty every frame, so the
    conservative dirty-rect path gives no win there.
ACTION: profile helix scroll frame time; if it's the QML HTML reflow,
dirty-rect stage-c (patch only changed rows into the QML view) is the
fix. See docs/design/terminal_render_perf.md (NEW).

### F6 [PERF/ARCH] Large text files (12k lines / 99k words) lag on render
VERIFIED. Dragging a big .txt/.md/book into the browse tab renders the
WHOLE thing eagerly → severe lag. Needs: a syntax/chunk tree +
lazy/subsection loading + processing-budgeted chunk sizing + a better
QML scroll view. This is an ARCH arc — see
docs/design/text_syntax_tree.md (NEW).

---

## BEHAVIOUR BUGS (interaction model)

### F7 [BUG] Folder-picker `cd` injects into helix buffer instead of new tab
VERIFIED on device. Running `cd` via the folder picker while helix is
open in the term tab just types the cd into helix's buffer (wrong) —
it should NOT hijack the running program. Desired behaviour (user):
  - offer to open a NEW term tab (confirm/deny via button AND voice);
  - term tabs then openable by pressing the term tab button;
  - sticky-on-mouse-click; cycle-on-tap on touchscreen; tap-and-hold →
    the GestureHost/SlidePopup popup listing tabs with NLP buffer
    summaries in a condensed list.
This ties into the bottom-bar/tab work (F2) and the tab-popup is the
same GestureHost+SlidePopup primitive. See
docs/design/term_tabs_and_cd_flow.md (NEW).

---

## NEW ARCHITECTURE ARCS (own design docs, substantial)

### F8 [ARCH] Multi-microphone processing & dynamic pipeline
The big one. Support many mic topologies (phone top+bottom phase-
cancel; studio room mics; separate per-room mics for a podcast), auto-
classify the setup via fingerprinting, build a pipeline graph, Kalman-
filter for speaker isolation when the array allows, and a RESOURCE
SCHEDULER (RAM, N CPUs, N mics → how many whisper instances / async
insertion strategy), re-evaluated on a VAD-gated FSM every 1500ms with
a 1500ms raw buffer. Screen-off interacts (studio/broadcast mode keeps
mics hot). See docs/design/multi_mic_pipeline.md (NEW).

### F9 [ARCH] Voice-hinted on-screen buttons
Buttons (accept/cancel, browse-tab buttons) take voice hints: when the
voice capture matches a button's context, the button animates to show
it's the target. Generalizes across the confirm/deny dialogs (incl.
F7's tab prompt) and the browse-tab controls. See
docs/design/voice_hinted_buttons.md (NEW).

---

## SUGGESTED ORDER

1. F1 homepage action (tiny, isolated win).
2. F3 model-setup CTA (small wiring fix — same class as v0.1.502).
3. F4 welcome-page clutter (localize first; likely one layout bug).
4. F2 bottom-bar Row + F7 tab popup (shared GestureHost work, live
   surface — needs the running build).
5. F9 voice-hinted buttons (builds on F2/F7's dialogs).
6. F5 terminal render perf (profile → likely dirty-rect stage-c).
7. F6 text syntax tree (arc).
8. F8 multi-mic pipeline (biggest arc; phased).

Most of F2/F4/F5/F7 need the RUNNING BUILD to verify fixes (layout,
frame timing, interaction) — they're live-surface, not sandbox-
verifiable. The design docs scope each so the on-device work is fast.
