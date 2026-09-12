//! An empty room is a box.
//!
//! The brief schedules rooms, the layout places them, doors connect
//! them and circulation reaches them. Then nothing is put in any of
//! them. A fourteen-room castle is fourteen empty boxes, and a
//! "kitchen" is a kitchen only because a data structure says so --
//! walk into it and it is indistinguishable from the stores.
//!
//! Loot is the same gap seen from the other side. There is not one
//! chest anywhere in this crate. A player who walks into a treasury
//! finds a room labelled treasury.
//!
//! Both are one question: WHAT IS THIS ROOM FOR, AND WHAT WOULD BE IN
//! IT. A kitchen has a hearth against a wall, a work surface, storage.
//! A treasury has chests and little else, because that is what makes
//! it read as a treasury rather than as a small hall.
//!
//! WHERE THINGS GO IS MOST OF IT
//!
//! Furniture against walls, not floating in the middle; a hearth on an
//! outside wall because it needs a flue; a table in the centre because
//! that is what a table is for; a chest in a corner. Placing furniture
//! by scattering it is worse than leaving the room empty -- an empty
//! room reads as unfinished, a randomly furnished one reads as broken.

use crate::weather::Change;
use std::collections::BTreeSet;

/// Where a thing wants to be in its room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// Against any wall.
    Wall,
    /// Against an OUTSIDE wall: a hearth needs a flue, a window needs
    /// somewhere to look.
    OuterWall,
    /// In a corner, which is where things are put to be out of the way.
    Corner,
    /// The middle of the floor.
    Centre,
    /// Anywhere on the floor, spread out.
    Floor,
    /// On the ceiling or hanging from it.
    Ceiling,
}

/// One thing that goes in a room.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitting {
    pub name: &'static str,
    pub block: &'static str,
    /// Footprint and height, in blocks.
    pub size: (i32, i32, i32),
    pub spot: Spot,
    /// How many, for a room of ordinary size. Scaled by area.
    pub count: i32,
    /// It holds something: a chest, a barrel, a shelf.
    pub holds: bool,
}

const fn f(name: &'static str, block: &'static str, size: (i32, i32, i32), spot: Spot,
           count: i32, holds: bool) -> Fitting {
    Fitting { name, block, size, spot, count, holds }
}

