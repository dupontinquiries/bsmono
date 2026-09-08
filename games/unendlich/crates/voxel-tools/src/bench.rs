//! Try something, see what changed, keep it or not.
//!
//! Every review round showed the model the CURRENT structure and asked
//! what to change. It never showed the previous one, so there was no
//! way to answer the only question that matters in an iteration: did
//! the last change help? A model editing blind repeats itself, undoes
//! its own improvements, and plateaus -- which is exactly what the
//! long-run convergence tracking was measuring without being able to
//! explain.
//!
//! A bench holds attempts. Each is source, the cells it produced, and
//! its measurements. `compare` says what moved and in which direction,
//! in the words a builder would use rather than as a vector of floats.
//!
//! THREE WAYS IN, deliberately
//!
//!   PRIMITIVES   `part`/`rel`, which is most of what a model should
//!                write and the only level where relations survive an
//!                edit.
//!   ASSETS       a named piece built once from primitives and stamped
//!                many times. This is new-primitive creation, and it
//!                already existed -- an `asset` IS a user-defined
//!                primitive, it was just never described as one.
//!   RAW CELLS    explicit coordinates. The escape hatch for when
//!                nothing composes: a face, a logo, a shape from
//!                outside. Expensive in tokens and it does not survive
//!                a resize, which is why it is last and not first.
//!
//! The point of having all three is that a model can DROP A LEVEL when
//! the higher one fails, rather than failing.

use crate::weather::Change;
use std::collections::BTreeSet;

/// One thing tried.
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    pub label: String,
    pub source: String,
    pub cells: BTreeSet<(i32, i32, i32)>,
    pub notes: Vec<String>,
}

/// What can be measured about a structure without knowing what it is
/// meant to be. Deliberately few: a long list of numbers is not
/// feedback, and a model given twenty measures optimises the easiest.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Measures {
    pub cells: usize,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    /// Cells on the outside: how much of it you can actually see.
    pub surface: usize,
    /// Fraction of the bounding box that is filled. Low is airy, high
    /// is a block.
    pub density: f32,
    /// How much the two halves differ, 0 (identical) to 1. PERFECT
    /// SYMMETRY IS A FAULT in anything sculpted -- it is the stiffness
    /// the reference critique named, and nothing was measuring it.
    pub asymmetry: f32,
    /// Distinct heights at which the width changes: a proxy for how
    /// much silhouette there is to look at.
    pub steps: usize,
    /// Connected components. More than one means it is in pieces.
    pub islands: usize,
}

pub fn measure(cells: &BTreeSet<(i32, i32, i32)>) -> Measures {
    if cells.is_empty() { return Measures::default(); }
    let (lo, hi) = cells.iter().fold(
        ((i32::MAX, i32::MAX, i32::MAX), (i32::MIN, i32::MIN, i32::MIN)),
        |(l, h), c| ((l.0.min(c.0), l.1.min(c.1), l.2.min(c.2)),
                     (h.0.max(c.0), h.1.max(c.1), h.2.max(c.2))));
    let (w, ht, d) = (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1, hi.2 - lo.2 + 1);
    let surface = cells.iter().filter(|c| {
        [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)].iter()
            .any(|d| !cells.contains(&(c.0 + d.0, c.1 + d.1, c.2 + d.2)))
    }).count();
    // asymmetry about the x mid-plane, which is where a figure's
    // symmetry lives
    let mid2 = lo.0 + hi.0;
    let mirrored = cells.iter().filter(|c| cells.contains(&(mid2 - c.0, c.1, c.2))).count();
    let asymmetry = 1.0 - mirrored as f32 / cells.len() as f32;
    // silhouette steps: how often the width changes going up
    let mut widths: Vec<i32> = Vec::new();
    for y in lo.1..=hi.1 {
        let at: Vec<i32> = cells.iter().filter(|c| c.1 == y).map(|c| c.0).collect();
        widths.push(if at.is_empty() { 0 } else { at.iter().max().unwrap() - at.iter().min().unwrap() });
    }
    let steps = widths.windows(2).filter(|w| (w[0] - w[1]).abs() > 1).count();
    Measures {
        cells: cells.len(), width: w, height: ht, depth: d, surface,
        density: cells.len() as f32 / (w as f32 * ht as f32 * d as f32).max(1.0),
        asymmetry, steps, islands: islands(cells),
    }
}

