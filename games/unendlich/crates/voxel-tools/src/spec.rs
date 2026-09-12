//! Spec-driven tuning: the model says what the build should MEASURE
//! like, and a genetic pool of placers and destroyers makes it so.
//!
//! THE IDEA
//!
//! A model is bad at coordinates and good at intent. Alongside the
//! plan it can emit one line of PROPERTIES -- block count, height,
//! durability, how it sounds inside, how sheltered it is, symmetry,
//! openness -- as numbers, ranges and enums. Those are cheap tokens
//! and they are the part a model actually knows: a bunker is durable,
//! sealed and low; a pavilion is airy, open and light.
//!
//! The plan's dry run produces a first build. It is measured against
//! the spec. Then a POOL of small edits -- place a slab here, remove a
//! block there, thicken this wall, cap that opening, raise the top --
//! is evolved: each genome is a short list of edits, fitness is the
//! distance to the spec plus a penalty for floating blocks, and the
//! winner's edits are appended to the plan as a tuning pass. The model
//! never saw a coordinate; the build still ends up 900 blocks, 14
//! tall, thick-walled and sealed, because those were the numbers.
//!
//! POST-TRAINING
//!
//! Every tuned build appends one line to a training record: the
//! prose, the spec the model wrote, what the untuned plan measured,
//! what the tuned one measured, and how many generations it took. That
//! is a dataset -- prompt, target, outcome -- for fine-tuning the model
//! to write specs its plans already meet, which is the loop closing.
//!
//! Everything here is pure and seeded: the same plan, spec and seed
//! tune the same way.

use crate::weather::Change;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level { Low, Mid, High }

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.to_lowercase().as_str() {
            "low" | "light" | "thin" | "fragile" | "exposed" | "open" => Level::Low,
            "mid" | "medium" | "normal" | "some" => Level::Mid,
            "high" | "heavy" | "thick" | "solid" | "sheltered" | "sealed" => Level::High,
            _ => return None,
        })
    }
    pub fn target(self) -> f32 { match self { Level::Low => 0.2, Level::Mid => 0.5, Level::High => 0.8 } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound { Echoing, Muffled, Airy }

impl Sound {
    pub fn parse(s: &str) -> Option<Sound> {
        Some(match s.to_lowercase().as_str() {
            "echoing" | "echo" | "cavernous" | "hall" => Sound::Echoing,
            "muffled" | "quiet" | "close" | "snug" => Sound::Muffled,
            "airy" | "open" | "outdoor" | "windy" => Sound::Airy,
            _ => return None,
        })
    }
}

/// What the model asked for. Every field optional: a spec that names
/// only height only constrains height.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spec {
    pub blocks: Option<(usize, usize)>,
    pub height: Option<(i32, i32)>,
    /// Wall thickness: fraction of solid cells with 5+ solid neighbours.
    pub durability: Option<Level>,
    /// Enclosure: how bounded the interior air is.
    pub sound: Option<Sound>,
    /// Shelter: fraction of top surfaces under other solid.
    pub shelter: Option<Level>,
    pub symmetry: Option<f32>,
    /// Interior air over bounding box.
    pub openness: Option<f32>,
}

impl Spec {
    /// `spec blocks=800-1200 height=12-16 durability=high sound=echoing
    /// shelter=high symmetry=0.7 openness=0.3`
    pub fn parse(line: &str) -> Result<Spec, String> {
        let mut s = Spec::default();
        let mut any = false;
        for tok in line.split_whitespace().skip_while(|t| t.eq_ignore_ascii_case("spec")) {
            let Some((k, v)) = tok.split_once('=') else { continue };
            let range = |v: &str| -> Option<(i64, i64)> {
                if let Some((a, b)) = v.split_once('-') {
                    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                } else { let n: i64 = v.parse().ok()?; Some((n * 9 / 10, n * 11 / 10)) }
            };
            match k.to_lowercase().as_str() {
                "blocks" => { let (a, b) = range(v).ok_or("blocks wants N or A-B")?; s.blocks = Some((a.max(0) as usize, b.max(0) as usize)); }
                "height" => { let (a, b) = range(v).ok_or("height wants N or A-B")?; s.height = Some((a as i32, b as i32)); }
                "durability" => s.durability = Some(Level::parse(v).ok_or("durability: low|mid|high")?),
                "sound" => s.sound = Some(Sound::parse(v).ok_or("sound: echoing|muffled|airy")?),
                "shelter" | "weather" => s.shelter = Some(Level::parse(v).ok_or("shelter: low|mid|high")?),
                "symmetry" => s.symmetry = Some(v.parse::<f32>().map_err(|_| "symmetry: 0..1")?.clamp(0.0, 1.0)),
                "openness" => s.openness = Some(v.parse::<f32>().map_err(|_| "openness: 0..1")?.clamp(0.0, 1.0)),
                _ => continue,
            }
            any = true;
        }
        if any { Ok(s) } else { Err("spec names no property".into()) }
    }

