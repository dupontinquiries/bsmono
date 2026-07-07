# HANDOFF — items tooltip wiring (`r330-items`)

**Pure core delivered:** `crates/unendlich-engine/src/sim/item_tooltip.rs`
(`compose`, `wrap_lore`, `rarity_name`, types `ItemTooltipView` / `AffixView` /
`TooltipLine` / `TooltipRole` / `Emphasis`). **PROPOSAL ONLY — not wired.** The bin
edit below is for the coordinator/operator to apply on-device (SHARED_RULES §2).

## What this replaces
`render/world_renderer.rs` already has a "canonical tooltip" concept:
- **`UiState.craft_lines: Vec<(String, bool, String, String)>`** — `world_renderer.rs:462`;
  the 4th field is the *canonical tooltip* (r176 comment: "doc format: effects, italic
  lore, attribution"), consumed at **`:3941`–`:3948`** as
  `b.on_hover_text(tip.clone())` — a single flat `String`, so it can't style rarity color
  or italicise lore.
- **In-hand / creative labels** use bare `items[selected].display_name()` (`:4186`) and
  the cell-grid short name (`:4243`) — name only, no rarity/affix/lore.

Today the `tip` string is assembled **in the bin** ad hoc. `compose()` is the pure,
tested core that should produce it — as ordered `Vec<TooltipLine>` (Title / Affix / Lore /
Flavor + emphasis), so the renderer can color the Title by rarity and render Lore italic
instead of dumping one grey blob.

## Proposed call-site
Wherever the bin currently builds that 4th `craft_lines` field (and the hover for the
creative picker cell at `:4236`–`:4243`):

```rust
use crate::sim::{item_tooltip, rarity_tier, affix_count, affix_magnitude};

// caller flattens the existing cores into the view (no new state):
let affix_labels: Vec<item_tooltip::AffixView> = affixes.iter().map(|a| {
    item_tooltip::AffixView { label: a.display.as_str(), magnitude: a.magnitude }
}).collect();
let view = item_tooltip::ItemTooltipView {
    name:        &kind.display_name(),                 // item.rs
    rarity:      rarity_tier::tier(quality_score),     // sim::rarity_tier
    kind_label:  Some(category_label(kind)),           // "Bending Instrument" etc.
    slot_label:  slot_label(kind),                     // "Off-hand / Focus slot" or None
    affixes:     &affix_labels,                        // built via affix_count/affix_magnitude
    lore:        lore_text,                             // design data string
    attribution: Some(attribution_line),               // "— Teaching inscription, …"
    lore_cols:   28,
};
let lines = item_tooltip::compose(&view);
```

**Interim, zero-risk swap** (keeps current egui hover API): collapse to the existing flat
string so behaviour is byte-compatible with r176, just sourced from the core —
```rust
let tip: String = lines.iter().map(|l| l.text.as_str())
    .collect::<Vec<_>>().join("\n");
// … craft_lines.push((name, can, have, tip));  // unchanged downstream
```
**Full styling upgrade** (recommended next): render `lines` directly in a
`egui::show_tooltip`, mapping `TooltipRole::Title` → rarity-tinted heading,
`Affix` → body, `Lore` → italic, `Flavor` → muted; `Emphasis` picks the weight.

## Notes / perf
- `compose` allocates one small `Vec<TooltipLine>` + the line strings per call. That's
  fine at hover/craft-menu cadence. If you ever build it **every frame** for the in-hand
  HUD (`:4186`), cache it and rebuild only when the selected item changes.
- No bin signatures change. `craft_lines`' tuple shape is untouched in the interim swap.
- `rarity_name`/tier ordering matches `sim::rarity_tier` (0 Common … 5 Mythic).
- The `magnitude` on `AffixView` is **ordering only** — the displayed text is the caller's
  pre-formatted `label`, so the core never does panicking float formatting.
