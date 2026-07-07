//! sim::locomotion_mode — which medium a character is moving through.
//!
//! Walking (`fauna_gait`/`foot_ik`), swimming (`swim`), and flying
//! (`dragon_flight`) each exist as their own physics; this is the controller
//! that decides *which one is active* from the body's state — grounded,
//! submerged, or airborne — and drives the transitions between them (wading in,
//! diving, taking off, losing flight, landing). Boundaries use **hysteresis** so
//! a character standing waist-deep or skimming a surface doesn't flip modes
//! every tick. Pure state machine, deterministic, std-only; the caller then runs
//! the matching physics for the returned mode.

/// The active locomotion medium.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocomotionMode {
    /// On the ground — gait + foot IK.
    Walk,
    /// Submerged enough to swim — buoyancy + stroke thrust + drag.
    Swim,
    /// Airborne under active flight — lift/glide/flap.
    Fly,
    /// Airborne with no flight — ballistic fall.
    Fall,
}

/// Per-tick body state feeding the mode decision.
#[derive(Clone, Copy, Debug)]
pub struct MediumInputs {
    /// Feet in contact with ground/surface.
    pub grounded: bool,
    /// Fraction of the body underwater, `0..=1`.
    pub submersion: f32,
    /// Flight is engaged and available (can fly *and* wants to).
    pub flying: bool,
}

/// Submersion at which a walking/falling character starts swimming.
pub const WATER_ENTER: f32 = 0.60;
/// Submersion below which a swimmer stops swimming (hysteresis vs [`WATER_ENTER`]).
pub const WATER_EXIT: f32 = 0.35;

/// The mode controller. Holds the current mode so transitions can be hysteretic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locomotion {
    mode: LocomotionMode,
}

impl Default for Locomotion {
    fn default() -> Self {
        Locomotion { mode: LocomotionMode::Walk }
    }
}

impl Locomotion {
    pub fn new() -> Locomotion {
        Locomotion::default()
    }

    pub fn mode(&self) -> LocomotionMode {
        self.mode
    }

    /// Advance the mode from this tick's body state and return it.
    ///
    /// Priority: deep enough water ⇒ Swim (even while nominally grounded on a
    /// riverbed); else grounded ⇒ Walk; else Fly if flight is engaged, otherwise
    /// Fall. The water boundary is hysteretic — you start swimming at
    /// [`WATER_ENTER`] and don't stop until you rise above [`WATER_EXIT`].
    pub fn update(&mut self, i: &MediumInputs) -> LocomotionMode {
        let s = i.submersion.clamp(0.0, 1.0);
        let in_water = if self.mode == LocomotionMode::Swim {
            s > WATER_EXIT
        } else {
            s >= WATER_ENTER
        };

        self.mode = if in_water {
            LocomotionMode::Swim
        } else if i.grounded {
            LocomotionMode::Walk
        } else if i.flying {
            LocomotionMode::Fly
        } else {
            LocomotionMode::Fall
        };
        self.mode
    }

    /// Is the character off the ground (flying or falling)?
    pub fn is_airborne(&self) -> bool {
        matches!(self.mode, LocomotionMode::Fly | LocomotionMode::Fall)
    }

    /// Which physics the caller should run this tick.
    pub fn applies_buoyancy(&self) -> bool {
        self.mode == LocomotionMode::Swim
    }
    pub fn applies_gait(&self) -> bool {
        self.mode == LocomotionMode::Walk
    }
    pub fn applies_flight(&self) -> bool {
        self.mode == LocomotionMode::Fly
    }
    pub fn applies_gravity(&self) -> bool {
        // gravity acts while walking (normal force cancels it), falling, and
        // flying (lift opposes it); swimming replaces it with buoyancy.
        self.mode != LocomotionMode::Swim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(grounded: bool, submersion: f32, flying: bool) -> MediumInputs {
        MediumInputs { grounded, submersion, flying }
    }

    #[test]
    fn defaults_to_walking() {
        assert_eq!(Locomotion::new().mode(), LocomotionMode::Walk);
    }

    #[test]
    fn grounded_and_dry_walks() {
        let mut l = Locomotion::new();
        assert_eq!(l.update(&inp(true, 0.0, false)), LocomotionMode::Walk);
        assert_eq!(l.update(&inp(true, 0.2, false)), LocomotionMode::Walk, "shallow wading still walks");
        assert!(l.applies_gait());
    }

    #[test]
    fn deep_water_swims_even_when_grounded() {
        let mut l = Locomotion::new();
        assert_eq!(l.update(&inp(true, 0.7, false)), LocomotionMode::Swim, "on a riverbed but submerged");
        assert!(l.applies_buoyancy());
        assert!(!l.applies_gravity());
    }

    #[test]
    fn water_boundary_is_hysteretic() {
        let mut l = Locomotion::new();
        // rising water: walk until 0.6
        assert_eq!(l.update(&inp(true, 0.5, false)), LocomotionMode::Walk);
        assert_eq!(l.update(&inp(true, 0.6, false)), LocomotionMode::Swim);
        // falling water: keep swimming until below 0.35
        assert_eq!(l.update(&inp(true, 0.45, false)), LocomotionMode::Swim, "still swimming at 0.45");
        assert_eq!(l.update(&inp(true, 0.30, false)), LocomotionMode::Walk, "surfaced ⇒ walk");
    }

    #[test]
    fn leaving_the_ground_flies_or_falls() {
        let mut l = Locomotion::new();
        assert_eq!(l.update(&inp(false, 0.0, true)), LocomotionMode::Fly);
        assert!(l.is_airborne() && l.applies_flight());
        assert_eq!(l.update(&inp(false, 0.0, false)), LocomotionMode::Fall, "flight lost ⇒ fall");
        assert!(l.is_airborne() && !l.applies_flight());
    }

    #[test]
    fn diving_from_air_into_water_switches_to_swim() {
        let mut l = Locomotion::new();
        l.update(&inp(false, 0.0, false)); // falling
        assert_eq!(l.update(&inp(false, 0.8, false)), LocomotionMode::Swim, "plunged in");
    }

    #[test]
    fn landing_returns_to_walk() {
        let mut l = Locomotion::new();
        l.update(&inp(false, 0.0, true)); // flying
        assert_eq!(l.update(&inp(true, 0.0, false)), LocomotionMode::Walk);
    }

    #[test]
    fn deterministic() {
        let mut a = Locomotion::new();
        let mut b = Locomotion::new();
        let seq = [inp(true, 0.0, false), inp(false, 0.0, true), inp(false, 0.7, false), inp(true, 0.1, false)];
        for i in &seq {
            assert_eq!(a.update(i), b.update(i));
        }
        assert_eq!(a, b);
    }
}
