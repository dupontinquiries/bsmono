# hatch — parity and debt ledger

House convention (`PARITY.md`, per-project): record what was *not* carried over, what was
simplified, and what has not actually been observed working — so the next session inherits facts
instead of assumptions. Nothing here is a bug report; these are known, chosen, or unverified.

The plasmoid (`apps/hatch/qml`) is the original; `hatch-egui` is the port. `maintenance/tools/
parity_audit.py` now includes hatch (added alongside this file) — its 25% is a file-name-presence
measurement and reads worse than reality, per below.

## What the audit's 6 "absent" surfaces actually are

`parity_audit.py` flags a QML surface absent if its filename never appears in the port's source.
That is the right default and the wrong tool for a monolithic `lib.rs` (1,256 lines, no per-view
file split), so most of these are naming mismatches, not missing features. Checked each by hand:

- **`DashboardRepresentation`, `ItemGridView`, `ActionMenu` — false positives, functionality
  exists.** The fullscreen grid and long-press context menu are all in `lib.rs` under hatch's own
  names, not the plasmoid's. `a_long_press_opens_the_menu_and_swallows_the_release_click` in
  `lib.rs`'s tests exercises the ActionMenu-equivalent path directly.
- **`CompactRepresentation` — does not apply, not a gap.** This is the Plasma panel-button applet
  representation (the icon that lives in the taskbar and opens the dash). hatch-egui is a
  standalone toggled window (`hatch-egui --toggle`, single-instance poke-file), not a panel
  applet, so there is no panel-button concept to port. Recorded so nobody spends time on it later
  under the audit's prompting.
- **`ItemGridDelegate` — real gap, see below.** This is where the missing feature actually lives.
- **`ConfigGeneral` — real gap, see below.**

Net: 2 of the 6 are real, not 6.

## Open

- **Per-tile hover/press animation (`iconAnimationStyle`) is not ported.** The plasmoid's
  `ItemGridDelegate.qml` plays a bounce or wiggle on hover/press, driven by
  `Plasmoid.configuration.iconAnimationStyle`; `anim.rs`'s `Board` only carries the *grid-level*
  motion (`SlidePattern`: enter/restack/exit, 6 patterns + `None`, fully ported and tested). No
  file under `egui-frontend/src/` or `hatch-core/src/` references bounce, wiggle, or any per-tile
  press animation. `LAUNCHER_MERGE.md` describes this system as "eight icon styles" but the
  delegate's own code comment says only ids `0`, `3`, and `6` are meaningfully distinct now
  (anything else legacy-coerces to bounce) — confirm the live count with whoever wrote that
  comment before porting a number that may already be stale in the source of truth itself.

- **No in-app settings UI at all.** The plasmoid's `ConfigGeneral.qml` exposes ~15 user-facing
  options (grid columns/rows, icon sizes, alpha sort, recent ordering, background opacity, custom
  button image, extra runners, and more) through Plasma's KCM config system. `hatch-core::Config`
  already carries backing fields for several of these — `columns`, `icon_px`, `opacity`,
  `animations`, `anim: BoardConfig` — so the *model* supports them; grep confirms zero hits for a
  settings/config panel function anywhere in `egui-frontend/src/`. Today, changing any of these on
  the egui side means hand-editing the config-kit ini file directly. Whether that's an acceptable
  permanent state (power-user tool) or a launcher-blocking gap (phone users won't hand-edit ini
  files) is a product call, not an engineering one — flagging rather than deciding.

