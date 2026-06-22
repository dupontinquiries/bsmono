# Uncaptured TODOs — things discussed this session but never filed

These came up in conversation (mostly while debugging or shipping other
things), were acknowledged as real, and would read on revisit as "yeah,
that still makes sense as a TODO" — but were never written into a TODO
file. Capturing them so they survive context loss. Roughly ordered by
value.

---

## 1. Garbled log-summary output (REAL bug, pre-existing — observed v0.1.549 log)

When the local model (or the extractive summarizer) produced a page
summary, the output came out as noise: the user's log showed a "+4
Summary" block with fragments like "what is M4" interleaved with IRC
channel text (`(asani) (channel) (gauges) ... (oftc)`) — i.e. raw page
chrome bleeding into the summary instead of a clean abstract.

### v0.1.567 INVESTIGATION (verified by standalone test, root cause found)
Ran the real garbled input through extract_keywords in a sandbox
harness. FINDING: the summarizer is NOT broken. On prose it extracts the
right content words ("borrow", "checker", "compile"); on the IRC input it
extracts "asani, channel, gauges, irc" — chrome tokens — but ONLY because
the INPUT was chrome. The web page's nav/widget text was rendered into
the terminal and captured raw; the summarizer faithfully summarized the
noise. **The garbage is in the INPUT, not the extraction.**

So the real fix is UPSTREAM main-content extraction: before summarizing a
WEB PAGE (vs command output), strip nav/sidebar/widget chrome and keep
the article body — the readability/main-content problem. That's a live-
surface change to the web→terminal render-or-capture path (where the
summarizer gets its `output`), NOT a dumb_summary change. Needs the
running build to verify against real pages. Do it alongside the
log_summary→active_model rewire (TODO_active_model_registry.md).

### Fixed in v0.1.567 (the one real, testable sub-bug)
extract_keywords kept a word-final '.' ("performance.", "license.").
Now trim_end_matches('.') — verified to preserve versions (v0.1.567) and
paths (src/main.rs) which end in alphanumerics. Small quality win;
NOT the main garble fix (that's the upstream content-extraction above).

## 2. Download UX polish — no per-download feedback (led to the re-click bug)

The duplicate-download fights (fixed at the backend in v0.1.547 with the
in-flight guard) had a UX root cause that was NEVER fixed: the per-row
Download button doesn't visibly disable or show progress during an
active download, so the user reasonably re-clicks (and re-opens the
wizard and clicks again) because a multi-GB download gives no immediate
feedback. The backend guard now prevents the damage, but the UX still
invites the re-click.

- Two pieces, both live-QML (need testing):
  (a) per-row Download button DISABLES + shows a spinner/"downloading…"
      state while `_download_state.status == "downloading"` for that
      repo/file.
  (b) a live progress PERCENTAGE for the big GGUF (the backend has the
      bytes — download_to_file knows content_len + resume_from; would
      need to publish progress to a qt_property the row binds to).
- (b) is the bigger change (needs a progress channel from the download
  thread to QML). (a) is cheaper (bind button enabled-state to the
  existing _download_state).
- Files: qml/SetupWizard.qml + qml/ModelPicker.qml (Download rows),
  src/model_download.rs + src/main.rs (progress publish for (b)).

## 3. gdb cursor-alignment — AWAITING DATA, then fix (v0.1.563 instrumented)

Not forgotten — this is the one explicitly waiting on the user. v0.1.563
added the `[cursor-align]` diagnostic (logs charW / calibratedCharW /
advanceWidth / cursorX when the cursor is at col>=30). The hypothesis is
charW calibration drift accumulating across columns (shows on gdb's long
lines). NEXT: user opens gdb, moves cursor past col 30, grabs the
`[cursor-align]` lines; then the fix is one calibration number, applied
with confidence. Do NOT blind-fix the calibration before the data.

## 4. helix selection off-by-one — narrowed, capture path now exists (v0.1.565)

See TODO_selection_off_by_one.md (updated v0.1.565). Parser ruled out
(SGR 7/27 handled correctly); bug is render geometry. WEB2QML_TAP=1 now
dumps the raw byte stream to /tmp/web2qml-tap.log so the next session can
compare the inverted-row span to what render_html emits. Listed here only
as a cross-reference so it's not lost.

## 5. Alt+Tab may be WM-grabbed (v0.1.561 caveat)

The Alt+Tab fix (v0.1.561) is correct for when the event reaches the app,
but most desktops' window managers grab Alt+Tab first. Added a `[keys]
modified Tab reached app` breadcrumb. If the user tests and Alt+Tab never
logs that line, the real follow-up is: either pick a different chord for
the zsh-autosuggestion accept (one the WM doesn't grab), or document that
the user must reconfigure their WM. Decide once the breadcrumb data says
whether the event arrives.

---

## Cross-reference: already-filed deferrals (NOT duplicated here)
- TODO_coalesce_wiring.md — wire llm_coalesce to the 4 live LLM callers.
- TODO_active_model_registry.md — rewire log_summary to active_model()
  (also fixes the Ollama churn; see item 1 for the related quality fix).
- TODO_chat_local_wiring.md — chat UI done (v0.1.553); per-model chat
  templates still open.
- TEST_ON_RETURN.md — the dependency-ordered verification checklist for
  the v0.1.550+ unverified stack.
- TODO_selection_off_by_one.md — see item 4.

---

## 6. Graph-onboarding arc (STARTED v0.1.568) — welcome man page as graph entry node

User's idea: redo onboarding to start with a single welcome man page ON
the graph view, so first contact teaches the spatial layout by being in
it; man pages double as help AND as the model's context for "what can I
do"; every step has clear voice forward/back. Bootstrapping
familiarization through the interface itself.

### Done v0.1.568 (the navigation-model-agnostic foundation)
- Rewrote welcome.md as the graph entry hub: voice-friendly, "go home"/
  "back"/"forward" vocabulary, links via wikilinks to topic nodes.
- New topic pages: getting-around.md (movement vocabulary),
  capabilities.md (the "what can I do" page).
- Registered all + a voice-setup alias (→ onboarding.md, whose H1 is
  already "Voice Setup") so [[Voice Setup]] resolves. All wikilinks
  verified to resolve to registered pages — no dead nodes.

### PENDING — needs the user's two design answers + live build
The ask_user_input questions were posed (entry-node behavior: single-
expanding vs few-visible vs live-try-it; nav model: graph-edges vs
linear vs both). Build the actual first-launch behavior once answered:
- onboarding_should_auto_trigger currently gates on "no speakers + no
  stamp". Rewire first launch to DROP THE USER ON THE GRAPH VIEW with
  the welcome node focused (instead of / before the current voice-reg
  dialog).
- Render the welcome manpage AS a graph node and its wikilinks AS edges
  (the link_graph / manpage_backlinks infra already computes these).
- Wire the voice/tap/type "back"/"forward"/"go home"/"what can I do"
  verbs to graph navigation (some may already exist — audit
  voice_parse_intent for nav verbs first).

### THE BOOTSTRAPPING PIECE (the user's deeper point)
capabilities.md is currently curated. The vision: generate it LIVE from
the app's current context (voice_commands_available already does this
per-context for REPLs/editors — generalize it to a whole-app
"what can I do" that feeds BOTH the capabilities node AND the model's
context). This makes the man-page corpus the model's self-knowledge:
the same text that onboards the user tells the local model what the app
can do. Big, high-value, needs design + live build.
