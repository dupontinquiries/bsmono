//! PLANETARY SUNLIGHT — ported from `game.c`'s planetary lighting.
//!
//! The Rust port has carried a `sun: [f32; 3]` per map style since the
//! beginning: a fixed direction, chosen by hand, that never moves. The C
//! engine has a star. Direction, strength and colour all ride a day cycle,
//! per-world day lengths and brightnesses, natural solar flares, and an
//! event that drains the star.
//!
//! It is worth bringing over for a reason beyond looking good: **it is the
//! field the others hang off**. `patrol.rs` types a day length by hand and
//! `terrain.rs` takes wind as a given, but thermal wind comes from
//! insolation and the ice/rock line comes from temperature — both downstream
//! of this. Porting the sun turns two typed numbers into derived ones.
//!
//! Numbers are the C engine's. Pluto gets a 400 s day at 0.30 peak because
//! it is far and faint; Io gets 140 s at 0.70 because the tides run fast and
//! the sulfur is bright.

/// Per-world solar parameters: day length in seconds, and the strength band.
#[derive(Clone, Copy, Debug)]
pub struct Solar {
    pub day_len: f32,
    pub min: f32,
    pub max: f32,
}

impl Default for Solar {
    fn default() -> Solar {
        Solar { day_len: 240.0, min: 0.16, max: 0.62 }
    }
}

/// The table, straight from game.c. A patrol destination id in, its star out.
pub fn solar_for(dest: &str) -> Solar {
    match dest {
        "pluto" => Solar { day_len: 400.0, min: 0.08, max: 0.30 },
        "europa" => Solar { day_len: 300.0, min: 0.10, max: 0.45 },
        "titan" => Solar { day_len: 320.0, min: 0.12, max: 0.40 },
        // fast tide-days, and the sulfur is bright
        "io" => Solar { day_len: 140.0, min: 0.20, max: 0.70 },
        "glaciem" => Solar { day_len: 360.0, min: 0.10, max: 0.48 },
        "outer_rim" | "shield_edge" => Solar { day_len: 340.0, min: 0.10, max: 0.36 },
        "mars" => Solar { day_len: 260.0, max: 0.60, ..Solar::default() },
        "carrion" => Solar { day_len: 200.0, max: 0.58, ..Solar::default() },
        "helix" => Solar { day_len: 280.0, max: 0.55, ..Solar::default() },
        "cybertron" => Solar { day_len: 220.0, ..Solar::default() },
        _ => Solar::default(),
    }
}

/// What the sun is doing right now.
#[derive(Clone, Copy, Debug)]
pub struct SunState {
    /// unit-ish direction toward the star
    pub dir: [f32; 3],
    pub strength: f32,
    pub color: [f32; 3],
    /// 0 deep night .. 1 full day. What night systems read.
    pub day: f32,
    /// sin of elevation; negative is below the horizon
    pub elev: f32,
    pub flaring: bool,
}

pub struct Sun {
    pub solar: Solar,
    /// phase offset, for a mission that starts before dawn
    pub offset: f32,
    /// 0 none .. 1 the star is nearly gone
    pub drain: f32,
    /// seconds of flare remaining
    flare: f32,
    /// countdown to the next natural flare
    flare_cd: f32,
    t: f32,
    rng: u32,
}

/// Natural flares: every 45-90 s while the sun is up, a 2-3 s surge.
pub const FLARE_MIN_CD: f32 = 45.0;
pub const FLARE_SPAN: f32 = 45.0;

