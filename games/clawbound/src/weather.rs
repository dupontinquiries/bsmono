//! weather.rs — a living wind + weather + season system, shared across modes.
//!
//! Warriors-flavoured: four seasons (Newleaf … Leaf-bare) set how much prey is about
//! and which skies roll in, and scent rides the wind — a trail drifts downwind and a
//! gusty day scatters it, so cats hunt into the wind and the hunter noses along it.
//! SMITE-2-flavoured: the sky shifts *during* a match (clear → rain → fog → storm),
//! changing sightlines and the value of cover from one teamfight to the next.
//!
//! Pure and deterministic (a small hash drives gusts and sky rolls) so it unit-tests
//! and the sim, prowl and the renderer can all share one source of truth.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Season { Newleaf, Greenleaf, LeafFall, LeafBare }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sky { Clear, Overcast, Rain, Fog, Storm, Snow }

impl Season {
    pub fn name(self) -> &'static str {
        match self { Season::Newleaf => "Newleaf", Season::Greenleaf => "Greenleaf", Season::LeafFall => "Leaf-fall", Season::LeafBare => "Leaf-bare" }
    }
    /// The skies that can roll in this season, as (Sky, weight) pairs.
    fn sky_table(self) -> &'static [(Sky, f32)] {
        match self {
            Season::Newleaf   => &[(Sky::Clear, 3.0), (Sky::Overcast, 2.0), (Sky::Rain, 2.5), (Sky::Fog, 1.0)],
            Season::Greenleaf => &[(Sky::Clear, 5.0), (Sky::Overcast, 1.5), (Sky::Rain, 1.0), (Sky::Storm, 1.2)],
            Season::LeafFall  => &[(Sky::Clear, 2.0), (Sky::Overcast, 2.5), (Sky::Rain, 2.5), (Sky::Fog, 2.0), (Sky::Storm, 1.0)],
            Season::LeafBare  => &[(Sky::Overcast, 2.5), (Sky::Snow, 3.0), (Sky::Fog, 2.0), (Sky::Clear, 1.5)],
        }
    }
    /// Baseline prey abundance for the season (greenleaf is plenty, leaf-bare is lean).
    pub fn prey_base(self) -> f32 {
        match self { Season::Newleaf => 1.05, Season::Greenleaf => 1.3, Season::LeafFall => 0.9, Season::LeafBare => 0.5 }
    }
}

impl Sky {
    pub fn name(self) -> &'static str {
        match self { Sky::Clear => "clear", Sky::Overcast => "overcast", Sky::Rain => "rain", Sky::Fog => "fog", Sky::Storm => "storm", Sky::Snow => "snow" }
    }
    pub fn is_wet(self) -> bool { matches!(self, Sky::Rain | Sky::Storm) }
    /// Base wind strength this sky tends toward (0..1).
    fn wind_base(self) -> f32 {
        match self { Sky::Clear => 0.25, Sky::Overcast => 0.4, Sky::Rain => 0.5, Sky::Fog => 0.12, Sky::Storm => 0.9, Sky::Snow => 0.35 }
    }
}

fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}

#[derive(Clone, Copy)]
pub struct Weather {
    pub season: Season,
    pub sky: Sky,
    pub wind_dir: f32,       // heading (radians) the wind blows *toward*
    pub wind_strength: f32,  // 0..1 sustained
    pub gust: f32,           // 0..1 gust riding on top
    pub wetness: f32,        // 0..1 ground wetness (rain soaks, then dries)
    pub time: f32,
    tick: u32,
    sky_t: f32,              // seconds until the next sky roll
}

impl Weather {
    pub fn new(season: Season) -> Weather {
        let sky = Self::roll_sky(season, 1);
        Weather {
            season, sky,
            wind_dir: 0.7, wind_strength: sky.wind_base(), gust: 0.0,
            wetness: if sky.is_wet() { 0.5 } else { 0.0 },
            time: 0.0, tick: 0, sky_t: 24.0,
        }
    }

    fn roll_sky(season: Season, salt: u32) -> Sky {
        let table = season.sky_table();
        let total: f32 = table.iter().map(|(_, w)| *w).sum();
        let mut r = hashf(salt.wrapping_mul(2_654_435_761) ^ 0xBEEF) * total;
        for &(s, w) in table {
            if r < w { return s; }
            r -= w;
        }
        table[0].0
    }

    pub fn step(&mut self, dt: f32) {
        self.time += dt;
        self.tick = self.tick.wrapping_add(1);

        // wind slowly veers, with a gust layered on a couple of noise octaves
        let n = hashf(self.tick ^ 0x51D0) - 0.5;
        self.wind_dir += n * 0.25 * dt;
        let g = 0.5 + 0.5 * (self.time * 0.9).sin() * (self.time * 0.37 + 1.3).cos();
        self.gust += (g.clamp(0.0, 1.0) - self.gust) * (dt * 1.5).min(1.0);

        // wind strength eases toward this sky's baseline
        let target = self.sky.wind_base();
        self.wind_strength += (target - self.wind_strength) * (dt * 0.4).min(1.0);

        // ground wetness: wet skies soak it, otherwise it dries
        if self.sky.is_wet() { self.wetness = (self.wetness + dt * 0.15).min(1.0); }
        else { self.wetness = (self.wetness - dt * 0.05).max(0.0); }

        // roll a fresh sky every so often (a match's weather turns over)
        self.sky_t -= dt;
        if self.sky_t <= 0.0 {
            self.sky = Self::roll_sky(self.season, self.tick.wrapping_add(7));
            self.sky_t = 18.0 + hashf(self.tick ^ 0x9E3D) * 22.0;
        }
    }

