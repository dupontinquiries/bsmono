//! GRIME — dirt and scorch accumulate on a body, and fade slowly.
//!
//! Per SECTION (`sections::Section`, the same six-part split damage
//! already uses — a section that got shot reads scorched there, a
//! section that hit the ground reads dusty there), two independent
//! channels because they read differently and come from different
//! events:
//! * DUST — ground contact: a hard landing, a fall, kicked-up terrain.
//!   Reads as pale, matte, terrain-coloured.
//! * SCORCH — weapon impact: carbon, heat discolouration. Reads as
//!   dark, independent of whether the hit did lasting hp damage —
//!   armour that shrugged off a shot still shows where it hit.
//!
//! Both are 0..1 and both fade — slowly, and only that: this is
//! WEATHERING, not cleaning. A body standing clean for a long time
//! reads cleaner than one mid-fight, but grime is meant to feel
//! earned and linger, not evaporate the moment the fight ends the way
//! `sections::Body`'s hp regen deliberately does.

use crate::sections::Section;

/// Seconds of fade to remove one full (1.0) layer, each channel. Dust
/// fades faster than scorch — mud brushes off in normal movement;
/// carbon scoring takes a real cleaning, not just time, so it fades
/// far slower to read as "this needs maintenance" rather than "this
/// will go away on its own."
pub const DUST_FADE_SECONDS: f32 = 90.0;
pub const SCORCH_FADE_SECONDS: f32 = 600.0;

#[derive(Clone, Debug)]
pub struct Grime {
    dust: [f32; 6],
    scorch: [f32; 6],
}

impl Default for Grime {
    fn default() -> Grime {
        Grime { dust: [0.0; 6], scorch: [0.0; 6] }
    }
}

fn idx(s: Section) -> usize {
    Section::ALL.iter().position(|x| *x == s).unwrap_or(1)
}

impl Grime {
    pub fn dust_of(&self, s: Section) -> f32 {
        self.dust[idx(s)]
    }
    pub fn scorch_of(&self, s: Section) -> f32 {
        self.scorch[idx(s)]
    }
    /// Combined 0..1 reading a renderer can use for a single "how
    /// dirty does this section look" blend, when it does not need the
    /// two channels separately (a tint pass, an ambient-occlusion-style
    /// darkening) — dust and scorch are not additive in real life
    /// either (a badly scorched panel is not made worse by mud on top
    /// of it in the same visual sense two dust layers would stack),
    /// so this is the louder of the two, not the sum.
    pub fn total(&self, s: Section) -> f32 {
        self.dust_of(s).max(self.scorch_of(s))
    }

    pub fn add_dust(&mut self, s: Section, amount: f32) {
        let i = idx(s);
        self.dust[i] = (self.dust[i] + amount.max(0.0)).min(1.0);
    }
    pub fn add_scorch(&mut self, s: Section, amount: f32) {
        let i = idx(s);
        self.scorch[i] = (self.scorch[i] + amount.max(0.0)).min(1.0);
    }

    /// Advance the fade. Both channels move toward 0, never below it,
    /// at their own independent rate.
    pub fn tick(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let dust_rate = dt / DUST_FADE_SECONDS;
        let scorch_rate = dt / SCORCH_FADE_SECONDS;
        for i in 0..6 {
            self.dust[i] = (self.dust[i] - dust_rate).max(0.0);
            self.scorch[i] = (self.scorch[i] - scorch_rate).max(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_body_is_clean() {
        let g = Grime::default();
        for s in Section::ALL {
            assert_eq!(g.dust_of(s), 0.0);
            assert_eq!(g.scorch_of(s), 0.0);
            assert_eq!(g.total(s), 0.0);
        }
    }

    #[test]
    fn dust_accumulates_and_clamps_at_one() {
        let mut g = Grime::default();
        g.add_dust(Section::LegL, 0.4);
        assert!((g.dust_of(Section::LegL) - 0.4).abs() < 1e-6);
        g.add_dust(Section::LegL, 0.9);
        assert_eq!(g.dust_of(Section::LegL), 1.0, "dust should clamp at 1.0, not overflow");
        // untouched sections are unaffected
        assert_eq!(g.dust_of(Section::Head), 0.0);
    }

    #[test]
    fn scorch_and_dust_are_independent_channels() {
        let mut g = Grime::default();
        g.add_scorch(Section::Torso, 0.5);
        assert_eq!(g.dust_of(Section::Torso), 0.0, "scorch should not bleed into the dust channel");
        g.add_dust(Section::Torso, 0.3);
        assert!((g.scorch_of(Section::Torso) - 0.5).abs() < 1e-6, "adding dust should not touch scorch");
    }

    #[test]
    fn total_is_the_louder_channel_not_the_sum() {
        let mut g = Grime::default();
        g.add_dust(Section::ArmR, 0.3);
        g.add_scorch(Section::ArmR, 0.7);
        assert!((g.total(Section::ArmR) - 0.7).abs() < 1e-6, "total should be max(dust, scorch), not 1.0 (their sum)");
    }

    #[test]
    fn dust_fades_faster_than_scorch() {
        let mut g = Grime::default();
        g.add_dust(Section::Torso, 1.0);
        g.add_scorch(Section::Torso, 1.0);
        g.tick(DUST_FADE_SECONDS * 0.5); // half the dust fade time
        assert!(g.dust_of(Section::Torso) < 0.6, "dust should have faded substantially: {}", g.dust_of(Section::Torso));
        assert!(g.scorch_of(Section::Torso) > 0.9, "scorch should barely have moved yet: {}", g.scorch_of(Section::Torso));
    }

    #[test]
    fn fade_never_goes_negative() {
        let mut g = Grime::default();
        g.add_dust(Section::Torso, 0.1);
        g.tick(DUST_FADE_SECONDS * 100.0); // way past full fade
        assert_eq!(g.dust_of(Section::Torso), 0.0);
    }
}
