//! `player_character` — the player's chosen avatar: a small, serializable
//! **profile** (body plan + seed + colour + name) that resolves into the same
//! [`CreatureBlueprint`](crate::creature_design::CreatureBlueprint) the engine
//! already renders, plus a **global default** persisted to the config dir so a
//! player's customized character loads across every world (r487, WID P1).
//!
//! Pure data + filesystem I/O only — no GPU, no windowing. The blueprint it
//! produces feeds the existing creature render path (preview tiles, the in-world
//! avatar, LAN remotes), so nothing new is needed downstream to draw it.

use serde::{Deserialize, Serialize};

use crate::creature_design::{Arche, CreatureBlueprint, EggSpec};

/// Stable string key for an [`Arche`] — used in the on-disk profile so saved
/// characters survive enum reordering (unlike a numeric discriminant).
pub fn arche_key(a: Arche) -> &'static str {
    match a {
        Arche::Biped => "biped",
        Arche::Quadruped => "quadruped",
        Arche::Critter => "critter",
        Arche::Serpent => "serpent",
        Arche::Heavy => "heavy",
        Arche::Bird => "bird",
        Arche::Canid => "canid",
        Arche::Lauscher => "lauscher",
        Arche::Dragon => "dragon",
        Arche::Dinosaur => "dinosaur",
    }
}

/// Parse an [`Arche`] from its [`arche_key`]; unknown keys fall back to `Biped`.
pub fn arche_from_key(s: &str) -> Arche {
    match s {
        "quadruped" => Arche::Quadruped,
        "critter" => Arche::Critter,
        "serpent" => Arche::Serpent,
        "heavy" => Arche::Heavy,
        "bird" => Arche::Bird,
        "canid" => Arche::Canid,
        "lauscher" => Arche::Lauscher,
        "dragon" => Arche::Dragon,
        "dinosaur" => Arche::Dinosaur,
        _ => Arche::Biped,
    }
}

/// Fixed arche ordering for compact wire encoding (LAN avatar appearance). Stable
/// — append only; never reorder, or saved/networked indices shift.
pub const ALL_ARCHES: [Arche; 10] = [
    Arche::Biped, Arche::Quadruped, Arche::Critter, Arche::Serpent, Arche::Heavy,
    Arche::Bird, Arche::Canid, Arche::Lauscher, Arche::Dragon, Arche::Dinosaur,
];

pub fn arche_to_u8(a: Arche) -> u8 {
    ALL_ARCHES.iter().position(|&x| x == a).unwrap_or(0) as u8
}

pub fn arche_from_u8(i: u8) -> Arche {
    *ALL_ARCHES.get(i as usize).unwrap_or(&Arche::Biped)
}

/// The arches a player may pick for their avatar (request-only plans like Dragon
/// are intentionally excluded from the roster but remain valid if hand-edited).
pub const SELECTABLE_ARCHES: [Arche; 7] = [
    Arche::Biped,
    Arche::Quadruped,
    Arche::Critter,
    Arche::Bird,
    Arche::Canid,
    Arche::Heavy,
    Arche::Serpent,
];

/// A player avatar: a body plan + a seed (fixes proportions/limbs) + a colour
/// r538: per-character BODY BUILD knobs, surfaced as sliders in the character
/// menu (muscularity, tallness, …). All default to 1.0 (the neutral build), and
/// the field is `#[serde(default)]` so older `character.json` blobs that predate
/// it still load. Consumed by [`crate::hero_forge`] for the voxel hero shape.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildParams {
    /// Overall stature in **blocks** (default 1.8, range 0.9..15.0). This is the
    /// master physical scalar — see `docs/STATURE.md` / `sim::stature`. It drives
    /// the model height *and* the derived collider/mass/health/locomotion.
    pub height: f32,
    /// Muscularity / bulk (scales torso + limb thickness, and mass/girth).
    pub bulk: f32,
    /// Head size.
    pub head: f32,
    /// Shoulder width (pauldron spread).
    pub shoulders: f32,
    /// Arm thickness.
    pub arm: f32,
    /// Leg length.
    pub leg: f32,
}

impl Default for BuildParams {
    fn default() -> Self {
        BuildParams { height: 1.8, bulk: 1.0, head: 1.0, shoulders: 1.0, arm: 1.0, leg: 1.0 }
    }
}

fn default_build() -> BuildParams {
    BuildParams::default()
}

