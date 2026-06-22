# Bin shell adoption — wiring + the seed constraint I found

## The behaviour-preserving wiring (bin-only, compile-on-end)
Construct `App` once, map the renderer's existing menu/wizard signals into
`AppInput`, call `App::tick`, act on `AppStep` — replacing the hand-rolled match
at unendlich_fullgame.rs ~3626-3722:
```rust
use unendlich_engine::app::{App, AppInput, AppStep};
use unendlich_engine::sim::title_flow::MenuId;
let mut app = App::with_defaults();
// each frame, translate the renderer's current menu_choice / wizard_action:
let input = match (renderer.menu_choice, renderer.wizard_action) {
    (1, _) => AppInput::Select(MenuId::NewGame),
    (4, _) => AppInput::Select(MenuId::Quit),
    (_, 2) => AppInput::WizardNext,
    (_, 1) => AppInput::WizardBack,
    (_, 3) => AppInput::Confirm,           // Create
    _      => AppInput::None,
};
renderer.menu_choice = 0; renderer.wizard_action = 0;
match app.tick(input, frame_entropy) {
    AppStep::Menu => { renderer.menu_open = app.menu().screen()==Screen::Title;
                       renderer.wizard_open = app.menu().screen()==Screen::NewGame; }
    AppStep::Launch(plan) => begin_world_from(plan),  // see below
    AppStep::Quit => window.set_should_close(true),
}
```
Once `menu_view` is adopted in the renderer overlay, the `menu_choice` ints go away
entirely and the panel returns `MenuClick` directly.

## CONSTRAINT: "Create" cannot change the world until the generator is rebuildable
`TerrainGenerator::new(WORLD_SEED_DEFAULT, …)` is built ONCE at startup (line ~315),
and the field WorldModel too (~365), BEFORE the menu runs. So `AppStep::Launch(
StartLocal{ seed })` has a real seed, but `begin_world_from` can't apply it without
tearing down + rebuilding the terrain generator + GPU terrain resources + re-streaming
chunks. That generator-lifecycle restructure is the actual unit of work behind the
"regenerate worldgen from spec.seed" TODO — and it's the thing that makes Create
*visible* (different seeds → different worlds). Until then the shell is behaviour-
preserving plumbing only (no visible change), which is why it's worth pairing with
either that restructure or the offscreen-overlay (so the render path is verifiable
here) rather than shipping alone.

## APPLIED (r86): the menu/wizard block now runs through `App`
unendlich_fullgame.rs: removed `fg_wizard`/`fg_catalog`, added `fg_app = App::with_defaults()`,
and replaced the hand-rolled menu_choice/wizard_action handling with: translate the panel's
signals → one `AppInput` → `fg_app.tick` → act on `AppStep` (Launch enters the spawn world for
now + `enter_game()`; Quit closes) → reflect `fg_app.menu()` state back into the display-only
panel, gated on `state()==MainMenu`.
COMPILE-ON-END: the bin needs glfw (can't build in-sandbox). The App logic is unit-tested and the
lib compiles. If the build trips, the likely spots are: `NewGameWizard::config()` accessor shape,
or an enum variant name — paste the error and it's a one-line fix. Behaviour should match the old
menu; the win is it now flows through the tested App + collapses the dead menu chain.
