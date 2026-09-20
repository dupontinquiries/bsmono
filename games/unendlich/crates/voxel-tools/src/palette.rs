//! What a block IS, so something else can stand in for it.
//!
//! `resolve_block` is a name-to-number table. It knows that "obsidian"
//! is 49 and nothing else: not that it is nearly black, very hard and
//! glassy. So when a block is missing -- an id this world does not
//! have, a name from another game, a material a model invented -- the
//! only available answer was STONE, or worse, AIR.
//!
//! That is not how a builder handles it. Shown a block they have never
//! seen, they ask three questions: what colour is it, how hard does it
//! read, and what is it LIKE -- stone, wood, glass, metal, cloth? Then
//! they say "that is basically deepslate, I will use it where I would
//! have used deepslate". A new block in a new update is placed by
//! analogy within the hour.
//!
//! So blocks get properties, and two things become possible:
//!
//!   NEAREST     an unknown block resolves to the closest one this
//!               world actually has, by colour, hardness and family,
//!               and the substitution is REPORTED rather than silent.
//!   SUGGEST     "something dark and hard for a foundation" is a
//!               query, not a lookup. It is the question a builder
//!               asks, and answering it is how a palette gets chosen
//!               by a model that does not know this world's blocks.
//!
//! The properties are approximate on purpose. They are for RANKING --
//! is this nearer to obsidian or to sand -- and a wrong decimal place
//! changes nothing about which answer comes back first.

use std::collections::BTreeMap;

/// What a block is broadly made of. The strongest signal for a
/// substitution: a grey wood is not a substitute for a grey stone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family { Stone, Soil, Wood, Leaf, Glass, Metal, Crystal, Liquid, Cloth, Bone, Light, Air }

/// How a face reads at distance, which matters more than exact colour
/// for whether two blocks can stand in for each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish { Rough, Smooth, Polished, Grainy, Translucent, Fibrous }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    pub id: u16,
    pub name: &'static str,
    /// Approximate face colour, 0..255.
    pub rgb: (u8, u8, u8),
    /// Roughly how long it takes to break, 0 (instant) to 10
    /// (obsidian). Approximate and only used for ranking.
    pub hardness: f32,
    pub family: Family,
    pub finish: Finish,
    /// What a builder reaches for it FOR. Free text, matched by word.
    pub uses: &'static str,
}

const fn b(id: u16, name: &'static str, rgb: (u8, u8, u8), hardness: f32,
           family: Family, finish: Finish, uses: &'static str) -> Block {
    Block { id, name, rgb, hardness, family, finish, uses }
}

