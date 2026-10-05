//! `sim::death_hold` — the moment between dying and standing up again.
//!
//! Survival death (r125) dropped the inventory where you fell and teleported you to spawn in
//! the same frame, with a notice: there was no moment of having died -- the screen simply
//! jumped. This is the hold: the veil comes down with "YOU DIED", the world keeps running for
//! [`DeathHold::SECS`], then the respawn happens and the veil lifts the way the loading veil
//! does. Pure; the fullgame owns the veil, the drops and the respawn.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeathHold {
    /// Seconds of hold left; 0 when not dead.
    left: f32,
}

impl DeathHold {
    pub const SECS: f32 = 2.5;

    /// Start the hold. `false` if one is already running (the vitality stays at zero for the
    /// whole hold, so the death condition keeps being true; only the first frame starts it).
    pub fn begin(&mut self) -> bool {
        if self.left > 0.0 {
            return false;
        }
        self.left = Self::SECS;
        true
    }

    pub fn holding(&self) -> bool {
        self.left > 0.0
    }

    /// Advance; `true` on the frame the hold ends -- the frame to respawn on.
    pub fn tick(&mut self, dt: f32) -> bool {
        if self.left <= 0.0 {
            return false;
        }
        self.left -= dt.max(0.0);
        if self.left <= 0.0 {
            self.left = 0.0;
            return true;
        }
        false
    }

    /// The veil's opacity while holding: full. Written every frame of the hold, so the
    /// loading veil's own decay takes over the moment it ends.
    pub fn veil(&self) -> f32 {
        if self.holding() { 1.0 } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hold_starts_once_runs_its_seconds_and_ends_on_one_frame() {
        let mut d = DeathHold::default();
        assert!(!d.holding() && d.veil() == 0.0);
        assert!(!d.tick(1.0), "nothing to end while not dead");
        assert!(d.begin());
        assert!(!d.begin(), "a second death inside the hold does not restart it");
        assert!(d.holding() && d.veil() == 1.0);
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut ended_at = None;
        for _ in 0..600 {
            t += dt;
            if d.tick(dt) {
                ended_at = Some(t);
                break;
            }
        }
        let at = ended_at.expect("the hold ends");
        assert!((at - DeathHold::SECS).abs() < 0.05, "ended at {at}");
        assert!(!d.holding() && d.veil() == 0.0);
        assert!(!d.tick(dt), "and ends only once");
        assert!(d.begin(), "a later death holds again");
    }
}
