//! HYDRO — running on a seabed, and the tide that tries to take you off it.
//!
//! Water is the same fluid problem as air with one number changed, and that
//! number changes everything: seawater is about 830 times denser than air at
//! one atmosphere. Drag goes as density, so a current a swimmer would call
//! brisk is a wall.
//!
//! **That is what makes a water world a different game rather than a
//! reskin.** On land a machine's mass is a cost — `posture.rs` says a heavy
//! bot folds slowly, `actuator.rs` says its limbs are slow. Underwater the
//! same mass is what keeps it on the floor while lighter things are swept
//! away. The heavy chassis nobody picks for a race is the one that walks the
//! trench.
//!
//! **The tide is a clock you can read**, like the flare warning. It is
//! periodic and it telegraphs: the water slacks, reverses, and builds. So an
//! open crossing is not "hope you make it", it is "go on the slack and be
//! behind something before the flood" — which is a timing problem with a
//! visible signal rather than a dice roll.

use std::f32::consts::TAU;

/// Density of seawater, kg/m^3. Air at sea level is 1.225.
pub const RHO_SEA: f32 = 1_025.0;
/// Ratio, which is the whole reason this module exists.
pub const DENSITY_RATIO: f32 = RHO_SEA / 1.225;

/// Where you are in the water column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    /// wading; waves reach you and the tide is at its most violent
    Surf,
    /// the lost-city depth: light, structure, and the current still bites
    Shallows,
    /// open water between structures — nothing to hold, nothing to hide behind
    OpenWater,
    /// deep enough that surface tide is muted and pressure is the problem
    Trench,
}

impl Zone {
    /// How much of the surface tide reaches this depth. A tide is a surface
    /// phenomenon; it attenuates with depth, which is why the trench is calm
    /// and terrifying for a different reason.
    pub fn tide_reach(self) -> f32 {
        match self {
            Zone::Surf => 1.0,
            Zone::Shallows => 0.75,
            Zone::OpenWater => 0.55,
            Zone::Trench => 0.15,
        }
    }
    /// Is there anything to grab?
    pub fn has_cover(self) -> bool {
        matches!(self, Zone::Shallows | Zone::Trench)
    }
    pub fn label(self) -> &'static str {
        match self {
            Zone::Surf => "SURF",
            Zone::Shallows => "SHALLOWS",
            Zone::OpenWater => "OPEN WATER",
            Zone::Trench => "TRENCH",
        }
    }
}

/// A world's tide, as a rhythm rather than a number.
#[derive(Clone, Copy, Debug)]
pub struct Tide {
    /// seconds for a full ebb-and-flood cycle
    pub period_s: f32,
    /// peak current in m/s at the surface
    pub peak_ms: f32,
}

impl Default for Tide {
    fn default() -> Tide {
        Tide { period_s: 90.0, peak_ms: 4.5 }
    }
}

impl Tide {
    /// Current speed at time `t` in zone `z`. Signed: it reverses, and the
    /// reversal is the tell.
    pub fn current(&self, t: f32, z: Zone) -> f32 {
        (TAU * t / self.period_s.max(1e-3)).sin() * self.peak_ms * z.tide_reach()
    }

    /// Seconds until the next slack water — the moment to move.
    ///
    /// This is the mechanic: an open crossing is timed against a rhythm you
    /// can see, not gambled. The water goes still, reverses, and builds.
    pub fn next_slack_s(&self, t: f32) -> f32 {
        let half = self.period_s * 0.5;
        let phase = t.rem_euclid(half);
        half - phase
    }

    /// How long the water stays crossable, given a body that can hold
    /// against `hold_ms` of current.
    pub fn window_s(&self, hold_ms: f32, z: Zone) -> f32 {
        let peak = self.peak_ms * z.tide_reach();
        if peak <= hold_ms {
            return f32::INFINITY; // it never gets strong enough to matter
        }
        // fraction of the half-cycle where |sin| stays under the threshold
        let frac = (hold_ms / peak).clamp(0.0, 1.0).asin() / (TAU / 4.0);
        self.period_s * 0.5 * frac
    }
}

