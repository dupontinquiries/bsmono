//! builder.rs — the custom-map data model and its on-disk format.
//!
//! A `CustomMap` is just a biome + size + spawn point + a flat list of placed assets (`Placed`). The map
//! editor (see `game::Editor`) mutates this list live; `Level::from_custom` turns it into a playable level by
//! instancing the same mesh primitives the procedural generator uses. Everything serialises to a tiny
//! sectioned text file in the user's config dir (`maps/<slug>.map`) — no serde, same spirit as `db.rs`.

use crate::level::MapTheme;
use std::path::PathBuf;

/// A placeable asset kind. Each maps to one or more mesh primitives + (optionally) a collision obstacle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssetKind {
    Tree,
    Pine,
    Bush,
    Rock,
    Boulder,
    Crate,
    Barrel,
    Pillar,
    Crystal,
    Cairn,
    Lamp,
    Reeds,
    ManCannon,
    HealthPack,
    ArmorPack,
    AmmoPack,
}

impl AssetKind {
    pub const ALL: [AssetKind; 16] = [
        AssetKind::Tree,
        AssetKind::Pine,
        AssetKind::Bush,
        AssetKind::Rock,
        AssetKind::Boulder,
        AssetKind::Crate,
        AssetKind::Barrel,
        AssetKind::Pillar,
        AssetKind::Crystal,
        AssetKind::Cairn,
        AssetKind::Lamp,
        AssetKind::Reeds,
        AssetKind::ManCannon,
        AssetKind::HealthPack,
        AssetKind::ArmorPack,
        AssetKind::AmmoPack,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AssetKind::Tree => "Tree",
            AssetKind::Pine => "Pine",
            AssetKind::Bush => "Bush",
            AssetKind::Rock => "Rock",
            AssetKind::Boulder => "Boulder",
            AssetKind::Crate => "Crate",
            AssetKind::Barrel => "Barrel",
            AssetKind::Pillar => "Pillar",
            AssetKind::Crystal => "Crystal",
            AssetKind::Cairn => "Cairn",
            AssetKind::Lamp => "Lamp",
            AssetKind::Reeds => "Reeds",
            AssetKind::ManCannon => "Man Cannon",
            AssetKind::HealthPack => "Health",
            AssetKind::ArmorPack => "Armor",
            AssetKind::AmmoPack => "Ammo",
        }
    }

    /// Asset family, used to group the brush picker into tabs.
    pub fn category(self) -> &'static str {
        match self {
            AssetKind::Tree | AssetKind::Pine | AssetKind::Bush | AssetKind::Reeds => "Flora",
            AssetKind::Rock | AssetKind::Boulder | AssetKind::Cairn | AssetKind::Crystal => "Terrain",
            AssetKind::Crate | AssetKind::Barrel | AssetKind::Pillar | AssetKind::Lamp => "Props",
            AssetKind::ManCannon => "Gadgets",
            AssetKind::HealthPack | AssetKind::ArmorPack | AssetKind::AmmoPack => "Pickups",
        }
    }

    /// Pickups carried by this asset (for the game to spawn collectibles), if any.
    pub fn pickup(self) -> Option<crate::pickups::PickupKind> {
        match self {
            AssetKind::HealthPack => Some(crate::pickups::PickupKind::Health),
            AssetKind::ArmorPack => Some(crate::pickups::PickupKind::Armor),
            AssetKind::AmmoPack => Some(crate::pickups::PickupKind::Ammo),
            _ => None,
        }
    }

    pub fn is_cannon(self) -> bool {
        matches!(self, AssetKind::ManCannon)
    }

    /// Default value for the asset's tunable `param` (man-cannon launch strength; 0 = unused).
    pub fn default_param(self) -> f32 {
        match self {
            AssetKind::ManCannon => 9.0,
            _ => 0.0,
        }
    }

    pub fn id(self) -> u8 {
        AssetKind::ALL.iter().position(|&k| k == self).unwrap_or(0) as u8
    }

    pub fn from_id(i: u8) -> AssetKind {
        AssetKind::ALL[(i as usize).min(AssetKind::ALL.len() - 1)]
    }

    /// Whether placing this asset also drops a collision obstacle (trees/rocks/crates block; bushes/reeds/
    /// lamps/litter do not).
    pub fn solid(self) -> bool {
        matches!(
            self,
            AssetKind::Tree
                | AssetKind::Pine
                | AssetKind::Rock
                | AssetKind::Boulder
                | AssetKind::Crate
                | AssetKind::Barrel
                | AssetKind::Pillar
                | AssetKind::Crystal
                | AssetKind::Cairn
        )
    }
}

/// One placed asset in world space (x,z on the ground plane; rotation in radians; uniform scale).
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub kind: AssetKind,
    pub x: f32,
    pub z: f32,
    pub rot: f32,
    pub scale: f32,
    /// Tunable parameter (man-cannon launch strength; 0 for most assets).
    pub param: f32,
}

/// A complete custom map: a biome base + a spawn point + everything the author placed.
#[derive(Clone, Debug)]
pub struct CustomMap {
    pub name: String,
    pub theme: MapTheme,
    pub size: i32,
    pub spawn: (f32, f32),
    pub placed: Vec<Placed>,
}

