//! SURVIVAL — what a body can take, and how long you have to get inside.
//!
//! The star model generates worlds that will kill you. That is only
//! interesting if survival is legible: a player has to be able to look at a
//! place and know what it costs to stand there, and be told before it kills
//! them rather than after.
//!
//! **The race against time is not invented.** A solar flare announces itself
//! in light and arrives in particles, and those travel at very different
//! speeds. The X-ray and optical flash crosses 1 AU in about 8 minutes; the
//! coronal mass ejection behind it takes 15 to 90 HOURS. Carrington in 1859
//! took about 17.6 hours; the 2012 near-miss took 18.6. That gap is a real,
//! measured warning window — the sky brightens, and then you have a known
//! amount of time before the front hits.
//!
//! Which gives the mechanic for free and honestly: the flash is the siren,
//! the transit time is the clock, and the clock is SHORTER ON A CLOSE ORBIT,
//! so the worlds that flare hardest also warn you least. An M-dwarf world at
//! 0.05 AU gets minutes. Pluto would get days.
//!
//! Bodies differ in the way the fiction wants them to and the physics
//! supports. A Cybertronian is a machine: vacuum is nothing, the temperature
//! band is enormous, and what actually threatens it is the flare — charged
//! particles into unshielded electronics. A human in a suit has the opposite
//! profile: the suit handles vacuum and radiation for a while, and the
//! narrow, unforgiving limit is thermal and consumable.

use crate::sun::{Orbit, SunState, Surface};

/// Who is standing there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Body {
    /// a machine. Wide thermal band, indifferent to vacuum, vulnerable to
    /// charged particles.
    Cybertronian,
    /// pressure suit with radiation shielding and finite consumables
    HumanSuited,
    /// no suit. Included because the campaign has civilians in it and the
    /// numbers should say what that means.
    HumanBare,
}

/// What a body can take before it needs help.
#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    /// kelvin, sustained
    pub t_min: f32,
    pub t_max: f32,
    /// survives vacuum unaided
    pub vacuum_ok: bool,
    /// 0..1 flare severity it shrugs off
    pub flare_tolerance: f32,
    /// seconds of consumables; infinite for a machine
    pub endurance_s: f32,
}

impl Body {
    pub fn envelope(self) -> Envelope {
        match self {
            // metal, sealed, and built for a war fought across a solar
            // system. The band is wide but not infinite: past rock's solidus
            // the ground it stands on is liquid.
            Body::Cybertronian => Envelope {
                t_min: 20.0,
                t_max: 1_350.0,
                vacuum_ok: true,
                flare_tolerance: 0.35,
                endurance_s: f32::INFINITY,
            },
            // a good suit: vacuum and moderate radiation handled, thermal
            // band narrow, and the clock is the tank
            Body::HumanSuited => Envelope {
                t_min: 120.0,
                t_max: 400.0,
                vacuum_ok: true,
                flare_tolerance: 0.20,
                endurance_s: 8.0 * 3600.0,
            },
            Body::HumanBare => Envelope {
                t_min: 258.0,
                t_max: 318.0,
                vacuum_ok: false,
                flare_tolerance: 0.02,
                endurance_s: 90.0,
            },
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Body::Cybertronian => "CYBERTRONIAN",
            Body::HumanSuited => "SUITED",
            Body::HumanBare => "UNPROTECTED",
        }
    }
}

/// Why a place is dangerous, in the order it will kill you.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Threat {
    None,
    /// the ground is liquid
    Molten,
    Heat,
    Cold,
    Vacuum,
    /// particle front inbound or arrived
    Flare,
    /// air ran out
    Consumables,
}

impl Threat {
    pub fn label(self) -> &'static str {
        match self {
            Threat::None => "",
            Threat::Molten => "MOLTEN GROUND",
            Threat::Heat => "HEAT",
            Threat::Cold => "COLD",
            Threat::Vacuum => "VACUUM",
            Threat::Flare => "SOLAR FLARE",
            Threat::Consumables => "AIR",
        }
    }
}