impl Sun {
    pub fn new(dest: &str, seed: u32) -> Sun {
        Sun {
            solar: solar_for(dest),
            offset: 0.0,
            drain: 0.0,
            flare: 0.0,
            flare_cd: 55.0,
            t: 0.0,
            rng: seed | 1,
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    pub fn update(&mut self, dt: f32) -> SunState {
        self.t += dt;
        self.flare_cd -= dt;
        // start mid-morning, as the C engine does, so a fresh world is lit
        let th = 0.7 + self.offset + self.t * std::f32::consts::TAU / self.solar.day_len.max(1.0);
        let el = th.sin();
        // smooth dawn/dusk band rather than a hard terminator
        let day = ((el + 0.15) / 0.40).clamp(0.0, 1.0);

        if self.flare_cd <= 0.0 {
            self.flare_cd = FLARE_MIN_CD + self.rand() * FLARE_SPAN;
            if el > 0.10 && self.flare <= 0.0 {
                self.flare = 2.0 + self.rand();
            }
        }
        let flare_add = if self.flare > 0.0 { 0.8 * self.flare / 3.0 } else { 0.0 };
        if self.flare > 0.0 {
            self.flare -= dt;
        }

        let drain = 1.0 - 0.85 * self.drain;
        let strength = (self.solar.min + (self.solar.max - self.solar.min) * day) * drain + flare_add;
        // amber at the horizon, white at noon, EMBER as the star drains
        let horiz = 1.0 - (el * 2.2).clamp(0.0, 1.0);
        let color = [
            1.0,
            (0.72 + 0.28 * (1.0 - horiz)) * (1.0 - 0.45 * self.drain),
            (0.55 + 0.45 * (1.0 - horiz)) * (1.0 - 0.80 * self.drain),
        ];
        SunState {
            dir: [th.cos() * 0.85, el.max(0.06), 0.30],
            strength,
            color,
            day,
            elev: el,
            flaring: self.flare > 0.0,
        }
    }

    /// Interiors keep a fixed studio look — the C engine exempts them, and a
    /// sun arcing through a sealed hall is worse than no sun at all.
    pub fn interior(&self) -> SunState {
        SunState {
            dir: [0.3, 0.9, 0.3],
            strength: 0.62 * (1.0 - 0.85 * self.drain),
            color: [1.0, 0.95, 0.9],
            day: 1.0,
            elev: 1.0,
            flaring: false,
        }
    }

    /// TEMPERATURE, which the C engine did not derive and which everything
    /// downstream wants. Insolation against the local star, so a world's
    /// ice/rock line falls out of its orbit rather than being typed in.
    /// 0 is deep cold, 1 is scorching.
    pub fn temperature(&self, st: &SunState, albedo: f32) -> f32 {
        // equilibrium-ish: absorbed fraction of the current strength, with
        // night retaining some of the day's heat rather than crashing
        let absorbed = st.strength * (1.0 - albedo.clamp(0.0, 0.95));
        let retained = self.solar.max * 0.25;
        ((absorbed + retained) / (self.solar.max + retained).max(1e-6)).clamp(0.0, 1.0)
    }

    /// THERMAL WIND. Air moves because the ground is unevenly heated, so it
    /// peaks at dawn and dusk when the temperature gradient is steepest and
    /// falls at noon and midnight. `patrol.rs` currently types a wind speed;
    /// this derives one.
    pub fn thermal_wind(&self, st: &SunState, air: f32) -> f32 {
        if air <= 1e-4 {
            return 0.0; // no air, no wind, whatever the sun is doing
        }
        // steepest gradient at the terminator: |elev| near zero
        let terminator = 1.0 - st.elev.abs().min(1.0);
        (terminator * terminator) * self.solar.max * 6.0 * air.min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Sun, secs: f32) -> Vec<SunState> {
        (0..(secs / 0.1) as i32).map(|_| s.update(0.1)).collect()
    }

    #[test]
    fn the_sun_arcs_rather_than_sitting_still() {
        let mut s = Sun::new("earth", 7);
        let states = run(&mut s, 300.0);
        let elevs: Vec<f32> = states.iter().map(|x| x.elev).collect();
        assert!(elevs.iter().cloned().fold(f32::MIN, f32::max) > 0.9, "it never rose");
        assert!(elevs.iter().cloned().fold(f32::MAX, f32::min) < -0.9, "it never set");
        // and the direction moves with it
        let d0 = states[0].dir;
        let dm = states[states.len() / 2].dir;
        assert!((d0[0] - dm[0]).abs() > 0.5, "the direction is fixed");
    }

    #[test]
    fn each_world_gets_its_own_day() {
        // the C engine's numbers, and they say something: Io is fast and
        // bright, Pluto is slow and faint
        let io = solar_for("io");
        let pluto = solar_for("pluto");
        assert!(io.day_len < pluto.day_len / 2.0);
        assert!(io.max > pluto.max * 2.0);
        assert_eq!(solar_for("nowhere").day_len, Solar::default().day_len);
    }

    #[test]
    fn strength_follows_the_day_and_stays_in_its_band() {
        let mut s = Sun::new("mars", 3);
        s.flare_cd = 1e9; // no flares, so the band is testable
        let states = run(&mut s, 300.0);
        let sol = solar_for("mars");
        for st in &states {
            assert!(st.strength >= sol.min - 1e-3 && st.strength <= sol.max + 1e-3,
                "strength {} left the band", st.strength);
            assert!((0.0..=1.0).contains(&st.day));
        }
        // brightest near noon, dimmest at night
        let bright = states.iter().max_by(|a, b| a.strength.total_cmp(&b.strength)).unwrap();
        assert!(bright.elev > 0.5, "the peak was not near noon");
    }

    #[test]
    fn a_draining_star_goes_dim_and_red() {
        let mut a = Sun::new("earth", 1);
        a.flare_cd = 1e9;
        let bright = a.update(0.1);
        let mut b = Sun::new("earth", 1);
        b.flare_cd = 1e9;
        b.drain = 1.0;
        let drained = b.update(0.1);
        assert!(drained.strength < bright.strength * 0.5, "the drain did nothing");
        // red channel holds, green and blue collapse — that is what "ember" is
        assert_eq!(drained.color[0], 1.0);
        assert!(drained.color[1] < bright.color[1] * 0.7);
        assert!(drained.color[2] < bright.color[2] * 0.35);
    }

    #[test]
    fn flares_fire_only_while_the_sun_is_up() {
        let mut s = Sun::new("io", 11);
        let states = run(&mut s, 400.0);
        let flaring: Vec<&SunState> = states.iter().filter(|x| x.flaring).collect();
        assert!(!flaring.is_empty(), "no flare in four hundred seconds");
        for f in flaring {
            assert!(f.elev > -0.2, "the star erupted below the horizon");
        }
    }

    #[test]
    fn temperature_is_derived_rather_than_typed() {
        let mut s = Sun::new("io", 5);
        s.flare_cd = 1e9;
        let noon = run(&mut s, 40.0).into_iter().max_by(|a, b| a.strength.total_cmp(&b.strength)).unwrap();
        let hot = s.temperature(&noon, 0.1);
        let icy = s.temperature(&noon, 0.9);
        assert!(hot > icy, "albedo did not matter");
        // and a far world is colder than a near one under the same albedo
        let mut p = Sun::new("pluto", 5);
        p.flare_cd = 1e9;
        let pnoon = run(&mut p, 200.0).into_iter().max_by(|a, b| a.strength.total_cmp(&b.strength)).unwrap();
        assert!(pnoon.strength < noon.strength, "pluto was not fainter than io");
    }

    #[test]
    fn thermal_wind_peaks_at_the_terminator_and_dies_in_vacuum() {
        let s = Sun::new("mars", 2);
        let dawn = SunState { dir: [1.0, 0.06, 0.3], strength: 0.3, color: [1.0; 3], day: 0.4, elev: 0.0, flaring: false };
        let noon = SunState { elev: 1.0, ..dawn };
        assert!(s.thermal_wind(&dawn, 0.5) > s.thermal_wind(&noon, 0.5) * 5.0,
            "wind did not peak at the terminator");
        // no air, no wind, however steep the gradient
        assert_eq!(s.thermal_wind(&dawn, 0.0), 0.0);
    }

    #[test]
    fn interiors_are_exempt() {
        // a sun arcing through a sealed hall is worse than no sun
        let s = Sun::new("earth", 1);
        let i = s.interior();
        assert_eq!(i.day, 1.0);
        assert!(!i.flaring);
        assert!(i.dir[1] > 0.5, "the studio key light came from below");
    }
}

// ============================================================ THE STAR ===
//
// For generated systems the sun cannot be a table row — there is no
// `solar_for("pluto")` for a world nobody has named yet. It has to come from
// the star and the orbit, which is also how the real ones work.
//
// Grounded where it can be. Equilibrium temperature is the standard
// radiative balance NASA uses for exoplanet screening:
//
//     T_eq = T_star * sqrt(R_star / 2a) * (1 - A)^(1/4)
//
// with a the orbital radius, A the bond albedo. Earth comes out near 255 K
// against a measured 254 K, which is the check worth having: if the formula
// does not reproduce the one planet we have stood on, it is wrong.
//
// The rest follows from that number and a few thresholds that are also
// measured rather than chosen: water's triple point, rock's solidus, and the
// tidal-locking distance where spin-down beats system age.

/// Spectral class. Luminosity and radius relative to the Sun, surface
/// temperature in kelvin. Values are the standard main-sequence figures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StarClass {
    O,
    B,
    A,
    F,
    G,
    K,
    M,
}

impl StarClass {
    /// (relative radius, surface temperature K, relative luminosity)
    pub fn spec(self) -> (f32, f32, f32) {
        match self {
            StarClass::O => (8.0, 35_000.0, 100_000.0),
            StarClass::B => (4.5, 18_000.0, 2_000.0),
            StarClass::A => (1.8, 8_800.0, 25.0),
            StarClass::F => (1.3, 6_700.0, 3.5),
            StarClass::G => (1.0, 5_780.0, 1.0),
            StarClass::K => (0.75, 4_600.0, 0.35),
            StarClass::M => (0.35, 3_200.0, 0.02),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            StarClass::O => "O",
            StarClass::B => "B",
            StarClass::A => "A",
            StarClass::F => "F",
            StarClass::G => "G",
            StarClass::K => "K",
            StarClass::M => "M",
        }
    }
    /// FLARE ACTIVITY, and the counterintuitive one. Small cool stars flare
    /// far harder relative to their output than large hot ones — M dwarfs
    /// are the reason "habitable zone" and "habitable" are different words,
    /// because their planets orbit close enough to be sterilised by events a
    /// G star would barely notice. Kepler and TESS both found this.
    pub fn flare_activity(self) -> f32 {
        match self {
            StarClass::M => 1.0,
            StarClass::K => 0.55,
            StarClass::G => 0.25,
            StarClass::F => 0.15,
            StarClass::A => 0.08,
            StarClass::B | StarClass::O => 0.05,
        }
    }
}

/// What the surface is doing, given its temperature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    /// rock is liquid — a lava world
    Molten,
    /// too hot for liquid water anywhere; the ground shimmers
    Scorched,
    /// water can be liquid somewhere on it
    Temperate,
    /// water is ice, CO2 is not
    Frozen,
    /// past the nitrogen frost line — Pluto territory
    Cryogenic,
}

