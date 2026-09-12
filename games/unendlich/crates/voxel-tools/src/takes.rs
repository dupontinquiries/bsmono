//! Three takes, not one answer.
//!
//! The same request builds the same thing every time. Ask for a
//! lighthouse twice and get the identical lighthouse, because the seed
//! is fixed, the typology is fixed, and every choice below them is
//! arithmetic. That is a good property for a compiler and a poor one
//! for a design tool: nobody's first idea is their best, and a builder
//! working alone tries three things before keeping one.
//!
//! `evolve::diverse` has existed for a while and picks "three
//! different good ones" out of a Pareto front. It was only ever used
//! on greebles -- a decoration pattern -- and never on a whole build,
//! which is where having options actually matters.
//!
//! WHAT MAKES A TAKE DIFFERENT
//!
//! Not noise. Changing a seed gives three builds that differ in ways
//! nobody asked about and nobody can choose between. A take differs by
//! a DECISION that could reasonably have gone another way:
//!
//!   MASSING     one tall thing, or several low ones
//!   SYMMETRY    mirrored, or weighted to one side
//!   DENSITY     solid and heavy, or open and framed
//!   MATERIAL    one material, or a mass with contrasting trim
//!   DETAIL      plain surfaces, or worked ones
//!
//! Each is a knob a builder would actually turn, and each produces a
//! build somebody could prefer for a reason they could state. Three
//! takes that differ only in seed are one take shown three times.

use crate::bench::{self, Measures};

/// A decision that could reasonably have gone another way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis { Massing, Symmetry, Density, Material, Detail }

impl Axis {
    pub fn all() -> [Axis; 5] {
        [Axis::Massing, Axis::Symmetry, Axis::Density, Axis::Material, Axis::Detail]
    }
    pub fn name(self) -> &'static str {
        match self { Axis::Massing => "massing", Axis::Symmetry => "symmetry",
                     Axis::Density => "density", Axis::Material => "material",
                     Axis::Detail => "detail" }
    }
    /// The two ends of the choice, in words a person would use.
    pub fn ends(self) -> (&'static str, &'static str) {
        match self {
            Axis::Massing => ("one tall mass", "several low masses"),
            Axis::Symmetry => ("mirrored and formal", "weighted to one side"),
            Axis::Density => ("solid and heavy", "open and framed"),
            Axis::Material => ("one material throughout", "a mass with contrasting trim"),
            Axis::Detail => ("plain surfaces", "worked and ornamented"),
        }
    }
}

/// One take: how it was decided, and what it came out as.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    pub label: String,
    /// Where this take sits on each axis, -1 to 1.
    pub leans: Vec<(Axis, f32)>,
    pub source: String,
    pub measures: Measures,
}

impl Take {
    /// What makes this one different, said plainly. A player choosing
    /// between three builds needs to know what they are choosing
    /// BETWEEN, not just see three pictures.
    pub fn why(&self) -> String {
        let mut v: Vec<String> = self.leans.iter()
            .filter(|(_, l)| l.abs() > 0.3)
            .map(|(a, l)| {
                let (low, high) = a.ends();
                (if *l > 0.0 { high } else { low }).to_string()
            })
            .collect();
        v.truncate(3);
        if v.is_empty() { "the plain reading".into() } else { v.join(", ") }
    }
}

/// How far apart two takes are, on the axes that were varied. Used to
/// keep three takes from collapsing into one.
pub fn distance(a: &Take, b: &Take) -> f32 {
    // AN AXIS ONE LEANS ON AND THE OTHER DOES NOT IS A DIFFERENCE, and
    // summing only over SHARED axes made two takes leaning on entirely
    // different things score zero apart -- maximally different, read as
    // identical. Every axis counts, with an absent lean as zero.
    let lean = |t: &Take, ax: Axis| t.leans.iter()
        .find(|(x, _)| *x == ax).map(|(_, l)| *l).unwrap_or(0.0);
    Axis::all().iter().map(|ax| (lean(a, *ax) - lean(b, *ax)).powi(2)).sum::<f32>().sqrt()
}

