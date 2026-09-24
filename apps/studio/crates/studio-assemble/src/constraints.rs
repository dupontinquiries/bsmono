//! constraints — what the story must satisfy, from two kinds of evidence, and
//! whether a timeline satisfies it: a CAD sketch's constraint panel, for a cut.
//!
//! A CAD program does not guess a part; it holds the dimensions and relations
//! the drawing states, solves for what they leave free, and shows which ones a
//! change violates. A story has the same shape. Some relations are STATED --
//! by the script, the shot list, the song's lyric sheet, a note the director
//! wrote -- and some are merely EVIDENCED by the footage. They are kept apart:
//!
//! * **Documents give hard constraints** ([`Strength::Hard`]): [`from_outline`]
//!   (each scene block's lines in order), [`from_lrc`] (a lyric line pinned to
//!   its time in the song, give or take), and [`parse`] -- a plain text file of
//!   explicit relations anyone can write:
//!   ```text
//!   C0031 before C0035            # clips, by file stem
//!   "are you ready" follows "the show opens friday"
//!   scene N7 at 3:10 ±2           # a scene label, in seconds
//!   total <= 20:00
//!   ```
//! * **Transcripts give soft ones, cheaply** ([`infer`], [`Strength::Soft`]):
//!   if every take that says lines A and B says A first, A is before B; takes
//!   that disagree give nothing. A question followed in the same take by
//!   another speaker's line is followed by it. That is pairwise over each
//!   take's lines, no model -- not a story, but it RULES OUT orders the
//!   footage contradicts, which is most of what a first cut gets wrong.
//!
//! [`check`] evaluates every constraint against what is on the timeline:
//! satisfied, violated (by how much), or unresolved (an anchor not placed).
//! Hard `before`/`follows` constraints that form a cycle are OVER-CONSTRAINED
//! and named with every edge's source; a soft inference that would close a
//! cycle with hard ones is DROPPED and reported (the footage disagreeing with
//! the script is information, not a vote). [`order`] sorts items under the
//! constraints: hard first, soft as tie-breaks, original order last.

use std::collections::{BTreeMap, BTreeSet};

use studio_core::Ticks;

use crate::{similarity, Take};

/// What a constraint is about.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Anchor {
    /// A spoken line, matched by what is said.
    Line(String),
    /// A clip, by its source's file stem.
    Clip(String),
    /// A scene, by label.
    Scene(String),
}

impl Anchor {
    pub fn describe(&self) -> String {
        match self { Anchor::Line(t) => format!("\"{}\"", crate::short(t, 32)), Anchor::Clip(c) => c.clone(),
                     Anchor::Scene(s) => format!("scene {s}") }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength { Soft, Hard }

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// All of `a` ends before any of `b` starts.
    Before(Anchor, Anchor),
    /// `b` starts right after `a` ends (within half a second).
    Follows(Anchor, Anchor),
    /// `a` starts at `t`, give or take `tol`.
    At(Anchor, Ticks, Ticks),
    /// The timeline is no longer than this.
    Total(Ticks),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Constraint {
    pub kind: Kind,
    pub strength: Strength,
    /// Where it came from: "script.txt:12", "transcripts: 3 takes agree".
    pub source: String,
}

impl Constraint {
    fn precedence(&self) -> Option<(&Anchor, &Anchor)> {
        match &self.kind { Kind::Before(a, b) | Kind::Follows(a, b) => Some((a, b)), _ => None }
    }
    pub fn describe(&self) -> String {
        match &self.kind {
            Kind::Before(a, b) => format!("{} before {}", a.describe(), b.describe()),
            Kind::Follows(a, b) => format!("{} right after {}", b.describe(), a.describe()),
            Kind::At(a, t, tol) => format!("{} at {:.1} s ±{:.1}", a.describe(), t.to_seconds(), tol.to_seconds()),
            Kind::Total(t) => format!("total ≤ {:.1} s", t.to_seconds()),
        }
    }
}

// ── documents ───────────────────────────────────────────────────────────

/// `mm:ss`, `h:mm:ss` or plain seconds (fractions allowed).
pub fn parse_time(s: &str) -> Option<Ticks> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 { return None; }
    let mut secs = 0.0f64;
    for p in &parts { secs = secs * 60.0 + p.trim().parse::<f64>().ok()?; }
    (secs >= 0.0).then(|| Ticks::from_seconds(secs))
}

fn anchor(s: &str) -> Option<Anchor> {
    let s = s.trim();
    if let Some(q) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) { return Some(Anchor::Line(q.to_string())); }
    if let Some(sc) = s.strip_prefix("scene ") { return Some(Anchor::Scene(sc.trim().to_string())); }
    (!s.is_empty() && !s.contains(' ')).then(|| Anchor::Clip(s.to_string()))
}

