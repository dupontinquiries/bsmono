//! Item identity — what a slot or palette entry actually *is*. The storage grids
//! in `inventory.rs` are keyed by `BlockId`, but not everything a player carries
//! is a block: spawn eggs name a creature to summon. `ItemKind` widens the notion
//! of an item to cover both, and the palettes here populate the creative tabs the
//! inventory screen shows — the Items tab (placeable blocks) and the SpawnEggs tab
//! (the dragon and the Lauscher).
//!
//! Each creature has an elemental affinity, so its egg borrows that element's
//! emblem — a Wood-Dragon egg reads as a leaf, the air-listening Lauscher as a
//! gust. Pure data.

use crate::emblem::{self, Emblem};
use crate::sim::elemental::Element;
use crate::sim::legendary_weapon::{forge, Weapon, WeaponClass};
use crate::world::blocks::BlockId;

/// A summonable creature. The actual spawning (building its `Anatomy` / brain) is
/// gameplay wiring; this is just the identity an egg references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CreatureKind {
    /// The earth/wood dragon — branchy, vine-breathing.
    Dragon,
    /// The Lauscher ("listener") — an air-attuned creature that hears the world.
    Lauscher,
    /// The Späher ("watcher") — a winged sentinel bird. Sees and hears keenly;
    /// its alarm reveals players and hunting horrors to anyone watching it.
    Spaeher,
    /// Generic wild fauna — the everyday surface animals that fill the overworld
    /// (the general body plans, picked at spawn time).
    Fauna,
    /// Great reptilian megafauna of the warm, open lands.
    Dinosaur,
}

impl CreatureKind {
    pub const ALL: [CreatureKind; 5] = [
        CreatureKind::Dragon,
        CreatureKind::Lauscher,
        CreatureKind::Spaeher,
        CreatureKind::Fauna,
        CreatureKind::Dinosaur,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CreatureKind::Dragon => "Wood Dragon",
            CreatureKind::Lauscher => "Lauscher",
            CreatureKind::Spaeher => "Späher",
            CreatureKind::Fauna => "Wild Animal",
            CreatureKind::Dinosaur => "Dinosaur",
        }
    }

    /// The element this creature is attuned to.
    pub fn affinity(self) -> Element {
        match self {
            CreatureKind::Dragon => Element::Wood,
            CreatureKind::Lauscher => Element::Air,
            CreatureKind::Spaeher => Element::Air,
            CreatureKind::Fauna => Element::Wood,
            CreatureKind::Dinosaur => Element::Earth,
        }
    }

    /// The egg/creature's emblem, borrowed from its elemental affinity.
    pub fn emblem(self) -> Emblem {
        emblem::for_element(self.affinity())
    }

    /// A representative tint for the spawn egg in the picker.
    pub fn color(self) -> [u8; 3] {
        match self {
            CreatureKind::Dragon => [86, 125, 70],   // wood/earthen green
            CreatureKind::Lauscher => [171, 205, 222], // airy pale blue
            CreatureKind::Spaeher => [120, 126, 150],  // slate blue-grey
            CreatureKind::Fauna => [122, 96, 64],      // earthen brown
            CreatureKind::Dinosaur => [96, 112, 78],   // mossy reptilian green
        }
    }
}

/// A smeltable metal. Each is won from one ore block and smelted into an ingot,
/// which in turn becomes tool heads. Pure identity — the ore→ingot→tool chain
/// lives in `recipe.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetalKind {
    Iron,
    Gold,
    Copper,
}

impl MetalKind {
    pub const ALL: [MetalKind; 3] = [MetalKind::Iron, MetalKind::Gold, MetalKind::Copper];