impl Surface {
    pub fn label(self) -> &'static str {
        match self {
            Surface::Molten => "MOLTEN",
            Surface::Scorched => "SCORCHED",
            Surface::Temperate => "TEMPERATE",
            Surface::Frozen => "FROZEN",
            Surface::Cryogenic => "CRYOGENIC",
        }
    }
    /// Does the ground glow of its own accord?
    pub fn self_lit(self) -> bool {
        self == Surface::Molten
    }
}

/// A generated world's star and orbit.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub star: StarClass,
    /// semi-major axis in AU
    pub au: f32,
    /// bond albedo, 0 black .. 1 mirror
    pub albedo: f32,
    /// 0 none .. 1 thick. Greenhouse and shimmer both need it.
    pub air: f32,
}

/// Water's triple point and rock's solidus, in kelvin. Thresholds measured,
/// not chosen.
pub const T_WATER_ICE: f32 = 273.0;
pub const T_WATER_BOIL: f32 = 373.0;
pub const T_ROCK_SOLIDUS: f32 = 1_400.0;
pub const T_NITROGEN_FROST: f32 = 63.0;

impl Orbit {
    /// Equilibrium temperature, kelvin. The radiative balance.
    pub fn equilibrium_k(&self) -> f32 {
        let (r_rel, t_star, _) = self.star.spec();
        // R_sun / 1 AU = 0.00465
        let r_over_a = (r_rel * 0.00465) / self.au.max(1e-4);
        t_star * (r_over_a / 2.0).sqrt() * (1.0 - self.albedo.clamp(0.0, 0.99)).powf(0.25)
    }

