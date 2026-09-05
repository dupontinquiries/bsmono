//! Mood and population: the parts of a request that are not blocks.
//!
//! "Build an eerie forest with horrifying creatures." Three things are
//! being asked for and only one of them is voxels:
//!
//! * a FOREST — trees, undergrowth, ground: the paintbrush's job
//! * EERIE — palette, light, fog, decay, time of day: mostly the
//!   engine's atmosphere, partly a detail pass
//! * HORRIFYING CREATURES — the engine's fauna roster, spawned with a
//!   disposition
//!
//! A model asked to produce all three as IR will produce the first,
//! fake the second with dark blocks, and either ignore the third or
//! hallucinate a `spawn` verb that does not exist. So the request is
//! DECOMPOSED. Moods are a dictionary the model names by adjective;
//! population is a request the engine fulfils from its own roster; and
//! both are emitted as lines the model can write, because a line it can
//! write is a line it will write.
//!
//! The naming ("NPCs named Alex") is carried through untouched. The
//! engine decides what a name means for a creature; this crate only
//! makes sure it arrives.

/// What the engine is asked to spawn. This crate has no creatures and
/// does not pretend to; the engine maps `kind` and `disposition` onto
/// its own roster.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnRequest {
    /// Free text from the player: "wolves", "horrifying creatures",
    /// "a dragon". The engine matches it against its roster; an unknown
    /// kind falls back on disposition.
    pub kind: String,
    /// How it should behave toward the player, when the kind is vague.
    pub disposition: Disposition,
    pub count: u32,
    /// Where, as a footprint at ground level.
    pub min: (i32, i32),
    pub max: (i32, i32),
    /// Optional name, applied to each if there is one, or to the first.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition { Passive, Neutral, Hostile, Boss }

impl Disposition {
    /// Read a disposition from the adjectives around a creature word.
    /// "Horrifying", "deadly", "monstrous" are hostile; "friendly",
    /// "tame" passive; a single named thing that is large is a boss.
    pub fn infer(text: &str) -> Self {
        let t = text.to_lowercase();
        let any = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
        if any(&["dragon", "boss", "titan", "colossus", "ancient one", "king", "queen"]) {
            Disposition::Boss
        } else if any(&["horrif", "terrif", "deadly", "monst", "hostile", "vicious",
                        "evil", "nightmare", "savage", "feral", "undead", "zombie"]) {
            Disposition::Hostile
        } else if any(&["friendly", "tame", "peaceful", "gentle", "harmless", "pet", "villager",
                        "npc", "merchant", "farmer"]) {
            Disposition::Passive
        } else {
            Disposition::Neutral
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Disposition::Passive => "passive", Disposition::Neutral => "neutral",
            Disposition::Hostile => "hostile", Disposition::Boss => "boss",
        }
    }
}

/// A mood: the non-voxel half of an adjective, plus the voxel half.
#[derive(Debug, Clone, PartialEq)]
pub struct Mood {
    pub name: &'static str,
    /// Palette the build should use.
    pub palette: &'static str,
    /// Weathering preset, or None for pristine.
    pub weather: Option<&'static str>,
    /// Ground clutter to scatter: (block, density 0..1).
    pub scatter: &'static [(u16, f32)],
    /// Tree species that fits.
    pub tree: &'static str,
    /// Atmosphere the engine is asked for.
    pub atmosphere: Atmosphere,
}

/// What the engine's atmosphere system is asked to do. Every field is
/// a request; the renderer owns these and this crate cannot set them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Atmosphere {
    /// 0.0 = clear, 1.0 = cannot see the far wall.
    pub fog: f32,
    /// Ambient light multiplier. Below 1.0 darkens.
    pub light: f32,
    /// Time of day to set, in fraction of a day (0.0 dawn, 0.5 dusk,
    /// 0.75 midnight), or None to leave alone.
    pub time: Option<f32>,
    /// Temperature nudge, same units as `godmode`.
    pub warmth: f32,
    /// Tint the fog/sky toward a colour, RGB 0..1, or None.
    pub tint: Option<[f32; 3]>,
}

