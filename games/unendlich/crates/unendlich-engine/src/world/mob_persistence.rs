//! Mob persistence — saving and restoring the world's creatures across sessions.
//!
//! The trick is that ambient spawns are *deterministic*: [`super::ecosystem::
//! roll_spawn`] always yields the same creature at the same `(cell, seed)`. So a
//! pristine region's population costs **nothing** to persist — it regenerates
//! identically on reload, exactly like terrain. We only need to save the ways the
//! world has *diverged* from that pristine roll:
//!
//! * **Removed** — a creature the player killed (or that despawned for good). It
//!   must NOT come back when the region reloads.
//! * **Moved / changed** — a creature that wandered, took damage, or is carrying
//!   AI state. Its current snapshot is saved and restored in place of the pristine
//!   roll. Fully dynamic mobs with no deterministic origin (spawn-egg, bred) are
//!   stored here too, under a synthetic key.
//!
//! [`SpawnLedger`] is that delta. It's tiny (only edited mobs), pure, and the
//! single source of truth the save path serializes to the world root alongside
//! edited chunks (mirroring `storage::save_chunk_sync`). On region load, the
//! engine enumerates the deterministic spawns and runs each through
//! [`SpawnLedger::resolve`] to decide: spawn pristine, restore a snapshot, or
//! skip.

use crate::item::CreatureKind;
use std::collections::{HashMap, HashSet};

/// Identifies an ambient spawn by its deterministic surface cell `(x, z)` — the
/// same cell `roll_spawn` is keyed on, so it's stable across reloads. Dynamic
/// (player-created) mobs use a synthetic key outside the world's cell range.
pub type SpawnKey = (i32, i32);

/// A saved creature snapshot — enough to rebuild it where it was, in the state it
/// was in. Deliberately small; richer AI/memory state can extend this later.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedMob {
    pub kind: CreatureKind,
    pub pos: [f32; 3],
    /// Remaining health as a fraction of full, `0.0..=1.0`. (Per-part HP can
    /// replace this when the breakable-anatomy save lands.)
    pub hp_fraction: f32,
}

impl SavedMob {
    pub fn new(kind: CreatureKind, pos: [f32; 3], hp_fraction: f32) -> Self {
        SavedMob { kind, pos, hp_fraction: hp_fraction.clamp(0.0, 1.0) }
    }
}

/// What to do with one deterministic spawn after consulting the ledger.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// Spawn the deterministic creature unchanged (the common case).
    Pristine(CreatureKind),
    /// Restore this saved snapshot instead of the pristine roll.
    Restore(SavedMob),
    /// Killed/despawned — do not spawn it at all.
    Skip,
}

/// The persisted delta from the deterministic spawn baseline. Empty = the region
/// is pristine and regenerates entirely from the seed.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct SpawnLedger {
    removed: HashSet<SpawnKey>,
    moved: HashMap<SpawnKey, SavedMob>,
}

