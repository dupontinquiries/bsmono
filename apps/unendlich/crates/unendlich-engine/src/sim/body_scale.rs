//! body_scale — how a creature's **size** changes the way it senses and paths.
//!
//! A dragon shouldn't path like a rabbit. A big body sees farther, spans wider,
//! steps over bigger things, and — crucially — reasons about terrain in **coarser
//! tiles**: a 2×2 or 3×3 patch of blocks collapses to one cell "in its brain", so
//! it ignores pebbles and 1-block cracks its body simply bridges. This bundles
//! those size effects into one profile the bin reads when building a creature's
//! ground probe and perception. Pure; pairs with
//! [`crate::sim::collision_probe::coarsened`].

/// A creature's size relative to the baseline (a deer-ish `1.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyScale {
    pub size: f32,
}

impl Default for BodyScale {
    fn default() -> BodyScale {
        BodyScale { size: 1.0 }
    }
}

impl BodyScale {
    pub fn new(size: f32) -> BodyScale {
        BodyScale { size: size.max(0.1) }
    }

    /// A dragon: large, sees far, paths coarse.
    pub fn dragon() -> BodyScale {
        BodyScale { size: 3.0 }
    }

    /// Sight range scales with size but with diminishing return (√size), so a
    /// dragon sees noticeably farther than a deer without being absurd: size 1 →
    /// base, size 3 → ~1.7× base.
    pub fn sight_range(&self, base: f32) -> f32 {
        base * self.size.sqrt()
    }

    /// Probe reach to each side, in blocks — a wide body samples wider.
    pub fn probe_span(&self, base: f32) -> f32 {
        base * self.size
    }

    /// The terrain tile size this creature reasons in (blocks per cell): 1 for
    /// baseline, 2–3 for large bodies. Feeds [`coarsened`](crate::sim::collision_probe::coarsened).
    pub fn path_tile(&self) -> i32 {
        self.size.round().clamp(1.0, 4.0) as i32
    }

    /// How tall a step it walks over without needing to climb/leap (blocks).
    pub fn step_up(&self, base_blocks: i32) -> i32 {
        ((base_blocks as f32) * self.size).round() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dragon_sees_farther_than_a_deer() {
        let deer = BodyScale::default();
        let dragon = BodyScale::dragon();
        assert!(dragon.sight_range(20.0) > deer.sight_range(20.0));
        assert!(deer.sight_range(20.0) >= 20.0 * 0.99, "baseline keeps ~its base range");
    }

    #[test]
    fn a_dragon_paths_in_coarser_tiles() {
        assert_eq!(BodyScale::default().path_tile(), 1);
        assert_eq!(BodyScale::dragon().path_tile(), 3);
        assert!(BodyScale::dragon().path_tile() > BodyScale::default().path_tile());
    }

    #[test]
    fn a_dragon_spans_wider_and_steps_higher() {
        let dragon = BodyScale::dragon();
        assert!(dragon.probe_span(1.0) > BodyScale::default().probe_span(1.0));
        assert!(dragon.step_up(1) >= 3, "a dragon walks over a few blocks");
        assert_eq!(BodyScale::default().step_up(1), 1);
    }

    #[test]
    fn tiny_bodies_clamp_to_one_tile() {
        assert_eq!(BodyScale::new(0.2).path_tile(), 1);
    }
}
