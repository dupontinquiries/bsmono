//! Item genetics + the behaviour they express — the gene half of a sentient
//! item's brain.
//!
//! A live item (the hero's flying blade, a summoned guardian) is steered by six
//! heritable traits in `[0,1]` plus a categorical *temper*, the same way a
//! creature is steered by its [`crate::sim::personality::Disposition`]. We reuse
//! the deterministic fixed-point gene model of [`crate::sim::genetics`] —
//! crossover + small mutation, pure in a `seed` — so the forge can *breed* items
//! the way the world breeds mobs, and the whole thing snapshots / rolls back with
//! the sim.
//!
//! Two layers:
//! - [`ItemGenes`] — the genome (`acuity, coverage, aggression, loyalty, agility,
//!   verve` + [`Temper`]), with the forge stations [`breed`] / [`blend`] /
//!   [`reroll`], material-biased rolls, and gem sockets.
//! - [`BrainParams`] — what a genome *expresses* in engine units (blocks, ticks,
//!   fractions). [`BrainParams::from_genes`] is the mapping validated on the 2D
//!   feel-bench, rescaled from the bench's pixels to blocks (≈ 1 block / 10 px)
//!   and frames to ticks; each endpoint is documented inline.

use crate::sim::fixed::Fixed;

// ── deterministic gene helpers (same model as `sim::genetics`) ─────────────────

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn clamp01(x: Fixed) -> Fixed {
    x.clamp(Fixed::ZERO, Fixed::ONE)
}

fn lerp(a: Fixed, b: Fixed, t: Fixed) -> Fixed {
    a + (b - a) * t
}

/// A uniform gene value in `[0,1]` (thousandths) drawn from the stream.
fn rand01(state: &mut u64) -> Fixed {
    Fixed::from_frac((splitmix(state) % 1001) as i64, 1000)
}

/// Largest mutation nudge applied to an inherited gene (±0.10), matching
/// `sim::genetics`.
const MUT_MAX_PCT: i64 = 10;

/// Inherit one gene: cross (pick a parent 50/50) then mutate by ±`MUT_MAX_PCT`.
fn inherit(state: &mut u64, a: Fixed, b: Fixed) -> Fixed {
    let pick = if splitmix(state) & 1 == 0 { a } else { b };
    let m = (splitmix(state) % (2 * MUT_MAX_PCT as u64 + 1)) as i64 - MUT_MAX_PCT;
    clamp01(pick + Fixed::from_frac(m, 100))
}

/// Average two genes then mutate by ±`MUT_MAX_PCT`.
fn blend_gene(state: &mut u64, a: Fixed, b: Fixed) -> Fixed {
    let avg = (a + b) * Fixed::from_frac(1, 2);
    let m = (splitmix(state) % (2 * MUT_MAX_PCT as u64 + 1)) as i64 - MUT_MAX_PCT;
    clamp01(avg + Fixed::from_frac(m, 100))
}

// ── temper ──────────────────────────────────────────────────────────────────--

/// How a live item picks among the things it can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Temper {
    /// Strike the closest seen target.
    #[default]
    Nearest,
    /// Strike the lowest-health seen target (finisher).
    Weakest,
    /// Strike the strongest seen target (duelist).
    Boldest,
}

impl Temper {
    fn from_seed(state: &mut u64) -> Temper {
        match splitmix(state) % 3 {
            0 => Temper::Nearest,
            1 => Temper::Weakest,
            _ => Temper::Boldest,
        }
    }
}

// ── genome ──────────────────────────────────────────────────────────────────--

/// Six heritable traits in `[0,1]` plus a categorical [`Temper`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemGenes {
    /// Perception range / sharpness.
    pub acuity: Fixed,
    /// Field-of-view width.
    pub coverage: Fixed,
    /// Pull toward targets + how far it will wander to reach one.
    pub aggression: Fixed,
    /// Tether to its owner + how early it breaks off to recharge.
    pub loyalty: Fixed,
    /// Turn rate + flight speed.
    pub agility: Fixed,
    /// Charge economy: cheaper strikes, shorter cooldowns.
    pub verve: Fixed,
    /// Target-selection rule.
    pub temper: Temper,
}

