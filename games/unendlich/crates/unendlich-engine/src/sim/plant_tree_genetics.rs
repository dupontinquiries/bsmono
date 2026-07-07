//! `sim::plant_tree_genetics` — plant & tree sub-genome, inheritance, phenotype.
//!
//! Mirrors the shape of `dragon_archetype::Genome` (normalized [0,1] genes,
//! neutral 0.5, splitmix64 `from_seed`). Drives `plant_growth`/`flora_harvest`
//! via `phenotype`. Composes the Mendelian/mutation idea of `popgen`/`gene_mutate`.
//! Pure, deterministic, panic-free.

pub const PLANT_GENE_COUNT: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantGenome {
    pub max_height: f32,
    pub trunk_girth: f32,
    pub branch_density: f32,
    pub branch_angle: f32,
    pub leaf_size: f32,
    pub leaf_density: f32,
    pub canopy_spread: f32,
    pub root_spread: f32,
    pub fruitfulness: f32,
    pub growth_rate: f32,
    pub hardiness: f32,
    /// 0 = herb, 1 = tree.
    pub woodiness: f32,
    pub season_bias: f32,
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
#[inline]
fn unit(state: &mut u64) -> f32 {
    (splitmix64(state) >> 40) as f32 / (1u64 << 24) as f32 // [0,1)
}

impl PlantGenome {
    /// Deterministic genome from a seed; all genes in [0,1].
    pub fn from_seed(seed: u64) -> Self {
        let mut s = seed ^ 0x504C_414E_5400_0001;
        PlantGenome {
            max_height: unit(&mut s),
            trunk_girth: unit(&mut s),
            branch_density: unit(&mut s),
            branch_angle: unit(&mut s),
            leaf_size: unit(&mut s),
            leaf_density: unit(&mut s),
            canopy_spread: unit(&mut s),
            root_spread: unit(&mut s),
            fruitfulness: unit(&mut s),
            growth_rate: unit(&mut s),
            hardiness: unit(&mut s),
            woodiness: unit(&mut s),
            season_bias: unit(&mut s),
        }
    }

    pub fn neutral() -> Self {
        Self::from_array([0.5; PLANT_GENE_COUNT])
    }

    pub fn as_array(&self) -> [f32; PLANT_GENE_COUNT] {
        [
            self.max_height, self.trunk_girth, self.branch_density, self.branch_angle,
            self.leaf_size, self.leaf_density, self.canopy_spread, self.root_spread,
            self.fruitfulness, self.growth_rate, self.hardiness, self.woodiness, self.season_bias,
        ]
    }

    pub fn from_array(a: [f32; PLANT_GENE_COUNT]) -> Self {
        let c = |v: f32| v.clamp(0.0, 1.0);
        PlantGenome {
            max_height: c(a[0]), trunk_girth: c(a[1]), branch_density: c(a[2]), branch_angle: c(a[3]),
            leaf_size: c(a[4]), leaf_density: c(a[5]), canopy_spread: c(a[6]), root_spread: c(a[7]),
            fruitfulness: c(a[8]), growth_rate: c(a[9]), hardiness: c(a[10]), woodiness: c(a[11]),
            season_bias: c(a[12]),
        }
    }
}

/// Mendelian-ish cross: per gene pick one parent (seeded coin) and blend slightly
/// toward the other, plus a small seeded mutation. Deterministic.
pub fn cross(a: &PlantGenome, b: &PlantGenome, seed: u64) -> PlantGenome {
    let aa = a.as_array();
    let bb = b.as_array();
    let mut s = seed ^ 0xC305_5_u64.wrapping_mul(0x9E37_79B9);
    let mut out = [0.0f32; PLANT_GENE_COUNT];
    for i in 0..PLANT_GENE_COUNT {
        let coin = unit(&mut s);
        let (dom, rec) = if coin < 0.5 { (aa[i], bb[i]) } else { (bb[i], aa[i]) };
        let blended = dom * 0.75 + rec * 0.25; // dominant with a touch of the other
        let mutation = (unit(&mut s) - 0.5) * 0.06; // ±0.03 jitter
        out[i] = (blended + mutation).clamp(0.0, 1.0);
    }
    PlantGenome::from_array(out)
}

/// What the growth/harvest cores consume so a genome actually changes the plant.
#[derive(Clone, Copy, Debug)]
pub struct PlantPhenotype {
    pub mature_height: f32,  // metres
    pub growth_per_tick: f32,
    pub is_tree: bool,
    pub fruit_yield: f32,
    pub canopy_radius: f32,  // metres
}

/// Map a genome to real-unit phenotype ranges.
pub fn phenotype(g: &PlantGenome) -> PlantPhenotype {
    let tree = g.woodiness > 0.5;
    // trees are taller and broader than herbs
    let max_h = if tree { 2.0 + g.max_height * 18.0 } else { 0.2 + g.max_height * 1.8 };
    PlantPhenotype {
        mature_height: max_h,
        growth_per_tick: 0.01 + g.growth_rate * 0.20,
        is_tree: tree,
        fruit_yield: g.fruitfulness * g.leaf_density,
        canopy_radius: (if tree { 0.5 + g.canopy_spread * 6.0 } else { 0.1 + g.canopy_spread * 1.0 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_seed_deterministic_and_in_range() {
        let a = PlantGenome::from_seed(123);
        let b = PlantGenome::from_seed(123);
        assert_eq!(a, b);
        for v in a.as_array() {
            assert!((0.0..=1.0).contains(&v), "gene out of range: {v}");
        }
        assert_ne!(PlantGenome::from_seed(1), PlantGenome::from_seed(2));
    }

    #[test]
    fn neutral_is_half() {
        for v in PlantGenome::neutral().as_array() {
            assert!((v - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn array_round_trips() {
        let g = PlantGenome::from_seed(77);
        assert_eq!(PlantGenome::from_array(g.as_array()), g);
    }

    #[test]
    fn cross_finite_in_range_and_near_identical_parents() {
        let p = PlantGenome::from_seed(9);
        let c = cross(&p, &p, 5);
        for (cv, pv) in c.as_array().iter().zip(p.as_array().iter()) {
            assert!((0.0..=1.0).contains(cv));
            assert!((cv - pv).abs() <= 0.04, "identical parents ~ parent (got {cv} vs {pv})");
        }
        // a cross of two different parents stays in range
        let q = PlantGenome::from_seed(31);
        for v in cross(&p, &q, 12).as_array() {
            assert!((0.0..=1.0).contains(&v) && v.is_finite());
        }
    }

    #[test]
    fn woodiness_decides_tree() {
        let mut herb = PlantGenome::neutral(); herb.woodiness = 0.2;
        let mut tree = PlantGenome::neutral(); tree.woodiness = 0.9;
        assert!(!phenotype(&herb).is_tree);
        assert!(phenotype(&tree).is_tree);
        assert!(phenotype(&tree).mature_height > phenotype(&herb).mature_height);
    }

    #[test]
    fn phenotype_monotone() {
        let lo = PlantGenome::from_array([0.0; PLANT_GENE_COUNT]);
        let mut hi = lo;
        hi.max_height = 1.0; hi.woodiness = lo.woodiness; // same tree class
        assert!(phenotype(&hi).mature_height > phenotype(&lo).mature_height);
        let mut fr = lo; fr.fruitfulness = 1.0; fr.leaf_density = 1.0;
        assert!(phenotype(&fr).fruit_yield > phenotype(&lo).fruit_yield);
    }

    #[test]
    fn deterministic_cross() {
        let a = PlantGenome::from_seed(2);
        let b = PlantGenome::from_seed(3);
        assert_eq!(cross(&a, &b, 99), cross(&a, &b, 99));
    }
}
