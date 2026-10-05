//! db.rs — the persistence backend. A single on-disk store that holds the player's metaprofile (K/D and
//! matchmaking stats) and their saved loadout, written as a small sectioned `key=value` text file in the
//! user's config directory. It deliberately avoids any external database/serde crates: the format is a
//! handful of lines that load and save in a few millis, which is all a local single-player shell needs —
//! and it's the same shape a future networked backend would sync.
//!
//! This is the only module that touches the disk for game state; the headless tests never call `load`/`save`.

use crate::cosmetics::{Accent, ArmorFrame, Cosmetics, GunSkin, Reticle};
use crate::graphics::GraphicsPreset;
use crate::loadout::{Loadout, Perk};
use crate::profile::Profile;
use crate::weapons::WeaponKind;
use std::path::PathBuf;

/// The player's options as the settings page edits them, in the plain form the save file carries: numbers,
/// flags, a preset name, two RGB colours and `(action, key)` name pairs. The app's live settings struct
/// (egui keys and colours) converts to and from this; the record itself knows nothing about egui, so it is
/// built and tested headless. The defaults here ARE the game's defaults — the app derives its own from them.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub sensitivity: f32,
    pub invert_y: bool,
    pub fov_deg: f32,
    pub third_person: bool,
    pub show_minimap: bool,
    pub render_detail: f32,
    pub texture_detail: f32,
    pub graphics_preset: GraphicsPreset,
    pub hostile_color: [u8; 3],
    pub friendly_color: [u8; 3],
    /// `(action id, key name)` for every rebindable action, in the app's action order. Empty means "the
    /// layout's defaults"; an action missing from the list keeps its default key, so a save written by an
    /// older build (or hand-edited) never strips a binding.
    pub binds: Vec<(String, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sensitivity: 0.0022,
            invert_y: false,
            fov_deg: 74.0,
            third_person: false,
            show_minimap: true,
            render_detail: 1.4,
            texture_detail: 3.0,
            graphics_preset: GraphicsPreset::default(),
            hostile_color: [230, 120, 40], // orange
            friendly_color: [70, 190, 90], // green
            binds: Vec::new(),
        }
    }
}

impl Options {
    /// Apply one `key=value` line of the save file; `false` if the key is not an option key. Values that
    /// do not parse leave the field alone, so a damaged line costs one setting, not the record.
    fn apply_kv(&mut self, k: &str, v: &str) -> bool {
        match k {
            "opt.sensitivity" => self.sensitivity = parse_f32(v, self.sensitivity, 0.0001, 0.1),
            "opt.invert_y" => self.invert_y = flag(v),
            "opt.fov_deg" => self.fov_deg = parse_f32(v, self.fov_deg, 30.0, 140.0),
            "opt.third_person" => self.third_person = flag(v),
            "opt.show_minimap" => self.show_minimap = flag(v),
            "opt.render_detail" => self.render_detail = parse_f32(v, self.render_detail, 0.1, 10.0),
            "opt.texture_detail" => self.texture_detail = parse_f32(v, self.texture_detail, 0.5, 16.0),
            "opt.graphics_preset" => self.graphics_preset = GraphicsPreset::from_name(v).unwrap_or(self.graphics_preset),
            "opt.hostile_color" => self.hostile_color = parse_hex_rgb(v).unwrap_or(self.hostile_color),
            "opt.friendly_color" => self.friendly_color = parse_hex_rgb(v).unwrap_or(self.friendly_color),
            _ => {
                let Some(action) = k.strip_prefix("bind.") else { return false };
                if action.is_empty() || v.is_empty() {
                    return true;
                }
                match self.binds.iter_mut().find(|(a, _)| a == action) {
                    Some(slot) => slot.1 = v.to_string(),
                    None => self.binds.push((action.to_string(), v.to_string())),
                }
            }
        }
        true
    }

    fn to_text(&self) -> String {
        let mut out = format!(
            "opt.sensitivity={}\nopt.invert_y={}\nopt.fov_deg={}\nopt.third_person={}\nopt.show_minimap={}\n\
             opt.render_detail={}\nopt.texture_detail={}\nopt.graphics_preset={}\nopt.hostile_color={}\nopt.friendly_color={}\n",
            self.sensitivity,
            self.invert_y as u8,
            self.fov_deg,
            self.third_person as u8,
            self.show_minimap as u8,
            self.render_detail,
            self.texture_detail,
            self.graphics_preset.name(),
            hex_rgb(self.hostile_color),
            hex_rgb(self.friendly_color),
        );
        for (action, key) in &self.binds {
            out.push_str(&format!("bind.{action}={key}\n"));
        }
        out
    }
}

fn flag(v: &str) -> bool {
    v == "1" || v.eq_ignore_ascii_case("true")
}

