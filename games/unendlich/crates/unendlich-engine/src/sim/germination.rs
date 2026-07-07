//! `sim::germination` — environment-gated seed germination.
//!
//! The plant analog of the dragon incubation imprint (spec §14.5.2): a seed's
//! genome plus the local elemental fields decide whether and when it sprouts,
//! and how strongly the environment imprints a rare, field-shifting **bloom**
//! strain. Pure, deterministic, panic-free (R1/R6/R11); standalone
//! `rustc --test`-able.
//!
//! Model: warmth (`r`) and moisture (`h`) together make the environment
//! favourable — they both raise the chance to clear the dormancy threshold and
//! shorten the delay. The `dormancy` gene raises the threshold the environment
//! must clear; `vigor` shortens the delay once conditions are met. Jointly high
//! biotic/moist/lit fields (`b`,`h`,`l`) push `strain_bias` toward the bloom
//! strain, which (downstream, the coordinator's job) nudges the local `b`/`h`
//! fields upward — a living seed that shapes the elemental weather around it.

/// A seed's heritable germination genes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SeedGenome {
    /// Elemental lineage tag (identity only; not consumed by the germinate gate).
    pub element: u8,
    /// Growth vigor `0..1` — higher shortens the germination delay.
    pub vigor: f32,
    /// Dormancy `0..1` — higher raises the environmental threshold to sprout.
    pub dormancy: f32,
}

/// Local elemental field readings at the seed, each `0..1`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Fields {
    /// Moisture / Hydro.
    pub h: f32,
    /// Warmth / Radiant.
    pub r: f32,
    /// Light.
    pub l: f32,
    /// Spirit/soil (reserved — not consumed by this gate; kept for the field set).
    pub s: f32,
    /// Biotic.
    pub b: f32,
}

/// The germination decision for a seed in its environment.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Germination {
    /// Whether the seed sprouts at all under these conditions.
    pub will_germinate: bool,
    /// Time to sprout in seconds (finite, `>= 0`; long when conditions are poor).
    pub delay: f32,
    /// `0..1` lean toward the rare field-shifting bloom strain.
    pub strain_bias: f32,
}

