//! genome.rs — a small trait vector that gives each bot a distinct, heritable "personality": how far it
//! presses an engagement, how readily it breaks off wounded, how hard it flanks, how fast it jukes, how
//! sharp its aim is, and how long it keeps hunting a lost target. `Enemy::update` reads these to bend its
//! otherwise-fixed decision thresholds — the state machine's *shape* doesn't change, but *when* it fires
//! does, per bot. `evolve.rs` breeds populations of these against a headless fitness function; the game
//! can also spawn from the hand-tuned `ARCHETYPES` below as quirky presets when no evolved pool is loaded.

use crate::level::Rng;

/// Six heritable traits, each in [0,1]. All default to 0.5 — the midpoint reproduces the original,
/// hand-tuned enemy behaviour, so existing callers that never set a genome see no behavioural change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Genome {
    /// How close it wants to fight and how readily it presses on wounded instead of breaking off.
    pub aggression: f32,
    /// How far away it notices the target — a wary bot spots you sooner.
    pub awareness: f32,
    /// How hard it swings wide on approach instead of walking straight in.
    pub flank_bias: f32,
    /// How fast it flips its strafe direction — a jumpy bot is a harder target but a wilder shot.
    pub reaction: f32,
    /// Aim sharpness: higher tightens shot spread.
    pub accuracy: f32,
    /// How long it keeps hunting a lost target before giving up.
    pub persistence: f32,
}

impl Default for Genome {
    fn default() -> Self {
        Self { aggression: 0.5, awareness: 0.5, flank_bias: 0.5, reaction: 0.5, accuracy: 0.5, persistence: 0.5 }
    }
}

impl Genome {
    pub const N: usize = 6;

    pub fn random(rng: &mut Rng) -> Self {
        Self {
            aggression: rng.f32(),
            awareness: rng.f32(),
            flank_bias: rng.f32(),
            reaction: rng.f32(),
            accuracy: rng.f32(),
            persistence: rng.f32(),
        }
    }

    pub fn as_array(&self) -> [f32; Self::N] {
        [self.aggression, self.awareness, self.flank_bias, self.reaction, self.accuracy, self.persistence]
    }
    pub fn from_array(a: [f32; Self::N]) -> Self {
        Self { aggression: a[0], awareness: a[1], flank_bias: a[2], reaction: a[3], accuracy: a[4], persistence: a[5] }
    }

    /// Uniform crossover: each gene independently comes from `a` or `b` — the standard GA recombination
    /// that lets two good-but-different parents pass down whichever traits made each of them work.
    pub fn crossover(a: &Genome, b: &Genome, rng: &mut Rng) -> Genome {
        let (ga, gb) = (a.as_array(), b.as_array());
        let mut out = [0.0f32; Self::N];
        for i in 0..Self::N {
            out[i] = if rng.f32() < 0.5 { ga[i] } else { gb[i] };
        }
        Genome::from_array(out)
    }

    /// Each gene independently has `rate` chance to nudge by up to ±`amount`, clamped back into [0,1].
    /// Mutation is what keeps a converged population from going stale — it's the source of new quirks a
    /// crossover of existing parents could never produce on its own.
    pub fn mutate(&mut self, rate: f32, amount: f32, rng: &mut Rng) {
        let mut a = self.as_array();
        for g in a.iter_mut() {
            if rng.f32() < rate {
                *g = (*g + (rng.f32() * 2.0 - 1.0) * amount).clamp(0.0, 1.0);
            }
        }
        *self = Genome::from_array(a);
    }

    /// Euclidean distance between two genomes in trait-space — used to measure population diversity so an
    /// evolution run can report whether it's still exploring or has collapsed onto one dominant strategy.
    pub fn distance(a: &Genome, b: &Genome) -> f32 {
        let (ga, gb) = (a.as_array(), b.as_array());
        ga.iter().zip(gb.iter()).map(|(x, y)| (x - y).powi(2)).sum::<f32>().sqrt()
    }

