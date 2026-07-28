//! editor.rs — slice 8: the EDITOR's data layer, ported headless from
//! rts_editor.py's data mode. The floating-window UI stays in Python; what
//! Rust owns is the part worth owning: load → edit → VALIDATE → save, where
//! the validator encodes every match-start crash this project has actually
//! shipped and fixed:
//!
//!   • a pack map without `nodes` crashed the Watch button on Dragonspire
//!   • a map missing size/spawns can't start a match at all
//!   • an overlay snapshot predating expansion races was a KeyError
//!   • unknown effect kinds / projectile styles would no-op or crash later
//!
//! `pbedit` (the CLI binary) drives this over the real rts_data.json and
//! expansion packs, using the monorepo's shared cli-kit for arguments.

use std::path::{Path, PathBuf};

pub const EFFECT_KINDS: &[&str] = &[
    "damage", "heal", "slow", "stun", "burn", "buff", "shield", "shred", "drain",
    "knockback", "pull", "spawn", "reveal", "splinter", "inkpool", "quake",
    "build", "train", "research", "cast",
];

pub const PROJECTILE_STYLES: &[&str] =
    &["dart", "quill", "cinder", "inkglob", "gale", "thorn", "sting"];

pub const DELIVERY_MODES: &[&str] = &["area", "bolt", "chain", "rain"];

pub struct Editor {
    /// Lazily-loaded card models + part vocabulary, shared with the renderer so
    /// the editor's answers and the drawn model can never drift apart.
    book: Option<crate::origami::ModelBook>,
    pub path: PathBuf,
    pub doc: serde_json::Value,
}

impl Editor {
    pub fn open(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc = serde_json::from_str(&raw).map_err(|e| format!("parse: {e}"))?;
        Ok(Editor { path: path.into(), doc, book: None })
    }

    /// The card models + part vocabulary, loaded on first use from the game
    /// directory that holds this data file. Cached: the book is read-only here
    /// and re-reading it per command would be a needless file round-trip.
    pub fn model_book(&mut self) -> Result<&crate::origami::ModelBook, String> {
        if self.book.is_none() {
            let dir = self.path.parent().unwrap_or_else(|| std::path::Path::new("."));
            self.book = Some(crate::origami::ModelBook::load(dir)?);
        }
        Ok(self.book.as_ref().unwrap())
    }

    fn factions(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        // rts_data.json uses "factions"; expansion packs use "rts_factions"
        self.doc
            .get("factions")
            .or_else(|| self.doc.get("rts_factions"))?
            .as_object()
    }

    fn factions_mut(&mut self) -> Option<&mut serde_json::Map<String, serde_json::Value>> {
        let key = if self.doc.get("factions").is_some() { "factions" } else { "rts_factions" };
        self.doc.get_mut(key)?.as_object_mut()
    }

    fn maps(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.doc
            .get("maps")
            .or_else(|| self.doc.get("rts_maps"))?
            .as_object()
    }