/// tint + a display name. Serializes to a tiny JSON blob.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterProfile {
    pub name: String,
    /// Body-plan key (see [`arche_key`]).
    pub arche: String,
    /// Seed fixing all the seed-rolled genes (size, limbs, looseness, ...).
    pub seed: u64,
    /// Per-character colour multiplier applied over the blueprint's own colours.
    pub tint: [f32; 3],
    /// r538: body-build sliders (muscularity, tallness, …). Defaults to neutral.
    #[serde(default = "default_build")]
    pub build: BuildParams,
}

impl CharacterProfile {
    pub fn new(name: impl Into<String>, arche: Arche, seed: u64, tint: [f32; 3]) -> Self {
        CharacterProfile {
            name: name.into(),
            arche: arche_key(arche).to_string(),
            seed,
            tint,
            build: BuildParams::default(),
        }
    }

    pub fn arche(&self) -> Arche {
        arche_from_key(&self.arche)
    }

    /// The egg spec this profile resolves to (chosen body plan + seed).
    pub fn egg_spec(&self) -> EggSpec {
        EggSpec::chosen(self.arche(), self.seed)
    }

    /// Resolve to a renderable blueprint, with this character's colour tint
    /// pre-multiplied into the box colours (so previews match the in-world tint).
    pub fn blueprint(&self) -> CreatureBlueprint {
        let mut bp = self.egg_spec().hatch();
        let t = self.tint;
        for b in bp.boxes.iter_mut() {
            b.color = [b.color[0] * t[0], b.color[1] * t[1], b.color[2] * t[2]];
        }
        bp
    }

    /// Step to a different look while keeping the body plan + name + tint —
    /// the "reroll" affordance in the customizer.
    pub fn reroll(&mut self) {
        self.seed = self
            .seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0x1234_5678);
    }

    /// Set the body plan (customizer).
    pub fn set_arche(&mut self, a: Arche) {
        self.arche = arche_key(a).to_string();
    }

    /// Cycle to the next selectable body plan (customizer "Body ▸" button).
    pub fn cycle_arche(&mut self) {
        let cur = self.arche();
        let idx = SELECTABLE_ARCHES.iter().position(|&a| a == cur).unwrap_or(0);
        let next = SELECTABLE_ARCHES[(idx + 1) % SELECTABLE_ARCHES.len()];
        self.set_arche(next);
    }

    /// Set the colour tint (customizer swatch).
    pub fn set_tint(&mut self, t: [f32; 3]) {
        self.tint = t;
    }

    /// Compact appearance for the LAN wire: arche(1) + seed(8) + tint(3) = 12 bytes
    /// (the id is prepended by the caller). Lets remote players see each other's
    /// chosen character instead of a default avatar.
    pub fn to_appearance_bytes(&self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0] = arche_to_u8(self.arche());
        b[1..9].copy_from_slice(&self.seed.to_le_bytes());
        for k in 0..3 {
            b[9 + k] = (self.tint[k].clamp(0.0, 2.0) * 127.0) as u8;
        }
        b
    }

    pub fn from_appearance_bytes(b: &[u8]) -> Option<CharacterProfile> {
        if b.len() < 12 {
            return None;
        }
        let arche = arche_from_u8(b[0]);
        let mut s = [0u8; 8];
        s.copy_from_slice(&b[1..9]);
        let seed = u64::from_le_bytes(s);
        let tint = [b[9] as f32 / 127.0, b[10] as f32 / 127.0, b[11] as f32 / 127.0];
        Some(CharacterProfile::new("Remote", arche, seed, tint))
    }
}

/// Named colour tints the customizer offers as swatches.
pub const TINT_SWATCHES: [(&str, [f32; 3]); 6] = [
    ("Natural", [1.00, 0.96, 0.90]),
    ("Ember", [1.18, 0.84, 0.68]),
    ("Frost", [0.80, 0.92, 1.15]),
    ("Moss", [0.84, 1.06, 0.78]),
    ("Dusk", [0.96, 0.84, 1.08]),
    ("Shadow", [0.62, 0.64, 0.70]),
];

