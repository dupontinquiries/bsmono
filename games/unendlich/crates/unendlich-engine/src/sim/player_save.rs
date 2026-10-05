//! `sim::player_save` — the player's own state in a world, persisted beside the world's chunks.
//!
//! Save & Quit, the world switch and the autosave all called `ChunkManager::save_all`, which
//! writes edited CHUNKS and nothing else: where you stood, what you carried and what time it
//! was died with the process, and the next load put you at the seed's spawn with whatever
//! inventory the previous world left in the `App`. This is the per-world `player.json` that
//! fixes that — position and view, the inventory slot for slot (a forged weapon's
//! `LegendarySpec` included), the hotbar selection and the world clock.
//!
//! One file per world, in the world directory (`server_config::world_dir_for`), next to
//! `whitelist.json` and the flora store; written atomically (temp + rename) the way the
//! whitelist is. Pure apart from `save`/`load`; the fullgame fills and applies it.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::items::ItemStack;

/// Bumped when a field changes meaning; an older file still loads (serde defaults the rest).
pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSave {
    #[serde(default = "format_now")]
    pub format: u32,
    /// Eye position in world units (`FlyCamera::position`).
    pub position: [f32; 3],
    /// Radians, as the camera holds them.
    pub yaw: f32,
    pub pitch: f32,
    /// Every slot, hotbar first, `None` for empty — the inventory's own layout.
    #[serde(default)]
    pub inventory: Vec<Option<ItemStack>>,
    /// The selected hotbar slot.
    #[serde(default)]
    pub selected: usize,
    /// `world_clock_elapsed`: game minutes since the world's first 08:00.
    #[serde(default)]
    pub world_clock_elapsed: f32,
    /// The sky's time of day (`SkyState::time_seconds`); `None` in a file written before it
    /// was saved, and the launch keeps its mid-morning default then.
    #[serde(default)]
    pub sky_time_seconds: Option<f64>,
}

fn format_now() -> u32 {
    FORMAT
}

impl PlayerSave {
    pub fn store_path(world_root: &Path) -> PathBuf {
        world_root.join("player.json")
    }

    /// Everything a load must be able to trust: finite numbers, a position inside the world's
    /// vertical range, no zero-count stacks. A file that fails this is treated as absent rather
    /// than placing the player at NaN or handing them an empty stack that `slot` promises
    /// never exists.
    pub fn is_sane(&self) -> bool {
        let finite = self.position.iter().all(|v| v.is_finite())
            && self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.world_clock_elapsed.is_finite()
            && self.sky_time_seconds.map_or(true, |t| t.is_finite() && t >= 0.0);
        finite
            && self.position[1] > -4096.0
            && self.position[1] < 4096.0
            && self.world_clock_elapsed >= 0.0
            && self.inventory.iter().flatten().all(|s| s.count > 0)
    }