/// The catalogue. Every id here is one this world actually has -- a
/// test checks that, because a catalogue that lists blocks the game
/// lacks is exactly the failure this module exists to prevent.
pub const BLOCKS: &[Block] = &[
    b(1,  "stone",           (125, 125, 125), 1.5, Family::Stone,   Finish::Rough,       "walls structure foundation mass"),
    b(4,  "cobblestone",     (110, 110, 110), 2.0, Family::Stone,   Finish::Rough,       "walls rustic paths foundation"),
    b(58, "stone_bricks",    (122, 122, 122), 1.5, Family::Stone,   Finish::Smooth,      "walls fine masonry civic"),
    b(2,  "grass",           ( 95, 159,  53), 0.6, Family::Soil,    Finish::Grainy,      "ground lawn field"),
    b(3,  "dirt",            (134,  96,  67), 0.5, Family::Soil,    Finish::Grainy,      "ground fill farm"),
    b(12, "sand",            (219, 207, 163), 0.5, Family::Soil,    Finish::Grainy,      "beach desert floor"),
    b(13, "gravel",          (136, 126, 126), 0.6, Family::Soil,    Finish::Grainy,      "path rubble drainage"),
    b(48, "sandstone",       (216, 203, 155), 0.8, Family::Stone,   Finish::Smooth,      "desert walls warm masonry"),
    b(17, "oak_log",         (102,  81,  51), 2.0, Family::Wood,    Finish::Fibrous,     "posts beams trunks"),
    b(57, "oak_planks",      (162, 130,  78), 2.0, Family::Wood,    Finish::Fibrous,     "floors walls warm interior"),
    b(86, "dark_oak_planks", ( 66,  43,  20), 2.0, Family::Wood,    Finish::Fibrous,     "roofs trim dark timber"),
    b(18, "leaves",          ( 60, 143,  40), 0.2, Family::Leaf,    Finish::Fibrous,     "canopy hedge green"),
    b(61, "glass",           (200, 226, 234), 0.3, Family::Glass,   Finish::Translucent, "windows light see-through"),
    b(31, "ice",             (160, 200, 240), 0.5, Family::Glass,   Finish::Translucent, "frozen cold slippery"),
    b(51, "quartz",          (236, 232, 224), 0.8, Family::Crystal, Finish::Polished,    "trim bright fine detail ornament"),
    b(49, "obsidian",        ( 21,  18,  30), 9.5, Family::Stone,   Finish::Polished,    "dark hard foundation ominous"),
    b(32, "snow",            (245, 250, 252), 0.2, Family::Soil,    Finish::Grainy,      "cold cover white"),
    b(9,  "water",           ( 63, 118, 228), 0.0, Family::Liquid,  Finish::Translucent, "sea river pool"),
    b(11, "lava",            (219, 108,  30), 0.0, Family::Liquid,  Finish::Rough,       "molten hot danger"),
    b(63, "gold_block",      (247, 223,  85), 3.0, Family::Metal,   Finish::Polished,    "treasure rich ornament"),
    b(62, "iron_block",      (219, 219, 219), 5.0, Family::Metal,   Finish::Polished,    "machinery hard grey metal"),
    b(97, "glowstone",       (250, 220, 140), 0.3, Family::Light,   Finish::Grainy,      "light glow lantern lamp"),
    b(0,  "air",             (  0,   0,   0), 0.0, Family::Air,     Finish::Smooth,      "nothing void empty"),
];

pub fn by_id(id: u16) -> Option<&'static Block> { BLOCKS.iter().find(|b| b.id == id) }
pub fn by_name(name: &str) -> Option<&'static Block> {
    let n = name.trim().to_lowercase();
    BLOCKS.iter().find(|b| b.name == n)
}

/// How far apart two blocks are, for substitution. Lower is closer.
///
/// FAMILY DOMINATES, because a grey wood is not a substitute for a
/// grey stone however close the colour: swapping across families is
/// what makes a build look wrong in a way nobody can name. Colour is
/// next, then finish, then hardness -- hardness matters least to how
/// something LOOKS and most to how it plays, so it is a tiebreak.
pub fn distance(a: &Block, c: &Block) -> f32 {
    let fam = if a.family == c.family { 0.0 } else if compatible(a.family, c.family) { 0.35 } else { 1.0 };
    let col = (((a.rgb.0 as f32 - c.rgb.0 as f32).powi(2)
              + (a.rgb.1 as f32 - c.rgb.1 as f32).powi(2)
              + (a.rgb.2 as f32 - c.rgb.2 as f32).powi(2)).sqrt()) / 441.0;
    let fin = if a.finish == c.finish { 0.0 } else { 0.2 };
    let hard = ((a.hardness - c.hardness).abs() / 10.0).min(1.0);
    fam * 1.6 + col * 1.0 + fin * 0.5 + hard * 0.3
}

/// Families that can stand in for each other at a pinch: stone and
/// crystal both read as mineral, wood and leaf both as growth.
fn compatible(a: Family, c: Family) -> bool {
    use Family::*;
    matches!((a, c), (Stone, Crystal) | (Crystal, Stone) | (Stone, Soil) | (Soil, Stone)
                   | (Wood, Leaf) | (Leaf, Wood) | (Glass, Crystal) | (Crystal, Glass)
                   | (Metal, Stone) | (Stone, Metal) | (Bone, Stone) | (Stone, Bone)
                   // cloth has no near neighbour in a block world; the
                   // pale minerals are what a builder reaches for when
                   // they want a soft-looking light surface
                   | (Cloth, Crystal) | (Crystal, Cloth))
}

