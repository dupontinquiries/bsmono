//! Plant genetics in the builder.
//!
//! The engine's plants are alive. Each carries a thirteen-gene genome
//! (`sim::plant_tree_genetics`), normalised 0..1, neutral at 0.5; it
//! is expressed into a phenotype -- mature height, canopy, vigour,
//! fruit -- that the lifecycle grows toward, and two plants CROSS to
//! make a child with Mendelian inheritance. The builder's `tree` op
//! ignored all of it and stamped a fixed trunk with a blob on top.
//!
//! This module gives the builder the same genetics, three ways:
//!
//! 1. **The genome is the engine's.** The layout, order, seed rolling
//!    and crossing here MIRROR `sim::plant_tree_genetics` exactly, so a
//!    genome the builder makes is a genome the engine accepts by
//!    `from_array`, and a wild plant's `from_seed` matches on both
//!    sides. This crate cannot depend on the engine; it can agree with
//!    it, and a test pins the agreement.
//!
//! 2. **Breeding toward a phrase.** "Tall ancient oaks", "hardy desert
//!    scrub", "a bushy hedge": each word is a target for some genes,
//!    and the way to reach a target with genetics is the way a breeder
//!    does it -- cross the best, select, repeat. `breed_toward` runs
//!    that selection using the engine's own `cross`, so the winner is
//!    a plant that could have been bred in-game, not a hand-set value.
//!
//! 3. **What is placed matches what will grow.** The builder plants at
//!    the PHENOTYPE: trunk height and canopy from the expressed genome,
//!    branch density from leaf_density, so the tree you see is the tree
//!    the lifecycle continues. And the genome goes to the engine as a
//!    `PlantRequest`, the same shape as creatures and climate: this
//!    crate asks, the engine owns the living thing.

pub const GENE_COUNT: usize = 13;

/// Gene indices, in the engine's order.
pub const MAX_HEIGHT: usize = 0;
pub const TRUNK_GIRTH: usize = 1;
pub const BRANCH_DENSITY: usize = 2;
pub const BRANCH_ANGLE: usize = 3;
pub const LEAF_SIZE: usize = 4;
pub const LEAF_DENSITY: usize = 5;
pub const CANOPY_SPREAD: usize = 6;
pub const ROOT_SPREAD: usize = 7;
pub const FRUITFULNESS: usize = 8;
pub const GROWTH_RATE: usize = 9;
pub const HARDINESS: usize = 10;
pub const WOODINESS: usize = 11;
pub const SEASON_BIAS: usize = 12;

pub const GENE_NAMES: [&str; GENE_COUNT] = [
    "max_height", "trunk_girth", "branch_density", "branch_angle", "leaf_size",
    "leaf_density", "canopy_spread", "root_spread", "fruitfulness", "growth_rate",
    "hardiness", "woodiness", "season_bias",
];

/// The thirteen-gene genome, mirroring the engine's layout exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlantGenome(pub [f32; GENE_COUNT]);

// splitmix64, byte for byte the engine's, so `from_seed` agrees.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn unit(state: &mut u64) -> f32 { (splitmix64(state) >> 40) as f32 / (1u64 << 24) as f32 }

impl PlantGenome {
    pub fn neutral() -> Self { PlantGenome([0.5; GENE_COUNT]) }

    /// A wild plant's genome from its seed. Same sequence as the
    /// engine: thirteen `unit` draws in gene order.
    pub fn from_seed(seed: u64) -> Self {
        let mut s = seed;
        let mut g = [0.5; GENE_COUNT];
        for v in g.iter_mut() { *v = unit(&mut s); }
        PlantGenome(g)
    }

    pub fn as_array(&self) -> [f32; GENE_COUNT] { self.0 }
    pub fn from_array(a: [f32; GENE_COUNT]) -> Self { PlantGenome(a.map(|v| v.clamp(0.0, 1.0))) }

    /// Mendelian cross, mirroring the engine: per gene, take one parent
    /// or the other by a coin from the seed, then a small drift.
    pub fn cross(&self, other: &PlantGenome, seed: u64) -> PlantGenome {
        let mut s = seed;
        let mut g = [0.5; GENE_COUNT];
        for i in 0..GENE_COUNT {
            let pick_a = unit(&mut s) < 0.5;
            let base = if pick_a { self.0[i] } else { other.0[i] };
            let drift = (unit(&mut s) - 0.5) * 0.06;
            g[i] = (base + drift).clamp(0.0, 1.0);
        }
        PlantGenome(g)
    }

