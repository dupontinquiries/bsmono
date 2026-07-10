//! sim::object_permanence — keep believing a target exists after it's occluded.
//!
//! From `design_library/05/mob_brain_object_permanence`: a mob that loses sight
//! of its quarry and instantly forgets it has no object permanence — it behaves
//! as if the target ceased to exist. The doc is explicit that the fix is *not* an
//! AI-research problem but a **bookkeeping** one: keep a ghost record of where
//! the target was and how it was moving, grow an uncertainty volume as time
//! passes, dead-reckon the most-likely position for the movement system to
//! search, refresh on any new glimpse, and give up once the trail is cold.
//!
//! This module is that substrate (the terrain-aware volume shaping and full
//! Bayesian search-coverage update layer on top of it). `mob_memory` /
//! `landmark_memory` remember *places*; this remembers a *moving target*. Pure,
//! deterministic, std-only — time is passed in, never read.

/// The last known state of a tracked target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ghost {
    pub last_pos: [f32; 3],
    pub last_vel: [f32; 3],
    /// Time (seconds) at which the target was last actually seen.
    pub last_seen: f32,
}

/// Tracks one target's believed position while it's out of sight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracker {
    ghost: Option<Ghost>,
    /// How fast the target can move — sets how quickly uncertainty grows.
    target_max_speed: f32,
    /// A floor on the uncertainty volume even at the instant of sighting (the
    /// target's own body radius / sensor slop).
    base_radius: f32,
    /// Seconds of no sighting after which the trail is abandoned.
    give_up_after: f32,
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

impl Tracker {
    pub fn new(target_max_speed: f32, base_radius: f32, give_up_after: f32) -> Tracker {
        Tracker { ghost: None, target_max_speed: target_max_speed.max(0.0), base_radius: base_radius.max(0.0), give_up_after: give_up_after.max(0.0) }
    }

    /// Record a fresh sighting, resetting the trail's age and uncertainty.
    pub fn observe(&mut self, pos: [f32; 3], vel: [f32; 3], now: f32) {
        self.ghost = Some(Ghost { last_pos: pos, last_vel: vel, last_seen: now });
    }

    /// Is a target currently believed to exist (seen, not yet given up)?
    pub fn is_tracking(&self) -> bool {
        self.ghost.is_some()
    }

    pub fn ghost(&self) -> Option<Ghost> {
        self.ghost
    }

    /// Seconds since the target was last seen (clamped at 0), or `None` if not
    /// tracking.
    pub fn elapsed(&self, now: f32) -> Option<f32> {
        self.ghost.map(|g| (now - g.last_seen).max(0.0))
    }

    /// Dead-reckoned most-likely position: last seen position advanced by the
    /// last seen velocity over the elapsed time. This is the point the movement
    /// system should head toward / search first.
    pub fn predicted_position(&self, now: f32) -> Option<[f32; 3]> {
        let g = self.ghost?;
        let e = (now - g.last_seen).max(0.0);
        Some([g.last_pos[0] + g.last_vel[0] * e, g.last_pos[1] + g.last_vel[1] * e, g.last_pos[2] + g.last_vel[2] * e])
    }

    /// Radius of the uncertainty volume around the predicted position: it starts
    /// at `base_radius` on sighting and grows with `elapsed × target_max_speed`,
    /// since the target could have diverged from the dead-reckon at up to its top
    /// speed. `None` if not tracking.
    pub fn uncertainty_radius(&self, now: f32) -> Option<f32> {
        self.elapsed(now).map(|e| self.base_radius + e * self.target_max_speed)
    }

    /// Confidence the ghost is still accurate: `1` at sighting, falling linearly
    /// to `0` at `give_up_after`. `0` when not tracking.
    pub fn confidence(&self, now: f32) -> f32 {
        match self.elapsed(now) {
            Some(e) if self.give_up_after > 0.0 => (1.0 - e / self.give_up_after).clamp(0.0, 1.0),
            Some(_) => 0.0, // give_up_after == 0 ⇒ never confident once time passes
            None => 0.0,
        }
    }

    /// Has the trail gone cold (elapsed ≥ give_up_after)?
    pub fn should_give_up(&self, now: f32) -> bool {
        self.elapsed(now).map_or(false, |e| e >= self.give_up_after)
    }

    /// Advance the tracker: if the trail has gone cold, drop the ghost (the mob
    /// returns to idle/wander). Returns `true` if it just gave up this call.
    pub fn tick(&mut self, now: f32) -> bool {
        if self.is_tracking() && self.should_give_up(now) {
            self.ghost = None;
            true
        } else {
            false
        }
    }

