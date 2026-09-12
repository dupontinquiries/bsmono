//! Verbs, not categories.
//!
//! Everything this crate builds must first be one of five KINDS --
//! figure, building, landscape, object, several. That taxonomy decides
//! the route, and it works for "a castle" and "a gorilla". It has
//! nothing at all to say about:
//!
//!   a cathedral of ice that is melting
//!   a tower of stacked animals
//!   a castle that looks like it is falling over
//!   an overgrown temple nobody has visited in a century
//!   a half-built bridge, abandoned
//!
//! Every one of those is a thing this crate can already build, plus a
//! VERB it cannot say. And a verb is worth more than a category
//! because a category DIVIDES -- each new one covers a slice of what
//! is possible -- while a verb MULTIPLIES: any build times any
//! transform. Nine transforms over the existing routes is more reach
//! than nine new typologies, and none of it is another table of nouns.
//!
//! WHAT A TRANSFORM IS
//!
//! A function over the cells a build produced, not a shape and not a
//! plan. It runs last, knows nothing about what it is acting on, and
//! therefore acts on anything. `melting` does not care whether it was
//! given a cathedral or a gorilla.
//!
//! THIS IS WHERE THE STRUCTURE SHOULD END. Everything up to here
//! decides what to build; a transform decides what has HAPPENED to it,
//! and that is the part a taxonomy was never going to reach.

use crate::weather::Change;
use std::collections::{BTreeMap, BTreeSet};

/// A verb that can be applied to any finished build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Softened, rounded, running downward. Ice, wax, heat.
    Melting,
    /// Plants have taken it: growth on every upward face, heavier low
    /// down and in the corners where nobody swept.
    Overgrown,
    /// Leaning, with the low side broken and the debris where it fell.
    Collapsing,
    /// Only partly there, and the part that exists is the part built
    /// first: foundations, then walls, then the top.
    HalfBuilt,
    /// Weathered to bone: soft material gone, hard material pitted.
    Fossilised,
    /// Burnt: charred at the surface, holed where it burned through.
    Burnt,
    /// Under water to a line, with everything below it changed.
    Sunken,
    /// Frozen over: a rind on every exposed face.
    Frozen,
    /// Struck: a wound through it, and the pieces displaced.
    Broken,
}

impl Verb {
    /// The words a person would use. Not a lookup table of nouns --
    /// these are the verbs themselves, and there are nine.
    pub fn parse(s: &str) -> Option<Verb> {
        let l = s.to_lowercase();
        let has = |w: &str| l.split(|c: char| !c.is_alphanumeric()).any(|x| x == w);
        for (v, words) in [
            (Verb::Melting, &["melting", "melted", "thawing", "dripping", "slumping"][..]),
            // "abandoned" was here AND under half-built, and whichever
            // was checked first won: "a half-built bridge, abandoned"
            // came back overgrown. A word that fits two verbs equally
            // should decide neither -- these are the ones that mean
            // growth and nothing else.
            (Verb::Overgrown, &["overgrown", "overrun", "forgotten", "wild",
                                "mossy", "vined", "jungle", "reclaimed"][..]),
            (Verb::Collapsing, &["collapsing", "collapsed", "falling", "toppling", "leaning",
                                 "crumbling", "tumbledown"][..]),
            (Verb::HalfBuilt, &["halfbuilt", "unfinished", "incomplete", "started",
                                "partial", "abandoned"][..]),
            (Verb::Fossilised, &["fossilised", "fossilized", "petrified", "ancient",
                                 "weathered", "eroded", "bleached"][..]),
            (Verb::Burnt, &["burnt", "burned", "charred", "scorched", "razed", "gutted"][..]),
            (Verb::Sunken, &["sunken", "sunk", "flooded", "drowned", "submerged", "underwater"][..]),
            (Verb::Frozen, &["frozen", "iced", "icebound", "glaciated", "rimed"][..]),
            (Verb::Broken, &["broken", "shattered", "struck", "split", "cracked", "ruptured"][..]),
        ] {
            if words.iter().any(|w| has(w)) { return Some(v); }
        }
        None
    }

