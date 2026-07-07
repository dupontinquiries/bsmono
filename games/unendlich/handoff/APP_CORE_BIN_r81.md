# r81 — the thin bin shell over engine::app

`engine::app::App` now owns the menu and composes it to a launch decision in
library code (4 passing tests). The bin stops hand-driving menu_open/menu_choice/
wizard_open flags and becomes a shell:

```rust
use unendlich_engine::app::{App, AppInput, AppStep};
let mut app = App::with_defaults(); // or App::new(catalog, settings)
// per frame, while in the menu:
let input = map_egui_clicks_to_app_input(&renderer); // Select/Back/Confirm/SelectSlot/Wizard*
match app.tick(input, frame_entropy) {
    AppStep::Menu => render_menu(app.menu()),        // draw app.menu().screen() + sub-state
    AppStep::Launch(plan) => match plan {
        LaunchPlan::StartLocal(init) | LaunchPlan::Host(init) =>
            begin_world(init.seed, init.mode, &init.name, init.multiplayer),
        LaunchPlan::Join => session_join(app.menu().browser()),
        _ => {}
    },
    AppStep::Quit => break,
}
```
The egui menu panel still lives in the renderer (compile-on-end), but it now only
*reads* `app.menu()` to draw and *writes* an `AppInput` back — no game state in the
bin. The four overlapping menu abstractions collapse: `MenuController` is the
backend, `App` is the composition, `title_flow`/`game_flow` are its internals,
`sim::main_menu` is now redundant and can be removed.