/// The instructions for one take, appended to whatever prompt is being
/// asked. Deliberately short: a take is a LEAN, not a different brief.
pub fn lean_prompt(leans: &[(Axis, f32)]) -> String {
    let said: Vec<String> = leans.iter().filter(|(_, l)| l.abs() > 0.3)
        .map(|(a, l)| {
            let (low, high) = a.ends();
            format!("  {}: {}", a.name(), if *l > 0.0 { high } else { low })
        }).collect();
    if said.is_empty() { return String::new(); }
    format!("\nTHIS TAKE LEANS:\n{}\nSame request, this reading of it. Commit to the lean \
             rather than hedging -- three cautious takes are one take shown three times.\n",
            said.join("\n"))
}

/// Choose the leans for `n` takes so they differ on the axes that
/// matter and agree everywhere else.
///
/// Deterministic given a seed, because a player who liked take two
/// should be able to get take two again.
pub fn spread(n: usize, seed: u64) -> Vec<Vec<(Axis, f32)>> {
    let n = n.clamp(1, 5);
    let axes = Axis::all();
    let mut out = Vec::new();
    for i in 0..n {
        // The FIRST take is always the plain reading: someone who did
        // not want options should not have to look past the first one
        // to find the obvious answer.
        if i == 0 { out.push(Vec::new()); continue; }
        let mut leans = Vec::new();
        // each take leans on two axes, chosen so no two takes share
        // both, and in opposite directions where they overlap
        for k in 0..2 {
            let ax = axes[((i * 2 + k) as usize + (seed as usize % 3)) % axes.len()];
            let dir = if (i + k) % 2 == 0 { 1.0 } else { -1.0 };
            leans.push((ax, dir));
        }
        out.push(leans);
    }
    out
}