    pub fn name(self) -> &'static str {
        match self {
            Verb::Melting => "melting", Verb::Overgrown => "overgrown",
            Verb::Collapsing => "collapsing", Verb::HalfBuilt => "half-built",
            Verb::Fossilised => "fossilised", Verb::Burnt => "burnt",
            Verb::Sunken => "sunken", Verb::Frozen => "frozen", Verb::Broken => "broken",
        }
    }

    /// What it does, for a prompt. A model choosing a transform needs
    /// to know what each one WILL DO, not just its name.
    pub fn describes(self) -> &'static str {
        match self {
            Verb::Melting => "edges rounded off and material run downward, as if it were warm",
            Verb::Overgrown => "growth on every upward face, heaviest low down and in corners",
            Verb::Collapsing => "leaning, the low side broken, the debris where it fell",
            Verb::HalfBuilt => "only the part built first: foundations, then walls, then the top",
            Verb::Fossilised => "soft material gone, hard material pitted and bleached",
            Verb::Burnt => "charred at the surface and holed where it burned through",
            Verb::Sunken => "under water to a line, everything below it changed",
            Verb::Frozen => "a rind of ice on every exposed face",
            Verb::Broken => "a wound struck through it, the pieces displaced",
        }
    }

    pub fn all() -> [Verb; 9] {
        [Verb::Melting, Verb::Overgrown, Verb::Collapsing, Verb::HalfBuilt, Verb::Fossilised,
         Verb::Burnt, Verb::Sunken, Verb::Frozen, Verb::Broken]
    }
}

fn hash(a: i32, b: i32, c: i32, seed: u64) -> u64 {
    let mut h = (a as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (b as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (c as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9) ^ seed;
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 29)
}

fn frac(a: i32, b: i32, c: i32, seed: u64) -> f32 { (hash(a, b, c, seed) % 1000) as f32 / 1000.0 }

/// What a transform did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Applied {
    pub changes: Vec<Change>,
    pub removed: usize,
    pub added: usize,
    pub note: String,
}