    /// Is `point` inside the current uncertainty volume (i.e. a plausible place
    /// the target now is)? Used to test whether a search sweep has covered the
    /// belief, or whether a new glimpse corroborates the ghost.
    pub fn contains(&self, point: [f32; 3], now: f32) -> bool {
        match (self.predicted_position(now), self.uncertainty_radius(now)) {
            (Some(c), Some(r)) => dist(c, point) <= r,
            _ => false,
        }
    }

    /// Fold in a *partial* observation — a fleeting glimpse, a heard sound, a
    /// fresh footprint — rather than a clean line-of-sight sighting. Blends the
    /// ghost's last-known position toward `pos` with weight `confidence`, capped
    /// at 0.85 because partial information can't fully resolve the target, and
    /// rewinds the trail age toward `now` by that same weight (a glimpse cools the
    /// trail less than a full sighting resets it). Does nothing without an active
    /// ghost: partial info alone can't *acquire* a target — that needs `observe`.
    pub fn observe_partial(&mut self, pos: [f32; 3], confidence: f32, now: f32) {
        if let Some(g) = self.ghost.as_mut() {
            let w = confidence.clamp(0.0, 0.85);
            for i in 0..3 {
                g.last_pos[i] += (pos[i] - g.last_pos[i]) * w;
            }
            g.last_seen += (now - g.last_seen) * w;
        }
    }