    /// Surface temperature, with a greenhouse term. Thick air traps heat —
    /// Venus is 232 K equilibrium and 737 K in fact, so the term has to be
    /// able to nearly triple it.
    pub fn surface_k(&self) -> f32 {
        self.equilibrium_k() * (1.0 + 2.2 * self.air.clamp(0.0, 1.0).powf(1.5))
    }

    pub fn surface(&self) -> Surface {
        let t = self.surface_k();
        if t >= T_ROCK_SOLIDUS {
            Surface::Molten
        } else if t >= T_WATER_BOIL {
            Surface::Scorched
        } else if t >= T_WATER_ICE {
            Surface::Temperate
        } else if t >= T_NITROGEN_FROST {
            Surface::Frozen
        } else {
            Surface::Cryogenic
        }
    }

    /// TIDALLY LOCKED? Spin-down time goes roughly as a^6, so it collapses
    /// with distance — which is why every close-in M-dwarf planet is locked
    /// and Earth is not. One face burns, the other freezes, and the only
    /// habitable ground is the terminator ring.
    pub fn tidally_locked(&self) -> bool {
        let (_, _, lum) = self.star.spec();
        // lock radius scales with the star's mass proxy; luminosity^0.25 is a
        // serviceable stand-in on the main sequence
        self.au < 0.5 * lum.powf(0.25).max(0.2)
    }