    pub fn is_empty(&self) -> bool { *self == Spec::default() }

    pub fn describe(&self) -> String {
        let mut v = Vec::new();
        if let Some((a, b)) = self.blocks { v.push(format!("blocks {a}-{b}")); }
        if let Some((a, b)) = self.height { v.push(format!("height {a}-{b}")); }
        if let Some(d) = self.durability { v.push(format!("durability {d:?}").to_lowercase()); }
        if let Some(s) = self.sound { v.push(format!("sound {s:?}").to_lowercase()); }
        if let Some(s) = self.shelter { v.push(format!("shelter {s:?}").to_lowercase()); }
        if let Some(s) = self.symmetry { v.push(format!("symmetry {s:.1}")); }
        if let Some(o) = self.openness { v.push(format!("openness {o:.1}")); }
        v.join(", ")
    }
}

/// What a build measures.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Props {
    pub blocks: usize,
    pub height: i32,
    pub durability: f32,
    /// 0 = airy, 1 = fully enclosed
    pub enclosure: f32,
    pub shelter: f32,
    pub symmetry: f32,
    pub openness: f32,
    pub floating: usize,
}

impl Props {
    pub fn describe(&self) -> String {
        format!("{} blocks, {} tall, durability {:.2}, enclosure {:.2}, shelter {:.2}, symmetry {:.2}, \
                 openness {:.2}{}", self.blocks, self.height, self.durability, self.enclosure, self.shelter,
                self.symmetry, self.openness,
                if self.floating > 0 { format!(", {} floating", self.floating) } else { String::new() })
    }
}

