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

- **Per-tile hover/press animation (`iconAnimationStyle`) is not ported — and the original's own
  squish-as-a-hover-style path (id 6) is confirmed dead code, not a missing style.** The QML
  schema comment reads `<!-- 0 none, 1 bounce, 3 wiggle, 6 squish; other legacy ids map to bounce
  -->`, but `ItemGridDelegate.qml`'s hover handler only branches on `iconStyle === 1` (bounce) or
  `=== 3` (wiggle) — `6` matches neither branch, so selecting "squish" as a hover style produces
  no hover animation at all in the plasmoid itself. Squish already happens unconditionally on
  *press* (`squishIn`/`squishOut`), independent of `iconStyle` entirely — so porting "four icon
  styles" would mean porting a selector option that has never done anything in the original. The
  real, live set is 3: none/bounce/wiggle, hover-triggered.

  Separately: hatch-egui's existing press-scale (`tile()`'s `animate_bool_with_time`-driven
  `scale = 1.0 - 0.10 * t` over 80ms) already covers squish's job approximately — QML's
  `squishIn` goes to 0.86 over 70ms then eases back over 90+110ms asymmetric; egui's is a
  symmetric ease to 0.90 over 80ms. Different curve, same intent, already there. So the actual
  remaining gap is narrower than originally scoped: **bounce and wiggle, hover-triggered, are the
  only two behaviors with nothing on the egui side at all.** Not implemented in this pass —
  deprioritized in favor of the `Board` finding above, which affects every tile on every screen,
  versus this, which only fires on a hover event a touchscreen mostly won't send (hover is a
  desktop-only signal via mouse; on the phone target this pair of styles may barely ever trigger,
  worth weighing against implementation cost before doing the work).

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

- ~~**`hatch-core::anim::Board` is never called from `hatch-egui` — the grid has no motion at
  all.**~~ Closed — the headline finding of this whole review, now wired. `grid()` no longer
  lays tiles out with a static row/col loop; it calls `Board::retarget`/`tick` every frame from
  `self.core.results()`'s live order and paints each tile at its returned `(x, y, scale, alpha)`
  via `ui.interact(rect, stable_id, sense)` at an absolute position, rather than through egui's
  layout cursor. `board.config` is read from `core.config.anim` every frame — no extra clamp
  needed, since `Config::load()` already zeroes every duration when `animations=false`, exactly
  per that loader's own documented "one code path" intent.

  Required one small, justified addition to `hatch-core`: `Launcher::entry(id)`, a lookup that
  works regardless of the current query filter — `results()` alone can't find a tile that's mid
  exit-fade after being filtered out, and the shell still needs its name/icon for those last
  frames. Tested there directly.

  Two things intentionally simplified, recorded rather than hidden: cell size is now square
  (`Vec2::splat`, was a 4px-off rectangle) because `GridSpec`/`Board` assume one `cell: f32` for
  both axes; and `TilePose::rot` (used only by the Scatter slide pattern) is not applied to the
  painted icon — rotating a `p.image()` needs a transformed mesh, real additional work, and
  skipping it only mutes one pattern's flourish, not its position or timing.

  Compiled and tested for real: `cargo test` green across all three touched crates —
  `hatch-core` 71/71 (+1), `uikit-egui` 456/456 (unaffected), `hatch-egui` 29/29 (+3 new tests
  that directly prove the wiring is genuine rather than decorative: first paint is not already
  settled, a zero-duration config settles on frame one, a filtered-out tile survives one frame
  mid-fade before disappearing). Fixed two latent test-staleness bugs this exposed along the way
  (`open_menu` caching a tile's pre-entrance position; the empty-query test not waiting for the
  exit-fade) — neither was a production bug, both were tests assuming a static layout that no
  longer holds. `Harness` gained a synthetic clock (`RawInput.time`, stepped 1/60s per frame)
  instead of egui's real-wallclock fallback, matching `anim.rs`'s own no-real-sleep testing style.

  Not run on a real display — no GPU/session in this sandbox — so the actual on-screen look
  (does the stagger read as intended, does the ease feel right) is unverified beyond headless
  position/scale/alpha assertions.

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
- ~~**`icontheme.rs`/`anim.rs`/`Board` were only partially ported from `apps/launcher`.**~~
  Half-right, corrected above. `LAUNCHER_MERGE.md`'s steps 1–2 do check out as far as they go:
  full freedesktop icon-theme resolution (9 tests) and the tick-based animation `Board` (headless,
  tested) both landed intact in `hatch-core`. What this entry got wrong when I first wrote it: it
  stopped at "the code exists and its tests pass" and never checked whether `hatch-egui` calls
  it. It doesn't — see the `Board` entry under Open, which replaces this claim rather than sitting
  next to it.

## Not verified in-sandbox

- **hatch-egui has never been run.** This environment has no display and no Wayland/X11 session.
  What has been observed: `cargo test` results claimed in `LAUNCHER_MERGE.md` (hatch-core: 70
  tests) and the source-level checks in this file — not a running window, not an actual long-press
  gesture, not actual tile animation on screen.
- **`kicker` favorites bridge (`plasma.rs`) has never read a real `appletsrc`.** Its tests
  construct one by hand; whether a real Plasma Mobile install's file parses the same way (kicker
  version differences, an unexpected containment shape) is unconfirmed.
