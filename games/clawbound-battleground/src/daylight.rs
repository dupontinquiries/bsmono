//! daylight — what time it is, and what that does to a cat's senses.
//!
//! Step 5 of `docs/WARRIORS_MECHANICS.md`. `wildkit::sim::climate` says what
//! the weather is like at a place; it does not say what time it is, and
//! without a clock the brief's *"time of day to vary how well relying on
//! certain systems function like los vs ears"* has nothing to vary against.
//!
//! ## Night does not improve hearing
//!
//! The obvious implementation is to raise `noise_weight` after dark, and it is
//! wrong. A cat's ears are the same ears at midnight. What changes is that
//! sight stops contributing, so the same awareness has to come from somewhere
//! else — hearing matters MORE at night because sight matters less, not
//! because anything got better.
//!
//! That distinction is not pedantry, it changes the numbers. Raising hearing
//! would make a cat at night strictly more perceptive than by day, so night
//! would be the good time to hunt and the whole tension inverts. Lowering
//! sight and renormalising keeps total perception roughly constant and moves
//! it between channels, which is what the brief asks for: the same cat, worse
//! at seeing you and therefore relatively better at hearing you.
//!
//! ## Fog CAPS sight, it does not scale it
//!
//! `fog_model::visibility_distance` gives an absolute distance past which
//! nothing is visible. Multiplying sight range by a fog factor would let a
//! sharp-eyed creature see further through thick fog than a dull one, which is
//! not how fog works — it is a wall at a distance, the same wall for everyone.
//! So the fog contribution is a `min`, not a product.

use wildkit::sim::{fog_model, perception::PerceptParams};

/// Where the sun is, coarsely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Dawn,
    Day,
    Dusk,
    Night,
}

impl Phase {
    /// From the hour, 0..24. Out-of-range hours wrap rather than panicking —
    /// a clock that advanced past midnight should not crash the world.
    pub fn from_hour(hour: f32) -> Phase {
        let h = hour.rem_euclid(24.0);
        match h {
            h if h < 5.0 => Phase::Night,
            h if h < 7.5 => Phase::Dawn,
            h if h < 18.0 => Phase::Day,
            h if h < 20.5 => Phase::Dusk,
            _ => Phase::Night,
        }
    }

    /// How much light there is, 0..1.
    ///
    /// Dawn and dusk are a single value rather than a ramp because the phase
    /// is coarse by design; a caller wanting a smooth sunrise should
    /// interpolate [`Sky::light01`] over the hour instead.
    pub fn light(self) -> f32 {
        match self {
            Phase::Night => 0.08,
            Phase::Dawn | Phase::Dusk => 0.45,
            Phase::Day => 1.0,
        }
    }

    /// Is this the low-light window the books hunt in?
    pub fn is_twilight(self) -> bool {
        matches!(self, Phase::Dawn | Phase::Dusk)
    }
}

/// The sky's current state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sky {
    /// 0..24, fractional.
    pub hour: f32,
    /// Cloud cover, 0 clear to 1 solid overcast.
    pub overcast: f32,
    /// Fog extinction coefficient, fed to `fog_model`. 0 is clear air.
    pub fog_sigma: f32,
    /// Rain, 0..1. Masks sound and washes scent.
    pub rain: f32,
}

impl Default for Sky {
    /// Noon, clear. The state to test against, and the one that must leave
    /// `PerceptParams` untouched.
    fn default() -> Sky {
        Sky { hour: 12.0, overcast: 0.0, fog_sigma: 0.0, rain: 0.0 }
    }
}

/// Below this, `visibility_distance` is asked for a distance that is
/// effectively infinite; clear air should not impose a cap at all.
pub const FOG_EPSILON: f32 = 1e-4;

/// The contrast threshold `visibility_distance` resolves against. Standard
/// meteorological visibility uses 2%, which is what the module's own docs
/// describe.
pub const FOG_CONTRAST: f32 = 0.02;

impl Sky {
    pub fn phase(self) -> Phase {
        Phase::from_hour(self.hour)
    }

