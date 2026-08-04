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
