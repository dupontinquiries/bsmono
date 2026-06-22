//! Loot → world item-drop resolution (r76-01). The pure step between "an event
//! produced loot" and "items exist in the world": it rolls a [`LootTable`] for a
//! source event and lays the rolled stacks out as concrete [`DropSpawn`]s —
//! position offset + bounded scatter + a small outward/upward pop.
//!
//! It **composes** the existing item cores rather than duplicating them:
//! - rolls via `items::loot::LootTable::roll` using the engine's seeded
//!   `items::loot::LootRng` (so drop *chances/quantities* are exactly the loot
//!   table's, not re-implemented here);
//! - each [`DropSpawn`] is shaped to feed `items::drops::DropField::spawn(stack,
//!   pos, vel)` directly — the bin spawns the bodies (and `sim::item_entity`'s
//!   `ItemBody` carries them); inventory pickup is a separate thread and is
//!   intentionally NOT touched here.
//!
//! (Note: the brief also cited `items::{drop_chance,batch_loot_roll}` and
//! `items::combat_resolve` — those files are not present on this HEAD, so the
//! roll is composed from `loot.rs`, which already owns chance/quantity + the
//! seeded RNG. See `items/loot.rs:94` `LootTable::roll`.)
//!
//! Pure & deterministic: `(source, table, seed)` fully determines the output —
//! item rolls from `LootRng(seed)`, scatter from a local seeded mixer keyed by
//! `(seed, drop_index)`. No RNG beyond those seeds, no I/O, clock, GPU; never
//! panics. Horizontal scatter is bounded by [`SCATTER_RADIUS`].

use crate::items::loot::{LootRng, LootTable};
use crate::items::ItemStack;

/// Horizontal scatter radius around the source, in metres (a rolled disk).
pub const SCATTER_RADIUS: f32 = 0.4;
/// Vertical offset above the source origin where drops appear.
pub const DROP_Y_OFFSET: f32 = 0.25;
/// Upward pop speed imparted to a fresh drop.
pub const POP_VELOCITY: f32 = 2.0;
/// Outward lateral pop, as a multiple of the scatter offset (0 = straight up).
pub const POP_LATERAL: f32 = 1.5;

/// The event that produced loot. `table_key` is the `LootTable` key (a block id
/// / loot-source id as `u16`, matching `LootTable`'s u16 keyspace); `origin` is
/// the world position the loot bursts from; `has_tool` gates tool-locked drops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropSource {
    pub table_key: u16,
    pub origin: [f32; 3],
    pub has_tool: bool,
}

/// One resolved drop ready to be spawned: which stack, where, and with what
/// initial velocity. Maps 1:1 onto `items::drops::DropField::spawn(stack, pos,
/// vel)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropSpawn {
    pub stack: ItemStack,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
}

/// splitmix64 — a tiny deterministic mixer used ONLY for spatial scatter (not
/// loot chance, which stays in `LootTable::roll`). Defined unconditionally.
#[inline]
fn splitmix64(z: u64) -> u64 {
    // r341 dedup: bit-identical to the canonical finalizer — delegate (call sites unchanged).
    crate::util::hash_util::mix64(z)
}