    pub fn name(self) -> &'static str {
        match self {
            MetalKind::Iron => "Iron",
            MetalKind::Gold => "Gold",
            MetalKind::Copper => "Copper",
        }
    }

    /// A representative tint for ingots/tools of this metal in the picker.
    pub fn color(self) -> [u8; 3] {
        match self {
            MetalKind::Iron => [193, 193, 199],   // pale steel grey
            MetalKind::Gold => [222, 184, 64],     // warm gold
            MetalKind::Copper => [191, 107, 71],   // orange-brown copper
        }
    }

    /// The ore block this metal is smelted from.
    pub fn ore(self) -> BlockId {
        match self {
            MetalKind::Iron => BlockId::IronOre,
            MetalKind::Gold => BlockId::GoldOre,
            MetalKind::Copper => BlockId::CopperOre,
        }
    }

    /// Inverse of [`MetalKind::ore`]: which metal (if any) an ore block yields.
    pub fn from_ore(block: BlockId) -> Option<MetalKind> {
        MetalKind::ALL.into_iter().find(|m| m.ore() == block)
    }

    /// Mining tier of a tool head made from this metal. Higher tiers harvest
    /// harder blocks. Deliberate engine choice (not Minecraft's soft-gold
    /// trope): rarer metal = better tool, so Copper < Iron < Gold.
    pub fn mining_tier(self) -> u8 {
        match self {
            MetalKind::Copper => 1,
            MetalKind::Iron => 2,
            MetalKind::Gold => 3,
        }
    }
}

/// A tool's shape. The metal of its head is carried separately in
/// `ItemKind::Tool(kind, metal)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
}

impl ToolKind {
    pub const ALL: [ToolKind; 3] = [ToolKind::Pickaxe, ToolKind::Axe, ToolKind::Shovel];

    pub fn name(self) -> &'static str {
        match self {
            ToolKind::Pickaxe => "Pickaxe",
            ToolKind::Axe => "Axe",
            ToolKind::Shovel => "Shovel",
        }
    }

    /// How many ingots the head costs to forge. Broad heads (pick/axe) cost
    /// more than a single-scoop shovel.
    pub fn ingot_cost(self) -> u16 {
        match self {
            ToolKind::Pickaxe | ToolKind::Axe => 3,
            ToolKind::Shovel => 1,
        }
    }
}

// ── r330-swords ────────────────────────────────────────────────────────────
/// The compact, deterministic forge tuple that names a legendary weapon.
///
/// We store *this* on an item — **not** the full `Weapon` stat-block — so
/// inventory items stay small (`Copy`, 11 bytes of payload) and the stat-block
/// stays single-source: reconstruct it on demand with [`LegendarySpec::weapon`],
/// which is exactly `legendary_weapon::forge(seed, class, element_idx, tier)`.
/// `element_idx` is interpreted mod 6 (Wood/Water/Air/Dust/Fire/Storm), matching
/// `forge`'s own element→proc mapping; `tier` is the (uncapped) weapon tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegendarySpec {
    pub seed: u64,
    pub class: WeaponClass,
    pub element_idx: u8,
    pub tier: u8,
}

impl LegendarySpec {
    /// Reconstruct the full stat-block. Single source of truth: this is a thin
    /// call into `legendary_weapon::forge`, so the item never drifts from the
    /// forge. Pure, deterministic, panic-free (forge takes `element_idx` mod 6).
    pub fn weapon(&self) -> Weapon {
        forge(self.seed, self.class, self.element_idx, self.tier)
    }

    /// Element name from `element_idx` (mod 6), using the legendary system's own
    /// element table (the one `forge` documents) — Wood/Water/Air/Dust/Fire/Storm.
    /// Deliberately not coupled to `sim::elemental::Element`, whose variant set
    /// and order differ (it has Earth, no Storm).
    pub fn element_name(&self) -> &'static str {
        match self.element_idx % 6 {
            0 => "Wood",
            1 => "Water",
            2 => "Air",
            3 => "Dust",
            4 => "Fire",
            _ => "Storm",
        }
    }

    /// The weapon-class name (e.g. "Greatsword"), straight from the class enum.
    pub fn class_name(&self) -> &'static str {
        match self.class {
            WeaponClass::Dagger => "Dagger",
            WeaponClass::Blade => "Blade",
            WeaponClass::Greatsword => "Greatsword",
            WeaponClass::Axe => "Axe",
            WeaponClass::Greataxe => "Greataxe",
            WeaponClass::Spear => "Spear",
            WeaponClass::Staff => "Staff",
            WeaponClass::Wand => "Wand",
        }
    }

    /// Picker/hotbar tint: an element hue, brightened a little per tier so a
    /// mythic-tier blade reads hotter than a tier-0 one. Pure, always in range.
    pub fn display_color(&self) -> [u8; 3] {
        let base: [u8; 3] = match self.element_idx % 6 {
            0 => [96, 200, 96],   // Wood  — green
            1 => [80, 150, 230],  // Water — blue
            2 => [200, 230, 255], // Air   — pale sky
            3 => [180, 150, 110], // Dust  — tan
            4 => [230, 110, 60],  // Fire  — orange
            _ => [180, 120, 240], // Storm — violet
        };
        // Brighten with tier, capped so it never wraps or saturates to white.
        let boost = (self.tier as u16).saturating_mul(10).min(55);
        [
            (base[0] as u16 + boost).min(255) as u8,
            (base[1] as u16 + boost).min(255) as u8,
            (base[2] as u16 + boost).min(255) as u8,
        ]
    }
}
// ── end r330-swords ─────────────────────────────────────────────────────────

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Block(BlockId),
    SpawnEgg(CreatureKind),
    /// A smelted metal bar — the intermediate between ore and tool.
    Ingot(MetalKind),
    /// A finished tool: its shape and what metal its head is.
    Tool(ToolKind, MetalKind),
    /// r330-swords: a forged legendary weapon, carried as its compact forge
    /// tuple (reconstruct the stat-block via `LegendarySpec::weapon`).
    Legendary(LegendarySpec),
}

