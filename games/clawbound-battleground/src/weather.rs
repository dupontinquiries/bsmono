// Dynamic weather — a headless state machine owned by the sim (deterministic
// through the game rng, so tests and the headless harness see the same sky
// the player does). The renderer never reads `Weather` directly; it reads an
// `Atmosphere` sample, which is plain numbers. Map events can force a state
// (Sweetrain → Rain, Thunder Front → Storm, the Long Winter → Snow, the Long
// Night → night), otherwise the sky drifts on its own every 70–140 s.
//
// Gameplay reach is deliberately narrow and visible: Fog shortens AI sight
// (`vision_mul`), nothing else. Everything with teeth lives in map_events.rs.

use crate::sim::Rng;
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WeatherKind {
    Clear,
    Overcast,
    Rain,
    Storm,
    Fog,
    Snow,
}

impl WeatherKind {
    pub fn name(self) -> &'static str {
        match self {
            WeatherKind::Clear => "Clear",
            WeatherKind::Overcast => "Overcast",
            WeatherKind::Rain => "Rain",
            WeatherKind::Storm => "Storm",
            WeatherKind::Fog => "Fog",
            WeatherKind::Snow => "Snow",
        }
    }
}

pub const ALL_KINDS: [WeatherKind; 6] = [
    WeatherKind::Clear,
    WeatherKind::Overcast,
    WeatherKind::Rain,
    WeatherKind::Storm,
    WeatherKind::Fog,
    WeatherKind::Snow,
];

/// What the sky is doing right now, as numbers the renderer/VFX consume.
#[derive(Clone, Copy, Debug)]
pub struct Atmosphere {
    pub sun_dir: Vec3,
    /// Linear RGB sun colour × intensity (1.0 = the golden-hour baseline).
    pub sun_col: [f32; 3],
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    /// 0 = clear sky, 1 = solid overcast.
    pub cloud_cover: f32,
    /// Precipitation strengths (0..1). At most one is non-zero in a steady state.
    pub rain: f32,
    pub snow: f32,
    /// Ground wetness (0..1) — builds during rain, dries slowly. Drives specular.
    pub wetness: f32,
    /// Lightning flash (0..1, decays over ~0.3 s).
    pub lightning: f32,
    /// 0 = day, 1 = the Long Night.
    pub night: f32,
    /// Horizontal wind (world units/s) — leans rain, drifts snow and leaves.
    pub wind: Vec3,
    /// A gust front, 0..1: rises fast, falls slow. Bends the canopy, howls.
    pub gust: f32,
}

#[derive(Clone, Copy)]
struct Target {
    sun_mul: [f32; 3],
    fog_color: [f32; 3],
    fog_density: f32,
    cloud: f32,
    rain: f32,
    snow: f32,
    wind: f32,
}