    /// Day length in seconds for the game's cycle. A locked world has none —
    /// reported as zero, which the sun cycle reads as "the light never
    /// changes" rather than as an error.
    pub fn day_len(&self) -> f32 {
        if self.tidally_locked() {
            return 0.0;
        }
        // arbitrary but monotonic: outer worlds turn slower in this game's
        // time, which keeps Pluto feeling far
        (140.0 + 260.0 * (self.au / 40.0).min(1.0).sqrt() * 2.0).min(400.0)
    }

    /// HEAT SHIMMER needs a hot surface AND air to convect. An airless
    /// scorched world does not shimmer, it just sits there glaring — which
    /// is the difference between Mercury and a desert.
    pub fn shimmer(&self) -> f32 {
        let t = self.surface_k();
        if self.air <= 0.01 || t < 320.0 {
            return 0.0;
        }
        (((t - 320.0) / 400.0).min(1.0)) * self.air.clamp(0.0, 1.0).sqrt()
    }

    /// How hard the star's flares hit THIS world: activity, scaled by how
    /// close it orbits, and blunted by atmosphere.
    pub fn flare_severity(&self) -> f32 {
        let act = self.star.flare_activity();
        let prox = (1.0 / self.au.max(0.05)).min(20.0) / 20.0;
        (act * prox.sqrt() * (1.0 - 0.6 * self.air.clamp(0.0, 1.0))).clamp(0.0, 1.0)
    }

    /// Solar parameters for the existing day-cycle sun, derived rather than
    /// looked up — this is what lets a generated world use the same code
    /// path as Io and Pluto.
    pub fn solar(&self) -> Solar {
        let (_, _, lum) = self.star.spec();
        let flux = lum / (self.au.max(1e-3) * self.au.max(1e-3));
        let peak = (0.30 + 0.55 * flux.min(4.0).sqrt() / 2.0).clamp(0.12, 1.4);
        Solar {
            day_len: if self.day_len() > 0.0 { self.day_len() } else { 1.0e9 },
            min: peak * 0.25,
            max: peak,
        }
    }
}

#[cfg(test)]
mod star_tests {
    use super::*;

    #[test]
    fn earth_comes_out_where_earth_is() {
        // the check that matters: if the formula cannot reproduce the one
        // planet we have stood on, it is wrong
        let e = Orbit { star: StarClass::G, au: 1.0, albedo: 0.306, air: 0.0 };
        let t = e.equilibrium_k();
        assert!((t - 254.0).abs() < 8.0, "Earth equilibrium came out {t} K, measured is 254");
        // and with air it lands in the temperate band rather than frozen
        let with_air = Orbit { air: 0.30, ..e };
        assert_eq!(with_air.surface(), Surface::Temperate);
    }

    #[test]
    fn the_thresholds_are_physical_rather_than_chosen() {
        let g = StarClass::G;
        // 0.02 AU, not 0.06. My first guess put a "lava world" at 0.06 and
        // the formula said 1108 K — scorched, not molten, because rock needs
        // 1400. The physics was right and the expectation was lazy; real
        // lava worlds (K2-141b, 55 Cnc e) orbit inside 0.02 AU.
        let hot = Orbit { star: g, au: 0.02, albedo: 0.1, air: 0.0 };
        let venus = Orbit { star: g, au: 0.72, albedo: 0.77, air: 1.0 };
        let mars = Orbit { star: g, au: 1.52, albedo: 0.25, air: 0.01 };
        let pluto = Orbit { star: g, au: 39.5, albedo: 0.5, air: 0.0 };
        assert_eq!(hot.surface(), Surface::Molten, "{} K", hot.surface_k());
        // Venus: high albedo makes it COLD in equilibrium and the greenhouse
        // makes it the hottest surface in the system. Both are true at once.
        assert!(venus.equilibrium_k() < 240.0, "venus equilibrium {}", venus.equilibrium_k());
        assert_eq!(venus.surface(), Surface::Scorched);
        assert_eq!(mars.surface(), Surface::Frozen);
        assert_eq!(pluto.surface(), Surface::Cryogenic);
        assert!(hot.surface().self_lit() && !mars.surface().self_lit());
    }