/// Measure a set of solid cells. Pure; the ground is `ground_y` (cells
/// at or below it are supported by the world).
pub fn measure(cells: &BTreeSet<(i32, i32, i32)>, ground_y: i32) -> Props {
    if cells.is_empty() { return Props::default(); }
    let lo = cells.iter().fold((i32::MAX, i32::MAX, i32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1), a.2.min(p.2)));
    let hi = cells.iter().fold((i32::MIN, i32::MIN, i32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1), a.2.max(p.2)));
    let n6 = |p: &(i32, i32, i32)| [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
        .iter().filter(|d| cells.contains(&(p.0 + d.0, p.1 + d.1, p.2 + d.2))).count();
    // Five neighbours, not four: a cell in a one-thick wall already has
    // four along the wall, and counted as thick. Five means solid behind
    // it as well -- a two-thick wall or an interior cell.
    let thick = cells.iter().filter(|p| n6(p) >= 5).count();
    // Interior air: cells in the bounding box that are not solid and
    // are bounded on all four horizontal sides and above by solid
    // somewhere along each ray. Cheap version: the air cell has solid
    // above it AND solid on both x sides AND both z sides within the box.
    // Bounded-ness by PROJECTION rather than rays. A ray per air cell
    // per direction is O(box) each; over a 40x20x30 build that is
    // millions per evaluation, times a population, times generations.
    // The extents of solid along each row and column answer the same
    // question in O(1): an air cell is bounded on x if it lies between
    // the min and max solid x of its (y,z) row, and so on.
    let mut row_x: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();   // (y,z) -> (minx, maxx)
    let mut row_z: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();   // (x,y) -> (minz, maxz)
    let mut col_y: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();   // (x,z) -> (miny, maxy)
    for p in cells {
        let e = row_x.entry((p.1, p.2)).or_insert((p.0, p.0)); e.0 = e.0.min(p.0); e.1 = e.1.max(p.0);
        let e = row_z.entry((p.0, p.1)).or_insert((p.2, p.2)); e.0 = e.0.min(p.2); e.1 = e.1.max(p.2);
        let e = col_y.entry((p.0, p.2)).or_insert((p.1, p.1)); e.0 = e.0.min(p.1); e.1 = e.1.max(p.1);
    }
    let mut interior = 0usize;
    let mut bounded = 0usize;
    for x in lo.0..=hi.0 { for y in lo.1..=hi.1 { for z in lo.2..=hi.2 {
        if cells.contains(&(x, y, z)) { continue; }
        let bx = row_x.get(&(y, z)).map(|(a, b)| x > *a && x < *b).unwrap_or(false);
        let bz = row_z.get(&(x, y)).map(|(a, b)| z > *a && z < *b).unwrap_or(false);
        if bx && bz {
            interior += 1;
            if col_y.get(&(x, z)).map(|(_, b)| y < *b).unwrap_or(false) { bounded += 1; }
        }
    }}}
    // Shelter: top cells (nothing above) vs cells with solid somewhere above.
    let tops: BTreeMap<(i32, i32), i32> = cells.iter().fold(BTreeMap::new(), |mut m, p| {
        let e = m.entry((p.0, p.2)).or_insert(i32::MIN); *e = (*e).max(p.1); m });
    let covered = cells.iter().filter(|p| tops.get(&(p.0, p.2)).map(|t| *t > p.1).unwrap_or(false)).count();
    // Symmetry across the box's x midline.
    let mirror_ok = cells.iter().filter(|p| cells.contains(&(lo.0 + hi.0 - p.0, p.1, p.2))).count();
    // Floating: a cell with no solid below it anywhere down to the
    // ground and no solid neighbour at all.
    let floating = cells.iter().filter(|p| {
        p.1 > ground_y + 1 && n6(p) == 0
    }).count();
    let vol = ((hi.0 - lo.0 + 1) as f32) * ((hi.1 - lo.1 + 1) as f32) * ((hi.2 - lo.2 + 1) as f32);
    Props {
        blocks: cells.len(),
        height: hi.1 - lo.1 + 1,
        durability: thick as f32 / cells.len() as f32,
        enclosure: if interior == 0 { 0.0 } else { bounded as f32 / interior as f32 },
        shelter: covered as f32 / cells.len() as f32,
        symmetry: mirror_ok as f32 / cells.len() as f32,
        openness: interior as f32 / vol.max(1.0),
        floating,
    }
}

/// Distance from a spec: 0 is perfect. Each named property contributes
/// its normalised miss; floating blocks are a flat penalty.
pub fn distance(spec: &Spec, p: &Props) -> f32 {
    let mut d = 0.0;
    let miss = |v: f32, lo: f32, hi: f32| if v < lo { (lo - v) / lo.max(1.0) } else if v > hi { (v - hi) / hi.max(1.0) } else { 0.0 };
    if let Some((a, b)) = spec.blocks { d += miss(p.blocks as f32, a as f32, b as f32) * 2.0; }
    if let Some((a, b)) = spec.height { d += miss(p.height as f32, a as f32, b as f32) * 2.0; }
    if let Some(l) = spec.durability { d += (p.durability - l.target()).abs(); }
    if let Some(s) = spec.sound {
        let want = match s { Sound::Echoing => (0.85, 0.25), Sound::Muffled => (0.85, 0.08), Sound::Airy => (0.15, 0.4) };
        d += (p.enclosure - want.0).abs() + (p.openness - want.1).abs() * 0.5;
    }
    if let Some(l) = spec.shelter { d += (p.shelter - l.target()).abs(); }
    if let Some(s) = spec.symmetry { d += (p.symmetry - s).abs(); }
    if let Some(o) = spec.openness { d += (p.openness - o).abs(); }
    d + p.floating as f32 * 0.05
}

