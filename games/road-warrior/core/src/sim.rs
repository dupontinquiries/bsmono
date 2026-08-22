//! The seam between `vehicle` (dynamics) and `track` (terrain): neither module
//! knows about the other, so either can be swapped or tested alone. This is
//! the thin glue a game loop calls once per tick.

use crate::track::Track;
use crate::vehicle::{GroundContact, Vehicle};

/// Advance `vehicle` by `dt` against `track`, querying ground height/slope at
/// the vehicle's current x each tick. Panics-free even off the ends of the
/// track: `Track::height_at` returns `None` there, which this treats the same
/// as a Gap (no ground, i.e. driving off the end of the level).
pub fn advance(
    vehicle: &mut Vehicle,
    track: &Track,
    dt: f32,
    throttle: f32,
    tilt: f32,
    nitro_active: bool,
    gravity: f32,
) {
    let ground = track.height_at(vehicle.pos_x).map(|height| GroundContact {
        height,
        slope: track.slope_at(vehicle.pos_x),
    });
    vehicle.step_on_ground(dt, throttle, tilt, nitro_active, gravity, ground);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::Segment;
    use crate::vehicle::CarKind;

    fn ramp_then_gap_then_flat() -> Track {
        Track::new(
            "Test Strip",
            vec![
                Segment::Flat { length: 10.0, height: 0.0 },
                Segment::Ramp { length: 5.0, from_height: 0.0, to_height: 4.0 },
                Segment::Gap { length: 6.0 },
                Segment::Flat { length: 20.0, height: 0.0 },
            ],
        )
    }

    #[test]
    fn vehicle_follows_ramp_height_while_grounded() {
        let track = ramp_then_gap_then_flat();
        let mut v = Vehicle::new(CarKind::Pillager);
        // Drive to partway up the ramp (x in [10, 15)).
        v.pos_x = 12.5;
        advance(&mut v, &track, 1.0 / 60.0, 0.0, 0.0, false, -9.8);
        assert!(!v.airborne, "vehicle should stay grounded while on the ramp surface");
        let expected_height = track.height_at(v.pos_x).unwrap();
        assert!(
            (v.pos_y - expected_height).abs() < 1e-3,
            "vehicle should sit flush on the ramp: pos_y={} expected={}",
            v.pos_y,
            expected_height
        );
    }

    #[test]
    fn driving_off_ramp_into_gap_becomes_airborne_automatically() {
        let track = ramp_then_gap_then_flat();
        let mut v = Vehicle::new(CarKind::RedSedan);
        v.pos_x = 14.9; // just before the gap starts at x=15
        v.vel_x = 20.0; // fast enough to cross into the gap this tick
        assert!(!v.airborne);
        // Ground is queried at the *pre-move* position each tick, so crossing
        // the ramp's edge is detected one tick late (still grounded the frame
        // it crosses, airborne the next) — under 17ms at 60Hz, so two ticks
        // is the correct thing to assert here, not a workaround for a bug.
        advance(&mut v, &track, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
        advance(&mut v, &track, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
        assert!(v.airborne, "driving past the ramp's end into a Gap should auto-launch, not fall through the floor");
    }

    #[test]
    fn vehicle_lands_on_far_side_of_gap() {
        let track = ramp_then_gap_then_flat();
        let mut v = Vehicle::new(CarKind::SixWheeler);
        v.pos_x = 20.9; // inside the gap (15..21), just before far flat ground at x=21
        v.vel_x = 15.0;
        v.airborne = true;
        v.pos_y = 0.5; // still above ground level, falling
        v.vel_y = -2.0;
        for _ in 0..120 {
            advance(&mut v, &track, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
            if !v.airborne {
                break;
            }
        }
        assert!(!v.airborne, "vehicle should eventually land on the flat ground past the gap");
        assert!(v.pos_x > 21.0, "vehicle should have crossed onto the far platform");
    }

    /// Regression test for a real bug found by actually driving a fast car
    /// across a full-length track: outrunning the authored end of the level
    /// (easy to do — a RedSedan clears an 88-unit track in ~4.3s) used to put
    /// the vehicle into permanent freefall, because `height_at` returned
    /// `None` past `total_length` and nothing ever re-grounds a vehicle once
    /// airborne over `None` ground. Confirmed visually: the car fell off the
    /// bottom of the screen and never reappeared.
    #[test]
    fn driving_past_the_end_of_the_track_does_not_cause_permanent_freefall() {
        let track = ramp_then_gap_then_flat(); // total_length = 41.0
        let mut v = Vehicle::new(CarKind::RedSedan);
        for _ in 0..600 {
            // 10s at full throttle, far more than enough to clear 41 units
            advance(&mut v, &track, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
        }
        assert!(
            v.pos_y > -5.0,
            "vehicle should not be in freefall this far past the track's end: pos_y={}",
            v.pos_y
        );
        assert!(v.pos_x > track.total_length, "sanity check: vehicle should actually have outrun the track for this test to mean anything");
    }
}
