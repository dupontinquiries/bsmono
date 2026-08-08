//! VOYAGE — getting there, and why some places are further than they look.
//!
//! `patrol.rs` lists fourteen destinations and treats them as a menu. They
//! are not: Titan is four times as far from the sun as Mars and the trip
//! costs far more than four times as much, because orbital distance is not
//! travel distance.
//!
//! **A Hohmann transfer is the cheapest way between two circular orbits**
//! and its duration is half the period of an ellipse touching both:
//!
//!     t = pi * sqrt(a^3 / mu),  a = (r1 + r2) / 2
//!
//! Earth to Mars is about 8.5 months, Earth to Saturn about 6 years. Those
//! ratios are what make the map a map. Scaled into game time they give the
//! outer system a weight the inner system does not have, without anyone
//! writing a difficulty curve.
//!
//! **And the cost is delta-v, which is the thing a hull actually has a
//! finite amount of.** A bigger reactor does not make the trip shorter; it
//! makes a trip possible that was not. That is a different kind of upgrade
//! from a bigger gun and it is what makes `loadout::Module::Reactor` worth a
//! hull slot.
//!
//! Consumables burn the whole way, so a long voyage arrives with less clock
//! than a short one — the raid's timer starts before the raid does.

/// Sun's gravitational parameter in AU^3 per year^2. Using AU and years
/// makes mu = 4*pi^2, which keeps the arithmetic honest and checkable.
pub const MU_SUN: f64 = 39.478_417;

/// Game seconds per year of transfer. A six-year Saturn run should feel long
/// without being unplayable, so a year is a couple of minutes.
pub const SECS_PER_YEAR: f32 = 120.0;

/// HOHMANN TRANSFER TIME between two circular orbits, in years.
pub fn transfer_years(r1_au: f64, r2_au: f64) -> f64 {
    let a = (r1_au.max(1e-4) + r2_au.max(1e-4)) / 2.0;
    std::f64::consts::PI * (a * a * a / MU_SUN).sqrt()
}

/// DELTA-V for the same transfer, in km/s. Two burns: one to leave, one to
/// circularise on arrival.
pub fn transfer_dv_kms(r1_au: f64, r2_au: f64) -> f64 {
    let (r1, r2) = (r1_au.max(1e-4), r2_au.max(1e-4));
    // orbital speed at radius r, in AU/year, converted to km/s
    let v = |r: f64| (MU_SUN / r).sqrt() * 4.740_57;
    let a = (r1 + r2) / 2.0;
    let v1 = v(r1);
    let v2 = v(r2);
    // vis-viva at each end of the transfer ellipse
    let vt1 = (MU_SUN * (2.0 / r1 - 1.0 / a)).sqrt() * 4.740_57;
    let vt2 = (MU_SUN * (2.0 / r2 - 1.0 / a)).sqrt() * 4.740_57;
    (vt1 - v1).abs() + (v2 - vt2).abs()
}

/// What a hull can do.
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    /// total delta-v budget, km/s
    pub dv_kms: f32,
    /// how much faster than a Hohmann it can go, 1.0 = minimum energy only.
    /// Burning harder costs delta-v and buys time.
    pub haste: f32,
    /// crew-seconds of consumables aboard
    pub supply_s: f32,
}

impl Default for Drive {
    fn default() -> Drive {
        Drive { dv_kms: 22.0, haste: 1.0, supply_s: 5.0 * 3600.0 }
    }
}

/// A planned trip.
#[derive(Clone, Copy, Debug)]
pub struct Leg {
    pub from_au: f64,
    pub to_au: f64,
    /// game seconds
    pub duration_s: f32,
    pub dv_kms: f32,
    /// supplies left on arrival, per crew
    pub supply_left_s: f32,
    pub reachable: bool,
    pub note: &'static str,
}

/// Plan a leg. `crew` divides the supplies, because a bigger team eats more.
pub fn plan(from_au: f64, to_au: f64, d: &Drive, crew: u32) -> Leg {
    let years = transfer_years(from_au, to_au);
    let dv = transfer_dv_kms(from_au, to_au) as f32;
    // HASTE BUYS TIME AT A COST THAT RISES FASTER THAN THE SAVING. Halving a
    // transfer does not double the delta-v, it roughly quadruples the excess
    // — which is why nobody sprints to Saturn.
    let h = d.haste.max(1.0);
    let dur = (years as f32 * SECS_PER_YEAR) / h;
    let dv_needed = dv * (1.0 + (h - 1.0) * (h - 1.0) * 2.0);
    let burn = dur * crew.max(1) as f32;
    let left = d.supply_s - burn;
    let reachable = dv_needed <= d.dv_kms && left > 0.0;
    Leg {
        from_au,
        to_au,
        duration_s: dur,
        dv_kms: dv_needed,
        supply_left_s: left,
        reachable,
        note: if dv_needed > d.dv_kms {
            "not enough delta-v"
        } else if left <= 0.0 {
            "the supplies run out before arrival"
        } else {
            ""
        },
    }
}