/// A body in water, as the part measurements already describe it.
#[derive(Clone, Copy, Debug)]
pub struct Swimmer {
    /// kg
    pub mass: f32,
    /// m^3 of displaced water — from solidity x bbox, which partclass measures
    pub volume: f32,
    /// m^2 presented to the flow
    pub frontal_m2: f32,
    /// 0 sleek .. 1 bluff. partclass::aero returns this.
    pub cd: f32,
    /// how much of the flow passes THROUGH rather than around — a ducted or
    /// skeletal body is far harder to sweep than a slab of the same size
    pub through: f32,
}

impl Swimmer {
    /// Buoyant weight: what is left of your weight once the water has helped.
    /// Negative means you float, and floating on a tidal world is a way to
    /// leave it.
    pub fn effective_weight_n(&self, g: f32) -> f32 {
        // displaced mass is volume in m^3 times density in kg/m^3, and the
        // first version cancelled the density away with a chain of 1000s
        // that reduced to volume * 1.025 — so a 1.2 m^3 body displaced 1.2
        // kg and nothing ever floated.
        (self.mass - self.volume * RHO_SEA) * g
    }

    /// Drag force at this current, newtons.
    pub fn drag_n(&self, current_ms: f32) -> f32 {
        0.5 * RHO_SEA * current_ms * current_ms
            * self.frontal_m2
            * self.cd
            * (1.0 - self.through.clamp(0.0, 0.95))
    }

    /// The current at which the flow overcomes friction and takes you.
    /// `mu` is grip on the seabed — rock is high, silt is not.
    pub fn hold_ms(&self, g: f32, mu: f32) -> f32 {
        let w = self.effective_weight_n(g);
        if w <= 0.0 {
            return 0.0; // buoyant: nothing holds you down at all
        }
        let denom = 0.5 * RHO_SEA * self.frontal_m2 * self.cd * (1.0 - self.through.clamp(0.0, 0.95));
        if denom <= 1e-9 {
            return f32::INFINITY;
        }
        (w * mu / denom).sqrt()
    }

    /// Will this body stay put right now?
    pub fn holds(&self, current_ms: f32, g: f32, mu: f32) -> bool {
        current_ms.abs() <= self.hold_ms(g, mu)
    }
}