/// The constraints file. Returns the constraints and, per line that could not
/// be read, `(line number, text)` -- said back, never guessed at.
pub fn parse(text: &str, doc: &str) -> (Vec<Constraint>, Vec<(usize, String)>) {
    let (mut out, mut bad) = (Vec::new(), Vec::new());
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() { continue; }
        let src = format!("{doc}:{}", i + 1);
        let hard = |kind| Constraint { kind, strength: Strength::Hard, source: src.clone() };
        let kind = if let Some(rest) = line.strip_prefix("total") {
            rest.trim().strip_prefix("<=").and_then(parse_time).map(Kind::Total)
        } else if let Some((a, b)) = line.split_once(" before ") {
            anchor(a).zip(anchor(b)).map(|(a, b)| Kind::Before(a, b))
        } else if let Some((b, a)) = line.split_once(" follows ") {
            anchor(a).zip(anchor(b)).map(|(a, b)| Kind::Follows(a, b))
        } else if let Some((a, t)) = line.rsplit_once(" at ") {
            let (t, tol) = match t.split_once('±').or_else(|| t.split_once("+-")) {
                Some((t, tol)) => (parse_time(t), parse_time(tol)),
                None => (parse_time(t), Some(Ticks::from_seconds(1.0))),
            };
            anchor(a).zip(t).zip(tol).map(|((a, t), tol)| Kind::At(a, t, tol))
        } else { None };
        match kind { Some(k) => out.push(hard(k)), None => bad.push((i + 1, raw.to_string())) }
    }
    (out, bad)
}

/// An outline's lines in order, within each `#` block (a scene): consecutive
/// lines `before` each other. Blocks are NOT chained -- scene order is what an
/// editor moves -- unless `chain_scenes`.
pub fn from_outline(text: &str, doc: &str, chain_scenes: bool) -> Vec<Constraint> {
    let mut out = Vec::new();
    let mut prev: Option<(String, usize)> = None;
    for (i, raw) in text.lines().enumerate() {
        let l = raw.trim();
        if l.starts_with('#') { if !chain_scenes && l.starts_with("# ──") { prev = None; } continue; }
        let Some(line) = crate::parse_outline(l).into_iter().next() else { continue };
        if let Some((p, pi)) = prev.take() {
            out.push(Constraint { kind: Kind::Before(Anchor::Line(p), Anchor::Line(line.clone())),
                                  strength: Strength::Hard, source: format!("{doc}:{pi}-{}", i + 1) });
        }
        prev = Some((line, i + 1));
    }
    out
}

/// A `.lrc` lyric sheet: `[mm:ss.xx] words` pins each line to its time, ±`tol`.
pub fn from_lrc(text: &str, doc: &str, tol: Ticks) -> Vec<Constraint> {
    text.lines().enumerate().filter_map(|(i, l)| {
        let rest = l.trim().strip_prefix('[')?;
        let (t, words) = rest.split_once(']')?;
        let t = parse_time(t)?;
        let words = words.trim();
        (!words.is_empty()).then(|| Constraint { kind: Kind::At(Anchor::Line(words.to_string()), t, tol),
                                                 strength: Strength::Hard, source: format!("{doc}:{}", i + 1) })
    }).collect()
}

// ── transcripts: the cheap ruling-out ───────────────────────────────────

const SAME: f32 = 0.75;