    /// A short, human-readable read of the dominant trait — used for feed/HUD flavour ("BERSERKER
    /// down!") so an evolved population still feels like it has *characters*, not just numbers.
    pub fn nickname(&self) -> &'static str {
        let a = self.as_array();
        let names = ["Reckless", "Hawk-Eyed", "Flanking", "Twitchy", "Sharpshooting", "Relentless"];
        let mut best = 0;
        let mut bestv = a[0];
        for (i, &v) in a.iter().enumerate().skip(1) {
            if v > bestv {
                bestv = v;
                best = i;
            }
        }
        if bestv < 0.62 {
            "Balanced"
        } else {
            names[best]
        }
    }

    /// A handful of named, hand-tuned "quirky" archetypes — distinct fighting styles at a glance. Used as
    /// fallback personalities when no evolved pool is loaded yet, and as seed diversity for a fresh GA
    /// population (a pure-random start tends to hover near 0.5 on everything; salting in a few extremes
    /// gives selection something sharp to work with from generation one).
    pub const ARCHETYPES: [(&'static str, Genome); 6] = [
        ("Berserker", Genome { aggression: 0.95, awareness: 0.6, flank_bias: 0.15, reaction: 0.7, accuracy: 0.4, persistence: 0.85 }),
        ("Sniper", Genome { aggression: 0.2, awareness: 0.9, flank_bias: 0.3, reaction: 0.3, accuracy: 0.95, persistence: 0.5 }),
        ("Flanker", Genome { aggression: 0.6, awareness: 0.55, flank_bias: 0.95, reaction: 0.6, accuracy: 0.55, persistence: 0.6 }),
        ("Skirmisher", Genome { aggression: 0.45, awareness: 0.65, flank_bias: 0.5, reaction: 0.9, accuracy: 0.6, persistence: 0.4 }),
        ("Turtle", Genome { aggression: 0.15, awareness: 0.7, flank_bias: 0.2, reaction: 0.35, accuracy: 0.7, persistence: 0.9 }),
        ("Wildcard", Genome { aggression: 0.7, awareness: 0.4, flank_bias: 0.6, reaction: 0.8, accuracy: 0.3, persistence: 0.3 }),
    ];

    /// Pick an archetype by index (wrapping) — a stable, dependency-free way for the game to hand out
    /// varied starting personalities before any evolved pool exists.
    pub fn archetype(i: usize) -> Genome {
        Self::ARCHETYPES[i % Self::ARCHETYPES.len()].1
    }

    /// Serialize to one comma-separated line, in `as_array()` order. Hand-rolled rather than pulling in a
    /// serialization crate — a genome is six floats, and a whole bred population is a handful of these
    /// lines, so a real format would be pure overhead.
    pub fn to_line(&self) -> String {
        let a = self.as_array();
        format!("{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}", a[0], a[1], a[2], a[3], a[4], a[5])
    }

    /// Parse one line written by `to_line`. Tolerates surrounding whitespace; rejects malformed lines
    /// (wrong field count, unparsable floats) by returning `None` rather than guessing.
    pub fn from_line(s: &str) -> Option<Genome> {
        let parts: Vec<&str> = s.trim().split(',').collect();
        if parts.len() != Self::N {
            return None;
        }
        let mut a = [0.0f32; Self::N];
        for (i, p) in parts.iter().enumerate() {
            a[i] = p.trim().parse::<f32>().ok()?.clamp(0.0, 1.0);
        }
        Some(Genome::from_array(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_genome_is_neutral_midpoint() {
        let g = Genome::default();
        for v in g.as_array() {
            assert!((v - 0.5).abs() < 1e-6, "default genes should sit at the neutral midpoint");
        }
    }

    #[test]
    fn crossover_only_recombines_existing_alleles() {
        let mut rng = Rng::new(1);
        let a = Genome { aggression: 0.1, awareness: 0.1, flank_bias: 0.1, reaction: 0.1, accuracy: 0.1, persistence: 0.1 };
        let b = Genome { aggression: 0.9, awareness: 0.9, flank_bias: 0.9, reaction: 0.9, accuracy: 0.9, persistence: 0.9 };
        for _ in 0..50 {
            let c = Genome::crossover(&a, &b, &mut rng);
            for v in c.as_array() {
                assert!(v == 0.1 || v == 0.9, "crossover child should only carry parent alleles, got {v}");
            }
        }
    }

    #[test]
    fn mutation_stays_in_bounds_and_can_change_genes() {
        let mut rng = Rng::new(7);
        let mut g = Genome::default();
        let mut changed = false;
        for _ in 0..40 {
            let before = g;
            g.mutate(0.5, 0.3, &mut rng);
            for v in g.as_array() {
                assert!((0.0..=1.0).contains(&v), "mutated gene left [0,1]: {v}");
            }
            if g != before {
                changed = true;
            }
        }
        assert!(changed, "mutation should eventually change something over 40 tries at rate 0.5");
    }

    #[test]
    fn archetypes_are_distinct() {
        let arche = Genome::ARCHETYPES;
        for i in 0..arche.len() {
            for j in (i + 1)..arche.len() {
                assert!(Genome::distance(&arche[i].1, &arche[j].1) > 0.15, "{} and {} read too similar", arche[i].0, arche[j].0);
            }
        }
    }

    #[test]
    fn line_round_trips_through_serialization() {
        let mut rng = Rng::new(3);
        for _ in 0..20 {
            let g = Genome::random(&mut rng);
            let line = g.to_line();
            let back = Genome::from_line(&line).expect("a line we just wrote should parse back");
            for (a, b) in g.as_array().iter().zip(back.as_array().iter()) {
                assert!((a - b).abs() < 1e-3, "round-trip drifted: {a} vs {b}");
            }
        }
    }

    #[test]
    fn malformed_lines_are_rejected_not_guessed() {
        assert!(Genome::from_line("").is_none());
        assert!(Genome::from_line("0.1,0.2,0.3").is_none()); // too few fields
        assert!(Genome::from_line("a,b,c,d,e,f").is_none()); // not floats
        assert!(Genome::from_line("0.1,0.2,0.3,0.4,0.5,0.6,0.7").is_none()); // too many fields
    }
}