const CLEAR: Atmosphere = Atmosphere { fog: 0.0, light: 1.0, time: None, warmth: 0.0, tint: None };

/// The mood dictionary. Adjectives a player uses, mapped to everything
/// each one implies. Generated into the prompt so the model learns the
/// list without anyone editing prompt text.
pub const MOODS: &[Mood] = &[
    Mood { name: "eerie", palette: "darkwood", weather: Some("abandoned"),
           scatter: &[(34, 0.15), (28, 0.2), (33, 0.1)],   // cobweb, dead bush, mushroom
           tree: "oak",
           atmosphere: Atmosphere { fog: 0.6, light: 0.45, time: Some(0.7), warmth: -0.1,
                                    tint: Some([0.35, 0.4, 0.45]) } },
    Mood { name: "haunted", palette: "ruined", weather: Some("ancient"),
           scatter: &[(34, 0.25), (45, 0.08)],             // cobweb, bone
           tree: "oak",
           atmosphere: Atmosphere { fog: 0.7, light: 0.35, time: Some(0.75), warmth: -0.15,
                                    tint: Some([0.3, 0.3, 0.4]) } },
    Mood { name: "cosy", palette: "oak", weather: None,
           scatter: &[(25, 0.2)],                          // flowers
           tree: "birch",
           atmosphere: Atmosphere { fog: 0.05, light: 1.1, time: Some(0.45), warmth: 0.1,
                                    tint: Some([1.0, 0.9, 0.75]) } },
    Mood { name: "fiery", palette: "volcanic", weather: Some("burnt"),
           scatter: &[(96, 0.15), (38, 0.1)],              // magma, basalt
           tree: "pine",
           atmosphere: Atmosphere { fog: 0.3, light: 0.8, time: None, warmth: 0.35,
                                    tint: Some([1.0, 0.5, 0.2]) } },
    Mood { name: "frozen", palette: "frozen", weather: None,
           scatter: &[(32, 0.3), (31, 0.05)],              // snow, ice
           tree: "pine",
           atmosphere: Atmosphere { fog: 0.25, light: 1.0, time: None, warmth: -0.35,
                                    tint: Some([0.8, 0.9, 1.0]) } },
    Mood { name: "ancient", palette: "ruined", weather: Some("ancient"),
           scatter: &[(23, 0.3), (29, 0.15)],              // moss, vines
           tree: "jungle",
           atmosphere: Atmosphere { fog: 0.2, light: 0.9, time: None, warmth: 0.0, tint: None } },
    Mood { name: "sacred", palette: "polished", weather: None,
           scatter: &[(97, 0.05)],                         // glow crystal
           tree: "birch",
           atmosphere: Atmosphere { fog: 0.15, light: 1.15, time: Some(0.3), warmth: 0.0,
                                    tint: Some([0.95, 0.95, 1.0]) } },
    Mood { name: "lush", palette: "jungle", weather: None,
           scatter: &[(25, 0.3), (27, 0.3), (23, 0.2)],    // flower, fern, moss
           tree: "jungle",
           atmosphere: Atmosphere { fog: 0.15, light: 1.0, time: None, warmth: 0.15, tint: None } },
];

/// Words that mean each mood, beyond its own name. The model may use
/// the name; the player will use any of these.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("eerie", &["eerie", "spooky", "creepy", "unsettling", "ominous", "gloomy", "dark forest"]),
    ("haunted", &["haunted", "cursed", "ghost", "graveyard", "undead", "horrifying", "nightmare"]),
    ("cosy", &["cosy", "cozy", "warm and", "homely", "snug", "welcoming", "quaint"]),
    ("fiery", &["fiery", "fire", "lava", "volcanic", "burning", "infernal", "dragon", "hellish"]),
    ("frozen", &["frozen", "icy", "arctic", "glacial", "wintry", "snowbound"]),
    ("ancient", &["ancient", "overgrown", "forgotten", "lost temple", "ruins of"]),
    ("sacred", &["sacred", "holy", "temple", "shrine", "cathedral", "divine"]),
    ("lush", &["lush", "verdant", "tropical", "jungle", "overgrown garden"]),
];