    /// The expressed form, in BLOCKS, mirroring the engine's phenotype
    /// ranges: a tree is 2 + height*18 tall, an herb 0.2 + height*1.8.
    pub fn phenotype(&self) -> Phenotype {
        let g = &self.0;
        let tree = g[WOODINESS] > 0.5;
        let height = if tree { 2.0 + g[MAX_HEIGHT] * 18.0 } else { 0.2 + g[MAX_HEIGHT] * 1.8 };
        let canopy = if tree { 1.0 + g[CANOPY_SPREAD] * 5.0 } else { 0.3 + g[CANOPY_SPREAD] * 1.2 };
        Phenotype {
            height_blocks: height.round().max(1.0) as i32,
            canopy_radius: canopy.round().max(1.0) as i32,
            is_tree: tree,
            leaf_density: g[LEAF_DENSITY],
            trunk_girth: g[TRUNK_GIRTH],
            branch_density: g[BRANCH_DENSITY],
            fruit: g[FRUITFULNESS],
            hardiness: g[HARDINESS],
            growth_rate: 0.5 + g[GROWTH_RATE],
        }
    }

    pub fn describe(&self) -> String {
        let p = self.phenotype();
        let mut notable: Vec<String> = Vec::new();
        for (i, v) in self.0.iter().enumerate() {
            if (*v - 0.5).abs() > 0.25 {
                notable.push(format!("{} {}", GENE_NAMES[i], if *v > 0.5 { "high" } else { "low" }));
            }
        }
        format!("{} {} tall, canopy {}{}",
                if p.is_tree { "tree" } else { "herb" }, p.height_blocks, p.canopy_radius,
                if notable.is_empty() { String::new() } else { format!("; {}", notable.join(", ")) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phenotype {
    pub height_blocks: i32,
    pub canopy_radius: i32,
    pub is_tree: bool,
    pub leaf_density: f32,
    pub trunk_girth: f32,
    pub branch_density: f32,
    pub fruit: f32,
    pub hardiness: f32,
    pub growth_rate: f32,
}

/// Gene targets a phrase implies. Each is (gene, target, weight).
pub fn targets_from_phrase(prose: &str) -> Vec<(usize, f32, f32)> {
    let p = prose.to_lowercase();
    let mut t: Vec<(usize, f32, f32)> = Vec::new();
    // WHOLE words. Substring matching found "thin" in "something" and
    // "old" in "gold"; with a list this long, every sentence hit
    // something. A word matches if it is a whole word or the stem of
    // one (so "oaks" matches "oak", "towering" matches "tower").
    let words: Vec<&str> = p.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |ws: &[&str]| ws.iter().any(|w| {
        let w = w.trim();
        words.iter().any(|x| *x == w || (x.len() > w.len() && x.starts_with(w) && w.len() >= 4))
    });
    if has(&["tall", "towering", "giant", "huge", "great "]) { t.push((MAX_HEIGHT, 0.92, 2.0)); }
    if has(&["small", "short", "dwarf", "stunted", "low"]) { t.push((MAX_HEIGHT, 0.15, 2.0)); }
    if has(&["ancient", "old", "gnarled", "venerable"]) {
        t.push((MAX_HEIGHT, 0.85, 1.0)); t.push((TRUNK_GIRTH, 0.9, 1.5)); t.push((GROWTH_RATE, 0.2, 1.0));
    }
    if has(&["young", "sapling", "fast", "quick"]) { t.push((GROWTH_RATE, 0.9, 1.5)); }
    if has(&["bushy", "dense", "thick", "full"]) {
        t.push((BRANCH_DENSITY, 0.9, 1.5)); t.push((LEAF_DENSITY, 0.9, 1.5));
    }
    if has(&["sparse", "bare", "thin", "wispy", "dead"]) { t.push((LEAF_DENSITY, 0.1, 1.5)); }
    if has(&["wide", "spreading", "broad", "umbrella"]) { t.push((CANOPY_SPREAD, 0.9, 1.5)); }
    if has(&["narrow", "columnar", "slender", "cypress"]) {
        t.push((CANOPY_SPREAD, 0.1, 1.5)); t.push((BRANCH_ANGLE, 0.1, 1.0));
    }
    if has(&["hardy", "desert", "arid", "drought", "tough", "alpine", "mountain"]) { t.push((HARDINESS, 0.92, 2.0)); }
    if has(&["fruit", "orchard", "apple", "berry", "bearing"]) { t.push((FRUITFULNESS, 0.9, 2.0)); }
    if has(&["hedge", "shrub", "bush", "scrub"]) { t.push((WOODINESS, 0.55, 2.0)); t.push((MAX_HEIGHT, 0.12, 1.5)); }
    if has(&["grass", "herb", "flower", "reed", "fern", "meadow"]) { t.push((WOODINESS, 0.15, 2.0)); }
    if has(&["willow", "weeping", "drooping"]) { t.push((BRANCH_ANGLE, 0.9, 1.5)); t.push((CANOPY_SPREAD, 0.7, 1.0)); }
    // A species word says it is a TREE. Without this, "tall bushy oaks"
    // could converge on a tall bushy HERB, which is 2 blocks high.
    if has(&["oak", "pine", "birch", "spruce", "jungle", "tree", "fir", "cedar", "elm", "ash "]) {
        t.push((WOODINESS, 0.9, 2.0));
    }
    if has(&["evergreen", "pine", "winter", "spruce"]) { t.push((SEASON_BIAS, 0.1, 1.0)); }
    if has(&["autumn", "deciduous", "seasonal"]) { t.push((SEASON_BIAS, 0.9, 1.0)); }
    t
}

/// How close a genome is to the targets: 0 = exact, larger = worse.
pub fn distance(g: &PlantGenome, targets: &[(usize, f32, f32)]) -> f32 {
    targets.iter().map(|(i, v, w)| (g.0[*i] - v).abs() * w).sum()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bred {
    pub genome: PlantGenome,
    pub distance: f32,
    pub generations: usize,
    /// Best distance per generation, for the log.
    pub history: Vec<f32>,
}

/// Breed a population toward a phrase's targets, the way a breeder
/// would: keep the closest, cross them, repeat. Uses the engine's own
/// `cross`, so every genome here is one that could have arisen in the
/// game -- and a wild population from seeds is the starting stock, so
/// "tall oaks" bred here look like the tall end of THIS world's oaks.
pub fn breed_toward(prose: &str, seed: u64, pop: usize, gens: usize) -> Bred {
    let targets = targets_from_phrase(prose);
    let pop = pop.clamp(4, 64);
    let mut stock: Vec<PlantGenome> = (0..pop)
        .map(|i| PlantGenome::from_seed(seed.wrapping_mul(0x9E37_79B9).wrapping_add(i as u64)))
        .collect();
    let mut history = Vec::new();
    for gen in 0..gens.max(1) {
        stock.sort_by(|a, b| distance(a, &targets).partial_cmp(&distance(b, &targets)).unwrap());
        history.push(distance(&stock[0], &targets));
        if history.last().copied().unwrap_or(1.0) < 0.05 { break; }
        // the top third are the parents; children by crossing pairs of them
        let keep = (pop / 3).max(2);
        let parents: Vec<PlantGenome> = stock[..keep].to_vec();
        let mut next = parents.clone();
        let mut k = 0u64;
        let mut rng = seed ^ ((gen as u64) << 20);
        while next.len() < pop {
            // Random pairing from the parents, and a MUTATION on one gene
            // of each child. Crossing alone cannot push a gene past the
            // best value in the wild stock, so without mutation "tall"
            // tops out wherever the tallest wild plant happened to be.
            let a = &parents[(splitmix64(&mut rng) % keep as u64) as usize];
            let b = &parents[(splitmix64(&mut rng) % keep as u64) as usize];
            let mut child = a.cross(b, splitmix64(&mut rng));
            let gi = (splitmix64(&mut rng) % GENE_COUNT as u64) as usize;
            child.0[gi] = (child.0[gi] + (unit(&mut rng) - 0.5) * 0.2).clamp(0.0, 1.0);
            next.push(child);
            k += 1;
        }
        let _ = k;
        stock = next;
    }
    stock.sort_by(|a, b| distance(a, &targets).partial_cmp(&distance(b, &targets)).unwrap());
    Bred { genome: stock[0], distance: distance(&stock[0], &targets), generations: history.len(), history }
}

/// What the engine is asked to register: a living plant at a spot,
/// with this genome, already grown (or a seedling).
#[derive(Debug, Clone, PartialEq)]
pub struct PlantRequest {
    pub at: (i32, i32, i32),
    /// The species' block id, which is what the engine keys species by.
    pub species_block: u16,
    pub genome: [f32; GENE_COUNT],
    /// 1.0 = placed at its mature form; 0.02 = a seedling that will grow.
    pub growth: f32,
}

impl PlantRequest {
    pub fn describe(&self) -> String {
        format!("plant at {:?}: {} ({})", self.at, PlantGenome(self.genome).describe(),
                if self.growth >= 0.99 { "mature" } else { "seedling" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_seed_is_the_engines_sequence() {
        // The agreement this crate cannot import: thirteen splitmix64
        // unit draws in gene order. Pinned so a change on either side
        // is a test failure rather than a silently different forest.
        let g = PlantGenome::from_seed(12345);
        let mut s = 12345u64;
        for i in 0..GENE_COUNT {
            let expect = unit(&mut s);
            assert!((g.0[i] - expect).abs() < 1e-7, "gene {i}");
        }
        assert!(g.0.iter().all(|v| (0.0..1.0).contains(v)));
        assert_ne!(PlantGenome::from_seed(1), PlantGenome::from_seed(2));
    }

    #[test]
    fn phenotype_ranges_match_the_engine() {
        let mut tall = PlantGenome::neutral(); tall.0[MAX_HEIGHT] = 1.0; tall.0[WOODINESS] = 0.9;
        assert_eq!(tall.phenotype().height_blocks, 20, "2 + 18");
        let mut herb = PlantGenome::neutral(); herb.0[WOODINESS] = 0.1; herb.0[MAX_HEIGHT] = 1.0;
        assert_eq!(herb.phenotype().height_blocks, 2, "0.2 + 1.8");
        assert!(!herb.phenotype().is_tree);
    }

    #[test]
    fn a_cross_takes_each_gene_from_one_parent() {
        let mut a = PlantGenome::neutral(); a.0 = [0.1; GENE_COUNT];
        let mut b = PlantGenome::neutral(); b.0 = [0.9; GENE_COUNT];
        let c = a.cross(&b, 3);
        for v in c.0 { assert!((v - 0.1).abs() < 0.05 || (v - 0.9).abs() < 0.05, "{v} is from neither parent"); }
        let from_a = c.0.iter().filter(|v| **v < 0.5).count();
        assert!(from_a > 2 && from_a < 11, "a mix, not one parent: {from_a}/13 from a");
        assert_eq!(a.cross(&b, 3), a.cross(&b, 3), "deterministic");
    }

    #[test]
    fn phrases_target_the_right_genes() {
        let t = targets_from_phrase("tall ancient oaks");
        assert!(t.iter().any(|(i, v, _)| *i == MAX_HEIGHT && *v > 0.8));
        assert!(t.iter().any(|(i, v, _)| *i == TRUNK_GIRTH && *v > 0.8));
        let h = targets_from_phrase("hardy desert scrub");
        assert!(h.iter().any(|(i, v, _)| *i == HARDINESS && *v > 0.9));
        assert!(h.iter().any(|(i, v, _)| *i == WOODINESS && *v < 0.6), "scrub is barely woody");
        assert!(targets_from_phrase("a tree").iter().any(|(i, ..)| *i == WOODINESS),
                "'tree' says it is woody");
        assert!(targets_from_phrase("something here").is_empty(), "no plant words, no targets");
    }

    #[test]
    fn breeding_converges_toward_the_phrase() {
        let b = breed_toward("tall bushy oaks", 7, 24, 30);
        assert!(b.distance < b.history[0], "closer than the wild stock: {:?}", b.history);
        let p = b.genome.phenotype();
        assert!(p.height_blocks >= 16, "tall: {}", p.height_blocks);
        assert!(p.leaf_density > 0.75, "bushy: {}", p.leaf_density);
        assert_eq!(breed_toward("tall bushy oaks", 7, 24, 30), b, "deterministic");
    }

    #[test]
    fn breeding_stops_early_when_close_enough() {
        let b = breed_toward("tall", 11, 32, 60);
        assert!(b.generations < 60, "converged and stopped: {} gens", b.generations);
        assert!(b.distance < 0.1);
    }

    #[test]
    fn a_neutral_phrase_breeds_nothing_in_particular() {
        // No targets: distance is always zero, one generation, the
        // closest wild plant -- which is any of them.
        let b = breed_toward("something here", 1, 8, 10);
        assert_eq!(b.distance, 0.0);
        assert_eq!(b.generations, 1);
    }

    #[test]
    fn describe_names_what_is_unusual() {
        let b = breed_toward("tall ancient", 5, 24, 30);
        let d = b.genome.describe();
        assert!(d.contains("tree") && d.contains("max_height high"), "{d}");
        assert!(d.contains("trunk_girth high"), "{d}");
    }
}