fn target(kind: WeatherKind) -> Target {
    match kind {
        WeatherKind::Clear => Target { sun_mul: [1.0, 1.0, 1.0], fog_color: [0.58, 0.47, 0.33], fog_density: 0.0016, cloud: 0.12, rain: 0.0, snow: 0.0, wind: 1.0 },
        WeatherKind::Overcast => Target { sun_mul: [0.62, 0.64, 0.70], fog_color: [0.50, 0.50, 0.52], fog_density: 0.0024, cloud: 0.85, rain: 0.0, snow: 0.0, wind: 2.5 },
        WeatherKind::Rain => Target { sun_mul: [0.46, 0.50, 0.58], fog_color: [0.40, 0.43, 0.48], fog_density: 0.0034, cloud: 0.95, rain: 0.8, snow: 0.0, wind: 4.0 },
        WeatherKind::Storm => Target { sun_mul: [0.30, 0.33, 0.42], fog_color: [0.28, 0.30, 0.36], fog_density: 0.0042, cloud: 1.0, rain: 1.0, snow: 0.0, wind: 7.5 },
        WeatherKind::Fog => Target { sun_mul: [0.70, 0.70, 0.72], fog_color: [0.66, 0.66, 0.66], fog_density: 0.0110, cloud: 0.55, rain: 0.0, snow: 0.0, wind: 0.6 },
        WeatherKind::Snow => Target { sun_mul: [0.72, 0.76, 0.86], fog_color: [0.70, 0.74, 0.80], fog_density: 0.0050, cloud: 0.80, rain: 0.0, snow: 0.9, wind: 2.0 },
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

#[derive(Clone, Copy, Debug)]
pub enum WeatherEvent {
    Changed(WeatherKind),
    /// A bolt from the sky — VFX only; damage belongs to the Thunder Front map event.
    Lightning { pos: Vec3 },
}

#[derive(Clone, Debug)]
pub struct Weather {
    pub kind: WeatherKind,
    pub prev: WeatherKind,
    /// 0 → fully `prev`, 1 → fully `kind`.
    pub blend: f32,
    pub next_change_t: f32,
    /// Set by a map event: hold this kind until `forced_until`.
    pub forced_until: f32,
    pub wetness: f32,
    pub lightning: f32,
    strike_t: f32,
    pub night: f32,
    pub night_target: f32,
    pub wind_phase: f32,
    /// Sim clock, for the slow daily arc of the sun.
    pub clock: f32,
    /// Gust state: current strength, target, and when the next front arrives.
    pub gust: f32,
    gust_target: f32,
    next_gust_t: f32,
    /// Slowly wandering wind heading (radians), so gusts don't always come
    /// from the same quarter.
    pub wind_heading: f32,
}

pub const BLEND_SECS: f32 = 8.0;

/// What the sky drifts between, as a bag to draw from: a kind listed twice
/// is twice as likely. The jungle's is every kind once. The pass is cold and
/// grey: mostly overcast, fog and snow, clear now and then, rain rarely.
pub fn climate() -> &'static [WeatherKind] {
    use WeatherKind::*;
    match crate::map::active() {
        crate::map::MapId::Jungle => &ALL_KINDS,
        crate::map::MapId::Pass => &[Overcast, Overcast, Overcast, Fog, Fog, Fog, Snow, Snow, Clear, Rain],
        // above the cloud sea: mostly clear starlight, a drift of snow, a
        // spirit-mist now and then; no rain up here
        crate::map::MapId::Sky => &[Clear, Clear, Clear, Clear, Snow, Snow, Fog],
        // under the sea there is no weather: the light from above only
        // clears and dims as the surface far overhead does
        crate::map::MapId::Atlantis => &[Clear, Clear, Overcast],
        // an old labyrinth in a hollow of the hills: grey and misty more
        // often than not, and rain; no snow
        crate::map::MapId::Maze => &[Overcast, Overcast, Fog, Fog, Clear, Rain],
    }
}

/// How dark the map is when nothing else is happening: the sky map's night
/// never ends (0.6: moonlit, stars to the horizon, still readable).
/// Events that darken the sky raise it; ending them returns here.
pub fn base_night() -> f32 {
    match crate::map::active() {
        crate::map::MapId::Sky => 0.6,
        _ => 0.0,
    }
}

/// How a match opens.
pub fn opening() -> WeatherKind {
    match crate::map::active() {
        crate::map::MapId::Jungle => WeatherKind::Clear,
        crate::map::MapId::Pass => WeatherKind::Overcast,
        crate::map::MapId::Sky => WeatherKind::Clear,
        crate::map::MapId::Atlantis => WeatherKind::Clear,
        crate::map::MapId::Maze => WeatherKind::Overcast,
    }
}

impl Weather {
    pub fn new() -> Weather {
        Weather {
            kind: opening(),
            prev: opening(),
            blend: 1.0,
            next_change_t: 75.0,
            forced_until: 0.0,
            wetness: 0.0,
            lightning: 0.0,
            strike_t: 4.0,
            night: base_night(),
            night_target: base_night(),
            wind_phase: 0.0,
            clock: 0.0,
            gust: 0.0,
            gust_target: 0.0,
            next_gust_t: 20.0,
            wind_heading: 0.53,
        }
    }

    /// Switch weather (with a crossfade). No-op if already there.
    pub fn set(&mut self, kind: WeatherKind) {
        if kind == self.kind {
            return;
        }
        self.prev = self.kind;
        self.kind = kind;
        self.blend = 0.0;
    }

    /// Hold `kind` until sim time `until` (map events use this).
    pub fn force(&mut self, kind: WeatherKind, until: f32) {
        self.set(kind);
        self.forced_until = until;
    }

    /// One sim step. `t` is the sim clock.
    pub fn tick(&mut self, dt: f32, t: f32, rng: &mut Rng, out: &mut Vec<WeatherEvent>) {
        self.blend = (self.blend + dt / BLEND_SECS).min(1.0);
        self.wind_phase += dt;
        self.clock = t;

        // --- gust fronts: a sharp rise (~1.5 s), a ragged plateau, a slow
        // fall (~6 s). Frequency and size scale with the weather's wind.
        let base_wind = {
            let a = target(self.prev);
            let b = target(self.kind);
            lerp(a.wind, b.wind, smooth(self.blend))
        };
        if t >= self.next_gust_t {
            let windy = ((base_wind - 1.0) / 6.5).clamp(0.0, 1.0);
            self.gust_target = rng.range(0.45, 1.0) * (0.55 + 0.45 * windy);
            self.next_gust_t = t + rng.range(6.0, 22.0) * (1.25 - windy * 0.8);
            self.wind_heading += rng.range(-0.5, 0.5);
        }
        if self.gust < self.gust_target {
            self.gust = (self.gust + dt / 1.5 * self.gust_target).min(self.gust_target);
            if (self.gust_target - self.gust).abs() < 0.02 {
                self.gust_target = 0.0; // crest reached: start the fall
            }
        } else {
            self.gust = (self.gust - dt / 6.0).max(0.0);
        }

        // natural drift, unless a map event holds the sky
        if t >= self.next_change_t && t >= self.forced_until {
            let mut next = *rng.pick(climate());
            // storms are rare on their own; fog and snow only occasionally
            if matches!(next, WeatherKind::Storm) && rng.f32() < 0.6 {
                next = WeatherKind::Rain;
            }
            if next == self.kind {
                next = if self.kind == WeatherKind::Clear { WeatherKind::Overcast } else { WeatherKind::Clear };
            }
            self.set(next);
            self.next_change_t = t + rng.range(70.0, 140.0);
            out.push(WeatherEvent::Changed(next));
        }
        // a forced spell ends: let the sky drift back on its own soon after
        if self.forced_until > 0.0 && t >= self.forced_until {
            self.forced_until = 0.0;
            self.next_change_t = t + rng.range(6.0, 14.0);
        }

        // wetness: builds under rain, dries slowly (snow only dampens)
        let a = self.atmosphere();
        let gain = a.rain * 0.08 + a.snow * 0.02;
        let dry = 0.012 * (1.0 - a.cloud_cover * 0.6);
        self.wetness = (self.wetness + (gain - dry) * dt).clamp(0.0, 1.0);

        // lightning during storms
        self.lightning *= (-dt * 6.0).exp();
        if a.rain > 0.9 && self.kind == WeatherKind::Storm {
            self.strike_t -= dt;
            if self.strike_t <= 0.0 {
                self.strike_t = rng.range(2.5, 9.0);
                self.lightning = 1.0;
                let ang = rng.f32() * std::f32::consts::TAU;
                let r = rng.range(40.0, 160.0);
                out.push(WeatherEvent::Lightning { pos: Vec3::new(ang.cos() * r, 0.0, ang.sin() * r) });
            }
        }

        // night crossfade (driven by the Long Night map event)
        let d = self.night_target - self.night;
        self.night += d.signum() * (dt / 6.0).min(d.abs());
    }

    /// AI sight multiplier: fog closes the jungle in.
    pub fn vision_mul(&self) -> f32 {
        let a = self.atmosphere();
        let fogged = ((a.fog_density - 0.0016) / (0.0110 - 0.0016)).clamp(0.0, 1.0);
        1.0 - 0.45 * fogged - 0.15 * a.night
    }

    /// Sample the blended atmosphere.
    pub fn atmosphere(&self) -> Atmosphere {
        let a = target(self.prev);
        let b = target(self.kind);
        let t = smooth(self.blend);
        let n = self.night;

        let mut sun_mul = lerp3(a.sun_mul, b.sun_mul, t);
        let mut fog_color = lerp3(a.fog_color, b.fog_color, t);
        let fog_density = lerp(a.fog_density, b.fog_density, t);
        let cloud = lerp(a.cloud, b.cloud, t);
        let rain = lerp(a.rain, b.rain, t);
        let snow = lerp(a.snow, b.snow, t);
        let wind_s = lerp(a.wind, b.wind, t);

        // night: sun sinks, goes cold and dim; fog goes indigo
        let night_sun = [0.10, 0.13, 0.26];
        sun_mul = lerp3(sun_mul, [sun_mul[0] * night_sun[0], sun_mul[1] * night_sun[1], sun_mul[2] * night_sun[2]], n);
        fog_color = lerp3(fog_color, [0.06, 0.07, 0.14], n);
        // the sun crosses the valley over ~25 minutes: azimuth swings 2 rad,
        // elevation dips toward a late-golden-hour angle as the match runs long
        let arc = (self.clock / 1500.0).min(1.0);
        let az = (-0.42f32).atan2(0.32) + arc * 2.0;
        let el = 0.82 - arc * 0.30;
        let horiz = (1.0 - el * el).max(0.05).sqrt();
        let day_dir = Vec3::new(az.sin() * horiz, el, az.cos() * horiz);
        let night_dir = Vec3::new(0.35, 0.28, -0.55);
        let sun_dir = (day_dir * (1.0 - n) + night_dir * n).normalize();

        let base_sun = [1.14, 1.00, 0.84];
        let wp = self.wind_phase;
        let breath = 0.75 + 0.25 * (wp * 0.7).sin() * (wp * 0.23 + 1.0).cos();
        let hd = self.wind_heading;
        let wind = Vec3::new(hd.cos(), 0.0, hd.sin()) * wind_s * (breath + self.gust * 2.2);
        let air = Atmosphere {
            sun_dir,
            sun_col: [base_sun[0] * sun_mul[0], base_sun[1] * sun_mul[1], base_sun[2] * sun_mul[2]],
            fog_color,
            fog_density,
            cloud_cover: cloud.max(n * 0.3),
            rain,
            snow,
            wetness: self.wetness,
            lightning: self.lightning,
            night: n,
            wind,
            gust: self.gust,
        };
        if crate::map::active() == crate::map::MapId::Atlantis {
            return under_the_sea(air, self.clock);
        }
        air
    }
}

/// The light in the Drowned Halls: the sun almost overhead (it comes
/// down through forty fathoms, so it swings only a little as the surface
/// heaves), cool blue-green; the fog is the water itself -- dense and
/// teal; no rain, snow or lightning; a surface overcast (`Overcast` in the
/// climate) only dims it.
pub fn under_the_sea(air: Atmosphere, clock: f32) -> Atmosphere {
    let sway = Vec3::new((clock * 0.07).sin() * 0.12, 0.0, (clock * 0.05).cos() * 0.12);
    let dim = 1.0 - air.cloud_cover * 0.35;
    Atmosphere {
        sun_dir: (Vec3::new(0.14, 1.0, 0.22) + sway).normalize(),
        sun_col: [0.66 * dim, 0.98 * dim, 1.02 * dim],
        fog_color: [0.03, 0.19, 0.23],
        fog_density: 0.014,
        cloud_cover: 0.0,
        rain: 0.0,
        snow: 0.0,
        wetness: 0.12,
        lightning: 0.0,
        night: 0.0,
        wind: Vec3::new(0.3, 0.0, 0.2),
        gust: 0.0,
    }
}

impl Default for Weather {
    fn default() -> Self {
        Weather::new()
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_drifts_and_blends() {
        let mut w = Weather::new();
        let mut rng = Rng::new(7);
        let mut ev = Vec::new();
        let mut changes = 0;
        let mut t = 0.0;
        for _ in 0..(600 * 30) {
            ev.clear();
            w.tick(1.0 / 30.0, t, &mut rng, &mut ev);
            t += 1.0 / 30.0;
            changes += ev.iter().filter(|e| matches!(e, WeatherEvent::Changed(_))).count();
            let a = w.atmosphere();
            assert!(a.fog_density > 0.0 && a.cloud_cover >= 0.0 && a.cloud_cover <= 1.0);
            assert!(a.rain <= 1.0 && a.snow <= 1.0 && a.wetness <= 1.0);
        }
        assert!(changes >= 3, "sky should have changed a few times in 10 min: {changes}");
    }

    #[test]
    fn forced_weather_holds_then_releases() {
        let mut w = Weather::new();
        let mut rng = Rng::new(3);
        let mut ev = Vec::new();
        w.force(WeatherKind::Storm, 40.0);
        let mut t = 0.0;
        while t < 39.0 {
            w.tick(0.1, t, &mut rng, &mut ev);
            t += 0.1;
        }
        assert_eq!(w.kind, WeatherKind::Storm);
        assert!(w.wetness > 0.5, "a storm should soak the ground: {}", w.wetness);
        assert!(ev.iter().any(|e| matches!(e, WeatherEvent::Lightning { .. })));
        ev.clear();
        while t < 400.0 {
            w.tick(0.1, t, &mut rng, &mut ev);
            t += 0.1;
        }
        assert!(ev.iter().any(|e| matches!(e, WeatherEvent::Changed(_))), "the sky must drift again after the hold");
    }

    #[test]
    fn gusts_come_and_go() {
        let mut w = Weather::new();
        w.set(WeatherKind::Storm);
        w.blend = 1.0;
        let mut rng = Rng::new(11);
        let mut ev = Vec::new();
        let (mut peak, mut calm) = (0.0f32, 0usize);
        let mut t = 0.0;
        for _ in 0..(120 * 30) {
            w.tick(1.0 / 30.0, t, &mut rng, &mut ev);
            t += 1.0 / 30.0;
            peak = peak.max(w.gust);
            if w.gust < 0.05 {
                calm += 1;
            }
        }
        assert!(peak > 0.6, "a storm should throw big gusts: {peak}");
        assert!(calm > 30 * 5, "gusts must fall away between fronts: {calm}");
    }

    #[test]
    fn fog_shortens_sight() {
        let mut w = Weather::new();
        w.set(WeatherKind::Fog);
        w.blend = 1.0;
        assert!(w.vision_mul() < 0.6);
        w.set(WeatherKind::Clear);
        w.blend = 1.0;
        assert!((w.vision_mul() - 1.0).abs() < 1e-4);
    }
}

#[cfg(test)]
mod climate_tests {
    use super::*;

    /// Over a long match the pass is grey most of the time and still sees
    /// fog and snow; the jungle still sees every kind.
    #[test]
    fn the_pass_is_bleak_and_the_jungle_is_not() {
        let run = |m: crate::map::MapId| {
            crate::map::with(m, || {
                let mut w = Weather::new();
                let mut rng = Rng::new(7);
                let mut out = Vec::new();
                let mut secs = std::collections::HashMap::new();
                let mut t = 0.0;
                while t < 4.0 * 3600.0 {
                    w.tick(1.0, t, &mut rng, &mut out);
                    *secs.entry(w.kind.name()).or_insert(0.0f32) += 1.0;
                    t += 1.0;
                }
                (w.kind, secs)
            })
        };
        let (_, pass) = run(crate::map::MapId::Pass);
        let total: f32 = pass.values().sum();
        let grey = ["Overcast", "Fog", "Snow"].iter().map(|k| pass.get(k).copied().unwrap_or(0.0)).sum::<f32>() / total;
        assert!(grey > 0.6, "the pass is grey {grey:.2} of the time");
        assert!(pass.get("Fog").copied().unwrap_or(0.0) / total > 0.12, "fog comes through");
        assert!(pass.get("Snow").copied().unwrap_or(0.0) / total > 0.08, "and snow");
        let (_, jungle) = run(crate::map::MapId::Jungle);
        assert!(jungle.len() >= 5, "the jungle's sky still wanders: {jungle:?}");
        assert_eq!(crate::map::with(crate::map::MapId::Pass, Weather::new).kind, WeatherKind::Overcast);
    }
}