- **`uikit-egui`'s `adaptive.rs` is unused — mechanism now confirmed, fix deliberately not attempted.**
  `HatchUi::ui()` sets `cell = Vec2::new(icon_px + 44.0, icon_px + 40.0)` straight from
  `config.icon_px` (default 64, so a ~108×104pt cell) — no `DeviceClass` branch, no
  `Adaptive::with_min_touch_target` floor. In practice this is lower-severity than "unused"
  alone suggested: `icon_px` lives in a per-machine XDG config, so a phone and a desktop running
  hatch have independent files unless something explicitly syncs dotfiles between them — this
  isn't one config value silently fighting two screens.

  What's still real: nothing enforces a minimum comfortable touch target on the phone
  independent of whatever `icon_px` happens to be set to (default or user-changed), and nothing
  gives phone/desktop different *defaults* the way `Adaptive::pick(phone, tablet, desktop)` exists
  to do. I'm not fixing this now — picking an actual minimum touch-target size (44pt? 48pt?
  something FP5-DPI-specific?) is an ergonomics judgment call that wants a real device in hand,
  not a guess from a sandbox with no display. Wiring `Adaptive::detect`/`with_min_touch_target`
  in without a real number to give it would just replace an honest fixed constant with a
  dishonest-looking "adaptive" one that was never tuned against anything. Flagging with the
  mechanism now fully named so whoever has the phone can wire the actual number in directly.

## Closed

- ~~**`uikit-egui`'s `a11y.rs` is unused.**~~ Closed. Every tile `grid()` paints now registers a
  `Semantics::button` into an `A11yScene` field on `HatchUi`, keyed by a hash of the entry's own
  id (not grid position — the grid reorders every keystroke as favorites/recents float, and
  position-keyed ids would make focus visually stick to a screen slot across a reorder instead of
  following the app actually selected). `sel` — hatch's own 2D, row/col, edge-wrapping keyboard
  index — stays the single source of truth for movement; `A11yScene::apply(Action::Next/Prev)` is
  linear and would have regressed grid-shaped arrow-key nav, so instead of routing keyboard input
  through it, a small additive `A11yScene::set_focus(Option<u64>)` was added upstream in
  `shared/uikit/crates/uikit-egui/src/a11y.rs` (tested there directly) and `sel` is pushed into it
  every frame — the scene reports, it doesn't decide. `voiced` grammar registration and actual
  on-device screen-reader output are still unconnected; this closes the "tiles carry no semantics
  at all" gap, not the full voice pipeline.

  Compiled and tested for real, not just reasoned about: `cargo check`/`cargo test` both green for
  `uikit-egui` (13/13 a11y tests) and `hatch-egui` (26/26, including two new: one asserting every
  painted tile gets a collision-free registration and focus follows `sel` including after an
  arrow-key move, one asserting a zero-result query leaves no stale focus behind). Not run on an
  actual display — this sandbox has none — so the *visual* side (nothing currently paints a focus
  ring; the existing accent-highlight for `selected` was left as the only visual affordance rather
  than adding a second one) is unverified beyond the headless shape tests.

## Closed / not actually gaps (checked, ruled out)

- ~~**SVG icons rasterize at intrinsic viewBox size (blurry upscale).**~~ Not a bug.
  `icons_img.rs::decode_svg` rasterizes at the tile's on-screen `target_px`, confirmed by
  `an_svg_rasterises_at_the_requested_size`.
- ~~**`gesture_layer.rs` (uikit-egui) unused — is that a duplicated gesture implementation?**~~
  No. That module is swipe/page navigation (`Axis`, `horizontal_page`, `drag_owner`); hatch's grid
  doesn't page horizontally. The long-press-to-favorite timer is hand-rolled
  (`resp.is_pointer_button_down_on()`) because it solves a different problem than what
  `gesture_layer` offers — not a missed reuse.
- ~~**`icontheme.rs`/`anim.rs`/`Board` were only partially ported from `apps/launcher`.**~~ No —
  `LAUNCHER_MERGE.md`'s steps 1–2 check out: full freedesktop icon-theme resolution (9 tests) and
  the tick-based animation Board (headless, tested) both landed intact in `hatch-core`.

## Not verified in-sandbox

- **hatch-egui has never been run.** This environment has no display and no Wayland/X11 session.
  What has been observed: `cargo test` results claimed in `LAUNCHER_MERGE.md` (hatch-core: 70
  tests) and the source-level checks in this file — not a running window, not an actual long-press
  gesture, not actual tile animation on screen.
- **`kicker` favorites bridge (`plasma.rs`) has never read a real `appletsrc`.** Its tests
  construct one by hand; whether a real Plasma Mobile install's file parses the same way (kicker
  version differences, an unexpected containment shape) is unconfirmed.