impl CustomMap {
    /// A fresh, empty map of a biome at a size, spawn at the centre.
    pub fn new(name: &str, theme: MapTheme, size: i32) -> Self {
        let size = size.clamp(64, 240);
        let c = size as f32 * 2.0 * 0.5; // tile size is 2.0; centre in world units
        Self { name: name.to_string(), theme, size, spawn: (c, c), placed: Vec::new() }
    }

    /// Serialise to the sectioned text format.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("name={}\n", self.name));
        s.push_str(&format!("theme={}\n", self.theme.id()));
        s.push_str(&format!("size={}\n", self.size));
        s.push_str(&format!("spawn={:.2},{:.2}\n", self.spawn.0, self.spawn.1));
        for p in &self.placed {
            s.push_str(&format!("p={},{:.2},{:.2},{:.3},{:.3},{:.2}\n", p.kind.id(), p.x, p.z, p.rot, p.scale, p.param));
        }
        s
    }

    /// Parse the text format back; `None` if it isn't a map file.
    pub fn parse(text: &str) -> Option<Self> {
        let mut name = String::from("Custom");
        let mut theme = MapTheme::Canopy;
        let mut size = 160i32;
        let mut spawn = (160.0f32, 160.0f32);
        let mut placed = Vec::new();
        let mut saw_any = false;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            saw_any = true;
            match k {
                "name" => name = v.to_string(),
                "theme" => theme = MapTheme::from_id(v.parse().unwrap_or(8)),
                "size" => size = v.parse::<i32>().unwrap_or(160).clamp(64, 240),
                "spawn" => {
                    if let Some((a, b)) = v.split_once(',') {
                        spawn = (a.trim().parse().unwrap_or(160.0), b.trim().parse().unwrap_or(160.0));
                    }
                }
                "p" => {
                    let f: Vec<&str> = v.split(',').collect();
                    if f.len() >= 5 {
                        let kind = AssetKind::from_id(f[0].trim().parse().unwrap_or(0));
                        let param = if f.len() >= 6 {
                            f[5].trim().parse::<f32>().unwrap_or(kind.default_param())
                        } else {
                            kind.default_param()
                        };
                        placed.push(Placed {
                            kind,
                            x: f[1].trim().parse().unwrap_or(0.0),
                            z: f[2].trim().parse().unwrap_or(0.0),
                            rot: f[3].trim().parse().unwrap_or(0.0),
                            scale: f[4].trim().parse::<f32>().unwrap_or(1.0).clamp(0.3, 4.0),
                            param,
                        });
                    }
                }
                _ => {}
            }
        }
        if saw_any { Some(Self { name, theme, size, spawn, placed }) } else { None }
    }

    /// The directory custom maps live in (`<config>/early-fps/maps/`).
    pub fn dir() -> PathBuf {
        let base = crate::profile::Profile::path();
        base.parent().map(|d| d.join("maps")).unwrap_or_else(|| PathBuf::from("early-fps-maps"))
    }

    /// A filesystem-safe slug from the map name.
    fn slug(&self) -> String {
        let s: String = self
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        let s = s.trim_matches('_').to_string();
        if s.is_empty() { "map".to_string() } else { s }
    }

    /// Write the map to disk; returns whether it succeeded.
    pub fn save(&self) -> bool {
        let dir = Self::dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let path = dir.join(format!("{}.map", self.slug()));
        std::fs::write(path, self.to_text()).is_ok()
    }

    /// List the names of all saved custom maps (sorted), reading each file's `name=` field.
    pub fn list() -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(Self::dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("map") {
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        if let Some(m) = CustomMap::parse(&text) {
                            out.push(m.name);
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Load a map by its display name (matches the `name=` field, case-insensitively).
    pub fn load(name: &str) -> Option<CustomMap> {
        let want = name.to_lowercase();
        if let Ok(rd) = std::fs::read_dir(Self::dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("map") {
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        if let Some(m) = CustomMap::parse(&text) {
                            if m.name.to_lowercase() == want {
                                return Some(m);
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_text() {
        let mut m = CustomMap::new("Test Glade", MapTheme::Mistfall, 160);
        m.spawn = (80.0, 96.0);
        m.placed.push(Placed { kind: AssetKind::Tree, x: 12.0, z: 30.0, rot: 1.57, scale: 1.4, param: 0.0 });
        m.placed.push(Placed { kind: AssetKind::Crystal, x: 44.0, z: 8.0, rot: 0.0, scale: 0.8, param: 0.0 });
        let back = CustomMap::parse(&m.to_text()).expect("parses");
        assert_eq!(back.name, "Test Glade");
        assert_eq!(back.theme, MapTheme::Mistfall);
        assert_eq!(back.size, 160);
        assert_eq!(back.placed.len(), 2);
        assert_eq!(back.placed[0].kind, AssetKind::Tree);
        assert!((back.placed[1].x - 44.0).abs() < 0.01);
    }

    #[test]
    fn asset_ids_round_trip() {
        for k in AssetKind::ALL {
            assert_eq!(AssetKind::from_id(k.id()), k);
        }
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(CustomMap::parse("").is_none());
    }
}
