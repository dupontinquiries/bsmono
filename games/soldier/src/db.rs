//! db.rs — the persistence backend. A single on-disk store that holds the player's metaprofile (K/D and
//! matchmaking stats) and their saved loadout, written as a small sectioned `key=value` text file in the
//! user's config directory. It deliberately avoids any external database/serde crates: the format is a
//! handful of lines that load and save in a few millis, which is all a local single-player shell needs —
//! and it's the same shape a future networked backend would sync.
//!
//! This is the only module that touches the disk for game state; the headless tests never call `load`/`save`.

use crate::cosmetics::{Accent, ArmorFrame, Cosmetics, GunSkin, Reticle};
use crate::loadout::{Loadout, Perk};
use crate::profile::Profile;
use crate::weapons::WeaponKind;
use std::path::PathBuf;

/// The whole persisted player record.
#[derive(Clone, Debug)]
pub struct Db {
    pub profile: Profile,
    pub loadout: Loadout,
    pub cosmetics: Cosmetics,
    /// Preferred map preset indices — the matchmaker draws from these when non-empty.
    pub map_prefs: Vec<usize>,
    /// Preferred renderer: false = software painter (default), true = GPU (wgpu). Read at launch.
    pub gpu_renderer: bool,
}

impl Default for Db {
    fn default() -> Self {
        Self { profile: Profile::default(), loadout: Loadout::default(), cosmetics: Cosmetics::default(), map_prefs: Vec::new(), gpu_renderer: false }
    }
}

impl Db {
    fn path() -> PathBuf {
        // Co-locate with the profile path, under the same config dir.
        let p = Profile::path();
        p.parent().map(|d| d.join("save.txt")).unwrap_or_else(|| PathBuf::from("early-fps-save.txt"))
    }

    /// Load the combined record, falling back to defaults for anything missing.
    pub fn load() -> Self {
        let mut db = Db::default();
        if let Ok(text) = std::fs::read_to_string(Self::path()) {
            db.parse(&text);
        }
        db
    }

    fn parse(&mut self, text: &str) {
        let mut perks: [Option<Perk>; 3] = [None, None, None];
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "name" => self.profile.name = v.to_string(),
                "kills" => self.profile.kills = v.parse().unwrap_or(self.profile.kills),
                "deaths" => self.profile.deaths = v.parse().unwrap_or(self.profile.deaths),
                "matches" => self.profile.matches = v.parse().unwrap_or(self.profile.matches),
                "best_streak" => self.profile.best_streak = v.parse().unwrap_or(self.profile.best_streak),
                "best_wave" => self.profile.best_wave = v.parse().unwrap_or(self.profile.best_wave),
                "primary" => self.loadout.primary = WeaponKind::from_id(v.parse().unwrap_or(0)),
                "legendary" => self.loadout.legendary = v.parse::<i32>().ok().filter(|&n| n >= 0).map(|n| n as usize),
                "secondary" => self.loadout.secondary = WeaponKind::from_id(v.parse().unwrap_or(6)),
                "perk0" => perks[0] = v.parse().ok().and_then(Perk::from_id),
                "perk1" => perks[1] = v.parse().ok().and_then(Perk::from_id),
                "perk2" => perks[2] = v.parse().ok().and_then(Perk::from_id),
                "reticle" => self.cosmetics.reticle = Reticle::from_id(v.parse().unwrap_or(0)),
                "reticle_color" => self.cosmetics.reticle_color = Accent::from_id(v.parse().unwrap_or(0)),
                "skin" => self.cosmetics.skin = GunSkin::from_id(v.parse().unwrap_or(0)),
                "armor" => self.cosmetics.armor = ArmorFrame::from_id(v.parse().unwrap_or(0)),
                "armor_color" => self.cosmetics.armor_color = Accent::from_id(v.parse().unwrap_or(0)),
                "armorset_styles" | "armorset_accents" => self.cosmetics.set.apply_kv(k, v),
                "map_prefs" => {
                    self.map_prefs = v.split(',').filter_map(|t| t.trim().parse::<usize>().ok()).collect();
                }
                "gpu_renderer" => self.gpu_renderer = v == "1" || v == "true",
                _ => {}
            }
        }
        self.loadout.perks = perks;
    }

    fn to_text(&self) -> String {
        let p = &self.profile;
        let l = &self.loadout;
        let perk = |o: Option<Perk>| o.map(|p| p.id() as i32).unwrap_or(-1);
        let c = &self.cosmetics;
        let out = format!(
            "# early-fps save\nname={}\nkills={}\ndeaths={}\nmatches={}\nbest_streak={}\nbest_wave={}\n\
             primary={}\nsecondary={}\nperk0={}\nperk1={}\nperk2={}\nlegendary={}\n\
             reticle={}\nreticle_color={}\nskin={}\narmor={}\narmor_color={}\n{}",
            p.name,
            p.kills,
            p.deaths,
            p.matches,
            p.best_streak,
            p.best_wave,
            l.primary.id(),
            l.secondary.id(),
            perk(l.perks[0]),
            perk(l.perks[1]),
            perk(l.perks[2]),
            l.legendary.map(|i| i as i32).unwrap_or(-1),
            c.reticle.id(),
            c.reticle_color.id(),
            c.skin.id(),
            c.armor.id(),
            c.armor_color.id(),
            c.set.to_text(),
        );
        let prefs: Vec<String> = self.map_prefs.iter().map(|i| i.to_string()).collect();
        format!("{out}map_prefs={}\ngpu_renderer={}\n", prefs.join(","), self.gpu_renderer as u8)
    }

    /// Persist the whole record (best-effort).
    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_round_trips_profile_and_loadout() {
        let mut db = Db::default();
        db.profile.record_life(9, 6, 12);
        db.loadout.primary = WeaponKind::Lmg;
        db.loadout.perks = [Some(Perk::Camo), Some(Perk::Jetpack), None];
        db.cosmetics.reticle = Reticle::Chevron;
        db.cosmetics.skin = GunSkin::Gold;
        db.cosmetics.armor = ArmorFrame::Eod;
        db.loadout.legendary = Some(4);
        db.map_prefs = vec![2, 5, 9];
        db.cosmetics.set.set_style(crate::character::ArmorSlot::Chest, crate::character::PieceStyle::Heavy);
        let text = db.to_text();
        let mut q = Db::default();
        q.parse(&text);
        assert_eq!(q.profile, db.profile);
        assert_eq!(q.loadout, db.loadout);
        assert_eq!(q.loadout.legendary, Some(4));
        assert_eq!(q.map_prefs, vec![2, 5, 9]);
        assert_eq!(q.cosmetics.set.style(crate::character::ArmorSlot::Chest), crate::character::PieceStyle::Heavy);
    }
}