impl ItemKind {
    pub fn is_spawn_egg(self) -> bool {
        matches!(self, ItemKind::SpawnEgg(_))
    }

    /// A tint for the picker/hotbar tile, so every item kind renders uniformly:
    /// blocks use their mesh albedo, eggs their creature color, ingots and tools
    /// their metal color.
    pub fn display_color(self) -> [u8; 3] {
        match self {
            ItemKind::Block(b) => b.base_color(),
            ItemKind::SpawnEgg(c) => c.color(),
            ItemKind::Ingot(m) => m.color(),
            ItemKind::Tool(_, m) => m.color(),
            ItemKind::Legendary(spec) => spec.display_color(),
        }
    }

    /// Stack ceiling — blocks stack deep, eggs are precious, ingots stack like
    /// blocks, finished tools don't stack.
    pub fn max_stack(self) -> u16 {
        match self {
            ItemKind::Block(_) => 64,
            ItemKind::SpawnEgg(_) => 16,
            ItemKind::Ingot(_) => 64,
            ItemKind::Tool(..) => 1,
            ItemKind::Legendary(_) => 1,
        }
    }

    pub fn display_name(self) -> String {
        match self {
            ItemKind::Block(b) => format!("{b:?}"),
            ItemKind::SpawnEgg(c) => format!("{} Egg", c.name()),
            ItemKind::Ingot(m) => format!("{} Ingot", m.name()),
            ItemKind::Tool(t, m) => format!("{} {}", m.name(), t.name()),
            ItemKind::Legendary(spec) => format!("{} {}", spec.element_name(), spec.class_name()),
        }
    }
}

/// The SpawnEggs tab: one egg per creature.
pub fn spawn_egg_palette() -> Vec<ItemKind> {
    CreatureKind::ALL.iter().map(|&c| ItemKind::SpawnEgg(c)).collect()
}

/// Wrap a set of blocks as items (for a caller-supplied Items-tab catalog).
pub fn blocks_palette(blocks: &[BlockId]) -> Vec<ItemKind> {
    blocks.iter().map(|&b| ItemKind::Block(b)).collect()
}

/// A reasonable default Items tab of common placeable blocks.
pub fn building_blocks() -> Vec<ItemKind> {
    blocks_palette(&[
        BlockId::Stone,
        BlockId::Cobblestone,
        BlockId::Dirt,
        BlockId::Grass,
        BlockId::Sand,
        BlockId::Gravel,
    ])
}

