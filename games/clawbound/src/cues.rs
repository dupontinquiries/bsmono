//! cues — what just happened, named, for the sound (and anything else that wants a moment).
//!
//! The hunt (`sim::Game`) and the prowl (`prowl::Prowl`) are sims with state and no events:
//! a catch is a counter that went up, a hawk grab is a `carried_t` that is no longer zero,
//! the hunter spotting you is an `HState` that changed. The game had no sound at all, and
//! the first thing a sound needs is a list of moments. This module is that list -- a
//! [`Listener`] per mode that remembers the last state it saw and names the differences --
//! and, for the continuous half, an [`Ambience`]: how much wind, how much rain, how tense,
//! read every frame from the weather and the hunter.
//!
//! Pure. The backend (`audio`) plays what it is handed; every rule about WHAT is heard is
//! here, where it can be tested without a device.

use crate::hunter::HState;
use crate::prowl::Prowl;
use crate::sim::{Game, Outcome};
use crate::weather::{Sky, Weather};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cue {
    /// Your cat bounds.
    Leap,
    /// Your cat rolls.
    Dodge,
    /// A bramble has you.
    Snag,
    /// The hawk has you in its talons.
    HawkGrab,
    /// You tore free (crippled, limping).
    BrokeFree,
    /// The hunter went from patrolling to alert: it has something.
    HunterAlert,
    /// The hunter is on you.
    HunterChase,
    /// The hunter lost you: back to patrol or search.
    HunterLost,
    /// A cat of the pack was taken (your own, when `player`).
    CatCaught { player: bool },
    /// A cat reached the beacon.
    CatEscaped { player: bool },
    /// A critter pounced: a mouthful.
    PreyPounced,
    /// Into or out of the water (the prowl).
    Splash,
    /// A move landed as a combo (the prowl).
    Combo,
    /// A Thick Pelt soaked a catch (the prowl).
    Shielded,
    /// The sky turned: rain starting, a storm breaking, clearing.
    SkyChange(Sky),
    /// The round is decided.
    Win,
    Lose,
}

/// The continuous half: what the air is doing and how tense it is, 0..1 each.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Ambience {
    /// Wind in the leaves: strength with the gust on top.
    pub wind: f32,
    /// Rain on the canopy (a storm is rain and more wind).
    pub rain: f32,
    /// Thunder's room: 1 in a storm.
    pub storm: f32,
    /// The snow hush: 1 under snow, where the wind reads softer.
    pub snow: f32,
    /// The hunter's nearness and attention, for a drone that rises with it.
    pub tension: f32,
    /// Running, for the breath and the paws.
    pub running: f32,
}

/// The air from the weather alone; the tension is the caller's.
pub fn air(w: &Weather) -> Ambience {
    let (rain, storm, snow) = match w.sky {
        Sky::Rain => (0.7, 0.0, 0.0),
        Sky::Storm => (1.0, 1.0, 0.0),
        Sky::Snow => (0.0, 0.0, 1.0),
        Sky::Clear | Sky::Overcast | Sky::Fog => (0.0, 0.0, 0.0),
    };
    let wind = (w.wind_strength * 0.7 + w.gust * 0.5 + storm * 0.3).clamp(0.0, 1.0) * (1.0 - snow * 0.5);
    Ambience { wind, rain, storm, snow, tension: 0.0, running: 0.0 }
}

/// How tense a hunter in `state` with `suspicion` is to listen to, scaled by `danger` (the
/// hunt's own 0..1 read of nearness and attention; the prowl passes its suspicion twice).
pub fn tension(state: HState, suspicion: f32, danger: f32) -> f32 {
    let floor: f32 = match state {
        HState::Patrol => 0.0,
        HState::Search => 0.3,
        HState::Alert | HState::Track => 0.5,
        HState::Chase => 1.0,
    };
    floor.max(danger.clamp(0.0, 1.0) * 0.8 + suspicion.clamp(0.0, 1.0) * 0.2)
}

fn hunter_cues(was: HState, now: HState, out: &mut Vec<Cue>) {
    if was == now {
        return;
    }
    match (was, now) {
        (_, HState::Chase) => out.push(Cue::HunterChase),
        (HState::Patrol, HState::Alert) | (HState::Patrol, HState::Track) | (HState::Search, HState::Alert) | (HState::Search, HState::Track) => out.push(Cue::HunterAlert),
        (HState::Chase, HState::Patrol) | (HState::Chase, HState::Search) | (HState::Track, HState::Patrol) | (HState::Alert, HState::Patrol) => out.push(Cue::HunterLost),
        _ => {}
    }
}

