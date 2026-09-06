//! Greebles by natural selection.
//!
//! A greeble is a small surface detail with no function -- a stud, a
//! vent, a bracket, a pipe stub, a recessed panel -- whose job is to
//! make a large surface read as MADE rather than extruded. Every
//! sci-fi hull and every gothic buttress is greebles. The detailing
//! passes (`detail.rs`) apply one rule uniformly; a greeble pattern is
//! a RHYTHM, and rhythm is a state machine walking the surface.
//!
//! Nobody can write a good greeble FSM by hand on the first try, and a
//! model certainly cannot. But it is easy to say what a good result
//! LOOKS like: detailed but not noisy, emphasising edges, symmetric,
//! with some depth. Those are METRICS, and metrics plus a small
//! mutable program is a genetic algorithm.
//!
//! THE GENOME
//!
//! A decorator is a table of rules, each `state: condition -> action,
//! next-state [p=]`. The walker visits surface cells in order; at each
//! it takes the first rule of its current state whose condition holds
//! and whose probability passes, performs the action, and moves to the
//! next state. That is the whole language: small enough that mutation
//! is "change one field" and crossover is "splice two rule lists", and
//! printable as text so a player can read the winner, edit it, or save
//! it as vocabulary.
//!
//! THE METRICS
//!
//! * coverage    fraction of surface cells touched
//! * variety     distinct blocks used, normalised
//! * noise       fraction of adjacent touched cells that differ:
//!               rhythm is low noise, static is high noise
//! * edges       fraction of edge cells touched, vs interior
//! * depth       fraction of actions that protrude or recess
//! * symmetry    how well the result mirrors across the surface's axis
//!
//! A phrase sets the WEIGHTS: "industrial" wants depth and coverage,
//! "clean" wants low noise, "gothic" wants edges and symmetry. Evolution
//! does the rest, deterministically from a seed, so the same request
//! on the same wall gives the same greebles.

use crate::weather::Change;
use crate::VoxelWorld;

/// Where a surface cell sits on its face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Any,
    Edge,
    Corner,
    Interior,
    /// The top course.
    Top,
    Bottom,
    /// Every nth cell along the walk.
    Every(u8),
    /// Cells in a horizontal band: (from, to) as fractions of height * 8.
    Band(u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Skip,
    /// Replace the surface cell's block.
    Put(u16),
    /// Add a block in front of the surface (out along its normal).
    Protrude(u16),
    /// Remove the surface cell (a recess). Refused on one-thick walls
    /// by the walker, never by the genome.
    Recess,
    /// Add a block above the surface cell.
    Cap(u16),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub cond: Cond,
    pub act: Act,
    pub next: u8,
    /// 0..=1. A rule that fails its roll is skipped for the next rule.
    pub p: f32,
}

/// The decorator: states, each a short list of rules. Small on purpose.
#[derive(Debug, Clone, PartialEq)]
pub struct Genome {
    pub states: Vec<Vec<Rule>>,
}

pub const MAX_STATES: usize = 4;
pub const MAX_RULES: usize = 3;
/// The blocks a greeble may use. A palette-relative role would be
/// better still; these are the accents that read as detail on stone.
pub const GREEBLE_BLOCKS: &[u16] = &[58, 86, 4, 1, 17, 61, 37];

fn hash(seed: u64, a: u64, b: u64) -> u64 {
    let mut h = seed ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}
fn roll(seed: u64, a: u64, b: u64) -> f32 { (hash(seed, a, b) % 10_000) as f32 / 10_000.0 }
fn pick<T: Copy>(xs: &[T], seed: u64, a: u64, b: u64) -> T { xs[(hash(seed, a, b) % xs.len() as u64) as usize] }

impl Genome {
    /// A random genome. Biased toward Skip so that first generations
    /// are sparse: a wall that starts covered in noise has nowhere to
    /// evolve but toward less.
    pub fn random(seed: u64) -> Self {
        let n = 2 + (hash(seed, 1, 1) % (MAX_STATES as u64 - 1)) as usize;
        let states = (0..n).map(|s| {
            let m = 1 + (hash(seed, 2, s as u64) % MAX_RULES as u64) as usize;
            (0..m).map(|r| Self::random_rule(seed, s as u64 * 16 + r as u64, n)).collect()
        }).collect();
        Genome { states }
    }

    fn random_rule(seed: u64, k: u64, nstates: usize) -> Rule {
        let conds = [Cond::Any, Cond::Edge, Cond::Corner, Cond::Interior, Cond::Top, Cond::Bottom,
                     Cond::Every(2), Cond::Every(3), Cond::Every(4), Cond::Band(0, 3), Cond::Band(5, 8)];
        let blk = pick(GREEBLE_BLOCKS, seed, 7, k);
        let acts = [Act::Skip, Act::Skip, Act::Skip, Act::Put(blk), Act::Protrude(blk),
                    Act::Recess, Act::Cap(blk)];
        Rule { cond: pick(&conds, seed, 3, k), act: pick(&acts, seed, 4, k),
               next: (hash(seed, 5, k) % nstates as u64) as u8,
               p: 0.3 + roll(seed, 6, k) * 0.7 }
    }