impl SpawnLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a spawn permanently gone (killed/despawned). Clears any saved snapshot
    /// for it — a dead mob doesn't also need a position.
    pub fn mark_removed(&mut self, key: SpawnKey) {
        self.moved.remove(&key);
        self.removed.insert(key);
    }

    /// Save (or update) a creature's snapshot. A removed key stays removed — you
    /// can't resurrect by saving state over a kill.
    pub fn save(&mut self, key: SpawnKey, mob: SavedMob) {
        if !self.removed.contains(&key) {
            self.moved.insert(key, mob);
        }
    }

    /// Decide what to do with the deterministic spawn at `key` whose pristine roll
    /// is `pristine`.
    pub fn resolve(&self, key: SpawnKey, pristine: CreatureKind) -> Resolution {
        if self.removed.contains(&key) {
            Resolution::Skip
        } else if let Some(m) = self.moved.get(&key) {
            Resolution::Restore(m.clone())
        } else {
            Resolution::Pristine(pristine)
        }
    }

    pub fn is_removed(&self, key: SpawnKey) -> bool {
        self.removed.contains(&key)
    }

    /// How many deltas are stored (the rough on-disk cost). A pristine world is 0.
    pub fn len(&self) -> usize {
        self.removed.len() + self.moved.len()
    }

    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.moved.is_empty()
    }

    pub fn removed_count(&self) -> usize {
        self.removed.len()
    }

    pub fn moved_count(&self) -> usize {
        self.moved.len()
    }

    /// Serialize to a deterministic, self-describing byte blob for saving to the
    /// world root (alongside edited chunks). Entries are sorted so the bytes are
    /// stable run-to-run. Format: magic `MOBL`, version, then the removed cells
    /// and the moved snapshots, all little-endian.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"MOBL");
        b.push(1u8); // version
        let mut removed: Vec<&SpawnKey> = self.removed.iter().collect();
        removed.sort_unstable();
        b.extend_from_slice(&(removed.len() as u32).to_le_bytes());
        for &&(x, z) in &removed {
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&z.to_le_bytes());
        }
        let mut moved: Vec<(&SpawnKey, &SavedMob)> = self.moved.iter().collect();
        moved.sort_unstable_by_key(|(k, _)| **k);
        b.extend_from_slice(&(moved.len() as u32).to_le_bytes());
        for (&(x, z), m) in &moved {
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&z.to_le_bytes());
            b.push(kind_tag(m.kind));
            for v in m.pos {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&m.hp_fraction.to_le_bytes());
        }
        b
    }

    /// Parse a blob written by [`to_bytes`](Self::to_bytes). Returns an error
    /// string on a bad magic/version or a truncated/blob.
    pub fn from_bytes(bytes: &[u8]) -> Result<SpawnLedger, &'static str> {
        let mut r = ByteReader { b: bytes, i: 0 };
        if r.take(4)? != b"MOBL" {
            return Err("bad magic");
        }
        if r.u8()? != 1 {
            return Err("unsupported version");
        }
        let mut ledger = SpawnLedger::new();
        let n_removed = r.u32()?;
        for _ in 0..n_removed {
            let x = r.i32()?;
            let z = r.i32()?;
            ledger.removed.insert((x, z));
        }
        let n_moved = r.u32()?;
        for _ in 0..n_moved {
            let x = r.i32()?;
            let z = r.i32()?;
            let kind = kind_from_tag(r.u8()?).ok_or("unknown creature tag")?;
            let pos = [r.f32()?, r.f32()?, r.f32()?];
            let hp = r.f32()?;
            ledger.moved.insert((x, z), SavedMob::new(kind, pos, hp));
        }
        Ok(ledger)
    }
}

/// Stable on-disk tag for a creature kind (independent of `CreatureKind::ALL`
/// order, so reordering that array never corrupts saves).
fn kind_tag(k: CreatureKind) -> u8 {
    match k {
        CreatureKind::Dragon => 0,
        CreatureKind::Lauscher => 1,
        CreatureKind::Spaeher => 2,
        CreatureKind::Fauna => 3,
        CreatureKind::Dinosaur => 4,
    }
}

fn kind_from_tag(t: u8) -> Option<CreatureKind> {
    Some(match t {
        0 => CreatureKind::Dragon,
        1 => CreatureKind::Lauscher,
        2 => CreatureKind::Spaeher,
        3 => CreatureKind::Fauna,
        4 => CreatureKind::Dinosaur,
        _ => return None,
    })
}

/// Minimal little-endian cursor for [`SpawnLedger::from_bytes`].
struct ByteReader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> ByteReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        if self.i + n > self.b.len() {
            return Err("truncated");
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, &'static str> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, &'static str> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

/// One creature to instantiate after the ledger has spoken: its kind, where it/// goes, and its health. Skipped spawns don't appear in the output.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSpawn {
    pub key: SpawnKey,
    pub kind: CreatureKind,
    pub pos: [f32; 3],
    pub hp_fraction: f32,
}