fn islands(cells: &BTreeSet<(i32, i32, i32)>) -> usize {
    let mut seen: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    let mut n = 0;
    for start in cells {
        if seen.contains(start) { continue; }
        n += 1;
        if n > 40 { return n; }       // enough to say "it is in pieces"
        let mut stack = vec![*start];
        seen.insert(*start);
        while let Some(c) = stack.pop() {
            for d in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                let p = (c.0 + d.0, c.1 + d.1, c.2 + d.2);
                if cells.contains(&p) && seen.insert(p) { stack.push(p); }
            }
        }
    }
    n
}

/// What changed between two attempts, in words.
#[derive(Debug, Clone, PartialEq)]
pub struct Diff {
    pub added: usize,
    pub removed: usize,
    pub lines: Vec<String>,
    /// Did it get better on the measures that have a right direction?
    /// `None` when nothing moved enough to say.
    pub better: Option<bool>,
}

impl Diff {
    pub fn summary(&self) -> String {
        let verdict = match self.better {
            Some(true) => "BETTER", Some(false) => "WORSE", None => "about the same",
        };
        format!("{verdict}: +{} -{} cells. {}", self.added, self.removed, self.lines.join("; "))
    }
}

/// Compare two attempts. The direction is only asserted where it is
/// genuinely known: more cells is not better, a taller thing is not
/// better, but being in one piece IS better than being in pieces, and
/// perfect symmetry is worse than a little asymmetry in anything
/// sculpted.
pub fn compare(before: &Attempt, after: &Attempt) -> Diff {
    let (a, b) = (measure(&before.cells), measure(&after.cells));
    let added = after.cells.difference(&before.cells).count();
    let removed = before.cells.difference(&after.cells).count();
    let mut lines = Vec::new();
    let mut score = 0i32;
    if b.islands != a.islands {
        lines.push(format!("pieces {} -> {}", a.islands, b.islands));
        // one piece is better than several: a statue in three parts is
        // three objects
        score += if b.islands < a.islands { 2 } else { -2 };
    }
    if (b.asymmetry - a.asymmetry).abs() > 0.02 {
        lines.push(format!("asymmetry {:.0}% -> {:.0}%", a.asymmetry * 100.0, b.asymmetry * 100.0));
        // A LITTLE asymmetry is better than none -- perfect mirror
        // symmetry is the stiffness. Too much is a mistake.
        let good = |x: f32| if (0.03..0.35).contains(&x) { 1 } else { 0 };
        score += good(b.asymmetry) - good(a.asymmetry);
    }
    if b.steps != a.steps {
        lines.push(format!("silhouette steps {} -> {}", a.steps, b.steps));
        // more places where the width changes is more to look at
        score += if b.steps > a.steps { 1 } else { -1 };
    }
    if (b.density - a.density).abs() > 0.04 {
        lines.push(format!("density {:.0}% -> {:.0}%", a.density * 100.0, b.density * 100.0));
        // a solid block is not a sculpture; neither is a haze
        let good = |x: f32| if (0.15..0.75).contains(&x) { 1 } else { 0 };
        score += good(b.density) - good(a.density);
    }
    if b.cells != a.cells {
        lines.push(format!("{} -> {} cells", a.cells, b.cells));
    }
    if lines.is_empty() { lines.push("nothing measurable changed".into()); }
    Diff { added, removed, lines, better: match score { 0 => None, s => Some(s > 0) } }
}

/// A bench: attempts in order, so a model can see its own progress.
#[derive(Debug, Clone, Default)]
pub struct Bench {
    pub attempts: Vec<Attempt>,
}