/// One edit in the pool. Coordinates are relative to the build's box
/// as fractions, so a genome is meaningful for any box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edit {
    /// Fill a slab: at height fraction, spanning fractions of x and z.
    Slab { y: f32, x0: f32, x1: f32, z0: f32, z1: f32 },
    /// Remove a box.
    Cut { x0: f32, x1: f32, y0: f32, y1: f32, z0: f32, z1: f32 },
    /// Thicken: add a block outside every exposed face at height band.
    Thicken { y0: f32, y1: f32 },
    /// Cap: fill the top face at the box's max y + 1.
    Cap,
    /// Raise: copy the top course up by n.
    Raise(u8),
    /// Pillar at a fraction position, full height.
    Pillar { x: f32, z: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Genome { pub edits: Vec<Edit> }

pub const MAX_EDITS: usize = 6;

fn hash(seed: u64, a: u64, b: u64) -> u64 {
    let mut h = seed ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}
fn unit(seed: u64, a: u64, b: u64) -> f32 { (hash(seed, a, b) % 10_000) as f32 / 10_000.0 }

impl Genome {
    pub fn random(seed: u64) -> Self {
        let n = 1 + (hash(seed, 1, 0) % 3) as usize;
        Genome { edits: (0..n).map(|i| Self::random_edit(seed, i as u64)).collect() }
    }
    fn random_edit(seed: u64, k: u64) -> Edit {
        let u = |i: u64| unit(seed, k * 8 + i, 77);
        match hash(seed, k, 5) % 6 {
            0 => { let x0 = u(1) * 0.6; let z0 = u(2) * 0.6;
                   Edit::Slab { y: u(0), x0, x1: x0 + 0.3 + u(3) * 0.4, z0, z1: z0 + 0.3 + u(4) * 0.4 } }
            1 => { let x0 = u(1) * 0.7; let y0 = u(2) * 0.7; let z0 = u(3) * 0.7;
                   Edit::Cut { x0, x1: x0 + 0.1 + u(4) * 0.2, y0, y1: y0 + 0.1 + u(5) * 0.3, z0, z1: z0 + 0.1 + u(6) * 0.2 } }
            2 => { let y0 = u(1) * 0.8; Edit::Thicken { y0, y1: y0 + 0.2 + u(2) * 0.4 } }
            3 => Edit::Cap,
            4 => Edit::Raise(1 + (hash(seed, k, 9) % 3) as u8),
            _ => Edit::Pillar { x: 0.1 + u(1) * 0.8, z: 0.1 + u(2) * 0.8 },
        }
    }
    pub fn mutate(&self, seed: u64) -> Self {
        let mut g = self.clone();
        match hash(seed, 21, 0) % 3 {
            0 if g.edits.len() < MAX_EDITS => g.edits.push(Self::random_edit(seed, 99)),
            1 if g.edits.len() > 1 => { let i = (hash(seed, 22, 0) % g.edits.len() as u64) as usize; g.edits.remove(i); }
            _ => { let i = (hash(seed, 23, 0) % g.edits.len() as u64) as usize; g.edits[i] = Self::random_edit(seed, 98); }
        }
        g
    }
    pub fn crossover(&self, o: &Genome, seed: u64) -> Self {
        let cut = (hash(seed, 31, 0) % (self.edits.len() as u64 + 1)) as usize;
        let mut edits: Vec<Edit> = self.edits[..cut].to_vec();
        edits.extend(o.edits.iter().skip(cut).copied());
        edits.truncate(MAX_EDITS);
        if edits.is_empty() { edits.push(o.edits[0]); }
        Genome { edits }
    }
}

impl crate::evolve::Genome for Genome {
    fn mutate(&self, seed: u64) -> Self { Genome::mutate(self, seed) }
    fn crossover(&self, other: &Self, seed: u64) -> Self { Genome::crossover(self, other, seed) }
}

/// Each named property's miss as its own objective, plus floating.
/// Lower is better. The front then holds "closer on height, further on
/// durability" and its opposite, and the pick is by the spec's own
/// emphasis rather than one sum.
pub fn objectives(spec: &Spec, p: &Props) -> Vec<f32> {
    let miss = |v: f32, lo: f32, hi: f32| if v < lo { (lo - v) / lo.max(1.0) } else if v > hi { (v - hi) / hi.max(1.0) } else { 0.0 };
    let mut o = Vec::new();
    if let Some((a, b)) = spec.blocks { o.push(miss(p.blocks as f32, a as f32, b as f32)); }
    if let Some((a, b)) = spec.height { o.push(miss(p.height as f32, a as f32, b as f32)); }
    if let Some(l) = spec.durability { o.push((p.durability - l.target()).abs()); }
    if let Some(s) = spec.sound {
        let want = match s { Sound::Echoing => (0.85, 0.25), Sound::Muffled => (0.85, 0.08), Sound::Airy => (0.15, 0.4) };
        o.push((p.enclosure - want.0).abs() + (p.openness - want.1).abs() * 0.5);
    }
    if let Some(l) = spec.shelter { o.push((p.shelter - l.target()).abs()); }
    if let Some(s) = spec.symmetry { o.push((p.symmetry - s).abs()); }
    if let Some(v) = spec.openness { o.push((p.openness - v).abs()); }
    o.push(p.floating as f32 * 0.05);
    o
}

/// Apply a genome to a cell set within a box, returning the changes
/// (adds as `to = block`, removals as `to = 0`) and the new set.
pub fn apply(g: &Genome, cells: &BTreeSet<(i32, i32, i32)>, bounds: ((i32, i32, i32), (i32, i32, i32)),
             block: u16) -> (Vec<Change>, BTreeSet<(i32, i32, i32)>) {
    let (lo, hi) = bounds;
    let (w, h, d) = ((hi.0 - lo.0 + 1) as f32, (hi.1 - lo.1 + 1) as f32, (hi.2 - lo.2 + 1) as f32);
    let fx = |f: f32| lo.0 + (f * w) as i32;
    let fy = |f: f32| lo.1 + (f * h) as i32;
    let fz = |f: f32| lo.2 + (f * d) as i32;
    let mut set = cells.clone();
    let mut out: Vec<Change> = Vec::new();
    let add = |set: &mut BTreeSet<(i32, i32, i32)>, out: &mut Vec<Change>, p: (i32, i32, i32)| {
        if set.insert(p) { out.push(Change { at: p, to: block }); }
    };
    for e in &g.edits {
        match *e {
            Edit::Slab { y, x0, x1, z0, z1 } => {
                let yy = fy(y);
                for x in fx(x0)..=fx(x1) { for z in fz(z0)..=fz(z1) { add(&mut set, &mut out, (x, yy, z)); } }
            }
            Edit::Cut { x0, x1, y0, y1, z0, z1 } => {
                for x in fx(x0)..=fx(x1) { for y in fy(y0)..=fy(y1) { for z in fz(z0)..=fz(z1) {
                    if set.remove(&(x, y, z)) { out.push(Change { at: (x, y, z), to: 0 }); }
                }}}
            }
            Edit::Thicken { y0, y1 } => {
                let (ya, yb) = (fy(y0), fy(y1));
                let exposed: Vec<(i32, i32, i32)> = set.iter().filter(|p| p.1 >= ya && p.1 <= yb).copied().collect();
                for p in exposed {
                    for dd in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let q = (p.0 + dd.0, p.1, p.2 + dd.1);
                        if !set.contains(&q) && q.0 >= lo.0 - 1 && q.0 <= hi.0 + 1 && q.2 >= lo.2 - 1 && q.2 <= hi.2 + 1 {
                            add(&mut set, &mut out, q);
                        }
                    }
                }
            }
            Edit::Cap => {
                let top = hi.1;
                for x in lo.0..=hi.0 { for z in lo.2..=hi.2 { add(&mut set, &mut out, (x, top + 1, z)); } }
            }
            Edit::Raise(n) => {
                let top = set.iter().map(|p| p.1).max().unwrap_or(hi.1);
                let course: Vec<(i32, i32, i32)> = set.iter().filter(|p| p.1 == top).copied().collect();
                for k in 1..=n as i32 { for p in &course { add(&mut set, &mut out, (p.0, p.1 + k, p.2)); } }
            }
            Edit::Pillar { x, z } => {
                let (px, pz) = (fx(x), fz(z));
                for y in lo.1..=hi.1 { add(&mut set, &mut out, (px, y, pz)); }
            }
        }
    }
    (out, set)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tuned {
    pub genome: Genome,
    pub changes: Vec<Change>,
    pub before: Props,
    pub after: Props,
    pub distance_before: f32,
    pub distance_after: f32,
    pub generations: usize,
}

impl Tuned {
    pub fn improved(&self) -> bool { self.distance_after < self.distance_before - 1e-3 }
    pub fn summary(&self) -> String {
        format!("spec-tuned in {} gen(s): distance {:.2} -> {:.2}; {} edit(s), {} block change(s)",
                self.generations, self.distance_before, self.distance_after, self.genome.edits.len(),
                self.changes.len())
    }
}

/// Largest bounding box tuning will attempt. Each evaluation walks the
/// box; a landscape is not a thing to tune and a castle is at the edge.
pub const MAX_TUNE_CELLS: i64 = 60_000;

/// Evolve edits that bring the build toward the spec. Returns None when
/// the untuned build already meets it, nothing improves it, or the box
/// is too large to evaluate a population against.
pub fn tune(cells: &BTreeSet<(i32, i32, i32)>, bounds: ((i32, i32, i32), (i32, i32, i32)), ground_y: i32,
            spec: &Spec, block: u16, seed: u64, pop: usize, gens: usize) -> Option<Tuned> {
    let (lo, hi) = bounds;
    let vol = (hi.0 - lo.0 + 1) as i64 * (hi.1 - lo.1 + 1) as i64 * (hi.2 - lo.2 + 1) as i64;
    if vol > MAX_TUNE_CELLS { return None; }
    let before = measure(cells, ground_y);
    let d0 = distance(spec, &before);
    if d0 < 0.05 { return None; }
    let pop = pop.clamp(6, 48);
    // On the shared engine: Pareto-ranked over the spec's properties,
    // budgeted by evaluations, the pick by uniform weights (every named
    // property matters equally; the spec said which ones).
    let eval = |g: &Genome| { let (_, set) = apply(g, cells, bounds, block); objectives(spec, &measure(&set, ground_y)) };
    let out = crate::evolve::evolve(seed, crate::evolve::Budget::evals(pop * gens.max(1), pop), &[],
                                    Genome::random, eval, &[]);
    let used = out.generations;
    let best = crate::evolve::pick(&out.front, &[])?;
    let (changes, set) = apply(&best.genome, cells, bounds, block);
    let after = measure(&set, ground_y);
    let d1 = distance(spec, &after);
    let best: Option<(f32, Genome, Props, Vec<Change>)> = Some((d1, best.genome.clone(), after, changes));
    let (d1, genome, after, changes) = best?;
    let t = Tuned { genome, changes, before, after, distance_before: d0, distance_after: d1, generations: used };
    if t.improved() { Some(t) } else { None }
}

/// One line of the training record: prose, spec, before, after.
/// Tab-separated so it is greppable and loads into anything.
pub fn training_line(prose: &str, spec: &Spec, t: &Tuned) -> String {
    let clean = |s: &str| s.replace(['\t', '\n'], " ");
    format!("{}\t{}\t{}\t{}\t{}\t{:.3}\t{:.3}", clean(prose), spec.describe(), t.before.describe(),
            t.after.describe(), t.generations, t.distance_before, t.distance_after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hollow_box(w: i32, h: i32, d: i32) -> BTreeSet<(i32, i32, i32)> {
        let mut s = BTreeSet::new();
        for x in 0..w { for y in 0..h { for z in 0..d {
            if x == 0 || x == w - 1 || z == 0 || z == d - 1 || y == 0 { s.insert((x, y, z)); }
        }}}
        s
    }

    #[test]
    fn a_spec_line_parses_ranges_enums_and_fractions() {
        let s = Spec::parse("spec blocks=800-1200 height=14 durability=high sound=echoing shelter=sheltered symmetry=0.7 openness=0.3").unwrap();
        assert_eq!(s.blocks, Some((800, 1200)));
        assert_eq!(s.height, Some((12, 15)), "a bare number is +-10%");
        assert_eq!(s.durability, Some(Level::High));
        assert_eq!(s.sound, Some(Sound::Echoing));
        assert_eq!(s.shelter, Some(Level::High));
        assert!((s.symmetry.unwrap() - 0.7).abs() < 1e-6);
        assert!(Spec::parse("spec").is_err());
        assert!(Spec::parse("spec durability=purple").is_err());
    }

    #[test]
    fn measurement_reads_the_way_a_builder_would() {
        let open = hollow_box(10, 6, 8);           // walls and floor, no roof
        let p = measure(&open, -1);
        assert!(p.enclosure < 0.2, "no roof: airy {p:?}");
        let mut roofed = open.clone();
        for x in 0..10 { for z in 0..8 { roofed.insert((x, 5, z)); } }
        let q = measure(&roofed, -1);
        assert!(q.enclosure > 0.9, "roofed: enclosed {q:?}");
        assert!(q.shelter > p.shelter, "the roof shelters what is under it");
        assert!((q.symmetry - 1.0).abs() < 1e-3, "a box is symmetric");
        assert!(q.durability < 0.5, "one-thick walls are not durable: {}", q.durability);
        let mut floating = roofed.clone();
        floating.insert((4, 20, 4));
        assert_eq!(measure(&floating, -1).floating, 1);
    }

    #[test]
    fn tuning_makes_a_box_meet_a_bunker_spec() {
        // A thin open box asked to be durable, sealed and muffled: the
        // pool has to thicken the walls and cap it.
        let cells = hollow_box(10, 6, 8);
        let spec = Spec::parse("spec durability=high sound=muffled shelter=high").unwrap();
        let bounds = ((0, 0, 0), (9, 5, 7));
        let t = tune(&cells, bounds, -1, &spec, 1, 7, 12, 12).expect("something improves it");
        assert!(t.distance_after < t.distance_before, "{}", t.summary());
        assert!(t.after.enclosure > t.before.enclosure || t.after.durability > t.before.durability, "{:?} -> {:?}", t.before, t.after);
        assert!(t.after.floating == 0, "tuning never leaves floating blocks: {:?}", t.after);
        assert!(!t.changes.is_empty());
    }

    #[test]
    fn tuning_can_add_blocks_to_reach_a_count_and_a_height() {
        let cells = hollow_box(8, 4, 6);
        let n0 = cells.len();
        let spec = Spec::parse("spec blocks=300-400 height=8-10").unwrap();
        let t = tune(&cells, ((0, 0, 0), (7, 3, 5)), -1, &spec, 1, 3, 12, 14).expect("improves");
        assert!(t.after.blocks > n0, "grew: {} -> {}", n0, t.after.blocks);
        assert!(t.after.height > 4, "and rose: {}", t.after.height);
    }

    #[test]
    fn a_build_that_already_meets_its_spec_is_left_alone() {
        let cells = hollow_box(10, 6, 8);
        let p = measure(&cells, -1);
        let spec = Spec { blocks: Some((p.blocks - 5, p.blocks + 5)), height: Some((6, 6)), ..Default::default() };
        assert!(tune(&cells, ((0, 0, 0), (9, 5, 7)), -1, &spec, 1, 1, 12, 10).is_none());
    }

    #[test]
    fn tuning_is_deterministic() {
        let cells = hollow_box(10, 6, 8);
        let spec = Spec::parse("spec sound=echoing durability=mid").unwrap();
        let a = tune(&cells, ((0, 0, 0), (9, 5, 7)), -1, &spec, 1, 5, 10, 8);
        let b = tune(&cells, ((0, 0, 0), (9, 5, 7)), -1, &spec, 1, 5, 10, 8);
        assert_eq!(a, b);
    }

    #[test]
    fn the_training_line_is_one_line_with_the_fields_a_fine_tune_needs() {
        let cells = hollow_box(10, 6, 8);
        let spec = Spec::parse("spec durability=high sound=muffled").unwrap();
        let t = tune(&cells, ((0, 0, 0), (9, 5, 7)), -1, &spec, 1, 7, 10, 8).unwrap();
        let line = training_line("a bunker\nwith a\ttab", &spec, &t);
        assert_eq!(line.lines().count(), 1);
        assert_eq!(line.split('\t').count(), 7, "prose, spec, before, after, gens, d0, d1");
        assert!(line.starts_with("a bunker with a tab\t"));
        assert!(line.contains("durability high"));
    }
}
