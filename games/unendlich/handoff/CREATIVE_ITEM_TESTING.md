# Creative: make items testable + fillable inventory (r657)

## The root problem (why items couldn't be tested)
There were THREE decoupled "creative item" systems:
- the inventory UI showed `item::creative_palette(tab)` (the full catalog);
- placement used a *separate* bin-side `PLACE_PALETTE` of 9 fixed blocks, indexed
  by the click, gated to `< 9`;
- a third `sim::creative_tabs` list drove the G/V keyboard cycling.
So clicking a catalog item set an index that mapped to a *different* block (or was
ignored past slot 9). Most catalog items were simply not placeable.

The r655 drag-to-hotbar also relied on egui drag-and-drop that didn't work in
practice, so it's been removed in favour of clicks.

## The fix — one source of truth
The renderer now publishes the item the player actually picked in
`WorldRenderer::creative_place_item: Option<ItemKind>`, set on:
- clicking a catalog item (any tab),
- clicking a bottom-hotbar slot that holds an item,
- clicking the inline HOTBAR switcher,
- clicking a slot in the new 3×9 inventory.
The bin reads it each frame and, for `ItemKind::Block(b)`, makes `b` the creative
placement block — so **every catalog block is now testable by clicking it**. The
G/V keyboard cycling mirrors its pick into the same field, so keys and clicks stay
consistent (no per-frame fighting).

## Inventory + hotbar (all click-based, no drag)
- `creative_hotbar: [Option<ItemKind>; 10]` + `creative_hotbar_sel` — the bottom
  bar. Clicking a catalog item drops it into the active slot; clicking a slot makes
  it active and puts its item in hand.
- `creative_inventory: [Option<ItemKind>; 27]` — the **Inventory tab is now a real
  3×9 grid** (it used to render the catalog's empty "Inventory" category). The
  player fills it by clicking catalog items (deduped, first-empty slot); clicking a
  filled slot puts that item in hand.

## Known follow-ups (flagged honestly)
- Only **blocks** place. Spawn eggs / ingots / tools / legendaries become the
  `creative_place_item` but the bin only maps `Block(_)` → placement; wire the
  other `ItemKind`s to their actions (egg → spawn creature, etc.) next. The spawn
  path already exists elsewhere in the bin.
- `creative_inventory`/`creative_hotbar` are session state (not persisted).
- Right-click-to-remove from the 3×9 / hotbar isn't wired yet.

## Verify on build
All edits delimiter-checked; render + bin need your build to compile-confirm.
Playtest: creative → open inventory → click a block on any tab → it appears in the
hotbar + the Inventory tab's 3×9 → place it in the world and confirm it's that block.

## r659 — spawn eggs now hatch
Picking a **spawn egg** in the creative inventory sets `creative_place_item` to
`ItemKind::SpawnEgg(kind)`. Pressing **H** now hatches THAT species (in front of
the player, on the surface) instead of cycling the roster — it reuses the existing
H-key Roamer construction, so genome/senses/turn-profile are all set up normally.

Item-type model so far:
- **Block** → right-click places it (r657).
- **SpawnEgg** → H hatches the selected species (r659).
- **Ingot / Tool / Legendary** → carried/selectable but no creative place/spawn
  action yet (survival gives these to the hand; a creative "give to hand" is the
  next step if you want it).

Right-click while holding a non-block still places the last block (selected_block
is only updated for blocks). Making right-click spawn the egg directly is possible
but the place handler is deeply interwoven with combat/feeding — H is the safe path.