    /// Usable light, 0..1: the phase, dimmed by cloud.
    ///
    /// Overcast never takes light to zero even at noon — a solidly overcast
    /// day is dim, not dark, and treating it as night would make weather
    /// stronger than the sun.
    pub fn light01(self) -> f32 {
        let cloud = 1.0 - 0.45 * self.overcast.clamp(0.0, 1.0);
        (self.phase().light() * cloud).clamp(0.0, 1.0)
    }

    /// Apply the sky to a creature's perception.
    ///
    /// Takes the creature's own daylight params and returns what it can
    /// actually do right now. The creature keeps its own numbers; this is a
    /// modifier, not a replacement, so a keen-eyed creature is still the
    /// keen-eyed one at dusk.
    pub fn apply(self, base: &PerceptParams) -> PerceptParams {
        let light = self.light01();

        // Sight falls with light, but never to nothing: a cat at midnight can
        // still see a moving shape a few body-lengths away, and zero would
        // make night a blindfold rather than a disadvantage.
        let sight_floor = 0.18;
        let by_light = base.sight_range * (sight_floor + (1.0 - sight_floor) * light);

        // Fog is a WALL at a distance — a min, not a product. See the header.
        let sight_range = if self.fog_sigma > FOG_EPSILON {
            by_light.min(fog_model::visibility_distance(self.fog_sigma, FOG_CONTRAST))
        } else {
            by_light
        };

        // Rain masks sound. Hearing is the channel that degrades in weather
        // rather than in darkness, which is why rain at noon is its own kind
        // of dangerous.
        let noise = base.noise_weight * (1.0 - 0.5 * self.rain.clamp(0.0, 1.0));

        // The weights RENORMALISE rather than move independently. Sight loses
        // what the light lost; the total stays where the creature's own params
        // put it, so darkness redistributes perception instead of deleting it.
        let sight_w = base.sight_weight * (sight_floor + (1.0 - sight_floor) * light);
        let total_base = base.noise_weight + base.sight_weight;
        let total_now = noise + sight_w;
        let scale = if total_now > 1e-6 { total_base / total_now } else { 1.0 };

        PerceptParams {
            sight_range,
            noise_weight: noise * scale,
            sight_weight: sight_w * scale,
            ..*base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PerceptParams {
        PerceptParams::default()
    }

    #[test]
    fn the_clock_reads_phases_and_wraps() {
        assert_eq!(Phase::from_hour(2.0), Phase::Night);
        assert_eq!(Phase::from_hour(6.0), Phase::Dawn);
        assert_eq!(Phase::from_hour(12.0), Phase::Day);
        assert_eq!(Phase::from_hour(19.0), Phase::Dusk);
        assert_eq!(Phase::from_hour(23.0), Phase::Night);
        // A clock past midnight must not crash the world.
        assert_eq!(Phase::from_hour(26.0), Phase::Night);
        assert_eq!(Phase::from_hour(-1.0), Phase::Night);
        assert!(Phase::Dawn.is_twilight() && Phase::Dusk.is_twilight());
        assert!(!Phase::Day.is_twilight());
    }

    /// A clear noon must change nothing, or every creature's tuning shifts
    /// under a feature that was supposed to add a state.
    #[test]
    fn clear_noon_leaves_perception_exactly_as_it_was() {
        let b = base();
        let out = Sky::default().apply(&b);
        assert!((out.sight_range - b.sight_range).abs() < 1e-3);
        assert!((out.sight_weight - b.sight_weight).abs() < 1e-3);
        assert!((out.noise_weight - b.noise_weight).abs() < 1e-3);
        assert_eq!(out.frustum_half, b.frustum_half, "untouched fields pass through");
    }

    /// THE RULE. Hearing must not IMPROVE at night — a cat's ears are the same
    /// ears at midnight. Its share rises because sight's share falls.
    #[test]
    fn night_shifts_perception_from_sight_to_hearing_without_improving_hearing() {
        let b = base();
        let night = Sky { hour: 1.0, ..Sky::default() }.apply(&b);

        assert!(night.sight_range < b.sight_range * 0.4, "sight is badly cut");
        assert!(night.sight_weight < b.sight_weight, "and contributes less");
        assert!(night.noise_weight > b.noise_weight, "so hearing's SHARE rises");

        // The total is preserved: the cat is not more perceptive at night,
        // which would invert the tension and make darkness the good time to
        // hunt.
        let before = b.noise_weight + b.sight_weight;
        let after = night.noise_weight + night.sight_weight;
        assert!((before - after).abs() < 1e-3, "{before} vs {after}");
    }

    /// Night is a disadvantage, not a blindfold.
    #[test]
    fn a_cat_at_midnight_can_still_see_something() {
        let out = Sky { hour: 1.0, ..Sky::default() }.apply(&base());
        assert!(out.sight_range > 0.0, "zero would be a blindfold");
        assert!(out.sight_range > base().sight_range * 0.1);
    }

    /// FOG IS A WALL. A min, not a product — otherwise a sharp-eyed creature
    /// sees further through the same fog than a dull one.
    #[test]
    fn fog_caps_sight_equally_for_keen_and_dull_eyes() {
        let keen = PerceptParams { sight_range: 80.0, ..base() };
        let dull = PerceptParams { sight_range: 20.0, ..base() };
        let thick = Sky { fog_sigma: 0.5, ..Sky::default() };
        let (k, d) = (thick.apply(&keen), thick.apply(&dull));
        let wall = fog_model::visibility_distance(0.5, FOG_CONTRAST);
        assert!(k.sight_range <= wall + 1e-3, "keen eyes stop at the wall too");
        // The dull creature was already inside the wall, so fog does not
        // lengthen its sight.
        assert!(d.sight_range <= dull.sight_range + 1e-3);
    }

    /// Clear air imposes no cap at all.
    #[test]
    fn clear_air_does_not_cap_sight() {
        let b = PerceptParams { sight_range: 500.0, ..base() };
        assert!((Sky::default().apply(&b).sight_range - 500.0).abs() < 1e-3);
    }

    /// Rain degrades HEARING, which is why rain at noon is its own kind of
    /// dangerous rather than just a darker day.
    #[test]
    fn rain_masks_sound_independently_of_the_light() {
        let b = base();
        let wet_noon = Sky { rain: 1.0, ..Sky::default() }.apply(&b);
        let dry_noon = Sky::default().apply(&b);
        assert!(wet_noon.noise_weight < dry_noon.noise_weight);
        assert!((wet_noon.sight_range - dry_noon.sight_range).abs() < 1e-3,
            "and leaves sight alone");
    }

    /// Overcast dims; it must not be darker than night, or weather outranks
    /// the sun.
    #[test]
    fn a_solidly_overcast_noon_is_dim_but_not_night() {
        let dull = Sky { overcast: 1.0, ..Sky::default() };
        let night = Sky { hour: 2.0, ..Sky::default() };
        assert!(dull.light01() < Sky::default().light01());
        assert!(dull.light01() > night.light01(), "overcast day beats midnight");
    }

    /// Twilight sits between, which is what makes dawn and dusk the hunting
    /// hours rather than merely dim daytime.
    #[test]
    fn twilight_falls_between_day_and_night() {
        let b = base();
        let d = Sky { hour: 12.0, ..Sky::default() }.apply(&b).sight_range;
        let t = Sky { hour: 19.0, ..Sky::default() }.apply(&b).sight_range;
        let n = Sky { hour: 2.0, ..Sky::default() }.apply(&b).sight_range;
        assert!(d > t && t > n, "day {d}, dusk {t}, night {n}");
    }

    /// A creature keeps its own character: keen eyes are still the keener at
    /// dusk. The sky is a modifier, not a replacement.
    #[test]
    fn a_keen_eyed_creature_is_still_the_keen_one_after_dark() {
        let keen = PerceptParams { sight_range: 64.0, ..base() };
        let dull = PerceptParams { sight_range: 16.0, ..base() };
        let night = Sky { hour: 2.0, ..Sky::default() };
        assert!(night.apply(&keen).sight_range > night.apply(&dull).sight_range);
    }
}