impl ItemGenes {
    /// A neutral genome — every trait at 0.5, nearest-target temper.
    pub fn balanced() -> ItemGenes {
        let h = Fixed::from_frac(1, 2);
        ItemGenes { acuity: h, coverage: h, aggression: h, loyalty: h, agility: h, verve: h, temper: Temper::Nearest }
    }

    /// Build from raw trait values, each clamped to `[0,1]`.
    pub fn new(
        acuity: Fixed,
        coverage: Fixed,
        aggression: Fixed,
        loyalty: Fixed,
        agility: Fixed,
        verve: Fixed,
        temper: Temper,
    ) -> ItemGenes {
        ItemGenes {
            acuity: clamp01(acuity),
            coverage: clamp01(coverage),
            aggression: clamp01(aggression),
            loyalty: clamp01(loyalty),
            agility: clamp01(agility),
            verve: clamp01(verve),
            temper,
        }
    }

    /// Socket a gem: it lifts the trait it governs by `g = min(1, g*0.5 + 0.55)`
    /// (the bench's boost rule — a weak trait jumps, a strong one barely moves).
    pub fn socket(&mut self, gem: Gem) {
        let boost = |g: Fixed| clamp01(g * Fixed::from_frac(1, 2) + Fixed::from_frac(55, 100));
        match gem {
            Gem::Ruby => self.aggression = boost(self.aggression),
            Gem::Sapphire => self.acuity = boost(self.acuity),
            Gem::Emerald => self.loyalty = boost(self.loyalty),
            Gem::Topaz => self.agility = boost(self.agility),
            Gem::Amethyst => self.coverage = boost(self.coverage),
            Gem::Diamond => self.verve = boost(self.verve),
        }
    }
}

// ── forge inputs: gems & materials ─────────────────────────────────────────────

/// A socketable gem — each lifts one trait (see [`ItemGenes::socket`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gem {
    Ruby,
    Sapphire,
    Emerald,
    Topaz,
    Amethyst,
    Diamond,
}

/// The stock a blade is forged from — biases a fresh roll toward its character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// Neutral, no bias.
    Iron,
    /// The watcher: wide sight, loyal.
    Silver,
    /// The glass cannon: sharp, fast, aggressive.
    Meteoric,
    /// The berserker: aggressive, high verve.
    Bone,
    /// The savant: high verve, sharp.
    Oberine,
}

impl Material {
    /// Nudge a freshly-rolled genome toward the material's character (additive,
    /// clamped). Applied by [`roll_material`].
    fn bias(self, g: &mut ItemGenes) {
        let up = |x: &mut Fixed, by: i64| *x = clamp01(*x + Fixed::from_frac(by, 100));
        match self {
            Material::Iron => {}
            Material::Silver => {
                up(&mut g.coverage, 20);
                up(&mut g.loyalty, 20);
            }
            Material::Meteoric => {
                up(&mut g.acuity, 18);
                up(&mut g.agility, 18);
                up(&mut g.aggression, 18);
            }
            Material::Bone => {
                up(&mut g.aggression, 20);
                up(&mut g.verve, 18);
            }
            Material::Oberine => {
                up(&mut g.verve, 22);
                up(&mut g.acuity, 16);
            }
        }
    }
}

// ── forge stations: breed / blend / reroll ────────────────────────────────────-

/// Crossover two genomes gene by gene with mutation — the forge's *merge* slot
/// (pour two blades into one). Deterministic in `seed`.
pub fn breed(a: &ItemGenes, b: &ItemGenes, seed: u64) -> ItemGenes {
    let mut s = seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(0x9E37);
    ItemGenes {
        acuity: inherit(&mut s, a.acuity, b.acuity),
        coverage: inherit(&mut s, a.coverage, b.coverage),
        aggression: inherit(&mut s, a.aggression, b.aggression),
        loyalty: inherit(&mut s, a.loyalty, b.loyalty),
        agility: inherit(&mut s, a.agility, b.agility),
        verve: inherit(&mut s, a.verve, b.verve),
        temper: if splitmix(&mut s) & 1 == 0 { a.temper } else { b.temper },
    }
}