/// Speed of light and a typical CME, in AU per second.
/// Light: 1 AU in 499 s. A fast CME: 1 AU in roughly 15 h.
pub const LIGHT_AU_S: f32 = 1.0 / 499.0;
pub const CME_FAST_AU_S: f32 = 1.0 / (15.0 * 3600.0);
pub const CME_SLOW_AU_S: f32 = 1.0 / (90.0 * 3600.0);

/// HOW LONG BETWEEN SEEING A FLARE AND BEING HIT BY IT.
///
/// The flash travels at c and the particles do not. This is the difference,
/// and it is the whole timer. Returns seconds.
pub fn flare_warning_s(orbit: &Orbit, fast: bool) -> f32 {
    let au = orbit.au.max(0.005);
    let light = au / LIGHT_AU_S;
    let front = au / if fast { CME_FAST_AU_S } else { CME_SLOW_AU_S };
    (front - light).max(0.0)
}

/// What is threatening this body here, right now.
pub fn threat(body: Body, orbit: &Orbit, sun: &SunState, flare_inbound: bool) -> Threat {
    let e = body.envelope();
    let t = orbit.surface_k();
    if orbit.surface() == Surface::Molten && t > e.t_max {
        return Threat::Molten;
    }
    if !e.vacuum_ok && orbit.air < 0.15 {
        return Threat::Vacuum;
    }
    // a flare in flight outranks a temperature you are currently surviving:
    // it is the thing with a deadline
    if flare_inbound && orbit.flare_severity() > e.flare_tolerance {
        return Threat::Flare;
    }
    if t > e.t_max {
        return Threat::Heat;
    }
    // night on an airless world is the cold case, and the sun state is what
    // knows whether it is night
    let night_drop = if orbit.air < 0.05 { 0.45 } else { 0.85 };
    let effective = t * (night_drop + (1.0 - night_drop) * sun.day);
    if effective < e.t_min {
        return Threat::Cold;
    }
    Threat::None
}

/// Seconds a body can stand this before it is in trouble. Infinite means the
/// place is survivable indefinitely for that body.
pub fn time_to_trouble(body: Body, orbit: &Orbit, sun: &SunState, flare_inbound: bool) -> f32 {
    let e = body.envelope();
    match threat(body, orbit, sun, flare_inbound) {
        Threat::None => e.endurance_s,
        Threat::Molten => 0.0,
        Threat::Vacuum => 12.0,
        Threat::Flare => flare_warning_s(orbit, true),
        Threat::Heat | Threat::Cold => {
            // overshoot decides how fast: a little over the band is minutes,
            // far over is seconds
            let t = orbit.surface_k();
            let over = if t > e.t_max { t - e.t_max } else { e.t_min - t };
            (600.0 / (1.0 + over / 40.0)).max(3.0)
        }
        Threat::Consumables => 0.0,
    }
}

/// A place to get inside.
#[derive(Clone, Copy, Debug)]
pub struct Bunker {
    /// metres from the player
    pub distance_m: f32,
    /// does it stop particles as well as pressure?
    pub shielded: bool,
}

/// CAN THEY MAKE IT? Distance against the clock, at a speed the body can
/// actually manage. Returns (reachable, seconds to spare).
pub fn can_reach(body: Body, b: &Bunker, seconds: f32, speed_ms: f32) -> (bool, f32) {
    if seconds.is_infinite() {
        return (true, f32::INFINITY);
    }
    let travel = b.distance_m / speed_ms.max(0.1);
    // a machine does not slow down; a suited human loses time to terrain
    let fudge = match body {
        Body::Cybertronian => 1.0,
        Body::HumanSuited => 1.35,
        Body::HumanBare => 1.2,
    };
    let need = travel * fudge;
    (need <= seconds, seconds - need)
}