    /// Change ONE gene. Small mutations keep a good genome near itself.
    pub fn mutate(&self, seed: u64) -> Self {
        let mut g = self.clone();
        let s = (hash(seed, 11, 0) % g.states.len() as u64) as usize;
        let r = (hash(seed, 12, 0) % g.states[s].len() as u64) as usize;
        let n = g.states.len();
        let fresh = Self::random_rule(seed, 99, n);
        match hash(seed, 13, 0) % 5 {
            0 => g.states[s][r].cond = fresh.cond,
            1 => g.states[s][r].act = fresh.act,
            2 => g.states[s][r].next = fresh.next,
            3 => g.states[s][r].p = (g.states[s][r].p + (roll(seed, 14, 0) - 0.5) * 0.4).clamp(0.05, 1.0),
            _ => {
                // add a rule, or drop one if full
                if g.states[s].len() < MAX_RULES { g.states[s].push(fresh); }
                else if g.states[s].len() > 1 { g.states[s].remove(r); }
            }
        }
        g
    }

    /// Splice: states from `self` up to a cut, then from `other`.
    /// `next` indices are clamped so a spliced genome is always valid.
    pub fn crossover(&self, other: &Genome, seed: u64) -> Self {
        let cut = 1 + (hash(seed, 21, 0) % (self.states.len().min(other.states.len()) as u64)) as usize;
        let mut states: Vec<Vec<Rule>> = self.states[..cut].to_vec();
        states.extend(other.states.iter().skip(cut).cloned());
        let n = states.len().max(1) as u8;
        for s in &mut states { for r in s { r.next = r.next.min(n - 1); } }
        Genome { states }
    }

    /// The genome as text, the language a player reads and edits.
    pub fn to_text(&self) -> String {
        let mut s = String::from("# decorator genome\n");
        for (i, st) in self.states.iter().enumerate() {
            for r in st {
                s.push_str(&format!("s{i}: {} -> {}, s{}{}\n", cond_text(r.cond), act_text(r.act), r.next,
                                    if r.p < 0.999 { format!(" p={:.2}", r.p) } else { String::new() }));
            }
        }
        s
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut states: Vec<Vec<Rule>> = Vec::new();
        for (ln, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let (st, rest) = line.split_once(':').ok_or(format!("line {}: expected `sN:`", ln + 1))?;
            let si: usize = st.trim().trim_start_matches('s').parse()
                .map_err(|_| format!("line {}: bad state '{st}'", ln + 1))?;
            let (cond, rest) = rest.split_once("->").ok_or(format!("line {}: expected `->`", ln + 1))?;
            let (act, rest) = rest.split_once(',').ok_or(format!("line {}: expected `, sN`", ln + 1))?;
            let mut it = rest.split_whitespace();
            let next: u8 = it.next().unwrap_or("s0").trim_start_matches('s').parse()
                .map_err(|_| format!("line {}: bad next state", ln + 1))?;
            let p = it.find_map(|t| t.strip_prefix("p=")).and_then(|v| v.parse().ok()).unwrap_or(1.0);
            while states.len() <= si { states.push(Vec::new()); }
            states[si].push(Rule { cond: parse_cond(cond.trim())?, act: parse_act(act.trim())?, next, p });
        }
        if states.is_empty() || states.iter().any(|s| s.is_empty()) {
            return Err("every state needs at least one rule".into());
        }
        let n = states.len() as u8;
        for s in &mut states { for r in s { r.next = r.next.min(n - 1); } }
        Ok(Genome { states })
    }
}

fn cond_text(c: Cond) -> String {
    match c {
        Cond::Any => "any".into(), Cond::Edge => "edge".into(), Cond::Corner => "corner".into(),
        Cond::Interior => "interior".into(), Cond::Top => "top".into(), Cond::Bottom => "bottom".into(),
        Cond::Every(n) => format!("every{n}"), Cond::Band(a, b) => format!("band{a}-{b}"),
    }
}
fn parse_cond(s: &str) -> Result<Cond, String> {
    Ok(match s {
        "any" => Cond::Any, "edge" => Cond::Edge, "corner" => Cond::Corner,
        "interior" => Cond::Interior, "top" => Cond::Top, "bottom" => Cond::Bottom,
        s if s.starts_with("every") => Cond::Every(s[5..].parse().map_err(|_| format!("bad '{s}'"))?),
        s if s.starts_with("band") => {
            let (a, b) = s[4..].split_once('-').ok_or(format!("bad '{s}'"))?;
            Cond::Band(a.parse().map_err(|_| "band")?, b.parse().map_err(|_| "band")?)
        }
        _ => return Err(format!("unknown condition '{s}'")),
    })
}
fn act_text(a: Act) -> String {
    match a {
        Act::Skip => "skip".into(), Act::Put(b) => format!("put {b}"),
        Act::Protrude(b) => format!("protrude {b}"), Act::Recess => "recess".into(),
        Act::Cap(b) => format!("cap {b}"),
    }
}
fn parse_act(s: &str) -> Result<Act, String> {
    let mut it = s.split_whitespace();
    let v = it.next().unwrap_or("");
    let n = || it.clone().next().and_then(|x| x.parse::<u16>().ok()).ok_or(format!("'{s}' needs a block"));
    Ok(match v {
        "skip" => Act::Skip, "put" => Act::Put(n()?), "protrude" => Act::Protrude(n()?),
        "recess" => Act::Recess, "cap" => Act::Cap(n()?),
        _ => return Err(format!("unknown action '{s}'")),
    })
}

