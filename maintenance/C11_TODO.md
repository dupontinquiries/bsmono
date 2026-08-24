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

- **C11-5a — `unendlich-netview` does not compile: unresolved import
  `netview`.** `verified` (it blocks a plain `cargo check` of the engine).
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