/// What a room of this kind contains.
///
/// Matched on the room's NAME, which the brief already carries. Not a
/// taxonomy of rooms -- a room this table does not know gets the
/// default, which is a light and somewhere to sit, because that is
/// true of nearly any room a person uses.
pub fn fittings_for(room: &str) -> (Vec<Fitting>, &'static str) {
    // WHOLE WORDS. "moot chamber of the sept" matched `chamber` and
    // was furnished with beds -- a substring inside an unrelated phrase
    // deciding what the room is. The same fault the router had, one
    // layer down.
    let r = room.to_lowercase();
    let words: Vec<&str> = r.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.iter().any(|x| *x == w || x.trim_end_matches('s') == w);
    if has("kitchen") || has("bakery") || has("brewhouse") {
        (vec![f("hearth", "cobblestone", (3, 3, 2), Spot::OuterWall, 1, false),
              f("worktable", "oak_planks", (3, 1, 2), Spot::Centre, 1, false),
              f("barrel", "oak_log", (1, 1, 1), Spot::Corner, 3, true),
              f("shelf", "oak_planks", (2, 1, 1), Spot::Wall, 2, true)],
         "a fire against an outside wall, a surface to work on, and storage in the corners")
    } else if has("hall") || has("nave") || has("court") || has("bailey") {
        (vec![f("high table", "oak_planks", (5, 1, 2), Spot::Wall, 1, false),
              f("bench", "oak_planks", (6, 1, 1), Spot::Floor, 4, false),
              f("hearth", "cobblestone", (3, 2, 2), Spot::OuterWall, 1, false),
              f("banner", "wool", (1, 3, 1), Spot::Wall, 4, false)],
         "a table at the head, benches down the length, a fire, and banners on the walls")
    } else if has("treasury") || has("strongroom") || has("vault") || has("hoard") {
        (vec![f("chest", "oak_planks", (1, 1, 1), Spot::Wall, 6, true),
              f("stand", "gold_block", (1, 1, 1), Spot::Centre, 1, true)],
         "chests against every wall and one thing set apart in the middle")
    } else if has("bed") || has("chamber") || has("lodging") || has("dormitory") || has("solar") {
        (vec![f("bed", "wool", (1, 1, 2), Spot::Corner, 2, false),
              f("chest", "oak_planks", (1, 1, 1), Spot::Wall, 2, true),
              f("shutter", "oak_planks", (1, 2, 1), Spot::OuterWall, 1, false)],
         "beds in the corners, a chest at the foot of each, a window")
    } else if has("store") || has("cellar") || has("undercroft") || has("granary")
        || has("warehouse") || has("pantry") || has("buttery") {
        (vec![f("crate", "oak_planks", (1, 2, 1), Spot::Wall, 8, true),
              f("barrel", "oak_log", (1, 1, 1), Spot::Corner, 4, true),
              f("sack", "wool", (1, 1, 1), Spot::Floor, 3, true)],
         "stacked to the walls and piled in the corners, with a way through the middle")
    } else if has("chapel") || has("shrine") || has("cella") || has("altar") || has("temple") {
        (vec![f("altar", "quartz", (2, 1, 1), Spot::Wall, 1, false),
              f("pew", "oak_planks", (4, 1, 1), Spot::Floor, 4, false),
              f("candle", "glowstone", (1, 1, 1), Spot::Wall, 4, false)],
         "an altar at one end, seating facing it, and light on the walls")
    } else if has("guard") || has("barrack") || has("armoury") || has("watch") {
        (vec![f("rack", "oak_log", (2, 2, 1), Spot::Wall, 2, true),
              f("bunk", "wool", (1, 2, 2), Spot::Wall, 3, false),
              f("brazier", "cobblestone", (1, 1, 1), Spot::Corner, 1, false)],
         "racks and bunks along the walls, a brazier for warmth")
    } else if has("forge") || has("smithy") || has("workshop") || has("machinery") {
        (vec![f("forge", "cobblestone", (2, 2, 2), Spot::OuterWall, 1, false),
              f("anvil", "iron_block", (1, 1, 1), Spot::Centre, 1, false),
              f("bench", "oak_planks", (3, 1, 1), Spot::Wall, 2, false),
              f("crate", "oak_planks", (1, 1, 1), Spot::Corner, 2, true)],
         "a forge on an outside wall, an anvil where there is room to swing")
    } else if has("stable") || has("byre") || has("stall") {
        (vec![f("stall", "oak_planks", (1, 2, 3), Spot::Wall, 4, false),
              f("trough", "oak_log", (2, 1, 1), Spot::Wall, 2, false),
              f("hay", "leaves", (1, 1, 1), Spot::Corner, 3, false)],
         "stalls along the walls, troughs, hay in the corners")
    } else if has("passage") || has("corridor") || has("stair") || has("street") {
        // A corridor furnished is a corridor blocked. This is the one
        // room where the right answer is almost nothing.
        (vec![f("sconce", "torch", (1, 1, 1), Spot::Wall, 3, false)],
         "kept clear, with light on the walls")
    } else {
        // A ROOM THIS TABLE DOES NOT KNOW is not an empty room. Light
        // and somewhere to sit is true of nearly any room a person
        // uses, and it beats leaving it a box.
        (vec![f("sconce", "torch", (1, 1, 1), Spot::Wall, 2, false),
              f("bench", "oak_planks", (3, 1, 1), Spot::Wall, 1, false),
              f("chest", "oak_planks", (1, 1, 1), Spot::Corner, 1, true)],
         "a light, somewhere to sit, and somewhere to put something")
    }
}