    pub fn list(&self, what: &str) -> Vec<String> {
        match what {
            "factions" => self.factions().map(|f| f.keys().cloned().collect()).unwrap_or_default(),
            "maps" => self.maps().map(|m| m.keys().cloned().collect()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Read a dotted path: faction.units.soldier.dmg
    pub fn get(&self, dotted: &str) -> Option<&serde_json::Value> {
        // resolve against the whole doc but allow the shorthand of starting
        // at a faction key directly
        let parts: Vec<&str> = dotted.split('.').collect();
        let mut it = parts.iter();
        let first = it.next()?;
        let mut cur = if self.doc.get(first).is_some() {
            self.doc.get(first)?
        } else {
            self.factions()?.get(*first)?
        };
        for p in it {
            cur = cur.get(*p)?;
        }
        Some(cur)
    }

    /// Set a dotted path (numbers parse as numbers). Returns the old value.
    pub fn set(&mut self, dotted: &str, val: &str) -> Result<Option<serde_json::Value>, String> {
        let parts: Vec<String> = dotted.split('.').map(String::from).collect();
        if parts.is_empty() {
            return Err("empty path".into());
        }
        let new_val: serde_json::Value = if let Ok(n) = val.parse::<i64>() {
            n.into()
        } else if let Ok(f) = val.parse::<f64>() {
            serde_json::json!(f)
        } else if val == "true" || val == "false" {
            (val == "true").into()
        } else {
            val.into()
        };
        // domain guards for the fields the engine dispatches on
        if parts.last().map(|s| s.as_str()) == Some("projectile")
            && !PROJECTILE_STYLES.contains(&val)
        {
            return Err(format!("'{val}' is not a projectile style ({PROJECTILE_STYLES:?})"));
        }
        if parts.last().map(|s| s.as_str()) == Some("delivery") && !DELIVERY_MODES.contains(&val) {
            return Err(format!("'{val}' is not a delivery mode ({DELIVERY_MODES:?})"));
        }
        let in_factions = self.doc.get(&parts[0]).is_none();
        let mut cur: &mut serde_json::Value = if in_factions {
            let facs = self.factions_mut().ok_or("no factions")?;
            facs.get_mut(&parts[0]).ok_or(format!("no faction '{}'", parts[0]))?
        } else {
            self.doc.get_mut(&parts[0]).unwrap()
        };
        for p in &parts[1..parts.len() - 1] {
            cur = cur.get_mut(p).ok_or(format!("no '{p}' along {dotted}"))?;
        }
        let leaf = parts.last().unwrap();
        let obj = cur.as_object_mut().ok_or("not an object")?;
        Ok(obj.insert(leaf.clone(), new_val))
    }

    /// The validator: every rule here is a crash or silent failure this
    /// project has actually seen. Returns human-readable findings.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(maps) = self.maps() {
            for (mk, m) in maps {
                if m.get("size").is_none() || m.get("spawns").is_none() {
                    out.push(format!("map '{mk}': missing size/spawns — a match cannot start"));
                }
                if m.get("nodes").is_none() {
                    out.push(format!(
                        "map '{mk}': no 'nodes' — the Dragonspire Watch-button crash class \
                         (engine defaults to 12, but author it)"
                    ));
                }
                if m.get("seed").is_none() {
                    out.push(format!("map '{mk}': no 'seed' — layout won't be reproducible"));
                }
            }
        }
        if let Some(facs) = self.factions() {
            for (fk, f) in facs {
                if let Some(style) = f.get("projectile").and_then(|v| v.as_str()) {
                    if !PROJECTILE_STYLES.contains(&style) {
                        out.push(format!("faction '{fk}': unknown projectile '{style}'"));
                    }
                }
                let units = f.get("units").and_then(|u| u.as_object());
                for (uk, u) in units.into_iter().flatten() {
                    if let Some(style) = u.get("projectile").and_then(|v| v.as_str()) {
                        if !PROJECTILE_STYLES.contains(&style) {
                            out.push(format!("{fk}.units.{uk}: unknown projectile '{style}'"));
                        }
                    }
                    if let Some(ab) = u.get("ability") {
                        if let Some(d) = ab.get("delivery").and_then(|v| v.as_str()) {
                            if !DELIVERY_MODES.contains(&d) {
                                out.push(format!("{fk}.units.{uk}: unknown delivery '{d}'"));
                            }
                        }
                        for eff in ab
                            .get("effects")
                            .and_then(|e| e.as_array())
                            .into_iter()
                            .flatten()
                        {
                            if let Some(k) = eff.get("effect").and_then(|v| v.as_str()) {
                                if !EFFECT_KINDS.contains(&k) {
                                    out.push(format!(
                                        "{fk}.units.{uk}: unknown effect kind '{k}'"
                                    ));
                                }
                            }
                        }
                    }
                    // trained units must be affordable in principle
                    if u.get("cost").and_then(|c| c.as_f64()).unwrap_or(0.0) < 0.0 {
                        out.push(format!("{fk}.units.{uk}: negative cost"));
                    }
                }
                // structures' 'makes' must reference real units
                let structs = f.get("structures").and_then(|s| s.as_object());
                for (sk, s) in structs.into_iter().flatten() {
                    for mk in s.get("makes").and_then(|m| m.as_array()).into_iter().flatten() {
                        if let Some(mk) = mk.as_str() {
                            let known = units.map(|u| u.contains_key(mk)).unwrap_or(false);
                            if !known {
                                out.push(format!(
                                    "{fk}.structures.{sk}: makes unknown unit '{mk}'"
                                ));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    pub fn save(&self, to: Option<&Path>) -> Result<(), String> {
        let path = to.unwrap_or(&self.path);
        let out = serde_json::to_string_pretty(&self.doc).map_err(|e| e.to_string())?;
        std::fs::write(path, out + "\n").map_err(|e| e.to_string())
    }

    /// In-place save with a timestamped backup of the previous contents under
    /// config-kit's data dir (`<data_home>/pbedit/backups/`) — the fleet's
    /// XDG convention, so every app's state lands where the others' does.
    pub fn save_with_backup(&self) -> Result<PathBuf, String> {
        let backups = config_kit::data_dir("pbedit").join("backups");
        std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let name = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "data".into());
        let bpath = backups.join(format!("{name}.{stamp}.json"));
        if let Ok(prev) = std::fs::read(&self.path) {
            std::fs::write(&bpath, prev).map_err(|e| e.to_string())?;
        }
        self.save(None)?;
        Ok(bpath)
    }
}

/// One REPL step for the interactive shell front: execute a line against the
/// editor, return (output, keep_running). This is the UI seam — the terminal
/// shell drives it today; per UI_STRATEGY.md an egui front drives the same
/// function on a GUI host tomorrow.
pub fn shell_exec(ed: &mut Editor, line: &str) -> (String, bool) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        [] => (String::new(), true),
        ["quit"] | ["exit"] | ["q"] => ("folded.".into(), false),
        ["help"] | ["?"] => (
            "commands: list <factions|maps|parts|models> · get <path> · set <path> <value>\n             parts [name] · model <card> · validate · save [file] · help · quit"
                .into(),
            true,
        ),
        ["list", what] => (ed.list(what).join("\n"), true),
        // ── model vocabulary ────────────────────────────────────────────────
        // The editor edits FACTIONS and MAPS, but a designer writing a fold
        // recipe needs to know what parts exist and what a card currently
        // folds into — otherwise the only way to find out is to read
        // parts.json by hand and guess. Both answers come from the same
        // ModelBook the renderer uses, so the editor can never disagree with
        // what will actually be drawn.
        ["parts"] => match ed.model_book() {
            Ok(book) => {
                let mut names: Vec<String> = crate::origami::BUILTIN_PARTS
                    .iter().map(|s| format!("{s} (builtin)")).collect();
                names.extend(book.json_parts.keys().map(|k| format!("{k} (data)")));
                (format!("{} parts:\n{}", names.len(), names.join("\n")), true)
            }
            Err(e) => (format!("no model book: {e}"), true),
        },
        ["parts", name] => match ed.model_book() {
            Ok(book) => match book.json_parts.get(*name) {
                Some(spec) => (
                    format!("{name}: kind={} points={} raised={} pair={} thick={} tone×{:.2}",
                            spec.kind, spec.points.len(), spec.raised, spec.pair, spec.thick, spec.tone_mult),
                    true),
                None if crate::origami::BUILTIN_PARTS.contains(name) =>
                    (format!("{name}: builtin part (folded by the renderer, not data-defined)"), true),
                None => (format!("no part '{name}' — try `parts` for the list"), true),
            },
            Err(e) => (format!("no model book: {e}"), true),
        },
        ["model", card] => match ed.model_book() {
            Ok(book) => match book.models.get(*card) {
                Some(m) => {
                    let mut out = format!("{card}: {} parts, paper #{:02x}{:02x}{:02x}\n",
                        m.parts.len(), (m.paper[0] * 255.0) as u8, (m.paper[1] * 255.0) as u8,
                        (m.paper[2] * 255.0) as u8);
                    for p in &m.parts {
                        out.push_str(&format!("  {:<11} at ({:+.2},{:+.2}) size {:.2} h {:.2} tone {:.2}\n",
                            p.part, p.at.0, p.at.1, p.size, p.h, p.tone));
                    }
                    (out, true)
                }
                None => (format!("no model for '{card}'"), true),
            },
            Err(e) => (format!("no model book: {e}"), true),
        },
        ["get", path] => match ed.get(path) {
            Some(v) => (v.to_string(), true),
            None => (format!("nothing at '{path}'"), true),
        },
        ["set", path, val] => match ed.set(path, val) {
            Ok(old) => (
                format!(
                    "{path}: {} → {val}",
                    old.map(|o| o.to_string()).unwrap_or_else(|| "∅".into())
                ),
                true,
            ),
            Err(e) => (format!("refused: {e}"), true),
        },
        ["validate"] => {
            let f = ed.validate();
            if f.is_empty() {
                ("✅ clean".into(), true)
            } else {
                (f.join("\n"), true)
            }
        }
        ["save"] => match ed.save_with_backup() {
            Ok(b) => (format!("saved (backup at {})", b.display()), true),
            Err(e) => (format!("save failed: {e}"), true),
        },
        ["save", to] => match ed.save(Some(Path::new(to))) {
            Ok(()) => (format!("saved to {to}"), true),
            Err(e) => (format!("save failed: {e}"), true),
        },
        _ => (format!("unknown: '{line}' — try 'help'"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_shipped_data_validates_clean() {
        let ed = Editor::open(&game_dir().join("rts_data.json")).unwrap();
        let findings = ed.validate();
        assert!(findings.is_empty(), "shipped rts_data.json must be clean: {findings:?}");
        let fr = Editor::open(&game_dir().join("expansions/foldrealms.json")).unwrap();
        let findings = fr.validate();
        assert!(findings.is_empty(), "foldrealms must be clean: {findings:?}");
    }

    #[test]
    fn get_set_round_trip_with_domain_guards() {
        let mut ed = Editor::open(&game_dir().join("rts_data.json")).unwrap();
        let before = ed.get("court.units.soldier.dmg").unwrap().as_f64().unwrap();
        ed.set("court.units.soldier.dmg", "99").unwrap();
        assert_eq!(ed.get("court.units.soldier.dmg").unwrap().as_i64(), Some(99));
        ed.set("court.units.soldier.dmg", &before.to_string()).unwrap();
        // the projectile guard rejects nonsense but accepts real styles
        assert!(ed.set("court.projectile", "banana").is_err());
        ed.set("court.projectile", "quill").unwrap();
        // delivery guard
        assert!(ed.set("court.units.inkspeaker.ability.delivery", "postal").is_err());
    }

    #[test]
    fn the_validator_catches_the_dragonspire_class() {
        // reconstruct the EXACT shipped bug: dragon_legion's map without nodes
        let mut ed = Editor::open(&game_dir().join("expansions/dragon_legion.json")).unwrap();
        assert!(ed.validate().is_empty(), "fixed pack validates clean");
        if let Some(m) = ed
            .doc
            .get_mut("rts_maps")
            .and_then(|m| m.get_mut("dragonspire"))
            .and_then(|m| m.as_object_mut())
        {
            m.remove("nodes");
        }
        let findings = ed.validate();
        assert!(
            findings.iter().any(|f| f.contains("dragonspire") && f.contains("nodes")),
            "must flag the crash that actually shipped: {findings:?}"
        );
    }

    #[test]
    fn the_shell_front_drives_every_editor_verb() {
        let mut ed = Editor::open(&game_dir().join("rts_data.json")).unwrap();
        let (out, k) = shell_exec(&mut ed, "list factions");
        assert!(k && out.contains("court") && out.contains("press"));
        let (out, _) = shell_exec(&mut ed, "get court.units.soldier.hp");
        let hp = out.clone();
        let (out, _) = shell_exec(&mut ed, "set court.units.soldier.hp 555");
        assert!(out.contains("→ 555"));
        let (out, _) = shell_exec(&mut ed, "set court.projectile banana");
        assert!(out.contains("refused"), "domain guard speaks through the shell");
        let (out, _) = shell_exec(&mut ed, "validate");
        assert_eq!(out, "✅ clean");
        shell_exec(&mut ed, &format!("set court.units.soldier.hp {hp}"));
        let (out, keep) = shell_exec(&mut ed, "quit");
        assert!(!keep && out.contains("folded"));
        let (out, keep) = shell_exec(&mut ed, "frobnicate");
        assert!(keep && out.contains("unknown"));
    }

    #[test]
    fn backups_land_in_the_fleet_xdg_convention() {
        // sandbox XDG into target/ so the test owns its filesystem
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/xdg_test");
        std::fs::create_dir_all(&sandbox).unwrap();
        std::env::set_var("XDG_DATA_HOME", &sandbox);
        let tmp = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/backup_edit.json");
        std::fs::copy(game_dir().join("expansions/dragon_legion.json"), &tmp).unwrap();
        let mut ed = Editor::open(&tmp).unwrap();
        ed.set("rts_maps.dragonspire.nodes", "16").unwrap();
        let bpath = ed.save_with_backup().unwrap();
        assert!(bpath.starts_with(sandbox.join("pbedit/backups")), "{bpath:?}");
        assert!(bpath.exists());
        // the backup holds the OLD value; the file holds the new
        let old: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&bpath).unwrap()).unwrap();
        assert_eq!(old["rts_maps"]["dragonspire"]["nodes"], 14);
        let new: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&tmp).unwrap()).unwrap();
        assert_eq!(new["rts_maps"]["dragonspire"]["nodes"], 16);
        std::env::remove_var("XDG_DATA_HOME");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn save_round_trips_and_edits_persist() {
        let mut ed = Editor::open(&game_dir().join("rts_data.json")).unwrap();
        ed.set("ember.units.soldier.hp", "123").unwrap();
        let tmp = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/pbedit_test.json");
        ed.save(Some(&tmp)).unwrap();
        let re = Editor::open(&tmp).unwrap();
        assert_eq!(re.get("ember.units.soldier.hp").unwrap().as_i64(), Some(123));
        assert!(re.validate().is_empty());
        std::fs::remove_file(&tmp).ok();
    }
}
