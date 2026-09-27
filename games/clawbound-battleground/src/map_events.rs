// Map events — timed happenings that change the shape of a match. Headless,
// deterministic through the game rng, driven from `Game::tick`.
//
// Five events, drawn from Clawbound's lore:
//   Sweetrain      — the dragon's rain over the centre: both teams mend
//                    inside it (contested ground). Sky → Rain.
//   The Long Night — the moon goes out: ambushers (Assassins) run swift,
//                    AI sight closes in. Sky → night.
//   Thunder Front  — the ninth son's father passes: lightning strikes the
//                    lanes, hurting everyone it lands on. Sky → Storm.
//   The Long Winter— the season takes the field: everyone slows except
//                    Snowhush, who is home. Sky → Snow.
//   Elder's Bounty — the elders watch: kill gold is doubled.
//   Prey Run       — a great herd of stags pours down both rivers; catch
//                    them for gold and a mouthful of health (wild.rs).
//
// Each hook the sim reads is one function here (`speed_mul`, `gold_mul`),
// so the event table can grow without touching systems.rs.

use crate::consts::{lane_clearance, Team, ARENA_RADIUS};
use crate::gods::Role;
use crate::sim::{Event, Game, TargetRef};
use crate::weather::{WeatherEvent, WeatherKind};
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapEventKind {
    Sweetrain,
    LongNight,
    ThunderFront,
    LongWinter,
    EldersBounty,
    PreyRun,
}

pub const ALL_EVENTS: [MapEventKind; 6] = [
    MapEventKind::Sweetrain,
    MapEventKind::LongNight,
    MapEventKind::ThunderFront,
    MapEventKind::LongWinter,
    MapEventKind::EldersBounty,
    MapEventKind::PreyRun,
];

