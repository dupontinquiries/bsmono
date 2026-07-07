# Inventory Tools/Weapons tabs · menu responsiveness · load time (r620)

## 1. Weapons & Tools creative tabs — wiring
`sim::creative_items` is the pure model: `CreativeCategory` (All/Building/Natural/
Stone/Ingots/**Tools**/**Weapons**/Creatures), `category_of(ItemFacet)`,
`visible_indices(entries, tab, query, out)` (tab + search), `tab_counts`.

**Populate the palette in the bin** (enumerate the real `ItemKind`s once, at
menu build, then let `creative_items` bucket + filter them):
```rust
use unendlich_engine::item::{ItemKind, ToolKind, MetalKind, WeaponClass, LegendarySpec};
use unendlich_engine::sim::item_category;          // ItemFacet
use unendlich_engine::sim::creative_items::{Entry, CreativeCategory, visible_indices};

let mut palette: Vec<(ItemKind, String)> = Vec::new();
// blocks (existing creative_tabs palette) …
// TOOLS: every tool kind × metal
for &k in &ToolKind::ALL {
    for &m in &MetalKind::ALL {
        let it = ItemKind::Tool(k, m);
        palette.push((it, format!("{} {}", m.name(), k.name())));  // e.g. "Iron Pickaxe"
    }
}
// WEAPONS: one representative per forged class
for &c in &WeaponClass::ALL {                       // add `pub const ALL` to WeaponClass if absent
    let spec = LegendarySpec::base(c);              // a default/base spec for the palette
    palette.push((ItemKind::Legendary(spec), format!("{}", c.name())));
}
// spawn eggs, ingots … likewise.

// Build facet entries for the active tab + search box, then show visible:
let entries: Vec<Entry> = palette.iter()
    .map(|(it, name)| Entry { facet: item_category::facet_of(it), name })
    .collect();
let mut visible = Vec::new();
visible_indices(&entries, active_tab, &search_text, &mut visible);
// render palette[i] for i in visible
```
`item_category::facet_of(&ItemKind)` maps Tool→`ItemFacet::Tool`, Legendary→
`ItemFacet::Legendary`, etc. (add the one-line mapper if it isn't there yet).
Tabs with `tab_counts()[i] == 0` can be hidden. This gives the inventory its
Tools and Weapons tabs, populated and searchable.

## 2. Menu responsiveness
Likely causes and fixes (bin/render side):
- **Input polled at sim rate, not frame rate.** Menu navigation should read input
  every *frame* (and every egui pass), not once per fixed sim tick. If the menu
  update is inside the fixed-timestep loop, hoist input polling out so key presses
  register immediately.
- **egui repaint gating.** Ensure the menu/overlay calls `ctx.request_repaint()`
  on interaction so it doesn't wait for the next world frame; conversely, don't
  force a full-world redraw just to animate a menu.
- **Menu behind the upload/frame budget.** The perf env throttles chunk uploads
  (`UNENDLICH_MAX_UPLOADS_PER_FRAME`, `UNENDLICH_UPLOAD_BUDGET_MS=13`). Make sure
  UI submission isn't queued behind that budget — draw UI in its own pass that
  always runs.
- **Frame pacing:** `sim::refresh_snap` (r604) gives a stable present cadence
  (500 Hz target, snap ladder) — route the present rate through it so the menu
  isn't dithering.
- Key-repeat: add an initial-delay + repeat-rate for held nav keys so holding a
  direction scrolls smoothly (a small pure timer; can add to `menu_nav`).

## 3. Load time
- **Shader cache restored (this build):** all 35 `assets/shaders/*.spv` were
  recompiled fresh and their `.spv.hash` sidecars regenerated, so build.rs now
  **skips** shader recompilation (r610 had deleted the hashes, forcing a full
  recompile every build — slow). Keep the `.hash` files in the tree.
- **Forced CPU mesher (r612) is the *fast* path:** the CPU mesher is ~0.38 ms/
  chunk with no stalls; the octree GPU mesher (`=2`) is 38–171 ms/chunk on Asahi.
  So r612's forced CPU meshing also *reduces* load time — keep it until the
  planes are resolved, then choose per-GPU.
- **First-frame cost:** `UNENDLICH_RENDER_DISTANCE=4` + `UNENDLICH_PREFETCH*` are
  already tuned. For faster first playable frame, stream the near ring first and
  defer far LOD bands a few frames (prioritise `LOD::Near`).
- **Worldgen:** `UNENDLICH_MULTI_SCALE_CAVES=1` adds a cave pass per column; if
  load is column-bound, gating it to near chunks first shortens the initial gen.
