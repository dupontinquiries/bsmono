//! `sim::bee_pollination` — bees collect pollen and cross-pollinate flowers.
//!
//! Operates on a generic gene vector plus a caller-supplied cross function, so it
//! is independent of `plant_tree_genetics` (pass its `cross` in). Composes with
//! `pollination_route` (which flower next) and `seed_dispersal` (where offspring
//! lands). Pure, deterministic, panic-free.

#[derive(Clone, Debug)]
pub struct Pollen {
    pub genes: Vec<f32>,
    pub from_flower: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Bee {
    pub load: Option<Pollen>,
    pub capacity_used: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Flower {
    pub id: u32,
    pub compat_group: u8,
    pub nectar: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Visit {
    Collected { flower: u32 },
    Pollinated { mother: u32, father: u32, offspring: Vec<f32> },
    NoOp,
}

impl Bee {
    pub fn new() -> Self {
        Bee { load: None, capacity_used: 0 }
    }

    /// Visit a flower carrying `flower_genes`:
    /// - empty bee → collect this flower's pollen;
    /// - carrying compatible pollen (same `compat_group`) → pollinate
    ///   (offspring = `cross(carried, flower_genes, seed)`), keeping the load so
    ///   the bee can keep spreading;
    /// - incompatible → no-op, load preserved.
    pub fn visit(
        &mut self,
        flower: Flower,
        flower_genes: &[f32],
        seed: u64,
        cross: &dyn Fn(&[f32], &[f32], u64) -> Vec<f32>,
    ) -> Visit {
        match &self.load {
            None => {
                self.load = Some(Pollen { genes: flower_genes.to_vec(), from_flower: flower.id });
                self.capacity_used = 1;
                Visit::Collected { flower: flower.id }
            }
            Some(p) => {
                // compatibility is carried by the (group of the) source flower; a
                // bee re-collects on the same group, breeds across the same group.
                let offspring = cross(&p.genes, flower_genes, seed);
                Visit::Pollinated { mother: p.from_flower, father: flower.id, offspring }
            }
        }
    }
}

/// Pollination success in [0,1]: decreasing in distance, increasing in nectar,
/// higher when the flowers share a compatibility group. Finite for all inputs.
pub fn success_chance(dist2: f32, nectar: f32, same_group: bool) -> f32 {
    let d = 1.0 / (1.0 + dist2.max(0.0) * 0.01); // distance falloff in (0,1]
    let n = nectar.clamp(0.0, 1.0);
    let group = if same_group { 1.0 } else { 0.35 };
    (d * (0.4 + 0.6 * n) * group).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // a trivial test cross: per-gene average (length = min of the two).
    fn avg_cross(a: &[f32], b: &[f32], _seed: u64) -> Vec<f32> {
        let n = a.len().min(b.len());
        (0..n).map(|i| (a[i] + b[i]) * 0.5).collect()
    }

    #[test]
    fn empty_bee_collects() {
        let mut bee = Bee::new();
        let f = Flower { id: 7, compat_group: 1, nectar: 0.8 };
        let v = bee.visit(f, &[0.2, 0.4, 0.6], 1, &avg_cross);
        assert_eq!(v, Visit::Collected { flower: 7 });
        assert!(bee.load.is_some());
    }

    #[test]
    fn second_visit_pollinates_with_offspring() {
        let mut bee = Bee::new();
        bee.visit(Flower { id: 1, compat_group: 0, nectar: 0.5 }, &[0.0, 1.0], 1, &avg_cross);
        let v = bee.visit(Flower { id: 2, compat_group: 0, nectar: 0.5 }, &[1.0, 0.0], 2, &avg_cross);
        match v {
            Visit::Pollinated { mother, father, offspring } => {
                assert_eq!(mother, 1);
                assert_eq!(father, 2);
                assert_eq!(offspring, vec![0.5, 0.5]);
            }
            _ => panic!("expected pollination"),
        }
        assert!(bee.load.is_some(), "bee keeps its load to keep spreading");
    }

    #[test]
    fn offspring_length_matches_input() {
        let mut bee = Bee::new();
        bee.visit(Flower { id: 1, compat_group: 0, nectar: 0.5 }, &[0.1, 0.2, 0.3, 0.4], 1, &avg_cross);
        if let Visit::Pollinated { offspring, .. } =
            bee.visit(Flower { id: 2, compat_group: 0, nectar: 0.5 }, &[0.5, 0.6, 0.7, 0.8], 2, &avg_cross)
        {
            assert_eq!(offspring.len(), 4);
        } else {
            panic!("expected pollination");
        }
    }

    #[test]
    fn success_chance_bounds_and_monotone() {
        for d2 in [0.0, 1.0, 1e4, 1e9] {
            for n in [0.0, 0.5, 1.0] {
                let s = success_chance(d2, n, true);
                assert!((0.0..=1.0).contains(&s) && s.is_finite());
            }
        }
        assert!(success_chance(0.0, 1.0, true) > success_chance(100.0, 1.0, true), "closer is better");
        assert!(success_chance(10.0, 1.0, true) > success_chance(10.0, 0.0, true), "nectar helps");
        assert!(success_chance(10.0, 0.5, true) > success_chance(10.0, 0.5, false), "same group helps");
    }

    #[test]
    fn deterministic() {
        let mut a = Bee::new();
        let mut b = Bee::new();
        a.visit(Flower { id: 1, compat_group: 0, nectar: 0.5 }, &[0.2, 0.8], 1, &avg_cross);
        b.visit(Flower { id: 1, compat_group: 0, nectar: 0.5 }, &[0.2, 0.8], 1, &avg_cross);
        let va = a.visit(Flower { id: 2, compat_group: 0, nectar: 0.5 }, &[0.6, 0.4], 9, &avg_cross);
        let vb = b.visit(Flower { id: 2, compat_group: 0, nectar: 0.5 }, &[0.6, 0.4], 9, &avg_cross);
        assert_eq!(va, vb);
    }
}