/// Average two genomes with mutation — the *anvil* (children converge on the
/// mean instead of inheriting whole traits). Deterministic in `seed`.
pub fn blend(a: &ItemGenes, b: &ItemGenes, seed: u64) -> ItemGenes {
    let mut s = seed.wrapping_mul(0xD1B5_4A32_D192_ED03).wrapping_add(0x1234);
    ItemGenes {
        acuity: blend_gene(&mut s, a.acuity, b.acuity),
        coverage: blend_gene(&mut s, a.coverage, b.coverage),
        aggression: blend_gene(&mut s, a.aggression, b.aggression),
        loyalty: blend_gene(&mut s, a.loyalty, b.loyalty),
        agility: blend_gene(&mut s, a.agility, b.agility),
        verve: blend_gene(&mut s, a.verve, b.verve),
        temper: if splitmix(&mut s) & 1 == 0 { a.temper } else { b.temper },
    }
}

/// A fresh random genome — the *reroll* station. Deterministic in `seed`.
pub fn reroll(seed: u64) -> ItemGenes {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xABCD);
    ItemGenes {
        acuity: rand01(&mut s),
        coverage: rand01(&mut s),
        aggression: rand01(&mut s),
        loyalty: rand01(&mut s),
        agility: rand01(&mut s),
        verve: rand01(&mut s),
        temper: Temper::from_seed(&mut s),
    }
}

/// Reroll then bias toward a material's character — a blade forged from stock.
pub fn roll_material(material: Material, seed: u64) -> ItemGenes {
    let mut g = reroll(seed);
    material.bias(&mut g);
    g
}

// ── expressed behaviour ────────────────────────────────────────────────────────

/// What a genome expresses, in engine units. Consumed by the item brain to steer,
/// perceive, and budget charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrainParams {
    /// Perception radius, blocks.
    pub range: Fixed,
    /// Vision half-cone, radians (used once perception moves into the brain;
    /// today perception is fed by the caller via `sim::vision`).
    pub half_angle: Fixed,
    /// Steering gain toward a target.
    pub pull: Fixed,
    /// Max wander from the owner while hunting, blocks.
    pub stray: Fixed,
    /// Homing gain toward the owner.
    pub tether: Fixed,
    /// Charge fraction below which it breaks off to recharge.
    pub recall_at: Fixed,
    /// Max heading change per tick, radians.
    pub turn: Fixed,
    /// Max move per tick, blocks.
    pub speed: Fixed,
    /// Multiplier on the element's per-cast charge cost (verve cheapens).
    pub strike_cost_mul: Fixed,
    /// Ticks between strikes.
    pub cooldown: u16,
}