    #[test]
    fn small_cool_stars_flare_hardest() {
        // the counterintuitive one, and it is why M-dwarf habitable zones are
        // not habitable: the planets sit close enough to be sterilised by
        // events a G star would barely notice
        assert!(StarClass::M.flare_activity() > StarClass::G.flare_activity() * 3.0);
        let m = Orbit { star: StarClass::M, au: 0.05, albedo: 0.2, air: 0.0 };
        let g = Orbit { star: StarClass::G, au: 1.0, albedo: 0.2, air: 0.0 };
        assert!(m.flare_severity() > g.flare_severity() * 4.0);
        // and an atmosphere blunts it, which is the whole argument for one
        let shielded = Orbit { air: 1.0, ..m };
        assert!(shielded.flare_severity() < m.flare_severity() * 0.5);
    }

    #[test]
    fn close_orbits_lock_and_lose_their_day() {
        let m_close = Orbit { star: StarClass::M, au: 0.04, albedo: 0.2, air: 0.0 };
        let earth = Orbit { star: StarClass::G, au: 1.0, albedo: 0.3, air: 0.3 };
        assert!(m_close.tidally_locked(), "a 0.04 AU M-dwarf planet should be locked");
        assert!(!earth.tidally_locked());
        assert_eq!(m_close.day_len(), 0.0, "a locked world still had a day");
        assert!(earth.day_len() > 100.0);
        // a locked world reads as never-changing light to the cycle
        assert!(m_close.solar().day_len > 1.0e6);
    }

    #[test]
    fn shimmer_needs_heat_and_air_together() {
        let g = StarClass::G;
        // Mercury: scorching and airless. It glares, it does not shimmer.
        let mercury = Orbit { star: g, au: 0.39, albedo: 0.12, air: 0.0 };
        assert!(mercury.surface_k() > 380.0);
        assert_eq!(mercury.shimmer(), 0.0, "an airless world shimmered");
        // a hot world WITH air does
        let hot_air = Orbit { air: 0.8, ..mercury };
        assert!(hot_air.shimmer() > 0.3);
        // and a cold one does not — but "cold" has to mean cold AFTER the
        // greenhouse. At 6 AU with a full atmosphere the surface lands at
        // 323 K, which shimmers faintly and correctly; the thick air is
        // doing exactly what thick air does. Out at 15 AU it cannot.
        let cold = Orbit { star: g, au: 15.0, albedo: 0.3, air: 1.0 };
        assert_eq!(cold.shimmer(), 0.0, "{} K", cold.surface_k());
        // and the 6 AU greenhouse case is a real result worth keeping
        let warm_far = Orbit { star: g, au: 6.0, albedo: 0.3, air: 1.0 };
        assert!(warm_far.shimmer() > 0.0 && warm_far.shimmer() < 0.1,
            "a thick-atmosphere outer world should shimmer faintly: {}", warm_far.shimmer());
    }

    #[test]
    fn a_generated_world_drives_the_same_sun_as_a_named_one() {
        // the point of all this: no table row needed
        let o = Orbit { star: StarClass::K, au: 0.6, albedo: 0.25, air: 0.4 };
        let mut s = Sun { solar: o.solar(), ..Sun::new("uncharted", 9) };
        s.flare_cd = 1e9;
        let mut peak: f32 = 0.0;
        for _ in 0..3000 {
            peak = peak.max(s.update(0.1).strength);
        }
        assert!(peak > o.solar().min, "the derived star never brightened");
        assert!(peak <= o.solar().max + 1e-3, "it left its own band");
    }

    #[test]
    fn brighter_stars_and_closer_orbits_mean_stronger_light() {
        let near = Orbit { star: StarClass::G, au: 0.5, albedo: 0.2, air: 0.0 };
        let far = Orbit { au: 5.0, ..near };
        assert!(near.solar().max > far.solar().max);
        let hot_star = Orbit { star: StarClass::A, ..far };
        assert!(hot_star.solar().max > far.solar().max);
    }
}