pub fn lookup(name: &str) -> Option<&'static Mood> {
    MOODS.iter().find(|m| m.name.eq_ignore_ascii_case(name.trim()))
}

/// Which moods a phrase invokes, strongest first. "An eerie forest with
/// horrifying creatures" is eerie AND haunted; both apply and the
/// atmosphere is blended.
pub fn recognize(prose: &str) -> Vec<&'static Mood> {
    let p = prose.to_lowercase();
    let mut out: Vec<&'static Mood> = Vec::new();
    for (name, words) in SYNONYMS {
        if words.iter().any(|w| p.contains(w)) {
            if let Some(m) = lookup(name) {
                if !out.iter().any(|o| o.name == m.name) { out.push(m); }
            }
        }
    }
    out
}

/// Blend several moods' atmospheres: fog and darkness take the max
/// (eerie plus haunted is not less foggy), warmth sums and clamps, the
/// first mood's time and tint win.
pub fn blend(moods: &[&Mood]) -> Atmosphere {
    let mut a = CLEAR;
    for (i, m) in moods.iter().enumerate() {
        let at = m.atmosphere;
        a.fog = a.fog.max(at.fog);
        a.light = if i == 0 { at.light } else { a.light.min(at.light) };
        a.warmth = (a.warmth + at.warmth).clamp(-0.35, 0.35);
        if i == 0 { a.time = at.time; a.tint = at.tint; }
    }
    a
}

/// Find a name in a phrase: "NPCs named Alex", "a dragon called Smaug".
pub fn named(prose: &str) -> Option<String> {
    let p = prose;
    for marker in [" named ", " called ", " name is "] {
        if let Some(i) = p.to_lowercase().find(marker) {
            let after = &p[i + marker.len()..];
            let name: String = after.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '-'))
                .next().unwrap_or("").to_string();
            if !name.is_empty() { return Some(name); }
        }
    }
    None
}

/// Pull creature requests out of a phrase. "with horrifying creatures",
/// "with wolves", "NPCs named Alex", "a fire dragon". Returns what to
/// spawn; the footprint is filled in by the caller from the build.
pub fn creatures(prose: &str) -> Vec<(String, Disposition, u32, Option<String>)> {
    let p = prose.to_lowercase();
    let mut out = Vec::new();
    // Creature nouns, with a default count for plural vs singular.
    const NOUNS: &[(&str, u32)] = &[
        ("creatures", 6), ("monsters", 5), ("beasts", 4), ("animals", 6),
        ("wolves", 4), ("bears", 2), ("deer", 5), ("rabbits", 6), ("ravens", 4),
        ("trolls", 2), ("harpies", 3), ("basilisks", 1), ("crabs", 5), ("leeches", 6),
        ("npcs", 4), ("villagers", 6), ("guards", 4), ("merchants", 2), ("people", 6),
        ("dragon", 1), ("troll", 1), ("wolf", 1), ("bear", 1), ("npc", 1), ("villager", 1),
        ("guard", 1), ("boss", 1),
    ];
    let name = named(prose);
    for (noun, count) in NOUNS {
        if let Some(i) = p.find(noun) {
            // the words just before the noun carry the disposition
            let start = i.saturating_sub(40);
            let ctx = &p[start..(i + noun.len()).min(p.len())];
            let disp = Disposition::infer(ctx);
            // "a dragon" is one; "dragons" would be more
            out.push((noun.to_string(), disp, *count, name.clone()));
            break;   // one creature request per phrase; the model can add more
        }
    }
    out
}