/// Rank finished takes: keep the ones that are good AND unlike each
/// other, which is what makes three of them worth showing.
pub fn choose(takes: Vec<Take>, n: usize) -> Vec<Take> {
    if takes.len() <= n { return takes; }
    let quality = |t: &Take| -> f32 {
        let m = &t.measures;
        // the same judgements the bench makes, since they are the only
        // ones this crate can defend
        let mut q = 0.0;
        if m.islands == 1 { q += 2.0; }
        if (0.03..0.35).contains(&m.asymmetry) { q += 1.0; }
        if (0.15..0.75).contains(&m.density) { q += 1.0; }
        q += (m.steps as f32 / 20.0).min(1.0);
        q
    };
    let mut pool = takes;
    pool.sort_by(|a, b| quality(b).partial_cmp(&quality(a)).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = vec![pool.remove(0)];
    while out.len() < n && !pool.is_empty() {
        // the one most unlike everything already chosen, among those
        // still decent -- a bad take is not worth showing however
        // different it is
        let best = pool.iter().enumerate()
            .max_by(|(_, a), (_, b)| {
                let da = out.iter().map(|o| distance(o, a)).fold(f32::MAX, f32::min) + quality(a);
                let db = out.iter().map(|o| distance(o, b)).fold(f32::MAX, f32::min) + quality(b);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i);
        match best { Some(i) => out.push(pool.remove(i)), None => break }
    }
    out
}

/// Measure a take from the cells it produced.
pub fn measure_take(label: &str, leans: Vec<(Axis, f32)>, source: &str,
                    cells: &std::collections::BTreeSet<(i32, i32, i32)>) -> Take {
    Take { label: label.into(), leans, source: source.into(), measures: bench::measure(cells) }
}

/// What to show a player who has three builds in front of them.
pub fn offer(takes: &[Take]) -> String {
    let mut s = format!("{} take(s):\n", takes.len());
    for (i, t) in takes.iter().enumerate() {
        s.push_str(&format!("  {}. {} -- {} ({} cells, {}w x {}h)\n",
                            i + 1, t.label, t.why(), t.measures.cells,
                            t.measures.width, t.measures.height));
    }
    s.push_str("/pick 1, 2 or 3.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn cells(w: i32, h: i32, d: i32) -> BTreeSet<(i32, i32, i32)> {
        let mut s = BTreeSet::new();
        for x in 0..w { for y in 64..64 + h { for z in 0..d { s.insert((x, y, z)); } } }
        s
    }

    #[test]
    fn takes_differ_by_a_decision_not_by_noise() {
        // three builds that differ only in seed are one take shown
        // three times: nobody can choose between them for a reason
        let s = spread(3, 7);
        assert_eq!(s.len(), 3);
        assert!(s[0].is_empty(), "the first take is the plain reading");
        assert!(!s[1].is_empty() && !s[2].is_empty());
        // and the leans really differ
        let t = |leans: Vec<(Axis, f32)>| Take {
            label: "t".into(), leans, source: String::new(), measures: Measures::default() };
        assert!(distance(&t(s[1].clone()), &t(s[2].clone())) > 0.5,
                "{:?} vs {:?}", s[1], s[2]);
    }

    #[test]
    fn a_take_says_what_makes_it_different_in_words() {
        // a player choosing between three builds needs to know what
        // they are choosing BETWEEN, not just see three pictures
        let t = Take { label: "b".into(),
                       leans: vec![(Axis::Massing, 1.0), (Axis::Density, -1.0)],
                       source: String::new(), measures: Measures::default() };
        let w = t.why();
        assert!(w.contains("several low masses") && w.contains("solid and heavy"), "{w}");
        // the plain reading says so rather than inventing a reason
        let plain = Take { label: "a".into(), leans: vec![], source: String::new(),
                           measures: Measures::default() };
        assert_eq!(plain.why(), "the plain reading");
        // and a lean too small to matter is not mentioned
        let faint = Take { label: "c".into(), leans: vec![(Axis::Detail, 0.1)],
                           source: String::new(), measures: Measures::default() };
        assert_eq!(faint.why(), "the plain reading");
    }

    #[test]
    fn the_lean_prompt_asks_for_commitment_not_hedging() {
        let p = lean_prompt(&[(Axis::Symmetry, 1.0), (Axis::Detail, -1.0)]);
        assert!(p.contains("symmetry: weighted to one side"), "{p}");
        assert!(p.contains("detail: plain surfaces"), "{p}");
        assert!(p.contains("Commit to the lean"), "{p}");
        assert!(p.contains("three cautious takes are one take shown three times"));
        assert!(p.len() < 400, "a take is a LEAN, not a different brief: {} chars", p.len());
        // no lean, no words
        assert!(lean_prompt(&[]).is_empty());
    }

    #[test]
    fn choosing_keeps_the_good_ones_that_are_unlike_each_other() {
        // a bad take is not worth showing however different it is
        let mk = |name: &str, leans: Vec<(Axis, f32)>, c: BTreeSet<(i32, i32, i32)>|
            measure_take(name, leans, "", &c);
        let good_a = mk("a", vec![(Axis::Massing, 1.0)], cells(12, 20, 12));
        let good_b = mk("b", vec![(Axis::Massing, -1.0)], cells(30, 8, 30));
        // in pieces: bad however different
        let mut broken = cells(6, 6, 6);
        broken.extend(cells(6, 6, 6).iter().map(|c| (c.0 + 40, c.1, c.2)));
        let bad = mk("c", vec![(Axis::Density, 1.0)], broken);
        let picked = choose(vec![good_a, bad, good_b], 2);
        assert_eq!(picked.len(), 2);
        let names: Vec<&str> = picked.iter().map(|t| t.label.as_str()).collect();
        assert!(names.contains(&"a") && names.contains(&"b"), "{names:?}");
        assert!(!names.contains(&"c"), "the broken one is not shown: {names:?}");
        // fewer takes than asked for is not an error
        assert_eq!(choose(vec![mk("x", vec![], cells(4, 4, 4))], 3).len(), 1);
    }

    #[test]
    fn the_same_seed_gives_the_same_takes() {
        // a player who liked take two should be able to get take two
        assert_eq!(spread(3, 11), spread(3, 11));
        assert_ne!(spread(3, 11), spread(3, 12));
    }

    #[test]
    fn the_offer_says_what_each_one_is() {
        let t = vec![
            measure_take("plain", vec![], "", &cells(10, 10, 10)),
            measure_take("low", vec![(Axis::Massing, -1.0)], "", &cells(24, 5, 24)),
        ];
        let o = offer(&t);
        assert!(o.contains("2 take(s)"), "{o}");
        assert!(o.contains("the plain reading") && o.contains("one tall mass"), "{o}");
        assert!(o.contains("cells") && o.contains("/pick"), "{o}");
    }
}