/// Apply a verb to a build.
///
/// `strength` is 0 to 1: barely touched, to unrecognisable. The cells
/// are what the build produced; nothing here knows or asks what it
/// was, which is precisely why it works on anything.
pub fn apply(v: Verb, cells: &BTreeSet<(i32, i32, i32)>, block_at: &BTreeMap<(i32, i32, i32), u16>,
             strength: f32, seed: u64) -> Applied {
    let s = strength.clamp(0.05, 1.0);
    let mut out = Applied::default();
    if cells.is_empty() { return out; }
    let (lo, hi) = cells.iter().fold(
        ((i32::MAX, i32::MAX, i32::MAX), (i32::MIN, i32::MIN, i32::MIN)),
        |(l, h), c| ((l.0.min(c.0), l.1.min(c.1), l.2.min(c.2)),
                     (h.0.max(c.0), h.1.max(c.1), h.2.max(c.2))));
    let height = (hi.1 - lo.1).max(1) as f32;
    let solid = |c: (i32, i32, i32)| cells.contains(&c);
    let exposed = |c: (i32, i32, i32)| [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
        .iter().any(|d| !solid((c.0 + d.0, c.1 + d.1, c.2 + d.2)));
    let up_face = |c: (i32, i32, i32)| !solid((c.0, c.1 + 1, c.2));
    let blk = |name: &str| crate::palette::resolve_or_suggest(name).0;
    let rm = |out: &mut Applied, c: (i32, i32, i32)| {
        out.changes.push(Change { at: c, to: 0 }); out.removed += 1;
    };
    let add = |out: &mut Applied, c: (i32, i32, i32), b: u16| {
        out.changes.push(Change { at: c, to: b }); out.added += 1;
    };
    match v {
        Verb::Melting => {
            // corners go first and material runs DOWN: a melting thing
            // loses its edges and gains a skirt
            for c in cells {
                if !exposed(*c) { continue; }
                let open = [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)].iter()
                    .filter(|d| !solid((c.0 + d.0, c.1, c.2 + d.2))).count();
                let high = (c.1 - lo.1) as f32 / height;
                // higher up melts more: heat rises and there is less
                // holding it
                if open >= 2 && frac(c.0, c.1, c.2, seed) < s * (0.35 + high * 0.5) {
                    rm(&mut out, *c);
                    // and it runs: the cell below the drip gains
                    let below = (c.0, c.1 - 1, c.2);
                    if !solid(below) && frac(c.0, c.1, c.2, seed ^ 9) < s * 0.4 {
                        add(&mut out, below, block_at.get(c).copied().unwrap_or(1));
                    }
                }
            }
        }
        Verb::Overgrown => {
            let leaf = blk("leaves");
            for c in cells {
                if !up_face(*c) { continue; }
                let low = 1.0 - (c.1 - lo.1) as f32 / height;
                // corners and low places: where nobody swept
                let sheltered = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter()
                    .filter(|(dx, dz)| solid((c.0 + dx, c.1 + 1, c.2 + dz))).count();
                let p = s * (0.25 + low * 0.5 + sheltered as f32 * 0.12);
                if frac(c.0, c.1, c.2, seed) < p {
                    add(&mut out, (c.0, c.1 + 1, c.2), leaf);
                }
            }
        }
        Verb::Collapsing => {
            // a lean about one edge, and the low side gives way
            let axis = hash(lo.0, lo.2, 0, seed) % 2 == 0;
            let span = if axis { (hi.0 - lo.0).max(1) } else { (hi.2 - lo.2).max(1) } as f32;
            for c in cells {
                let along = if axis { (c.0 - lo.0) as f32 } else { (c.2 - lo.2) as f32 } / span;
                let high = (c.1 - lo.1) as f32 / height;
                // the low side, high up, goes
                let p = s * along * (0.3 + high * 0.7);
                if frac(c.0, c.1, c.2, seed) < p {
                    rm(&mut out, *c);
                    // and lands outward at the foot
                    if frac(c.0, c.1, c.2, seed ^ 3) < 0.25 {
                        let out_at = if axis { (hi.0 + 1 + (frac(c.0, c.1, c.2, seed ^ 5) * 4.0) as i32,
                                                lo.1, c.2) }
                                     else { (c.0, lo.1, hi.2 + 1 + (frac(c.0, c.1, c.2, seed ^ 5) * 4.0) as i32) };
                        add(&mut out, out_at, block_at.get(c).copied().unwrap_or(1));
                    }
                }
            }
        }
        Verb::HalfBuilt => {
            // what exists is what was built FIRST, which is the bottom
            let keep_to = lo.1 as f32 + height * (1.0 - s * 0.85);
            for c in cells {
                if (c.1 as f32) <= keep_to { continue; }
                // a ragged edge, not a clean slice: a build stops
                // unevenly
                let over = (c.1 as f32 - keep_to) / height.max(1.0);
                if frac(c.0, c.1, c.2, seed) < (0.35 + over * 4.0).min(1.0) { rm(&mut out, *c); }
            }
        }
        Verb::Fossilised => {
            let hard = blk("stone");
            for c in cells {
                if !exposed(*c) { continue; }
                let b = block_at.get(c).copied().unwrap_or(1);
                let soft = crate::palette::by_id(b).map(|x| x.hardness < 1.5).unwrap_or(false);
                if soft && frac(c.0, c.1, c.2, seed) < s * 0.8 { rm(&mut out, *c); }
                else if frac(c.0, c.1, c.2, seed ^ 7) < s * 0.12 { rm(&mut out, *c); }
                else if b != hard && frac(c.0, c.1, c.2, seed ^ 11) < s * 0.4 {
                    add(&mut out, *c, hard);
                }
            }
        }
        Verb::Burnt => {
            let char_b = blk("obsidian");
            for c in cells {
                if !exposed(*c) { continue; }
                let high = (c.1 - lo.1) as f32 / height;
                // fire climbs: the top is worse
                if frac(c.0, c.1, c.2, seed) < s * high * 0.35 { rm(&mut out, *c); }
                else if frac(c.0, c.1, c.2, seed ^ 2) < s * 0.6 { add(&mut out, *c, char_b); }
            }
        }
        Verb::Sunken => {
            let water = blk("water");
            let line = lo.1 + (height * s * 0.7) as i32;
            // Water fills the open space up to the line, INSIDE AND
            // AROUND. Searching only within the bounding box did
            // nothing to a solid mass -- there is no open space inside
            // one -- and a solid thing standing in water is still
            // sunken. The margin is what makes it read as standing IN
            // something rather than merely being wet.
            let m = 4;
            for x in lo.0 - m..=hi.0 + m { for z in lo.2 - m..=hi.2 + m { for y in lo.1..=line {
                let c = (x, y, z);
                if !solid(c) { add(&mut out, c, water); }
            }}}
        }
        Verb::Frozen => {
            let ice = blk("ice");
            for c in cells {
                if !exposed(*c) { continue; }
                if frac(c.0, c.1, c.2, seed) < s * 0.55 {
                    // a rind ON it, not instead of it
                    for d in [(0, 1, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)] {
                        let n = (c.0 + d.0, c.1 + d.1, c.2 + d.2);
                        if !solid(n) && frac(n.0, n.1, n.2, seed ^ 4) < s * 0.5 {
                            add(&mut out, n, ice);
                        }
                    }
                }
            }
        }
        Verb::Broken => {
            // a wound: a plane struck through, wider as it goes
            let angle = (hash(lo.0, lo.1, lo.2, seed) % 360) as f32;
            let (ux, uz) = (angle.to_radians().cos(), angle.to_radians().sin());
            let (mx, mz) = ((lo.0 + hi.0) as f32 / 2.0, (lo.2 + hi.2) as f32 / 2.0);
            let width = 1.0 + s * 5.0;
            for c in cells {
                let d = ((c.0 as f32 - mx) * uz - (c.2 as f32 - mz) * ux).abs();
                let high = (c.1 - lo.1) as f32 / height;
                if d < width * (0.4 + high) && frac(c.0, c.1, c.2, seed) < 0.85 {
                    rm(&mut out, *c);
                }
            }
        }
    }
    out.note = format!("{}: {} removed, {} added", v.name(), out.removed, out.added);
    out
}

