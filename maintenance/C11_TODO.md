# C11_TODO — open threads from the c11 session

Same convention as `C9_TODO.md` / `C10_TODO.md`: every item says how far it
actually got. Those two files are NOT superseded — both were edited in place
this session where status changed (C10-0a), and C9 remains the larger backlog.

The c11 session installed the offline 1.96.0 toolchain from the pack-rust
bundle and did the C10-0a state model (`2e9a9695`). What follows is what it
found and did not get to.

---

## 1. C10-0a's remainder — tuning, on hardware

REVISED mid-session on the user's call: appearance work gets DEMOED in the
codebase and tuned on device, not deferred to it. So the forms now exist:
every transient surface renders through egui's Modal (scrim, click-outside,
Esc consumed) in its QML form, and the PTY key drain is gated while a surface
is up — see the second c11 commit. What remains on-device is TUNING, not
implementation: every knob is in `overlay.rs`'s `form()` table (QML line
cited per row) and the two constants under it. Known open judgments for the
device pass: settings is centered rather than QML's nav-anchored position;
the cmd sheet's radius-20 top corners were not ported; workspaces' right-
drawer form has no QML counterpart and was chosen. Appearance is UNVERIFIED
here — no display.

## 2. Checker findings, all pre-existing, none fixed

Run with the repo root as the argument — the default path does not exist in
this sandbox, and a checker pointed at nothing reports success
(`scanned 0 crates`), which is exactly the vacuous-pass shape CLAUDE.md §5b
warns about. The runs below scanned 254 crates.

- **C11-2a — `lock_check`: `games/unendlich/crates/unendlich-engine` declares
  but has not locked** audiopus, egui, linkme, netcode, ogg, rayon, rodio,
  unendlich-ui (and its dash sibling serde_json). `verified` (the checker ran;
  the cause is not diagnosed). NOTE the same crate is C10-1a's one "looks
  real" feature-sweep FAIL — a stale lock resolves online and fails offline,
  which is consistent with an 82 s failure in an offline sandbox. Check the
  lock BEFORE debugging the crate.
- **C11-2b — `orphan_check`: 7 orphaned .rs files, 0 tests.**
  `games/blackstar-rs/src/rig.rs` (446 lines),
  `games/unendlich-engine`'s `render/bloom.rs`, `render/postfx.rs`,
  `render_meshref_shim.rs`, `system/snapclone/camera-bridge/src/shared_frames.rs`,
  plus two more past the listing cut. Each wants the §7b decision: archive
  with provenance, or declare and compile. Nobody has looked at why they
  exist.