/// The starter roster shown in the character-select menu. Deterministic.
pub fn presets() -> Vec<CharacterProfile> {
    vec![
        CharacterProfile::new("Wanderer", Arche::Biped, 0x5EED_0001, [1.0, 0.96, 0.90]),
        CharacterProfile::new("Strider", Arche::Biped, 0x5EED_0002, [0.85, 0.92, 1.05]),
        CharacterProfile::new("Drift", Arche::Bird, 0x5EED_0003, [1.05, 1.0, 0.8]),
        CharacterProfile::new("Pad", Arche::Canid, 0x5EED_0004, [1.1, 0.95, 0.85]),
        CharacterProfile::new("Bulwark", Arche::Heavy, 0x5EED_0005, [0.9, 0.95, 1.0]),
        CharacterProfile::new("Whisker", Arche::Critter, 0x5EED_0006, [1.0, 1.05, 0.95]),
        // r534: these five are HUMANOID player character types (voxel "heroes"),
        // each tied to a bending element — NOT dragons. They render with the upright
        // biped model; their element drives bending separation (see the bend cast).
        CharacterProfile::new("Heliographer", Arche::Biped, 0x5EED_F1A1, [1.25, 1.02, 0.55]), // sun / Fire
        CharacterProfile::new("Windwalker", Arche::Biped, 0x5EED_F1A2, [0.92, 1.04, 1.06]),   // wind / Air
        CharacterProfile::new("Tidecaller", Arche::Biped, 0x5EED_F1A3, [0.55, 0.95, 1.20]),   // tide / Water
        CharacterProfile::new("Stormweaver", Arche::Biped, 0x5EED_F1A4, [0.80, 0.70, 1.15]),  // storm / Air
        CharacterProfile::new("Gravitybender", Arche::Biped, 0x5EED_F1A5, [0.92, 0.86, 1.02]), // gravity / Earth
    ]
}

/// r534: BENDING SEPARATION. The voxel "hero" character types each bend a fixed
/// element, independent of the surrounding environment. Returns the element a
/// character is attuned to, or `None` for ordinary characters (whose bending then
/// falls back to the environment-derived element). Matched case-insensitively on
/// the character name so custom/renamed profiles built on a hero still attune.
pub fn bend_element(name: &str) -> Option<crate::sim::gesture_resolve::Element> {
    use crate::sim::gesture_resolve::Element as E;
    let n = name.to_ascii_lowercase();
    if n.contains("helio") {
        Some(E::Fire) // heliographer — sun
    } else if n.contains("tide") {
        Some(E::Water) // tidecaller — tide
    } else if n.contains("wind") {
        Some(E::Air) // windwalker — wind
    } else if n.contains("storm") {
        Some(E::Air) // stormweaver — storm winds
    } else if n.contains("gravity") {
        Some(E::Earth) // gravitybender — gravity / stone
    } else {
        None
    }
}

impl Default for CharacterProfile {
    fn default() -> Self {
        presets().into_iter().next().unwrap_or_else(|| {
            CharacterProfile::new("Wanderer", Arche::Biped, 0x5EED_0001, [1.0, 1.0, 1.0])
        })
    }
}

// ── global default persistence (loads across worlds) ────────────────────────

/// `~/.config/unendlich/character.json` (or `$XDG_CONFIG_HOME/...`). `None` if
/// no config/home dir is discoverable (then the global default is in-memory only).
pub fn config_path() -> Option<std::path::PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .ok()
        .map(|p| p.join("unendlich").join("character.json"))
}

/// Load the player's saved global-default character, or the first preset if none
/// is saved (or it can't be read/parsed). Never fails — always returns a usable
/// profile so a world can always start.
pub fn load_global() -> CharacterProfile {
    if let Some(path) = config_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(profile) = serde_json::from_str::<CharacterProfile>(&text) {
                return profile;
            }
        }
    }
    CharacterProfile::default()
}