fn hash(a: i32, b: i32, c: i32, seed: u64) -> u64 {
    let mut h = (a as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (b as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (c as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9) ^ seed;
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 29)
}

/// A room to be furnished: its floor cells and which of them are
/// against a wall.
#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    pub name: String,
    /// Cells a person could stand on, at floor level.
    pub floor: BTreeSet<(i32, i32, i32)>,
    /// Which floor cells touch an outside wall of the building.
    pub outer: BTreeSet<(i32, i32, i32)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Furnished {
    pub changes: Vec<Change>,
    /// Where something that holds things ended up, for a loot pass.
    pub containers: Vec<(&'static str, (i32, i32, i32))>,
    pub notes: Vec<String>,
}

/// Put the fittings in.
///
/// Placement is most of the work: against walls, in corners, in the
/// middle, and never overlapping. A randomly furnished room reads as
/// broken, which is worse than an empty one reading as unfinished.
pub fn furnish(room: &Room, seed: u64) -> Furnished {
    let mut out = Furnished::default();
    if room.floor.is_empty() { return out; }
    let (fits, why) = fittings_for(&room.name);
    out.notes.push(format!("{}: {why}", room.name));
    let area = room.floor.len() as i32;
    // which floor cells are against SOME wall: a cell with a missing
    // neighbour in the floor set
    let against: BTreeSet<(i32, i32, i32)> = room.floor.iter().copied()
        .filter(|c| [(1, 0), (-1, 0), (0, 1), (0, -1)].iter()
            .any(|(dx, dz)| !room.floor.contains(&(c.0 + dx, c.1, c.2 + dz))))
        .collect();
    let corners: BTreeSet<(i32, i32, i32)> = room.floor.iter().copied()
        .filter(|c| {
            let mx = !room.floor.contains(&(c.0 + 1, c.1, c.2)) || !room.floor.contains(&(c.0 - 1, c.1, c.2));
            let mz = !room.floor.contains(&(c.0, c.1, c.2 + 1)) || !room.floor.contains(&(c.0, c.1, c.2 - 1));
            mx && mz
        }).collect();
    let (lo, hi) = room.floor.iter().fold(((i32::MAX, i32::MAX), (i32::MIN, i32::MIN)),
        |(l, h), c| ((l.0.min(c.0), l.1.min(c.2)), (h.0.max(c.0), h.1.max(c.2))));
    let mid = ((lo.0 + hi.0) / 2, (lo.1 + hi.1) / 2);
    let mut taken: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    for (fi, fit) in fits.iter().enumerate() {
        // MORE OF A THING IN A BIGGER ROOM, but not linearly: a hall
        // twice the size does not want twice the tables, it wants a
        // longer one and a few more benches.
        let n = ((fit.count as f32 * (area as f32 / 40.0).sqrt()).round() as i32).clamp(1, 24);
        let pool: Vec<(i32, i32, i32)> = match fit.spot {
            Spot::OuterWall => {
                if room.outer.is_empty() { against.iter().copied().collect() }
                else { room.outer.iter().copied().collect() }
            }
            Spot::Wall => against.iter().copied().collect(),
            Spot::Corner => if corners.is_empty() { against.iter().copied().collect() }
                            else { corners.iter().copied().collect() },
            Spot::Centre => room.floor.iter().copied()
                .filter(|c| (c.0 - mid.0).abs() <= 2 && (c.2 - mid.1).abs() <= 2).collect(),
            Spot::Floor | Spot::Ceiling => room.floor.iter().copied().collect(),
        };
        if pool.is_empty() { continue; }
        let block = crate::palette::resolve_or_suggest(fit.block).0;
        let mut placed = 0;
        // walk the pool in a seeded but stable order
        let mut order: Vec<&(i32, i32, i32)> = pool.iter().collect();
        order.sort_by_key(|c| hash(c.0, c.2, fi as i32, seed));
        for at in order {
            if placed >= n { break; }
            // room for its footprint, and nothing there already
            let cells: Vec<(i32, i32, i32)> = (0..fit.size.0).flat_map(|dx| {
                (0..fit.size.1).flat_map(move |dy| {
                    (0..fit.size.2).map(move |dz| (dx, dy, dz))
                })
            }).map(|(dx, dy, dz)| (at.0 + dx, at.1 + dy, at.2 + dz)).collect();
            if cells.iter().any(|c| taken.contains(c)) { continue; }
            // its footprint must be on the floor: a table half in a
            // wall is worse than no table
            if cells.iter().any(|c| c.1 == at.1 && !room.floor.contains(c)) { continue; }
            for c in &cells { taken.insert(*c); }
            for c in &cells { out.changes.push(Change { at: *c, to: block }); }
            if fit.holds { out.containers.push((fit.name, *at)); }
            placed += 1;
        }
        if placed < n {
            out.notes.push(format!("  only {placed} of {n} {}(s) fitted", fit.name));
        }
    }
    out
}

// ── loot ─────────────────────────────────────────────────────────────

/// What is in the containers.
///
/// Loot follows the ROOM, not a global table: a chest in a kitchen has
/// food in it, a chest in an armoury has weapons. A single loot table
/// for the whole world is why chests everywhere feel the same.
#[derive(Debug, Clone, PartialEq)]
pub struct Loot {
    pub at: (i32, i32, i32),
    pub container: &'static str,
    /// What is in it, as names the game can resolve to items.
    pub items: Vec<String>,
}

/// Fill the containers a furnishing left.
pub fn stock(room: &str, containers: &[(&'static str, (i32, i32, i32))], seed: u64) -> Vec<Loot> {
    let r = room.to_lowercase();
    let words: Vec<&str> = r.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.iter().any(|x| *x == w || x.trim_end_matches('s') == w);
    let (common, rare): (&[&str], &[&str]) = if has("kitchen") || has("pantry") || has("buttery") {
        (&["bread", "salt", "dried fish", "onions", "ale"], &["spices", "honey"])
    } else if has("treasury") || has("vault") || has("hoard") {
        (&["coins", "silver plate", "a gold chain"], &["a crown", "a jewelled cup", "a deed"])
    } else if has("armoury") || has("guard") || has("barrack") {
        (&["a helm", "a shield", "arrows", "a spear"], &["a good sword", "mail"])
    } else if has("store") || has("cellar") || has("granary") || has("warehouse") {
        (&["grain", "rope", "nails", "cloth", "tallow"], &["a locked box"])
    } else if has("chapel") || has("shrine") || has("temple") {
        (&["candles", "a psalter"], &["a reliquary", "a silver cross"])
    } else if has("forge") || has("smithy") || has("workshop") {
        (&["iron bar", "charcoal", "a hammer", "tongs"], &["a fine blade"])
    } else if has("chamber") || has("lodging") || has("solar") || has("bed") {
        (&["clothes", "a comb", "a candle"], &["a letter", "a ring"])
    } else {
        (&["rags", "a broken tool", "dust"], &["a coin"])
    };
    containers.iter().enumerate().map(|(i, (name, at))| {
        let h = hash(at.0, at.1, at.2, seed ^ i as u64);
        let n = 1 + (h % 3) as usize;
        let mut items: Vec<String> = (0..n)
            .map(|k| common[(hash(at.0, at.2, k as i32, seed) as usize) % common.len()].to_string())
            .collect();
        items.sort(); items.dedup();
        // ONE IN SIX HOLDS SOMETHING WORTH FINDING. If every chest has
        // treasure then no chest does, and the search stops being worth
        // making.
        if h % 6 == 0 && !rare.is_empty() {
            items.push(rare[(h >> 8) as usize % rare.len()].to_string());
        }
        Loot { at: *at, container: name, items }
    }).collect()
}

/// A line per room, for the player and the log.
pub fn describe(f: &Furnished, loot: &[Loot]) -> String {
    let mut s = f.notes.join("\n");
    if !loot.is_empty() {
        let rare = loot.iter().filter(|l| l.items.len() > 2).count();
        s.push_str(&format!("\n  {} container(s), {} worth opening", loot.len(), rare.max(1)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(name: &str, w: i32, d: i32) -> Room {
        let mut floor = BTreeSet::new();
        for x in 0..w { for z in 0..d { floor.insert((x, 64, z)); } }
        let outer: BTreeSet<(i32, i32, i32)> = floor.iter().copied()
            .filter(|c| c.0 == 0).collect();
        Room { name: name.into(), floor, outer }
    }

    #[test]
    fn a_room_gets_what_its_purpose_implies() {
        // a "kitchen" was a kitchen only because a data structure said
        // so -- walk into it and it was the stores
        let k = furnish(&room("kitchen", 8, 6), 3);
        assert!(!k.changes.is_empty());
        assert!(k.notes[0].contains("fire against an outside wall"), "{:?}", k.notes);
        let t = furnish(&room("treasury", 8, 6), 3);
        assert!(t.containers.len() >= 4, "a treasury is chests: {:?}", t.containers.len());
        assert!(t.notes[0].contains("chests against every wall"));
        // and the two rooms are not the same room
        assert_ne!(k.changes.len(), t.changes.len());
        assert_ne!(k.notes[0], t.notes[0]);
    }

    #[test]
    fn things_go_where_they_belong_and_never_overlap() {
        // a randomly furnished room reads as BROKEN, which is worse
        // than an empty one reading as unfinished
        let r = room("great hall", 12, 10);
        let f = furnish(&r, 7);
        // nothing occupies the same cell twice
        let mut seen = BTreeSet::new();
        for c in &f.changes {
            assert!(seen.insert(c.at), "{:?} furnished twice", c.at);
        }
        // and everything sits ON the floor, not in a wall
        for c in f.changes.iter().filter(|c| c.at.1 == 64) {
            assert!(r.floor.contains(&c.at), "{:?} is not on the floor", c.at);
        }
        // a hearth is on an OUTER wall, because it needs a flue
        let hearth_block = crate::palette::resolve_or_suggest("cobblestone").0;
        let hearths: Vec<&Change> = f.changes.iter()
            .filter(|c| c.to == hearth_block && c.at.1 == 64).collect();
        assert!(!hearths.is_empty());
        assert!(hearths.iter().any(|c| r.outer.contains(&c.at)), "the hearth found the outer wall");
    }

    #[test]
    fn a_corridor_is_kept_clear_because_a_furnished_corridor_is_blocked() {
        let c = furnish(&room("corridor", 3, 14), 1);
        let hall = furnish(&room("great hall", 12, 10), 1);
        assert!(c.changes.len() * 4 < hall.changes.len(),
                "a corridor is nearly empty: {} vs {}", c.changes.len(), hall.changes.len());
        assert!(c.notes[0].contains("kept clear"));
        // and it still has light
        assert!(!c.changes.is_empty());
    }

    #[test]
    fn a_room_nobody_has_a_word_for_is_not_an_empty_room() {
        // light and somewhere to sit is true of nearly any room a
        // person uses, and it beats leaving it a box
        // NOT "moot chamber of the sept": that contains the whole word
        // `chamber` and being furnished as one is right. A room with no
        // known word in it at all is the case that matters.
        let f = furnish(&room("scriptorium", 7, 7), 2);
        assert!(!f.changes.is_empty());
        assert!(f.notes[0].contains("a light, somewhere to sit"), "{:?}", f.notes);
        assert!(!f.containers.is_empty(), "and somewhere to put something");
    }

    #[test]
    fn a_bigger_room_holds_more_but_not_proportionally() {
        // a hall twice the size does not want twice the tables
        let small = furnish(&room("stores", 5, 5), 4).changes.len();
        let big = furnish(&room("stores", 20, 20), 4).changes.len();
        assert!(big > small, "{big} vs {small}");
        assert!(big < small * 16, "sixteen times the area, not sixteen times the crates: \
                 {big} vs {small}");
    }

    #[test]
    fn loot_follows_the_room_it_is_in() {
        // a single loot table for the whole world is why chests
        // everywhere feel the same
        let t = furnish(&room("treasury", 8, 8), 5);
        let k = furnish(&room("kitchen", 8, 8), 5);
        let tl = stock("treasury", &t.containers, 5);
        let kl = stock("kitchen", &k.containers, 5);
        assert!(!tl.is_empty() && !kl.is_empty());
        let tall: BTreeSet<&str> = tl.iter().flat_map(|l| l.items.iter().map(|s| s.as_str())).collect();
        let kall: BTreeSet<&str> = kl.iter().flat_map(|l| l.items.iter().map(|s| s.as_str())).collect();
        assert!(tall.iter().any(|i| i.contains("coin") || i.contains("silver") || i.contains("gold")),
                "a treasury holds treasure: {tall:?}");
        assert!(kall.iter().any(|i| i.contains("bread") || i.contains("ale") || i.contains("fish")),
                "a kitchen holds food: {kall:?}");
        assert!(tall.intersection(&kall).count() == 0, "and they share nothing: {tall:?} {kall:?}");
    }

    #[test]
    fn not_every_chest_is_worth_opening() {
        // if every chest has treasure then no chest does, and the
        // search stops being worth making
        let containers: Vec<(&'static str, (i32, i32, i32))> =
            (0..40).map(|i| ("chest", (i, 64, 0))).collect();
        let l = stock("treasury", &containers, 9);
        let rich = l.iter().filter(|x| x.items.len() > 2).count();
        assert!(rich > 0, "some are");
        assert!(rich < l.len() / 2, "most are not: {rich} of {}", l.len());
        // and it is the same every time for the same seed
        assert_eq!(stock("treasury", &containers, 9), l);
        assert_ne!(stock("treasury", &containers, 10), l);
    }

    #[test]
    fn an_empty_room_is_not_a_panic() {
        let empty = Room { name: "void".into(), floor: BTreeSet::new(), outer: BTreeSet::new() };
        assert!(furnish(&empty, 1).changes.is_empty());
        assert!(stock("void", &[], 1).is_empty());
    }
}
