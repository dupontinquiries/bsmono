# tomerge_18 reconciliation plan

The upload (183 MB) is app snapshots (zips) + an egui-kit exploration. Map + plan:

## What's in it
- **tomerge_18/egui-kit-exploration/** — the user's egui/gesture/kit spike. **Converged with my
  kits**: its `uikit-demo` has `uikit-gesture` / `uikit-egui` / `snapclone-theme` (same names).
  - `uikit-gesture`: SAME API as mine (TwoTierGesture: on_scroll/on_release/Outcome). Aligned.
  - `uikit-egui`: mine is a **superset** (theirs: gesture_layer/tokens/theme_bridge/carved/skin;
    mine adds a11y, adaptive, livebg, nav). No merge needed — mine is ahead.
  - `touchkit`: **NEW** multitouch recognizer (swipe/fling/momentum/pinch/tap/long-press via
    `GestureRecognizer::update(touches)`) — complements uikit-gesture's pager. **BROUGHT IN** as
    shared/touchkit (26th kit): compiles + 5 tests + doctest PASS on a real toolchain (their
    session had none; they'd asked for this confirmation). egui feature pinned 0.29 to match.
  - Sketches (egui_input bridge, snapclone photo grid, logscope strand timeline) kept under
    shared/touchkit/sketches/ as reference; the demo's 7 scenarios (Feel/Energy/Multitouch/
    Arena/Trace/Reach/Gallery) are a useful on-device test harness to import later.
- **tomerge_18/bump-projects/** — updated apps (zipped snapshots):
  - `voiceterm-w2q/` web2qml v0.1.903 (mail-signin-diagnosis, dep-bump-analysis).
  - `logscope/` 4 suite variants (codeblock-context, tour-voice-fixes, voice-nav-complete,
    voiceterm-full), 84 MB each.
  - `unendlich/` (96 MB).
- **tomerge_18/new-projects/** — av1tuner_m1, beatspace, cybertron-cmake, fao_codec, kRapper,
  paperbound.
- **existing/** — logscope + zips (cybertron, paperbound, unendlich snapshots).

## Kit rewrites to re-apply on the app bumps (my adoptions the bumps predate)
- **voiceterm**: `mod niftyplot/graph_layout/mermaid;` → `pub use nifty_core::…` (nifty-core
  extraction); the egui-frontend (terminal core + web→egui + viz + cell renderer); fastpath +
  voice pipeline; audiomix/mic-kit if it grew audio.
- **logscope**: mind-palace → `audiomix_kit::soundscape` bridge; voice commands → mic-kit
  broker + FallbackChain + voice-kit pipeline; touchkit on the strand timeline (their sketch).
- **unendlich**: soundscape step-up (entities → audiomix_kit::soundscape).
- **all egui apps**: uikit-egui (adaptive+a11y+livebg+nav) + apply_settings + touchkit where
  multitouch (pinch/zoom) is wanted.

## Approach (per the user's "rebase")
Kits are shared + ahead, so they DON'T get rebased — the **apps** do. Per bumped app: unzip →
diff its engines against my kit-adopted version → re-apply the thin adoption edits (mostly
`mod X` → `pub use kit::X` re-exports + egui-frontend crates, which are additive). New projects:
add under apps/, then adopt kits as they settle. This is incremental, one app per pass.

## logscope (done — first rebase)
- **Already integrated:** my monorepo's `apps/logscope/crates/logscope-egui` is byte-identical to
  the bump's voice-nav-complete variant (diff empty). The 4 variants differ only in
  mic-pearl-view.rs / chat.rs / voice_intent.rs sizes. So logscope didn't need re-importing.
- **egui version gap:** logscope-egui is on **egui/eframe 0.34** (kits are 0.29) with default
  features gui+gpu-scene. Direct uikit-egui/touchkit-egui adoption is **blocked until egui is
  unified** (bump kits → 0.34, or logscope → 0.29). The std-only kits adopt fine regardless.
- **Applied — the soundscape step-up:** `logscope-egui::spatial_audio::graph_soundscape` bridges
  the 3D conversation-graph layout (`spatial::layout` → Vec3 per node) to `audiomix_kit::soundscape`
  (project world-x→L/R, world-z depth→front/rear, normalize to the graph bounds). So a spatial
  view can *sound* where nodes sit. audiomix-kit is std-only → no egui conflict. Logic verified
  against audiomix-kit in isolation (leftmost node → front-left); full-crate build is on-device
  (egui 0.34 + gpu). gui-gated (uses spatial).
- **Flow-BACK, not push-in:** logscope's `fastpath.rs` (220) / `voice_intent*.rs` (multi-language)
  / `whisper_pool.rs` are the **mature originals** my voice-kit `context`/`pipeline`/`fastpath`
  abstracted this session. The right direction is voice-kit adopting logscope's real logic, not
  replacing it. Left as a noted follow-up (needs the real fastpath source diffed into voice-kit).

## voiceterm (egui-kit adoption — done, first uikit-egui multitouch consumer)
Chosen over logscope for the egui-kit work: voiceterm-egui + the kits are **both egui 0.29** (no
version gap, unlike logscope's 0.34), and it's the right touch target. Sequencing per the user:
land the adoption on the *stable* kit now, add transparency kit-side afterward so it flows to
every adopter.
- **Kit-side (benefits all egui apps):** promoted touchkit's egui bridge into
  `uikit-egui::touch` — `TouchSource` folds each frame's egui input (touch events + mouse-as-1-
  finger + trackpad wheel/zoom) into touchkit gestures (swipe/fling+momentum/pinch/tap/long-
  press). uikit-egui now deps touchkit (egui feature, 0.29). Builds.
- **voiceterm-egui adopts it:** App gains `TouchSource` + `GestureRecognizer`; `handle_gestures`
  polls per frame and pinch-zooms the terminal font (`apply_pinch`, clamped 7–48px, tested);
  fling momentum reports `gliding` so the driver repaints. Graceful: no touch → no-op. 29 tests.
  The on-device eframe main calls `handle_gestures(ctx)` then `render(ui)` (this crate is a lib).
- **Next (kit-side, then it flows):** add **transparency** to uikit-egui (palette alpha / a
  translucent surface token) — do it once in the kit and voiceterm + every port picks it up.
  Then the other egui-0.29 ports (rusty-board/bot-arena/bo-suite/snapclone) adopt `touch` the
  same way voiceterm did.

## egui version unification (logscope unblock — feasibility PROVEN, plan de-risked)
logscope-egui is on egui/eframe **0.34**; the kits + 5 ports are on **0.29**. To let logscope
adopt uikit-egui + the new `touch` multitouch, everything must land on one egui. Probed it:
- **egui 0.34 builds with rust 1.96** (verified in a scratch crate, ~47s).
- **The one real API break is `Rounding` → `CornerRadius`** (`::same(f32)` → `::same(u8)`; the
  probe confirmed `CornerRadius::same(4)` compiles on 0.34). Everything else the kits use
  (Color32/Stroke/Visuals/Painter/from_id_salt/zoom_delta/TouchPhase) is unchanged.
- **Scope: 10 crates** on egui 0.29, **~13 Rounding sites**: uikit-egui (a11y 2, theme_bridge 2,
  carved 5), rusty-board (2), snapclone (1), voiceterm/viz (1).
- **Why it's atomic:** bumping uikit-egui breaks all its 0.29 consumers at once, so the bump must
  land together: uikit-egui + snapclone-theme + touchkit(egui feature) + the 5 egui-frontends +
  settings-app, all 0.29→0.34, fixing the Rounding sites, verifying each. uikit-gesture is
  std-only (no bump). Then logscope drops the `uikit-egui`/`touch` deps in and adopts like
  voiceterm did.
- **Status:** feasibility + exact change nailed down; this is now a bounded mechanical PR, not an
  unknown. Left as the next focused task (needs the whole set green in one pass — not safe to
  half-land). Transparency + touch already landed on 0.29, and both port straight across (no
  Rounding involved), so no rework lost by bumping after.