/// Run a region's deterministic spawns (cell + kind + pristine position) through
/// the ledger, yielding exactly the creatures the engine should instantiate:
/// pristine ones as-is, moved/damaged ones from their snapshot, killed ones
/// dropped. Pure — the load path calls this and registers each result.
pub fn resolve_region(
    ledger: &SpawnLedger,
    pristine: impl IntoIterator<Item = (SpawnKey, CreatureKind, [f32; 3])>,
) -> Vec<ResolvedSpawn> {
    let mut out = Vec::new();
    for (key, kind, pos) in pristine {
        match ledger.resolve(key, kind) {
            Resolution::Pristine(k) => out.push(ResolvedSpawn { key, kind: k, pos, hp_fraction: 1.0 }),
            Resolution::Restore(m) => out.push(ResolvedSpawn {
                key,
                kind: m.kind,
                pos: m.pos,
                hp_fraction: m.hp_fraction,
            }),
            Resolution::Skip => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pristine_ledger_leaves_every_spawn_alone() {
        let l = SpawnLedger::new();
        assert!(l.is_empty());
        assert_eq!(l.resolve((3, 4), CreatureKind::Fauna), Resolution::Pristine(CreatureKind::Fauna));
    }

    #[test]
    fn a_killed_mob_does_not_come_back() {
        let mut l = SpawnLedger::new();
        l.mark_removed((10, -2));
        assert_eq!(l.resolve((10, -2), CreatureKind::Dinosaur), Resolution::Skip);
        // And saving state can't resurrect it.
        l.save((10, -2), SavedMob::new(CreatureKind::Dinosaur, [0.0; 3], 1.0));
        assert_eq!(l.resolve((10, -2), CreatureKind::Dinosaur), Resolution::Skip);
    }

    #[test]
    fn a_moved_mob_restores_from_its_snapshot() {
        let mut l = SpawnLedger::new();
        let snap = SavedMob::new(CreatureKind::Fauna, [5.0, 64.0, -3.0], 0.4);
        l.save((1, 1), snap.clone());
        assert_eq!(l.resolve((1, 1), CreatureKind::Fauna), Resolution::Restore(snap));
        // hp is clamped on the way in.
        let over = SavedMob::new(CreatureKind::Fauna, [0.0; 3], 9.0);
        assert_eq!(over.hp_fraction, 1.0);
    }

    #[test]
    fn killing_after_moving_wins() {
        let mut l = SpawnLedger::new();
        l.save((2, 2), SavedMob::new(CreatureKind::Spaeher, [1.0, 2.0, 3.0], 0.5));
        l.mark_removed((2, 2));
        assert_eq!(l.resolve((2, 2), CreatureKind::Spaeher), Resolution::Skip);
        assert_eq!(l.moved_count(), 0, "the snapshot is dropped on kill");
        assert_eq!(l.removed_count(), 1);
    }

    #[test]
    fn resolve_region_filters_killed_and_restores_moved() {
        let mut l = SpawnLedger::new();
        l.mark_removed((0, 0)); // killed
        l.save((2, 0), SavedMob::new(CreatureKind::Fauna, [9.0, 70.0, 1.0], 0.3)); // wandered + hurt
        let pristine = vec![
            ((0, 0), CreatureKind::Fauna, [0.5, 64.0, 0.5]),   // killed -> dropped
            ((1, 0), CreatureKind::Spaeher, [1.5, 65.0, 0.5]), // pristine
            ((2, 0), CreatureKind::Fauna, [2.5, 64.0, 0.5]),   // restored from snapshot
        ];
        let got = resolve_region(&l, pristine);
        assert_eq!(got.len(), 2, "the killed one is gone");
        // The pristine Späher is untouched at full health.
        let spaeher = got.iter().find(|r| r.key == (1, 0)).unwrap();
        assert_eq!(spaeher.kind, CreatureKind::Spaeher);
        assert_eq!(spaeher.hp_fraction, 1.0);
        // The moved fauna comes back at its saved position + health, not the roll's.
        let moved = got.iter().find(|r| r.key == (2, 0)).unwrap();
        assert_eq!(moved.pos, [9.0, 70.0, 1.0]);
        assert_eq!(moved.hp_fraction, 0.3);
    }

    #[test]
    fn bytes_round_trip_and_are_deterministic() {
        let mut l = SpawnLedger::new();
        l.mark_removed((10, -2));
        l.mark_removed((-7, 30));
        l.save((3, 3), SavedMob::new(CreatureKind::Dinosaur, [3.5, 64.0, 3.5], 0.6));
        l.save((100, -100), SavedMob::new(CreatureKind::Fauna, [100.5, 71.0, -99.5], 1.0));
        let bytes = l.to_bytes();
        let back = SpawnLedger::from_bytes(&bytes).expect("round-trips");
        assert_eq!(back, l);
        // Deterministic: re-serializing yields identical bytes (sorted entries).
        assert_eq!(back.to_bytes(), bytes);
        // An empty ledger round-trips too.
        let empty = SpawnLedger::new();
        assert_eq!(SpawnLedger::from_bytes(&empty.to_bytes()).unwrap(), empty);
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(SpawnLedger::from_bytes(b"nope").is_err());
        assert!(SpawnLedger::from_bytes(b"MOBL\x02").is_err()); // wrong version
        assert!(SpawnLedger::from_bytes(b"MOB").is_err()); // truncated magic
    }
}