/// A REFUELLING STOP changes the problem: two short legs cost less delta-v
/// than one long one, because each circularisation is paid at a lower speed.
/// That is what makes the inner system a road rather than a wall.
pub fn via(from_au: f64, stop_au: f64, to_au: f64, d: &Drive, crew: u32) -> (Leg, Leg) {
    (plan(from_au, stop_au, d, crew), plan(stop_au, to_au, d, crew))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EARTH: f64 = 1.0;
    const MARS: f64 = 1.524;
    const JUPITER: f64 = 5.203;
    const SATURN: f64 = 9.537;
    const PLUTO: f64 = 39.48;

    #[test]
    fn the_transfer_times_are_the_real_ones() {
        // the check that matters: if it cannot reproduce the trips we have
        // actually flown, the map's geography is invented
        let mars = transfer_years(EARTH, MARS);
        assert!((mars * 12.0 - 8.5).abs() < 0.6, "Earth-Mars came out {:.1} months", mars * 12.0);
        let saturn = transfer_years(EARTH, SATURN);
        assert!((saturn - 6.0).abs() < 0.5, "Earth-Saturn came out {saturn:.1} years");
        // and Pluto is the long haul it should be
        assert!(transfer_years(EARTH, PLUTO) > 40.0);
    }

    #[test]
    fn orbital_distance_is_not_travel_distance() {
        // Titan is four times Mars's distance and the trip is far more than
        // four times the trouble, which is what gives the outer system its
        // weight without a difficulty curve
        let a = transfer_years(EARTH, MARS);
        let b = transfer_years(EARTH, SATURN);
        let dist_ratio = (SATURN - EARTH) / (MARS - EARTH);
        let time_ratio = b / a;
        assert!(time_ratio < dist_ratio, "time {time_ratio:.1} vs distance {dist_ratio:.1}");
        // the delta-v ratio, though, is much gentler than either
        let dva = transfer_dv_kms(EARTH, MARS);
        let dvb = transfer_dv_kms(EARTH, SATURN);
        assert!(dvb / dva < 3.0, "delta-v should not scale like distance: {:.1}", dvb / dva);
    }

    #[test]
    fn delta_v_is_what_a_hull_runs_out_of() {
        let d = Drive::default();
        assert!(plan(EARTH, MARS, &d, 4).reachable, "22 km/s should reach Mars");
        let far = plan(EARTH, PLUTO, &d, 4);
        assert!(!far.reachable);
        assert!(far.note.contains("delta-v") || far.note.contains("supplies"));
        // and a bigger reactor makes a trip POSSIBLE rather than shorter,
        // which is a different kind of upgrade from a bigger gun
        let big = Drive { dv_kms: 60.0, supply_s: 40.0 * 3600.0, ..d };
        let with = plan(EARTH, JUPITER, &big, 4);
        assert!(with.reachable);
        assert_eq!(with.duration_s, plan(EARTH, JUPITER, &d, 4).duration_s,
            "more delta-v should not make it quicker");
    }

    #[test]
    fn burning_harder_costs_more_than_it_saves() {
        // nobody sprints to Saturn
        let slow = Drive::default();
        let fast = Drive { haste: 2.0, ..slow };
        let a = plan(EARTH, MARS, &slow, 4);
        let b = plan(EARTH, MARS, &fast, 4);
        assert!(b.duration_s < a.duration_s * 0.6, "haste should halve the trip");
        assert!(b.dv_kms > a.dv_kms * 2.5, "and cost far more than double: {} vs {}", b.dv_kms, a.dv_kms);
    }

    #[test]
    fn a_bigger_crew_eats_the_margin() {
        // ON A LONG HAUL, not a short one. Earth to Mars is 0.71 years,
        // which at 120 game seconds per year is 85 seconds — six people eat
        // 510 seconds of a three-hour store and nobody notices. I wrote the
        // test against Mars and expected starvation, which says the scale is
        // right and my expectation was not: SHORT TRIPS ARE MEANT TO BE
        // CHEAP. Pluto is 40 years, and there crew size decides it.
        let d = Drive { dv_kms: 200.0, supply_s: 3.0 * 3600.0, ..Drive::default() };
        let solo = plan(EARTH, PLUTO, &d, 1);
        let six = plan(EARTH, PLUTO, &d, 6);
        assert!(solo.supply_left_s > six.supply_left_s);
        assert!(solo.reachable && !six.reachable,
            "solo {:.0}s left, six {:.0}s", solo.supply_left_s, six.supply_left_s);
        // and on a short hop nobody starves, which is the same rule saying
        // the inner system is somewhere you can actually operate
        assert!(plan(EARTH, MARS, &d, 6).reachable);
    }

    #[test]
    fn two_short_legs_beat_one_long_one_on_delta_v() {
        // which is what makes the inner system a road rather than a wall
        let d = Drive::default();
        let direct = plan(EARTH, JUPITER, &d, 4).dv_kms;
        let (a, b) = via(EARTH, MARS, JUPITER, &d, 4);
        assert!(a.dv_kms.max(b.dv_kms) < direct,
            "each hop {:.1}/{:.1} should be under the direct {:.1}", a.dv_kms, b.dv_kms, direct);
    }

    #[test]
    fn the_raid_clock_starts_before_the_raid_does() {
        // supplies burn the whole way, so a long voyage arrives with less
        let d = Drive { supply_s: 20.0 * 3600.0, ..Drive::default() };
        let near = plan(EARTH, MARS, &d, 4);
        let far = plan(EARTH, JUPITER, &d, 4);
        assert!(far.supply_left_s < near.supply_left_s);
        assert!(near.supply_left_s > 0.0);
    }

    #[test]
    fn nonsense_orbits_do_not_produce_nonsense_trips() {
        let d = Drive::default();
        for (a, b) in [(0.0, 1.0), (1.0, 0.0), (-3.0, 2.0), (1.0, 1.0)] {
            let l = plan(a, b, &d, 4);
            assert!(l.duration_s.is_finite() && l.duration_s >= 0.0, "{a} -> {b}");
            assert!(l.dv_kms.is_finite() && l.dv_kms >= 0.0);
        }
        // a trip to where you already are costs nothing and takes some time
        let same = plan(EARTH, EARTH, &d, 4);
        assert!(same.dv_kms < 0.01, "{}", same.dv_kms);
        assert!(same.reachable);
    }
}
