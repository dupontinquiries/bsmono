# Backlog — deferred ideas (parked, not dropped)

Ideas proposed but deferred. Revisit when relevant.

## Read-aloud / utterance-DB cluster (v0.1.701 cores landed; runtime parked)
The speech_plan blend (TTS + recorded clips) is built + tested; these would
make it audible and richer:
1. **TTS/clip audio player** — runtime `speak_segments`: shell to a TTS engine
   for `T` segments, play clip WAVs for `C` segments in sequence. Makes "read
   this section" actually produce sound. (Hooks already guarded in QML:
   speak_segments / speak_clip / speak_tts.)
2. **Clip capture for the utterance DB** — a "record this phrase" flow saving
   a WAV into ~/.local/share/web2qml/clips/ named after the phrase, so the
   blend has real recordings (directly feeds speech_plan realism).
3. **Reader codemap toggle button** — a 🗺 button in the browser toolbar (the
   reader codemap currently opens via the "o" key / voice-jump only).
4. **Read-aloud highlight follow** — karaoke-style: highlight each segment's
   text as it plays, reusing section + mdhdg anchor machinery.
5. **Graph breadcrumb voice nav** — "go back" / "back to <node>" walks the
   focus breadcrumb by voice (complements the clickable strip).

## Other parked (earlier rounds)
- path_complete.rs — WIRED v0.1.720: path_complete_query(prefix, current_file,
  link_style) qt_method ranks the path_fuzzy index by fuzzy + proximity, returns
  "insert\tdisplay" rows. link_style → markdown-link form (strip .md, ./-prefix).
  Verified ("par" → parser.rs first). REMAINING: a QML completion popup trigger
  (e.g. on `[[` in notes or a key in the editor) — backend ready.
- row_cache::classify_change — foreground cursor-blink re-render suppression.
- prezi_walk camera drive — fit-big-graph camera, QML drive pending.

