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
