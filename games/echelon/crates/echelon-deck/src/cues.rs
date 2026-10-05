//! The moments of a match named, for the ears: read off the world between two frames of
//! the deck (the sim keeps no event log beyond "defeated"), so the tape, the window and a
//! LAN mirror all hear the same things from the same state. Pure: a `Listener` holds what
//! it saw last frame and says what changed.
//!
//! What is heard, for the deck's own player (`me`): an order acknowledged (the deck emits
//! that itself when it sends a command), a unit trained, a structure finished, one of
//! ours lost, blows landing on either side (one tick per [`HIT_EVERY`] ticks while any
//! hit point falls, so a battle is a crackle rather than a wall), and the banner.

use echelon_data::Kind;
use echelon_sim::World;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cue {
    /// A command went out.
    Ack,
    /// One of ours finished training.
    Trained,
    /// One of our structures finished building.
    Built,
    /// One of ours died (a unit or a structure).
    Lost,
    /// Hit points fell somewhere this frame: `ours` if any of them were ours.
    Hit { ours: bool },
    Victory,
    Defeat,
}

/// Ticks between two `Hit` cues during a sustained fight.
pub const HIT_EVERY: u32 = 11;

/// What a frame of the world looks like to the listener.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Seen {
    /// our finished units (not workers' sites: `progress == 0`)
    units: usize,
    /// our finished structures
    structures: usize,
    /// everything of ours alive
    alive: usize,
    /// hit points, ours and theirs, in whole points
    hp_ours: i64,
    hp_theirs: i64,
    banner: bool,
}

fn seen(w: &World, me: u8) -> Seen {
    let mut s = Seen::default();
    for e in &w.ents {
        if !e.alive || e.def == u16::MAX {
            continue;
        }
        let d = w.pack.def(e.def);
        let hp = e.hp.to_int_floor() as i64;
        if e.player == me {
            s.alive += 1;
            s.hp_ours += hp;
            if e.progress == 0 {
                match d.kind {
                    Kind::Structure => s.structures += 1,
                    Kind::Unit | Kind::Worker => s.units += 1,
                }
            }
        } else {
            s.hp_theirs += hp;
        }
    }
    s
}

#[derive(Clone, Debug, Default)]
pub struct Listener {
    last: Option<Seen>,
    last_hit_tick: Option<u32>,
}

impl Listener {
    pub fn new() -> Listener {
        Listener::default()
    }