/// Watches the hunt (`sim::Game`), whose player is `cats[0]`.
#[derive(Clone)]
pub struct HuntListener {
    leaping: bool,
    dodging: bool,
    snagged: bool,
    carried: bool,
    crippled: bool,
    hunter: HState,
    caught: u32,
    escaped: u32,
    prey: u32,
    alive: Vec<bool>,
    safe: Vec<bool>,
    sky: Sky,
    outcome: Option<Outcome>,
}

impl HuntListener {
    pub fn new(g: &Game) -> HuntListener {
        let me = &g.cats[0];
        HuntListener {
            leaping: me.is_leaping(),
            dodging: me.dodge_t > 0.0,
            snagged: me.snag_t > 0.0,
            carried: me.is_carried(),
            crippled: me.crippled_t > 0.0,
            hunter: g.hunter.state,
            caught: g.caught,
            escaped: g.escaped,
            prey: g.prey_caught,
            alive: g.cats.iter().map(|c| c.alive).collect(),
            safe: g.cats.iter().map(|c| c.safe).collect(),
            sky: g.weather.sky,
            outcome: g.outcome,
        }
    }

    pub fn ambience(g: &Game) -> Ambience {
        let mut a = air(&g.weather);
        a.tension = tension(g.hunter.state, g.hunter.suspicion, g.danger);
        let me = &g.cats[0];
        a.running = if me.alive && me.moving && !me.crouched { (me.speed / 6.0).clamp(0.0, 1.0) } else { 0.0 };
        a
    }

    pub fn step(&mut self, g: &Game) -> Vec<Cue> {
        let mut out = Vec::new();
        let me = &g.cats[0];
        let edge = |was: bool, now: bool| !was && now;
        if edge(self.leaping, me.is_leaping()) {
            out.push(Cue::Leap);
        }
        if edge(self.dodging, me.dodge_t > 0.0) {
            out.push(Cue::Dodge);
        }
        if edge(self.snagged, me.snag_t > 0.0) {
            out.push(Cue::Snag);
        }
        if edge(self.carried, me.is_carried()) {
            out.push(Cue::HawkGrab);
        }
        if edge(self.crippled, me.crippled_t > 0.0) {
            out.push(Cue::BrokeFree);
        }
        hunter_cues(self.hunter, g.hunter.state, &mut out);
        // a catch is heard as the cat it took: yours, or one of the pack
        if g.caught > self.caught {
            let mine = self.alive.first().copied().unwrap_or(true) && !me.alive;
            out.push(Cue::CatCaught { player: mine });
        }
        if g.escaped > self.escaped {
            let mine = !self.safe.first().copied().unwrap_or(false) && me.safe;
            out.push(Cue::CatEscaped { player: mine });
        }
        if g.prey_caught > self.prey {
            out.push(Cue::PreyPounced);
        }
        if g.weather.sky != self.sky {
            out.push(Cue::SkyChange(g.weather.sky));
        }
        if self.outcome.is_none() {
            match g.outcome {
                Some(Outcome::CatsWin) => out.push(Cue::Win),
                Some(Outcome::HunterWins) => out.push(Cue::Lose),
                None => {}
            }
        }
        self.leaping = me.is_leaping();
        self.dodging = me.dodge_t > 0.0;
        self.snagged = me.snag_t > 0.0;
        self.carried = me.is_carried();
        self.crippled = me.crippled_t > 0.0;
        self.hunter = g.hunter.state;
        self.caught = g.caught;
        self.escaped = g.escaped;
        self.prey = g.prey_caught;
        self.alive = g.cats.iter().map(|c| c.alive).collect();
        self.safe = g.cats.iter().map(|c| c.safe).collect();
        self.sky = g.weather.sky;
        self.outcome = g.outcome;
        out
    }
}

/// Watches the prowl (`prowl::Prowl`), where the player IS the struct.
#[derive(Clone)]
pub struct ProwlListener {
    leaping: bool,
    dodging: bool,
    snagged: bool,
    hunter: HState,
    caught: u32,
    escapes: u32,
    prey: u32,
    splash: bool,
    combo: bool,
    shielded: bool,
    sky: Sky,
    outcome: Option<u8>,
}

