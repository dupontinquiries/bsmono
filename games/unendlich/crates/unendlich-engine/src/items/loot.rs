//! Loot tables — what breaking a block yields.
//!
//! A [`LootTable`] maps a block id (the `BlockId` discriminant, as `u16`) to a
//! [`LootDrop`]: a list of [`LootEntry`] (item + count range) plus an optional
//! tool requirement. Rolls are driven by [`LootRng`], a tiny deterministic
//! PRNG, so a given `(block, seed)` always yields the same drop — which means
//! loot can move into the rolled-back sim later by feeding it the sim's seed.
//!
//! A block with no table entry **drops itself once** (`ItemId == BlockId`
//! convention from the palette), so unknown/new blocks still behave sensibly.
//! A tool-gated drop yields nothing when broken without the right tool — the
//! `has_tool` flag is a placeholder until a real tool/tier system exists.

use super::{ItemId, ItemStack};
use std::collections::HashMap;

/// One possible drop: `item`, with a count chosen uniformly in `[min, max]`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LootEntry {
    pub item: ItemId,
    pub min: u16,
    pub max: u16,
}

impl LootEntry {
    /// A fixed-count entry (`min == max`).
    pub fn fixed(item: ItemId, count: u16) -> LootEntry {
        LootEntry { item, min: count, max: count }
    }
}

/// The drop for a single block kind.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LootDrop {
    pub entries: Vec<LootEntry>,
    /// If true, breaking without a tool yields nothing.
    pub needs_tool: bool,
}

/// Maps block id (`BlockId` discriminant as `u16`) to its drop definition.
#[derive(Clone, Default)]
pub struct LootTable {
    table: HashMap<u16, LootDrop>,
}

/// Small deterministic PRNG (xorshift64*). Reproducible from a seed so loot is
/// replayable and can be driven by the sim's per-tick seed when loot folds into
/// the rolled-back sim.
#[derive(Clone, Copy, Debug)]
pub struct LootRng(u64);

impl LootRng {
    /// Seed the generator. Zero is remapped (xorshift's fixed point) so a `0`
    /// seed still produces a useful stream.
    pub fn new(seed: u64) -> LootRng {
        LootRng(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed })
    }

    fn next_u32(&mut self) -> u32 {
        let mut s = self.0;
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        self.0 = s;
        (s.wrapping_mul(0x2545F491_4F6CDD1D) >> 32) as u32
    }

    /// Uniform integer in `[lo, hi]` (inclusive). `hi <= lo` returns `lo`.
    fn range(&mut self, lo: u16, hi: u16) -> u16 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo + 1) as u32;
        lo + (self.next_u32() % span) as u16
    }
}

impl LootTable {
    pub fn new() -> LootTable {
        LootTable { table: HashMap::new() }
    }

    pub fn set(&mut self, block_id: u16, drop: LootDrop) {
        self.table.insert(block_id, drop);
    }

    pub fn get(&self, block_id: u16) -> Option<&LootDrop> {
        self.table.get(&block_id)
    }

    /// Roll the drops for breaking `block_id`. No table entry ⇒ the block drops
    /// itself once. Tool-gated drops yield nothing without `has_tool`. Counts
    /// are merged per item, and zero-count rolls are skipped.
    pub fn roll(&self, block_id: u16, has_tool: bool, rng: &mut LootRng) -> Vec<ItemStack> {
        let Some(drop) = self.table.get(&block_id) else {
            return vec![ItemStack::new(ItemId(block_id as u32), 1)];
        };
        if drop.needs_tool && !has_tool {
            return Vec::new();
        }
        let mut out: Vec<ItemStack> = Vec::new();
        for e in &drop.entries {
            let n = rng.range(e.min, e.max);
            if n == 0 {
                continue;
            }
            match out.iter_mut().find(|s| s.item == e.item) {
                Some(s) => s.count = s.count.saturating_add(n),
                None => out.push(ItemStack::new(e.item, n)),
            }
        }
        out
    }
}

/// Default loot over the engine's known palette ids (see `items::default_palette`).
/// Most blocks drop themselves; grass drops dirt; fluids and bedrock drop
/// nothing. Block ids absent here fall back to the self-drop rule in `roll`.
pub fn default_loot_table() -> LootTable {
    let mut t = LootTable::new();
    let one = |item: u32| LootDrop {
        entries: vec![LootEntry::fixed(ItemId(item), 1)],
        needs_tool: false,
    };
    let none = || LootDrop { entries: Vec::new(), needs_tool: false };
    t.set(1, one(1)); // Stone   -> Stone
    t.set(2, one(3)); // Grass   -> Dirt
    t.set(3, one(3)); // Dirt    -> Dirt
    t.set(7, none()); // Bedrock -> nothing
    t.set(9, none()); // Water   -> nothing
    t.set(11, none()); // Lava   -> nothing
    t.set(12, one(12)); // Sand  -> Sand
    t.set(17, one(17)); // Oak Log -> Oak Log
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_block_drops_itself() {
        let t = LootTable::new();
        let mut rng = LootRng::new(1);
        assert_eq!(t.roll(250, false, &mut rng), vec![ItemStack::new(ItemId(250), 1)]);
    }

    #[test]
    fn fluids_and_bedrock_drop_nothing() {
        let t = default_loot_table();
        let mut rng = LootRng::new(1);
        assert!(t.roll(9, false, &mut rng).is_empty(), "water");
        assert!(t.roll(11, false, &mut rng).is_empty(), "lava");
        assert!(t.roll(7, false, &mut rng).is_empty(), "bedrock");
    }

    #[test]
    fn grass_drops_dirt() {
        let t = default_loot_table();
        let mut rng = LootRng::new(1);
        assert_eq!(t.roll(2, false, &mut rng), vec![ItemStack::new(ItemId(3), 1)]);
    }

    #[test]
    fn stone_and_sand_drop_themselves() {
        let t = default_loot_table();
        let mut rng = LootRng::new(7);
        assert_eq!(t.roll(1, false, &mut rng), vec![ItemStack::new(ItemId(1), 1)]);
        assert_eq!(t.roll(12, false, &mut rng), vec![ItemStack::new(ItemId(12), 1)]);
    }

    #[test]
    fn tool_gated_drop() {
        let mut t = LootTable::new();
        t.set(99, LootDrop { entries: vec![LootEntry { item: ItemId(1), min: 1, max: 3 }], needs_tool: true });
        let mut rng = LootRng::new(42);
        assert!(t.roll(99, false, &mut rng).is_empty(), "no tool -> nothing");
        let got = t.roll(99, true, &mut rng);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].item, ItemId(1));
        assert!((1..=3).contains(&got[0].count));
    }

    #[test]
    fn rolls_are_deterministic() {
        let mut t = LootTable::new();
        t.set(99, LootDrop { entries: vec![LootEntry { item: ItemId(1), min: 1, max: 8 }], needs_tool: false });
        let seq = |seed| {
            let mut rng = LootRng::new(seed);
            (0..6).map(|_| t.roll(99, false, &mut rng)).collect::<Vec<_>>()
        };
        assert_eq!(seq(12345), seq(12345), "same seed -> same sequence");
    }

    #[test]
    fn count_ranges_respected() {
        let mut t = LootTable::new();
        t.set(99, LootDrop { entries: vec![LootEntry { item: ItemId(1), min: 2, max: 5 }], needs_tool: false });
        for seed in 1..300u64 {
            let mut rng = LootRng::new(seed);
            let d = t.roll(99, false, &mut rng);
            assert_eq!(d.len(), 1);
            assert!((2..=5).contains(&d[0].count), "count {} out of range", d[0].count);
        }
    }
}