## Voice corpus / TTS — forward ideas (parked v0.1.705)
- **TTS-unreliable skiplist auto-feed** — voice_tts_compare.sh already emits a
  skiplist; wire voice_tts_harness.sh to consume it automatically (skip phrases
  TTS mangles). (#3 produces it; the consume side is the follow-up.)
- **Hybrid / multi-speaker synthesis** — learn which TTS engines / voice-changer
  plugins produce convincing distinct speakers, to synthesize multi-speaker
  "meeting" audio for diarization + speaker-ID testing without recording many
  real people. Evaluate piper voices, espeak-ng variants, RVC-style changers.
- **Large default CI corpus** — grow a committed default corpus (beyond the
  user's personal recordings) so Layer 3 has a baseline that runs in CI for
  everyone, not just locally. Needs licensing-clean audio (e.g. read public-
  domain text) + a size budget decision.

## qml_gen output-string accumulation (deferred from v0.1.728 compiler audit)
The recursive generate_safe_qml_with_stats builds QML by pushing each child's
result into a Vec<String> and joining — many small heap allocs per node. The
measured-clean win (tag-gating the tracking-wrapper block) shipped in v728. The
larger refactor — thread a single `&mut String` output buffer through the
recursion instead of returning Strings — would cut the allocation count
substantially but touches ~5600 lines of recursive builder and every emit_*
helper. High risk, needs its own focused pass with the compile gate + a golden-
output diff harness (compile a fixed corpus of pages before/after, assert
byte-identical QML). Park until a dedicated session.

### UPDATE (v0.1.730): the golden-output harness now EXISTS
scripts/qml_golden.sh stages qml_gen + its dep closure (ad_filter, table_ops,
anim_detect, layout_detect) and diffs generate_safe_qml output over 5 fixtures
(article, tracking, forms, nested_deep, edge_cases) against committed goldens in
tests/golden/expected/. The output-string refactor is now SAFE to attempt:
run `bash scripts/qml_golden.sh` before, do the refactor, run it after — any
byte diff is a behavior change. Proven to catch regressions (verified by
injecting a fillMode change). Remaining: the refactor itself.

### UPDATE (v0.1.731): contained allocation cut done; full threading still parked
The hot child-collection loop's per-child q.clone() (dedup key) is eliminated and
the child Vec is pre-reserved — ~9% faster walk, byte-identical (golden 9/9). The
FULL Vec<String>→&mut String threading is still parked: the generic container's
Row/Column overflow heuristic counts full-width children by inspecting the parts
Vec, so it needs the list materialized. A buffer-threading rewrite would have to
preserve that inspection (e.g. two-pass, or track full-width count during emit).
Needs its own pass against the now-broadened golden corpus (9 fixtures).

### Graph card overlap + source-text bleed (dogfood v0.1.732)
Dogfood screenshot of the native mindmap graph (markdown→cards) shows cards
overlapping heavily with faint raw source text visible behind them. Likely the
force-layout/placement not spreading nodes enough for many same-size cards, plus
a z-order/background-clear gap letting the underlying text layer show through.
Needs the live graph to diagnose (placement is runtime gesture/layout, not
sandbox-testable). Candidate pass: increase node repulsion / min-spacing in the
graph layout, and ensure each card paints an opaque background rect above the
content text layer.

### Touch/click broken on device (dogfood v0.1.734) — NEEDS LOCALIZATION
Reported: "the touch and click interactions are broken" on the phone (pmOS),
observed on the terminal screen. Audited recent changes — none explain it:
v729 cursor signal is a benign timer restart; v733 density toggle is inside the
cardsCanvas Popup (modal:false, only present when graph open); byte fixes are
pure logic. cardsCanvas is a Popup so it doesn't capture when closed. Touch
handling is pure-runtime (not sandbox-reproducible). Need to localize: is click
broken (a) on terminal links/manpage suggestions, (b) on graph cards, (c) on
toolbar buttons, (d) everywhere incl typing? Likely a touch-vs-mouse event
handling gap (TapHandler vs MouseArea, or a Flickable/PinchHandler grabbing the
press) rather than a recent regression. Candidate areas: the terminal
_selClearMa propagation, the full-fill scroll MouseArea at ~6586, and any
PointHandler/PinchHandler that may steal touch grabs.

### Zip preview → peer share (dogfood v0.1.734) — half done
summarize_zip + parse_zip_report shipped (ZipReport: entries, file/dir counts,
avg deflate, ext breakdown) — feeds the existing terminal command-block summary
chip, so a zip run now previews "zipped N files (.json, .xml) in M dirs · avg
40% deflate". REMAINING: the "pipe to an env arg for nearby developers to
receive" half — serialize a ZipReport + the archive path into the peer handoff
(workspace_share already exists for git-style sharing). Design: a
zip_share_wire(report, archive) → a manifest line a paired peer's handshake can
fetch, so "share this zip" hands the archive + its summary to a nearby dev.
Runtime-surface; hold until touch is fixed (don't stack input features on a
broken-input build).

### Graph layout algorithm — the big one (dogfood v0.1.737, images)
Side-by-side: a mature mermaid renderer lays out a MIPS-datapath flowchart as a
clean hierarchical/layered graph with routed edges + subgraph boxes; OURS
scatters the same nodes with force-directed placement and heavy overlap ("good
component layout cannot be solved by [force] placement; a good placement
algorithm is needed"). The fix is a real LAYERED layout (Sugiyama/dagre-style):
(1) assign layers by longest-path from roots respecting edge direction, (2)
order within layers to minimize crossings (median heuristic), (3) assign x by
layer + within-layer slot, y by layer depth (honoring flow_direction). This is a
pure, testable core (graph in → positions out) — ideal for the measure/test
discipline + a golden-style position snapshot. Big but high-value; the node_id
qml-proofing (v737) was the prerequisite cleanup. NOTE: subgraph boxes (Data
Memory / Write Registers in the reference) are a second feature on top.

### Zip by voice (dogfood v0.1.737)
parse_zip_report/summarize_zip exist (v734). "zip by voice" = a voice intent
that (a) runs a zip of a named dir, (b) shows the preview summary, (c) optionally
hands it to a paired peer (the workspace_share path). Wire: voice "zip <dir>" /
"share this zip with <peer>" → Intent → run + summarize + peer_share manifest
line carrying the ZipReport. Runtime surface; pair with the peer arc.

### Layered layout — DONE v0.1.738; polish follow-ups
layout_layered (Sugiyama: layer assign + median ordering + coord assign) ships,
wired into mermaid_to_layout for directional graphs (force-directed kept for
freeform mindmaps/edgeless). Also fixed two mermaid PARSER gaps that blocked the
dogfood diagram: digit-first ids (1s/5a/13c were silently dropped) + labeled
dash-edges quote-stripping. MIPS source now parses 42 nodes/34 edges (was 9/3)
and lays out top-to-bottom. REMAINING polish: (1) control-signal nodes
({{RegDst}} etc., joined by `---` undirected) cluster at layer 0 — could pin
them beside their target instead; (2) {{ }} hexagon double-brace labels keep a
stray brace/quote (single-bracket forms are clean); (3) subgraph boxes (gray
Data Memory / Write Registers containers) — a separate rendering feature.

### Graph layout polish (dogfood v0.1.739) — still open
After the layered layout + edge-arrow fix + subgraph-node removal, remaining
from the dogfood image: (1) min card sizes too small / labels cut off
("subgra…", "{"ALU…") — flowcharts read better at EXPANDED density; consider
defaulting directional graphs to expanded, or raising the compact floor. (2)
source-node clustering: control signals ({{RegDst}} etc., joined by `---`) and
other in-edge-less nodes still pile in the top layer; pull single-successor
sources down beside their target. (3) subgraph BOX rendering: DONE v0.1.758 — group_layout.rs computes labeled
container boxes from final card positions (nesting depth, gn_ prefix tolerant,
singleton skip); mermaid_to_layout emits a GROUPS wire section; QML draws titled
rects behind cards (z:-2). Verified end-to-end (parse→layout→box). v0.1.759: (a) FIXED a parser bug — subgraph members referenced by an edge before
their declaration were dropped (MIPS captured only 1 of 6); now all captured.
(b) within-layer CONTIGUITY pass (layout_layered_grouped) clusters members
adjacently per layer (helps box WIDTH). STILL OPEN: flow-axis spread — a group
whose members span many layers gets a TALL box (MIPS 'Write Registers' is
1714px tall) because contiguity only clusters cross-axis; the real lever is a
layer-assignment constraint pulling group members to nearby layers (bigger
change, deferred).
(4) {{ }} hexagon labels keep a stray brace. (5) "no spatial awareness beyond
lines" — edges route straight through other cards; orthogonal/spline routing is
a later feature.

### Crossing reduction — knobs added v0.1.741; dummy nodes still the lever
LayeredConfig now has transpose_passes + transpose_break_ties (adjacent-swap
crossing reduction after median sweeps; bilayer_crossings/local_crossings
helpers). HONEST: on the corpus the median pass already resolves adjacent-layer
inversions, so transpose doesn't move MIPS (stuck at 9) — those residual
crossings are MULTI-LAYER edges, which need the skipped Sugiyama DUMMY-NODE
phase (lesson #10, the real lever). Knobs are correct infra + the user's
requested tuning surface; dummy nodes are the next crossing work.

### Native terminal tabs/panes — core DONE v0.1.743; live wiring held for greenlight
src/terminal_workspace.rs (19 tests): TermWorkspace = Vec<Tab>, each Tab a binary
Node split-tree of Panes (PTY sessions). Ops: new/close/next/prev/goto/rename/
find tab, split (H/V) / close / focus pane, total_panes, to_wire (QML), snapshot/
restore (layout-shape persistence, fresh pane ids on restore). WsCommand +
parse_command cover the established zellij_voice vocabulary natively (new pane
[dir], close pane, focus dir, new/next/prev tab, go to tab N, rename tab, switch
to <name>); apply() returns pane ids to tear down. CLEAN-ROOM HOLD on live
wiring: the backend currently holds a SINGLE pty (main.rs ~887) and delegates
multiplexing to zellij/tmux. Going native needs: (a) backend Vec<PtyTerminal>
keyed by pane id, spawn/teardown reacting to apply()'s ids; (b) QML split-pane
render of the Node tree + tab strip from to_wire; (c) route parse_command through
voice_intent; (d) persist snapshot() via workspaces.rs. Each is real runtime
surface — greenlight before wiring.

### Native terminal multi-pane — backend layer DONE v0.1.745 (RUNTIME-UNTESTED)
Step 1 of the wiring: backend now holds pane_ptys: HashMap<u64,PtyTerminal> +
term_ws: TermWorkspace alongside the existing single `pty` (which stays the
ACTIVE pane, so all existing read/render/write code is untouched). ws_command
qt_method runs a phrase through the tested model + reconciles PTYs (spawn on
split/new-tab, ws_swap_active parks/pulls on focus/switch, teardown on close).
ws_wire (tab strip), ws_pane_rects (QML layout). Compiles clean; pure model
tested. RUNTIME-UNTESTED: PTY spawn/teardown + active-swap (no GUI/pty in
sandbox). ROLLBACK: the feature is additive + inert until QML calls ws_command —
existing single-terminal path is byte-identical (empty map + 1-pane ws). Remaining:
(2) QML split-pane render from ws_pane_rects + tab strip from ws_wire (route key
input to the focused pane); (3) voice_intent route to ws_command; (4) persist
term_ws.snapshot() via workspaces.rs; (5) reader-thread output for non-active
panes (currently only active pane's grid renders — background panes accumulate
output but their render isn't surfaced until focused).

### Native terminal panes — voice routing DONE v0.1.746 (step 3)
voice_intent gained Intent::TermPane(phrase): matches the unambiguous pane
vocabulary (new pane [dir], split <dir>, close pane, focus left/right/up/down,
resize <dir>) BEFORE the browser-tab block, guarded by terminal_workspace::
parse_command so "split the bill" / "next tab" / "new tab" don't false-match.
Dispatches as ("term_ws", phrase) → QML calls backend.ws_command(phrase) →
updates the model + active-pane PTY, stashes the tab strip in root.termPaneStrip.
Routing decision verified standalone (6 match / 4 reject) + compile gate.
RUNTIME-UNTESTED on device but the model/PTY path is the v745 layer. Terminal-TAB
voice verbs deferred (collide with browser "tab"). Remaining: (2) split-pane QML
overlay rendering from ws_pane_rects + tab strip from termPaneStrip, route
focused-pane input; (4) snapshot persistence; (5) background-pane render.

### Native terminal panes — tab strip DONE v0.1.748 (step 2a)
QML termTabStrip above termScreen: renders tabs from root.termPaneStrip (ws_wire
format), tap-to-switch (ws_command "go to tab N"), + button (ws_command "new
tab"), shows pane count per tab, hidden when <=1 tab. Switching swaps the
focused pane's PTY into the existing termScreen view (unchanged render path).
qmllint + custom lint clean; RUNTIME-UNTESTED on device. Remaining for panes:
(2b) SPLIT render — needs background-pane rendering first (step 5) since splits
show multiple panes at once; (4) snapshot persistence via workspaces.rs; (5)
background-pane grid surfacing (each pane's TermGrid renderable, not just the
active one swapped into self.pty). Order: step 5 (background render) then 2b
(splits) then 4 (persist).

### Cheap cross-layer signaling — primitive DONE v0.1.750
src/event_signal.rs (7 tests): lock-free AtomicU32 bitmask, Channel enum (Wake,
AskAi, VoiceResult, LlmResult, Confirm), raise/clear/peek/is_set/take/idle.
Producer raises a bit on enqueue (atomic OR, no lock); consumer reads one
relaxed load (pending_signals() qt_method) and only descends into a queue
(locking) when its bit is set. Wired: wake + ask-ai event queues raise/clear
their bits; QML wake + ask-ai polls gate the expensive voice_pop_* call on the
cheap pending_signals() read. NEXT (the user's "deeper penetration" idea, fully):
(1) DONE v0.1.751: wake+askai coalesced into ONE eventPollTimer reading
pending_signals() and dispatching per-channel (86->85 timers). Remaining: migrate
the transient voiceResult/llm/confirm polls onto Channels too so they join the
same heartbeat;
(2) producer-side raise for those channels; (3) coalesce on_term_output_changed's
7 handlers into one per-frame repaint (separate in-use lever).

### Graph aspect ratio — adaptive gap DONE v0.1.754 (partial)
Adaptive layer gap shrinks the flow-axis gap for thin chains (cpu_pipeline
15.96->9.54 etc, harness-measured), clamped to an 18px floor. PARTIAL: long LR
chains are inherently wide; full fix = wrap chain across columns (breaks
directional read, deferred). Other graph items still open: subgraph BOX render
(groups parsed not drawn — layout/membership harness-checkable, box-draw is QML);
polyline edge routing through dummy waypoints; feedback edges as distinct curves.

### Source/disconnected node placement — DONE v0.1.760
leaf-source docking generalized from single- to multi-successor (dock to min
consumer layer - 1); fully-disconnected nodes (no edges) now distribute to
least-occupied layers instead of piling at rank 0. Measured: mips layer_imbalance
2.84->1.26, crossings 7->4; the 'lopsided layer' lesson dropped mips off the
list. Trade-off: mips aspect 0.57->0.39 (taller, still balanced). STILL OPEN:
lesson #4 (treat '---' Line edges as placement hints); #7 the disconnected nodes
are now spread but ideally a parse pass would reconnect breakout/wire nodes.

### Backward/feedback edges rendered distinctly — DONE v0.1.761 (lesson #6)
classify_back_edges (pure, 2 tests) flags edges pointing against flow; mermaid_to_layout
emits a 7th edge wire field (back=1); QML draws back-edges dashed + accent-tinted
(solid arrowhead preserved). Verified end-to-end (A->B->C->A: only C->A flagged).
QML draw runtime-untested but additive/low-risk. STILL OPEN: lesson #4 ('---' Line
edges as placement hints), per-graph gap scaling for small graphs.

### Split-pane RENDER (step 2b) — SHIPPED v0.1.762
The on-device greenlight (aaab.zip: tab strip shows '1 (2)' = 2-pane model live)
confirmed the multi-PTY foundation, so built the visible split. paneSplitLayer
overlay (child of termScreen, visible when >1 pane) tiles all panes from
ws_pane_rects, each rendering its grid via pane_row_html; focused pane (first in
list) bordered in accent; tap a pane -> ws_command('focus pane <id>') (new
FocusPane(u64) workspace command, syncs self.pty). Rich single-pane renderer
hidden when split active (no regression to 1-pane path). MVP TRADE-OFF: split
panes use the lightweight row renderer (no cursor blink/selection/scrollback) -
you SEE the split; rich per-pane fidelity deferred (step 2c). RUNTIME-UNTESTED:
the render itself (foundation now on-device-confirmed, but the overlay draw +
tap-focus need a dogfood pass). Rollback: paneSplitLayer visible:false.
REMAINING pane arc: step 2c rich per-pane render; step4 snapshot persistence.

### Pane arc step 4 (persistence) + step 2c (cursor) — v0.1.763
Step 4: TermWorkspace::snapshot/restore EXISTED (prior session) but dropped split
ratios + focus AND were never wired to disk. Fixed: serialize_node now emits
`:ratio`, snapshot persists focused-pane INDEX (ids reassigned on restore),
restore reads both (backward-compat with old snapshots). Wired: save_term_layout
(global file ~/.config/web2qml/term_layout.txt, NOT speaker-scoped so it survives
the no-speaker case) called after every ws_command; restore_term_layout at
startup (in workspace_restore_tabs, before the speaker gate) rebuilds the model +
spawns a PTY per pane. 4 new tests (ratio/focus/round-trip/old-format).
RUNTIME-UNTESTED: the PTY-respawn-on-restore (sandbox has no PTY).
Step 2c: per-pane cursor block in the split render (pane_cursor_row/col
accessors) — focused solid, others dimmed. Still deferred: per-pane selection +
scrollback in splits. REMAINING pane arc: 2c selection/scrollback fidelity.

### Build-output extractor (cargo/cmake error menu) — core SHIPPED v0.1.764
NEW src/build_diagnostics.rs (pure, 10 tests): parse_cargo/parse_cmake parse a
wall of build output into categorized Diagnostics (severity, rustc code, file:
line:col, message, condensed fix-hint) + rolled-up counts. Categories: resolve/
type/borrow/trait/unused/deprecated/cfg/style/link/cmake/other (each has a key()
for theme tinting + label()). minify() = token-frugal text (the user's real log:
1321 chars -> 215, 6x, leading with the error + 'try `fg_music`' hint, warnings
collapsed to category counts). to_wire() = SUMMARY/CAT/DIAG records for the menu.
Handles rustc 'similar name' hint extraction + gcc/clang 'file:line:col: error:'
embedded in make output. Backend: summarize_build(text,kind) / 
summarize_build_minified(text,kind), kind=auto|cargo|cmake (auto sniffs markers).
DONE: the parser + wiring (qt_struct clean, harness 1477+28).
NEXT (the UI): a fuzzy menu over the DIAG wire — shortcut/button opens it,
category-tinted rows, voice select ('jump to error one', 'show warnings',
'open <file>'), warning-count chips. Wire format is ready; menu is QML (runtime-
untested) so deferred to a turn where the full compile can be verified.
NOTE: v764 full-crate compile UNVERIFIED — sandbox reset wiped the modern
toolchain; apt cargo 1.75 can't build the edition2024 deps. Pure core fully
tested + qt_struct clean + wiring matches established QString idiom; rollback =
drop `mod build_diagnostics` + the 2 qt_method decls/inits + 3 methods.

### Build-error fuzzy MENU — SHIPPED v0.1.765 (the v764 follow-on)
Toolchain restored (user supplied rust 1.96 + I apt-installed Qt6/QtQuick/alsa/
libclang/cmake), so the FULL `voice claude` compile gate runs again — v764's
wiring + this menu are compile-verified, not just qt_struct-checked.
NEW: Grid::recent_text(max_rows) (scrollback tail + live viewport, trailing
blanks trimmed) + PtyTerminal delegate; backend summarize_focused_build() grabs
the focused pane's recent output and returns the menu wire directly. QML
errorMenu Popup (Ctrl+E, in chord registry): parses SUMMARY/CAT/DIAG, shows the
header (✓/✗ + counts + target), category chips + per-row left bars theme-tinted
by _catColor(key), error-first list with code/file:line/message/fix-hint; click a
row copies file:line to clipboard. RUNTIME-UNTESTED: the menu render + Ctrl+E +
recent_text capture (no PTY/display in sandbox) — but compiles + qmllint clean.
Rollback: remove the Ctrl+E Shortcut + errorMenu Popup.
NEXT: voice phrases ("show errors"/"jump to error one"), and jump-to-editor on
row click instead of copy.

### Voice for the build-error menu — SHIPPED v0.1.766
New Intent::ShowBuildErrors → "build_errors" wire → QML errorMenu.openFromFocused()
(same as Ctrl+E). Phrases: "show errors", "show build errors", "show me the build
errors", "what are the errors", "build errors", "summarize the build", "error
menu" (+ filler-stripped "errors"/"error menu"). Guarded so "show bookmarks" /
pane verbs don't hijack. Verified: matching logic standalone (all 7 positives
match, 5 negatives reject); compile gate passes (Intent handled exhaustively);
qmllint clean. NOTE: also fixed a pre-existing broken test module
(high_confidence_tests used `use super::Intent` not `*`, so `parse` was out of
scope) — but the bin test binary has further accumulated drift errors (e.g. a
test CommandBlock missing parent_block_id), so the full voice_intent suite still
isn't runnable via cargo test; left for a dedicated test-repair pass.
NEXT (this thread): "next error"/"jump to error N" navigation within the menu;
row-click jump-to-editor instead of clipboard copy.

### Bin test suite UNBLOCKED + reset_cadence cluster green — v0.1.767
The ~2526-test in-binary suite (incl. all of voice_intent) was dormant: it needs
the FULL crate to build, and had accumulated compile-drift. Fixed the blockers
(audit_log test: CommandBlock missing parent_block_id; high_confidence_tests:
`use super::Intent`→`*`), so it BUILDS + RUNS again: 2497 pass, 29 fail.
Added scripts/bin_tests.sh (documents the Qt/audio/libclang env + how to run).
Cleared the reset_cadence cluster (7): recover_name_case() preserves speaker-name
capitalization (real fix — names were lowercased by the parse buffer); updated 4
stale tests (bare "reset"/"reset her cadence" now Freeform not Unknown — assert
the real not-ResetCadence invariant; wire tests read parts[2] not [1] after a
desc field was inserted).

### REMAINING 29 bin-test failures — triaged for a focused repair pass
All PRE-EXISTING drift (dormant tests that stopped being enforced), NOT new:
- manpages (7): stale SNAPSHOT tests comparing exact generated QML; the renderer's
  theme color (#3b3732) + font size evolved. Need per-test review that current
  output is correct, then snapshot update. (underscore_bold/italic, italic i-tag,
  table_with_bold, to_qml_paragraph_wraps, welcome_doc, wikilink)
- voice_intent high_confidence (6): parser-expectation drift — e.g.
  term_pane_intents_parse expects TermPane("new pane right") but parse yields
  TermPane("new pane") (direction dropped); filter_by_tag/filter_cards/
  open_connections/workspace_intents/workspace_save_as. Need to confirm
  intended behavior (real bug vs stale test) per case.
- voice_intent recipe_catalog (3): 17 user-facing intents lack VOICE_RECIPES
  entries (search/find/scroll_page/go_top/go_bottom/palette/term_cmd/settings/
  homepage/sysinfo/email/llm/scroll_heading/hist_search + internal
  workflow_rec_start/stop/tier1_autotune). Fix = add recipes for user-facing +
  allowlist the 3 internal ones. Content work.
- singles (6): llm_provider::parse_claude_response_simple, log_summary email cap,
  qml_gen extract_title nested tags, recipes every_main_rs_url_has_a_recipe,
  smart_cd marker_detection_recognizes_cargo, voice_intent corpus_regression
  narration_open_tail_not_searched, switch_to_tab empty_query_rejected,
  workspace_save_load reserved_names_rejected.
DISCIPLINE: do NOT mass-update snapshots/assertions to current output without
confirming the behavior is correct (risk of cementing a real bug). Fix in
confident clusters.

### Terminal render perf: per-row HTML cache — SHIPPED v0.1.768
MEASURED FIRST (standalone replica /tmp/render_bench.rs): the QML Repeater
re-queries term_row_html for EVERY visible row on EVERY output event, and each
call rebuilt the HTML spans from scratch — even for rows that didn't change.
Baseline 164.9 us/frame (50x100 screen) vs 41.9 us/frame with a per-row cache =
3.9x less render CPU (same on idle frames). (Qt already skips Text re-layout on
an equal string, so this is purely the Rust-side regeneration cost — matters most
during streaming build output and on the phone where CPU = battery/thermal.)
IMPLEMENTED: TermGrid gained row_html_cache/row_html_sig + render_row_html_cached
(FNV-1a signature over ch/fg/bg/attrs; serves cache on unchanged sig, else
renders + stores). Self-correcting (signature-keyed → can't serve stale HTML even
if dirty-tracking missed a mutation). Extracted row_slice() so the renderer and
the signature read the SAME scrollback-aware cells. PtyTerminal::render_row_html
(the QML hot path) now routes through the cache; the uncached &self renderer is
unchanged for other callers. TRANSPARENT: no QML changes, no GUI risk, protected
by the existing grid mutex. VERIFIED: cached==uncached byte-identical (new test +
all 11 render/scrollback tests), full suite no new failures (2498 pass / same 29
pre-existing), pure harness 1477+28, perf thresholds OK.
NOTE: the dirty_rows range (v0.1.494, still otherwise unused) could later gate
which rows even get a signature check, cutting the idle residual (~42us is mostly
the per-row FNV) — a follow-on; signatures alone already give the 3.9x.
The 3.9x is the HTML-regen cost; it does NOT reduce the NUMBER of per-row FFI
calls (QML still calls per row) — reducing those needs exposing dirty rows to QML
(GUI-untestable), parked.

### Terminal load responsiveness: nuanced defer + non-blocking lock — SHIPPED v0.1.769
Addresses helix-open / app-load latency. Two changes in the UI frame driver:
1. NON-BLOCKING LOCK: the idle-mutation republish check now uses grid.try_lock()
   instead of .lock(), so the UI tick is NEVER stalled waiting for the grid lock
   while the reader thread parses a load burst. A contended lock means the reader
   has it AND is publishing snapshots, so there's nothing for the main thread to
   republish → skip this tick (safe; that path only catches idle scroll/resize).
2. NUANCED DEFER POLICY (was: any gen_delta>1 → defer up to MAX_DEFER): factored
   into pure should_defer_reparse(gen_delta, defer_count, ms_since_render,
   leading_edge_ms, max_defer) with 4 tests. Single-frame (typing) never defers;
   the LEADING EDGE of a burst after real idle (screen quiescent >=250ms, e.g.
   helix opening while the user was reading) renders IMMEDIATELY instead of
   waiting out the coalescing window — that window was latency added to the app's
   first paint; a SUSTAINED storm still coalesces (count-capped) so a build log
   doesn't re-parse more often. New field term_last_render_at stamps each
   re-parse so the next burst can tell leading-edge (stale) from storm (recent).
VERIFIED: 4 policy unit tests, compile gate, full suite no new failures (2502
pass / same 29 pre-existing), pure harness 1477+28, perf OK.
RUNTIME-UNTESTED (no GUI in sandbox): the actual helix-open latency improvement is
the HYPOTHESIS the tested policy implements; the try_lock reasoning is sound but
the real-world contention behavior is unmeasured here. Rollback: revert try_lock
to lock + defer_reparse to `gen_delta>1 && defer_count<MAX_DEFER`.

### Konsole-style pane shortcuts + _anchorCacheReady refix — v0.1.770
From a dogfood log (v769 run). Keyboard shortcuts for the split panes (all gated
to the terminal view so they never hijack the browser):
  Ctrl+Shift+9 → split left/right ("split right" → NewPane(Right))
  Ctrl+Shift+0 → split top/bottom ("split down" → NewPane(Down))
  Ctrl+Shift+[ / ] → shrink / grow focused pane (Resize(Left/Right))
  Shift+Arrows → focus pane in direction (Focus(Dir)) — ONLY when paneSplitLayer
    visible (>1 pane); single-pane passes Shift+Arrows through to the term app.
Tab cycling relocated off Ctrl+Shift+[/] (now resize) to Ctrl+PgUp/PgDown
(Konsole's own tab keys). New root._ws(cmd) helper routes to ws_command + updates
termPaneStrip, gated on swipeView index 1. Chord registry + F1 help updated.
Verified: all phrases parse to the right WsCommand (standalone test), compile
gate, qmllint + custom lint clean, harness 1477+28. RUNTIME-UNTESTED: the actual
key bindings firing + Shift+Arrows gating (no GUI in sandbox).
FIXED (regression from the log): _anchorCacheReady write threw "Invalid write to
global property" at main.qml:3799. The flag is declared on voicePopup (~18523,
alongside the reader anchor functions), but the write is inside browseFlick
(OUTSIDE voicePopup) → unqualified resolved to a JS global. My v762/v763
"unqualified" reasoning was WRONG (browseFlick is not in voicePopup's scope).
Fixed by qualifying with the owning id: voicePopup._anchorCacheReady (ids are
document-wide). In-scope reads at 18527/18549/18565 unchanged.
STILL OPEN (harmless, deferred): one-time "Could not attach Keys property to
Popup" warning at load — no Keys.on* found on a Popup in grep; a Keys handler
silently doesn't attach, no functional breakage observed.

### Split-pane "repeat shell" + tab "no text" — FIXED v0.1.771
Two dogfood bugs. Reasoned through in Python first
(docs/design/models/pane_render_model.py) which proved the Rust pane->pty mapping
is SOUND (each pane resolves to a distinct shell) — both bugs are in the
render/refresh layer, not the model.
BUG 1 (panes repeat their shell): the split overlay's INNER per-row Repeater has
model = paneCell._rows (an integer), so inside it `modelData` is the row NUMBER,
not the pane object — `modelData.id` was undefined → coerced to pane id 0, so
every pane rendered pane 0. FIX: capture `property real paneId: modelData.id` in
the OUTER paneCell delegate (where modelData IS the pane) and use paneCell.paneId
in the inner Repeater (_rows + pane_row_html). Model: buggy→all panes identical,
fixed→distinct.
BUG 2 (2nd/3rd tabs blank/stale): ws_command swaps self.pty to a different
terminal, but the poll's html-dedup (_last_published_norm) SUPPRESSES
term_output_changed when the swapped-in content matches the previous tab (two
fresh shells, identical prompts) → termRowsHost never refreshes → stale/blank.
FIX: ws_command now resets term_render_generation (sentinel) + _last_published_norm
after every command, forcing the next poll to re-publish the new active pane.
VERIFIED: Python model (mapping sound + bug1 reproduced+fixed + bug2 suppression
shown), compile gate, qmllint + custom lint clean, harness 1477+28.
RUNTIME-UNTESTED (no GUI): the visual result on-device. Bug 1 fix is proven by the
model + is a direct id-binding correction; bug 2 fix is a sound refresh-guarantee
that addresses the suppression I could reproduce, though the exact "blank" wasn't
reproducible in the headless model. Rollback: revert the paneId binding + the two
reset lines in ws_command.

## Toolbar tab/pane management UX (v0.1.773 — item 1 shipped untested; 2 & 3 parked)
Three dogfood thoughts from the user. RUNTIME context: all three are pure
QML/layout and were NOT runtime-testable in the build sandbox (no display).

1. **Tab/pane organizer breakout button** — SHIPPED in v0.1.773 as a first cut,
   RUNTIME-UNTESTED. Added `paneOrganizer` (a right-edge Drawer, mirrors
   browserDrawer) with Split / Focus / Resize-merge / Tabs action groups, each
   routing through the existing `root._ws()` helper (no new backend). Opened by
   a ▦ ToolBtn in the browser topBar (qml ~9459) and Ctrl+Shift+O (chord
   registered). `doWs()` switches to terminal mode first since `_ws` is
   terminal-only.
   OPEN / to refine once testable:
   - **Button placement.** The user said "like the char entry button" — that is
     almost certainly the terminal toolbar's pipe/brace **breakout** buttons
     (GestureHost + popup, qml ~9165-9220), not the browser bar. Move the ▦
     button there (GestureHost pattern) so it's contextual to the terminal.
     Need the user to confirm which button "char entry" refers to.
   - **True organizer VIEW.** Current drawer is action-buttons only. A visual
     grid/list of open panes + tabs (tap a tile to focus, drag to rearrange,
     ✕ to close) is the richer ask. Needs a tab/pane *model*: terminal panes are
     derivable from `ws_pane_rects(w,h)` / `termPaneStrip`; terminal tabs from
     `ws_command("list tabs")` (parse the wire). Browser tabs (if the organizer
     should include them) need a separate model — unclear today whether the
     browser is multi-tab in the UI (no tab-strip Repeater found; voice has
     TabOpen/SwitchToTab, backend has tab_open + tab_count_key).
   - Mobile: drawer is `min(360, width*0.9)`; fine, but verify on a phone.

2. **Tabs to the bottom on mobile + collapse when idle** — PARKED (high blast
   radius, untestable). The toolbar already reparents between `topMountSlot` and
   `bottomMountSlot` (the v0.1.279 dynamic-parent system; see qml ~9433 topBar,
   ~10124 "reparented to the top slot. Mirrors bottomMountSlot"). compactLayout
   (width<720 || portrait, qml ~46) is the mobile signal and is already used in
   the topBar (~9468/9482).
   PLAN: bind the toolbar's active mount slot to `root.compactLayout` so it
   docks to `bottomMountSlot` on mobile; add a collapsed state (height-animate
   to a thin strip / peek handle when not the focused surface, expand on
   tap/focus — reuse the existing `Behavior on Layout.preferredHeight`).
   WHY PARKED: the mount-slot reparenting is core UI; a wrong binding can drop
   the whole toolbar, and I can't verify visually. Also need to confirm WHICH
   "tabs" — the browser/terminal mode bar, the terminal tab strip, or both.
   Do this with the user able to dogfood each step.

3. **Cards view padding/alignment off-center on mobile** — PARKED (needs the
   exact view identified). User: "the padding on what was last cards and
   alignment on mobile screens is a bit strange to the side instead of
   centered." Candidate is the cards/graph canvas (`cardsCanvas` Popup, qml
   ~22432, fills root; pan/zoom `cardsFlick`) — but "centered" on a pan/zoom
   canvas usually means initial content-centering (the `_centerXAnim`/
   `_centerYAnim` SmoothedAnimations + centerOn logic ~23264). Could also be a
   different, simpler cards LIST view I haven't located. NEED: a screenshot or
   which screen ("cards" = the graph canvas, or a saved-pages list?). Likely fix
   once identified: ensure the content's initial pan centers it horizontally
   when contentWidth < viewport (compact), and symmetric margins.

## ═══ Big dogfood list (v0.1.774+) ═══
User's batch. [DOING] = tackling now; [PLAN] = scaffold/plan only.

1. [DONE v0.1.774] **Keybind swap:** Ctrl+Shift+Arrow = switch/focus PANES; Shift+Arrow
   = switch TABS (prev/next on Left/Right). Was: Shift+Arrow = pane focus,
   Ctrl+PgUp/Dn = tabs. Caveat: Shift+Up/Down stays pass-through (terminal line
   selection); only Shift+Left/Right take tabs.

2. [PLAN+scaffold] **`exit` leaves an empty tab → session-summary card.** On
   shell exit, instead of a blank tab: offer to save the session as a
   "history card" — Mermaid tree of commands run, git-activity graphs,
   important codeblocks, optional AI summary — then a shortcut to close for
   good or start a new session. Scaffold: detect child exit (pty reader EOF →
   signal QML), show an overlay with [Save as card] / [New session] / [Close].
   Rich summary (mermaid cmd-tree from BlockTracker history + git graph +
   AI) is the larger follow-up.

3. [PLAN] **Terminal still slow.** Needs on-device profiling (WEB2QML_PROFILE
   taps). Candidates already parked: gate render_row_html_cached sig-checks by
   dirty_rows; coalesce republish; the v769 try_lock/defer is in. Measure the
   poll→parse→html→publish path before changing.

4. [DONE v0.1.774] **Graph-view "open" button:** fixed — it navigated but never switched to the browser view; now does.  Find the open/launch
   action on a cards-canvas node and wire it (likely should backend.nav the
   node's URL / open the man page). cardsCanvas ~22432.

5. [PLAN] **Terminal-log step-back UX, app-styled.** Scrollback browsing in the
   app's idiom (not raw terminal scroll) — paged history with the same
   selection/card affordances. Relates to #2 (session history).

6. [READY - needs Rust rebuild] **Settings → Reading: padding options** for reading width AND
   terminal. Add sliders/steps; wire reading width to the page content margin
   and terminal to the terminal content padding. New persisted settings.

7. [PARTLY DONE v0.1.775] **More themes, grouped by vibe** (9 added, 7 vibe groups, grouped picker + live preview on a SAMPLE bar). REMAINING: preview on the REAL navbar/full-button + drag-reposition-release gesture. with a tap-hold→expand→reposition→
   release picker (GestureHost pattern) + LIVE partial preview on just the full
   button + the navbar so you see what changes before committing. Themes are
   data (extend the `th` palette set); picker is the big UI piece.

8. [DONE v0.1.774] **URL-bar jitter:** direction-anchored hysteresis.  Add direction
   hysteresis / min-delta + debounce so rapid scroll reversals don't thrash the
   bar. Find the scroll-driven show/hide binding.

9. [PLAN] **Card removal + email-like archive/trash; email integration.** First:
   a way to remove cards from the web view and an archive/trash model (soft
   delete → trash → purge). Then EMAIL INTEGRATION — akonadi/kmail or Proton
   Mail Bridge (user will provide bridge details). Scaffold an abstract mail
   provider trait + a "mail" surface to preempt; Proton Bridge speaks
   IMAP/SMTP on localhost, so a generic IMAP client core is the seam.


## Implementation notes mapped (ready, pending a Rust rebuild)
- **#6 padding (minimal-invasive plan):** add `reading_pad: i32` + `term_pad: i32`
  qt_properties near font_size (main.rs ~486); init via a NEW small
  `load_padding()` reader (parse two keys from settings.conf, default 0) so the
  20-field load_settings_from_disk tuple (main.rs 7891, single destructure at
  3057) is left untouched; append `\nreading_pad={}\nterm_pad={}` to the
  save_settings format string (~6433). QML: two sliders in the Settings
  Reading section; bind the page content horizontal margin (browseFlick /
  pageLoader, ~3808/3892) to reading_pad and terminal content padding
  (termRowsHost ~6165) to term_pad. Default 0 = no change (safe).
- **#2 exit → session card:** pty reader EOF already knownable in pty_term.rs;
  add a backend signal `shell_exited(tab_id)` + a `session_summary(tab_id)`
  that walks BlockTracker history into {commands[], cwd, git_touched} and emits
  a Mermaid command tree. QML overlay on the dead tab: [Save as card] /
  [New session] / [Close for good].