impl ProwlListener {
    pub fn new(p: &Prowl) -> ProwlListener {
        ProwlListener {
            leaping: p.leap_t > 0.0,
            dodging: p.dodge_t > 0.0,
            snagged: p.snag_t > 0.0,
            hunter: p.hstate,
            caught: p.times_caught,
            escapes: p.escapes,
            prey: p.prey_caught,
            splash: p.splash_flash > 0.0,
            combo: p.combo_flash > 0.0,
            shielded: p.shielded_flash > 0.0,
            sky: p.weather.sky,
            outcome: p.outcome,
        }
    }

    pub fn ambience(p: &Prowl) -> Ambience {
        let mut a = air(&p.weather);
        a.tension = if p.tutorial_mode { 0.0 } else { tension(p.hstate, p.suspicion, p.suspicion) };
        a.running = if p.moving && !p.crouched { 0.7 } else { 0.0 };
        a
    }

    pub fn step(&mut self, p: &Prowl) -> Vec<Cue> {
        let mut out = Vec::new();
        let edge = |was: bool, now: bool| !was && now;
        if edge(self.leaping, p.leap_t > 0.0) {
            out.push(Cue::Leap);
        }
        if edge(self.dodging, p.dodge_t > 0.0) {
            out.push(Cue::Dodge);
        }
        if edge(self.snagged, p.snag_t > 0.0) {
            out.push(Cue::Snag);
        }
        hunter_cues(self.hunter, p.hstate, &mut out);
        if p.times_caught > self.caught {
            out.push(Cue::CatCaught { player: true });
        }
        if p.escapes > self.escapes {
            out.push(Cue::CatEscaped { player: true });
        }
        if p.prey_caught > self.prey {
            out.push(Cue::PreyPounced);
        }
        if edge(self.splash, p.splash_flash > 0.0) {
            out.push(Cue::Splash);
        }
        if edge(self.combo, p.combo_flash > 0.0) {
            out.push(Cue::Combo);
        }
        if edge(self.shielded, p.shielded_flash > 0.0) {
            out.push(Cue::Shielded);
        }
        if p.weather.sky != self.sky {
            out.push(Cue::SkyChange(p.weather.sky));
        }
        if self.outcome.is_none() {
            match p.outcome {
                Some(0) => out.push(Cue::Win),
                Some(_) => out.push(Cue::Lose),
                None => {}
            }
        }
        self.leaping = p.leap_t > 0.0;
        self.dodging = p.dodge_t > 0.0;
        self.snagged = p.snag_t > 0.0;
        self.hunter = p.hstate;
        self.caught = p.times_caught;
        self.escapes = p.escapes;
        self.prey = p.prey_caught;
        self.splash = p.splash_flash > 0.0;
        self.combo = p.combo_flash > 0.0;
        self.shielded = p.shielded_flash > 0.0;
        self.sky = p.weather.sky;
        self.outcome = p.outcome;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class::Class;
    use crate::hunter::Kind;
    use crate::sim::Mode;
    use crate::weather::Season;

    /// The hunt's moments, each from the state that means it, once per edge.
    #[test]
    fn the_hunt_listener_names_each_moment_once() {
        let mut g = Game::new_match(Kind::Fox, Mode::Rendezvous);
        let mut l = HuntListener::new(&g);
        assert!(l.step(&g).is_empty(), "nothing happened");
        g.cats[0].leap_t = 0.4;
        g.cats[0].dodge_t = 0.2;
        g.cats[0].snag_t = 0.3;
        g.cats[0].carried_t = 1.0;
        let c = l.step(&g);
        assert_eq!(c, vec![Cue::Leap, Cue::Dodge, Cue::Snag, Cue::HawkGrab]);
        assert!(l.step(&g).is_empty(), "still leaping is not leaping again");
        g.cats[0].carried_t = 0.0;
        g.cats[0].crippled_t = 2.0;
        assert_eq!(l.step(&g), vec![Cue::BrokeFree]);
        // the hunter
        g.hunter.state = HState::Alert;
        assert_eq!(l.step(&g), vec![Cue::HunterAlert]);
        g.hunter.state = HState::Chase;
        assert_eq!(l.step(&g), vec![Cue::HunterChase]);
        g.hunter.state = HState::Search;
        assert_eq!(l.step(&g), vec![Cue::HunterLost]);
        g.hunter.state = HState::Track;
        assert_eq!(l.step(&g), vec![Cue::HunterAlert]);
        // a pack cat taken, then you; a pack cat out, then you; a mouthful
        let other = g.cats.len() - 1;
        g.cats[other].alive = false;
        g.caught += 1;
        assert_eq!(l.step(&g), vec![Cue::CatCaught { player: false }]);
        g.cats[0].alive = false;
        g.caught += 1;
        assert_eq!(l.step(&g), vec![Cue::CatCaught { player: true }]);
        g.cats[0].alive = true;
        g.cats[1].safe = true;
        g.escaped += 1;
        assert_eq!(l.step(&g), vec![Cue::CatEscaped { player: false }]);
        g.cats[0].safe = true;
        g.escaped += 1;
        assert_eq!(l.step(&g), vec![Cue::CatEscaped { player: true }]);
        g.prey_caught += 1;
        assert_eq!(l.step(&g), vec![Cue::PreyPounced]);
        g.weather.sky = Sky::Storm;
        assert_eq!(l.step(&g), vec![Cue::SkyChange(Sky::Storm)]);
        g.outcome = Some(Outcome::CatsWin);
        assert_eq!(l.step(&g), vec![Cue::Win]);
        assert!(l.step(&g).is_empty(), "a decided round is decided once");
    }

    /// The prowl's moments, the flashes included.
    #[test]
    fn the_prowl_listener_names_its_moments_once() {
        let mut p = Prowl::new(Class::Cat, Kind::Dog);
        let mut l = ProwlListener::new(&p);
        assert!(l.step(&p).is_empty());
        p.leap_t = 0.3;
        p.splash_flash = 0.5;
        p.combo_flash = 0.5;
        p.shielded_flash = 0.5;
        assert_eq!(l.step(&p), vec![Cue::Leap, Cue::Splash, Cue::Combo, Cue::Shielded]);
        assert!(l.step(&p).is_empty());
        p.hstate = HState::Chase;
        p.times_caught += 1;
        assert_eq!(l.step(&p), vec![Cue::HunterChase, Cue::CatCaught { player: true }]);
        p.hstate = HState::Patrol;
        p.escapes += 1;
        p.prey_caught += 1;
        assert_eq!(l.step(&p), vec![Cue::HunterLost, Cue::CatEscaped { player: true }, Cue::PreyPounced]);
        p.outcome = Some(3);
        assert_eq!(l.step(&p), vec![Cue::Lose]);
        let mut p2 = Prowl::new(Class::Cat, Kind::Dog);
        let mut l2 = ProwlListener::new(&p2);
        p2.outcome = Some(0);
        assert_eq!(l2.step(&p2), vec![Cue::Win], "clan 0 is the player's");
    }

    /// The air: a storm is rain, thunder and more wind; snow hushes the wind; tension has a
    /// floor per hunter state and rises with the hunt's own danger.
    #[test]
    fn the_air_and_the_tension_follow_the_weather_and_the_hunter() {
        let mut w = Weather::new(Season::Greenleaf);
        w.sky = Sky::Clear;
        w.wind_strength = 0.5;
        w.gust = 0.0;
        let clear = air(&w);
        assert_eq!(clear.rain, 0.0);
        assert_eq!(clear.storm, 0.0);
        assert!((clear.wind - 0.35).abs() < 1e-5);
        w.sky = Sky::Storm;
        let storm = air(&w);
        assert_eq!(storm.rain, 1.0);
        assert_eq!(storm.storm, 1.0);
        assert!(storm.wind > clear.wind);
        w.sky = Sky::Snow;
        let snow = air(&w);
        assert_eq!(snow.snow, 1.0);
        assert!(snow.wind < clear.wind, "snow hushes the wind");
        w.sky = Sky::Rain;
        assert!(air(&w).rain > 0.0 && air(&w).storm == 0.0);
        assert_eq!(tension(HState::Patrol, 0.0, 0.0), 0.0);
        assert_eq!(tension(HState::Chase, 0.0, 0.0), 1.0);
        assert!(tension(HState::Alert, 0.0, 0.0) >= 0.5);
        assert!(tension(HState::Patrol, 0.0, 1.0) > 0.7, "danger alone raises it");
        assert!(tension(HState::Patrol, 1.0, 1.0) <= 1.0);
        let g = Game::new_match(Kind::Hawk, Mode::Survive);
        let a = HuntListener::ambience(&g);
        assert_eq!(a.running, 0.0, "standing still at the start");
        assert!((0.0..=1.0).contains(&a.tension));
    }
}
