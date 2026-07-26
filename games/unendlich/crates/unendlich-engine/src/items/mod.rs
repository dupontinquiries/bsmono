//! Items and inventory — gameplay item storage, decoupled from block defs.
//!
//! # Why decoupled
//!
//! Concrete item/block definitions live in `blocks.rs` (the block enum, the
//! eventual stable-hash id registry, field-derived signatures). This module
//! must not depend on that, the same way [`crate::sim`] collides against the
//! world through a [`crate::sim::physics::WorldQuery`] rather than knowing the
//! block enum. So items are referred to by an opaque [`ItemId`], and the one
//! thing the inventory needs to know about an item — how many fit in a stack —
//! comes through the [`ItemRegistry`] seam. The concrete registry (mapping
//! `BlockId`/tools → ids and stack sizes) plugs in on the game side.
//!
//! # Determinism
//!
//! [`Inventory`] is integer-only, has deterministic iteration order, and
//! exposes a stable [`Inventory::checksum`]. It is standalone presentation /
//! session state today, but is built so it can be folded into the rolled-back
//! sim state later, when item pickup/drops need to be networked and
//! mispredicted-then-corrected like everything else.

use std::collections::HashMap;

pub mod inventory;

/// Shapeless crafting over the inventory.
pub mod crafting;
pub mod recipe_gen;
pub mod recipe_build;
pub mod inventory_filter;

/// Item drops — fallen item entities with pickup-by-proximity.
pub mod drops;

/// Loot tables — what a broken block yields (deterministic, tool-gateable).
pub mod loot;

/// Debug HUD that renders the hotbar via the existing `debug_overlay` — no GUI
/// framework or new dependency required. See `docs/ITEMS.md`.
pub mod debug_hud;

/// egui debug UI. Gated behind the `egui_debug` feature so the engine builds
/// without egui unless you opt in (see `docs/ITEMS.md`).
#[cfg(feature = "egui_debug")]
pub mod debug_ui;

pub use inventory::Inventory;

/// Opaque handle for an item kind. `0` is reserved as "no item" so callers can
/// use it as a sentinel if they ever need a non-`Option` representation; the
/// inventory itself uses `Option<ItemStack>` for empty slots instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ItemId(pub u32);

impl ItemId {
    pub const NONE: ItemId = ItemId(0);
}

/// A non-empty pile of identical items. An empty slot is `None`, never a
/// `count == 0` stack — that invariant is upheld by [`Inventory`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ItemStack {
    pub item: ItemId,
    pub count: u16,
}

impl ItemStack {
    pub const fn new(item: ItemId, count: u16) -> ItemStack {
        ItemStack { item, count }
    }
}

/// The inventory's view of an item kind. The only property storage needs is
/// the stack ceiling; richer item data (names, models, behaviour) stays on the
/// game side.
pub trait ItemRegistry {
    /// Maximum count of `item` in a single stack (e.g. 64 for blocks, 1 for
    /// tools). `0` means the item cannot be stored at all.
    fn max_stack(&self, item: ItemId) -> u16;
}

/// Every item shares one stack ceiling — a fine default before per-item data
/// exists.
pub struct UniformRegistry(pub u16);

impl ItemRegistry for UniformRegistry {
    fn max_stack(&self, _item: ItemId) -> u16 {
        self.0
    }
}

/// Per-item stack ceilings over a default — handy for tests and tools.
pub struct TableRegistry {
    table: HashMap<ItemId, u16>,
    pub default: u16,
}

impl TableRegistry {
    pub fn new(default: u16) -> TableRegistry {
        TableRegistry { table: HashMap::new(), default }
    }
    pub fn set(&mut self, item: ItemId, max: u16) {
        self.table.insert(item, max);
    }
}

impl ItemRegistry for TableRegistry {
    fn max_stack(&self, item: ItemId) -> u16 {
        *self.table.get(&item).unwrap_or(&self.default)
    }
}

// ── Debug palette ────────────────────────────────────────────────────────

/// One selectable item in the debug palette: an id, a display name, and a
/// swatch colour for the UI. `item` ids here mirror the engine's `BlockId`
/// numbers (Stone=1, Grass=2, …) so the debug inventory lines up with real
/// blocks; map them to your `BlockId` on the game side.
#[derive(Clone, Copy, Debug)]
pub struct PaletteEntry {
    pub item: ItemId,
    pub name: &'static str,
    pub color: [u8; 3],
}

/// A handful of common blocks to drop into the hotbar while testing.
pub fn default_palette() -> Vec<PaletteEntry> {
    vec![
        PaletteEntry { item: ItemId(1), name: "Stone", color: [128, 128, 128] },
        PaletteEntry { item: ItemId(2), name: "Grass", color: [96, 176, 64] },
        PaletteEntry { item: ItemId(3), name: "Dirt", color: [134, 96, 67] },
        PaletteEntry { item: ItemId(12), name: "Sand", color: [219, 209, 160] },
        PaletteEntry { item: ItemId(17), name: "Oak Log", color: [110, 81, 47] },
        PaletteEntry { item: ItemId(9), name: "Water", color: [64, 108, 220] },
        PaletteEntry { item: ItemId(11), name: "Lava", color: [220, 90, 30] },
        PaletteEntry { item: ItemId(7), name: "Bedrock", color: [40, 40, 44] },
        // r481: forged weapons (ids 4000..4007, one per WeaponClass). Names +
        // swatches for the hotbar / drop cubes; geometry lives in sim::viewmodel.
        PaletteEntry { item: ItemId(4000), name: "Dagger", color: [180, 188, 200] },
        PaletteEntry { item: ItemId(4001), name: "Blade", color: [196, 168, 96] },
        PaletteEntry { item: ItemId(4002), name: "Greatsword", color: [210, 214, 224] },
        PaletteEntry { item: ItemId(4003), name: "Axe", color: [150, 120, 80] },
        PaletteEntry { item: ItemId(4004), name: "Greataxe", color: [120, 96, 64] },
        PaletteEntry { item: ItemId(4005), name: "Spear", color: [170, 178, 190] },
        PaletteEntry { item: ItemId(4006), name: "Staff", color: [196, 168, 96] },
        PaletteEntry { item: ItemId(4007), name: "Wand", color: [150, 160, 220] },
    ]
}