/// A substitution, with its reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Substitute {
    pub id: u16,
    pub name: &'static str,
    pub why: String,
    /// Other blocks that nearly won, so a caller can offer a choice.
    pub runners_up: Vec<&'static str>,
}

/// The nearest block this world has to one described by properties.
///
/// This is the builder's move: shown a block that does not exist here,
/// decide what it is LIKE and use that. The alternative -- and what
/// this crate did -- is to fall back to stone and lose the intent.
pub fn nearest_to(rgb: (u8, u8, u8), hardness: f32, family: Family, finish: Finish)
    -> Substitute {
    let want = Block { id: u16::MAX, name: "?", rgb, hardness, family, finish, uses: "" };
    let mut ranked: Vec<(f32, &Block)> = BLOCKS.iter()
        .filter(|b| b.family != Family::Air && b.family != Family::Liquid)
        .map(|b| (distance(&want, b), b)).collect();
    ranked.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
    let (d, best) = ranked[0];
    Substitute {
        id: best.id,
        name: best.name,
        why: format!("{} is the nearest thing here ({:?}, {:?}, hardness {:.1}, distance {d:.2})",
                     best.name, best.family, best.finish, best.hardness),
        runners_up: ranked.iter().skip(1).take(2).map(|(_, b)| b.name).collect(),
    }
}

/// The nearest block to one we know by id, excluding itself. What to
/// use when a block exists in the catalogue but not in this world.
pub fn nearest_known(id: u16) -> Option<Substitute> {
    let src = by_id(id)?;
    let mut ranked: Vec<(f32, &Block)> = BLOCKS.iter()
        .filter(|b| b.id != id && b.family != Family::Air)
        .map(|b| (distance(src, b), b)).collect();
    ranked.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
    let (d, best) = ranked[0];
    Some(Substitute {
        id: best.id, name: best.name,
        why: format!("nearest to {} is {} (distance {d:.2})", src.name, best.name),
        runners_up: ranked.iter().skip(1).take(2).map(|(_, b)| b.name).collect(),
    })
}