impl Bench {
    pub fn push(&mut self, label: &str, source: &str, cells: BTreeSet<(i32, i32, i32)>,
                notes: Vec<String>) {
        self.attempts.push(Attempt { label: label.into(), source: source.into(), cells, notes });
    }

    /// The report a model sees before its next edit. THIS is the thing
    /// that was missing: without it, every round starts from nothing
    /// and the model cannot tell whether its last change helped.
    pub fn report(&self, elevation_cols: usize) -> String {
        let Some(last) = self.attempts.last() else { return "nothing tried yet".into() };
        let m = measure(&last.cells);
        let mut s = format!("ROUND {} -- {}\n", self.attempts.len(), last.label);
        s.push_str(&format!("{} cells, {}w x {}h x {}d, density {:.0}%, {} piece(s), \
                             asymmetry {:.0}%, {} silhouette step(s)\n",
                            m.cells, m.width, m.height, m.depth, m.density * 100.0,
                            m.islands, m.asymmetry * 100.0, m.steps));
        if self.attempts.len() > 1 {
            let d = compare(&self.attempts[self.attempts.len() - 2], last);
            s.push_str(&format!("SINCE LAST ROUND: {}\n", d.summary()));
        }
        // the history, so a plateau is visible as a plateau
        if self.attempts.len() > 2 {
            s.push_str("HISTORY: ");
            for w in self.attempts.windows(2) {
                let d = compare(&w[0], &w[1]);
                s.push_str(match d.better {
                    Some(true) => "+", Some(false) => "-", None => "=",
                });
            }
            s.push('\n');
        }
        s.push_str(&crate::review::elevations(&last.cells, elevation_cols, 14));
        for n in last.notes.iter().take(4) { s.push_str(&format!("\n{n}")); }
        s
    }

    /// Has it stopped improving? Three rounds without a `better` is a
    /// plateau, and continuing to spend model turns on it is waste.
    pub fn plateaued(&self) -> bool {
        if self.attempts.len() < 4 { return false; }
        self.attempts.windows(2).rev().take(3)
            .all(|w| compare(&w[0], &w[1]).better != Some(true))
    }

    /// The best attempt by the same judgement `compare` uses, so a run
    /// that wandered can be rolled back to its high point rather than
    /// ending wherever it happened to stop.
    pub fn best(&self) -> Option<&Attempt> {
        let mut best = self.attempts.first()?;
        for a in self.attempts.iter().skip(1) {
            if compare(best, a).better == Some(true) { best = a; }
        }
        Some(best)
    }
}

// ── raw cells: the escape hatch ──────────────────────────────────────

/// Explicit coordinates, for when nothing composes.
///
/// A face, a logo, a shape from outside the language. Costly in tokens
/// and it does not survive a resize, which is why it is the LAST
/// resort and not the first -- but a model that cannot express
/// something at all is worse off than one that can spell it out.
///
///   cells stone 0,0,0 1,0,0 2,0,0 2,1,0
///   cells oak_planks -1,4,2 0,4,2
pub fn parse_cells(src: &str) -> (Vec<Change>, Vec<String>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let mut w = t.split_whitespace();
        match w.next().map(|v| v.to_lowercase()) {
            Some(v) if v == "cells" || v == "blocks" => {}
            _ => continue,
        }
        let Some(mat) = w.next() else { errs.push("cells needs a material".into()); continue };
        let (block, why) = crate::palette::resolve_or_suggest(mat);
        if let Some(w) = why { errs.push(w); }
        let mut n = 0;
        for tok in w {
            let p: Vec<i32> = tok.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if p.len() != 3 {
                errs.push(format!("'{tok}' is not x,y,z"));
                continue;
            }
            out.push(Change { at: (p[0], p[1], p[2]), to: block });
            n += 1;
        }
        if n == 0 { errs.push(format!("cells {mat} listed no positions")); }
    }
    (out, errs)
}

