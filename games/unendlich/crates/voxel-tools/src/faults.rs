//! The things that actually go wrong here.
//!
//! The review shows the model elevations, measurements and a diff, and
//! asks what to change. That relies on the model SPOTTING the fault,
//! and a model looking at an ASCII elevation of its own work says
//! "looks good" far more often than it should.
//!
//! But this crate's failures are not a mystery. They have been
//! reported, one at a time, all the way through:
//!
//!   "only a few shapes basic and floating"
//!   "the pose is too symmetrical"
//!   a fourteen-room castle with nothing in any room
//!   an interior with no light in it
//!   everything one material, everything equally worn
//!
//! Every one of those is CHECKABLE. A build that floats can be
//! detected; a build with two-fold mirror symmetry can be measured; a
//! build made of four boxes can be counted. Telling the model "this
//! floats four blocks above the ground" is worth more than any number
//! of rounds of asking it to look harder, because it turns a question
//! of taste into a fact.
//!
//! WHAT THIS IS NOT
//!
//! Not a score, and not a gate. A build can be deliberately floating
//! and deliberately symmetric. These are things a builder would
//! NOTICE, raised so the model can dismiss them for a reason instead
//! of never seeing them.

use std::collections::BTreeSet;

/// How much a fault matters, which decides whether it is worth a
/// round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Weight {
    /// Would read as a bug to anyone who saw it.
    Broken,
    /// Would read as unfinished.
    Poor,
    /// Would read as plain.
    Flat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fault {
    pub weight: Weight,
    /// What is wrong, said as a fact rather than a suggestion.
    pub what: String,
    /// What would fix it. One concrete move, not "add detail".
    pub fix: String,
}

/// Everything known about a build at review time.
pub struct Look<'a> {
    pub cells: &'a BTreeSet<(i32, i32, i32)>,
    /// Ground level under the build.
    pub ground: i32,
    /// Distinct block ids used.
    pub materials: usize,
    /// How many separate parts or passes made it.
    pub parts: usize,
    /// Is it meant to have an inside? A statue is not.
    pub enclosed: bool,
}

/// Check everything, worst first.
pub fn check(l: &Look) -> Vec<Fault> {
    let mut out = Vec::new();
    if l.cells.is_empty() {
        out.push(Fault { weight: Weight::Broken, what: "nothing was built".into(),
                         fix: "the plan produced no cells at all".into() });
        return out;
    }
    let m = crate::bench::measure(l.cells);
    let lowest = l.cells.iter().map(|c| c.1).min().unwrap();

    // FLOATING. Reported directly: "a gorilla statue built but was
    // only a few shapes basic and floating".
    let gap = lowest - l.ground;
    if gap > 1 {
        out.push(Fault { weight: Weight::Broken,
            what: format!("it floats: the lowest block is {gap} above the ground"),
            fix: "put something under it, or lower the whole thing so it stands".into() });
    }

    // IN PIECES. A statue in three parts is three objects.
    if m.islands > 1 {
        out.push(Fault { weight: Weight::Broken,
            what: format!("it is in {} separate pieces", m.islands),
            fix: "join them, or the parts read as unrelated objects".into() });
    }

    // PERFECTLY SYMMETRIC. The reference critique's first complaint
    // about its own statue, and `pair` mirrors exactly, so every
    // figure scored zero and nothing noticed.
    if m.asymmetry < 0.02 && m.cells > 200 {
        out.push(Fault { weight: Weight::Poor,
            what: "the two halves match exactly".into(),
            fix: "shift the weight to one side -- a figure bearing its weight on one leg, \
                  or a building whose wings are not the same length".into() });
    }

    // A MANNEQUIN. "only a few shapes" -- five parts is a head, a body
    // and three sticks.
    if l.parts > 0 && l.parts < 5 && m.cells > 600 {
        out.push(Fault { weight: Weight::Poor,
            what: format!("the whole thing is {} shape(s)", l.parts),
            fix: "name the parts that would be visible at this size and build those too"
                .into() });
    }

    // ONE MATERIAL. Everything the same stone reads as untouched.
    if l.materials <= 1 && m.cells > 400 {
        out.push(Fault { weight: Weight::Flat,
            what: "it is all one material".into(),
            fix: "a second material at the edges, the base or the openings is what makes \
                  the first one read".into() });
    }

    // A SOLID BLOCK. Something meant to be entered, and you cannot.
    if l.enclosed && m.density > 0.85 {
        out.push(Fault { weight: Weight::Broken,
            what: format!("it is {:.0}% solid -- there is no inside", m.density * 100.0),
            fix: "hollow it and cut a way in".into() });
    }

    // NO SILHOUETTE. The width never changes going up: a box.
    if m.steps < 2 && m.height > 8 {
        out.push(Fault { weight: Weight::Flat,
            what: "the outline never changes from bottom to top".into(),
            fix: "a step, a taper, an overhang -- one change in width is what makes a \
                  silhouette".into() });
    }

    // A SLAB. Tall and thin in one axis only, or flat as a pancake.
    let (w, h, d) = (m.width.max(1), m.height.max(1), m.depth.max(1));
    let thinnest = w.min(d);
    if thinnest * 6 < w.max(d) && m.cells > 300 {
        out.push(Fault { weight: Weight::Poor,
            what: format!("it is a slab: {w} by {d} on the ground"),
            fix: "give it depth, or it reads as scenery painted on a wall".into() });
    }
    let _ = h;

    out.sort_by_key(|f| f.weight);
    out
}