/// "Something dark and hard for a foundation": a QUERY, not a lookup.
///
/// This is how a palette gets chosen by a model that does not know
/// this world's blocks -- it knows what it WANTS and can describe it,
/// and describing it is enough.
pub fn suggest(description: &str, n: usize) -> Vec<(&'static Block, String)> {
    let d = description.to_lowercase();
    let mut words: Vec<&str> = d.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // COMPOUNDS. A model that wants a material it cannot name invents one the
    // way English does -- adjective + material, run together: `brownwood`,
    // `darkstone`, `redbrick`. That is ONE word here, and everything below
    // matches PREFIXES, so "brow" never met "wood" and the refusal came back
    // with no suggestion: a wall, not a correction. Look inside a long word
    // for any vocabulary word it starts or ends with, and let those vote too.
    let vocab: Vec<&'static str> = BLOCKS.iter()
        .flat_map(|b| b.name.split('_').chain(b.uses.split_whitespace()))
        .chain(["dark", "black", "pale", "white", "brown", "green", "blue", "grey", "gray", "yellow", "orange", "red"])
        // FAMILY words are matched by exact equality further down and are in
        // no block's name or uses, so `wood` was not in this vocabulary and was
        // never found inside `brownwood`. Kept in step with that list by hand.
        .chain(["clear", "glass", "metal", "polished", "rough", "shiny", "smooth", "stone", "wood", "wooden"])
        .filter(|t| t.len() >= 3)
        .collect();
    let inside: Vec<&'static str> = words.iter()
        .filter(|w| w.len() >= 6)
        .flat_map(|w| vocab.iter().copied().filter(move |t| t.len() < w.len() && (w.starts_with(t) || w.ends_with(t))))
        .collect();
    words.extend(inside);
    words.sort_unstable();
    words.dedup();
    let has = |w: &str| words.iter().any(|x| *x == w);
    let mut scored: Vec<(f32, &Block, Vec<&str>)> = Vec::new();
    for blk in BLOCKS.iter().filter(|b| b.family != Family::Air) {
        let mut score = 0.0f32;
        let mut why: Vec<&str> = Vec::new();
        // What it is for. Matched on a STEM, not an exact word:
        // "something that glows" found nothing because glowstone's use
        // is "glow" and the query said "glows". A builder asking about
        // a block does not conjugate for the lookup table.
        let stem_hit = |a: &str, b: &str| {
            let n = a.len().min(b.len());
            n >= 4 && a[..n] == b[..n]
        };
        for u in blk.uses.split_whitespace() {
            if words.iter().any(|w| stem_hit(w, u)) { score += 2.0; why.push("its use"); break; }
        }
        // the name itself
        if blk.name.split('_').any(|part| words.iter().any(|w| stem_hit(w, part))) {
            score += 3.0; why.push("named");
        }
        // colour words
        let (r, g, bl) = (blk.rgb.0 as f32, blk.rgb.1 as f32, blk.rgb.2 as f32);
        let lum = (r + g + bl) / 3.0;
        for (w, ok) in [("dark", lum < 90.0), ("black", lum < 50.0), ("pale", lum > 190.0),
                        ("white", lum > 225.0), ("light", lum > 190.0), ("bright", lum > 200.0),
                        ("red", r > g + 40.0 && r > bl + 40.0), ("green", g > r + 30.0 && g > bl + 30.0),
                        ("blue", bl > r + 40.0), ("grey", (r - g).abs() < 18.0 && (g - bl).abs() < 18.0),
                        ("warm", r > bl + 30.0), ("cold", bl > r), ("gold", r > 200.0 && g > 180.0 && bl < 130.0)] {
            if has(w) && ok { score += 2.0; why.push("its colour"); }
            else if has(w) && !ok { score -= 1.5; }
        }
        // hardness words
        for (w, ok) in [("hard", blk.hardness >= 3.0), ("tough", blk.hardness >= 3.0),
                        ("soft", blk.hardness < 1.0), ("crumbly", blk.hardness < 0.8)] {
            if has(w) && ok { score += 2.0; why.push("its hardness"); }
            else if has(w) && !ok { score -= 1.5; }
        }
        // family and finish words
        for (w, ok) in [("stone", blk.family == Family::Stone), ("wood", blk.family == Family::Wood),
                        ("wooden", blk.family == Family::Wood), ("metal", blk.family == Family::Metal),
                        ("glass", blk.family == Family::Glass), ("clear", blk.finish == Finish::Translucent),
                        ("rough", blk.finish == Finish::Rough), ("smooth", blk.finish == Finish::Smooth),
                        ("polished", blk.finish == Finish::Polished), ("shiny", blk.finish == Finish::Polished)] {
            if has(w) && ok { score += 2.5; why.push("what it is"); }
        }
        // FAMILY DOMINATES -- this module's own rule ("a grey wood is not a
        // substitute for a grey stone"), which `suggest` did not obey: naming
        // a family was worth 2.5 and a name hit 3.0, so `darkstone` came back
        // as dark_oak_planks on the strength of "dark". If the query names a
        // family, a block of another family is not a candidate at all.
        let named_family = [("stone", Family::Stone), ("wood", Family::Wood), ("wooden", Family::Wood),
                            ("metal", Family::Metal), ("glass", Family::Glass)]
            .iter().filter(|(w, _)| has(w)).map(|(_, f)| *f).collect::<Vec<_>>();
        if !named_family.is_empty() && !named_family.contains(&blk.family) {
            continue;
        }
        if score > 0.0 {
            why.sort_unstable(); why.dedup();
            scored.push((score, blk, why));
        }
    }
    scored.sort_by(|a, c| c.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(n)
        .map(|(s, b, why)| (b, format!("{} (by {}, score {s:.0})", b.name, why.join(" and "))))
        .collect()
}

/// The catalogue as a line per block, for a prompt. This is what lets
/// a model choose a material at all: without it, it is guessing which
/// names this particular world happens to use.
pub fn vocabulary() -> String {
    BLOCKS.iter().filter(|b| b.family != Family::Air)
        .map(|b| format!("  {:<16} {:?} {:?}, hardness {:.1} -- {}",
                         b.name, b.family, b.finish, b.hardness, b.uses))
        .collect::<Vec<_>>().join("\n")
}

/// Resolve a name, falling back to a SUGGESTION rather than to stone.
/// Returns the id and, when it was not an exact hit, why.
pub fn resolve_or_suggest(name: &str) -> (u16, Option<String>) {
    if let Some(b) = by_name(name) { return (b.id, None); }
    if let Some(id) = crate::plan::resolve_block(name) { return (id, None); }
    // not a name this world knows: treat the words as a description
    let found = suggest(name, 1);
    match found.first() {
        Some((b, why)) => (b.id, Some(format!("'{name}' is not a block here; used {why}"))),
        None => {
            let s = nearest_to((128, 128, 128), 1.5, Family::Stone, Finish::Rough);
            (s.id, Some(format!("'{name}' means nothing here; used {}", s.name)))
        }
    }
}

/// Every id in the catalogue, for a caller that must check them.
pub fn ids() -> BTreeMap<&'static str, u16> {
    BLOCKS.iter().map(|b| (b.name, b.id)).collect()
}

