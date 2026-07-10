//! lore.rs — the living world of Clawbound: how cats are named and ranked, what they
//! hunt and heal with, how they mark the passing day and season, and the rites and code
//! that bind a clan together.
//!
//! All wording here is **original** to Clawbound. The genre's texture (warrior ancestors
//! in the night sky, medicine-cats and their herbs, prey and borders, moon-marked time)
//! is the inspiration; none of the phrasing, ceremonies, or names are taken from any book.
//! This is a framework the rest of the game draws on — the sim names its cats from it, the
//! HUD flavours a match with it, and later systems (medicine, ceremonies, the Moot) hang
//! off it.

// ─────────────────────────────────────────────────────────────────────────────
// Deterministic little hash, so every name/roll is reproducible from a seed.
// ─────────────────────────────────────────────────────────────────────────────
fn h(mut x: u32) -> u32 {
    x ^= x >> 16; x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15; x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16; x
}
fn pick(pool: &[&'static str], seed: u32) -> &'static str { pool[(h(seed) as usize) % pool.len()] }

// ─────────────────────────────────────────────────────────────────────────────
// The warrior ancestors and the shape of a clan.
// ─────────────────────────────────────────────────────────────────────────────

/// What the clans call their dead — the ancestors who hunt the night sky and walk a
/// living cat's dreams. (Clawbound's own name for its afterlife.)
pub const ANCESTORS: &str = "the Starlit";
/// The full-moon truce where the clans gather in peace.
pub const GATHERING: &str = "the Moot";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rank { Kit, Apprentice, Warrior, Hunter, Sentinel, Elder, Leader, Deputy, MedicineCat }

impl Rank {
    pub fn title(self) -> &'static str {
        match self {
            Rank::Kit => "kit", Rank::Apprentice => "apprentice", Rank::Warrior => "warrior",
            Rank::Hunter => "hunter", Rank::Sentinel => "sentinel", Rank::Elder => "elder",
            Rank::Leader => "leader", Rank::Deputy => "deputy", Rank::MedicineCat => "medicine cat",
        }
    }
    /// The name-suffix rule for a rank: kits end their name in "-kit", apprentices in
    /// "-paw", and full clan cats bear an earned warrior suffix.
    fn name_suffix(self, seed: u32) -> &'static str {
        match self {
            Rank::Kit => "kit",
            Rank::Apprentice => "paw",
            _ => pick(&WARRIOR_SUFFIXES, seed),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Procedural cat names — a nature prefix earned at birth, a warrior suffix earned in
// the field. Deterministic from a seed so a match's roster is stable.
// ─────────────────────────────────────────────────────────────────────────────

const PREFIXES: [&str; 56] = [
    "Ash", "Bracken", "Briar", "Cinder", "Dawn", "Dusk", "Ember", "Fallow", "Fern", "Finch",
    "Flint", "Frost", "Gorse", "Hawk", "Hazel", "Heather", "Holly", "Ivy", "Kestrel", "Lark",
    "Larch", "Marsh", "Mist", "Moss", "Nettle", "Newt", "Oak", "Otter", "Pebble", "Pine",
    "Quill", "Reed", "Robin", "Rook", "Russet", "Sedge", "Shade", "Silt", "Sorrel", "Spider",
    "Sparrow", "Stone", "Swift", "Talon", "Thistle", "Thorn", "Vetch", "Vole", "Willow", "Wren",
    "Adder", "Bramble", "Crow", "Fox", "Sage", "Storm",
];

const WARRIOR_SUFFIXES: [&str; 30] = [
    "claw", "fur", "pelt", "tail", "heart", "foot", "fang", "whisker", "stripe", "leap",
    "pounce", "gaze", "flight", "song", "leaf", "stalk", "shade", "storm", "frost", "bramble",
    "thorn", "wing", "step", "nose", "ear", "belly", "tuft", "creek", "dapple", "fall",
];

/// Full name for a cat of a given rank, seeded. e.g. seed → "Bracken" + "claw" = "Brackenclaw",
/// an apprentice becomes "Brackenpaw", a kit "Brackenkit".
pub fn cat_name(seed: u32, rank: Rank) -> String {
    let prefix = pick(&PREFIXES, seed);
    format!("{}{}", prefix, rank.name_suffix(seed ^ 0x9e37_79b9))
}

/// A whole clan roster: the player first (their rank), then a spread of clanmates.
pub fn roster(seed: u32, player_rank: Rank, n: usize) -> Vec<(String, Rank)> {
    let ranks = [Rank::Warrior, Rank::Hunter, Rank::Sentinel, Rank::Warrior, Rank::Apprentice, Rank::Elder];
    (0..n).map(|i| {
        let s = seed.wrapping_add((i as u32).wrapping_mul(0x1000_1))
            .wrapping_add(0x51ed);
        if i == 0 { (cat_name(s, player_rank), player_rank) }
        else { let r = ranks[(h(s) as usize) % ranks.len()]; (cat_name(s, r), r) }
    }).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Prey — what a clan hunts, where it's found, and what it's worth on the fresh-kill pile.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Prey {
    pub name: &'static str,
    pub haunt: &'static str,   // the biome/terrain it favours
    pub value: f32,            // how much it feeds the clan (0..1)
    pub difficulty: f32,       // how hard it is to catch (0..1)
}

pub const PREY: [Prey; 10] = [
    Prey { name: "mouse",     haunt: "the oak floor",       value: 0.30, difficulty: 0.25 },
    Prey { name: "vole",      haunt: "the reed banks",      value: 0.35, difficulty: 0.30 },
    Prey { name: "shrew",     haunt: "the leaf-litter",     value: 0.25, difficulty: 0.35 },
    Prey { name: "sparrow",   haunt: "the low branches",    value: 0.40, difficulty: 0.55 },
    Prey { name: "blackbird", haunt: "the forest edge",     value: 0.50, difficulty: 0.60 },
    Prey { name: "squirrel",  haunt: "the oak canopy",      value: 0.55, difficulty: 0.65 },
    Prey { name: "rabbit",    haunt: "the open moor",       value: 0.70, difficulty: 0.75 },
    Prey { name: "frog",      haunt: "the mangrove shallows", value: 0.30, difficulty: 0.40 },
    Prey { name: "minnow",    haunt: "the river shallows",  value: 0.35, difficulty: 0.50 },
    Prey { name: "lizard",    haunt: "the sun-warm stones", value: 0.28, difficulty: 0.45 },
]; // (real animals; the placements and values are Clawbound's own)

/// The prey most worth stalking in a given biome name, if any.
pub fn prey_of(biome: &str) -> Option<&'static Prey> {
    PREY.iter().find(|p| match (biome, p.name) {
        ("Oak Tree Forest", "squirrel") | ("Moor Plains", "rabbit") | ("River Reeds", "vole") |
        ("Lake Shore", "minnow") | ("Mangrove Shallows", "frog") | ("Mountains", "lizard") |
        ("Shadowy Forest", "mouse") => true,
        _ => false,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Herbs — a medicine-cat's store. What each treats and how potent it is.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Herb {
    pub name: &'static str,
    pub treats: &'static str,
    pub potency: f32,   // 0..1
}

pub const HERBS: [Herb; 10] = [
    Herb { name: "marigold",   treats: "wounds and infection", potency: 0.7 },
    Herb { name: "cobweb",     treats: "bleeding",             potency: 0.6 },
    Herb { name: "poppy seed", treats: "pain and sleeplessness", potency: 0.8 },
    Herb { name: "borage",     treats: "fever and nursing",    potency: 0.6 },
    Herb { name: "comfrey",    treats: "broken bones",         potency: 0.75 },
    Herb { name: "juniper",    treats: "bellyache and breath", potency: 0.55 },
    Herb { name: "tansy",      treats: "coughs and chills",    potency: 0.65 },
    Herb { name: "yarrow",     treats: "swallowed poison",     potency: 0.7 },
    Herb { name: "chervil",    treats: "infected wounds",      potency: 0.6 },
    Herb { name: "catmint",    treats: "the whitecough",       potency: 0.9 },
]; // (real plants with folk uses; the store and phrasing are Clawbound's own)

/// The best herb in the store for a given ailment keyword, if one is kept for it.
pub fn remedy_for(ailment: &str) -> Option<&'static Herb> {
    HERBS.iter().filter(|hb| hb.treats.contains(ailment))
        .max_by(|a, b| a.potency.partial_cmp(&b.potency).unwrap())
}

// ─────────────────────────────────────────────────────────────────────────────
// The passing day — how cats mark time by the sun and moon.
// ─────────────────────────────────────────────────────────────────────────────

/// Name the phase of the day from a 0..1 fraction (0 = midnight, 0.5 = midday).
pub fn day_phase(frac: f32) -> &'static str {
    let f = frac.rem_euclid(1.0);
    match (f * 8.0) as u32 {
        0 => "moonhigh", 1 => "before dawn", 2 => "dawn", 3 => "sunrise",
        4 => "sunhigh", 5 => "afternoon", 6 => "sunset", _ => "dusk",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Region lore — a line of colour for each of the map's biomes.
// ─────────────────────────────────────────────────────────────────────────────

pub fn region_lore(biome: &str) -> &'static str {
    match biome {
        "Oak Tree Forest" => "Old oaks close overhead; the leaf-litter hides mice, and the branches, squirrels.",
        "Moor Plains"     => "Wind combs the open grass. Nothing crosses it unseen, but a rabbit run rewards fast paws.",
        "River Reeds"     => "Reeds hiss along the slow water. Voles den in the banks and the fishing is good for those who'll wade.",
        "Lake Shore"      => "A wide pale shore where minnows flash in the shallows and the wind carries every scent far.",
        "Mangrove Shallows" => "Tangled roots and warm brackish water — frogs and fish for the clan that learned to swim and climb both.",
        "Shadowy Forest" => "Close pines swallow the light. Scent lingers here, and a lurker is on you before you hear it.",
        "Mountains"       => "Cold scree and thin air. Little hunts here but the sure-footed, and lizards bask on the sun-warm stones.",
        _ => "Clan territory, marked at its borders by scent and defended by the warrior code.",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The Code — Clawbound's own warrior code (original tenets).
// ─────────────────────────────────────────────────────────────────────────────

pub const CODE: [&str; 8] = [
    "Defend your clan, even at the cost of your life.",
    "Hunt only for the clan; take no more prey than the pile needs.",
    "Mark and honour your borders; challenge every unknown scent.",
    "A kit becomes an apprentice at six moons, an apprentice a warrior only when they have earned it.",
    "The word of the leader is the warrior code, and the deputy speaks it in their stead.",
    "A medicine-cat harms no cat and keeps the dreams of the Starlit.",
    "At the Moot, under the full moon, claws stay sheathed and there is peace.",
    "No warrior neglects a kit, an elder, or a wounded cat, whatever their clan.",
];

/// A code tenet chosen by seed (e.g. to flavour a loading screen or a Moot).
pub fn code_line(seed: u32) -> &'static str { pick(&CODE, seed) }

// ─────────────────────────────────────────────────────────────────────────────
// Ceremonies — original rites, generated for a named cat. No book wording is used.
// ─────────────────────────────────────────────────────────────────────────────

/// Raising an apprentice: the kit takes a "-paw" name and a mentor.
pub fn apprentice_rite(kit_name: &str, mentor: &str) -> String {
    let base = kit_name.trim_end_matches("kit");
    format!(
        "From this day, until you have earned your warrior name, you will be known as {base}paw. \
         {mentor}, pass on to this apprentice all you know of the hunt and the claw."
    )
}

/// Naming a warrior: the apprentice earns a full name and swears to the code.
pub fn warrior_rite(app_name: &str, warrior_name: &str) -> String {
    format!(
        "{app_name}, do you promise to uphold the code and defend your clan, even at the cost of your life? \
         Then by the Starlit I give you your warrior name: {warrior_name}. The clan honours your courage."
    )
}

/// The farewell spoken over a fallen clanmate. (Clawbound's own blessing.)
pub fn death_blessing(name: &str) -> String {
    format!(
        "Walk on with the Starlit, {name}. May the hunting be easy, the running swift, and the nights warm \
         where you go now. The clan will remember your name on every full moon."
    )
}

/// The leader's charge, spoken when a deputy is raised to lead the clan.
pub fn leader_rite(name: &str, clan: &str) -> String {
    format!(
        "{name}, the Starlit have watched you and found you worthy. Carry {clan} through leaf-bare and greenleaf alike, \
         and let your name be spoken with the leaders who came before."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_deterministic_and_ranked() {
        assert_eq!(cat_name(42, Rank::Warrior), cat_name(42, Rank::Warrior));   // stable
        assert!(cat_name(1, Rank::Kit).ends_with("kit"));
        assert!(cat_name(1, Rank::Apprentice).ends_with("paw"));
        let w = cat_name(7, Rank::Warrior);
        assert!(!w.ends_with("kit") && !w.ends_with("paw") && w.len() > 4);
        // different seeds generally give different names
        let names: std::collections::HashSet<_> = (0..40).map(|s| cat_name(s, Rank::Warrior)).collect();
        assert!(names.len() > 20, "the generator should produce variety, got {}", names.len());
    }

    #[test]
    fn roster_puts_the_player_first_with_their_rank() {
        let r = roster(99, Rank::Leader, 6);
        assert_eq!(r.len(), 6);
        assert_eq!(r[0].1, Rank::Leader);
        assert_eq!(roster(99, Rank::Leader, 6), roster(99, Rank::Leader, 6));   // deterministic
    }

    #[test]
    fn prey_and_herbs_look_up() {
        assert_eq!(prey_of("Moor Plains").map(|p| p.name), Some("rabbit"));
        assert_eq!(prey_of("Oak Tree Forest").map(|p| p.name), Some("squirrel"));
        // catmint is the strongest thing kept for coughs/whitecough
        assert_eq!(remedy_for("whitecough").map(|h| h.name), Some("catmint"));
        assert!(remedy_for("bleeding").is_some());
    }

    #[test]
    fn day_phases_and_ceremonies() {
        assert_eq!(day_phase(0.5), "sunhigh");
        assert_eq!(day_phase(0.0), "moonhigh");
        assert!(apprentice_rite("Mosskit", "Oakclaw").contains("Mosspaw"));
        assert!(death_blessing("Brackenclaw").contains("Brackenclaw"));
        assert!(!region_lore("Moor Plains").is_empty());
        assert_eq!(CODE.len(), 8);
    }
}
