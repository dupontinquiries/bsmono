//! emitters -- sounds that move with what made them.
//!
//! A cue is placed once, when it starts (`cues::place`). For a short hit
//! that is right; for anything that lasts, what made it moves on while the
//! sound stays behind: a stone's rumble left at the chute's head while the
//! stone rolls off, a cat's line left where it began while it runs, the
//! camera turning and the sound not turning with it.
//!
//! So a cue whose event has a mover ([`follow_for`]) is started with a
//! handle (`mixer::Play::handle`) and remembered here with what it follows
//! and how long it lasts. Every frame [`Emitters::places`] works out where
//! each one's source is now, and the gain and pan a listener at `ear` hears
//! it at -- the same law as `cues::place` -- and the app sends those to the
//! mixer (`mixer::Cmd::Place`), which glides the voice there.
//!
//! Headless: the mixer side is `mixer`, the wiring `app`.

use glam::Vec3;

use crate::mixer::Bus;
use crate::sim::{Event, Game};

/// What a sound follows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Follow {
    /// A cat (its lines, its casts, its heart).
    God(usize),
    /// One of the Quarry's stones, by its pair, side and run (`machine`).
    Ball { pair: usize, side: usize, run: u32 },
    /// A mudslide's front, by its owner (`clay`).
    Mudslide(usize),
}

/// The mover an event's sounds follow, if it has one.
pub fn follow_for(ev: &Event, game: &Game) -> Option<Follow> {
    match *ev {
        Event::Cast { god, .. } | Event::CastStart { god, .. } | Event::Heartbeat { god, .. } => Some(Follow::God(god)),
        Event::BoulderRoll { pos } => {
            // the ball let go there, this tick
            crate::machine::boulders_at(game.wake.seed, game.t)
                .into_iter()
                .min_by(|a, b| a.pos.distance(pos).total_cmp(&b.pos.distance(pos)))
                .map(|b| Follow::Ball { pair: b.pair, side: b.side, run: b.run })
        }
        Event::MudslideFx { pos, .. } => game.mudslides.iter().min_by(|a, b| a.leg_from.distance(pos).total_cmp(&b.leg_from.distance(pos))).map(|w| Follow::Mudslide(w.owner)),
        _ => None,
    }
}

/// Where a mover is now (None: gone -- the sound stays where it last was).
pub fn where_now(f: Follow, game: &Game) -> Option<Vec3> {
    match f {
        Follow::God(i) => game.gods.get(i).filter(|g| g.respawn_t <= 0.0).map(|g| g.pos),
        Follow::Ball { pair, side, run } => crate::machine::boulders_at(game.wake.seed, game.t).into_iter().find(|b| b.pair == pair && b.side == side && b.run == run).map(|b| b.pos),
        Follow::Mudslide(owner) => game.mudslides.iter().find(|w| w.owner == owner).map(|w| w.pos),
    }
}

/// A playing sound that follows something.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    pub handle: u32,
    pub follow: Follow,
    /// The cue's own gain (before distance), bus and speaker (a voice line
    /// out of range is silenced, not stopped: `cues::VOICE_RANGE`).
    pub gain: f32,
    pub bus: Bus,
    pub speaker: Option<usize>,
    /// Game time it ends.
    pub until: f32,
    /// Where its source last was.
    pub last: Vec3,
}

#[derive(Debug, Clone, Default)]
pub struct Emitters {
    pub list: Vec<Emitter>,
    next: u32,
}

impl Emitters {
    /// Remember a sound about to start: its handle for `Play::handle`.
    #[allow(clippy::too_many_arguments)]
    pub fn start(&mut self, follow: Follow, gain: f32, bus: Bus, speaker: Option<usize>, at: Vec3, now: f32, secs: f32) -> u32 {
        self.next = self.next.wrapping_add(1).max(1);
        let handle = self.next;
        self.list.push(Emitter { handle, follow, gain, bus, speaker, until: now + secs, last: at });
        handle
    }

    /// Every following sound's gain and pan for a listener at `ear` whose
    /// right is `right`, now; the finished ones forgotten.
    pub fn places(&mut self, game: &Game, ear: Vec3, right: Vec3) -> Vec<(u32, f32, f32)> {
        let now = game.t;
        self.list.retain(|e| e.until > now);
        let mut out = Vec::with_capacity(self.list.len());
        for e in self.list.iter_mut() {
            if let Some(p) = where_now(e.follow, game) {
                e.last = p;
            }
            let cue = crate::cues::Cue { keys: Vec::new(), bus: e.bus, gain: e.gain, pos: Some(e.last), speaker: e.speaker, priority: 0 };
            let (g, pan) = crate::cues::place(&cue, ear, right).unwrap_or((0.0, 0.0));
            out.push((e.handle, g, pan));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cat's sound follows the cat: it runs off to the listener's right
    /// and the sound pans right and fades; it dies and the sound stays
    /// where it fell; the sound is forgotten when it ends.
    #[test]
    fn a_sound_follows_its_cat() {
        let mut g = Game::new_teams(&[0], &[1]);
        g.gods[0].pos = Vec3::ZERO;
        let (ear, right) = (Vec3::new(0.0, 0.0, -5.0), Vec3::X);
        let mut em = Emitters::default();
        let h = em.start(Follow::God(0), 1.0, Bus::Sfx, None, g.gods[0].pos, g.t, 2.0);
        let p0 = em.places(&g, ear, right);
        assert_eq!(p0.len(), 1);
        assert_eq!(p0[0].0, h);
        assert!(p0[0].2.abs() < 0.05, "centred at first: {}", p0[0].2);
        g.gods[0].pos = Vec3::new(20.0, 0.0, -5.0);
        let p1 = em.places(&g, ear, right);
        assert!(p1[0].2 > 0.8, "not panned right as it ran right: {}", p1[0].2);
        assert!(p1[0].1 < p0[0].1, "not quieter further off");
        // dead: it stays where it last was
        g.gods[0].respawn_t = 5.0;
        g.gods[0].pos = Vec3::new(-50.0, 0.0, 0.0);
        let p2 = em.places(&g, ear, right);
        assert!((p2[0].2 - p1[0].2).abs() < 1e-4, "a dead cat's sound moved");
        g.t += 2.5;
        assert!(em.places(&g, ear, right).is_empty(), "outlived its clip");
    }

    /// A Quarry stone's rumble follows the stone down its track.
    #[test]
    fn a_rumble_follows_its_stone() {
        crate::map::with(crate::map::MapId::Maze, || {
            let mut g = Game::new_teams(&[0], &[1]);
            g.wake = crate::maze::Wake::new(&mut crate::sim::Rng::new(3));
            g.t = crate::maze::FIRST_TURN + crate::maze::TURN + crate::machine::ROLL_AT + 0.01;
            let b = crate::machine::boulders_at(g.wake.seed, g.t)[0];
            let f = follow_for(&Event::BoulderRoll { pos: b.pos }, &g).expect("no stone to follow");
            let at0 = where_now(f, &g).unwrap();
            g.t += 3.0;
            let at1 = where_now(f, &g).unwrap();
            assert!(at0.distance(at1) > 30.0, "the rumble's stone moved only {}", at0.distance(at1));
            assert!((at1 - crate::machine::boulders_at(g.wake.seed, g.t).into_iter().find(|x| x.side == b.side).unwrap().pos).length() < 1e-3);
        });
    }
}