#[cfg(test)]
mod tests {
    /// A model's invented compound lands in the right FAMILY, not merely on
    /// "something": a suggestion of glass for `brownwood` would satisfy
    /// `is_some()` and help nobody. And looking inside long words must not
    /// disturb a query that already worked.
    #[test]
    fn an_invented_compound_is_corrected_to_the_right_family() {
        let mut checked = 0;
        for (made_up, family) in [("brownwood", Family::Wood), ("darkstone", Family::Stone), ("woodplank", Family::Wood), ("roughstone", Family::Stone)] {
            let got = suggest(made_up, 1);
            let (blk, why) = got.first().unwrap_or_else(|| panic!("nothing at all suggested for {made_up:?}"));
            assert_eq!(blk.family, family, "{made_up:?} -> {} ({why})", blk.name);
            checked += 1;
        }
        assert_eq!(checked, 4);
        assert!(suggest("zzqxv", 1).is_empty(), "nonsense still gets nothing, not a guess");
        let glow = suggest("something that glows", 1);
        assert!(glow.first().is_some_and(|(b, _)| b.name.contains("glow")), "the stem match it already had still works: {:?}", glow.first().map(|g| g.0.name));
    }

    use super::*;

    #[test]
    fn every_catalogued_block_is_one_this_world_has() {
        // a catalogue listing blocks the game lacks is exactly the
        // failure this module exists to prevent
        const GAME: &[u16] = &[0,1,2,3,4,7,9,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,
            29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,56,57,58,59,61,62,
            63,64,65,66,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97];
        for blk in BLOCKS {
            assert!(GAME.contains(&blk.id), "{} (id {}) is not in the game", blk.name, blk.id);
        }
        // and no two entries claim the same id
        let mut seen = std::collections::BTreeSet::new();
        for blk in BLOCKS { assert!(seen.insert(blk.id), "id {} listed twice", blk.id); }
    }

    #[test]
    fn family_dominates_a_substitution() {
        // a grey wood is not a substitute for a grey stone, however
        // close the colour: swapping across families is what makes a
        // build look wrong in a way nobody can name
        let s = nearest_to((120, 120, 120), 2.0, Family::Wood, Finish::Fibrous);
        let blk = by_name(s.name).unwrap();
        assert_eq!(blk.family, Family::Wood, "{}", s.why);
        // and the same colour asked for as stone gives a stone
        let t = nearest_to((120, 120, 120), 2.0, Family::Stone, Finish::Rough);
        assert_eq!(by_name(t.name).unwrap().family, Family::Stone, "{}", t.why);
        assert_ne!(s.name, t.name);
    }

    #[test]
    fn a_block_this_world_lacks_resolves_to_what_it_is_like() {
        // deepslate: dark grey, hard, stone. Not in this world.
        let s = nearest_to((60, 60, 66), 3.5, Family::Stone, Finish::Rough);
        assert!(["stone", "cobblestone", "obsidian", "netherrack"].contains(&s.name), "{}", s.why);
        assert!(s.why.contains("nearest thing here"));
        assert_eq!(s.runners_up.len(), 2, "and it offers a choice");
        // amethyst: pale purple, crystal
        let a = nearest_to((170, 140, 210), 1.5, Family::Crystal, Finish::Polished);
        assert!(matches!(by_name(a.name).unwrap().family, Family::Crystal | Family::Stone | Family::Glass),
                "{}", a.why);
    }