/// Soft constraints the footage evidences: a pair of lines every take that
/// says both says in the same order (at least 2 takes, or 1 with nothing
/// against it -- said, with its count), and a question answered by another
/// speaker in the same take.
pub fn infer(takes: &[Take], noise: &dyn Fn(&str) -> bool) -> Vec<Constraint> {
    // canonical lines: the first saying of each distinct line
    let mut canon: Vec<String> = Vec::new();
    let mut per_take: Vec<Vec<(usize, Option<String>)>> = Vec::new(); // (canon index, speaker)
    for t in takes {
        let lines = crate::clean(t, noise);
        let mut seq = Vec::new();
        for l in lines.iter().filter(|l| l.dropped.is_none()) {
            let k = match canon.iter().position(|c| similarity(c, &l.text) >= SAME) {
                Some(k) => k,
                None => { canon.push(l.text.trim().to_string()); canon.len() - 1 }
            };
            if !seq.iter().any(|(x, _)| *x == k) { seq.push((k, t.segments[l.index].speaker.clone())); }
        }
        per_take.push(seq);
    }
    let mut votes: BTreeMap<(usize, usize), (u32, u32)> = BTreeMap::new(); // (a<b) -> (a first, b first)
    let mut out = Vec::new();
    for seq in &per_take {
        for i in 0..seq.len() {
            for j in i + 1..seq.len() {
                let (a, b) = (seq[i].0, seq[j].0);
                let e = votes.entry((a.min(b), a.max(b))).or_default();
                if a < b { e.0 += 1 } else { e.1 += 1 }
            }
            // a question, answered by someone else, next
            if let Some(next) = seq.get(i + 1) {
                let q = &canon[seq[i].0];
                if q.trim_end().ends_with('?') && seq[i].1.is_some() && next.1.is_some() && seq[i].1 != next.1 {
                    out.push(Constraint { kind: Kind::Follows(Anchor::Line(q.clone()), Anchor::Line(canon[next.0].clone())),
                                          strength: Strength::Soft, source: "transcripts: a question and its answer".into() });
                }
            }
        }
    }
    for ((a, b), (ab, ba)) in votes {
        let (first, second, n) = match (ab, ba) { (n, 0) if n > 0 => (a, b, n), (0, n) if n > 0 => (b, a, n), _ => continue };
        out.push(Constraint { kind: Kind::Before(Anchor::Line(canon[first].clone()), Anchor::Line(canon[second].clone())),
                              strength: Strength::Soft,
                              source: format!("transcripts: {n} take{} agree{}, none against", if n == 1 { "" } else { "s" },
                                              if n == 1 { "s" } else { "" }) });
    }
    out
}

// ── the timeline, as the checker sees it ────────────────────────────────

/// One placed piece: its names (clip stem, scene labels), the lines it says,
/// and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed { pub names: Vec<String>, pub lines: Vec<String>, pub at: Ticks, pub end: Ticks }

