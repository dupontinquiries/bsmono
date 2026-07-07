//! `sim::save_slots` — world seed derivation + save-slot catalog (round R38).
//!
//! The main-menu flow needs (a) a deterministic world SEED derived from the
//! world name and (b) a save-SLOT catalog. The world is seed-deterministic
//! (terrain / caves / flora / genome all key off a seed), so a world must be
//! reconstructable from `name -> seed`. This is the pure logic; the actual file
//! IO and menu drawing are coordinator-side.
//!
//! [`derive_seed`] turns a `name_hash` into a master `u64` plus four DISTINCT
//! sub-streams via a fixed splitmix64 finalizer. Because that finalizer is a
//! bijection and each stream mixes the name with a distinct salt, the five
//! values are guaranteed distinct for *every* input. [`apply`] folds a
//! [`SlotOp`] over the fixed-capacity catalog.
//!
//! Mobile: the whole [`Catalog`] is a fixed-size `Copy` stack value
//! (`MAX_SLOTS = 8`) — zero heap, no growth, trivially serializable for
//! save/restore. And because seeds are *derived* rather than stored RNG state, a
//! world is fully reconstructable from `name_hash` + seed with nothing else
//! persisted — ideal for an 8 GB mobile SoC.
//!
//! Pure, deterministic, panic-free — no `crate::render`, no GPU, no I/O, no
//! wall-clock (ticks are passed in), no RNG state, no `.unwrap()`/`.expect()`.
//! Release-safe: every helper is unconditional (no `#[cfg]` gating).

/// Maximum number of save slots.
pub const MAX_SLOTS: usize = 8;

/// A world's master seed plus its derived per-system sub-streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldSeed {
    pub master: u64,
    pub terrain: u64,
    pub caves: u64,
    pub flora: u64,
    pub dragons: u64,
}

/// One save slot. `updated_tick` is a sim tick supplied by the caller (no clock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub used: bool,
    pub name_key: u64,
    pub seed: WorldSeed,
    pub updated_tick: u64,
}

impl Slot {
    /// The empty/unused slot.
    pub const EMPTY: Slot = Slot {
        used: false,
        name_key: 0,
        seed: WorldSeed { master: 0, terrain: 0, caves: 0, flora: 0, dragons: 0 },
        updated_tick: 0,
    };
}

/// Fixed-capacity, `Copy`, heap-free catalog of save slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Catalog {
    pub slots: [Slot; MAX_SLOTS],
}

impl Catalog {
    /// A catalog with all slots empty.
    pub const fn empty() -> Catalog {
        Catalog { slots: [Slot::EMPTY; MAX_SLOTS] }
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Catalog::empty()
    }
}

/// An operation to fold over a [`Catalog`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotOp {
    /// Create a world in the first free slot (no-op when full).
    NewWorld { name_hash: u64, tick: u64 },
    /// Clear the slot at `index` (no-op when out of range).
    Delete { index: usize },
    /// Update only the `updated_tick` of the slot at `index`
    /// (no-op when out of range or unused).
    Touch { index: usize, tick: u64 },
}

// Distinct per-stream salts (R20: fixed, documented). Distinct salts ⇒ distinct
// finalizer inputs ⇒ distinct outputs (the splitmix64 finalizer is a bijection).
const SALT_MASTER: u64 = 0x5EED_0000_0000_0001;
const SALT_TERRAIN: u64 = 0x7E44_A100_0000_0002;
const SALT_CAVES: u64 = 0xCA7E_5000_0000_0003;
const SALT_FLORA: u64 = 0xF10A_0000_0000_0004;
const SALT_DRAGONS: u64 = 0xD4A6_0000_0000_0005;

/// splitmix64 finalizer (a bijection on `u64`).
fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Derive a [`WorldSeed`] from a world `name_hash`. Deterministic; the master
/// and the four sub-streams are all distinct for every input.
pub fn derive_seed(name_hash: u64) -> WorldSeed {
    WorldSeed {
        master: mix64(name_hash ^ SALT_MASTER),
        terrain: mix64(name_hash ^ SALT_TERRAIN),
        caves: mix64(name_hash ^ SALT_CAVES),
        flora: mix64(name_hash ^ SALT_FLORA),
        dragons: mix64(name_hash ^ SALT_DRAGONS),
    }
}

