//! Genetic fill — evolve an assembly of catalog parts to match a target region.
//!
//! Why this rather than the GAN: the conditional net kept finding the output
//! DISTRIBUTION without conditioning on its input (+35% vs identity, -0.6% vs
//! mean, and visibly the same texture for different targets). That is a hard
//! problem with a few hundred samples. Evolutionary search sidesteps it entirely:
//!
//!   * no training data needed -- the target IS the fitness function
//!   * it optimises the metric we actually care about, rather than a proxy loss
//!   * every candidate is real catalog geometry, so it cannot produce a smear
//!   * no GPU
//!
//! MULTI-PASS with STRIDES: pass 1 fits a few large boxes over the whole region,
//! pass 2 subdivides wherever the residual is worst and fits smaller boxes there,
//! and so on. Coarse structure is settled before detail, which is also why the
//! result reads as built rather than scattered.
//!
//! LINKER: after each pass, parts that end up adjacent are pulled into contact and
//! aligned, so the assembly reads as connected structure instead of a swarm. That
//! was the single most-reported fault of the earlier placement work.

use crate::mesh::Mesh;

#[derive(Clone, Copy)]
pub struct GenePart {
    pub part: usize,        // index into the catalog
    pub pos: [f32; 3],
    pub scale: [f32; 3],
    pub yaw: f32,           // quarter-turn family; parts are box-ish so this is enough
}

#[derive(Clone)]
pub struct Genome { pub genes: Vec<GenePart>, pub fitness: f32 }

