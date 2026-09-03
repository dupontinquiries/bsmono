//! THE PARALYSIS FIELD — Blackstar's reverse-interface network, timed.
//!
//! Source: `TF_DOMINION_SHIELD_HINTS.md`'s Act 3 LHC showdown, and the
//! wider network described in `Assay_Horror.md`'s Dominion overview
//! — Blackstar threading a reverse-interface through the LHC and
//! pillar infrastructure while the Corrupted occupy every other
//! Autobot resource ("the horror is the noise. the network is the
//! signal.") The field itself is described precisely enough to run on:
//!
//!   "Keelwright is engaging Blackstar directly — timing strikes within
//!   the 12-second paralysis intervals... Blackstar holds Keelwright at
//!   the peak of a pulse window. Three seconds of stillness while the
//!   paralysis field resets."
//!
//! Twelve-second cycle, three of it safe. The rest of it is not a
//! random lockout — same design rule as the aerial clinch
//! (`clinch.rs`'s own words: "random" means "I could not have seen it
//! coming"): the field's phase is always readable, and a strike landed
//! outside the safe window is a player choice, not a dice roll. This
//! module is that clock, kept honest to the scene: nothing here fires
//! blind.
//!
//! **Not yet wired to a live encounter.** `raidrun.rs` is this game's
//! per-tick boss-mechanic layer and the natural home for it, but
//! Blackstar himself is a campaign champion (`content.rs`), not one of
//! the nineteen raid wardens (`raidcon.rs`), and the campaign's own
//! fight resolution (`skirmish.rs`) is an aggregate DPS model with no
//! phase timing today. Wiring this field into an actual Blackstar
//! encounter is a real seam — bolting it onto either system now risked
//! repeating a mistake already made twice this session (a change that
//! looks additive quietly breaking the campaign's winnability
//! guarantee). The clock is built, tested, and correct; where it
//! plugs in is a deliberate next step, not an oversight.

/// Full cycle length, per the doc's own number.
pub const PERIOD_SEC: f32 = 12.0;
/// How much of the cycle is safe — "three seconds of stillness."
pub const SAFE_SEC: f32 = 3.0;

/// The field's phase, live. `t` runs 0..PERIOD_SEC and wraps; safety is
/// a pure function of `t`, so the field can be sampled from anywhere
/// (a HUD, an AI's decision, a test) without owning the clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct Field {
    t: f32,
}

impl Field {
    /// A fresh field, phase-locked to the start of a safe window — the
    /// scene's own opening beat: the fight begins at the peak of a
    /// pulse window, not mid-paralysis.
    pub fn new() -> Field {
        Field { t: 0.0 }
    }
    pub fn tick(&mut self, dt: f32) {
        self.t = (self.t + dt.max(0.0)) % PERIOD_SEC;
    }
    /// The doc's "peak of a pulse window" / "three seconds of
    /// stillness" — a strike lands clean here.
    pub fn safe(&self) -> bool {
        self.t < SAFE_SEC
    }
    pub fn paralyzed(&self) -> bool {
        !self.safe()
    }
    /// How long until the field flips state — what a HUD or an AI
    /// reads to time its own strike, same as the scene's own logic
    /// ("timing strikes within the... intervals" implies knowing when
    /// they open, not guessing).
    pub fn until_flip(&self) -> f32 {
        if self.safe() {
            SAFE_SEC - self.t
        } else {
            PERIOD_SEC - self.t
        }
    }
    /// 0..1 through the CURRENT state (safe or paralyzed) — a bar the
    /// draw layer can fill without recomputing the phase math.
    pub fn progress(&self) -> f32 {
        if self.safe() {
            (self.t / SAFE_SEC).clamp(0.0, 1.0)
        } else {
            ((self.t - SAFE_SEC) / (PERIOD_SEC - SAFE_SEC)).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_field_opens_safe_and_the_cycle_matches_the_scene() {
        let f = Field::new();
        assert!(f.safe() && !f.paralyzed());
        assert_eq!(PERIOD_SEC, 12.0);
        assert_eq!(SAFE_SEC, 3.0);
    }

    #[test]
    fn the_field_flips_exactly_where_the_doc_says_it_does() {
        let mut f = Field::new();
        f.tick(SAFE_SEC - 0.01);
        assert!(f.safe(), "flipped early");
        f.tick(0.02);
        assert!(f.paralyzed(), "did not flip at the three-second mark");
        // stays paralyzed for the rest of the twelve-second cycle
        f.tick(PERIOD_SEC - SAFE_SEC - 0.02 - 0.01);
        assert!(f.paralyzed(), "flipped early on the long side");
        f.tick(0.02);
        assert!(f.safe(), "the cycle did not wrap back to safe at 12s");
    }

    #[test]
    fn the_field_never_fires_blind() {
        // every state transition is announced by until_flip reaching
        // zero, never by a jump a caller could not have seen coming
        let mut f = Field::new();
        let mut last_state = f.safe();
        let mut min_eta_before_flip = f32::MAX;
        for _ in 0..(PERIOD_SEC / 0.01) as u32 {
            let eta = f.until_flip();
            f.tick(0.01);
            if f.safe() != last_state {
                min_eta_before_flip = min_eta_before_flip.min(eta);
                last_state = f.safe();
            }
        }
        assert!(min_eta_before_flip < 0.02, "a transition arrived with no warning: eta {min_eta_before_flip}");
    }

    #[test]
    fn until_flip_counts_down_and_progress_stays_in_bounds() {
        let mut f = Field::new();
        let mut last_eta = f.until_flip();
        for _ in 0..((SAFE_SEC - 0.01) / 0.05) as u32 {
            f.tick(0.05);
            let eta = f.until_flip();
            assert!(eta < last_eta, "until_flip did not count down");
            last_eta = eta;
            assert!((0.0..=1.0).contains(&f.progress()));
        }
    }

    #[test]
    fn a_full_cycle_returns_to_where_it_started() {
        let mut f = Field::new();
        f.tick(PERIOD_SEC);
        assert_eq!(f.t, 0.0);
        assert!(f.safe());
    }

    #[test]
    fn the_safe_fraction_matches_the_scene_exactly() {
        // three of twelve seconds -- a quarter of the cycle is the
        // window "LA Hero is running node sabotage between," the rest
        // is what Keelwright has to survive
        assert!((SAFE_SEC / PERIOD_SEC - 0.25).abs() < 1e-6);
    }
}