/// First free (unused) slot index, if any.
pub fn first_free(cat: &Catalog) -> Option<usize> {
    let mut i = 0;
    while i < MAX_SLOTS {
        if !cat.slots[i].used {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Fold a [`SlotOp`] over the catalog, returning a new catalog. Out-of-range /
/// full / unused conditions are no-ops (the catalog is returned unchanged).
/// Does not mutate its input (`Catalog` is `Copy`; the argument is taken by value).
pub fn apply(cat: Catalog, op: SlotOp) -> Catalog {
    let mut out = cat;
    match op {
        SlotOp::NewWorld { name_hash, tick } => {
            if let Some(i) = first_free(&out) {
                out.slots[i] = Slot {
                    used: true,
                    name_key: name_hash,
                    seed: derive_seed(name_hash),
                    updated_tick: tick,
                };
            }
        }
        SlotOp::Delete { index } => {
            if index < MAX_SLOTS {
                out.slots[index] = Slot::EMPTY;
            }
        }
        SlotOp::Touch { index, tick } => {
            if index < MAX_SLOTS && out.slots[index].used {
                out.slots[index].updated_tick = tick;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_seed_deterministic() {
        for nh in [0u64, 1, 42, 0xDEAD_BEEF, u64::MAX] {
            assert_eq!(derive_seed(nh), derive_seed(nh));
        }
    }

    #[test]
    fn substreams_distinct_from_master_and_each_other() {
        for nh in [0u64, 1, 7, 0xABCD, 0xFFFF_0000_1234_5678, u64::MAX] {
            let s = derive_seed(nh);
            let all = [s.master, s.terrain, s.caves, s.flora, s.dragons];
            for i in 0..all.len() {
                for j in (i + 1)..all.len() {
                    assert_ne!(all[i], all[j], "streams {i},{j} collide for nh={nh}");
                }
            }
        }
    }

    #[test]
    fn newworld_fills_first_free_slot() {
        let c0 = Catalog::empty();
        let c1 = apply(c0, SlotOp::NewWorld { name_hash: 99, tick: 5 });
        assert!(c1.slots[0].used);
        assert_eq!(c1.slots[0].name_key, 99);
        assert_eq!(c1.slots[0].seed, derive_seed(99));
        assert_eq!(c1.slots[0].updated_tick, 5);
        // remaining slots untouched
        for i in 1..MAX_SLOTS {
            assert_eq!(c1.slots[i], Slot::EMPTY);
        }
        // a second NewWorld fills slot 1
        let c2 = apply(c1, SlotOp::NewWorld { name_hash: 100, tick: 6 });
        assert!(c2.slots[1].used && c2.slots[1].name_key == 100);
    }

    #[test]
    fn newworld_is_noop_when_full() {
        let mut c = Catalog::empty();
        for k in 0..MAX_SLOTS as u64 {
            c = apply(c, SlotOp::NewWorld { name_hash: k, tick: k });
        }
        assert!(first_free(&c).is_none());
        let after = apply(c, SlotOp::NewWorld { name_hash: 1234, tick: 9 });
        assert_eq!(after, c, "full catalog: NewWorld is a no-op");
    }

    #[test]
    fn delete_frees_slot_and_oob_is_noop() {
        let c = apply(Catalog::empty(), SlotOp::NewWorld { name_hash: 5, tick: 1 });
        let d = apply(c, SlotOp::Delete { index: 0 });
        assert_eq!(d.slots[0], Slot::EMPTY);
        assert_eq!(first_free(&d), Some(0));
        // out of range ⇒ unchanged
        assert_eq!(apply(c, SlotOp::Delete { index: MAX_SLOTS }), c);
        assert_eq!(apply(c, SlotOp::Delete { index: 999 }), c);
    }

    #[test]
    fn touch_updates_only_tick() {
        let c = apply(Catalog::empty(), SlotOp::NewWorld { name_hash: 77, tick: 1 });
        let t = apply(c, SlotOp::Touch { index: 0, tick: 42 });
        assert_eq!(t.slots[0].updated_tick, 42);
        // everything else about the slot is unchanged
        assert_eq!(t.slots[0].used, c.slots[0].used);
        assert_eq!(t.slots[0].name_key, c.slots[0].name_key);
        assert_eq!(t.slots[0].seed, c.slots[0].seed);
        // touch on an unused slot ⇒ no-op
        assert_eq!(apply(c, SlotOp::Touch { index: 3, tick: 99 }), c);
        // touch out of range ⇒ no-op
        assert_eq!(apply(c, SlotOp::Touch { index: 50, tick: 99 }), c);
    }

    #[test]
    fn apply_does_not_mutate_input() {
        let c = apply(Catalog::empty(), SlotOp::NewWorld { name_hash: 8, tick: 2 });
        let snapshot = c;
        let _ = apply(c, SlotOp::NewWorld { name_hash: 9, tick: 3 });
        let _ = apply(c, SlotOp::Delete { index: 0 });
        let _ = apply(c, SlotOp::Touch { index: 0, tick: 1000 });
        assert_eq!(c, snapshot, "apply must not mutate its argument");
    }

    #[test]
    fn first_free_empty_partial_full() {
        let mut c = Catalog::empty();
        assert_eq!(first_free(&c), Some(0));
        c = apply(c, SlotOp::NewWorld { name_hash: 1, tick: 1 });
        c = apply(c, SlotOp::NewWorld { name_hash: 2, tick: 1 });
        assert_eq!(first_free(&c), Some(2));
        // delete the middle ⇒ that becomes first free
        c = apply(c, SlotOp::Delete { index: 0 });
        assert_eq!(first_free(&c), Some(0));
        // fill everything
        let mut full = Catalog::empty();
        for k in 0..MAX_SLOTS as u64 {
            full = apply(full, SlotOp::NewWorld { name_hash: k, tick: k });
        }
        assert_eq!(first_free(&full), None);
    }

    #[test]
    fn apply_deterministic() {
        let c = apply(Catalog::empty(), SlotOp::NewWorld { name_hash: 3, tick: 3 });
        let op = SlotOp::NewWorld { name_hash: 4, tick: 4 };
        assert_eq!(apply(c, op), apply(c, op));
    }
}