/// The item list a creative tab should display. `Inventory` is the player's own
/// grid (no fixed catalog — the caller renders the live `SlotGrid`), so it
/// returns empty; `Items` and `SpawnEggs` are the creative palettes.
pub fn creative_palette(tab: crate::inventory_screen::Tab) -> Vec<ItemKind> {
    use crate::inventory_screen::Tab;
    match tab {
        Tab::Inventory => Vec::new(),
        Tab::Items => building_blocks(),
        Tab::SpawnEggs => spawn_egg_palette(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emblem::Motif;

    #[test]
    fn display_color_routes_each_item_kind_to_a_source() {
        // Blocks defer to their mesh albedo.
        assert_eq!(
            ItemKind::Block(BlockId::Grass).display_color(),
            BlockId::Grass.base_color()
        );
        // Eggs use the creature color; ingots/tools use the metal color.
        assert_eq!(
            ItemKind::SpawnEgg(CreatureKind::Dragon).display_color(),
            CreatureKind::Dragon.color()
        );
        assert_eq!(ItemKind::Ingot(MetalKind::Gold).display_color(), MetalKind::Gold.color());
        assert_eq!(
            ItemKind::Tool(ToolKind::Pickaxe, MetalKind::Copper).display_color(),
            MetalKind::Copper.color()
        );
        // The two creatures and three metals are visually distinct.
        assert_ne!(CreatureKind::Dragon.color(), CreatureKind::Lauscher.color());
        assert_ne!(MetalKind::Iron.color(), MetalKind::Gold.color());
        assert_ne!(MetalKind::Gold.color(), MetalKind::Copper.color());
    }

    #[test]
    fn creative_palette_matches_each_tab() {
        use crate::inventory_screen::Tab;
        assert!(creative_palette(Tab::Inventory).is_empty(), "player grid, not a catalog");
        assert_eq!(creative_palette(Tab::Items), building_blocks());
        let eggs = creative_palette(Tab::SpawnEggs);
        assert_eq!(eggs, spawn_egg_palette());
        assert!(eggs.iter().all(|i| i.is_spawn_egg()));
    }

    #[test]
    fn the_spawn_egg_tab_carries_the_dragon_and_the_lauscher() {
        let eggs = spawn_egg_palette();
        assert_eq!(eggs.len(), CreatureKind::ALL.len());
        assert!(eggs.iter().all(|e| e.is_spawn_egg()));
        assert!(eggs.contains(&ItemKind::SpawnEgg(CreatureKind::Dragon)));
        assert!(eggs.contains(&ItemKind::SpawnEgg(CreatureKind::Lauscher)));
    }

    #[test]
    fn creature_kinds_have_distinct_names_and_affinities() {
        assert_eq!(CreatureKind::Dragon.affinity(), Element::Wood);
        assert_eq!(CreatureKind::Lauscher.affinity(), Element::Air);
        assert_ne!(CreatureKind::Dragon.name(), CreatureKind::Lauscher.name());
        assert!(!CreatureKind::Dragon.name().is_empty());
    }

    #[test]
    fn an_eggs_emblem_comes_from_its_affinity() {
        // Dragon is Wood → a leaf motif; Lauscher is Air → a gust.
        assert_eq!(CreatureKind::Dragon.emblem().motif, Motif::Leaf);
        assert_eq!(CreatureKind::Lauscher.emblem().motif, Motif::Gust);
    }

    #[test]
    fn blocks_stack_deep_eggs_stay_precious() {
        assert_eq!(ItemKind::Block(BlockId::Stone).max_stack(), 64);
        assert_eq!(ItemKind::SpawnEgg(CreatureKind::Dragon).max_stack(), 16);
    }

    #[test]
    fn display_names_read_right() {
        assert_eq!(ItemKind::SpawnEgg(CreatureKind::Dragon).display_name(), "Wood Dragon Egg");
        assert!(!ItemKind::Block(BlockId::Stone).display_name().is_empty());
    }

    #[test]
    fn the_items_tab_has_placeable_blocks() {
        let items = building_blocks();
        assert!(!items.is_empty());
        assert!(items.iter().all(|i| !i.is_spawn_egg()), "the Items tab is blocks");
        assert!(items.contains(&ItemKind::Block(BlockId::Stone)));
    }

    // ── r330-swords ─────────────────────────────────────────────────────────
    fn spec(seed: u64, class: WeaponClass, el: u8, tier: u8) -> LegendarySpec {
        LegendarySpec { seed, class, element_idx: el, tier }
    }

    #[test]
    fn legendary_spec_reconstructs_the_same_weapon_as_forge() {
        let s = spec(0xDEAD_BEEF, WeaponClass::Greatsword, 4, 3);
        let direct = crate::sim::legendary_weapon::forge(0xDEAD_BEEF, WeaponClass::Greatsword, 4, 3);
        // spec.weapon() is forge — bit-identical, deterministic, single-source.
        assert_eq!(s.weapon(), direct);
        assert_eq!(s.weapon().base_damage.to_bits(), direct.base_damage.to_bits());
    }

    #[test]
    fn a_legendary_item_round_trips_its_spec() {
        let s = spec(7, WeaponClass::Wand, 5, 2);
        let item = ItemKind::Legendary(s);
        match item {
            ItemKind::Legendary(got) => assert_eq!(got, s),
            _ => panic!("not a legendary"),
        }
        // It is not a spawn egg, and (Copy + Eq) it compares equal to itself.
        assert!(!item.is_spawn_egg());
        assert_eq!(item, ItemKind::Legendary(s));
    }

    #[test]
    fn legendaries_do_not_stack() {
        assert_eq!(ItemKind::Legendary(spec(1, WeaponClass::Blade, 0, 0)).max_stack(), 1);
    }

    #[test]
    fn legendary_display_name_is_element_then_class_and_nonempty() {
        assert_eq!(
            ItemKind::Legendary(spec(1, WeaponClass::Greatsword, 4, 2)).display_name(),
            "Fire Greatsword"
        );
        // Every element index yields a non-empty "<Element> <Class>" name.
        for el in 0u8..6 {
            let name = ItemKind::Legendary(spec(1, WeaponClass::Dagger, el, 0)).display_name();
            assert!(!name.is_empty());
            assert!(name.contains("Dagger"));
        }
        // element_idx is taken mod 6 — out-of-range never panics, wraps cleanly.
        assert_eq!(
            ItemKind::Legendary(spec(1, WeaponClass::Wand, 6, 0)).display_name(),
            ItemKind::Legendary(spec(1, WeaponClass::Wand, 0, 0)).display_name()
        );
    }

    #[test]
    fn legendary_display_color_distinguishes_elements_and_stays_in_range() {
        let wood = ItemKind::Legendary(spec(1, WeaponClass::Blade, 0, 0)).display_color();
        let fire = ItemKind::Legendary(spec(1, WeaponClass::Blade, 4, 0)).display_color();
        assert_ne!(wood, fire, "different elements should tint differently");
        // Tier brightens but never overflows (all components remain valid u8).
        let t0 = ItemKind::Legendary(spec(1, WeaponClass::Blade, 4, 0)).display_color();
        let t5 = ItemKind::Legendary(spec(1, WeaponClass::Blade, 4, 5)).display_color();
        assert!(t5 >= t0, "higher tier is no darker on any channel");
    }
}

// ===========================================================================
// r332-swords-creative — ADDITIVE block for `crates/unendlich-engine/src/item.rs`
//
// Paste this immediately AFTER `blocks_palette` / `building_blocks`
// (i.e. alongside `spawn_egg_palette`), and append the test module to the end
// of the file. It is purely additive: no existing item is changed.
//
// Depends on the r332 swords substrate already in `item.rs`:
//   * `ItemKind::Legendary(LegendarySpec)`
//   * `LegendarySpec { seed: u64, class: WeaponClass, element_idx: u8, tier: u8 }`
//   * `LegendarySpec::weapon(&self) -> Weapon`   (forwards to `legendary_weapon::forge`)
//   * `WeaponClass` + `WeaponClass::ALL`         (sim::legendary_weapon)
// `WeaponClass`/`LegendarySpec` are already in scope in `item.rs` (the swords
// thread imported them to declare the `Legendary` variant), so the bare names
// below resolve with no new `use`.
// ===========================================================================

/// The **Legendaries** creative tab: a curated, reproducible sample of forged
/// legendaries so an operator can put every class/element in hand and test them.
///
/// Covers **every `WeaponClass` × every element** (`element_idx 0..6`), each at a
/// representative tier — a deterministic mix of `0` and a high tier so the tab
/// shows both ends of the stat curve (every class and every element appears once
/// at low tier and once at high tier across the grid). 48 entries; the seeds are
/// fixed (splitmix-derived per entry), so the palette is byte-identical every run.
pub fn legendary_palette() -> Vec<ItemKind> {
    /// A clearly-high sample tier. `forge`'s `tier` is uncapped; this only picks a
    /// showcase value to sit next to the tier-0 entries (not a gameplay cap).
    const SAMPLE_HIGH_TIER: u8 = 7;
    /// Number of elements (`forge` takes `element_idx % 6`).
    const ELEMENTS: u8 = 6;
    /// Fixed base so per-entry seeds are reproducible yet well-spread.
    const PALETTE_SEED_BASE: u64 = 0x5EED_1E6E_DA1A_C0DE;

    let mut out = Vec::with_capacity(WeaponClass::ALL.len() * ELEMENTS as usize);
    for (ci, &class) in WeaponClass::ALL.iter().enumerate() {
        for element_idx in 0..ELEMENTS {
            // Alternate low / high across the grid so every class AND every element
            // is represented at both tier 0 and the high tier.
            let tier = if (ci as u8 + element_idx) % 2 == 0 { 0 } else { SAMPLE_HIGH_TIER };
            // splitmix64 of (base, class, element, tier) → a stable, distinct seed.
            let mut z = PALETTE_SEED_BASE
                ^ ((ci as u64) << 16)
                ^ ((element_idx as u64) << 8)
                ^ (tier as u64);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            let seed = z ^ (z >> 31);
            out.push(ItemKind::Legendary(LegendarySpec { seed, class, element_idx, tier }));
        }
    }
    out
}

// --- append to the end of item.rs (additive; does not touch `mod tests`) ------

#[cfg(test)]
mod legendary_palette_tests {
    use super::*;
    use crate::sim::legendary_weapon::WeaponClass;

    fn palette() -> Vec<ItemKind> {
        legendary_palette()
    }

    #[test]
    fn palette_is_non_empty_and_bounded() {
        let p = palette();
        assert!(!p.is_empty(), "legendary palette must not be empty");
        assert!(p.len() <= 48, "palette should stay a sensible size (<= ~48), got {}", p.len());
        // 8 classes * 6 elements.
        assert_eq!(p.len(), WeaponClass::ALL.len() * 6);
    }

    #[test]
    fn every_entry_is_a_legendary_with_valid_element() {
        for it in palette() {
            match it {
                ItemKind::Legendary(spec) => {
                    assert!(spec.element_idx < 6, "element_idx out of 0..6: {}", spec.element_idx);
                }
                other => panic!("non-legendary entry in palette: {other:?}"),
            }
        }
    }

    #[test]
    fn display_name_and_color_are_non_trivial() {
        for it in palette() {
            let name = it.display_name();
            assert!(!name.trim().is_empty(), "empty display_name for {it:?}");
            // tint must carry some colour (not pure black) so the tile is visible.
            assert_ne!(it.display_color(), [0, 0, 0], "black tile for {it:?}");
        }
    }

    #[test]
    fn each_spec_forges_a_matching_weapon() {
        for it in palette() {
            if let ItemKind::Legendary(spec) = it {
                let w = spec.weapon();
                assert_eq!(w.class, spec.class);
                assert_eq!(w.element_idx, spec.element_idx % 6);
                assert_eq!(w.tier, spec.tier);
                assert!(w.base_damage.is_finite() && w.base_damage >= 0.0);
            }
        }
    }

    #[test]
    fn palette_is_deterministic() {
        assert_eq!(legendary_palette(), legendary_palette());
    }

    #[test]
    fn no_duplicate_specs() {
        let p = palette();
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                assert_ne!(p[i], p[j], "duplicate palette entry at {i},{j}");
            }
        }
    }

    #[test]
    fn covers_every_class_every_element_and_both_tier_ends() {
        let p = palette();
        let mut classes = std::collections::BTreeSet::new();
        let mut elements = std::collections::BTreeSet::new();
        let mut saw_low = false;
        let mut saw_high = false;
        for it in &p {
            if let ItemKind::Legendary(spec) = it {
                classes.insert(format!("{:?}", spec.class));
                elements.insert(spec.element_idx);
                if spec.tier == 0 { saw_low = true; } else { saw_high = true; }
            }
        }
        assert_eq!(classes.len(), WeaponClass::ALL.len(), "not every class represented");
        assert_eq!(elements.len(), 6, "not every element 0..6 represented");
        assert!(saw_low && saw_high, "palette should show both a low and a high tier");
    }
}