/// Longest germination delay (seconds) — returned when conditions are barely/not
/// favourable.
pub const MAX_DELAY: f32 = 120.0;
/// Base threshold the environment must clear at zero dormancy.
const DORMANCY_BASE: f32 = 0.2;
/// How much the dormancy gene raises that threshold.
const DORMANCY_GAIN: f32 = 0.5;
/// Fraction of the delay that full vigor removes.
const VIGOR_CUT: f32 = 0.6;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Decide whether/when a seed germinates and how much it leans bloom.
pub fn germinate(seed: &SeedGenome, f: &Fields) -> Germination {
    let vigor = clamp01(seed.vigor);
    let dormancy = clamp01(seed.dormancy);
    let h = clamp01(f.h);
    let r = clamp01(f.r);
    let l = clamp01(f.l);
    let b = clamp01(f.b);

    // Favourability: warmth + moisture, both needed.
    let env = 0.5 * (r + h);
    // Dormancy raises the bar the environment must clear.
    let threshold = DORMANCY_BASE + DORMANCY_GAIN * dormancy;
    let will_germinate = env >= threshold;

    // Delay shortens with favourability and with vigor; always finite, >= 0.
    let env_factor = (1.0 - env).max(0.0).min(1.0); // high env -> short
    let vigor_factor = 1.0 - VIGOR_CUT * vigor; // high vigor -> shorter (down to 0.4x)
    let delay = (MAX_DELAY * env_factor * vigor_factor).max(0.0);

    // Bloom strain bias: only high when biotic + moist + lit are *jointly* high
    // (product → rare). Downstream, a high-bias sprout edits the local b/h fields.
    let strain_bias = clamp01(b * h * l);

    Germination { will_germinate, delay, strain_bias }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(vigor: f32, dormancy: f32) -> SeedGenome {
        SeedGenome { element: 0, vigor, dormancy }
    }
    fn fields(h: f32, r: f32, l: f32, b: f32) -> Fields {
        Fields { h, r, l, s: 0.5, b }
    }

    #[test]
    fn dry_cold_is_dormant() {
        let g = germinate(&seed(0.5, 0.3), &fields(0.0, 0.0, 0.2, 0.2));
        assert!(!g.will_germinate, "dry+cold should not germinate");
        // and the delay is long when conditions are poor
        assert!(g.delay > MAX_DELAY * 0.5, "poor conditions should give a long delay");
    }

    #[test]
    fn warm_moist_germinates_short_delay() {
        let g = germinate(&seed(0.5, 0.2), &fields(1.0, 1.0, 0.8, 0.5));
        assert!(g.will_germinate, "warm+moist should germinate");
        assert!(g.delay < MAX_DELAY * 0.5, "favourable conditions should be quick: {}", g.delay);
    }

    #[test]
    fn high_dormancy_needs_stronger_conditions() {
        let env = fields(0.5, 0.5, 0.5, 0.5); // env = 0.5
        assert!(germinate(&seed(0.5, 0.0), &env).will_germinate, "low dormancy clears env 0.5");
        assert!(!germinate(&seed(0.5, 1.0), &env).will_germinate, "high dormancy should not clear env 0.5");
    }

    #[test]
    fn high_vigor_shortens_delay() {
        let env = fields(0.6, 0.6, 0.5, 0.4);
        let slow = germinate(&seed(0.0, 0.2), &env).delay;
        let fast = germinate(&seed(1.0, 0.2), &env).delay;
        assert!(fast < slow, "vigor should shorten delay: {} !< {}", fast, slow);
    }

    #[test]
    fn strain_bias_rises_with_b_h_l_jointly() {
        let low = germinate(&seed(0.5, 0.2), &fields(0.2, 0.5, 0.2, 0.2)).strain_bias;
        let high = germinate(&seed(0.5, 0.2), &fields(0.9, 0.5, 0.9, 0.9)).strain_bias;
        assert!(high > low, "joint high B/H/L should raise strain_bias");
        // raising any single one (others fixed high) raises it
        let mid = germinate(&seed(0.5, 0.2), &fields(0.9, 0.5, 0.5, 0.9)).strain_bias;
        assert!(high > mid, "raising L should raise strain_bias");
        // one factor near zero keeps it rare
        let rare = germinate(&seed(0.5, 0.2), &fields(0.9, 0.5, 0.9, 0.02)).strain_bias;
        assert!(rare < 0.1, "low biotic should keep bloom rare");
    }

    #[test]
    fn delay_finite_nonneg_and_clamps_out_of_range() {
        let mut s = 0x34FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        for _ in 0..3000 {
            // deliberately out-of-range and NaN-ish inputs
            let sd = seed(rng() * 4.0 - 2.0, rng() * 4.0 - 2.0);
            let fl = Fields {
                h: rng() * 4.0 - 2.0,
                r: rng() * 4.0 - 2.0,
                l: rng() * 4.0 - 2.0,
                s: f32::NAN,
                b: rng() * 4.0 - 2.0,
            };
            let g = germinate(&sd, &fl);
            assert!(g.delay.is_finite() && g.delay >= 0.0 && g.delay <= MAX_DELAY, "delay oob: {}", g.delay);
            assert!((0.0..=1.0).contains(&g.strain_bias), "strain_bias oob: {}", g.strain_bias);
        }
        // explicit NaN genome doesn't panic / NaN out
        let g = germinate(&seed(f32::NAN, f32::INFINITY), &fields(f32::NAN, 1.0, 1.0, 1.0));
        assert!(g.delay.is_finite() && (0.0..=1.0).contains(&g.strain_bias));
    }

    #[test]
    fn deterministic() {
        let sd = seed(0.4, 0.3);
        let fl = fields(0.7, 0.6, 0.5, 0.8);
        assert_eq!(germinate(&sd, &fl), germinate(&sd, &fl));
    }
}
