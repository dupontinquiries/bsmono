# MENU CLEANUP — the verdict, and the map

Operator question (2026-08-29): the Mode-screen menus were "hacked together
quickly multiple times" — are they scattered through hot loops waiting for a
cleanup + abstractions, or a rewrite?

## The verdict: CLEANUP BY MIGRATION, not a rewrite

Half the answer already exists in-tree. `src/menu.rs` is a clean menus-as-data
abstraction — rows/items/events, wrap-navigation that skips headings, scroll
windows, slider clamping — whose own header describes exactly the disease it
was written to cure ("nine screens in `Mode`, each with its own selection
index, its own scroll handling, its own arrow-key branch, its own draw
function; they agree about none of it"). It is proven: the shipboard station
consoles run on it, `ui::menu_rows`/`menu_hit` solved the draw-vs-hit geometry
split that made Maps "unmigratable", `ui::menu_cmds` draws it, and
`menusim.rs` photographs it in text (input tapes, no window). What never
happened is the migration of the old `Mode` screens onto it.

A rewrite would throw away the parts the operator explicitly likes — the
swipe/fling/scrub touch feel (`touch_menu_down`/`touch_scrub_to`/`mfling`)
and the arrow-key navigation model — which live in the SHARED machinery, not
in the per-screen duplication. The duplication is the disease; the machinery
is fine. So: migrate screen by screen, keep the machinery.

## The map (order = risk, easiest first)

Every migration is the same move: the screen becomes (a) a `fn rows() ->
Vec<menu::Row>` builder, (b) a `match` on `menu::Event` applying results,
(c) draw via `ui::menu_cmds`, mouse/touch via `ui::menu_hit`. Delete the
screen's private sel/scroll/key-branch/draw quadruplet as it lands.

1. **Pause** — ✅ DONE (2026-08-29): arrow-nav added the lightweight way
   (`Mode::Pause { sel }`, shared `pause_activate`, cyan keyboard rail) —
   Pause is a horizontal button bar, not a list, so it got nav without being
   forced into `menu.rs`'s vertical row model.
2. **Maps** — AUDITED, STAYS BESPOKE (2026-08-29): each row carries real art
   (grass/accent swatches of the map style, current-map marker, nets tags,
   gold selected edge) that generic `menu_cmds` rows would erase, and its
   input side already has wrap-nav, scroll, shared mouse + touch. Migrating
   it would trade art for process. The draw-vs-hit geometry fix that once
   blocked it (`menu_rows`/`menu_hit`) still stands available if its layout
   ever simplifies.
3. **Online** — ✅ MIGRATED (2026-08-29): the FIRST live `menu::Menu`
   consumer (the machinery was built + tested but had zero callers until
   now). `online_screen.rs` builds the six rows (two `Item::Field` — a new
   item added to menu.rs with input_char/backspace ops + Value::Text — and
   four Actions, LEAVE properly disabled while offline instead of the old
   no-op toast); `online_key` shrank to key-routing + event application;
   mouse hover/click go through `ui::menu_hit` so draw and hit share ONE
   geometry; the status/players list stayed as a bespoke live footer
   (`hud::online_footer`). Old `hud::online_menu`, `OnlineUi`, and the
   ONL_* geometry consts deleted. Suite 1222/0 (2 new). VISUALLY UNVERIFIED
   in-sandbox — eyeball pass on device, plus: hover highlight now follows
   the MENU's selection (mode.sel is a legacy mirror only).
4. **Tweaks** — the big one, but `tweak_rows()` is ALREADY data-driven
   (Slider/Toggle rows with get/set fns); it maps ~1:1 onto `menu::Item`.
   The touch slider-scrub (`tscrub`/`bar_t`) must keep working: route the
   scrub through `Event::Changed` with the same snap-to-step.
5. **Binds** — the `capturing` state is the odd one out; model capture as a
   screen-level flag over an `Action` row, not as a new Item kind.
6. **Catalog / Garage / Academy / Trials** — grid-ish pickers; migrate last,
   or leave (they are pickers with bespoke layouts more than list menus —
   forcing them into rows may not improve them).

Shared bits to KEEP as-is: `hover_row` (retarget it to `ui::menu_hit` per
migrated screen), the touch fling/scrub state machine, the nav strip
(`hit_nav`), `open_menu`/`menu_from_pause` return-path handling.

## Styling (done 2026-08-29)

The "neon glass" palette in `hud.rs` is the operator-directed interim mix of
cyberpunk × unendlich (deep glass backdrop, ice text, #6fc3ff accent,
electric-amber pick, magenta warn) applied through the shared constants so
every screen shifts together. The custom blackstar art style replaces it
later. VISUALLY UNVERIFIED in-sandbox (no display); the 1220-test suite is
green — colors are data, behavior is untouched.

## Cross-game note

unendlich's menu structural work is the mirror image: its LIVE menu is a
~2000-line egui closure inside `world_renderer::prepare` (locals-in/
write-back — a ready extraction seam), and it has no arrow-key navigation.
`menu.rs`'s navigation model (wrap, skip-unselectable, scroll-follow) is the
thing worth porting THERE once the closure is extracted; the two games then
share the navigation brain while keeping their own art.