impl crate::evolve::Genome for Genome {
    fn mutate(&self, seed: u64) -> Self { Genome::mutate(self, seed) }
    fn crossover(&self, other: &Self, seed: u64) -> Self { Genome::crossover(self, other, seed) }
}

/// A surface cell the walker visits, with what the conditions need.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub at: (i32, i32, i32),
    /// Outward normal, one axis.
    pub normal: (i32, i32, i32),
    pub edge: bool,
    pub corner: bool,
    pub top: bool,
    pub bottom: bool,
    /// 0..8 vertical band.
    pub band: u8,
    /// True if the cell has solid behind it (not a one-thick wall).
    pub backed: bool,
}

/// The exposed faces of a region, in a stable walk order (by y, then
/// around), so `Every(n)` produces courses and rhythms rather than
/// scatter.
pub fn faces<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32)) -> Vec<Face> {
    let (lo, hi) = ((min.0.min(max.0), min.1.min(max.1), min.2.min(max.2)),
                    (min.0.max(max.0), min.1.max(max.1), min.2.max(max.2)));
    let h = (hi.1 - lo.1).max(1);
    let mut out = Vec::new();
    for y in lo.1..=hi.1 {
        for z in lo.2..=hi.2 { for x in lo.0..=hi.0 {
            let b = w.get(x, y, z);
            if b == 0 { continue; }
            for n in [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)] {
                if w.get(x + n.0, y, z + n.2) != 0 { continue; }
                // edge: a neighbour along the face is air or a different block
                let (tx, tz) = (n.2, n.0);   // tangent
                let side_a = w.get(x + tx, y, z + tz);
                let side_b = w.get(x - tx, y, z - tz);
                let up = w.get(x, y + 1, z);
                let down = w.get(x, y - 1, z);
                let edge = side_a == 0 || side_b == 0 || up == 0 || down == 0;
                let corner = (side_a == 0 || side_b == 0) && (up == 0 || down == 0);
                out.push(Face {
                    at: (x, y, z), normal: n, edge, corner,
                    top: up == 0, bottom: down == 0,
                    band: (((y - lo.1) * 8) / h).clamp(0, 7) as u8,
                    backed: w.get(x - n.0, y, z - n.2) != 0,
                });
            }
        }}
    }
    out
}