    /// A deterministic *systematic* search position for the movement system:
    /// sample `i` of a phyllotaxis (sunflower) spiral over the current uncertainty
    /// disc in the XZ plane, centred on the dead-reckoned position. Successive `i`
    /// spread evenly from the most-likely centre outward, so the searcher sweeps
    /// the volume in confidence order instead of wandering blindly. `total` is how
    /// many samples the sweep will use (sets the outer radius reach). `None` when
    /// the trail is cold / no ghost.
    pub fn search_point(&self, now: f32, i: u32, total: u32) -> Option<[f32; 3]> {
        if self.should_give_up(now) {
            return None;
        }
        let centre = self.predicted_position(now)?;
        let r_max = self.uncertainty_radius(now)?;
        let total = total.max(1);
        // sqrt() radius mapping = equal-area rings → even coverage.
        let frac = (i.min(total - 1) as f32 + 0.5) / total as f32;
        let r = r_max * frac.sqrt();
        // golden angle (~137.5°) — the phyllotaxis spacing that avoids spokes.
        const GOLDEN_ANGLE: f32 = 2.399_963_2;
        let a = i as f32 * GOLDEN_ANGLE;
        Some([centre[0] + r * a.cos(), centre[1], centre[2] + r * a.sin()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> Tracker {
        // target tops out at 4 b/s, 0.5 base radius, gives up after 5 s.
        Tracker::new(4.0, 0.5, 5.0)
    }

    #[test]
    fn a_new_tracker_believes_nothing() {
        let t = tracker();
        assert!(!t.is_tracking());
        assert_eq!(t.predicted_position(1.0), None);
        assert_eq!(t.confidence(1.0), 0.0);
        assert!(!t.should_give_up(1.0));
    }

    #[test]
    fn at_the_moment_of_sighting_belief_is_exact_and_certain() {
        let mut t = tracker();
        t.observe([2.0, 0.0, 3.0], [1.0, 0.0, 0.0], 10.0);
        assert!(t.is_tracking());
        assert_eq!(t.predicted_position(10.0), Some([2.0, 0.0, 3.0]));
        assert!((t.uncertainty_radius(10.0).unwrap() - 0.5).abs() < 1e-6, "starts at base radius");
        assert!((t.confidence(10.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn belief_dead_reckons_along_the_last_velocity() {
        let mut t = tracker();
        t.observe([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], 0.0);
        let p = t.predicted_position(2.0).unwrap(); // 2 s at 2 b/s ⇒ x=4
        assert!((p[0] - 4.0).abs() < 1e-6, "got {p:?}");
    }

    #[test]
    fn uncertainty_grows_with_time_and_target_speed() {
        let mut t = tracker();
        t.observe([0.0; 3], [0.0; 3], 0.0);
        let r1 = t.uncertainty_radius(1.0).unwrap(); // 0.5 + 1*4
        let r3 = t.uncertainty_radius(3.0).unwrap(); // 0.5 + 3*4
        assert!((r1 - 4.5).abs() < 1e-6);
        assert!((r3 - 12.5).abs() < 1e-6);
        assert!(r3 > r1, "volume must expand as the trail ages");
    }

    #[test]
    fn confidence_falls_linearly_to_zero_at_give_up() {
        let mut t = tracker();
        t.observe([0.0; 3], [0.0; 3], 0.0);
        assert!((t.confidence(2.5) - 0.5).abs() < 1e-6, "halfway ⇒ 0.5");
        assert!((t.confidence(5.0) - 0.0).abs() < 1e-6);
        assert!((t.confidence(9.0) - 0.0).abs() < 1e-6, "never negative");
    }

    #[test]
    fn the_trail_goes_cold_and_tick_drops_the_ghost() {
        let mut t = tracker();
        t.observe([0.0; 3], [0.0; 3], 0.0);
        assert!(!t.should_give_up(4.9));
        assert!(t.should_give_up(5.0));
        assert!(!t.tick(4.0), "still warm ⇒ keep tracking");
        assert!(t.is_tracking());
        assert!(t.tick(6.0), "cold ⇒ give up this call");
        assert!(!t.is_tracking(), "ghost dropped");
        assert!(!t.tick(7.0), "already given up ⇒ no second give-up");
    }

    #[test]
    fn a_fresh_sighting_resets_age_uncertainty_and_confidence() {
        let mut t = tracker();
        t.observe([0.0; 3], [1.0, 0.0, 0.0], 0.0);
        // ...time passes, trail widens...
        assert!(t.uncertainty_radius(3.0).unwrap() > 5.0);
        // ...then we see it again at a new spot:
        t.observe([9.0, 0.0, 0.0], [0.0; 3], 3.0);
        assert_eq!(t.predicted_position(3.0), Some([9.0, 0.0, 0.0]));
        assert!((t.uncertainty_radius(3.0).unwrap() - 0.5).abs() < 1e-6, "reset to base");
        assert!((t.confidence(3.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn contains_reports_whether_a_point_is_in_the_belief_volume() {
        let mut t = tracker();
        t.observe([0.0; 3], [0.0; 3], 0.0);
        // at t=1, predicted [0,0,0], radius 4.5
        assert!(t.contains([4.0, 0.0, 0.0], 1.0), "inside the volume");
        assert!(!t.contains([10.0, 0.0, 0.0], 1.0), "outside the volume");
        assert!(!t.contains([0.0; 3], 1.0) == false, "the centre is always inside");
    }

    #[test]
    fn belief_queries_are_deterministic() {
        let mut t = tracker();
        t.observe([1.0, 2.0, 3.0], [0.5, 0.0, -0.5], 2.0);
        assert_eq!(t.predicted_position(5.0), t.predicted_position(5.0));
        assert_eq!(t.uncertainty_radius(5.0), t.uncertainty_radius(5.0));
    }

    #[test]
    fn partial_observation_blends_not_resets() {
        let mut t = tracker();
        t.observe([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 0.0);
        // A glimpse of the target at x=10 with 50% confidence pulls the belief
        // partway (0.5 of the way), not all the way.
        t.observe_partial([10.0, 0.0, 0.0], 0.5, 2.0);
        let g = t.ghost().unwrap();
        assert!((g.last_pos[0] - 5.0).abs() < 1e-4, "blended halfway, got {}", g.last_pos[0]);
        // Confidence is capped at 0.85 — even a "certain" partial can't fully resolve.
        let mut t2 = tracker();
        t2.observe([0.0, 0.0, 0.0], [0.0; 3], 0.0);
        t2.observe_partial([10.0, 0.0, 0.0], 1.0, 2.0);
        assert!(t2.ghost().unwrap().last_pos[0] <= 8.5 + 1e-3);
        // Partial info without a prior sighting acquires nothing.
        let mut t3 = tracker();
        t3.observe_partial([1.0, 2.0, 3.0], 0.8, 1.0);
        assert!(!t3.is_tracking());
    }

    #[test]
    fn search_points_cover_the_uncertainty_disc() {
        let mut t = tracker();
        t.observe([0.0, 0.0, 0.0], [0.0; 3], 0.0);
        let now = 3.0;
        let r = t.uncertainty_radius(now).unwrap();
        let c = t.predicted_position(now).unwrap();
        // Every sample lies within the disc, and the first is nearer the centre
        // than the last (centre-out ordering).
        let p0 = t.search_point(now, 0, 16).unwrap();
        let plast = t.search_point(now, 15, 16).unwrap();
        for i in 0..16 {
            let p = t.search_point(now, i, 16).unwrap();
            let d = ((p[0] - c[0]).powi(2) + (p[2] - c[2]).powi(2)).sqrt();
            assert!(d <= r + 1e-3, "sample {i} outside disc");
        }
        let d0 = ((p0[0] - c[0]).powi(2) + (p0[2] - c[2]).powi(2)).sqrt();
        let dl = ((plast[0] - c[0]).powi(2) + (plast[2] - c[2]).powi(2)).sqrt();
        assert!(d0 < dl, "search should sweep centre-outward");
        // Cold trail → no search point.
        assert_eq!(t.search_point(100.0, 0, 16), None);
    }
}