/// The one safe band on a tidally locked world: the terminator ring, where
/// the sun sits permanently on the horizon. Returns the fraction of the
/// surface that is survivable, 0..1.
pub fn habitable_fraction(body: Body, orbit: &Orbit) -> f32 {
    let e = body.envelope();
    if !orbit.tidally_locked() {
        // the whole surface passes through day and night
        let t = orbit.surface_k();
        return if t <= e.t_max && t >= e.t_min { 1.0 } else { 0.0 };
    }
    // locked: the day face runs hot and the night face cold, and only a ring
    // between them sits inside the band
    let day_t = orbit.surface_k() * 1.35;
    let night_t = orbit.surface_k() * 0.35;
    if day_t <= e.t_max && night_t >= e.t_min {
        return 1.0; // an atmosphere thick enough to even it out
    }
    if orbit.surface_k() > e.t_max || orbit.surface_k() < e.t_min {
        return 0.0;
    }
    // a band whose width grows with how much air there is to move heat
    (0.05 + 0.35 * orbit.air.clamp(0.0, 1.0)).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sun::StarClass;

    fn day() -> SunState {
        SunState { dir: [0.0, 1.0, 0.0], strength: 0.6, color: [1.0; 3], day: 1.0, elev: 1.0, flaring: false }
    }
    fn night() -> SunState {
        SunState { day: 0.0, elev: -1.0, strength: 0.1, ..day() }
    }

    #[test]
    fn a_bot_stands_where_a_person_cannot() {
        // the whole reason the campaign can go to these places
        let luna = Orbit { star: StarClass::G, au: 1.0, albedo: 0.12, air: 0.0 };
        assert_eq!(threat(Body::Cybertronian, &luna, &day(), false), Threat::None);
        assert_eq!(threat(Body::HumanBare, &luna, &day(), false), Threat::Vacuum);
        // and a suit buys the middle ground rather than parity
        assert_eq!(threat(Body::HumanSuited, &luna, &day(), false), Threat::None);
        assert!(time_to_trouble(Body::HumanSuited, &luna, &day(), false).is_finite());
        assert!(time_to_trouble(Body::Cybertronian, &luna, &day(), false).is_infinite());
    }

    #[test]
    fn the_warning_is_real_and_shorter_where_it_matters_most() {
        // light arrives in minutes, the front in hours. That gap is the
        // timer, and it is not invented.
        let earth = Orbit { star: StarClass::G, au: 1.0, albedo: 0.3, air: 0.5 };
        let w = flare_warning_s(&earth, true);
        assert!(w > 13.0 * 3600.0 && w < 16.0 * 3600.0, "1 AU warning was {:.1} h", w / 3600.0);
        // an M-dwarf world orbits close, so it warns least AND flares most
        let m = Orbit { star: StarClass::M, au: 0.05, albedo: 0.2, air: 0.05 };
        let wm = flare_warning_s(&m, true);
        assert!(wm < 3000.0, "close-orbit warning was {:.0} s", wm);
        assert!(m.flare_severity() > earth.flare_severity());
        // that pairing is the point: worst hazard, least notice
        assert!(wm < w / 10.0);
    }

    #[test]
    fn a_flare_outranks_a_temperature_you_are_currently_surviving() {
        // it is the one with a deadline
        let m = Orbit { star: StarClass::M, au: 0.05, albedo: 0.2, air: 0.0 };
        assert_eq!(threat(Body::HumanSuited, &m, &day(), true), Threat::Flare);
        // and with nothing inbound the same place reads by its temperature
        assert_ne!(threat(Body::HumanSuited, &m, &day(), false), Threat::Flare);
    }

    #[test]
    fn molten_ground_is_immediate_for_everything() {
        let lava = Orbit { star: StarClass::G, au: 0.02, albedo: 0.1, air: 0.0 };
        assert_eq!(lava.surface(), Surface::Molten);
        for b in [Body::Cybertronian, Body::HumanSuited, Body::HumanBare] {
            assert_eq!(time_to_trouble(b, &lava, &day(), false), 0.0, "{b:?} stood on lava");
        }
    }

    #[test]
    fn night_on_an_airless_world_is_its_own_hazard() {
        // no air to hold the heat: the day side is survivable and the night
        // side is not, on the same rock
        let rock = Orbit { star: StarClass::G, au: 1.4, albedo: 0.2, air: 0.0 };
        let d = threat(Body::HumanSuited, &rock, &day(), false);
        let n = threat(Body::HumanSuited, &rock, &night(), false);
        assert_ne!(d, n, "day and night read the same on an airless world");
        assert_eq!(n, Threat::Cold);
    }

    #[test]
    fn the_bunker_run_is_winnable_and_losable() {
        let m = Orbit { star: StarClass::M, au: 0.05, albedo: 0.2, air: 0.0 };
        let secs = flare_warning_s(&m, true);
        let near = Bunker { distance_m: 400.0, shielded: true };
        let far = Bunker { distance_m: 40_000.0, shielded: true };
        assert!(can_reach(Body::Cybertronian, &near, secs, 18.0).0, "a bot could not make 400 m");
        assert!(!can_reach(Body::HumanSuited, &far, secs, 2.5).0, "a suited human crossed 40 km");
        // and the bot beats the human over the same ground, which is the
        // reason to be one
        let (_, bot_spare) = can_reach(Body::Cybertronian, &near, secs, 18.0);
        let (_, human_spare) = can_reach(Body::HumanSuited, &near, secs, 2.5);
        assert!(bot_spare > human_spare);
    }

    #[test]
    fn a_locked_world_is_habitable_only_at_its_terminator() {
        // 0.15 AU, not 0.04. At 0.04 the mean surface is 454 K and a suit
        // fails EVERYWHERE on it, terminator included — habitable_fraction
        // returned 0 and was right to. A terminator band only exists where
        // the mean is inside the band and the two faces are not, which is a
        // narrower set of worlds than I assumed.
        let locked = Orbit { star: StarClass::M, au: 0.15, albedo: 0.25, air: 0.10 };
        assert!(locked.tidally_locked());
        let f = habitable_fraction(Body::HumanSuited, &locked);
        assert!(f > 0.0 && f < 0.25, "terminator band was {f} at {} K", locked.surface_k());
        // and the too-close case is genuinely uninhabitable, not a band
        let scorched = Orbit { au: 0.04, ..locked };
        assert_eq!(habitable_fraction(Body::HumanSuited, &scorched), 0.0,
            "a 454 K locked world offered a habitable band");
        // MORE AIR IS NOT SIMPLY BETTER, which is the trade the model found
        // and I did not expect. Atmosphere spreads heat around a locked
        // world AND greenhouses it, so past a point the band it widens is a
        // band it has already cooked. At this orbit 0.9 atm takes the
        // surface from 234 K to 630 and the answer becomes zero.
        let thicker = Orbit { air: 0.9, ..locked };
        assert_eq!(habitable_fraction(Body::HumanSuited, &thicker), 0.0,
            "0.9 atm here should cook it: {} K", thicker.surface_k());
        // out where the greenhouse cannot dominate, more air DOES widen it
        let cold = Orbit { star: StarClass::M, au: 0.18, albedo: 0.3, air: 0.05 };
        let colder_thick = Orbit { air: 0.25, ..cold };
        if habitable_fraction(Body::HumanSuited, &cold) > 0.0 {
            assert!(habitable_fraction(Body::HumanSuited, &colder_thick)
                    >= habitable_fraction(Body::HumanSuited, &cold),
                "air did not widen the band where it could");
        }
    }

    #[test]
    fn an_unlocked_temperate_world_is_survivable_all_over() {
        let earth = Orbit { star: StarClass::G, au: 1.0, albedo: 0.3, air: 0.4 };
        assert!(!earth.tidally_locked());
        assert_eq!(habitable_fraction(Body::HumanSuited, &earth), 1.0);
    }
}
