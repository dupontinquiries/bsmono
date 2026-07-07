# HANDOFF — nested submenu router (`r335-menus`)

**Worker:** `r335-menus`  **Status:** pure scaffolding shipped + gated; bin/overlay wiring is
*proposal-only* (SHARED_RULES §2).

## 0. What shipped (additive, in allow-list)
- `sim::menu_router` (+ `// r335-menus`): `MenuPage` (the one new enum — page identity for the
  nested tree), `MenuAction` (leaf-action ids), `ID_BACK`, and **`MenuTreeRouter`** — wraps the
  real `MenuNav` over the page tree plus a parallel `MenuPage` stack kept in lockstep
  (invariant `pages.len() == nav.depth()+1`).
- `sim::menu_screen_layout` (+ `// r335-menus`): **`main_menu_tree()`** and its sub-page builders
  composing `Root → { Singleplayer, Host → { World Management }, Join, Settings → { Graphics,
  Camera & Gameplay, Audio, Gamemode toggle } }` as `menu_nav::MenuItem` submenus.

`MenuTreeRouter` API: `new(tree)`, `move_focus(dir)`, `activate() -> NavEvent`, `back() -> bool`,
`current_page() -> MenuPage`, `focused_id()`, `depth()`, `pages()`. Activating a submenu descends
and pushes its `MenuPage`; a `Back` item or `back()` pops; an action returns `Activated(id)`
without changing the page.

## 1. Renderer wiring (propose — bin overlay)
The overlay draws from the router each frame:
- **Title:** `router.current_page().title()` ("Singleplayer", "World Management", …).
- **Items:** the current `MenuNav` frame's items (expose them if the overlay needs labels — today
  the renderer maps ids→labels; `MenuPage`/`MenuAction` give stable ids to switch on).
- **Focus highlight:** `router.focused_id()`.
- **Input:** Up/Down → `move_focus`; Confirm → `activate`; Cancel/Esc → `back()` (if it returns
  `false` at root, close the menu / return to the title or game).

This `MenuTreeRouter` is the *within-tree* navigator; it composes under the existing
`menu_router::Router` (the between-top-level-screen back-stack) untouched — e.g. push
`Router`'s `Screen::Settings` when the player opens this tree from the pause overlay.

## 2. Hook: gamemode toggle (T-gamemode)
The Settings page has an action leaf `MenuAction::GamemodeToggle`. On
`NavEvent::Activated(MenuAction::GamemodeToggle.id())`, call the T-gamemode model:
```rust
game_mode_state.toggle();           // sim::game_mode (r335-gamemode)
```
and refresh the creative picker visibility from `caps(mode).creative_inventory` (which gates
`item::legendary_palette()`). Optionally show the current mode as the item's value string.

## 3. Hook: World-Management page (T-worldmgmt)
`MenuPage::WorldManagement` (under Host) is a submenu whose children are placeholder action
slots (`OpenToDht`, `WhitelistToggle`, `MaxPlayers`, `WorldName`). On merge, replace those
placeholders with widgets bound to T-worldmgmt's `WorldMgmtSettings` validated setters, and on
the Open-to-DHT toggle call:
```rust
match world_management::apply_open_to_dht(&settings, self_id, role) {
    HostTransition::GoHost(advert) => /* start DHT announce loop (bin glue, §3c) */,
    HostTransition::CloseHost      => /* tear down host */,
    HostTransition::StayLocal      => {}
}
```
Toggling DHT is a session-lifecycle change (not live render) — gate it accordingly.

## 4. Merge notes
- The coordinator reconciles nothing in `mod.rs` (both `menu_router` and `menu_screen_layout`
  are already declared — this change touches only their bodies, additively).
- `MenuPage` is the single new enum this thread adds (per the brief's "ONE Screen enum addition
  max"); it is *separate* from `menu_router::Screen` and `title_flow::Screen` (page identity vs
  between-screen back-stack vs title-flow internal state) and adds no variant to either.
- Ids are namespaced: `MenuPage` `0xE000+`, `MenuAction` `0xE100+`, `ID_BACK` `0xE0FF` — disjoint
  from the Title screen's small widget ids, so no collision with `menu_screen_layout`'s existing
  `ID_*` constants (that module's own `ID_BACK = 100` is untouched; the tree uses
  `menu_router::ID_BACK` fully-qualified).

## 5. Verify commands I ran
```
rustc --edition 2021 --test flat_menus.rs && ./t   # 12 passed / 0 failed (real menu_nav embedded, rustc 1.96)
cargo check -p unendlich-engine --lib --tests --features "render-core egui"   # 0 errors (in-tree)
git apply --check -p1 CHANGES-r335-menus.patch     # CLEAN
```
Base: validated against `r330b` (the r335 base wasn't provided); the change is additive to two
existing modules and references only `menu_nav`/`menu_router` (no r335-only symbols), so it is
base-agnostic. Header is `// r335-menus` (my WID, per SHARED_RULES §4) — the brief's text said
`// r334-menus` as a leftover from the r334 re-issue; flagging the rename for the coordinator.