    /// Forget the last frame: the next call sees a fresh match, not a wipe-out (for a deck
    /// that rebuilds its world; today's does not).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn reset(&mut self) {
        *self = Listener::default();
    }

    /// The cues since the last call, in a fixed order (built before trained before lost
    /// before the hits, the banner last).
    pub fn step(&mut self, w: &World, me: u8, banner: Option<&str>) -> Vec<Cue> {
        let mut now = seen(w, me);
        now.banner = banner.is_some();
        let mut out = Vec::new();
        if let Some(prev) = self.last {
            if now.structures > prev.structures {
                out.push(Cue::Built);
            }
            if now.units > prev.units {
                out.push(Cue::Trained);
            }
            if now.alive < prev.alive {
                out.push(Cue::Lost);
            }
            let ours_fell = now.hp_ours < prev.hp_ours;
            let theirs_fell = now.hp_theirs < prev.hp_theirs;
            if ours_fell || theirs_fell {
                let due = self.last_hit_tick.map_or(true, |t| w.tick.saturating_sub(t) >= HIT_EVERY);
                if due {
                    self.last_hit_tick = Some(w.tick);
                    out.push(Cue::Hit { ours: ours_fell });
                }
            }
            if now.banner && !prev.banner {
                out.push(match banner {
                    Some("VICTORY") => Cue::Victory,
                    _ => Cue::Defeat,
                });
            }
        }
        self.last = Some(now);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_map, load_pack};
    use echelon_sim::fx::Fx;

    fn world() -> World {
        let pack = load_pack();
        let r0 = pack.race_index("Genetron").unwrap();
        let r1 = pack.race_index("Xayid").unwrap();
        World::new(pack, load_map("cauldron").0, [r0, r1], 7)
    }

    #[test]
    fn the_first_frame_is_silent_and_a_quiet_frame_says_nothing() {
        let w = world();
        let mut l = Listener::new();
        assert!(l.step(&w, 0, None).is_empty(), "nothing to compare the first frame with");
        assert!(l.step(&w, 0, None).is_empty());
    }

    #[test]
    fn a_death_a_hit_and_the_banner_are_heard_once_each() {
        let mut w = world();
        let mut l = Listener::new();
        l.step(&w, 0, None);
        // one of ours takes a blow
        let mine = w.ents.iter().position(|e| e.alive && e.player == 0 && e.def != u16::MAX).unwrap();
        let before = w.ents[mine].hp;
        w.ents[mine].hp = before.sub(Fx::from_int(5));
        assert_eq!(l.step(&w, 0, None), vec![Cue::Hit { ours: true }]);
        // more blows inside the window are one crackle, not one per frame
        w.ents[mine].hp = before.sub(Fx::from_int(10));
        assert!(l.step(&w, 0, None).is_empty(), "rate-limited");
        w.tick += HIT_EVERY;
        w.ents[mine].hp = before.sub(Fx::from_int(15));
        assert_eq!(l.step(&w, 0, None), vec![Cue::Hit { ours: true }]);
        // theirs falls: a hit that is not ours
        let theirs = w.ents.iter().position(|e| e.alive && e.player == 1 && e.def != u16::MAX).unwrap();
        w.tick += HIT_EVERY;
        w.ents[theirs].hp = w.ents[theirs].hp.sub(Fx::from_int(3));
        assert_eq!(l.step(&w, 0, None), vec![Cue::Hit { ours: false }]);
        // one of ours dies: Lost (its hit points leave the count too, but a death is not a hit)
        w.tick += HIT_EVERY;
        w.ents[mine].alive = false;
        let cues = l.step(&w, 0, None);
        assert!(cues.contains(&Cue::Lost), "{cues:?}");
        // the banner, once
        assert_eq!(l.step(&w, 0, Some("VICTORY")), vec![Cue::Victory]);
        assert!(l.step(&w, 0, Some("VICTORY")).is_empty(), "the banner is heard when it appears, not while it stays");
        let mut l2 = Listener::new();
        l2.step(&w, 0, None);
        assert_eq!(l2.step(&w, 0, Some("DEFEAT")), vec![Cue::Defeat]);
    }

    #[test]
    fn a_unit_arriving_is_trained_and_a_structure_finishing_is_built() {
        let mut w = world();
        let mut l = Listener::new();
        l.step(&w, 0, None);
        // a worker of ours is cloned alive: trained
        let mine = w.ents.iter().position(|e| e.alive && e.player == 0 && w.pack.def(e.def).kind == Kind::Worker).unwrap();
        let worker = w.ents[mine].clone();
        w.ents.push(worker);
        assert_eq!(l.step(&w, 0, None), vec![Cue::Trained]);
        // a structure site of ours under construction is nothing yet; finishing it is Built
        let site = w.ents.iter().position(|e| e.alive && e.player == 0 && w.pack.def(e.def).kind == Kind::Structure).unwrap();
        let mut s = w.ents[site].clone();
        s.progress = 100;
        w.ents.push(s);
        assert!(l.step(&w, 0, None).is_empty(), "a site is not a structure yet");
        let last = w.ents.len() - 1;
        w.ents[last].progress = 0;
        assert_eq!(l.step(&w, 0, None), vec![Cue::Built]);
        // reset: the same world is a fresh start, not a wipe-out
        l.reset();
        assert!(l.step(&w, 0, None).is_empty());
    }
}