/// Persist `profile` as the global default (created across worlds). Best-effort:
/// returns the error if the dir/file can't be written.
pub fn save_global(profile: &CharacterProfile) -> std::io::Result<()> {
    let Some(path) = config_path() else {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no config dir"));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(profile)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, text)
}

/// Character-select state for the join menu: the preset roster followed by the
/// player's saved global default (the "custom" slot), with a cursor.
pub struct CharacterRoster {
    pub options: Vec<CharacterProfile>,
    /// Index of the player's editable custom slot within `options` (the last one).
    pub custom_index: usize,
    pub selected: usize,
}

impl CharacterRoster {
    /// Build the roster: presets + the saved global default as the custom slot,
    /// with the cursor starting on the custom slot (the player's own character).
    pub fn load() -> Self {
        let mut options = presets();
        let custom = load_global();
        let custom_index = options.len();
        options.push(custom);
        CharacterRoster { options, custom_index, selected: custom_index }
    }

    pub fn current(&self) -> &CharacterProfile {
        &self.options[self.selected.min(self.options.len() - 1)]
    }

    pub fn select(&mut self, i: usize) {
        if i < self.options.len() {
            self.selected = i;
        }
    }

    pub fn step(&mut self, delta: i32) {
        let n = self.options.len() as i32;
        self.selected = (((self.selected as i32 + delta) % n + n) % n) as usize;
    }

    /// Mutable access to the editable custom slot (for the customizer UI).
    pub fn custom_mut(&mut self) -> &mut CharacterProfile {
        &mut self.options[self.custom_index]
    }

    /// Persist the custom slot as the global default so it loads in future worlds.
    pub fn save_custom(&self) -> std::io::Result<()> {
        save_global(&self.options[self.custom_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arche_key_round_trips() {
        for a in [
            Arche::Biped, Arche::Quadruped, Arche::Critter, Arche::Serpent,
            Arche::Heavy, Arche::Bird, Arche::Canid, Arche::Lauscher,
            Arche::Dragon, Arche::Dinosaur,
        ] {
            assert_eq!(arche_from_key(arche_key(a)), a);
        }
    }

    #[test]
    fn profile_json_round_trips() {
        let p = CharacterProfile::new("Test", Arche::Canid, 12345, [1.0, 0.5, 0.25]);
        let text = serde_json::to_string(&p).unwrap();
        let back: CharacterProfile = serde_json::from_str(&text).unwrap();
        assert_eq!(p, back);
        assert_eq!(back.arche(), Arche::Canid);
    }

    #[test]
    fn profile_resolves_to_a_nonempty_tinted_blueprint() {
        let p = CharacterProfile::new("T", Arche::Biped, 7, [0.5, 0.5, 0.5]);
        let bp = p.blueprint();
        assert!(!bp.boxes.is_empty(), "a character must produce boxes");
        // tint halves every channel vs the untinted hatch
        let raw = p.egg_spec().hatch();
        assert!((bp.boxes[0].color[0] - raw.boxes[0].color[0] * 0.5).abs() < 1e-5);
    }

    #[test]
    fn presets_are_distinct_and_selectable() {
        let ps = presets();
        assert!(ps.len() >= 4);
        // distinct seeds
        for i in 1..ps.len() {
            assert_ne!(ps[i].seed, ps[0].seed);
        }
    }

    #[test]
    fn roster_defaults_to_custom_slot_and_steps() {
        let mut r = CharacterRoster::load();
        assert_eq!(r.selected, r.custom_index, "cursor starts on the player's own character");
        let start = r.selected;
        r.step(1);
        r.step(-1);
        assert_eq!(r.selected, start, "stepping wraps and returns");
        r.select(0);
        assert_eq!(r.current().name, presets()[0].name);
    }

    #[test]
    fn default_profile_is_a_preset() {
        assert_eq!(CharacterProfile::default(), presets()[0]);
    }

    #[test]
    fn cycle_arche_walks_the_selectable_set_and_wraps() {
        let mut p = CharacterProfile::new("T", SELECTABLE_ARCHES[0], 1, [1.0; 3]);
        for i in 0..SELECTABLE_ARCHES.len() {
            assert_eq!(p.arche(), SELECTABLE_ARCHES[i]);
            p.cycle_arche();
        }
        // wrapped back to the start
        assert_eq!(p.arche(), SELECTABLE_ARCHES[0]);
    }

    #[test]
    fn set_tint_recolors_the_blueprint() {
        let mut p = CharacterProfile::new("T", Arche::Biped, 3, [1.0; 3]);
        let bright = p.blueprint().boxes[0].color;
        p.set_tint([0.25, 0.25, 0.25]);
        let dim = p.blueprint().boxes[0].color;
        assert!(dim[0] < bright[0], "darker tint dims the silhouette");
    }

    #[test]
    fn arche_u8_round_trips_over_all() {
        for a in ALL_ARCHES {
            assert_eq!(arche_from_u8(arche_to_u8(a)), a);
        }
    }

    #[test]
    fn appearance_wire_round_trips() {
        let p = CharacterProfile::new("Me", Arche::Bird, 0xABCD_1234, [1.0, 0.5, 1.5]);
        let bytes = p.to_appearance_bytes();
        let back = CharacterProfile::from_appearance_bytes(&bytes).unwrap();
        assert_eq!(back.arche(), Arche::Bird);
        assert_eq!(back.seed, 0xABCD_1234);
        // tint quantized to ~1/127; within tolerance
        for k in 0..3 {
            assert!((back.tint[k] - p.tint[k]).abs() < 0.02, "tint channel {k}");
        }
    }
}