fn matches(a: &Anchor, p: &Placed) -> bool {
    match a {
        Anchor::Line(t) => p.lines.iter().any(|l| similarity(l, t) >= SAME),
        Anchor::Clip(c) | Anchor::Scene(c) => p.names.iter().any(|n| n.eq_ignore_ascii_case(c)),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Satisfied,
    /// Off by this much (seconds): overlap for before/follows, distance for at/total.
    Violated(f64),
    /// These anchors are not on the timeline.
    Unresolved(Vec<Anchor>),
    /// A soft constraint set aside because it contradicts hard ones.
    Dropped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Every constraint with its status, in the order given.
    pub statuses: Vec<(Constraint, Status)>,
    /// Cycles among HARD precedence constraints: the documents contradict
    /// each other. Each is the constraints around the loop.
    pub conflicts: Vec<Vec<Constraint>>,
    /// Placed pieces no constraint touches: the free degrees of freedom.
    pub free: Vec<usize>,
}

impl Report {
    pub fn violated(&self) -> impl Iterator<Item = &(Constraint, Status)> {
        self.statuses.iter().filter(|(_, s)| matches!(s, Status::Violated(_)))
    }
    pub fn summary(&self) -> String {
        let n = |f: fn(&Status) -> bool| self.statuses.iter().filter(|(_, s)| f(s)).count();
        let mut s = format!("{} satisfied, {} violated, {} unresolved", n(|s| matches!(s, Status::Satisfied)),
                            n(|s| matches!(s, Status::Violated(_))), n(|s| matches!(s, Status::Unresolved(_))));
        let dropped = n(|s| matches!(s, Status::Dropped));
        if dropped > 0 { s.push_str(&format!(", {dropped} transcript inference{} set aside against the documents", if dropped == 1 { "" } else { "s" })); }
        if !self.conflicts.is_empty() { s.push_str(&format!(", {} conflict{} between documents", self.conflicts.len(), if self.conflicts.len() == 1 { "" } else { "s" })); }
        if !self.free.is_empty() { s.push_str(&format!(", {} piece{} unconstrained", self.free.len(), if self.free.len() == 1 { "" } else { "s" })); }
        s
    }
}

/// Precedence edges as a graph over anchors; `Some(path)` = a cycle through
/// the edges given, as indices into `edges`.
fn find_cycle(edges: &[(&Anchor, &Anchor, usize)]) -> Option<Vec<usize>> {
    let nodes: BTreeSet<&Anchor> = edges.iter().flat_map(|(a, b, _)| [*a, *b]).collect();
    // DFS with colours
    let mut colour: BTreeMap<&Anchor, u8> = nodes.iter().map(|n| (*n, 0u8)).collect();
    fn dfs<'a>(n: &'a Anchor, edges: &[(&'a Anchor, &'a Anchor, usize)], colour: &mut BTreeMap<&'a Anchor, u8>,
               path: &mut Vec<(usize, &'a Anchor)>) -> Option<Vec<usize>> {
        colour.insert(n, 1);
        for (a, b, i) in edges.iter().filter(|(a, _, _)| *a == n) {
            let _ = a;
            match colour[b] {
                1 => {
                    // the loop: from where `b` was entered, to here, plus this edge
                    let start = path.iter().position(|(_, from)| from == b).unwrap_or(0);
                    let mut cyc: Vec<usize> = path[start..].iter().map(|(e, _)| *e).collect();
                    cyc.push(*i);
                    return Some(cyc);
                }
                0 => {
                    path.push((*i, n));
                    if let Some(c) = dfs(b, edges, colour, path) { return Some(c); }
                    path.pop();
                }
                _ => {}
            }
        }
        colour.insert(n, 2);
        None
    }
    for n in nodes {
        if colour[n] == 0 {
            if let Some(c) = dfs(n, edges, &mut colour, &mut Vec::new()) { return Some(c); }
        }
    }
    None
}

/// Check `constraints` against the timeline `placed`.
pub fn check(constraints: &[Constraint], placed: &[Placed]) -> Report {
    // ── consistency: hard cycles are conflicts; soft edges closing a cycle are dropped
    let mut conflicts = Vec::new();
    let mut hard: Vec<(&Anchor, &Anchor, usize)> = constraints.iter().enumerate()
        .filter(|(_, c)| c.strength == Strength::Hard).filter_map(|(i, c)| c.precedence().map(|(a, b)| (a, b, i))).collect();
    let mut in_conflict: BTreeSet<usize> = BTreeSet::new();
    while let Some(cyc) = find_cycle(&hard) {
        conflicts.push(cyc.iter().map(|i| constraints[*i].clone()).collect());
        in_conflict.extend(cyc.iter().copied());
        hard.retain(|(_, _, i)| !cyc.contains(i)); // report it once, then look for others
    }
    let mut kept = hard.clone();
    let mut dropped: BTreeSet<usize> = BTreeSet::new();
    for (i, c) in constraints.iter().enumerate().filter(|(_, c)| c.strength == Strength::Soft) {
        if let Some((a, b)) = c.precedence() {
            kept.push((a, b, i));
            if find_cycle(&kept).is_some() { kept.pop(); dropped.insert(i); }
        }
    }
    // ── evaluation
    let end = placed.iter().map(|p| p.end).max().unwrap_or(Ticks::ZERO);
    let mut touched: BTreeSet<usize> = BTreeSet::new();
    let statuses = constraints.iter().enumerate().map(|(i, c)| {
        if dropped.contains(&i) { return (c.clone(), Status::Dropped); }
        let find = |a: &Anchor| placed.iter().enumerate().filter(|(_, p)| matches(a, p)).map(|(k, _)| k).collect::<Vec<_>>();
        let st = match &c.kind {
            Kind::Total(max) => if end <= *max { Status::Satisfied } else { Status::Violated(end.saturating_sub(*max).to_seconds()) },
            Kind::At(a, t, tol) => {
                let hits = find(a);
                touched.extend(hits.iter().copied());
                match hits.iter().map(|k| (placed[*k].at.0 - t.0).abs()).min() {
                    None => Status::Unresolved(vec![a.clone()]),
                    Some(d) if d <= tol.0 => Status::Satisfied,
                    Some(d) => Status::Violated(Ticks(d).to_seconds()),
                }
            }
            Kind::Before(a, b) | Kind::Follows(a, b) => {
                let (ha, hb) = (find(a), find(b));
                touched.extend(ha.iter().chain(&hb).copied());
                let missing: Vec<Anchor> = [(a, &ha), (b, &hb)].iter().filter(|(_, h)| h.is_empty()).map(|(x, _)| (*x).clone()).collect();
                if !missing.is_empty() { Status::Unresolved(missing) }
                else if matches!(c.kind, Kind::Before(..)) {
                    let a_end = ha.iter().map(|k| placed[*k].end).max().unwrap();
                    let b_at = hb.iter().map(|k| placed[*k].at).min().unwrap();
                    if a_end <= b_at { Status::Satisfied } else { Status::Violated(a_end.saturating_sub(b_at).to_seconds()) }
                } else {
                    let gap = Ticks::from_seconds(0.5);
                    let ok = ha.iter().any(|x| hb.iter().any(|y| placed[*y].at >= placed[*x].end && placed[*y].at.saturating_sub(placed[*x].end) <= gap));
                    if ok { Status::Satisfied } else {
                        let best = ha.iter().flat_map(|x| hb.iter().map(move |y| (x, y)))
                            .map(|(x, y)| (placed[*y].at.0 - placed[*x].end.0).abs()).min().unwrap();
                        Status::Violated(Ticks(best).to_seconds())
                    }
                }
            }
        };
        (c.clone(), st)
    }).collect();
    let _ = in_conflict;
    let free = (0..placed.len()).filter(|k| !touched.contains(k)).collect();
    Report { statuses, conflicts, free }
}

/// Sort `items` (anchors, in their current order) under the constraints:
/// hard precedence always, soft where it does not contradict hard, the
/// current order to break ties. Constraints about anchors not in `items`
/// are ignored. Returns the order and the constraints it could not honour
/// (cycles).
pub fn order(items: &[Anchor], constraints: &[Constraint]) -> (Vec<Anchor>, Vec<Constraint>) {
    let idx = |a: &Anchor| items.iter().position(|x| match (x, a) {
        (Anchor::Line(p), Anchor::Line(q)) => similarity(p, q) >= SAME,
        _ => x == a,
    });
    let mut edges: Vec<(usize, usize, usize)> = Vec::new(); // (from, to, constraint)
    let mut refused = Vec::new();
    let mut by_strength: Vec<(usize, &Constraint)> = constraints.iter().enumerate().collect();
    by_strength.sort_by(|a, b| b.1.strength.cmp(&a.1.strength)); // hard first
    for (ci, c) in by_strength {
        let Some((a, b)) = c.precedence() else { continue };
        let (Some(x), Some(y)) = (idx(a), idx(b)) else { continue };
        if x == y { continue; }
        edges.push((x, y, ci));
        // would it close a cycle?
        let mut seen = vec![false; items.len()];
        let mut stack = vec![y];
        let mut cycle = false;
        while let Some(n) = stack.pop() {
            if n == x { cycle = true; break; }
            if seen[n] { continue; }
            seen[n] = true;
            stack.extend(edges.iter().filter(|(f, _, _)| *f == n).map(|(_, t, _)| *t));
        }
        if cycle { edges.pop(); refused.push(c.clone()); }
    }
    // Kahn, smallest current index first
    let mut indeg = vec![0usize; items.len()];
    for (_, t, _) in &edges { indeg[*t] += 1; }
    let mut out = Vec::new();
    let mut ready: BTreeSet<usize> = (0..items.len()).filter(|i| indeg[*i] == 0).collect();
    while let Some(n) = ready.pop_first() {
        out.push(items[n].clone());
        for (_, t, _) in edges.iter().filter(|(f, _, _)| *f == n) {
            indeg[*t] -= 1;
            if indeg[*t] == 0 { ready.insert(*t); }
        }
    }
    (out, refused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::SourceId;
    use studio_voice::Segment;

    fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }
    fn line(t: &str) -> Anchor { Anchor::Line(t.into()) }
    fn placed(name: &str, text: &str, at: f64, end: f64) -> Placed {
        Placed { names: vec![name.into()], lines: vec![text.into()], at: s(at), end: s(end) }
    }
    fn take(id: u64, lines: &[(&str, Option<&str>)]) -> Take {
        Take { source: SourceId(id), name: format!("C{id:04}"), len: s(60.0), angle_group: None,
               segments: lines.iter().enumerate().map(|(i, (t, who))| Segment { start: s(i as f64 * 3.0), end: s(i as f64 * 3.0 + 2.0),
                   text: t.to_string(), speaker: who.map(|w| w.to_string()), confidence: 0.9 }).collect() }
    }
    fn none(_: &str) -> bool { false }

    #[test]
    fn the_constraints_file_reads_every_form_and_says_back_what_it_could_not() {
        let (c, bad) = parse("C0031 before C0035\n\"are you ready\" follows \"the show opens friday\"  # an answer\n\
                              scene N7 at 3:10 ±2\ntotal <= 20:00\nthis is not a constraint\n\n# only a comment\n", "constraints.txt");
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].kind, Kind::Before(Anchor::Clip("C0031".into()), Anchor::Clip("C0035".into())));
        assert_eq!(c[1].kind, Kind::Follows(line("the show opens friday"), line("are you ready")));
        assert_eq!(c[2].kind, Kind::At(Anchor::Scene("N7".into()), s(190.0), s(2.0)));
        assert_eq!(c[3].kind, Kind::Total(s(1200.0)));
        assert_eq!(c[1].source, "constraints.txt:2");
        assert_eq!(bad, vec![(5, "this is not a constraint".to_string())]);
    }

    #[test]
    fn an_outline_chains_lines_within_a_scene_block_and_a_lyric_sheet_pins_lines_to_times() {
        let o = from_outline("# ── scene A\nOne.\nTwo.\n# ── scene B\nThree.\n", "outline.txt", false);
        assert_eq!(o.len(), 1, "blocks are not chained: scene order is the editor's");
        assert_eq!(o[0].kind, Kind::Before(line("One."), line("Two.")));
        assert_eq!(from_outline("# ── A\nOne.\n# ── B\nTwo.\n", "o", true).len(), 1, "unless asked");
        let l = from_lrc("[00:12.50] I walked in\n[01:02.00] and you were there\n[ar: nobody]\n", "song.lrc", s(0.5));
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].kind, Kind::At(line("and you were there"), s(62.0), s(0.5)));
    }

    #[test]
    fn transcripts_rule_out_orders_every_take_contradicts_and_say_nothing_where_takes_disagree() {
        let takes = vec![
            take(1, &[("Where were you?", Some("MAX")), ("Out.", Some("JUDE")), ("Out where?", Some("MAX"))]),
            take(2, &[("Where were you?", Some("MAX")), ("Out where?", Some("MAX")), ("Out.", Some("JUDE"))]),
        ];
        let c = infer(&takes, &none);
        let before: Vec<_> = c.iter().filter_map(|c| match &c.kind { Kind::Before(Anchor::Line(a), Anchor::Line(b)) => Some((a.as_str(), b.as_str())), _ => None }).collect();
        assert!(before.contains(&("Where were you?", "Out.")) && before.contains(&("Where were you?", "Out where?")), "{before:?}");
        assert!(!before.iter().any(|(a, b)| (*a == "Out." && *b == "Out where?") || (*a == "Out where?" && *b == "Out.")),
                "the takes disagree about those two: no constraint");
        assert!(c.iter().any(|c| c.kind == Kind::Follows(line("Where were you?"), line("Out.")) && c.strength == Strength::Soft),
                "a question answered by the other speaker: {c:?}");
        assert!(c.iter().all(|c| c.strength == Strength::Soft));
        // a question the SAME speaker goes on from is not answered by it
        let one = vec![take(3, &[("Ready?", Some("MAX")), ("Ready now, I said?", Some("MAX"))])];
        assert!(!infer(&one, &none).iter().any(|c| matches!(c.kind, Kind::Follows(..))), "{:?}", infer(&one, &none));
    }

    #[test]
    fn check_reports_satisfied_violated_unresolved_and_free_pieces() {
        let placed = vec![placed("C1", "Where were you?", 0.0, 2.0), placed("C2", "Out where?", 2.0, 4.0),
                          placed("C3", "Out.", 6.0, 7.0), placed("C9", "Unrelated.", 8.0, 9.0)];
        let cs = vec![
            Constraint { kind: Kind::Before(line("Where were you?"), line("Out.")), strength: Strength::Hard, source: "o:1".into() },
            Constraint { kind: Kind::Follows(line("Where were you?"), line("Out.")), strength: Strength::Soft, source: "t".into() },
            Constraint { kind: Kind::At(Anchor::Clip("C2".into()), s(2.5), s(1.0)), strength: Strength::Hard, source: "c:3".into() },
            Constraint { kind: Kind::Before(line("A line nobody placed."), line("Out.")), strength: Strength::Hard, source: "o:2".into() },
            Constraint { kind: Kind::Total(s(8.0)), strength: Strength::Hard, source: "c:4".into() },
        ];
        let r = check(&cs, &placed);
        let st: Vec<&Status> = r.statuses.iter().map(|(_, s)| s).collect();
        assert_eq!(st[0], &Status::Satisfied);
        assert!(matches!(st[1], Status::Violated(d) if (*d - 4.0).abs() < 1e-6), "the answer is 4 s after, not right after: {st:?}");
        assert_eq!(st[2], &Status::Satisfied);
        assert_eq!(st[3], &Status::Unresolved(vec![line("A line nobody placed.")]));
        assert!(matches!(st[4], Status::Violated(d) if (*d - 1.0).abs() < 1e-6));
        assert_eq!(r.free, vec![3], "C9 is under no constraint");
        assert!(r.summary().starts_with("2 satisfied, 2 violated, 1 unresolved"), "{}", r.summary());
    }

    #[test]
    fn documents_that_contradict_are_a_conflict_and_footage_against_a_document_is_set_aside() {
        let h = |a: &str, b: &str, src: &str| Constraint { kind: Kind::Before(line(a), line(b)), strength: Strength::Hard, source: src.into() };
        let soft = Constraint { kind: Kind::Before(line("Three."), line("One.")), strength: Strength::Soft, source: "transcripts".into() };
        let cs = vec![h("One.", "Two.", "script:1"), h("Two.", "Three.", "script:2"), soft.clone()];
        let r = check(&cs, &[]);
        assert!(r.conflicts.is_empty());
        assert_eq!(r.statuses[2].1, Status::Dropped, "the footage disagrees with the script: set aside, reported");
        // now two DOCUMENTS disagree
        let cs = vec![h("One.", "Two.", "script:1"), h("Two.", "One.", "director-note:4")];
        let r = check(&cs, &[]);
        assert_eq!(r.conflicts.len(), 1);
        let srcs: Vec<&str> = r.conflicts[0].iter().map(|c| c.source.as_str()).collect();
        assert!(srcs.contains(&"script:1") && srcs.contains(&"director-note:4"), "both sources named: {srcs:?}");
    }

    #[test]
    fn order_honours_hard_first_soft_where_it_can_and_keeps_the_rest_where_it_was() {
        let items = vec![line("C."), line("A."), line("B."), line("D.")];
        let cs = vec![
            Constraint { kind: Kind::Before(line("A."), line("B.")), strength: Strength::Hard, source: "doc".into() },
            Constraint { kind: Kind::Before(line("B."), line("C.")), strength: Strength::Hard, source: "doc".into() },
            Constraint { kind: Kind::Before(line("C."), line("A.")), strength: Strength::Soft, source: "t".into() }, // contradicts
        ];
        let (o, refused) = order(&items, &cs);
        assert_eq!(o, vec![line("A."), line("B."), line("C."), line("D.")]);
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].strength, Strength::Soft, "the soft one gives way, never the document");
    }
}
