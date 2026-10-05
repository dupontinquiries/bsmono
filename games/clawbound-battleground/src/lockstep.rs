//! lockstep -- the rule that lets two machines play one match.
//!
//! Nobody sends the world. Each player sends only what they PRESSED, stamped
//! with the tick it is for; every machine runs the same simulation on the
//! same inputs and gets the same world. That only works if the simulation is
//! deterministic, so the first test here is that claim, tested rather than
//! asserted -- I had said "fixed-tick and seeded, which is what lockstep
//! needs" twice without checking.
//!
//! No sockets: [`Lockstep`] is fed inputs by whoever has them (the den's UDP
//! socket, a test, a replay file). Three rules, each with a test:
//!
//! 1. A tick advances only when EVERY player's input for it has arrived.
//!    A missing input stalls the match; it never guesses. Stalling is
//!    visible and recoverable, a guess is a silent fork.
//! 2. Inputs are scheduled `delay` ticks ahead, so normal latency is
//!    absorbed instead of stalling every frame.
//! 3. Every machine checksums its world at the same ticks. A mismatch is a
//!    desync, detected the tick it happens rather than when a player
//!    notices a cat walking through a wall.

use std::collections::BTreeMap;

use glam::Vec3;

use crate::sim::Game;

/// What one player pressed for one tick. Quantised: floats that differ in
/// their last bit between two machines would fork the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Input {
    /// Move direction, each axis -127..=127.
    pub mv: [i8; 2],
    pub attack: bool,
    pub sneak: bool,
    pub leap: bool,
    /// Ability slot and aim point in eighths of a unit.
    pub cast: Option<(u8, [i16; 2])>,
}

pub const WIRE_LEN: usize = 9;

impl Input {
    pub fn encode(&self) -> [u8; WIRE_LEN] {
        let flags = self.attack as u8 | (self.sneak as u8) << 1 | (self.leap as u8) << 2 | (self.cast.is_some() as u8) << 3;
        let (slot, aim) = self.cast.unwrap_or((0, [0, 0]));
        let (ax, az) = (aim[0].to_le_bytes(), aim[1].to_le_bytes());
        [self.mv[0] as u8, self.mv[1] as u8, flags, slot, ax[0], ax[1], az[0], az[1], 0xC1]
    }
    pub fn decode(b: &[u8]) -> Option<Input> {
        if b.len() != WIRE_LEN || b[8] != 0xC1 || b[2] & 0xF0 != 0 || b[3] > 3 {
            return None;
        }
        let cast = (b[2] & 8 != 0).then(|| (b[3], [i16::from_le_bytes([b[4], b[5]]), i16::from_le_bytes([b[6], b[7]])]));
        Some(Input { mv: [b[0] as i8, b[1] as i8], attack: b[2] & 1 != 0, sneak: b[2] & 2 != 0, leap: b[2] & 4 != 0, cast })
    }

    /// Put this input on god `slot`, the way the local shell does.
    pub fn apply(&self, game: &mut Game, slot: usize) {
        let dir = Vec3::new(self.mv[0] as f32 / 127.0, 0.0, self.mv[1] as f32 / 127.0);
        let Some(g) = game.gods.get_mut(slot) else { return };
        g.move_dir = dir;
        g.attack_held = self.attack;
        g.sneak = self.sneak;
        if self.leap {
            game.leap_queue.push((slot, dir));
        }
        if let Some((s, aim)) = self.cast {
            game.cast_queue.push((slot, s as u32, Vec3::new(aim[0] as f32 / 8.0, 0.0, aim[1] as f32 / 8.0)));
        }
    }
}

/// A digest of everything that decides the match. Bit-exact on purpose.
pub fn checksum(game: &Game) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |v: u32| {
        for b in v.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    };
    for g in &game.gods {
        for v in [g.pos.x, g.pos.y, g.pos.z, g.hp, g.yaw, g.respawn_t] {
            eat(v.to_bits());
        }
    }
    eat(game.minions.len() as u32);
    for m in &game.minions {
        for v in [m.pos.x, m.pos.z, m.hp] {
            eat(v.to_bits());
        }
    }
    for t in &game.towers {
        eat(t.hp.to_bits());
    }
    for l in &game.logs {
        eat(l.x.to_bits());
    }
    eat(game.projectiles.len() as u32);
    h
}

pub struct Lockstep {
    /// God slots driven by humans, in a fixed order every machine agrees on.
    pub slots: Vec<usize>,
    pub delay: u32,
    /// The next tick to simulate.
    pub tick: u32,
    pending: BTreeMap<u32, Vec<Option<Input>>>,
}

impl Lockstep {
    pub fn new(slots: Vec<usize>, delay: u32) -> Lockstep {
        let mut l = Lockstep { slots, delay, tick: 0, pending: BTreeMap::new() };
        // Nobody can have sent anything for the first `delay` ticks: they are
        // empty by agreement, or the match could never start.
        for t in 0..delay {
            l.pending.insert(t, vec![Some(Input::default()); l.slots.len()]);
        }
        l
    }

    /// The tick a press made NOW should be stamped with.
    pub fn stamp(&self) -> u32 {
        self.tick + self.delay
    }

    /// Record `player`'s input for `tick`. Late (already simulated), absurdly
    /// early, duplicate or out-of-range inputs are dropped: the first one
    /// for a tick wins, so a resend cannot change history.
    pub fn submit(&mut self, player: usize, tick: u32, input: Input) -> bool {
        if player >= self.slots.len() || tick < self.tick || tick > self.tick + 600 {
            return false;
        }
        let n = self.slots.len();
        let row = self.pending.entry(tick).or_insert_with(|| vec![None; n]);
        if row[player].is_some() {
            return false;
        }
        row[player] = Some(input);
        true
    }