pub struct Rng(u64);
impl Rng {
    pub fn new(s: u64) -> Self { Rng(s | 1) }
    pub fn n(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    pub fn f(&mut self) -> f32 { (self.n() % 1_000_000) as f32 / 1_000_000.0 }
    pub fn range(&mut self, a: f32, b: f32) -> f32 { a + self.f() * (b - a) }
    pub fn idx(&mut self, n: usize) -> usize { (self.n() % n.max(1) as u64) as usize }
}

/// Coarse occupancy grid — the shared currency for fitness. Comparing occupancy
/// rather than depth maps means a part counts if it is in the right PLACE, which
/// is what "fill this region" actually asks.
pub const G: usize = 16;

pub fn occupancy(v: &[[f32; 3]], lo: [f32; 3], span: [f32; 3]) -> Vec<bool> {
    let mut o = vec![false; G * G * G];
    for p in v {
        let x = (((p[0] - lo[0]) / span[0]) * (G - 1) as f32) as isize;
        let y = (((p[1] - lo[1]) / span[1]) * (G - 1) as f32) as isize;
        let z = (((p[2] - lo[2]) / span[2]) * (G - 1) as f32) as isize;
        if x < 0 || y < 0 || z < 0 || x >= G as isize || y >= G as isize || z >= G as isize { continue; }
        o[(x as usize * G + y as usize) * G + z as usize] = true;
    }
    o
}

/// Isolate a sub-region of a model to fill. Returns the vertices inside it.
///
/// The torso must be isolated LATERALLY, not radially: a T-posed bot is as wide as
/// its arm span, so a height band alone sweeps in both arms. That mistake put an
/// earlier rib pass out at x +/-1.03 on a torso 0.34 wide -- the "weird arm plates"
/// the user reported. Clipping radially instead then cut the chest DEPTH and hid
/// the result behind the shell. Lateral only is the form that works.
pub fn region(v: &[[f32; 3]], y0: f32, y1: f32, lateral: f32, ax: usize)
    -> (Vec<[f32; 3]>, [f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let ylo = lo[1] + (hi[1] - lo[1]) * y0;
    let yhi = lo[1] + (hi[1] - lo[1]) * y1;
    // centre line from the median x of the band, so an off-centre model still works
    // Clip on the model's OWN lateral axis. Assuming X put Jazz's chest region at
    // 0.14 wide by 2.00 deep -- a thin slice through the body with the full arm
    // span still in it -- because Jazz mirrors about Z. Same trap as the limb
    // splitter and the rib bands before it.
    let mut xs: Vec<f32> = v.iter().filter(|p| p[1] >= ylo && p[1] <= yhi).map(|p| p[ax]).collect();
    if xs.is_empty() { return (Vec::new(), lo, [1.0, 1.0, 1.0]); }
    xs.sort_by(|a, b| a.total_cmp(b));
    let cx = xs[xs.len() / 2];
    let halfw = (hi[ax] - lo[ax]) * 0.5 * lateral;
    let sel: Vec<[f32; 3]> = v.iter().copied()
        .filter(|p| p[1] >= ylo && p[1] <= yhi && (p[ax] - cx).abs() <= halfw).collect();
    if sel.is_empty() { return (Vec::new(), lo, [1.0, 1.0, 1.0]); }
    let mut rlo = [f32::MAX; 3];
    let mut rhi = [f32::MIN; 3];
    for p in &sel { for k in 0..3 { rlo[k] = rlo[k].min(p[k]); rhi[k] = rhi[k].max(p[k]); } }
    let span = [(rhi[0]-rlo[0]).max(1e-6), (rhi[1]-rlo[1]).max(1e-6), (rhi[2]-rlo[2]).max(1e-6)];
    (sel, rlo, span)
}

/// Cells INSIDE the shell rather than on it.
///
/// A chest region is a thin surface: matching it with solid parts is close to
/// impossible, because any box thick enough to register also spills through the
/// skin. What the original goal actually asked for -- structure inside a hollow
/// torso -- is the ENCLOSED VOLUME, which is a different target entirely.
///
/// Found by flood-filling from the grid boundary: whatever the flood cannot reach
/// is interior. That handles concave shapes correctly, where a convex hull would
/// swallow the gaps between limbs.
pub fn interior(shell: &[bool]) -> Vec<bool> {
    let idx = |x: usize, y: usize, z: usize| (x * G + y) * G + z;
    let mut outside = vec![false; G * G * G];
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    for x in 0..G { for y in 0..G { for z in 0..G {
        let boundary = x == 0 || y == 0 || z == 0 || x == G-1 || y == G-1 || z == G-1;
        if boundary && !shell[idx(x, y, z)] {
            outside[idx(x, y, z)] = true;
            stack.push((x, y, z));
        }
    }}}
    while let Some((x, y, z)) = stack.pop() {
        let mut push = |nx: isize, ny: isize, nz: isize, out: &mut Vec<bool>,
                        st: &mut Vec<(usize, usize, usize)>| {
            if nx < 0 || ny < 0 || nz < 0 || nx >= G as isize || ny >= G as isize || nz >= G as isize { return; }
            let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
            let i = idx(nx, ny, nz);
            if !out[i] && !shell[i] { out[i] = true; st.push((nx, ny, nz)); }
        };
        let (xi, yi, zi) = (x as isize, y as isize, z as isize);
        push(xi+1, yi, zi, &mut outside, &mut stack);
        push(xi-1, yi, zi, &mut outside, &mut stack);
        push(xi, yi+1, zi, &mut outside, &mut stack);
        push(xi, yi-1, zi, &mut outside, &mut stack);
        push(xi, yi, zi+1, &mut outside, &mut stack);
        push(xi, yi, zi-1, &mut outside, &mut stack);
    }
    (0..G*G*G).map(|i| !outside[i] && !shell[i]).collect()
}

pub struct Catalog { pub parts: Vec<Mesh>, pub names: Vec<String> }

impl Catalog {
    /// Part vertices placed by a gene, in the target's frame.
    pub fn place(&self, g: &GenePart) -> Vec<[f32; 3]> {
        let m = &self.parts[g.part % self.parts.len()];
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &m.v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
        let ext = [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)];
        let c = [(lo[0]+hi[0])*0.5, (lo[1]+hi[1])*0.5, (lo[2]+hi[2])*0.5];
        let (s, co) = (g.yaw.sin(), g.yaw.cos());
        m.v.iter().map(|p| {
            let q = [(p[0]-c[0]) / ext[0] * g.scale[0],
                     (p[1]-c[1]) / ext[1] * g.scale[1],
                     (p[2]-c[2]) / ext[2] * g.scale[2]];
            [g.pos[0] + q[0]*co + q[2]*s, g.pos[1] + q[1], g.pos[2] - q[0]*s + q[2]*co]
        }).collect()
    }
}

/// Fitness: IoU against the target occupancy, minus a penalty for spilling outside
/// it. Overlap alone would be gamed by one enormous part; the spill term is what
/// forces the population to actually fit the shape.
pub fn score(cat: &Catalog, gm: &Genome, target: &[bool], lo: [f32; 3], span: [f32; 3]) -> f32 {
    let mut have = vec![false; G * G * G];
    for g in &gm.genes {
        for p in cat.place(g) {
            let x = (((p[0] - lo[0]) / span[0]) * (G - 1) as f32) as isize;
            let y = (((p[1] - lo[1]) / span[1]) * (G - 1) as f32) as isize;
            let z = (((p[2] - lo[2]) / span[2]) * (G - 1) as f32) as isize;
            if x < 0 || y < 0 || z < 0 || x >= G as isize || y >= G as isize || z >= G as isize { continue; }
            have[(x as usize * G + y as usize) * G + z as usize] = true;
        }
    }
    let mut inter = 0usize;
    let mut union = 0usize;
    let mut spill = 0usize;
    for i in 0..target.len() {
        if target[i] && have[i] { inter += 1; }
        if target[i] || have[i] { union += 1; }
        if have[i] && !target[i] { spill += 1; }
    }
    let iou = inter as f32 / union.max(1) as f32;
    let sp = spill as f32 / target.iter().filter(|b| **b).count().max(1) as f32;
    iou - 0.35 * sp
}

/// Cell centres the target actually occupies, so genes can be seeded ONTO the
/// shape instead of scattered through its bounding box. Jazz fills 880 of 4096
/// cells -- uniform seeding therefore starts 79% of every gene in empty space and
/// spends its early generations just finding the model.
fn occupied_points(target: &[bool], lo: [f32; 3], span: [f32; 3]) -> Vec<[f32; 3]> {
    let mut v = Vec::new();
    for x in 0..G { for y in 0..G { for z in 0..G {
        if target[(x * G + y) * G + z] {
            v.push([lo[0] + (x as f32 + 0.5) / G as f32 * span[0],
                    lo[1] + (y as f32 + 0.5) / G as f32 * span[1],
                    lo[2] + (z as f32 + 0.5) / G as f32 * span[2]]);
        }
    }}}
    v
}

fn random_gene(rng: &mut Rng, n_parts: usize, lo: [f32; 3], span: [f32; 3], sc: f32,
               seeds: &[[f32; 3]]) -> GenePart {
    let pos = if seeds.is_empty() {
        [lo[0] + rng.f() * span[0], lo[1] + rng.f() * span[1], lo[2] + rng.f() * span[2]]
    } else {
        // jitter around an occupied cell rather than anywhere in the box
        let s = seeds[rng.idx(seeds.len())];
        [s[0] + rng.range(-0.5, 0.5) * span[0] / G as f32,
         s[1] + rng.range(-0.5, 0.5) * span[1] / G as f32,
         s[2] + rng.range(-0.5, 0.5) * span[2] / G as f32]
    };
    GenePart {
        part: rng.idx(n_parts),
        pos,
        scale: [span[0] * rng.range(sc * 0.4, sc), span[1] * rng.range(sc * 0.4, sc),
                span[2] * rng.range(sc * 0.4, sc)],
        yaw: (rng.idx(4) as f32) * std::f32::consts::FRAC_PI_2,
    }
}

/// The "hunting random walk": each survivor takes a step, and the step SHRINKS as
/// it improves. Large jumps early explore; small ones later refine. A fixed step
/// either never finds the basin or never settles in it.
fn mutate(rng: &mut Rng, g: &mut GenePart, n_parts: usize, span: [f32; 3], heat: f32,
          miss: &[[f32; 3]]) {
    match rng.idx(if miss.is_empty() { 4 } else { 5 }) {
        0 => g.part = rng.idx(n_parts),
        1 => for k in 0..3 { g.pos[k] += rng.range(-1.0, 1.0) * span[k] * 0.25 * heat; },
        4 => {
            // jump onto an unfilled target cell: undirected drift wastes most of
            // its moves, and coverage is what the fitness rewards
            if !miss.is_empty() {
                let m = miss[rng.idx(miss.len())];
                for k in 0..3 { g.pos[k] = m[k] + rng.range(-0.3, 0.3) * span[k] / G as f32; }
            }
        }
        2 => for k in 0..3 {
                g.scale[k] = (g.scale[k] * rng.range(1.0 - 0.5 * heat, 1.0 + 0.5 * heat))
                             .clamp(span[k] * 0.05, span[k] * 1.2);
             },
        _ => g.yaw = (rng.idx(4) as f32) * std::f32::consts::FRAC_PI_2,
    }
}

/// Pull adjacent parts into contact so the result reads as connected structure
/// rather than a swarm of floating pieces — the most-reported fault of the
/// earlier placement work.
pub fn link(cat: &Catalog, gm: &mut Genome, span: [f32; 3]) {
    let n = gm.genes.len();
    let reach = span[0].max(span[1]).max(span[2]) * 0.22;
    for i in 0..n {
        let mut best = f32::MAX;
        let mut bj = usize::MAX;
        for j in 0..n {
            if i == j { continue; }
            let d = (0..3).map(|k| (gm.genes[i].pos[k] - gm.genes[j].pos[k]).powi(2))
                          .sum::<f32>().sqrt();
            if d < best { best = d; bj = j; }
        }
        if bj != usize::MAX && best < reach && best > 1e-6 {
            // move a quarter of the way in: enough to close visible gaps without
            // collapsing the assembly onto a point
            for k in 0..3 {
                let d = gm.genes[bj].pos[k] - gm.genes[i].pos[k];
                gm.genes[i].pos[k] += d * 0.25;
            }
        }
    }
}

pub struct Cfg {
    pub pop: usize, pub gens: usize, pub genes: usize,
    pub elite: usize, pub seed: u64, pub scale: f32,
}

pub fn evolve(cat: &Catalog, target: &[bool], lo: [f32; 3], span: [f32; 3], cfg: &Cfg)
    -> (Genome, Vec<f32>) {
    let mut rng = Rng::new(cfg.seed);
    let np = cat.parts.len().max(1);
    let seeds = occupied_points(target, lo, span);
    let mut pop: Vec<Genome> = (0..cfg.pop).map(|_| {
        let genes = (0..cfg.genes)
            .map(|_| random_gene(&mut rng, np, lo, span, cfg.scale, &seeds)).collect();
        Genome { genes, fitness: f32::MIN }
    }).collect();
    for g in pop.iter_mut() { g.fitness = score(cat, g, target, lo, span); }
    let mut history = Vec::with_capacity(cfg.gens);

    let mut miss: Vec<[f32; 3]> = Vec::new();
    for gen in 0..cfg.gens {
        pop.sort_by(|a, b| b.fitness.total_cmp(&a.fitness));
        history.push(pop[0].fitness);
        // recompute what the current best is MISSING, so mutation has somewhere
        // useful to aim. Refreshed periodically rather than every generation --
        // it is the expensive part and it does not change fast.
        if gen % 5 == 0 {
            let mut have = vec![false; G * G * G];
            for g in &pop[0].genes {
                for p in cat.place(g) {
                    let x = (((p[0]-lo[0]) / span[0]) * (G-1) as f32) as isize;
                    let y = (((p[1]-lo[1]) / span[1]) * (G-1) as f32) as isize;
                    let z = (((p[2]-lo[2]) / span[2]) * (G-1) as f32) as isize;
                    if x < 0 || y < 0 || z < 0 || x >= G as isize || y >= G as isize || z >= G as isize { continue; }
                    have[(x as usize * G + y as usize) * G + z as usize] = true;
                }
            }
            miss.clear();
            for x in 0..G { for y in 0..G { for z in 0..G {
                let i = (x * G + y) * G + z;
                if target[i] && !have[i] {
                    miss.push([lo[0] + (x as f32 + 0.5) / G as f32 * span[0],
                               lo[1] + (y as f32 + 0.5) / G as f32 * span[1],
                               lo[2] + (z as f32 + 0.5) / G as f32 * span[2]]);
                }
            }}}
        }
        // heat decays: explore first, refine later
        let heat = 1.0 - (gen as f32 / cfg.gens.max(1) as f32) * 0.85;
        let mut next: Vec<Genome> = pop[..cfg.elite.min(pop.len())].to_vec();
        while next.len() < cfg.pop {
            // tournament of 3 — cheap, and keeps diversity better than roulette
            let a = rng.idx(pop.len().min(cfg.pop));
            let b = rng.idx(pop.len().min(cfg.pop));
            let c = rng.idx(pop.len().min(cfg.pop));
            let p1 = [a, b, c].iter().copied()
                .max_by(|x, y| pop[*x].fitness.total_cmp(&pop[*y].fitness)).unwrap();
            let d = rng.idx(pop.len().min(cfg.pop));
            let e = rng.idx(pop.len().min(cfg.pop));
            let p2 = if pop[d].fitness > pop[e].fitness { d } else { e };
            // uniform crossover over genes
            let mut child = Genome {
                genes: (0..cfg.genes).map(|i| {
                    let src = if rng.f() < 0.5 { &pop[p1] } else { &pop[p2] };
                    src.genes[i % src.genes.len()]
                }).collect(),
                fitness: f32::MIN,
            };
            for g in child.genes.iter_mut() {
                if rng.f() < 0.30 { mutate(&mut rng, g, np, span, heat, &miss); }
            }
            child.fitness = score(cat, &child, target, lo, span);
            next.push(child);
        }
        pop = next;
    }
    pop.sort_by(|a, b| b.fitness.total_cmp(&a.fitness));
    let mut best = pop.remove(0);
    link(cat, &mut best, span);
    best.fitness = score(cat, &best, target, lo, span);
    (best, history)
}

/// Bake an assembly into a mesh.
pub fn bake(cat: &Catalog, gm: &Genome) -> Mesh {
    let mut out = Mesh::default();
    for g in &gm.genes {
        let v = cat.place(g);
        let m = &cat.parts[g.part % cat.parts.len()];
        let base = out.v.len() as u32;
        out.v.extend_from_slice(&v);
        for t in &m.f { out.f.push([base + t[0], base + t[1], base + t[2]]); }
    }
    out
}