/// Run a genome over faces. Pure: returns the changes.
pub fn run(g: &Genome, faces: &[Face], seed: u64) -> Vec<Change> {
    let mut state = 0usize;
    let mut out = Vec::new();
    for (i, f) in faces.iter().enumerate() {
        let rules = &g.states[state.min(g.states.len() - 1)];
        let mut taken = None;
        for (ri, r) in rules.iter().enumerate() {
            let ok = match r.cond {
                Cond::Any => true, Cond::Edge => f.edge, Cond::Corner => f.corner,
                Cond::Interior => !f.edge, Cond::Top => f.top, Cond::Bottom => f.bottom,
                Cond::Every(n) => i % n.max(1) as usize == 0,
                Cond::Band(a, b) => f.band >= a && f.band < b,
            };
            if ok && roll(seed, i as u64, ri as u64) < r.p { taken = Some(*r); break; }
        }
        let Some(r) = taken else { continue };
        match r.act {
            Act::Skip => {}
            Act::Put(b) => out.push(Change { at: f.at, to: b }),
            Act::Protrude(b) => out.push(Change {
                at: (f.at.0 + f.normal.0, f.at.1, f.at.2 + f.normal.2), to: b }),
            // A recess in a one-thick wall is a hole; refuse it here,
            // not in the genome, so evolution learns where recesses work.
            Act::Recess => if f.backed { out.push(Change { at: f.at, to: 0 }) },
            Act::Cap(b) => if f.top { out.push(Change { at: (f.at.0, f.at.1 + 1, f.at.2), to: b }) },
        }
        state = r.next as usize;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Metrics {
    pub coverage: f32,
    pub variety: f32,
    pub noise: f32,
    pub edges: f32,
    pub depth: f32,
    pub symmetry: f32,
    /// How REGULAR the decoration is along a course: the best
    /// autocorrelation of the decorated/undecorated pattern at lags 2-6.
    /// Studs every third block score high; scatter scores low. Noise
    /// says whether neighbours differ; rhythm says whether they differ
    /// in a pattern.
    pub rhythm: f32,
    /// Fraction of decorated cells adjacent to an undecorated one: the
    /// boundary between detail and plain surface. All-over detail has
    /// no contrast; a moulding does.
    pub contrast: f32,
}

/// Measure a result against the faces it was run on.
pub fn measure(faces: &[Face], changes: &[Change]) -> Metrics {
    if faces.is_empty() { return Metrics::default(); }
    let touched: std::collections::BTreeMap<(i32, i32, i32), u16> =
        changes.iter().map(|c| (c.at, c.to)).collect();
    let face_set: std::collections::BTreeMap<(i32, i32, i32), &Face> = faces.iter().map(|f| (f.at, f)).collect();
    // A face is COVERED if anything decorates it: a put on the cell, a
    // protrusion in front of it, or a cap above it. Counting only puts
    // let evolution cover a wall in protrusions while reporting 4%
    // coverage, and "clean minimal" came back wall-to-wall studs.
    let decorated = |f: &Face| {
        touched.contains_key(&f.at)
            || touched.contains_key(&(f.at.0 + f.normal.0, f.at.1, f.at.2 + f.normal.2))
            || (f.top && touched.contains_key(&(f.at.0, f.at.1 + 1, f.at.2)))
    };
    let on_face = faces.iter().filter(|f| decorated(f)).count();
    let coverage = on_face as f32 / faces.len() as f32;
    let blocks: std::collections::BTreeSet<u16> = touched.values().copied().collect();
    let variety = (blocks.len() as f32 / 4.0).min(1.0);
    let edge_faces = faces.iter().filter(|f| f.edge).count().max(1);
    let edge_hit = faces.iter().filter(|f| f.edge && decorated(f)).count();
    let edges = edge_hit as f32 / edge_faces as f32;
    let depth = if changes.is_empty() { 0.0 } else {
        changes.iter().filter(|c| !face_set.contains_key(&c.at)).count() as f32 / changes.len() as f32
    };
    // noise: among touched face cells with a touched horizontal
    // neighbour, how often do they differ?
    let (mut pairs, mut diff) = (0, 0);
    for (at, b) in &touched {
        for d in [(1, 0, 0), (0, 0, 1)] {
            if let Some(nb) = touched.get(&(at.0 + d.0, at.1, at.2 + d.2)) {
                pairs += 1; if nb != b { diff += 1; }
            }
        }
    }
    let noise = if pairs == 0 { 0.0 } else { diff as f32 / pairs as f32 };
    // symmetry: mirror every touched face cell across the surface's x
    // midline and count matches
    let (xmin, xmax) = (faces.iter().map(|f| f.at.0).min().unwrap(), faces.iter().map(|f| f.at.0).max().unwrap());
    let (mut sym_n, mut sym_ok) = (0, 0);
    for (at, b) in &touched {
        let mx = xmin + xmax - at.0;
        sym_n += 1;
        if touched.get(&(mx, at.1, at.2)) == Some(b) { sym_ok += 1; }
    }
    let symmetry = if sym_n == 0 { 1.0 } else { sym_ok as f32 / sym_n as f32 };
    // rhythm: per course (same y), the decorated pattern along the
    // walk; autocorrelation at lags 2..=6, best lag wins
    let mut courses: std::collections::BTreeMap<i32, Vec<bool>> = std::collections::BTreeMap::new();
    for f in faces { courses.entry(f.at.1).or_default().push(decorated(f)); }
    let mut best_ac = 0.0f32; let mut any = false;
    for (_, c) in &courses {
        if c.len() < 8 || !c.iter().any(|d| *d) { continue; }
        any = true;
        for lag in 2..=6usize {
            if c.len() <= lag { break; }
            let m = c.iter().zip(c.iter().skip(lag)).filter(|(a, b)| a == b).count() as f32 / (c.len() - lag) as f32;
            let on = c.iter().filter(|d| **d).count() as f32 / c.len() as f32;
            // agreement beyond what a random pattern of the same density gives
            let chance = on * on + (1.0 - on) * (1.0 - on);
            let ac = ((m - chance) / (1.0 - chance).max(1e-3)).clamp(0.0, 1.0);
            best_ac = best_ac.max(ac);
        }
    }
    let rhythm = if any { best_ac } else { 0.0 };
    let decorated_set: std::collections::BTreeSet<(i32, i32, i32)> = faces.iter().filter(|f| decorated(f)).map(|f| f.at).collect();
    let (mut dec_n, mut boundary) = (0usize, 0usize);
    for f in faces {
        if !decorated_set.contains(&f.at) { continue; }
        dec_n += 1;
        let (tx, tz) = (f.normal.2, f.normal.0);
        let nbrs = [(f.at.0 + tx, f.at.1, f.at.2 + tz), (f.at.0 - tx, f.at.1, f.at.2 - tz),
                    (f.at.0, f.at.1 + 1, f.at.2), (f.at.0, f.at.1 - 1, f.at.2)];
        if nbrs.iter().any(|n| face_set.contains_key(n) && !decorated_set.contains(n)) { boundary += 1; }
    }
    let contrast = if dec_n == 0 { 0.0 } else { boundary as f32 / dec_n as f32 };
    Metrics { coverage, variety, noise, edges, depth, symmetry, rhythm, contrast }
}

/// What the player wants, as weights over the metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fitness {
    pub coverage: f32,
    pub variety: f32,
    pub noise: f32,
    pub edges: f32,
    pub depth: f32,
    pub symmetry: f32,
    /// Coverage above this is penalised: a wall entirely covered in
    /// detail has no detail.
    pub max_coverage: f32,
}

impl Default for Fitness {
    fn default() -> Self {
        Self { coverage: 1.0, variety: 0.5, noise: -1.0, edges: 0.8, depth: 0.5, symmetry: 0.5,
               max_coverage: 0.45 }
    }
}

impl Fitness {
    /// Weights from a phrase. Every word is additive, so "clean
    /// industrial" is low noise AND deep.
    pub fn from_phrase(p: &str) -> Self {
        let p = p.to_lowercase();
        let mut f = Fitness::default();
        let has = |ws: &[&str]| ws.iter().any(|w| p.contains(w));
        if has(&["industrial", "mechanical", "sci-fi", "scifi", "machine", "hull"]) {
            f.depth += 1.0; f.coverage += 0.5; f.symmetry -= 0.3; f.max_coverage = 0.6;
        }
        if has(&["gothic", "ornate", "cathedral", "baroque"]) {
            f.edges += 1.0; f.symmetry += 1.0; f.variety += 0.5;
        }
        if has(&["clean", "minimal", "subtle", "restrained", "not noisy", "calm"]) {
            f.noise -= 1.5; f.max_coverage = 0.25; f.coverage -= 0.5;
        }
        if has(&["busy", "dense", "heavy", "rich", "detailed"]) {
            f.coverage += 1.0; f.variety += 0.5; f.max_coverage = 0.7;
        }
        if has(&["symmetric", "symmetrical", "formal", "balanced"]) { f.symmetry += 1.5; }
        if has(&["rustic", "organic", "irregular", "weathered"]) { f.symmetry -= 1.0; f.noise += 0.5; }
        f
    }

    /// The objectives for the shared engine, all "lower is better":
    /// coverage miss against the cap, noise, edge miss, depth miss,
    /// symmetry miss, rhythm miss, contrast miss. Kept separate so the
    /// front holds the trade-offs -- a noisier-but-deeper wall and a
    /// cleaner-but-flatter one both survive to be chosen between.
    pub fn objectives(&self, m: &Metrics) -> Vec<f32> {
        let want_cov = (self.max_coverage * 0.8).clamp(0.05, 0.95);
        vec![
            (m.coverage - want_cov).abs() + if m.coverage < 0.02 { 1.0 } else { 0.0 },
            m.noise * self.noise.abs().max(0.1),
            (1.0 - m.edges) * self.edges.max(0.0),
            (1.0 - m.depth) * self.depth.max(0.0),
            (1.0 - m.symmetry) * self.symmetry.max(0.0),
            (1.0 - m.rhythm) * 0.5,
            (0.6 - m.contrast).abs() * 0.5,
        ]
    }

    /// Weights over `objectives`, from the same phrase.
    pub fn weights(&self) -> Vec<f32> {
        vec![self.coverage.abs().max(0.2), 1.0, 1.0, 1.0, 1.0, 0.6, 0.4]
    }

    pub fn score(&self, m: &Metrics) -> f32 {
        let over = (m.coverage - self.max_coverage).max(0.0) * 3.0;
        self.coverage * m.coverage + self.variety * m.variety + self.noise * m.noise
            + self.edges * m.edges + self.depth * m.depth + self.symmetry * m.symmetry - over
            // nothing at all is not a decoration
            - if m.coverage < 0.02 { 2.0 } else { 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evolved {
    pub genome: Genome,
    pub score: f32,
    pub metrics: Metrics,
    pub changes: Vec<Change>,
    /// Best score per generation, for the player to see it converge.
    pub history: Vec<f32>,
}

/// Evolve on the shared engine: Pareto-ranked, budgeted, seeded from
/// an archive. Returns the front's best by the phrase's weights.
pub fn evolve_ranked(faces: &[Face], fit: &Fitness, seed: u64, budget: crate::evolve::Budget,
                     seeds: &[Genome]) -> (Evolved, crate::evolve::Outcome<Genome>) {
    let eval = |g: &Genome| { let c = run(g, faces, seed); fit.objectives(&measure(faces, &c)) };
    let out = crate::evolve::evolve(seed, budget, seeds, Genome::random, eval, &fit.weights());
    let best = crate::evolve::pick(&out.front, &fit.weights()).expect("a front");
    let changes = run(&best.genome, faces, seed);
    let metrics = measure(faces, &changes);
    let e = Evolved { genome: best.genome.clone(), score: fit.score(&metrics), metrics, changes,
                      history: out.history.clone() };
    (e, out)
}

/// LEVELS OF DETAIL. Evolve at three scales in turn -- coarse (edge
/// emphasis, large rhythm), medium, fine (studs, relief) -- each
/// level's winner applied before the next level re-reads the surface
/// it made. Detail on detail: a cornice, then panels between the
/// cornice's members, then rivets on the panels. One pass could not
/// do this, because the fine features have nothing to sit on until
/// the coarse ones exist.
pub fn evolve_levels<W: crate::VoxelWorld>(w: &mut W, min: (i32, i32, i32), max: (i32, i32, i32),
                                           fit: &Fitness, seed: u64, levels: usize,
                                           budget_per_level: crate::evolve::Budget,
                                           archive: &mut crate::evolve::Archive<Genome>,
                                           key: &str) -> Vec<(usize, Evolved)> {
    let mut out = Vec::new();
    for level in 0..levels.max(1) {
        let faces = faces(w, min, max);
        if faces.is_empty() { break; }
        // each level's fitness leans finer: less edge emphasis, more
        // rhythm and contrast, a lower coverage cap
        let mut f = *fit;
        f.edges *= 1.0 - 0.35 * level as f32;
        f.max_coverage *= 0.75f32.powi(level as i32);
        f.depth *= 1.0 + 0.3 * level as f32;
        let k = format!("{key}/L{level}");
        let (e, res) = evolve_ranked(&faces, &f, seed.wrapping_add(level as u64 * 7919), budget_per_level,
                                     &archive.seeds(&k));
        archive.offer(&k, &res.front);
        for ch in &e.changes { w.set(ch.at.0, ch.at.1, ch.at.2, ch.to); }
        out.push((level, e));
    }
    out
}

/// Evolve a decorator for these faces. Deterministic from the seed.
///
/// Tournament selection, elitism of one, half the children by
/// crossover and half by mutation. Small populations and few
/// generations: the surface is the expensive part and evaluation is a
/// full run over it, so 24 x 12 is a few hundred thousand face visits
/// -- fast, and enough to find rhythm.
pub fn evolve(faces: &[Face], fit: &Fitness, seed: u64, pop: usize, gens: usize) -> Evolved {
    let pop = pop.clamp(4, 64);
    let mut population: Vec<Genome> = (0..pop).map(|i| Genome::random(hash(seed, 100, i as u64))).collect();
    let eval = |g: &Genome, s: u64| { let c = run(g, faces, s); let m = measure(faces, &c); (fit.score(&m), m, c) };
    let mut history = Vec::new();
    let mut best: Option<Evolved> = None;
    for gen in 0..gens.max(1) {
        let mut scored: Vec<(f32, usize)> = population.iter().enumerate()
            .map(|(i, g)| (eval(g, seed).0, i)).collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let (top_score, top_i) = scored[0];
        history.push(top_score);
        if best.as_ref().map(|b| top_score > b.score).unwrap_or(true) {
            let (score, metrics, changes) = eval(&population[top_i], seed);
            best = Some(Evolved { genome: population[top_i].clone(), score, metrics, changes,
                                  history: Vec::new() });
        }
        // next generation
        let tourney = |k: u64| {
            let a = scored[(hash(seed, 200 + gen as u64, k) % pop as u64) as usize];
            let b = scored[(hash(seed, 300 + gen as u64, k) % pop as u64) as usize];
            if a.0 >= b.0 { a.1 } else { b.1 }
        };
        let mut next = vec![population[top_i].clone()];
        let mut k = 0u64;
        while next.len() < pop {
            let a = &population[tourney(k)];
            let child = if k % 2 == 0 {
                a.crossover(&population[tourney(k + 1000)], hash(seed, 400 + gen as u64, k))
            } else {
                a.mutate(hash(seed, 500 + gen as u64, k))
            };
            next.push(child);
            k += 1;
        }
        population = next;
    }
    let mut b = best.expect("at least one generation");
    b.history = history;
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct W(BTreeMap<(i32, i32, i32), u16>);
    impl VoxelWorld for W {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x, y, z), b); true }
    }
    /// A hollow 13x9x13 block: four exterior walls, two thick.
    fn hall() -> W {
        let mut w = W(BTreeMap::new());
        for x in 0..13 { for y in 65..74 { for z in 0..13 {
            let shell = x < 2 || x > 10 || z < 2 || z > 10;
            if shell { w.set(x, y, z, 1); }
        }}}
        w
    }

    #[test]
    fn the_genome_language_round_trips() {
        let g = Genome::random(7);
        let text = g.to_text();
        let back = Genome::parse(&text).unwrap();
        assert_eq!(back.to_text(), text, "{text}");
        assert!(Genome::parse("s0: nonsense -> skip, s0").is_err());
        assert!(Genome::parse("# nothing\n").is_err(), "a genome needs a state");
    }

    #[test]
    fn a_hand_written_genome_makes_a_rhythm() {
        // Every third face gets a stud: that is a course of studs, not
        // scatter, and the metrics should say so.
        let g = Genome::parse("s0: every3 -> protrude 58, s0\ns0: any -> skip, s0\n").unwrap();
        let w = hall();
        let f = faces(&w, (0, 65, 0), (12, 73, 12));
        let c = run(&g, &f, 1);
        assert!(!c.is_empty());
        let m = measure(&f, &c);
        assert!(m.depth > 0.99, "all protrusions: {m:?}");
        assert!(m.noise < 0.01, "one block, no noise: {m:?}");
        // protrusions sit OUTSIDE the wall
        for ch in &c { assert_eq!(w.get(ch.at.0, ch.at.1, ch.at.2), 0, "{:?} is not air", ch.at); }
    }

    #[test]
    fn a_recess_never_opens_a_one_thick_wall() {
        let mut w = W(BTreeMap::new());
        for x in 0..10 { for y in 65..70 { w.set(x, y, 5, 1); } }    // one thick
        let f = faces(&w, (0, 65, 5), (9, 69, 5));
        let g = Genome::parse("s0: any -> recess, s0\n").unwrap();
        let c = run(&g, &f, 1);
        // The wall is one thick along Z, so no face with a +-Z normal
        // may be recessed: that would be a through-hole. The END faces
        // (+-X normals) have solid behind them along X, and recessing
        // one shortens the wall rather than piercing it -- allowed. The
        // first version of this test expected zero recesses and was
        // wrong about the ends.
        // A corner cell has two faces and a map keeps only one, so
        // attribute by the property itself: every recessed cell is an
        // END cell, reached through its backed X face.
        for ch in &c {
            assert!(ch.at.0 == 0 || ch.at.0 == 9, "recessed a mid-wall cell at {:?}", ch.at);
            assert!(f.iter().any(|fc| fc.at == ch.at && fc.backed && fc.normal.2 == 0));
        }
        assert!(c.len() <= 10, "at most the two ends x five courses: {}", c.len());
        let w2 = hall();
        let f2 = faces(&w2, (0, 65, 0), (12, 73, 12));
        assert!(!run(&g, &f2, 1).is_empty(), "but a two-thick wall can be recessed");
    }

    #[test]
    fn mutation_changes_one_gene_and_crossover_stays_valid() {
        let g = Genome::random(3);
        let m = g.mutate(9);
        let diff: usize = g.states.iter().zip(&m.states).map(|(a, b)| {
            a.iter().zip(b).filter(|(x, y)| x != y).count() + a.len().abs_diff(b.len())
        }).sum::<usize>() + g.states.len().abs_diff(m.states.len());
        assert!(diff <= 2, "one gene (or one rule added/removed): {diff}");
        let o = Genome::random(4);
        let x = g.crossover(&o, 5);
        for s in &x.states { for r in s { assert!((r.next as usize) < x.states.len()); } }
        assert!(Genome::parse(&x.to_text()).is_ok());
    }

    #[test]
    fn metrics_read_the_way_the_words_do() {
        let w = hall();
        let f = faces(&w, (0, 65, 0), (12, 73, 12));
        // uniform: no noise
        let uni = run(&Genome::parse("s0: any -> put 58, s0\n").unwrap(), &f, 1);
        let m = measure(&f, &uni);
        assert!(m.coverage > 0.99 && m.noise < 0.01, "{m:?}");
        // alternating two blocks: maximal noise
        let alt = Genome::parse("s0: any -> put 58, s1\ns1: any -> put 4, s0\n").unwrap();
        let m2 = measure(&f, &run(&alt, &f, 1));
        assert!(m2.noise > 0.5, "alternation is noise: {m2:?}");
        // edges only: high edge score, low coverage
        let e = Genome::parse("s0: edge -> put 58, s0\ns0: any -> skip, s0\n").unwrap();
        let m3 = measure(&f, &run(&e, &f, 1));
        assert!(m3.edges > 0.9 && m3.coverage < 0.6, "{m3:?}");
        // a symmetric result scores symmetric: 'every2' on a walk
        // ordered by x within a row is mirror-symmetric on odd widths
        assert!(m.symmetry > 0.99, "uniform is symmetric: {m:?}");
    }

    #[test]
    fn fitness_follows_the_phrase() {
        let ind = Fitness::from_phrase("industrial");
        let clean = Fitness::from_phrase("clean and minimal");
        let goth = Fitness::from_phrase("gothic");
        assert!(ind.depth > clean.depth);
        assert!(clean.noise < ind.noise);
        assert!(goth.edges > ind.edges && goth.symmetry > ind.symmetry);
        assert!(Fitness::from_phrase("clean industrial").depth > 1.0, "words are additive");
    }

    #[test]
    fn evolution_improves_and_is_deterministic() {
        let w = hall();
        let f = faces(&w, (0, 65, 0), (12, 73, 12));
        let fit = Fitness::from_phrase("gothic");
        let e = evolve(&f, &fit, 42, 16, 10);
        assert!(e.history.len() == 10);
        assert!(e.history.last().unwrap() >= e.history.first().unwrap(), "{:?}", e.history);
        assert!(e.score > e.history[0] - 1e-6);
        let e2 = evolve(&f, &fit, 42, 16, 10);
        assert_eq!(e.genome, e2.genome, "same seed, same greebles");
        assert_ne!(evolve(&f, &fit, 43, 16, 10).genome, e.genome);
        assert!(e.metrics.coverage > 0.02, "it decorated something: {:?}", e.metrics);
    }

    #[test]
    fn the_phrase_shapes_the_winner() {
        let w = hall();
        let f = faces(&w, (0, 65, 0), (12, 73, 12));
        // Evolution is a search, so the assertions are about the CAPS
        // and PENALTIES the phrase sets, which a converged search
        // respects, not about one metric beating another by a hair.
        let ind = evolve(&f, &Fitness::from_phrase("industrial"), 7, 24, 20);
        let clean = evolve(&f, &Fitness::from_phrase("clean minimal"), 7, 24, 20);
        assert!(clean.metrics.coverage <= 0.35, "clean respects its coverage cap: {:?}", clean.metrics);
        assert!(clean.metrics.noise <= 0.15, "clean is not noisy: {:?}", clean.metrics);
        assert!(ind.metrics.depth >= 0.5, "industrial has depth: {:?}", ind.metrics);
        assert!(ind.metrics.coverage >= 0.05, "industrial decorates: {:?}", ind.metrics);
    }

    #[test]
    fn protrusions_count_as_coverage() {
        // The loophole: a wall of studs reported 4% coverage because only
        // puts on the face cell were counted.
        let w = hall();
        let f = faces(&w, (0, 65, 0), (12, 73, 12));
        let studs = run(&Genome::parse("s0: any -> protrude 58, s0\n").unwrap(), &f, 1);
        let m = measure(&f, &studs);
        assert!(m.coverage > 0.99, "every face has a stud in front of it: {m:?}");
    }

    #[test]
    fn rhythm_and_contrast_read_the_way_the_words_do() {
        let w = hall();
        let f = faces(&w, (0, 65, 0), (12, 73, 12));
        let studs = run(&Genome::parse("s0: every3 -> put 58, s0\ns0: any -> skip, s0\n").unwrap(), &f, 1);
        let m = measure(&f, &studs);
        assert!(m.rhythm > 0.5, "every third is a rhythm: {m:?}");
        assert!(m.contrast > 0.8, "each stud borders plain surface: {m:?}");
        let all = run(&Genome::parse("s0: any -> put 58, s0\n").unwrap(), &f, 1);
        let m2 = measure(&f, &all);
        assert!(m2.contrast < 0.3, "all-over has no contrast: {m2:?}");
    }

    #[test]
    fn the_ranked_evolution_returns_a_front_with_choices_on_it() {
        let w = hall();
        let f = faces(&w, (0, 65, 0), (12, 73, 12));
        let fit = Fitness::from_phrase("gothic");
        let (e, out) = evolve_ranked(&f, &fit, 5, crate::evolve::Budget::evals(300, 20), &[]);
        assert!(e.metrics.coverage > 0.02);
        assert!(out.front.len() >= 2, "a front, not one answer: {}", out.front.len());
        let d = crate::evolve::diverse(&out.front, 3);
        assert!(d.len() >= 2);
    }

    #[test]
    fn levels_add_detail_on_detail_and_the_archive_fills() {
        let mut w = hall();
        let mut arch = crate::evolve::Archive::default();
        let fit = Fitness::from_phrase("gothic");
        let levels = evolve_levels(&mut w, (0, 65, 0), (12, 73, 12), &fit, 3, 3,
                                   crate::evolve::Budget::evals(160, 16), &mut arch, "hall/gothic");
        assert_eq!(levels.len(), 3);
        let total: usize = levels.iter().map(|(_, e)| e.changes.len()).sum();
        assert!(total > levels[0].1.changes.len(), "later levels added to the first: {total}");
        assert!(arch.len() >= 3, "each level left elites: {}", arch.len());
        assert!(!arch.seeds("hall/gothic/L2").is_empty());
    }
}