/// Walking speed underwater: drag scales the effort, so a bluff body is slow
/// even when it is heavy enough to stay down.
pub fn walk_speed_ms(s: &Swimmer, land_speed: f32, g: f32) -> f32 {
    let w = s.effective_weight_n(g).max(1.0);
    // the drag at your own walking speed opposes you; solve roughly
    let d = 0.5 * RHO_SEA * s.frontal_m2 * s.cd * (1.0 - s.through.clamp(0.0, 0.95));
    let v = (w / d.max(1e-6)).sqrt();
    land_speed.min(v).max(0.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: f32 = 9.81;

    /// a heavy dense machine: 900 kg, 0.55 m^3, 1.6 m^2 frontal, fairly bluff
    fn bot() -> Swimmer {
        Swimmer { mass: 900.0, volume: 0.55, frontal_m2: 1.6, cd: 0.9, through: 0.1 }
    }
    /// a flyer: light, big, and mostly wing. Volume 0.22 rather than 0.9 —
    /// a metal aircraft is not a balloon, and at 0.9 it displaces 922 kg and
    /// simply floats away, which is true of an airliner and not of this.
    fn flyer() -> Swimmer {
        Swimmer { mass: 260.0, volume: 0.22, frontal_m2: 3.4, cd: 0.7, through: 0.05 }
    }
    /// something skeletal — a frame the water goes through
    fn frame() -> Swimmer {
        Swimmer { mass: 700.0, volume: 0.4, frontal_m2: 1.8, cd: 0.9, through: 0.75 }
    }

    #[test]
    fn water_is_the_same_problem_as_air_with_one_number_changed() {
        assert!((DENSITY_RATIO - 837.0).abs() < 5.0, "{DENSITY_RATIO}");
        // which is why a current a swimmer calls brisk is a wall
        let b = bot();
        assert!(b.drag_n(2.0) > 2_000.0, "{} N at 2 m/s", b.drag_n(2.0));
    }

    #[test]
    fn the_heavy_chassis_nobody_picks_for_a_race_walks_the_trench() {
        // on land mass is a cost; here it is what keeps you on the floor
        let b = bot().hold_ms(G, 0.7);
        let f = flyer().hold_ms(G, 0.7);
        assert!(b > f * 1.5, "bot holds {b:.1} m/s, flyer {f:.1}");
    }

    #[test]
    fn a_body_the_water_goes_through_is_far_harder_to_sweep() {
        // through-flow is measured by partclass::aero and it matters more
        // here than it ever did in air
        let solid = bot().hold_ms(G, 0.7);
        let open = frame().hold_ms(G, 0.7);
        assert!(open > solid, "skeletal {open:.1} vs solid {solid:.1}");
    }

    #[test]
    fn a_buoyant_body_holds_nothing_at_all() {
        // floating on a tidal world is a way to leave it
        let float = Swimmer { mass: 200.0, volume: 1.2, ..flyer() };  // 1230 kg displaced
        assert!(float.effective_weight_n(G) < 0.0);
        assert_eq!(float.hold_ms(G, 0.9), 0.0);
        assert!(!float.holds(0.1, G, 0.9));
    }

    #[test]
    fn the_tide_reverses_and_the_reversal_is_the_tell() {
        let t = Tide::default();
        let a = t.current(0.0, Zone::OpenWater);
        let b = t.current(t.period_s * 0.5, Zone::OpenWater);
        assert!(a.abs() < 0.01 && b.abs() < 0.01, "slack at both ends: {a} {b}");
        let mid = t.current(t.period_s * 0.25, Zone::OpenWater);
        let back = t.current(t.period_s * 0.75, Zone::OpenWater);
        assert!(mid > 0.0 && back < 0.0, "it should run both ways");
    }

    #[test]
    fn slack_water_is_predictable_which_is_what_makes_it_a_timing_problem() {
        let t = Tide::default();
        // just after a slack, you have almost the whole half-cycle
        assert!((t.next_slack_s(0.1) - 44.9).abs() < 0.5, "{}", t.next_slack_s(0.1));
        // just before one, almost none
        assert!(t.next_slack_s(t.period_s * 0.5 - 0.1) < 0.5);
    }

    #[test]
    fn depth_mutes_the_tide_so_the_trench_is_calm_for_a_different_reason() {
        let t = Tide::default();
        let surf = t.current(t.period_s * 0.25, Zone::Surf).abs();
        let deep = t.current(t.period_s * 0.25, Zone::Trench).abs();
        assert!(surf > deep * 5.0, "surf {surf:.2} trench {deep:.2}");
        assert!(Zone::Trench.has_cover() && !Zone::OpenWater.has_cover());
    }

    #[test]
    fn the_crossing_window_shrinks_as_the_body_gets_lighter() {
        // which is the whole decision: who crosses, and when
        let t = Tide::default();
        let heavy = t.window_s(bot().hold_ms(G, 0.7), Zone::OpenWater);
        let light = t.window_s(flyer().hold_ms(G, 0.7), Zone::OpenWater);
        assert!(heavy > light, "heavy {heavy:.1}s light {light:.1}s");
        assert!(light > 0.0, "a flyer should get SOME window, not none");
    }

    #[test]
    fn open_water_is_worse_than_the_shallows_at_the_same_moment() {
        let t = Tide::default();
        let at = t.period_s * 0.25;
        assert!(t.current(at, Zone::OpenWater).abs() < t.current(at, Zone::Shallows).abs());
        // and the shallows have something to hold, which open water does not
        assert!(Zone::Shallows.has_cover() && !Zone::OpenWater.has_cover());
    }

    #[test]
    fn walking_underwater_is_slower_than_walking_and_never_stops_you_dead() {
        let land = 12.0;
        let v = walk_speed_ms(&bot(), land, G);
        assert!(v < land, "the water should cost something: {v}");
        assert!(v >= 0.2, "it should never pin you at zero");
        // a sleeker body walks faster at the same weight
        let sleek = Swimmer { cd: 0.35, ..bot() };
        assert!(walk_speed_ms(&sleek, land, G) > v);
    }
}