/// What to tell a model about the three levels.
pub fn levels_prompt() -> &'static str {
    "\
THREE LEVELS, and you may drop one when the level above will not say what you mean:

  1  PARTS AND RELATIONS   part x sphere r=6 material=stone / rel x on ground
     Use this. Relations survive an edit -- change one size and everything moves with it.

  2  ASSETS                asset window ... end, then repeat window ring count=8
     A piece you design ONCE and place many times. This is how you make your own
     primitive: build it out of parts, name it, and use the name.

  3  RAW CELLS             cells stone 0,0,0 1,0,0 2,1,0
     Exact positions, for what the other two cannot say -- a face, a symbol, a shape
     with no name. Expensive, and it will not resize with the rest. Last resort."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_at(x0: i32, y0: i32, z0: i32, w: i32, h: i32, d: i32) -> BTreeSet<(i32, i32, i32)> {
        let mut s = BTreeSet::new();
        for x in x0..x0 + w { for y in y0..y0 + h { for z in z0..z0 + d { s.insert((x, y, z)); } } }
        s
    }

    #[test]
    fn a_model_can_see_whether_its_last_change_helped() {
        // Every review round showed the CURRENT structure and asked
        // what to change, never the previous one. A model editing blind
        // repeats itself and undoes its own improvements.
        let mut b = Bench::default();
        // two separate lumps: in pieces
        let mut split = box_at(0, 64, 0, 6, 6, 6);
        split.extend(box_at(20, 64, 0, 6, 6, 6));
        b.push("first try", "...", split, vec![]);
        // joined: better
        let mut joined = box_at(0, 64, 0, 6, 6, 6);
        joined.extend(box_at(20, 64, 0, 6, 6, 6));
        for x in 6..20 { joined.insert((x, 66, 3)); }
        b.push("joined them", "...", joined, vec![]);
        let d = compare(&b.attempts[0], &b.attempts[1]);
        assert_eq!(d.better, Some(true), "{}", d.summary());
        assert!(d.summary().contains("pieces 2 -> 1"), "{}", d.summary());
        assert!(d.added > 0 && d.removed == 0);
        let r = b.report(24);
        assert!(r.contains("ROUND 2") && r.contains("SINCE LAST ROUND: BETTER"), "{r}");
        assert!(r.contains("piece(s)") && r.contains("asymmetry"));
    }

    #[test]
    fn perfect_symmetry_is_measured_as_a_fault() {
        // the reference critique's first complaint about its own statue
        // was "the pose is too symmetrical", and NOTHING was measuring
        // it -- `pair` mirrors exactly, so every figure scored perfect
        let mirror = box_at(-5, 64, 0, 11, 10, 4);
        assert!(measure(&mirror).asymmetry < 0.01, "a box is perfectly symmetric");
        // A shape that MOVES is still symmetric about its own new
        // centre, which is right and is what the first version of this
        // test got wrong. Asymmetry is about the shape being lopsided,
        // so put an arm on one side only.
        let mut lean = mirror.clone();
        for y in 70..74 { for x in 6..12 { for z in 0..4 { lean.insert((x, y, z)); } } }
        let m = measure(&lean);
        assert!(m.asymmetry > 0.05, "a leaning figure is not mirror-symmetric: {}", m.asymmetry);
        // and a LITTLE asymmetry scores better than none
        let a = Attempt { label: "mirror".into(), source: String::new(), cells: mirror, notes: vec![] };
        let c = Attempt { label: "lean".into(), source: String::new(), cells: lean, notes: vec![] };
        assert_eq!(compare(&a, &c).better, Some(true), "{}", compare(&a, &c).summary());
    }

    #[test]
    fn a_plateau_is_visible_as_a_plateau() {
        let mut b = Bench::default();
        for i in 0..5 {
            // the same shape, nudged: nothing improves
            let mut c = box_at(i % 2, 64, 0, 8, 8, 8);
            c.insert((30 + i, 64, 0));
            b.push(&format!("try {i}"), "...", c, vec![]);
        }
        assert!(b.plateaued(), "five rounds of nothing is a plateau");
        let r = b.report(20);
        assert!(r.contains("HISTORY:"), "{r}");
        // and a run that IS improving is not a plateau
        let mut good = Bench::default();
        let mut cells = box_at(0, 64, 0, 4, 4, 4);
        for i in 0..5 {
            cells.extend(box_at(0, 64 + 4 + i * 2, i, 4 - (i % 2), 2, 4));
            good.push(&format!("grow {i}"), "...", cells.clone(), vec![]);
        }
        assert!(!good.plateaued(), "it is still changing for the better");
    }

    #[test]
    fn the_best_attempt_is_recoverable_when_a_run_wanders() {
        let mut b = Bench::default();
        let mut split = box_at(0, 64, 0, 5, 5, 5);
        split.extend(box_at(20, 64, 0, 5, 5, 5));
        b.push("pieces", "a", split.clone(), vec![]);
        let mut whole = split.clone();
        for x in 5..20 { whole.insert((x, 66, 2)); }
        b.push("joined", "b", whole, vec![]);
        b.push("broke it again", "c", split, vec![]);
        let best = b.best().expect("something");
        assert_eq!(best.source, "b", "the high point, not wherever it stopped");
    }

    #[test]
    fn raw_cells_are_the_escape_hatch_and_they_work() {
        // a model that cannot express something at all is worse off
        // than one that can spell it out
        let (ch, errs) = parse_cells("cells stone 0,64,0 1,64,0 2,64,0 2,65,0\n\
                                      cells oak_planks -1,66,2 0,66,2\n");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(ch.len(), 6);
        assert_eq!(ch[0].at, (0, 64, 0));
        assert_ne!(ch[0].to, ch[4].to, "two materials");
        // and it says what is wrong rather than dropping it
        // both malformed tokens are named, AND the line is reported as
        // having placed nothing -- three complaints, all true
        let (_, e2) = parse_cells("cells stone 0,64 1,2,3,4\n");
        assert_eq!(e2.len(), 3, "{e2:?}");
        assert_eq!(e2.iter().filter(|e| e.contains("not x,y,z")).count(), 2, "{e2:?}");
        assert!(e2.iter().any(|e| e.contains("listed no positions")), "{e2:?}");
        let (_, e3) = parse_cells("cells stone\n");
        assert!(e3.iter().any(|e| e.contains("listed no positions")), "{e3:?}");
        // an unknown material is substituted, as everywhere else
        let (ch4, e4) = parse_cells("cells deepslate 0,64,0\n");
        assert_eq!(ch4.len(), 1);
        assert!(e4.iter().any(|e| e.contains("deepslate")), "{e4:?}");
    }

    #[test]
    fn the_three_levels_are_described_in_the_order_they_should_be_tried() {
        let p = levels_prompt();
        let (parts, assets, raw) = (p.find("PARTS AND RELATIONS").unwrap(),
                                    p.find("ASSETS").unwrap(), p.find("RAW CELLS").unwrap());
        assert!(parts < assets && assets < raw, "relations first, raw cells last");
        assert!(p.contains("Relations survive an edit"), "it says WHY: {p}");
        assert!(p.contains("your own"), "an asset is user-defined primitive creation");
        assert!(p.contains("Last resort"));
        assert!(p.len() < 900, "{} chars", p.len());
    }

    #[test]
    fn measures_stay_few_because_a_long_list_is_not_feedback() {
        // a model given twenty measures optimises the easiest one
        let m = measure(&box_at(0, 64, 0, 10, 10, 10));
        assert_eq!(m.cells, 1000);
        assert_eq!((m.width, m.height, m.depth), (10, 10, 10));
        assert_eq!(m.islands, 1);
        assert!((m.density - 1.0).abs() < 0.01, "a solid box is fully dense");
        assert!(m.surface < m.cells, "the inside is not surface");
        assert_eq!(measure(&BTreeSet::new()).cells, 0, "empty is not a panic");
    }
}
