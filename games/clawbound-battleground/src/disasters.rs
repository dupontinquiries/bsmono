// Catastrophes — rare, announced, match-altering nature events. At most one
// per match, rolled once at 1:30 (45 % chance), landing somewhere between
// 4:00 and 12:00 with a 12 s warning. Each hurts whoever is in the way and
// leaves the map changed:
//
//   Wildfire    a spark in the jungle (or a Tempest strike) spreads cell to
//               cell downwind through the flora, burning ~20 s per cell,
//               searing anyone inside. Rain puts it out. Burnt cells stay
//               CHARRED for the rest of the match (scene.rs darkens the
//               flora there, ash drifts, prey avoid it).
//   Tempest     the sky goes black: a storm with lightning every half-second
//               hunting the ancestors, gale-force gusts, and strikes that can
//               light fires.
//   Earthquake  25 s of tremor: the ground shakes (camera + a global slow),
//               FISSURES tear open across the map — glowing cracks that burn
//               anyone standing in them, then cool into scars that stay.
//   Flood       the rivers rise and spill: the whole floor is ankle-to-knee
//               deep (everyone wades), the channels become drowning-deep,
//               then it recedes over 20 s leaving mud.
//   Tornado     a funnel walks in from the rim and crosses the map, pulling
//               in whatever is near, hurling it, tearing up the canopy.
//
// Everything here is data on the game rng; render/vfx read `Game::disasters`.

use crate::consts::{lane_clearance, river_dist, water_depth, ARENA_RADIUS, WATER_LEVEL};
use crate::sim::{Event, Game, TargetRef};
use crate::weather::WeatherKind;
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DisasterKind {
    Wildfire,
    Tempest,
    Earthquake,
    Flood,
    Tornado,
}

pub const ALL: [DisasterKind; 5] = [DisasterKind::Wildfire, DisasterKind::Tempest, DisasterKind::Earthquake, DisasterKind::Flood, DisasterKind::Tornado];

impl DisasterKind {
    pub fn name(self) -> &'static str {
        match self {
            DisasterKind::Wildfire => "WILDFIRE",
            DisasterKind::Tempest => "TEMPEST",
            DisasterKind::Earthquake => "EARTHQUAKE",
            DisasterKind::Flood => "FLOOD",
            DisasterKind::Tornado => "TORNADO",
        }
    }
    pub fn warning(self) -> &'static str {
        match self {
            DisasterKind::Wildfire => "smoke on the wind — the jungle is going to burn",
            DisasterKind::Tempest => "the sky goes black — a tempest is coming",
            DisasterKind::Earthquake => "the birds fall silent — the ground is about to move",
            DisasterKind::Flood => "the rivers are swelling — high ground, now",
            DisasterKind::Tornado => "the wind turns — a funnel is walking in",
        }
    }
    pub fn duration(self) -> f32 {
        match self {
            DisasterKind::Wildfire => 130.0,
            DisasterKind::Tempest => 45.0,
            DisasterKind::Earthquake => 25.0,
            DisasterKind::Flood => 75.0,
            DisasterKind::Tornado => 55.0,
        }
    }
}

pub const WARNING_SECS: f32 = 12.0;
pub const FIRE_CELL: f32 = 6.0;
pub const FIRE_BURN_SECS: f32 = 20.0;
pub const FIRE_DPS_FRAC: f32 = 0.035;
pub const QUAKE_SLOW: f32 = 0.15;
pub const FISSURE_DMG: f32 = 70.0;
pub const FLOOD_RISE: f32 = 0.95;
pub const DROWN_DEPTH: f32 = 2.4;
pub const DROWN_DPS_FRAC: f32 = 0.05;
pub const TORNADO_RADIUS: f32 = 6.0;
pub const TORNADO_PULL_RADIUS: f32 = 16.0;
pub const TORNADO_DPS: f32 = 55.0;