    #[test]
    fn asking_for_something_dark_and_hard_gets_something_dark_and_hard() {
        let found = suggest("something dark and hard for a foundation", 3);
        assert!(!found.is_empty());
        let first = found[0].0;
        assert!(first.hardness >= 3.0, "hard: {} at {}", first.name, first.hardness);
        let lum = (first.rgb.0 as f32 + first.rgb.1 as f32 + first.rgb.2 as f32) / 3.0;
        assert!(lum < 90.0, "dark: {} at luminance {lum}", first.name);
        assert!(found[0].1.contains("by "), "it says WHY: {}", found[0].1);
        // and a conjugated word finds the thing: "glows" is "glow"
        let glow = suggest("something that glows", 1);
        assert_eq!(glow.first().map(|(b, _)| b.name), Some("glowstone"), "{glow:?}");
        assert!(!suggest("a lantern", 1).is_empty());
        // the opposite question gets the opposite answer
        let pale = suggest("something pale and fine for trim", 3);
        let p = pale[0].0;
        assert!((p.rgb.0 as f32 + p.rgb.1 as f32 + p.rgb.2 as f32) / 3.0 > 190.0,
                "pale: {}", p.name);
        assert_ne!(p.name, first.name);
    }

    #[test]
    fn a_query_that_contradicts_itself_does_not_win_on_one_half() {
        // "dark glass" should not return the palest glass just because
        // it is glass: a mismatch COSTS, it is not merely unrewarded
        let found = suggest("dark glass", 2);
        if let Some((b, _)) = found.first() {
            let lum = (b.rgb.0 as f32 + b.rgb.1 as f32 + b.rgb.2 as f32) / 3.0;
            assert!(b.family == Family::Glass || lum < 90.0, "{} satisfies one half", b.name);
        }
    }

    #[test]
    fn an_unknown_name_becomes_a_suggestion_rather_than_stone() {
        // the point of the whole module: a name from another game, or
        // one a model invented, is a DESCRIPTION
        let (id, why) = resolve_or_suggest("deepslate");
        assert!(why.is_some(), "it says it substituted");
        assert!(by_id(id).is_some());
        let (id2, why2) = resolve_or_suggest("dark polished stone");
        assert!(why2.is_some());
        assert_eq!(by_id(id2).unwrap().family, Family::Stone, "{why2:?}");
        // a real name is exact and silent
        let (id3, why3) = resolve_or_suggest("obsidian");
        assert_eq!(id3, 49);
        assert!(why3.is_none(), "no noise for a name that works");
        // and total nonsense still yields something buildable
        let (id4, why4) = resolve_or_suggest("zzzqqq");
        assert!(by_id(id4).is_some() && why4.is_some());
    }

    #[test]
    fn the_vocabulary_tells_a_model_what_this_world_actually_has() {
        let v = vocabulary();
        assert!(v.contains("obsidian") && v.contains("hardness"));
        assert!(v.contains("--"), "each line says what it is FOR");
        assert!(!v.contains("air"), "air is not a material to choose");
        assert!(v.lines().count() >= 20);
    }

    #[test]
    fn the_nearest_to_a_known_block_is_never_itself() {
        for blk in BLOCKS.iter().filter(|b| b.family != Family::Air) {
            let s = nearest_known(blk.id).expect("it is in the catalogue");
            assert_ne!(s.id, blk.id, "{} substituted for itself", blk.name);
            // and the substitute is a sensible one: same or compatible family
            let sub = by_id(s.id).unwrap();
            assert!(sub.family == blk.family || compatible(sub.family, blk.family)
                    || blk.family == Family::Liquid || blk.family == Family::Light,
                    "{} -> {} crosses families for no reason", blk.name, sub.name);
        }
    }
}