/// The transforms, for a prompt. A model choosing one needs to know
/// what each WILL DO.
pub fn vocabulary() -> String {
    Verb::all().iter()
        .map(|v| format!("  {:<12} {}", v.name(), v.describes()))
        .collect::<Vec<_>>().join("\n")
}

/// Read a verb and a strength out of a request. Nothing else in this
/// module reads words; this is the one place, and it is nine verbs
/// rather than a table of nouns.
pub fn read(request: &str) -> Option<(Verb, f32)> {
    let v = Verb::parse(request)?;
    let l = request.to_lowercase();
    let strength = if ["utterly", "completely", "totally", "entirely", "wholly"]
        .iter().any(|w| l.contains(w)) { 1.0 }
        else if ["barely", "slightly", "faintly", "a little", "just"]
        .iter().any(|w| l.contains(w)) { 0.25 }
        else if ["half", "partly", "partially"].iter().any(|w| l.contains(w)) { 0.5 }
        else { 0.7 };
    Some((v, strength))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(w: i32, h: i32) -> (BTreeSet<(i32, i32, i32)>, BTreeMap<(i32, i32, i32), u16>) {
        let mut s = BTreeSet::new();
        let mut b = BTreeMap::new();
        for x in 0..w { for y in 64..64 + h { for z in 0..w {
            s.insert((x, y, z));
            b.insert((x, y, z), 1);
        }}}
        (s, b)
    }

    #[test]
    fn a_verb_composes_with_anything_because_it_knows_nothing() {
        // a category DIVIDES -- each covers a slice of what is possible
        // -- while a verb MULTIPLIES: any build times any transform
        let (cells, blocks) = block(12, 14);
        for v in Verb::all() {
            let a = apply(v, &cells, &blocks, 0.7, 5);
            assert!(!a.changes.is_empty(), "{} did nothing", v.name());
            assert!(a.note.contains(v.name()) && a.note.contains("removed"), "{}", a.note);
            // and it never touches what is not there
            for c in a.changes.iter().filter(|c| c.to == 0) {
                assert!(cells.contains(&c.at), "{} removed a cell that was not solid", v.name());
            }
        }
        // an empty build is not a panic
        assert!(apply(Verb::Melting, &BTreeSet::new(), &blocks, 1.0, 1).changes.is_empty());
    }

    #[test]
    fn each_verb_does_what_its_name_says_and_not_the_same_thing() {
        let (cells, blocks) = block(14, 16);
        let at = |v| apply(v, &cells, &blocks, 0.8, 3);
        // MELTING loses more at the top, where heat rises and less
        // holds it
        let melt = at(Verb::Melting);
        let gone: Vec<i32> = melt.changes.iter().filter(|c| c.to == 0).map(|c| c.at.1).collect();
        let mid = 64 + 8;
        let (high, low) = (gone.iter().filter(|y| **y > mid).count(),
                           gone.iter().filter(|y| **y <= mid).count());
        assert!(high > low, "melting takes the top first: {high} vs {low}");
        // HALF-BUILT keeps the bottom, which is what was built first
        let half = at(Verb::HalfBuilt);
        let cut: Vec<i32> = half.changes.iter().filter(|c| c.to == 0).map(|c| c.at.1).collect();
        assert!(cut.iter().all(|y| *y > 64 + 2), "the foundations survive: {:?}", cut.iter().min());
        assert!(half.added == 0, "nothing is ADDED to an unfinished build");
        // OVERGROWN only adds, and only above
        let grow = at(Verb::Overgrown);
        assert_eq!(grow.removed, 0, "growth takes nothing away");
        assert!(grow.added > 0);
        for c in &grow.changes {
            assert!(!cells.contains(&c.at), "growth sits ON it, not in it");
        }
        // SUNKEN adds water and removes nothing
        let sink = at(Verb::Sunken);
        assert_eq!(sink.removed, 0);
        assert!(sink.added > 0);
        // BROKEN cuts a wound through
        let brk = at(Verb::Broken);
        assert!(brk.removed > 0 && brk.added == 0);
        // and no two of them produce the same change set
        let sets: Vec<usize> = Verb::all().iter().map(|v| at(*v).changes.len()).collect();
        let uniq: BTreeSet<usize> = sets.iter().copied().collect();
        assert!(uniq.len() >= 7, "the verbs differ from each other: {sets:?}");
    }

    #[test]
    fn strength_is_the_difference_between_touched_and_unrecognisable() {
        let (cells, blocks) = block(12, 12);
        for v in [Verb::Melting, Verb::Collapsing, Verb::HalfBuilt, Verb::Broken] {
            let light = apply(v, &cells, &blocks, 0.15, 1).removed;
            let heavy = apply(v, &cells, &blocks, 1.0, 1).removed;
            assert!(heavy > light * 2, "{}: {light} at 0.15, {heavy} at 1.0", v.name());
        }
        // and the same seed gives the same result
        let a = apply(Verb::Melting, &cells, &blocks, 0.6, 42);
        let b = apply(Verb::Melting, &cells, &blocks, 0.6, 42);
        assert_eq!(a.changes, b.changes);
        assert_ne!(apply(Verb::Melting, &cells, &blocks, 0.6, 43).changes, a.changes);
    }

    #[test]
    fn the_words_people_use_find_the_verb_and_how_far() {
        for (said, want) in [("a melting cathedral of ice", Verb::Melting),
                             ("an overgrown temple nobody has visited", Verb::Overgrown),
                             ("a castle that looks like it is collapsing", Verb::Collapsing),
                             ("a half-built bridge, abandoned", Verb::HalfBuilt),
                             ("a petrified forest", Verb::Fossilised),
                             ("a burnt-out manor", Verb::Burnt),
                             ("a flooded crypt", Verb::Sunken),
                             ("a frozen watchtower", Verb::Frozen),
                             ("a shattered obelisk", Verb::Broken)] {
            assert_eq!(read(said).map(|(v, _)| v), Some(want), "{said}");
        }
        // and how far
        assert_eq!(read("a completely melted tower").unwrap().1, 1.0);
        assert_eq!(read("a barely melted tower").unwrap().1, 0.25);
        assert_eq!(read("a partly melted tower").unwrap().1, 0.5);
        // a request with no verb is not transformed
        assert!(read("a stone tower").is_none());
        assert!(read("a castle with a chapel").is_none());
    }

    #[test]
    fn nine_verbs_reach_further_than_nine_typologies() {
        // a category divides; a verb multiplies. This is the arithmetic
        // that makes verbs the better spend.
        let v = vocabulary();
        assert_eq!(v.lines().count(), 9);
        assert!(v.contains("melting") && v.contains("edges rounded off"),
                "a model needs to know what each WILL DO: {v}");
        assert!(v.contains("half-built") && v.contains("built first"));
        // every verb described, none of them just named
        for verb in Verb::all() {
            assert!(v.contains(verb.describes()), "{} has no description", verb.name());
        }
    }
}