/// A float within `[lo, hi]`, else `keep` — a NaN, an inf or a number off the slider's scale would
/// otherwise come back as an unusable (or invisible) setting.
fn parse_f32(v: &str, keep: f32, lo: f32, hi: f32) -> f32 {
    match v.parse::<f32>() {
        Ok(x) if x.is_finite() && (lo..=hi).contains(&x) => x,
        _ => keep,
    }
}

/// `#rrggbb` (the `#` optional, case-insensitive).
pub fn parse_hex_rgb(v: &str) -> Option<[u8; 3]> {
    let h = v.trim().trim_start_matches('#');
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([c(0)?, c(2)?, c(4)?])
}

pub fn hex_rgb(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

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
    /// The settings page's state (mouse, view, graphics, colours, key bindings).
    pub options: Options,
}

impl Default for Db {
    fn default() -> Self {
        Self {
            profile: Profile::default(),
            loadout: Loadout::default(),
            cosmetics: Cosmetics::default(),
            map_prefs: Vec::new(),
            gpu_renderer: false,
            options: Options::default(),
        }
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
                _ => {
                    self.options.apply_kv(k, v);
                }
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
        format!("{out}map_prefs={}\ngpu_renderer={}\n{}", prefs.join(","), self.gpu_renderer as u8, self.options.to_text())
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

    fn edited_options() -> Options {
        Options {
            sensitivity: 0.0041,
            invert_y: true,
            fov_deg: 91.0,
            third_person: true,
            show_minimap: false,
            render_detail: 2.2,
            texture_detail: 5.0,
            graphics_preset: GraphicsPreset::Low,
            hostile_color: [255, 0, 128],
            friendly_color: [1, 2, 3],
            binds: vec![("forward".into(), "W".into()), ("jump".into(), "Space".into()), ("forge_menu".into(), "Escape".into())],
        }
    }

    #[test]
    fn options_round_trip_through_the_save_text() {
        let mut db = Db::default();
        db.options = edited_options();
        let text = db.to_text();
        let mut q = Db::default();
        q.parse(&text);
        assert_eq!(q.options, edited_options());
        // the option lines sit in the same file as the profile, not beside it
        assert!(text.contains("opt.fov_deg=91\n"), "{text}");
        assert!(text.contains("bind.jump=Space\n"), "{text}");
        assert_eq!(text.matches("bind.").count(), 3);
    }

    #[test]
    fn a_save_without_option_lines_yields_the_defaults() {
        // an older save (profile + loadout only) must not disturb the settings page's defaults
        let mut q = Db::default();
        q.parse("# early-fps save\nname=old\nkills=3\n");
        assert_eq!(q.options, Options::default());
        assert_eq!(q.profile.name, "old");
    }

    #[test]
    fn damaged_option_lines_cost_one_setting_not_the_record() {
        let mut q = Db::default();
        q.parse(
            "opt.sensitivity=NaN\nopt.fov_deg=9999\nopt.render_detail=abc\nopt.graphics_preset=Extreme\n\
             opt.hostile_color=#12345\nopt.friendly_color=#0a0B0c\nopt.invert_y=true\nbind.=X\nbind.jump=\nbind.dive=J\n",
        );
        let d = Options::default();
        assert_eq!(q.options.sensitivity, d.sensitivity, "NaN is refused");
        assert_eq!(q.options.fov_deg, d.fov_deg, "off-scale values are refused");
        assert_eq!(q.options.render_detail, d.render_detail);
        assert_eq!(q.options.graphics_preset, d.graphics_preset, "unknown preset keeps the default");
        assert_eq!(q.options.hostile_color, d.hostile_color, "a five-digit colour is refused");
        assert_eq!(q.options.friendly_color, [0x0a, 0x0b, 0x0c], "mixed-case hex parses");
        assert!(q.options.invert_y, "the good line beside the bad ones still lands");
        assert_eq!(q.options.binds, vec![("dive".to_string(), "J".to_string())], "empty action or key lines are skipped");
    }

    #[test]
    fn a_repeated_bind_line_overwrites_in_place() {
        let mut q = Db::default();
        q.parse("bind.forward=W\nbind.left=A\nbind.forward=Period\n");
        assert_eq!(q.options.binds, vec![("forward".to_string(), "Period".to_string()), ("left".to_string(), "A".to_string())]);
    }

    #[test]
    fn hex_colours_round_trip() {
        for c in [[0, 0, 0], [255, 255, 255], [230, 120, 40], [1, 2, 3]] {
            assert_eq!(parse_hex_rgb(&hex_rgb(c)), Some(c));
        }
        assert_eq!(parse_hex_rgb("ff0000"), Some([255, 0, 0]), "the hash is optional");
        assert_eq!(parse_hex_rgb("#ff00zz"), None);
        assert_eq!(parse_hex_rgb("#ff0000ff"), None, "no alpha");
    }
}