/// Top 24 bits of a hash → `[0,1)`.
#[inline]
fn unit01(h: u64) -> f32 {
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Resolve the concrete world drops for a source event.
///
/// Rolls `table` for `source.table_key` (honouring drop chances, quantities and
/// the tool gate) with `LootRng(seed)`, then scatters each rolled stack within a
/// [`SCATTER_RADIUS`] disk around `source.origin` with a small outward/upward
/// pop. Deterministic in `seed`. An empty / fully tool-gated roll yields no
/// spawns.
pub fn resolve_drops(source: DropSource, table: &LootTable, seed: u64) -> Vec<DropSpawn> {
    let mut rng = LootRng::new(seed);
    let stacks = table.roll(source.table_key, source.has_tool, &mut rng);

    let mut out = Vec::with_capacity(stacks.len());
    for (i, stack) in stacks.into_iter().enumerate() {
        let idx = i as u64;
        // Two independent deterministic draws per drop, keyed by (seed, index).
        let h_ang = splitmix64(seed ^ ((idx << 1) | 1));
        let h_rad = splitmix64(seed.rotate_left(32) ^ ((idx << 1) ^ 0xA5A5_5A5A));
        let angle = unit01(h_ang) * core::f32::consts::TAU;
        // sqrt(u) → uniform area over the disk; r ≤ SCATTER_RADIUS by construction.
        let r = unit01(h_rad).sqrt() * SCATTER_RADIUS;
        let ox = r * angle.cos();
        let oz = r * angle.sin();

        let pos = [
            source.origin[0] + ox,
            source.origin[1] + DROP_Y_OFFSET,
            source.origin[2] + oz,
        ];
        // Pop outward from the burst centre, plus a fixed upward kick.
        let vel = [ox * POP_LATERAL, POP_VELOCITY, oz * POP_LATERAL];

        out.push(DropSpawn { stack, pos, vel });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::loot::{LootDrop, LootEntry, LootTable};
    use crate::items::ItemId;

    fn src(key: u16, has_tool: bool) -> DropSource {
        DropSource { table_key: key, origin: [10.0, 64.0, -3.0], has_tool }
    }

    fn horiz_dist(a: [f32; 3], origin: [f32; 3]) -> f32 {
        let dx = a[0] - origin[0];
        let dz = a[2] - origin[2];
        (dx * dx + dz * dz).sqrt()
    }

    #[test]
    fn deterministic_same_seed_same_drops() {
        let mut t = LootTable::new();
        t.set(
            5,
            LootDrop {
                entries: vec![
                    LootEntry { item: ItemId(5), min: 1, max: 4 },
                    LootEntry::fixed(ItemId(42), 1),
                ],
                needs_tool: false,
            },
        );
        let a = resolve_drops(src(5, false), &t, 0xC0FFEE);
        for _ in 0..50 {
            assert_eq!(resolve_drops(src(5, false), &t, 0xC0FFEE), a, "non-deterministic");
        }
        // a different seed generally differs (positions and/or counts)
        let b = resolve_drops(src(5, false), &t, 0xC0FFEF);
        assert!(a != b || a.is_empty(), "distinct seeds should usually differ");
    }

    #[test]
    fn empty_table_entry_and_tool_gate_yield_no_drops() {
        let mut t = LootTable::new();
        t.set(8, LootDrop { entries: Vec::new(), needs_tool: false }); // explicitly nothing
        assert!(resolve_drops(src(8, false), &t, 1).is_empty());

        t.set(
            9,
            LootDrop { entries: vec![LootEntry::fixed(ItemId(9), 1)], needs_tool: true },
        );
        assert!(resolve_drops(src(9, false), &t, 1).is_empty(), "tool-gated, no tool → nothing");
        assert!(!resolve_drops(src(9, true), &t, 1).is_empty(), "with tool → drops");
    }

    #[test]
    fn unknown_key_self_drops_once() {
        // No table entry → loot.rs self-drop rule: one stack of ItemId(key).
        let t = LootTable::new();
        let out = resolve_drops(src(123, false), &t, 7);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stack, ItemStack::new(ItemId(123), 1));
    }

    #[test]
    fn drop_chance_distribution_over_many_rolls() {
        // An entry with count range [0,1] appears ~50% of the time (count 0 is
        // skipped by roll). resolve_drops must pass that chance straight through.
        let mut t = LootTable::new();
        t.set(
            3,
            LootDrop { entries: vec![LootEntry { item: ItemId(99), min: 0, max: 1 }], needs_tool: false },
        );
        let n = 4000;
        let mut present = 0;
        for seed in 0..n {
            if !resolve_drops(src(3, false), &t, seed as u64).is_empty() {
                present += 1;
            }
        }
        let frac = present as f32 / n as f32;
        assert!((frac - 0.5).abs() < 0.05, "expected ~50% drop rate, got {frac}");
    }

    #[test]
    fn scatter_stays_within_radius_and_at_offset_height() {
        // Three distinct items → three scattered spawns.
        let mut t = LootTable::new();
        t.set(
            4,
            LootDrop {
                entries: vec![
                    LootEntry::fixed(ItemId(1), 1),
                    LootEntry::fixed(ItemId(2), 1),
                    LootEntry::fixed(ItemId(3), 1),
                ],
                needs_tool: false,
            },
        );
        let origin = [10.0, 64.0, -3.0];
        for seed in 0..2000u64 {
            let out = resolve_drops(src(4, false), &t, seed);
            assert_eq!(out.len(), 3);
            for d in &out {
                assert!(
                    horiz_dist(d.pos, origin) <= SCATTER_RADIUS + 1e-4,
                    "scatter {} exceeded radius at seed {seed}",
                    horiz_dist(d.pos, origin)
                );
                assert!((d.pos[1] - (origin[1] + DROP_Y_OFFSET)).abs() < 1e-4);
                assert!(d.vel[1] > 0.0, "drops pop upward");
                assert!(d.pos.iter().all(|v| v.is_finite()) && d.vel.iter().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn empty_origin_table_returns_empty_vec() {
        // brand-new empty table + a key that explicitly maps to nothing.
        let mut t = LootTable::new();
        t.set(0, LootDrop { entries: Vec::new(), needs_tool: false });
        assert!(resolve_drops(src(0, true), &t, 12345).is_empty());
    }
}
