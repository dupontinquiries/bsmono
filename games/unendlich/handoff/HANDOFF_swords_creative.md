# HANDOFF — `r332-swords-creative` (bin wiring, done on-device)

The pure half is delivered: `item::legendary_palette() -> Vec<ItemKind>` (additive, 48 entries).
The rest is bin/UI glue (can't compile in-sandbox per SHARED_RULES §2) — proposed below, not wired.

## 1. A `Legendaries` creative tab
The creative palette is selected by `crate::inventory_screen::Tab` (the enum `creative_palette`
in `item.rs` matches on, and `world_renderer.rs` renders). Mirror the `SpawnEggs` path:

- **Tab enum** (`inventory_screen.rs`, the `pub enum Tab`): add a `Legendaries` variant
  (after `SpawnEggs`).
- **`item::creative_palette`** (`item.rs`): add
  `Tab::Legendaries => legendary_palette(),`
  next to `Tab::SpawnEggs => spawn_egg_palette(),`.
- **`world_renderer.rs` palette wiring** — at the `Tab::SpawnEggs` arm (currently **line 3008**:
  `Tab::SpawnEggs => crate::item::spawn_egg_palette(),`) add directly below:
  `Tab::Legendaries => crate::item::legendary_palette(),`
- **Title string** (`world_renderer.rs` ~**4209–4211**, alongside `Tab::SpawnEggs => "creative · eggs"`):
  `Tab::Legendaries => "creative \u{00b7} legendaries",`
- **Tab button label** (`world_renderer.rs` ~**4224–4226**, alongside `Tab::SpawnEggs => "Eggs"`):
  `Tab::Legendaries => "Lgnd",`  (short, fits the existing button row)
- Optionally make it the default landing tab — leave `creative_tab` (line **1390**) as `Tab::Items`.

These are exhaustive-match additions: once `Tab` gains `Legendaries`, the compiler flags every
`match tab` that needs the new arm — the four spots above are those sites.

## 2. In-hand behaviour: a legendary equips, it does not place
A legendary in hand must **not** place a block. The pure side already enforces this: after the
swords merge, `spawn.rs::resolve_use` (line **331**) has a `ItemKind::Legendary(_) => UseAction::Nothing`
arm (sibling of `Ingot|Tool => Nothing`, line **342**), so "use" on a world cell is a no-op for
legendaries — no spurious placement. The actual **equip** (legendary → active weapon slot) is the
held-item/equip path, which is on-device:

- In the hotbar/use handler, branch *before* `resolve_use`: if the held `ItemKind` is
  `Legendary(spec)`, route to equip (set the active weapon to `spec.weapon()` —
  `legendary_weapon::forge(spec)`), rather than to `resolve_use`'s place/spawn path.
- `spec.weapon()` reconstructs the full `Weapon` stat-block on demand (items stay compact — just
  the `LegendarySpec` forge-tuple), so nothing needs to store the stat-block in the inventory.

## Notes
- `legendary_palette()` relies only on the r332 swords substrate (`ItemKind::Legendary`,
  `LegendarySpec`, `WeaponClass::ALL`, `spec.weapon()`); no new deps, no shared-file edits beyond
  the one `creative_palette` arm above (which is itself part of the proposed wiring, not the pure
  deliverable).
- Palette size is 48 (8 classes × 6 elements). If the button row is tight, the tab can paginate
  exactly like a large `Items` tab — no change to `legendary_palette` needed.