    /// Atomic write of `player.json` in `world_root` (created if missing).
    pub fn save(&self, world_root: &Path) -> io::Result<()> {
        std::fs::create_dir_all(world_root)?;
        let path = Self::store_path(world_root);
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)
    }

    /// The world's saved player, or `None` when there is no file, it does not parse, or it
    /// fails [`Self::is_sane`] (each logged at the level it deserves: a missing file is the
    /// normal first visit).
    pub fn load(world_root: &Path) -> Option<PlayerSave> {
        let path = Self::store_path(world_root);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => {
                log::warn!("[player-save] {}: {e}", path.display());
                return None;
            }
        };
        match serde_json::from_slice::<PlayerSave>(&bytes) {
            Ok(p) if p.is_sane() => Some(p),
            Ok(_) => {
                log::warn!("[player-save] {}: unusable values; spawning fresh", path.display());
                None
            }
            Err(e) => {
                log::warn!("[player-save] {}: {e}; spawning fresh", path.display());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Inventory, ItemId};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("unendlich-player-save-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn sample() -> PlayerSave {
        let spec = crate::item::LegendarySpec { seed: 0xDEAD_BEEF, class: crate::sim::legendary_weapon::WeaponClass::Spear, element_idx: 4, tier: 3 };
        let mut inv = vec![None; 36];
        inv[0] = Some(ItemStack::new(ItemId(7), 12));
        inv[3] = Some(ItemStack::new_weapon(ItemId(900), 1, spec));
        inv[35] = Some(ItemStack::new(ItemId(2), 64));
        PlayerSave { format: FORMAT, position: [12.5, 71.0, -3.5], yaw: 1.25, pitch: -0.3, inventory: inv, selected: 3, world_clock_elapsed: 4321.0, sky_time_seconds: Some(640.0) }
    }

    #[test]
    fn round_trips_through_the_file_with_the_forged_weapon_intact() {
        let d = dir("roundtrip");
        assert_eq!(PlayerSave::load(&d), None, "no file is the normal first visit");
        let p = sample();
        p.save(&d).unwrap();
        let back = PlayerSave::load(&d).expect("saved");
        assert_eq!(back, p);
        assert_eq!(back.inventory[3].unwrap().weapon_instance.unwrap().tier, 3);
        assert!(!PlayerSave::store_path(&d).with_extension("json.tmp").exists(), "the temp file was renamed away");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file that does not parse, and files that parse to values a load must not trust.
    /// (serde_json writes NaN as `null`, which then fails to parse -- so the unusable cases
    /// are finite values that `is_sane` alone rejects, and `is_sane` is also asserted directly.)
    #[test]
    fn a_corrupt_or_unusable_file_loads_as_absent() {
        let d = dir("corrupt");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(PlayerSave::store_path(&d), b"{ not json").unwrap();
        assert_eq!(PlayerSave::load(&d), None);
        let mut p = sample();
        p.position[1] = 1.0e9;
        assert!(!p.is_sane());
        p.save(&d).unwrap();
        assert_eq!(PlayerSave::load(&d), None, "a kilometre above the build limit is not a place to stand");
        let mut p = sample();
        p.world_clock_elapsed = -5.0;
        assert!(!p.is_sane());
        p.save(&d).unwrap();
        assert_eq!(PlayerSave::load(&d), None, "a clock before the world began");
        let mut p = sample();
        p.inventory[0] = Some(ItemStack::new(ItemId(7), 0));
        assert!(!p.is_sane());
        p.save(&d).unwrap();
        assert_eq!(PlayerSave::load(&d), None, "a zero-count stack breaks the inventory's invariant");
        let mut p = sample();
        p.yaw = f32::NAN;
        assert!(!p.is_sane(), "NaN is refused by the check itself, whatever the file format does with it");
        assert!(sample().is_sane());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_older_file_without_the_optional_fields_still_loads() {
        let d = dir("older");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(PlayerSave::store_path(&d), br#"{"position":[1.0,2.0,3.0],"yaw":0.0,"pitch":0.0}"#).unwrap();
        let p = PlayerSave::load(&d).expect("loads with defaults");
        assert_eq!(p.format, FORMAT);
        assert!(p.inventory.is_empty());
        assert_eq!(p.world_clock_elapsed, 0.0);
        assert_eq!(p.sky_time_seconds, None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The inventory half of a load: slot for slot into the live inventory, sized by the
    /// inventory and not the file, and never a zero-count stack.
    #[test]
    fn restore_slots_puts_the_layout_back_and_keeps_the_invariant() {
        let p = sample();
        let mut inv = Inventory::standard();
        inv.set_selected(5);
        inv.restore_slots(&p.inventory);
        assert_eq!(inv.slot(0), p.inventory[0]);
        assert_eq!(inv.slot(3), p.inventory[3]);
        assert_eq!(inv.slot(35), p.inventory[35]);
        assert_eq!(inv.slot(1), None);
        // a longer save than the inventory, with a bad stack in it
        let mut long = vec![Some(ItemStack::new(ItemId(1), 0)); 50];
        long[2] = Some(ItemStack::new(ItemId(5), 3));
        inv.restore_slots(&long);
        assert_eq!(inv.slot(0), None, "the zero-count stack is dropped");
        assert_eq!(inv.slot(2), Some(ItemStack::new(ItemId(5), 3)));
        assert_eq!(inv.slots().len(), 36, "the inventory keeps its own size");
        assert_eq!(inv.selected(), 5, "selection within the hotbar is kept");
    }
}