/// The faults, as lines for the review prompt.
///
/// Only the worst few: a model given eight complaints fixes the
/// easiest, and the easiest is rarely the one that matters.
pub fn lines(faults: &[Fault], most: usize) -> String {
    if faults.is_empty() { return String::new(); }
    let n = most.clamp(1, 4);
    let mut s = String::from("WHAT IS WRONG WITH IT, measured rather than guessed:\n");
    for f in faults.iter().take(n) {
        s.push_str(&format!("  {} -- {}\n", f.what, f.fix));
    }
    if faults.len() > n {
        s.push_str(&format!("  ({} more, less important)\n", faults.len() - n));
    }
    s.push_str("These are facts about what was built. Fix the first one before the others, \
                or say why it is deliberate.\n");
    s
}

/// Is this build good enough to stop working on it?
///
/// Nothing broken and nothing poor. A flat build is finished, just not
/// interesting, and spending model turns to make a finished thing
/// interesting is a decision for the player rather than the loop.
pub fn good_enough(faults: &[Fault]) -> bool {
    !faults.iter().any(|f| f.weight <= Weight::Poor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: i32, h: i32, d: i32, y0: i32) -> BTreeSet<(i32, i32, i32)> {
        let mut s = BTreeSet::new();
        for x in 0..w { for y in y0..y0 + h { for z in 0..d { s.insert((x, y, z)); } } }
        s
    }
    fn look<'a>(c: &'a BTreeSet<(i32, i32, i32)>) -> Look<'a> {
        Look { cells: c, ground: 64, materials: 2, parts: 8, enclosed: false }
    }

    #[test]
    fn the_faults_that_were_actually_reported_are_the_ones_it_finds() {
        // "a gorilla statue built but was only a few shapes basic and
        // floating"
        let floating = solid(10, 14, 10, 68);
        let mut l = look(&floating);
        l.parts = 4;
        l.materials = 1;
        let f = check(&l);
        assert!(f.iter().any(|x| x.what.contains("floats") && x.what.contains("4 above")),
                "{f:?}");
        assert!(f.iter().any(|x| x.what.contains("4 shape(s)")), "{f:?}");
        assert!(f.iter().any(|x| x.what.contains("all one material")), "{f:?}");
        // and the worst comes first
        assert_eq!(f[0].weight, Weight::Broken);
        assert!(f[0].what.contains("floats"));
    }

    #[test]
    fn perfect_symmetry_is_named_because_nothing_else_notices_it() {
        // the reference critique's first complaint about its own statue
        let box_ = solid(14, 20, 8, 64);
        let f = check(&look(&box_));
        assert!(f.iter().any(|x| x.what.contains("two halves match exactly")), "{f:?}");
        assert!(f.iter().any(|x| x.fix.contains("bearing its weight on one leg")), "{f:?}");
        // a lopsided build is not complained about
        let mut lop = box_.clone();
        for y in 70..80 { for z in 0..8 { for x in 14..22 { lop.insert((x, y, z)); } } }
        let f2 = check(&look(&lop));
        assert!(!f2.iter().any(|x| x.what.contains("two halves match")), "{f2:?}");
    }

    #[test]
    fn a_thing_meant_to_be_entered_that_cannot_be_is_broken() {
        let block = solid(20, 12, 20, 64);
        let mut l = look(&block);
        l.enclosed = true;
        let f = check(&l);
        let solid_fault = f.iter().find(|x| x.what.contains("no inside")).expect("found");
        assert_eq!(solid_fault.weight, Weight::Broken);
        assert!(solid_fault.what.contains("100% solid"), "{:?}", solid_fault.what);
        // a statue is not expected to have an inside
        let mut l2 = look(&block);
        l2.enclosed = false;
        assert!(!check(&l2).iter().any(|x| x.what.contains("no inside")));
    }

    #[test]
    fn a_slab_is_named_as_a_slab() {
        let slab = solid(60, 20, 4, 64);
        let f = check(&look(&slab));
        assert!(f.iter().any(|x| x.what.contains("slab")), "{f:?}");
        assert!(f.iter().any(|x| x.fix.contains("painted on a wall")), "{f:?}");
        // something with depth is not
        assert!(!check(&look(&solid(20, 20, 16, 64))).iter().any(|x| x.what.contains("slab")));
    }

    #[test]
    fn only_the_worst_few_reach_the_prompt() {
        // a model given eight complaints fixes the EASIEST, and the
        // easiest is rarely the one that matters
        let bad = solid(60, 20, 4, 70);
        let mut l = look(&bad);
        l.parts = 1;
        l.materials = 1;
        l.enclosed = true;
        let f = check(&l);
        assert!(f.len() > 4, "this build is wrong in many ways: {}", f.len());
        let s = lines(&f, 3);
        assert_eq!(s.lines().filter(|x| x.starts_with("  ") && !x.contains("more,")).count(), 3);
        assert!(s.contains("more, less important"), "{s}");
        assert!(s.contains("Fix the first one before the others"), "{s}");
        assert!(s.contains("or say why it is deliberate"), "it is not a gate: {s}");
        // and the first line is a BROKEN one
        assert!(s.lines().nth(1).unwrap().contains("floats")
             || s.lines().nth(1).unwrap().contains("no inside"), "{s}");
    }

    #[test]
    fn a_good_build_is_left_alone() {
        // nothing broken and nothing poor. A flat build is FINISHED,
        // just not interesting, and spending turns to make a finished
        // thing interesting is the player's decision.
        let mut good = solid(16, 6, 16, 64);
        for x in 3..13 { for y in 70..80 { for z in 3..13 { good.insert((x, y, z)); } } }
        for x in 6..10 { for y in 80..86 { for z in 6..10 { good.insert((x, y, z)); } } }
        // Genuinely lopsided: a wing on ONE side that extends the
        // bounding box. My first attempt added cells inside the
        // existing footprint, which shifts nothing -- the shape stayed
        // symmetric about its own centre and the check was right to
        // say so.
        for x in 16..26 { for y in 64..70 { for z in 4..12 { good.insert((x, y, z)); } } }
        let f = check(&look(&good));
        assert!(good_enough(&f), "{f:?}");
        // and one with a broken fault is not
        let floating = solid(10, 10, 10, 70);
        assert!(!good_enough(&check(&look(&floating))));
    }

    #[test]
    fn nothing_built_is_the_first_thing_said() {
        let empty = BTreeSet::new();
        let f = check(&look(&empty));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].weight, Weight::Broken);
        assert!(f[0].what.contains("nothing was built"));
        assert!(!good_enough(&f));
        // and no faults at all produces no lines rather than a heading
        assert!(lines(&[], 3).is_empty());
    }
}
