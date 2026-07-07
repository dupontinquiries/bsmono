//! `world::named_locations` (r461) — procedural lore names for biome-scale regions (SPEC-010).
//!
//! Named locations are biome-scale areas the player discovers by exploring; on first entry the area
//! gets a deterministic lore name ("The Verdant Reach", "Ashen Crag", "Barrow of the Mire") and is
//! registered for the Collections tab. Pure name generation + a discovery registry; the bin tracks
//! the player's region and announces new discoveries. Deterministic in (region, world seed).

use crate::world::biomes::Biome;
use crate::util::hash_util::mix64;
use std::collections::HashMap;

/// Side length (blocks) of a named region. Biome-scale — a region spans several chunks.
pub const REGION_SIZE: i32 = 256;

/// The region grid cell a world position falls in.
pub fn region_of(wx: i32, wz: i32) -> (i32, i32) {
    (wx.div_euclid(REGION_SIZE), wz.div_euclid(REGION_SIZE))
}

fn h(rx: i32, rz: i32, seed: u64, salt: u64) -> u64 {
    mix64(seed ^ salt ^ ((rx as i64 as u64) << 32) ^ ((rz as i64 as u64) & 0xffff_ffff))
}

/// The evocative noun for a biome's lore name.
fn biome_noun(b: Biome) -> &'static str {
    use Biome::*;
    match b {
        Plains | Savanna => "Reach",
        TemperateForest | BirchForest | MapleForest | CloudForest => "Wood",
        Jungle => "Deep",
        Taiga => "Pinewood",
        Desert | Mesa | RedSandstoneCanyon | BloomingCanyon => "Wastes",
        Ocean => "Expanse",
        FrozenOcean => "Ice Expanse",
        Beach => "Shore",
        Swamp | Mangrove => "Mire",
        Tundra => "Tundra",
        Skylands => "Skyreach",
        MistyMountains | AlpineHighlands | TemperateHighlands | HighlandSteppe => "Highlands",
        VolcanicWastes => "Cinderlands",
        IceCave => "Frosthollow",
        FungalCave => "Sporehollow",
        ThermalCave => "Emberdeep",
        CrystalCave => "Crystalhollow",
        LushCave => "Hollow",
    }
}

const ADJ: [&str; 16] = [
    "Verdant", "Ashen", "Sunken", "Windswept", "Hidden", "Ancient", "Whispering", "Shattered",
    "Golden", "Frostbound", "Emerald", "Forgotten", "Silent", "Crimson", "Misty", "Wandering",
];
const PFX: [&str; 8] = ["Grove", "Spire", "Vale", "Crag", "Fen", "Barrow", "Hollow", "Reach"];

/// Deterministic lore name for the region containing biome `biome` under `seed`.
pub fn location_name(rx: i32, rz: i32, biome: Biome, seed: u64) -> String {
    let a = ADJ[(h(rx, rz, seed, 0xA1) % ADJ.len() as u64) as usize];
    let noun = biome_noun(biome);
    match h(rx, rz, seed, 0xB2) % 3 {
        0 => format!("The {a} {noun}"),
        1 => {
            let p = PFX[(h(rx, rz, seed, 0xC3) % PFX.len() as u64) as usize];
            format!("{a} {p}")
        }
        _ => {
            let p = PFX[(h(rx, rz, seed, 0xD4) % PFX.len() as u64) as usize];
            format!("{p} of the {noun}")
        }
    }
}

/// Tracks which regions the player has discovered (per world). Feeds the Collections tab.
#[derive(Default)]
pub struct LocationRegistry {
    discovered: HashMap<(i32, i32), String>,
}

impl LocationRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_discovered(&self, rx: i32, rz: i32) -> bool {
        self.discovered.contains_key(&(rx, rz))
    }
    /// Record a discovery. Returns true if it was new (first visit).
    pub fn discover(&mut self, rx: i32, rz: i32, name: String) -> bool {
        if self.discovered.contains_key(&(rx, rz)) {
            false
        } else {
            self.discovered.insert((rx, rz), name);
            true
        }
    }
    pub fn name_at(&self, rx: i32, rz: i32) -> Option<&str> {
        self.discovered.get(&(rx, rz)).map(|s| s.as_str())
    }
    pub fn count(&self) -> usize {
        self.discovered.len()
    }
    /// Clear all discoveries (called on world switch — the registry is per-world).
    pub fn clear(&mut self) {
        self.discovered.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic() {
        assert_eq!(
            location_name(3, 5, Biome::Plains, 42),
            location_name(3, 5, Biome::Plains, 42)
        );
    }
    #[test]
    fn region_math() {
        assert_eq!(region_of(0, 0), (0, 0));
        assert_eq!(region_of(255, 255), (0, 0));
        assert_eq!(region_of(256, -1), (1, -1));
    }
    #[test]
    fn registry_discovers_once() {
        let mut r = LocationRegistry::new();
        assert!(r.discover(1, 1, "The Verdant Reach".into()));
        assert!(!r.discover(1, 1, "The Verdant Reach".into()));
        assert_eq!(r.count(), 1);
        assert!(r.is_discovered(1, 1));
        r.clear();
        assert_eq!(r.count(), 0);
    }
    #[test]
    fn variety() {
        let mut s = std::collections::HashSet::new();
        for rx in 0..40 {
            for rz in 0..40 {
                s.insert(location_name(rx, rz, Biome::Plains, 1));
            }
        }
        assert!(s.len() > 20, "only {} distinct names", s.len());
    }
}