    pub fn ready(&self) -> bool {
        self.pending.get(&self.tick).is_some_and(|r| r.iter().all(Option::is_some))
    }

    /// Who the match is waiting on, for a "waiting for Fox..." line.
    pub fn waiting_on(&self) -> Vec<usize> {
        match self.pending.get(&self.tick) {
            Some(r) => r.iter().enumerate().filter(|(_, i)| i.is_none()).map(|(p, _)| p).collect(),
            None => (0..self.slots.len()).collect(),
        }
    }

    /// Advance one tick if every input is in. Returns the checksum when it did.
    pub fn step(&mut self, game: &mut Game, dt: f32) -> Option<u64> {
        if !self.ready() {
            return None;
        }
        let row = self.pending.remove(&self.tick)?;
        for (p, input) in row.iter().enumerate() {
            input.unwrap_or_default().apply(game, self.slots[p]);
        }
        game.tick(dt);
        // the tick's events stay for the caller: the shell drains them into floaters,
        // announcements and the mixer, the same as after a solo `tick` (they are view-only;
        // the checksum below does not see them)
        self.tick += 1;
        Some(checksum(game))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        g.gods[5].ai = None; // a human on the other team too
        g
    }
    /// A deterministic script of "presses": wander, fight, leap, cast.
    fn script(player: usize, tick: u32) -> Input {
        let k = tick.wrapping_mul(2654435761).wrapping_add(player as u32 * 97);
        let side = if player == 0 { 1 } else { -1 };
        Input {
            mv: [((k >> 8) % 255) as i8, (side * 90) as i8],
            attack: tick % 3 != 0,
            sneak: (tick / 200) % 4 == 1,
            leap: tick % 173 == 0,
            cast: (tick % 97 == 0).then(|| (((tick / 97) % 4) as u8, [(k % 400) as i16 - 200, side as i16 * 300])),
        }
    }

    /// THE question. Two copies of the match, the same presses, thirty
    /// seconds of bots, waves, towers, leaps and casts: bit-identical or not.
    #[test]
    fn the_simulation_is_deterministic_under_identical_inputs() {
        let (mut a, mut b) = (game(), game());
        let (mut la, mut lb) = (Lockstep::new(vec![0, 5], 3), Lockstep::new(vec![0, 5], 3));
        let mut compared = 0;
        for t in 0..1800u32 {
            for p in 0..2 {
                la.submit(p, t + 3, script(p, t));
                lb.submit(p, t + 3, script(p, t));
            }
            let (ca, cb) = (la.step(&mut a, 1.0 / 60.0).expect("a ready"), lb.step(&mut b, 1.0 / 60.0).expect("b ready"));
            assert_eq!(ca, cb, "the two machines FORKED at tick {t}: lockstep cannot work until this is found");
            compared += 1;
        }
        assert_eq!(compared, 1800);
        assert!((a.gods[0].pos - Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]).gods[0].pos).length() > 3.0, "and the inputs really drove the cat somewhere");
    }

    /// Rule 1 and rule 3 together: inputs arriving late and out of order
    /// STALL the match and never fork it; and a fork, when forced, shows.
    #[test]
    fn a_missing_input_stalls_never_guesses_and_a_fork_is_detected() {
        let (mut a, mut b) = (game(), game());
        let (mut la, mut lb) = (Lockstep::new(vec![0, 5], 2), Lockstep::new(vec![0, 5], 2));
        for t in 0..2 {
            assert!(la.step(&mut a, 1.0 / 60.0).is_some() && lb.step(&mut b, 1.0 / 60.0).is_some(), "the first `delay` ticks are empty by agreement ({t})");
        }
        // Machine A hears only player 0 for tick 2.
        la.submit(0, 2, script(0, 2));
        assert_eq!(la.step(&mut a, 1.0 / 60.0), None, "one input short: it waits");
        assert_eq!(la.waiting_on(), vec![1], "and can say for whom");
        let before = checksum(&a);
        assert_eq!(la.step(&mut a, 1.0 / 60.0), None);
        assert_eq!(checksum(&a), before, "a stalled match does not move");
        // It arrives -- after a duplicate tried to rewrite player 0's press.
        assert!(!la.submit(0, 2, Input { attack: true, leap: true, ..Default::default() }), "the first input for a tick wins");
        assert!(la.submit(1, 2, script(1, 2)));
        // Machine B got them in the other order.
        lb.submit(1, 2, script(1, 2));
        lb.submit(0, 2, script(0, 2));
        assert_eq!(la.step(&mut a, 1.0 / 60.0), lb.step(&mut b, 1.0 / 60.0), "arrival order changes nothing");
        assert!(!la.submit(0, 1, script(0, 1)), "an input for a tick already simulated is history");
        assert!(!la.submit(7, 9, script(0, 9)), "and there is no player 7");
        // Force a fork: one machine's cat is a hair off.
        b.gods[0].pos.x += 0.001;
        assert_ne!(checksum(&a), checksum(&b), "a millimetre is a desync, and it shows at once");
    }

    #[test]
    fn an_input_survives_the_wire_and_junk_does_not() {
        let mut n = 0;
        for t in 0..400u32 {
            let i = script((t % 2) as usize, t);
            assert_eq!(Input::decode(&i.encode()), Some(i));
            n += 1;
        }
        assert_eq!(n, 400);
        assert_eq!(Input::decode(&[0; 9]), None, "no magic");
        assert_eq!(Input::decode(&[1, 2, 3]), None);
        let mut bad = Input::default().encode();
        bad[3] = 9;
        assert_eq!(Input::decode(&bad), None, "there is no ability slot 9");
    }
}