    /// Total wind strength including the current gust (0..~1.4).
    pub fn total_wind(&self) -> f32 { (self.wind_strength + self.gust * 0.45).clamp(0.0, 1.4) }

    /// Wind as a world vector (points the way scent drifts).
    pub fn wind_vec(&self) -> (f32, f32) {
        let s = self.total_wind();
        (self.wind_dir.sin() * s, self.wind_dir.cos() * s)
    }
    /// Unit wind direction (for drifting scent along, independent of strength).
    pub fn wind_dir_vec(&self) -> (f32, f32) { (self.wind_dir.sin(), self.wind_dir.cos()) }

    // ── gameplay modifiers ───────────────────────────────────────────────────

    /// How fast a scent trail fades. Rain and storm wash it away; fog and snow
    /// preserve it (Warriors: tracks show plainly in snow); wind disperses it.
    pub fn scent_decay_mult(&self) -> f32 {
        let base = match self.sky {
            Sky::Clear => 1.0, Sky::Overcast => 0.95, Sky::Rain => 1.7,
            Sky::Storm => 2.2, Sky::Fog => 0.7, Sky::Snow => 0.6,
        };
        base * (1.0 + self.total_wind() * 0.6)
    }

    /// How fast scent slides downwind — scales with wind.
    pub fn scent_drift_mult(&self) -> f32 { 0.4 + self.total_wind() * 2.2 }

    /// Sight multiplier for the hunter — fog and storm blind it, snow dims it.
    pub fn sight_mult(&self) -> f32 {
        match self.sky {
            Sky::Clear => 1.0, Sky::Overcast => 0.9, Sky::Rain => 0.72,
            Sky::Storm => 0.55, Sky::Fog => 0.42, Sky::Snow => 0.75,
        }
    }

    /// Prey about right now — season baseline, thinned when weather drives it to shelter.
    pub fn prey_abundance(&self) -> f32 {
        let shelter = if self.sky.is_wet() || self.sky == Sky::Fog { 0.8 } else { 1.0 };
        self.season.prey_base() * shelter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seasons_scale_prey() {
        assert!(Season::Greenleaf.prey_base() > Season::LeafBare.prey_base());
        assert!(Weather::new(Season::Greenleaf).prey_abundance() > Weather::new(Season::LeafBare).prey_abundance());
    }

    #[test]
    fn rain_washes_scent_fog_holds_it() {
        let mut clear = Weather::new(Season::Greenleaf); clear.sky = Sky::Clear; clear.wind_strength = 0.2; clear.gust = 0.0;
        let mut rain = clear; rain.sky = Sky::Rain;
        let mut fog = clear; fog.sky = Sky::Fog;
        assert!(rain.scent_decay_mult() > clear.scent_decay_mult(), "rain should wash scent away faster");
        assert!(fog.scent_decay_mult() < clear.scent_decay_mult(), "fog should let scent linger");
    }

    #[test]
    fn fog_and_storm_blind_the_hunter() {
        let mut w = Weather::new(Season::LeafFall);
        w.sky = Sky::Clear; let clear = w.sight_mult();
        w.sky = Sky::Fog; assert!(w.sight_mult() < clear * 0.6, "fog should badly cut sight");
        w.sky = Sky::Storm; assert!(w.sight_mult() < clear, "a storm should cut sight too");
    }

    #[test]
    fn wind_carries_and_scatters_scent() {
        let mut calm = Weather::new(Season::Greenleaf); calm.sky = Sky::Fog; calm.wind_strength = 0.1; calm.gust = 0.0;
        let mut gale = calm; gale.wind_strength = 1.0; gale.gust = 0.4;
        assert!(gale.scent_drift_mult() > calm.scent_drift_mult(), "wind should drive scent downwind faster");
        let (gx, gz) = gale.wind_vec();
        assert!((gx * gx + gz * gz).sqrt() > 0.5, "a gale has a strong wind vector");
    }

    #[test]
    fn weather_evolves_and_stays_bounded() {
        let mut w = Weather::new(Season::LeafFall);
        let mut skies = std::collections::HashSet::new();
        for _ in 0..(200 * 30) {
            w.step(1.0 / 30.0);
            skies.insert(w.sky.name());
            assert!(w.wind_strength >= 0.0 && w.total_wind() <= 1.5);
            assert!(w.wetness >= 0.0 && w.wetness <= 1.0);
            assert!(w.wind_dir.is_finite());
        }
        assert!(skies.len() >= 2, "over 200s the sky should change at least once");
    }
}