impl BrainParams {
    /// The bench mapping, rescaled to engine units. Endpoints are the values the
    /// 2D feel-bench was tuned to; px→blocks at ≈ 1 block / 10 px.
    pub fn from_genes(g: &ItemGenes) -> BrainParams {
        BrainParams {
            // 70..230 px  →  7..23 blocks
            range: lerp(Fixed::from_int(7), Fixed::from_int(23), g.acuity),
            // 0.42..PI rad (verbatim); PI ≈ 355/113
            half_angle: lerp(Fixed::from_frac(42, 100), Fixed::from_frac(355, 113), g.coverage),
            // 0.5 + aggression*1.6  →  0.5..2.1
            pull: lerp(Fixed::from_frac(1, 2), Fixed::from_frac(21, 10), g.aggression),
            // 120..360 px  →  12..36 blocks
            stray: lerp(Fixed::from_int(12), Fixed::from_int(36), g.aggression),
            // 0.4 + loyalty*1.8  →  0.4..2.2
            tether: lerp(Fixed::from_frac(2, 5), Fixed::from_frac(11, 5), g.loyalty),
            // 0.12 + loyalty*0.22  →  0.12..0.34 (verbatim)
            recall_at: lerp(Fixed::from_frac(12, 100), Fixed::from_frac(34, 100), g.loyalty),
            // 0.045..0.24 rad/tick (verbatim)
            turn: lerp(Fixed::from_frac(45, 1000), Fixed::from_frac(24, 100), g.agility),
            // 2.4..5.6 px/frame  →  0.24..0.56 blocks/tick
            speed: lerp(Fixed::from_frac(24, 100), Fixed::from_frac(56, 100), g.agility),
            // verve cheapens strikes: 1.0..0.5×
            strike_cost_mul: lerp(Fixed::ONE, Fixed::from_frac(1, 2), g.verve),
            // verve quickens: 34..12 ticks
            cooldown: lerp(Fixed::from_int(34), Fixed::from_int(12), g.verve).floor_int().max(1) as u16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Fixed, b: f64) -> bool {
        (a.to_f64() - b).abs() < 0.05
    }

    #[test]
    fn params_hit_their_endpoints() {
        let mut g = ItemGenes::balanced();
        g.acuity = Fixed::ZERO;
        assert!(approx(BrainParams::from_genes(&g).range, 7.0));
        g.acuity = Fixed::ONE;
        assert!(approx(BrainParams::from_genes(&g).range, 23.0));
    }

    #[test]
    fn agility_drives_speed_and_turn_monotonically() {
        let mut lo = ItemGenes::balanced();
        lo.agility = Fixed::ZERO;
        let mut hi = ItemGenes::balanced();
        hi.agility = Fixed::ONE;
        assert!(BrainParams::from_genes(&lo).speed < BrainParams::from_genes(&hi).speed);
        assert!(BrainParams::from_genes(&lo).turn < BrainParams::from_genes(&hi).turn);
    }

    #[test]
    fn verve_cheapens_and_quickens() {
        let mut lo = ItemGenes::balanced();
        lo.verve = Fixed::ZERO;
        let mut hi = ItemGenes::balanced();
        hi.verve = Fixed::ONE;
        assert!(BrainParams::from_genes(&hi).strike_cost_mul < BrainParams::from_genes(&lo).strike_cost_mul);
        assert!(BrainParams::from_genes(&hi).cooldown < BrainParams::from_genes(&lo).cooldown);
    }

    #[test]
    fn loyalty_raises_the_recall_threshold() {
        let mut lo = ItemGenes::balanced();
        lo.loyalty = Fixed::ZERO;
        let mut hi = ItemGenes::balanced();
        hi.loyalty = Fixed::ONE;
        assert!(BrainParams::from_genes(&hi).recall_at > BrainParams::from_genes(&lo).recall_at);
    }

    #[test]
    fn gem_lifts_its_trait_and_caps_at_one() {
        let mut g = ItemGenes::balanced();
        let before = g.acuity;
        g.socket(Gem::Sapphire);
        assert!(g.acuity > before, "a gem lifts a mid trait");
        let mut maxed = ItemGenes::new(Fixed::ONE, Fixed::ONE, Fixed::ONE, Fixed::ONE, Fixed::ONE, Fixed::ONE, Temper::Nearest);
        maxed.socket(Gem::Sapphire);
        assert!(maxed.acuity <= Fixed::ONE, "boost never exceeds 1");
    }

    #[test]
    fn breeding_is_deterministic_and_stays_in_range() {
        let a = ItemGenes::new(
            Fixed::from_frac(2, 10),
            Fixed::from_frac(8, 10),
            Fixed::from_frac(3, 10),
            Fixed::from_frac(7, 10),
            Fixed::from_frac(4, 10),
            Fixed::from_frac(6, 10),
            Temper::Weakest,
        );
        let b = ItemGenes::balanced();
        assert_eq!(breed(&a, &b, 42), breed(&a, &b, 42), "same parents + seed → same child");
        let c = breed(&a, &b, 42);
        for t in [c.acuity, c.coverage, c.aggression, c.loyalty, c.agility, c.verve] {
            assert!(t >= Fixed::ZERO && t <= Fixed::ONE, "genes stay in [0,1]");
        }
    }

    #[test]
    fn reroll_is_deterministic() {
        assert_eq!(reroll(7), reroll(7));
        assert_ne!(reroll(7), reroll(8));
    }

    #[test]
    fn material_biases_upward() {
        let plain = reroll(99);
        let met = roll_material(Material::Meteoric, 99);
        assert!(met.acuity >= plain.acuity && met.agility >= plain.agility, "meteoric sharpens & quickens");
    }
}