impl MapEventKind {
    pub fn name(self) -> &'static str {
        match self {
            MapEventKind::Sweetrain => "SWEETRAIN",
            MapEventKind::LongNight => "THE LONG NIGHT",
            MapEventKind::ThunderFront => "THUNDER FRONT",
            MapEventKind::LongWinter => "THE LONG WINTER",
            MapEventKind::EldersBounty => "ELDER'S BOUNTY",
            MapEventKind::PreyRun => "PREY RUN",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            MapEventKind::Sweetrain => "the dragon's rain mends all who stand in it",
            MapEventKind::LongNight => "the moon goes out — the ambushers run swift",
            MapEventKind::ThunderFront => "lightning walks the lanes",
            MapEventKind::LongWinter => "the season takes the field — all but Snowhush slow",
            MapEventKind::EldersBounty => "the elders watch — kills pay double",
            MapEventKind::PreyRun => "the stags run the rivers — hunt them down",
        }
    }
    pub fn duration(self) -> f32 {
        match self {
            MapEventKind::Sweetrain => 30.0,
            MapEventKind::LongNight => 36.0,
            MapEventKind::ThunderFront => 28.0,
            MapEventKind::LongWinter => 30.0,
            MapEventKind::EldersBounty => 40.0,
            MapEventKind::PreyRun => 45.0,
        }
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            MapEventKind::Sweetrain => [0.55, 0.78, 0.95],
            MapEventKind::LongNight => [0.45, 0.4, 0.8],
            MapEventKind::ThunderFront => [0.75, 0.65, 1.0],
            MapEventKind::LongWinter => [0.85, 0.95, 1.0],
            MapEventKind::EldersBounty => [1.0, 0.85, 0.4],
            MapEventKind::PreyRun => [0.8, 0.6, 0.35],
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActiveEvent {
    pub kind: MapEventKind,
    pub pos: Vec3,
    pub radius: f32,
    pub until: f32,
    pub pulse_t: f32,
}

#[derive(Clone, Debug)]
pub struct MapEvents {
    pub active: Option<ActiveEvent>,
    pub next_t: f32,
    pub last: Option<MapEventKind>,
    pub count: u32,
    /// Ancestor roster id that is immune to the Long Winter (Snowhush).
    pub winter_home: Option<u32>,
}

pub const FIRST_EVENT_T: f32 = 60.0;
pub const SWEETRAIN_RADIUS: f32 = 15.0;
pub const SWEETRAIN_HEAL_FRAC_PER_S: f32 = 0.028;
pub const THUNDER_DMG: f32 = 140.0;
pub const THUNDER_RADIUS: f32 = 3.2;

impl MapEvents {
    pub fn new() -> MapEvents {
        let winter_home = crate::gods::ROSTER.iter().find(|d| d.name == "Snowhush").map(|d| d.id);
        MapEvents { active: None, next_t: FIRST_EVENT_T, last: None, count: 0, winter_home }
    }

    pub fn is(&self, k: MapEventKind) -> bool {
        self.active.as_ref().map_or(false, |a| a.kind == k)
    }

    /// Movement multiplier for an ancestor of `role` with roster id `def_id`.
    pub fn speed_mul(&self, role: Role, def_id: u32) -> f32 {
        match self.active.as_ref().map(|a| a.kind) {
            Some(MapEventKind::LongNight) if role == Role::Assassin => 1.25,
            Some(MapEventKind::LongWinter) => {
                if Some(def_id) == self.winter_home { 1.0 } else { 0.88 }
            }
            _ => 1.0,
        }
    }

    pub fn gold_mul(&self) -> f32 {
        if self.is(MapEventKind::EldersBounty) { 2.0 } else { 1.0 }
    }
}

impl Default for MapEvents {
    fn default() -> Self {
        MapEvents::new()
    }
}

impl Game {
    /// Weather + map events, called once per sim tick after zones.
    pub fn atmosphere_tick(&mut self, dt: f32) {
        // -- weather
        let mut wev = Vec::new();
        let t = self.t;
        self.weather.tick(dt, t, &mut self.rng, &mut wev);
        for e in wev {
            match e {
                WeatherEvent::Changed(k) => {
                    self.events.push(Event::WeatherChanged { kind: k });
                }
                WeatherEvent::Lightning { pos } => {
                    self.events.push(Event::BoltFx { from: pos + Vec3::Y * 60.0, to: pos, color: [0.85, 0.85, 1.0], thick: 0.5 });
                }
            }
        }

        // -- map events: schedule
        if self.map_events.active.is_none() && self.t >= self.map_events.next_t && self.over.is_none() {
            let mut kind = *self.rng.pick(&ALL_EVENTS);
            if Some(kind) == self.map_events.last {
                let i = ALL_EVENTS.iter().position(|&k| k == kind).unwrap();
                kind = ALL_EVENTS[(i + 1) % ALL_EVENTS.len()];
            }
            self.start_map_event(kind);
        }

        // -- map events: run
        let Some(a) = self.map_events.active.clone() else { return };
        if self.t >= a.until {
            self.end_map_event(a.kind);
            return;
        }
        let mut pulse_t = a.pulse_t - dt;
        match a.kind {
            MapEventKind::Sweetrain => {
                for gi in 0..self.gods.len() {
                    let g = &mut self.gods[gi];
                    if g.respawn_t <= 0.0 && g.pos.distance(a.pos) <= a.radius {
                        g.hp = (g.hp + g.max_hp * SWEETRAIN_HEAL_FRAC_PER_S * dt * g.heal_mul).min(g.max_hp);
                    }
                }
                if pulse_t <= 0.0 {
                    pulse_t = 1.5;
                    self.events.push(Event::HealFx { pos: a.pos, radius: a.radius });
                }
            }
            MapEventKind::ThunderFront => {
                if pulse_t <= 0.0 {
                    pulse_t = 1.1;
                    // strike a lane-ish point within the playable ring
                    let mut at = Vec3::ZERO;
                    for _ in 0..12 {
                        let ang = self.rng.f32() * std::f32::consts::TAU;
                        let r = self.rng.range(10.0, ARENA_RADIUS * 0.75);
                        let p = Vec3::new(ang.cos() * r, 0.0, ang.sin() * r);
                        at = p;
                        if lane_clearance(p.x, p.z) < 6.0 {
                            break;
                        }
                    }
                    // prefer a point near a living ancestor a third of the time
                    if self.rng.f32() < 0.34 {
                        let alive: Vec<Vec3> = self.gods.iter().filter(|g| g.respawn_t <= 0.0).map(|g| g.pos).collect();
                        if !alive.is_empty() {
                            let p = *self.rng.pick(&alive);
                            let ang = self.rng.f32() * std::f32::consts::TAU;
                            let r = self.rng.range(1.0, 5.0);
                            at = p + Vec3::new(ang.cos() * r, 0.0, ang.sin() * r);
                        }
                    }
                    self.events.push(Event::BoltFx { from: at + Vec3::Y * 40.0, to: at, color: [0.8, 0.75, 1.0], thick: 0.55 });
                    self.events.push(Event::Explosion { pos: at, radius: THUNDER_RADIUS, color: [0.8, 0.75, 1.0] });
                    self.weather.lightning = 1.0;
                    self.strike_everyone(at, THUNDER_RADIUS, THUNDER_DMG);
                }
            }
            MapEventKind::LongWinter | MapEventKind::LongNight | MapEventKind::EldersBounty | MapEventKind::PreyRun => {}
        }
        if let Some(a) = self.map_events.active.as_mut() {
            a.pulse_t = pulse_t;
        }
    }

    pub fn start_map_event(&mut self, kind: MapEventKind) {
        let until = self.t + kind.duration();
        let (pos, radius) = match kind {
            MapEventKind::Sweetrain => (Vec3::ZERO, SWEETRAIN_RADIUS),
            _ => (Vec3::ZERO, 0.0),
        };
        match kind {
            MapEventKind::Sweetrain => self.weather.force(WeatherKind::Rain, until),
            MapEventKind::ThunderFront => self.weather.force(WeatherKind::Storm, until),
            MapEventKind::LongWinter => self.weather.force(WeatherKind::Snow, until),
            MapEventKind::LongNight => self.weather.night_target = 1.0,
            MapEventKind::EldersBounty => {}
            MapEventKind::PreyRun => {
                let mut wild = std::mem::take(&mut self.wild);
                wild.spawn_prey_run(&mut self.rng);
                self.wild = wild;
            }
        }
        self.map_events.active = Some(ActiveEvent { kind, pos, radius, until, pulse_t: 0.0 });
        self.map_events.last = Some(kind);
        self.map_events.count += 1;
        self.events.push(Event::Announce { text: format!("{} — {}", kind.name(), kind.blurb()) });
        self.events.push(Event::MapEventStart { kind, pos, radius });
        if kind == MapEventKind::Sweetrain {
            self.events.push(Event::StormFx { pos, radius, color: kind.color(), dur: 1.4 });
        }
    }

    pub fn end_map_event(&mut self, kind: MapEventKind) {
        if kind == MapEventKind::LongNight {
            self.weather.night_target = crate::weather::base_night();
        }
        self.map_events.active = None;
        self.map_events.next_t = self.t + self.rng.range(75.0, 110.0);
        self.events.push(Event::MapEventEnd { kind });
    }

    /// Team-agnostic AoE (the sky takes no side).
    fn strike_everyone(&mut self, at: Vec3, radius: f32, dmg: f32) {
        let mut targets = Vec::new();
        for gi in 0..self.gods.len() {
            let g = &self.gods[gi];
            if g.respawn_t <= 0.0 && g.pos.distance(at) <= radius {
                targets.push(TargetRef::God(gi));
            }
        }
        // minions: iterate by index descending so removals don't shift the rest
        for mi in (0..self.minions.len()).rev() {
            if self.minions[mi].pos.distance(at) <= radius {
                targets.push(TargetRef::Minion(mi));
            }
        }
        for t in targets {
            self.deal_damage(t, dmg, true, None, 0.0, 0.0);
        }
        let _ = Team::Blue; // (kept for symmetry with aoe_damage's team-scoped API)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Game;

    fn game() -> Game {
        Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9])
    }

    #[test]
    fn events_fire_in_sequence() {
        let mut g = game();
        // nobody fights, so the match can't end early and cut the schedule short
        for u in g.gods.iter_mut() {
            u.ai = None;
        }
        let mut seen = Vec::new();
        for _ in 0..(420 * 30) {
            g.tick(1.0 / 30.0);
            for e in g.events.drain(..) {
                if let Event::MapEventStart { kind, .. } = e {
                    seen.push(kind);
                }
            }
        }
        assert!(seen.len() >= 3, "expected several map events in 7 min, got {seen:?}");
        for w in seen.windows(2) {
            assert_ne!(w[0], w[1], "the same event should not repeat back-to-back");
        }
    }

    #[test]
    fn sweetrain_heals_both_teams() {
        let mut g = game();
        for gi in [0usize, 5] {
            g.gods[gi].pos = Vec3::new(2.0, 0.0, 2.0);
            g.gods[gi].hp = 100.0;
            g.gods[gi].ai = None;
        }
        g.start_map_event(MapEventKind::Sweetrain);
        g.events.clear();
        for _ in 0..30 {
            g.atmosphere_tick(1.0 / 30.0);
        }
        assert!(g.gods[0].hp > 100.0 && g.gods[5].hp > 100.0, "rain should mend both sides");
        assert_eq!(g.weather.kind, WeatherKind::Rain);
    }

    #[test]
    fn long_winter_spares_snowhush() {
        let g = game();
        let mut me = MapEvents::new();
        let snow = me.winter_home.expect("Snowhush is on the roster");
        me.active = Some(ActiveEvent { kind: MapEventKind::LongWinter, pos: Vec3::ZERO, radius: 0.0, until: 1e9, pulse_t: 0.0 });
        assert_eq!(me.speed_mul(Role::Guardian, snow), 1.0);
        assert!(me.speed_mul(Role::Warrior, 0) < 1.0);
        let _ = g;
    }

    #[test]
    fn thunder_hurts_whoever_it_lands_on() {
        let mut g = game();
        let hp0 = g.gods[0].hp;
        let hp5 = g.gods[5].hp;
        g.gods[0].pos = Vec3::new(0.0, 0.0, 0.0);
        g.gods[5].pos = Vec3::new(1.0, 0.0, 0.0);
        g.strike_everyone(Vec3::ZERO, THUNDER_RADIUS, THUNDER_DMG);
        assert!(g.gods[0].hp < hp0 && g.gods[5].hp < hp5);
    }
}