- **C11-2c — `rustcheck` flags 10 `missing-mod-file` WARNs in
  voiceterm/egui-frontend** (`mod recall;` in cmd_sheet.rs etc.) that the
  green compile contradicts — the mods resolve, so the checker's file-layout
  expectation is wrong for however these are declared. Either teach rustcheck
  the pattern or record it as known noise; a checker that cries wolf on a
  green crate gets ignored (same failure mode as C10-1b's skip list).
  Also `ambient-learner` (1), `krapper` (2), `rusty-board` (1), undiagnosed.

## 3. Not reached

The rest of C10_TODO §0 (C10-0b snapclone-egui, C10-0c fao-codec inventory,
C10-0d rusty-board keyboard parity inventory) and §1 (the feature sweep's
games/ tail and the whole apps/ slice) were not touched this session.

## 4. The eframe backend thread (snapshot-cam's on-device crash)

- **C11-4a — `camera-client` does not compile.** `verified` pre-existing (by
  stash): `PinningVerifier` lacks `Debug` where something formats it. Nothing
  to do with the limits fix in the same crate; undiagnosed beyond that.
- **C11-4b — road-warrior's wgpu limits are unclamped.** `read`. It passes
  `eframe_backend_check.py` via an explicit `Renderer::Wgpu`, so the choice
  is deliberate — but it inherits egui-wgpu 0.27's default device request,
  the same over-ask that killed snapshot-cam. Old eframe, different API;
  wants the camera-bridge treatment when someone is in there.
- **C11-4c — beatspace's app feature and unendlich-dash are unverified by
  compilation.** Graphs resolve with the new explicit features; cpal blocks
  beatspace's GUI here (§3) and dash is the known sweep-timeout unit.
- gstreamer dev headers ARE installable here (`apt-get update` first — the
  baked lists are stale and 404). That is what made camera-bridge verifiable;
  noted in SANDBOX_BUILD_LIMITS.md.

## 5. The rain thread (unendlich-fullgame)

- **C11-5a — `unendlich-netview` does not compile.** `DONE` · `verified`
  end-to-end: the missing shared/netview dep is declared, the engine's
  default check is green, and the spectator served a live rollback snapshot
  over TCP in-sandbox. C10-1a's engine sweep FAIL should be RE-RUN — this
  was almost certainly its root. Was:
  Very likely THE root of C10-1a's engine sweep FAIL, and possibly related to
  the C11-2b orphans — a module that exists on disk but is not declared, or
  was renamed. Untouched by the rain fix; diagnose the import before anything
  else in that crate.
- **C11-5b — the rain dripline is unverified by eye.** The cull tests pin the
  geometry; whether it READS right (streak pop at the boundary, snow drift at
  cave mouths) is the on-device pass. `UNENDLICH_RAIN_CULL=0` reverts live.
- **C11-5c — the r156 fire-shelter check scans only 1..=3 blocks up**
  (`fullgame` ~5783), so a fire in a tall cave counts as unsheltered and rain
  can douse it through 40 blocks of rock. `read`, not fixed — same bug family
  as the rain, different mechanic; wants the same per-column surface query.
- cmake and xorg-dev install via apt here (after `apt-get update`), which
  makes glfw — and therefore the fullgame bin's window feature — buildable in
  the sandbox. Added to SANDBOX_BUILD_LIMITS.md alongside the gstreamer note.

## 6. The device-test batch (voiceterm-egui field report, three screenshots)

Fixed this session: the live-grid-in-a-strip bug and the 3x3 menu's dead
Split/Close/NewTab arm (see the commit). Everything below is open, in the
report's own order, each with the pointer a next session needs.

- **C11-6a — input routing is one-grid-per-app, not per-pane.** `DONE` ·
  `verified` (see the pane_host commit; 1110 tests). The session follows its
  pane; a fresh split starts fresh (the inline swap's inheritance hole is
  fixed for pane_apps too). REMAINS on-device: per-pane REAL ptys — one
  byte-stream per pane to an actual shell; the state model is ready. Was:
  `self.app` (block log) swaps per focus; `self.grid` (the live VT screen,
  where typed bytes actually land) is a singleton now painted in the focused
  rect. Correct next step: a `HashMap<pane_id, TermGrid>` swapped exactly
  like `pane_apps`, plus per-pane `pty_out`/`pty_echoed`, so a session stays
  with its pane. The swap pattern to copy is at the PER-PANE TERMINALS
  comment (~2990); the routing itself is headlessly testable.
- **C11-6b — the file picker: right-edge popout, macOS-style cascading
  columns, cap 3, like the Qt original.** `not started`. The QML's own
  comment (main.qml:17117) says the single drawer was interim and names the
  cascade as the intended evolution (TODO_v68_deferred.md #1). Shape: a
  headless miller-column model first (current path, per-column selection,
  eviction beyond 3 columns — all testable), then a `Surface::FilePicker`
  row in overlay.rs's form table (RightDrawer; widen per column up to the
  QML's columnWidth math at main.qml:17173).
- **C11-6c — the command palette KEY is dead on device.** `not reproduced
  here`. Ctrl+P handling reads `i.modifiers.command && key_pressed(P)` and
  toggles `Surface::Palette`; tests pass headlessly. Suspects, in order: the
  keyboard gate (a stuck `captures_input` would eat the whole keyboard —
  check whether ANY surface is open when it fails), P reaching the PTY as
  0x10 and something swallowing the press first, or device-side modifier
  mapping. Needs a device with a log line on the toggle.
- **C11-6d — the voice dialog is not ported.** `not started`, and NOT small:
  the Qt version's voice path runs through voiced + the recogniser; the egui
  crate has `voice_routing.rs` and `repl_voice_glow.rs` as parts. Inventory
  first (C10-0c style): enumerate what the QML voice dialog DOES before
  porting widgets.
- **C11-6e — the graph view is not ported.** `read`. `graph_view.rs`,
  `connections_graph.rs`, `layered_graph.rs` exist in the crate;
  `SidebarAction::Graph` was deliberately repointed at the workspaces page
  (comment at the sidebar action site). Likely wiring, not writing: check
  what graph_view renders and give it a Surface row + a route.
- **C11-6f — sidebar double-draw, ALSO SEEN IN BEATSPACE.** `read`. Two
  apps, one symptom (a tab painted twice, offset — screenshots 2 and 3) —
  the shared suspect is uikit-egui's tab/button rendering; voiceterm's
  `tab_row` cites the QML row shape and both apps consume the kit. Diff one
  widget's paint path for a double `rect_filled`/galley emit under
  animation; a structural test can pin single-paint once found.

## 7. The responsive-pager thread (uikit-egui::pager landed; adoption open)

- **C11-7a — wire ambient-learner-egui onto the pager.** `not started`; the
  motivating app. Sketch: its `Page` enum (main.rs:18) becomes the metas
  (min_width ~360 to match the phone screenshot); the tab strip keeps
  working by setting `pager.active`; the body renders through
  `pager.ui(ui, gutter, |ui, id| match id { ... })`. The centered-on-small /
  dashboard-on-large behavior then arrives with zero app-side layout code —
  which is the point; if it needs app-side code, the abstraction is wrong,
  push back on the kit instead.
- **C11-7b — snapclone-egui onto the same pager**, replacing its bespoke page
  navigation, so qt/egui swipe finally share one implementation. While
  there: **the settings submenus don't overflow on scroll gracefully like
  the qt counterpart** (field report) — a separate bug in its settings
  scroll containment; reproduce against the qt behavior first.
- **C11-7c — GestureConfig::for_page_size vs SwipeNavigator's constants.**
  `read`. The kit's TwoTierGesture has its own thresholds; nobody has
  diffed them against kCommitFraction=0.22 and the 340ms settle. If they
  disagree, the kit should adopt the Qt numbers (they are the documented
  reference) — pager.rs exports them as pure fns already, tested.
- **C11-7d — per-page min_width packing.** fit_cols packs by the WIDEST
  page (conservative, cannot overflow); real per-page packing is a
  refinement with the same test harness.

## 8. The camera thread (desktop webcam)

- **C11-8a — verify on a real desktop.** The plan + fallback chain is tested;
  frames are not (no camera here). Watch the log: `camera source: trying ...`
  lines show the chain; a wrong pick on a multi-camera desktop wants the
  override (`pw_node` config accepts a /dev/video path now).
- **C11-8b — the egui frontend's camera_source/camera_select decision layer
  is still not fed by this.** `read`. camera-bridge now enumerates; the
  frontend's `choose()` (device+format+front-facing) should consume the same
  v4l2 listing so the viewfinder mirror/rotation decisions apply to the
  desktop path too. The seam exists on both sides; nobody has joined them.
- **C11-8c — pipewiresrc parse-validation** needs a machine with the
  gst-pipewire plugin; here it is string-asserted only, said so in the test.

## 9. Sweep: everything not yet attended to, so nothing is lost

Cross-checked against the whole c11 conversation. Already recorded above:
picker cascade (6b), palette key (6c), voice dialog (6d), graph view (6e),
uikit double-draw (6f), pager adoption + snapclone settings scroll (7a/7b),
gesture-constant diff (7c), per-page packing (7d), camera verify + frontend
seam (8a/8b), netview import (5a), fire-shelter scan (5c). Newly recorded:

- **C11-9a — rusty-board dictation against a real microphone.** The voice
  feature compiles now (c11-009); nobody has spoken into it. Also verify the
  GTK build still compiles with the `super::` paths — it should by
  construction (same module tree), but it was NOT built here (GTK4 absent).
- **C11-9b — voiceterm on-device recheck list for the next field session:**
  the strip's disappearance (grid-in-pane), 3x3 new-tab/split/close, surface
  modality feel + form-table tuning, the session-follows-pane behavior, and
  whether the palette key came back (6c's suspects stand).
- **C11-9c — the C10 §0 parity items untouched by c11:** C10-0b
  (snapclone-egui fullscreen/overlays), C10-0c (fao-codec inventory — still
  the suggested first), C10-0d's keyboard parity inventory. And C10-1a's
  feature-sweep apps/ slice has still never run — it would have caught
  rusty-board's voice-feature breakage (c11-009) years of sessions ago,
  which is now the concrete argument for spending the hours.
- **C11-9d — snapshot-cam / beatspace / unendlich-dash on-device smoke** after
  the eframe backend switch (c11-003): glow must actually present on the
  desktop and the phone; nobody has launched them since.

## 10. The logscope field report (large repos, gibberish, git ingestion)

- **C11-10a — the 2.50 GB spec-prefill crash: DONE**, verified on lavapipe
  with in-tree fixtures (see the commit). The sandbox can now run the FULL
  gpu-gemm suite: mesa-vulkan-drivers + numpy + tools/tiny-variants/gen-all.sh
  — 1221 tests, formerly self-skipping without fixtures.
- **C11-10b — qwen3 + deepseek-coder gibberish.** `narrowed, not fixed`. The
  tiny-qwen3 fixture tests PASS here, so qwen3's architecture path (qk-norm
  et al) is sound at small scale — the field gibberish points at real-model
  metadata instead: deepseek-coder-6.7b ships rope linear scaling (factor 4),
  and mishandled rope scaling garbles both families. Check the gguf loader
  for `rope.scaling.type/factor` handling first; a detect-and-refuse with a
  named message beats silent gibberish if implementing takes longer.
- **C11-10c — git-history sanitizer.** `not started`. A pure module: git
  trees/branch names/commit messages are attacker-controlled text headed for
  a model prompt; fence, strip control chars, cap lengths, mark provenance.
  Foundation for history ingestion + evidence handling; fully testable here.
- **C11-10d — summarize-off large-repo crash + file-reading crashes with
  larger models.** `unreproduced`. The 10a fix removes the known allocator
  blow-up on that path; retest on device first — remaining crashes get their
  own backtraces before anyone guesses.
- The engine TODO in the pasted snippet (log prompt head+tail for loop
  introspection) — the prompt diag already prints tail ids; add the head.

## 11. Continuation of §10 (c11, later)

- **C11-11a — rope linear scaling: DONE at loader level** (+ named refusal
  for unknown/incomplete scaling). Retest deepseek-coder-6.7b on device —
  the log will now say "linear rope scaling ON" instead of "(inactive)".
- **C11-11b — qwen3 gibberish: NARROWED to the vocab-alignment hypothesis**
  (151936 % 256 = 128, the fleet's only misaligned vocab; field transcript
  shows index-ramp logits). Harness landed (vocab_alignment_tests), aligned
  control green; BLOCKED on gen3.py honouring VOCAB for qwen3 (tokenizer
  array overrides it — extend the list to VOCAB entries, regenerate
  /tmp/tiny-qwen3-oddvocab.gguf via /tmp/gen3_odd.py's edit, rerun). If the
  misaligned run diverges: hunt argmax_at / sample_at / logits projection /
  int8 embed-table stride with the failing case in hand. If it agrees:
  hypothesis dead, fall back to real-gguf metadata probes. Previously: tiny-qwen3 green; head_dim
  explicit-key handled; linear/yarn now handled-or-refused. Next probe on
  device: load the real gguf and read the rope line + any refusal; if it
  loads clean and still gibbers, dump the first-token logits against
  llama.cpp on the same prompt (the engine has --validate machinery).
- **C11-11c — Gpu::alloc systemic cap guard: DONE.** Any remaining crash on
  large repos now names its shape and site.
- **C11-11d — wire read_harness into backend prompt assembly**: chunking,
  fences, git log formatting through the budget; the module has no consumer
  yet. Also feed ModelClass from the engine's real param count.

## 12. The agent-format thread (prose in the fence, six turns zero files)

- **C11-12a — line grammar + corrective + refusal ladder: DONE** at unit +
  compile level (see the commit). Retest the original task on device with
  the 1B: expect either line-grammar calls landing, or a bounded stop whose
  verdict names the format failure — never the silent loop.
- **C11-12b — mock-Llm integration test under ToolGate::Required**: drive
  step_turn with a scripted model that fails twice then emits a valid line
  call; assert the corrective landed in the prompt between turns and the
  ladder reset. The fixtures all run Open today, which bypasses the ladder.
- **C11-12c — per-class prompt shaping**: the protocol note now teaches both
  shapes to every model; read_harness::ModelClass should choose — Small gets
  the line grammar FIRST and the fence form omitted entirely (one shape to
  hold is the point), Large keeps fences primary.

## 13. Reverse generation + the tofu sweep

- **C11-13a — the reverse experiment is runnable**: gpu-gemm::reverse on any
  loaded model. First plays on device: shortlist from the prompt's own
  vocabulary (prompt-lookup's table, reversed), walk a reply back 10 tokens
  and read what the model reconstructs; score a suspected loop's return
  step. The primitive is proven; the ideas are the point.
- **C11-13b — the ⧉ sweep**: chat.rs done; remaining hardcoded ⧉/icon chars
  in logscope's other tabs, voiceterm shell.rs, and any app with copy
  buttons → uikit_egui::icons::copy_label / glyph_or. Mechanical; grep ⧉.
- **C11-13c — consider a kit-installed icon font** (one embedded subset with
  the handful of UI glyphs the fleet uses) so the probe's fallback branch
  becomes rare instead of machine-dependent. Weigh binary size on device.

## 14. Repository-work thread (c11, later)

- **C11-14a — summarize dropped-signal: FIXED** (compact_poll). On device,
  confirm the amber "context is full" offer now appears on a long
  auto-continue chat — it never could before, so nobody has seen it work.
- **C11-14b — patch mode silence: FIXED** (no_patch_reason always notes).
  Remaining question the fix does NOT answer: whether the 1B's diffs are
  merely unparseable-but-close. If the note says "diff-looking lines this
  parser did not accept" on device, capture one reply and widen
  parse_patches against it — that is a different bug wearing the same coat.
- **C11-14c — git history reading landed** (parse_git_log + git_history_block,
  hostile refs contained). NOT wired: no caller runs `git log` yet. Next:
  a workspace_exec tool (`git_log` verb, and a line-grammar `!<nonce> log N`
  for small models) feeding git_history_block at the session's ModelClass.
- **C11-14d — still the big one: wire read_harness into backend prompt
  assembly** (was C11-11d). Files AND history now have budgeted, fenced
  renderers with no consumer; the prompt builder should stop hand-rolling
  file inclusion and call them, taking ModelClass from the engine's real
  parameter count.
- **C11-14e — `--- ` in prose false-positives no_patch_reason's diff branch**
  (a reply quoting an em-dash list could match). Harmless today (it only
  picks the wording of a note) but tighten if it ever gates behaviour.

## 15. The allocation defect, closed as a class

- **C11-15a — DONE**: verify_forward slices to fit (all rows, bounded
  buffers); the ordinary streaming reuse path uses prefill_chunked_from.
  The c11-013 guard is what caught the second site — keep that pattern:
  a named assert at the allocation is how the next class gets found.
- **C11-15b — retest `:ws` on a big repo with the 1B.** Expect: no cap line,
  slices in the log, an answer. If a NEW shape appears, Gpu::alloc's guard
  names it with the call site.
- **C11-15c — the lesson, for the checklist**: when a fix is "this caller
  should chunk", grep every caller of the primitive BEFORE committing. Two
  sessions were spent on one defect because the first fix stopped at the
  backtrace it was handed.

## 16. Read-harness wiring (C11-14c/14d closed; what is left)

- **DONE**: agent_context builds from read_harness at the model's class;
  Tool::GitLog + `!<nonce> log N` + protocol note; Workspace carries a model
  label. End-to-end test against a real repo with a hostile subject/branch.
- **C11-16a — the CHAT prompt (not just the agent) still hand-rolls its file
  context.** agent_context covers agent/fan runs; a plain `:ws` chat turn
  (the field transcript where the 1B said "we would need to see the full
  definition") goes through a different assembly. Find it and give it the
  same treatment — this is the exact field symptom.
- **C11-16b — feed ModelClass from the engine's PARAM COUNT, not the label.**
  resolve() already prefers Some(params); nobody passes it. The engine knows
  its own tensor-derived size; thread it to chat/workspace.
- **C11-16c — read_range through the harness**: line-numbered chunks at
  budget.chunk_lines, so a Small model gets 60 lines rather than a whole
  file. The budget field exists and is still unused by the read tools.
- **C11-16d — `Tool::GitLog` for a PATH** (`git log -- <path>`): "what
  happened to this file" is the question a repo-reading agent actually asks.

## 17. Read-harness, second wiring pass (16a-d done)

- **DONE**: chat turns carry a workspace block (the field symptom's source);
  read_range clamps to the class with a continuation hint; ModelClass prefers
  the engine's param count; git log takes a pathspec; the line grammar takes
  `log <path> <count>`.
- **C11-17a — nobody CALLS with_model_params yet.** The field exists and is
  tested; the chat/agent construction sites still pass only the label. Thread
  the engine's real count from Evt::Loaded (the loader prints d/layers/vocab,
  so the number is already computed somewhere).
- **C11-17b — Tool::ReadFile is still unbudgeted**: a whole-file read into a
  Small context is the one remaining unclamped path. Either route it through
  read_range's clamp or give it the same treatment.
- **C11-17c — on-device: does the 1B now read?** The whole thread's value
  rests on this single observation. Watch for: does it call read_range/log
  at all, and does the continuation hint produce a SECOND call rather than a
  repeat of the first?
- **C11-17d — `git show <hash>` / `git blame <path> <line>`** would answer
  "why is this line here", the natural next question after path-scoped log.

## 18. The hive matrix (docs/11-hive-matrix.md + hive.rs)

- **DONE**: the matrix cell (model x node x KV warmth x busy), place() with
  warmth-over-speed and hard reach, explain_empty's three causes, and the
  design doc the dangling HiveMind_Strategy citations now point at.
- **C11-18a — assemble() DONE** (topology + NodeReport -> Vec<Cell>, latency
  from Route, silent peers excluded). What remains is the I/O either side:
  the ENGINE must fill a NodeReport (it already computes every number — decode
  tok/s from the profile line, warm prefix from pick_anchor, busy from the
  session) and the FABRIC must ask peers for theirs over the existing
  announce/enlist path. Neither is pure, which is why neither is in hive.rs.
- **C11-18b — place_fan() DONE**: Mode's breadth now meets placement's choice.
  Wiring: coordinator's consumers should call place_fan(plan.workers.len())
  instead of assuming local execution.
- was C11-18a: hive::place takes `&[Cell]`;
  assembling it from discovery/clusternode (live nodes) + the engine's own
  anchor pool (warm_prefix per cell) is the wiring step. The warmth number
  must come from pick_anchor, not be guessed — the doc says why.
- **C11-18b — coordinator::Mode should choose HOW MANY cells**, then place()
  picks which. Today Mode plans workers and place() is unaware of it; the
  join is small and makes aggressive fan-out warmth-aware.
- **C11-18c — preemption, if ever**: needs numbers proving interrupting a
  warm cell beats waiting. Deliberately unbuilt.
- **C11-18d — the strategy doc itself.** If the original
  HiveMind_Strategy_Logscope.md still exists outside the repo, diff it
  against 11-hive-matrix.md; anything it mandates that the code does not do
  is a gap worth listing rather than silently dropping.