#[derive(Clone, Copy, Debug)]
pub struct FireCell {
    pub pos: Vec3,
    /// seconds left burning
    pub burn: f32,
    pub spread_t: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Fissure {
    pub a: Vec3,
    pub b: Vec3,
    /// 1 = glowing (deadly), decays to 0 (a cold scar)
    pub heat: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Tornado {
    pub pos: Vec3,
    pub vel: Vec3,
    pub spin: f32,
}

#[derive(Clone, Debug)]
pub struct Active {
    pub kind: DisasterKind,
    pub until: f32,
    pub pulse_t: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Disasters {
    pub rolled: bool,
    /// scheduled (kind, start time); warning goes out WARNING_SECS before
    pub scheduled: Option<(DisasterKind, f32)>,
    pub warned: bool,
    pub active: Option<Active>,
    pub fire: Vec<FireCell>,
    pub charred: Vec<Vec3>,
    pub fissures: Vec<Fissure>,
    pub flood_rise: f32,
    pub tornado: Option<Tornado>,
    /// camera shake 0..1
    pub shake: f32,
    /// how many catastrophes have struck (scoreboard)
    pub struck: u32,
}

/// Water depth including a flood.
pub fn water_depth_rise(x: f32, z: f32, rise: f32) -> f32 {
    (WATER_LEVEL + rise - crate::render::terrain_height(x, z)).max(0.0)
}

fn fuel_at(x: f32, z: f32) -> bool {
    let r = (x * x + z * z).sqrt();
    r < ARENA_RADIUS - 6.0 && lane_clearance(x, z) > 4.5 && river_dist(x, z) > 7.0 && !crate::bosses::in_pit(x, z)
}

fn cell_of(p: Vec3) -> Vec3 {
    Vec3::new((p.x / FIRE_CELL).round() * FIRE_CELL, 0.0, (p.z / FIRE_CELL).round() * FIRE_CELL)
}

impl Disasters {
    pub fn is_charred(&self, p: Vec3) -> bool {
        let c = cell_of(p);
        self.charred.iter().any(|q| q.distance(c) < 0.5)
    }
    pub fn burning_at(&self, p: Vec3) -> bool {
        self.fire.iter().any(|f| f.pos.distance(Vec3::new(p.x, 0.0, p.z)) < FIRE_CELL * 0.72)
    }
    /// Global movement multiplier (quake tremor).
    pub fn speed_mul(&self) -> f32 {
        if matches!(self.active.as_ref().map(|a| a.kind), Some(DisasterKind::Earthquake)) { 1.0 - QUAKE_SLOW } else { 1.0 }
    }
    pub fn active_kind(&self) -> Option<DisasterKind> {
        self.active.as_ref().map(|a| a.kind)
    }
    /// Charred fraction near a point (for scene tinting), 0..1.
    pub fn char_near(&self, p: Vec3, radius: f32) -> f32 {
        if self.charred.is_empty() {
            return 0.0;
        }
        let n = self.charred.iter().filter(|c| c.distance(Vec3::new(p.x, 0.0, p.z)) < radius).count();
        let cells = ((radius * 2.0 / FIRE_CELL).powi(2) * 0.78).max(1.0);
        (n as f32 / cells).clamp(0.0, 1.0)
    }
}

impl Game {
    /// Water depth at a point, flood included — what movement and drowning use.
    pub fn water_depth_now(&self, x: f32, z: f32) -> f32 {
        // On a crossing log is dry, whatever is under it. Not during a flood:
        // the log is awash along with everything else.
        if self.disasters.flood_rise <= 0.001 && self.on_a_log(x, z) {
            return 0.0;
        }
        if self.disasters.flood_rise > 0.001 { water_depth_rise(x, z, self.disasters.flood_rise) } else { water_depth(x, z) }
    }

    pub fn disasters_tick(&mut self, dt: f32) {
        let now = self.t;
        // --- schedule once
        if !self.disasters.rolled && now >= 90.0 {
            self.disasters.rolled = true;
            if self.rng.f32() < 0.45 {
                let kind = *self.rng.pick(&ALL);
                let at = now + self.rng.range(150.0, 630.0);
                self.disasters.scheduled = Some((kind, at));
            }
        }
        if let Some((kind, at)) = self.disasters.scheduled {
            if !self.disasters.warned && now >= at - WARNING_SECS {
                self.disasters.warned = true;
                self.events.push(Event::Announce { text: format!("{} — {}", kind.name(), kind.warning()) });
                self.events.push(Event::DisasterWarning { kind });
            }
            if now >= at && self.disasters.active.is_none() {
                self.disasters.scheduled = None;
                self.start_disaster(kind);
            }
        }

        // --- decay shake, cool fissures, recede flood
        self.disasters.shake = (self.disasters.shake - dt * 1.5).max(0.0);
        for f in self.disasters.fissures.iter_mut() {
            f.heat = (f.heat - dt / 30.0).max(0.0);
        }
        let flooding = matches!(self.disasters.active_kind(), Some(DisasterKind::Flood));
        if !flooding && self.disasters.flood_rise > 0.0 {
            self.disasters.flood_rise = (self.disasters.flood_rise - dt * FLOOD_RISE / 20.0).max(0.0);
        }

        // --- fire spreads regardless of the active event (a Tempest can start one)
        self.fire_tick(dt);

        // --- fissures burn while hot
        if !self.disasters.fissures.is_empty() {
            let hot: Vec<(Vec3, Vec3)> = self.disasters.fissures.iter().filter(|f| f.heat > 0.5).map(|f| (f.a, f.b)).collect();
            if !hot.is_empty() {
                let mut hits = Vec::new();
                for (gi, g) in self.gods.iter().enumerate() {
                    if g.respawn_t <= 0.0 && hot.iter().any(|(a, b)| seg_dist(*a, *b, g.pos) < 1.4) {
                        hits.push(gi);
                    }
                }
                for gi in hits {
                    self.deal_damage(TargetRef::God(gi), FISSURE_DMG * dt, true, None, 0.0, 0.0);
                }
            }
        }

        // --- drowning
        if self.disasters.flood_rise > 0.2 {
            let mut hits = Vec::new();
            for (gi, g) in self.gods.iter().enumerate() {
                if g.respawn_t <= 0.0 && self.water_depth_now(g.pos.x, g.pos.z) > DROWN_DEPTH && g.blessing_t <= 0.0 {
                    hits.push((gi, g.max_hp));
                }
            }
            for (gi, mh) in hits {
                self.deal_damage(TargetRef::God(gi), mh * DROWN_DPS_FRAC * dt, true, None, 1.0, 0.0);
            }
        }

        // --- the active event
        let Some(a) = self.disasters.active.clone() else { return };
        if now >= a.until {
            self.end_disaster(a.kind);
            return;
        }
        let mut pulse_t = a.pulse_t - dt;
        match a.kind {
            DisasterKind::Wildfire => {}
            DisasterKind::Tempest => {
                self.weather.force(WeatherKind::Storm, a.until);
                self.weather.gust = self.weather.gust.max(0.85);
                if pulse_t <= 0.0 {
                    pulse_t = 0.5;
                    // strikes hunt the ancestors
                    let alive: Vec<Vec3> = self.gods.iter().filter(|g| g.respawn_t <= 0.0).map(|g| g.pos).collect();
                    let at = if !alive.is_empty() && self.rng.f32() < 0.55 {
                        let p = *self.rng.pick(&alive);
                        let ang = self.rng.f32() * std::f32::consts::TAU;
                        let r = self.rng.range(0.0, 4.0);
                        p + Vec3::new(ang.cos() * r, 0.0, ang.sin() * r)
                    } else {
                        let ang = self.rng.f32() * std::f32::consts::TAU;
                        let r = self.rng.range(10.0, ARENA_RADIUS * 0.8);
                        Vec3::new(ang.cos() * r, 0.0, ang.sin() * r)
                    };
                    self.events.push(Event::BoltFx { from: at + Vec3::Y * 45.0, to: at, color: [0.85, 0.8, 1.0], thick: 0.6 });
                    self.events.push(Event::Explosion { pos: at, radius: 3.0, color: [0.85, 0.8, 1.0] });
                    self.weather.lightning = 1.0;
                    self.strike_all(at, 3.0, 110.0);
                    if fuel_at(at.x, at.z) && self.rng.f32() < 0.12 {
                        self.ignite(at);
                    }
                }
            }
            DisasterKind::Earthquake => {
                self.disasters.shake = 0.7 + ((now * 9.0).sin() * 0.3).abs();
                if pulse_t <= 0.0 {
                    pulse_t = 2.2;
                    // a new fissure somewhere in the playable ring, often near a lane
                    let ang = self.rng.f32() * std::f32::consts::TAU;
                    let r = self.rng.range(15.0, ARENA_RADIUS * 0.8);
                    let c = Vec3::new(ang.cos() * r, 0.0, ang.sin() * r);
                    let dir = {
                        let a = self.rng.f32() * std::f32::consts::TAU;
                        Vec3::new(a.cos(), 0.0, a.sin())
                    };
                    let len = self.rng.range(8.0, 22.0);
                    let (a0, b0) = (c - dir * len * 0.5, c + dir * len * 0.5);
                    self.disasters.fissures.push(Fissure { a: a0, b: b0, heat: 1.0 });
                    self.events.push(Event::FissureOpen { a: a0, b: b0 });
                    // the tear itself hurts and staggers
                    let hits: Vec<usize> = self.gods.iter().enumerate().filter(|(_, g)| g.respawn_t <= 0.0 && seg_dist(a0, b0, g.pos) < 3.0).map(|(i, _)| i).collect();
                    for gi in hits {
                        self.deal_damage(TargetRef::God(gi), 120.0, true, None, 0.0, 0.0);
                        let g = &mut self.gods[gi];
                        g.slow = g.slow.max(0.5);
                        g.slow_t = g.slow_t.max(1.2);
                    }
                }
            }
            DisasterKind::Flood => {
                self.weather.force(WeatherKind::Rain, a.until);
                let t_in = a.until - now;
                let ramp = ((a.kind.duration() - t_in) / 15.0).clamp(0.0, 1.0);
                self.disasters.flood_rise = FLOOD_RISE * ramp;
            }
            DisasterKind::Tornado => {
                self.weather.gust = self.weather.gust.max(0.6);
                if let Some(mut tn) = self.disasters.tornado {
                    // wander: drift across the map with a slow weave
                    tn.spin += dt * 9.0;
                    let weave = Vec3::new(-tn.vel.z, 0.0, tn.vel.x).normalize_or_zero() * (now * 0.7).sin() * 2.5;
                    tn.pos += (tn.vel + weave) * dt;
                    self.disasters.tornado = Some(tn);
                    // pull, hurl, hurt
                    for gi in 0..self.gods.len() {
                        let g = &self.gods[gi];
                        if g.respawn_t > 0.0 {
                            continue;
                        }
                        let to = Vec3::new(tn.pos.x - g.pos.x, 0.0, tn.pos.z - g.pos.z);
                        let d = to.length();
                        if d < TORNADO_PULL_RADIUS && d > TORNADO_RADIUS * 0.5 {
                            let pull = (1.0 - d / TORNADO_PULL_RADIUS) * 7.0;
                            self.gods[gi].pos += to.normalize_or_zero() * pull * dt;
                        }
                        if d < TORNADO_RADIUS {
                            // spin around the eye and get thrown out the far side now and then
                            let tang = Vec3::new(-to.z, 0.0, to.x).normalize_or_zero();
                            self.gods[gi].pos += tang * 9.0 * dt;
                            self.gods[gi].slow = 0.6;
                            self.gods[gi].slow_t = 0.5;
                        }
                    }
                    let hits: Vec<usize> = self.gods.iter().enumerate().filter(|(_, g)| g.respawn_t <= 0.0 && g.pos.distance(tn.pos) < TORNADO_RADIUS).map(|(i, _)| i).collect();
                    for gi in hits {
                        self.deal_damage(TargetRef::God(gi), TORNADO_DPS * dt, true, None, 0.0, 0.0);
                    }
                    if pulse_t <= 0.0 {
                        pulse_t = 0.9;
                        // it flings a hapless animal, and tears the canopy
                        self.events.push(Event::TornadoDebris { pos: tn.pos });
                        let mut wild = std::mem::take(&mut self.wild);
                        wild.prey.retain(|p| p.pos.distance(tn.pos) > TORNADO_RADIUS);
                        self.wild = wild;
                    }
                    // the one that lands in the pit or the base blows itself out early
                    if tn.pos.length() > ARENA_RADIUS + 20.0 {
                        self.end_disaster(a.kind);
                        return;
                    }
                }
            }
        }
        if let Some(a) = self.disasters.active.as_mut() {
            a.pulse_t = pulse_t;
        }
    }

    fn start_disaster(&mut self, kind: DisasterKind) {
        let now = self.t;
        self.disasters.active = Some(Active { kind, until: now + kind.duration(), pulse_t: 0.0 });
        self.disasters.struck += 1;
        self.disasters.warned = false;
        self.events.push(Event::Announce { text: format!("{} !", kind.name()) });
        self.events.push(Event::DisasterStart { kind });
        match kind {
            DisasterKind::Wildfire => {
                // a spark in the jungle, upwind of the middle so it has room to run
                let w = self.weather.atmosphere().wind;
                let dir = if w.length() > 0.1 { w.normalize() } else { Vec3::X };
                let r = self.rng.range(30.0, 70.0);
                let mut at = -dir * r + Vec3::new(self.rng.range(-20.0, 20.0), 0.0, self.rng.range(-20.0, 20.0));
                for _ in 0..30 {
                    if fuel_at(at.x, at.z) {
                        break;
                    }
                    let ang = self.rng.f32() * std::f32::consts::TAU;
                    let rr = self.rng.range(25.0, 90.0);
                    at = Vec3::new(ang.cos() * rr, 0.0, ang.sin() * rr);
                }
                self.ignite(at);
                self.ignite(at + Vec3::new(FIRE_CELL, 0.0, 0.0));
                self.weather.gust = self.weather.gust.max(0.5);
            }
            DisasterKind::Tempest => {
                self.weather.force(WeatherKind::Storm, now + kind.duration());
                self.weather.night_target = crate::weather::base_night().max(0.6);
            }
            DisasterKind::Earthquake => {
                self.disasters.shake = 1.0;
            }
            DisasterKind::Flood => {
                self.weather.force(WeatherKind::Rain, now + kind.duration());
            }
            DisasterKind::Tornado => {
                // walks in from the rim, aimed roughly through the middle
                let ang = self.rng.f32() * std::f32::consts::TAU;
                let start = Vec3::new(ang.cos(), 0.0, ang.sin()) * (ARENA_RADIUS + 10.0);
                let aim = Vec3::new(self.rng.range(-30.0, 30.0), 0.0, self.rng.range(-30.0, 30.0));
                let vel = (aim - start).normalize_or_zero() * 6.5;
                self.disasters.tornado = Some(Tornado { pos: start, vel, spin: 0.0 });
                self.weather.force(WeatherKind::Overcast, now + kind.duration());
            }
        }
    }

    fn end_disaster(&mut self, kind: DisasterKind) {
        self.disasters.active = None;
        self.disasters.tornado = None;
        if kind == DisasterKind::Tempest {
            self.weather.night_target = if self.map_events.is(crate::map_events::MapEventKind::LongNight) { 1.0 } else { crate::weather::base_night() };
        }
        self.events.push(Event::DisasterEnd { kind });
        self.events.push(Event::Announce { text: match kind {
            DisasterKind::Wildfire => "the fire has burned out".into(),
            DisasterKind::Tempest => "the tempest passes".into(),
            DisasterKind::Earthquake => "the ground is still".into(),
            DisasterKind::Flood => "the waters recede".into(),
            DisasterKind::Tornado => "the funnel lifts".into(),
        } });
    }

    pub fn ignite(&mut self, p: Vec3) {
        let c = cell_of(p);
        if !fuel_at(c.x, c.z) || self.disasters.is_charred(c) || self.disasters.fire.iter().any(|f| f.pos.distance(c) < 0.5) {
            return;
        }
        self.disasters.fire.push(FireCell { pos: c, burn: FIRE_BURN_SECS, spread_t: 2.0 });
        self.events.push(Event::FireIgnite { pos: c });
    }

    fn fire_tick(&mut self, dt: f32) {
        if self.disasters.fire.is_empty() {
            return;
        }
        let atm = self.weather.atmosphere();
        let wind = atm.wind;
        let wdir = if wind.length() > 0.1 { wind.normalize() } else { Vec3::ZERO };
        let rain = atm.rain;
        let mut ignite = Vec::new();
        let mut out = Vec::new();
        for i in 0..self.disasters.fire.len() {
            let f = &mut self.disasters.fire[i];
            f.burn -= dt * (1.0 + rain * 4.0);
            f.spread_t -= dt;
            if f.spread_t <= 0.0 {
                f.spread_t = 2.5;
                let pos = f.pos;
                for (dx, dz) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                    let d = Vec3::new(dx, 0.0, dz);
                    let downwind = d.dot(wdir);
                    let p = (0.55 + downwind * 0.35) * (1.0 - rain * 0.8) * (1.0 + atm.gust * 0.4);
                    if self.rng.f32() < p {
                        ignite.push(pos + d * FIRE_CELL);
                    }
                }
            }
            if f.burn <= 0.0 {
                out.push(i);
            }
        }
        for i in out.into_iter().rev() {
            let f = self.disasters.fire.remove(i);
            self.disasters.charred.push(f.pos);
        }
        for p in ignite {
            self.ignite(p);
        }
        // it hurts to stand in it; animals flee it or burn
        let burning: Vec<Vec3> = self.disasters.fire.iter().map(|f| f.pos).collect();
        let mut hits = Vec::new();
        for (gi, g) in self.gods.iter().enumerate() {
            if g.respawn_t <= 0.0 && burning.iter().any(|b| b.distance(Vec3::new(g.pos.x, 0.0, g.pos.z)) < FIRE_CELL * 0.72) {
                hits.push((gi, g.max_hp));
            }
        }
        for (gi, mh) in hits {
            self.deal_damage(TargetRef::God(gi), mh * FIRE_DPS_FRAC * dt, true, None, 1.0, 0.0);
        }
        let mut wild = std::mem::take(&mut self.wild);
        for p in wild.prey.iter_mut() {
            if let Some(b) = burning.iter().find(|b| b.distance(p.pos) < FIRE_CELL * 1.5) {
                p.fear = 3.0;
                p.vel += (p.pos - *b).normalize_or_zero() * 4.0 * dt;
            }
        }
        wild.prey.retain(|p| !burning.iter().any(|b| b.distance(p.pos) < FIRE_CELL * 0.5));
        self.wild = wild;
        // the fire that runs out of fuel is over
        if self.disasters.fire.is_empty() && matches!(self.disasters.active_kind(), Some(DisasterKind::Wildfire)) {
            self.end_disaster(DisasterKind::Wildfire);
        }
    }

    /// Team-agnostic strike used by the tempest.
    fn strike_all(&mut self, at: Vec3, radius: f32, dmg: f32) {
        let hits: Vec<usize> = self.gods.iter().enumerate().filter(|(_, g)| g.respawn_t <= 0.0 && g.pos.distance(at) <= radius).map(|(i, _)| i).collect();
        for gi in hits {
            self.deal_damage(TargetRef::God(gi), dmg, true, None, 0.0, 0.0);
        }
        for mi in (0..self.minions.len()).rev() {
            if self.minions[mi].pos.distance(at) <= radius {
                self.deal_damage(TargetRef::Minion(mi), dmg, true, None, 0.0, 0.0);
            }
        }
    }
}

fn seg_dist(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-4)).clamp(0.0, 1.0);
    (a + ab * t - Vec3::new(p.x, 0.0, p.z)).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        for u in g.gods.iter_mut() {
            u.ai = None;
            u.pos = Vec3::new(0.0, 0.0, -30.0);
        }
        g
    }

    #[test]
    fn wildfire_spreads_chars_and_burns_who_stands_in_it() {
        let mut g = game();
        g.weather.set(WeatherKind::Clear);
        g.weather.blend = 1.0;
        g.start_disaster(DisasterKind::Wildfire);
        let first = g.disasters.fire[0].pos;
        g.gods[0].pos = first;
        let hp = g.gods[0].hp;
        for _ in 0..(40 * 30) {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.gods[0].hp < hp, "standing in the fire hurts");
        assert!(!g.disasters.charred.is_empty(), "burnt cells become scars");
        assert!(g.disasters.fire.len() + g.disasters.charred.len() > 3, "it spread: {} burning, {} charred", g.disasters.fire.len(), g.disasters.charred.len());
        assert!(g.disasters.is_charred(first) || g.disasters.burning_at(first));
    }

    #[test]
    fn rain_puts_a_fire_out() {
        let mut g = game();
        g.weather.set(WeatherKind::Storm);
        g.weather.blend = 1.0;
        g.start_disaster(DisasterKind::Wildfire);
        for _ in 0..(30 * 30) {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.disasters.fire.is_empty(), "rain should have doused it");
        assert!(g.disasters.charred.len() <= 6, "and kept it small: {}", g.disasters.charred.len());
    }

    #[test]
    fn flood_makes_everyone_wade_and_drowns_in_the_channel() {
        let mut g = game();
        let dry = g.move_speed(0);
        g.start_disaster(DisasterKind::Flood);
        for _ in 0..(20 * 30) {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.disasters.flood_rise > FLOOD_RISE * 0.9);
        assert!(g.move_speed(0) < dry * 0.9, "the floor is under water: {} vs {}", g.move_speed(0), dry);
        let z = 10.0;
        g.gods[0].pos = Vec3::new(crate::consts::river_x(1.0, z), 0.0, z);
        let hp = g.gods[0].hp;
        for _ in 0..30 {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.gods[0].hp < hp, "the channel drowns");
        // it recedes after the event
        g.t = g.disasters.active.as_ref().unwrap().until + 0.1;
        for _ in 0..(30 * 30) {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.disasters.flood_rise < 0.01);
    }

    #[test]
    fn tornado_drags_and_crosses_the_map() {
        let mut g = game();
        g.start_disaster(DisasterKind::Tornado);
        let tn = g.disasters.tornado.unwrap();
        // put an ancestor a little inside the pull radius, off the path
        g.gods[0].pos = tn.pos + tn.vel.normalize() * 10.0 + Vec3::new(0.0, 0.0, 6.0);
        let d0 = g.gods[0].pos.distance(g.disasters.tornado.unwrap().pos);
        g.disasters_tick(0.5);
        let d1 = g.gods[0].pos.distance(g.disasters.tornado.unwrap().pos);
        assert!(d1 < d0, "pulled toward the funnel: {d0} -> {d1}");
        for _ in 0..(60 * 30) {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.disasters.tornado.is_none(), "it walks off or lifts");
    }

    #[test]
    fn earthquake_tears_fissures_that_cool() {
        let mut g = game();
        g.start_disaster(DisasterKind::Earthquake);
        for _ in 0..(10 * 30) {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.disasters.fissures.len() >= 3);
        assert!(g.disasters.shake > 0.3);
        assert!(g.disasters.speed_mul() < 1.0);
        for _ in 0..(60 * 30) {
            g.t += 1.0 / 30.0;
            g.disasters_tick(1.0 / 30.0);
        }
        assert!(g.disasters.active.is_none() && g.disasters.fissures.iter().all(|f| f.heat < 0.5), "scars cool");
    }
}