/// The prompt lines, generated from the tables so they cannot drift.
pub fn vocabulary() -> String {
    let moods: Vec<&str> = MOODS.iter().map(|m| m.name).collect();
    format!("MOODS (use `mood NAME`, it sets palette, decay, clutter and atmosphere): {}\n\
             CREATURES (use `populate KIND [count=N] [name=X]`; the engine picks from its own \
             roster by disposition): passive | neutral | hostile | boss",
            moods.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eerie_forest_with_horrifying_creatures_decomposes() {
        let prose = "build an eerie forest with horrifying creatures";
        let moods = recognize(prose);
        assert!(moods.iter().any(|m| m.name == "eerie"));
        assert!(moods.iter().any(|m| m.name == "haunted"), "horrifying -> haunted too");
        let c = creatures(prose);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, "creatures");
        assert_eq!(c[0].1, Disposition::Hostile, "horrifying is hostile");
        assert!(c[0].2 > 1, "plural means several");
        let a = blend(&moods);
        assert!(a.fog >= 0.6 && a.light < 0.5, "and it is foggy and dark: {a:?}");
    }

    #[test]
    fn npcs_named_alex_carry_the_name() {
        let prose = "dig an underground city with NPCs named Alex";
        let c = creatures(prose);
        assert_eq!(c[0].0, "npcs");
        assert_eq!(c[0].1, Disposition::Passive, "an NPC is not a threat");
        assert_eq!(c[0].3.as_deref(), Some("Alex"), "the name survives, case intact");
    }

    #[test]
    fn a_fire_dragon_nest_is_fiery_with_a_boss() {
        let prose = "build a fire dragon nest";
        let moods = recognize(prose);
        assert_eq!(moods[0].name, "fiery");
        assert_eq!(moods[0].palette, "volcanic");
        let c = creatures(prose);
        assert_eq!(c[0].0, "dragon");
        assert_eq!(c[0].1, Disposition::Boss);
        assert_eq!(c[0].2, 1, "a dragon, singular");
        assert!(blend(&moods).warmth > 0.3, "and the nest is hot");
    }

    #[test]
    fn a_plain_build_has_no_mood_and_no_creatures() {
        assert!(recognize("a small stone hut").is_empty());
        assert!(creatures("a small stone hut").is_empty());
        assert!(named("a small stone hut").is_none());
    }

    #[test]
    fn names_are_found_after_the_usual_markers() {
        assert_eq!(named("a wolf called Fang").as_deref(), Some("Fang"));
        assert_eq!(named("guards named O'Brien on the wall").as_deref(), Some("O'Brien"));
        assert_eq!(named("a dragon whose name is Vex, red").as_deref(), Some("Vex"));
    }

    #[test]
    fn disposition_reads_the_adjectives() {
        assert_eq!(Disposition::infer("friendly villagers"), Disposition::Passive);
        assert_eq!(Disposition::infer("vicious wolves"), Disposition::Hostile);
        assert_eq!(Disposition::infer("some deer"), Disposition::Neutral);
        assert_eq!(Disposition::infer("an ancient dragon"), Disposition::Boss);
    }

    #[test]
    fn blending_never_makes_a_dark_mood_brighter() {
        let e = lookup("eerie").unwrap();
        let c = lookup("cosy").unwrap();
        let a = blend(&[e, c]);
        assert!(a.light <= e.atmosphere.light, "cosy cannot lift eerie: {a:?}");
        assert!(a.fog >= e.atmosphere.fog);
    }

    #[test]
    fn every_mood_names_a_real_palette_and_species() {
        let pals = crate::palettes();
        for m in MOODS {
            assert!(pals.contains_key(m.palette), "mood '{}' names unknown palette '{}'",
                    m.name, m.palette);
            assert!(["oak", "birch", "pine", "cactus", "jungle"].contains(&m.tree),
                    "mood '{}' names unknown species '{}'", m.name, m.tree);
            if let Some(w) = m.weather {
                let _ = crate::weather::Weather::preset(w);   // must not panic
            }
        }
        let v = vocabulary();
        for m in MOODS { assert!(v.contains(m.name)); }
    }
}
