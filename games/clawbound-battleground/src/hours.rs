//! hours -- Hourwatch, the Watcher: a cat who keeps the hours, and can
//! turn them back.
//!
//! Every cat leaves a TRAIL: where it stood and how it was, a few times a
//! second, for the last [`TRAIL_SECS`] ([`Game::hours_tick`]). Hourwatch
//! reads it:
//!
//! * **Turn Back** (`Archetype::Rewind`): she is where she was `secs` ago,
//!   and as well as she was then -- her health then, if it was more (never
//!   less: a hurt she has since healed stays healed).
//! * **Send Back** (`Archetype::SendBack`): the enemy nearest her aim is
//!   where IT was `secs` ago -- whatever ground it gained, whatever it was
//!   winding up, gone -- and is struck.
//! * **Wind Forward** (`Archetype::WindForward`): every friend near her has
//!   the hours of its cooldowns wound on `secs`.
//! * **The Long Look** (`Archetype::LongLook`, the ultimate): she leaps into
//!   the stars and comes down at her aim, `flight` seconds later, and no
//!   blow can reach her up there. While she is aloft she sees the whole
//!   field BEHIND her -- every enemy on her own side of the line across the
//!   map where she is (offside, the way a lineswoman sees it: the nearer
//!   her own base, the more it is behind her) is shown up for `reveal`
//!   seconds (`GodUnit::revealed_t`: seen through walls and cloud, no
//!   stalking). The further forward she flies, the more of the field is
//!   behind her. And coming down she is seen herself: shown up for
//!   `exposed` seconds where she lands.
//!
//! Headless; the scene draws her trail-ghost and the stars, the cues play
//! the clock.

use std::collections::VecDeque;

use glam::Vec3;

use crate::consts::Team;
use crate::sim::{Event, Game, TargetRef};

/// How often a cat's trail is marked, per second, and how far back it goes.
pub const TRAIL_HZ: f32 = 10.0;
pub const TRAIL_SECS: f32 = 4.0;

/// One mark of a trail: when, where, and the health then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    pub t: f32,
    pub pos: Vec3,
    pub hp: f32,
}

/// Where a cat was `secs` ago (the oldest mark if its trail is shorter).
pub fn past(trail: &VecDeque<Mark>, now: f32, secs: f32) -> Option<Mark> {
    trail.iter().find(|m| m.t >= now - secs - 1e-3).or(trail.front()).copied()
}

/// The way from `team`'s base to the other's, flat: "forward".
pub fn forward(team: Team) -> Vec3 {
    let (a, b) = (team.base_pos(), team.other().base_pos());
    Vec3::new(b.x - a.x, 0.0, b.z - a.z).normalize_or_zero()
}

/// Is `p` behind a line across the field at `at`, for `team`: no further
/// forward than it?
pub fn behind(team: Team, at: Vec3, p: Vec3) -> bool {
    Vec3::new(p.x - at.x, 0.0, p.z - at.z).dot(forward(team)) <= 0.0
}

impl Game {
    /// Once a tick: every cat's trail marked; Hourwatch's long look kept
    /// up while she is aloft, and her landing.
    pub fn hours_tick(&mut self, dt: f32) {
        let t = self.t;
        for g in self.gods.iter_mut() {
            if g.respawn_t > 0.0 {
                g.trail.clear();
                continue;
            }
            if g.trail.back().is_none_or(|m| t - m.t >= 1.0 / TRAIL_HZ - 1e-4) {
                g.trail.push_back(Mark { t, pos: g.pos, hp: g.hp });
            }
            while g.trail.front().is_some_and(|m| t - m.t > TRAIL_SECS) {
                g.trail.pop_front();
            }
        }
        for i in 0..self.gods.len() {
            if self.gods[i].aloft_t <= 0.0 {
                continue;
            }
            let (team, at, reveal) = (self.gods[i].team, self.gods[i].pos, self.gods[i].look_reveal);
            for j in 0..self.gods.len() {
                let e = &mut self.gods[j];
                if e.team != team && e.respawn_t <= 0.0 && behind(team, at, e.pos) {
                    e.revealed_t = e.revealed_t.max(reveal);
                }
            }
            let g = &mut self.gods[i];
            g.aloft_t -= dt;
            if g.aloft_t <= 0.0 {
                g.aloft_t = 0.0;
                g.revealed_t = g.revealed_t.max(g.look_exposed);
                let pos = g.pos;
                self.events.push(Event::LookLand { god: i, pos });
            }
        }
    }

    /// Turn Back: Hourwatch where she was `secs` ago, with the health she
    /// had then if it was more.
    pub fn turn_back(&mut self, gi: usize, secs: f32) {
        let now = self.t;
        let Some(m) = past(&self.gods[gi].trail, now, secs) else { return };
        let from = self.gods[gi].pos;
        let to = crate::map::onto_floor(m.pos, crate::consts::GOD_RADIUS);
        let to = self.blockers.resolve(Vec3::new(to.x, crate::map::ground_y(to.x, to.z), to.z), crate::consts::GOD_RADIUS);
        let g = &mut self.gods[gi];
        g.pos = to;
        g.hp = g.hp.max(m.hp.min(g.max_hp));
        g.dash = None;
        g.leap = None;
        g.slow_t = 0.0;
        g.trail.clear();
        self.events.push(Event::TimeTurned { god: gi, from, to, back: true });
    }

    /// Send Back: the enemy nearest `aim` (within `range` of her, and 5 u
    /// of the aim, one her team can see) where it was `secs` ago, its
    /// wind-up lost, struck for `dmg`. Returns who, if anyone.
    pub fn send_back(&mut self, gi: usize, aim: Vec3, range: f32, dmg: f32, secs: f32) -> Option<usize> {
        let (team, me) = (self.gods[gi].team, self.gods[gi].pos);
        let flat = |p: Vec3| Vec3::new(p.x, 0.0, p.z);
        let j = (0..self.gods.len())
            .filter(|&j| {
                let e = &self.gods[j];
                e.team != team && e.respawn_t <= 0.0 && e.aloft_t <= 0.0 && self.sees(team, j) && flat(e.pos).distance(flat(me)) <= range && flat(e.pos).distance(flat(aim)) <= 5.0
            })
            .min_by(|&a, &b| flat(self.gods[a].pos).distance(flat(aim)).total_cmp(&flat(self.gods[b].pos).distance(flat(aim))))?;
        let now = self.t;
        let m = past(&self.gods[j].trail, now, secs)?;
        let from = self.gods[j].pos;
        let to = crate::map::onto_floor(m.pos, crate::consts::GOD_RADIUS);
        let to = self.blockers.resolve(Vec3::new(to.x, crate::map::ground_y(to.x, to.z), to.z), crate::consts::GOD_RADIUS);
        let e = &mut self.gods[j];
        e.pos = to;
        e.casting = None;
        e.dash = None;
        e.leap = None;
        e.trail.clear();
        self.events.push(Event::TimeTurned { god: j, from, to, back: false });
        if dmg > 0.0 {
            self.deal_damage(TargetRef::God(j), dmg, true, Some(gi), 0.0, 0.0);
        }
        Some(j)
    }

    /// Wind Forward: every friend within `radius` of her (not herself) has
    /// its cooldowns wound on `secs`.
    pub fn wind_forward(&mut self, gi: usize, radius: f32, secs: f32) -> usize {
        let (team, me) = (self.gods[gi].team, self.gods[gi].pos);
        let mut n = 0;
        for (j, g) in self.gods.iter_mut().enumerate() {
            if j == gi || g.team != team || g.respawn_t > 0.0 || g.pos.distance(me) > radius {
                continue;
            }
            for cd in g.cds.iter_mut() {
                *cd = (*cd - secs).max(0.0);
            }
            n += 1;
        }
        self.events.push(Event::WindFx { pos: me, radius });
        n
    }

    /// The Long Look: up into the stars from where she stands, down at
    /// `dest` `flight` seconds later; aloft, out of reach, looking back.
    #[allow(clippy::too_many_arguments)]
    pub fn long_look(&mut self, gi: usize, dest: Vec3, flight: f32, reveal: f32, exposed: f32) {
        let from = self.gods[gi].pos;
        let g = &mut self.gods[gi];
        g.dash = Some(crate::sim::DashState { from, to: dest, t: 0.0, dur: flight, dmg: 0.0, arc: LOOK_ARC, blast: None });
        g.aloft_t = flight;
        g.look_reveal = reveal;
        g.look_exposed = exposed;
        self.events.push(Event::LookUp { god: gi, pos: from, to: dest });
    }
}

/// How high the long look goes: out of the fight, into the stars.
pub const LOOK_ARC: f32 = 38.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::ROSTER;

    fn hourwatch() -> u32 {
        ROSTER.iter().find(|d| d.name == "Hourwatch").expect("Hourwatch on the roster").id
    }

    /// Hourwatch (blue, 0) and an enemy (red, 1) 10 u ahead of her, their
    /// trails walked: both 1 u a tenth of a second along +x, for `secs`.
    fn walked(secs: f32) -> Game {
        let mut g = Game::new_teams(&[hourwatch()], &[1]);
        g.gods[0].pos = Vec3::new(-20.0, 0.0, 0.0);
        g.gods[1].pos = Vec3::new(-10.0, 0.0, 0.0);
        let steps = (secs * TRAIL_HZ) as usize;
        for _ in 0..steps {
            g.t += 0.1;
            g.gods[0].pos.x += 1.0;
            g.gods[1].pos.x += 1.0;
            g.hours_tick(0.1);
        }
        g
    }

    /// The trail keeps TRAIL_SECS of marks at TRAIL_HZ, oldest first, and
    /// `past` finds the one `secs` back.
    #[test]
    fn a_trail_remembers_the_last_few_seconds() {
        let g = walked(6.0);
        let tr = &g.gods[0].trail;
        assert!((tr.len() as f32 - TRAIL_SECS * TRAIL_HZ).abs() <= 1.5, "{} marks", tr.len());
        let m = past(tr, g.t, 3.0).unwrap();
        assert!((g.gods[0].pos.x - m.pos.x - 30.0).abs() < 1.5, "3 s back is 30 u back: {} vs {}", m.pos.x, g.gods[0].pos.x);
    }

    /// Turn Back: 3 s back along her way, and the health she had then --
    /// but never less than she has.
    #[test]
    fn turn_back_returns_her_and_her_health() {
        let mut g = walked(5.0);
        let hp0 = g.gods[0].hp;
        g.gods[0].hp = hp0 * 0.4;
        let x = g.gods[0].pos.x;
        g.turn_back(0, 3.0);
        assert!((x - g.gods[0].pos.x - 30.0).abs() < 1.5, "back to {}", g.gods[0].pos.x);
        assert!((g.gods[0].hp - hp0).abs() < 1.0, "health then: {}", g.gods[0].hp);
        // healed since: kept
        let mut g = walked(5.0);
        g.gods[0].trail.iter_mut().for_each(|m| m.hp = 10.0);
        let hp = g.gods[0].hp;
        g.turn_back(0, 3.0);
        assert_eq!(g.gods[0].hp, hp, "turned back into a wound");
    }

    /// Send Back: the enemy by the aim goes back 3 s along its way, loses
    /// its wind-up and is struck; one out of range, or away from the aim,
    /// is not touched.
    #[test]
    fn send_back_returns_the_enemy_by_the_aim() {
        let mut g = walked(5.0);
        let (x, hp) = (g.gods[1].pos.x, g.gods[1].hp);
        let me = g.gods[0].pos;
        let aim = g.gods[1].pos + Vec3::new(1.0, 0.0, 1.0);
        assert!(me.distance(aim) < 16.0);
        assert_eq!(g.send_back(0, aim, 16.0, 60.0, 3.0), Some(1));
        assert!((x - g.gods[1].pos.x - 30.0).abs() < 1.5, "sent back to {} from {x}", g.gods[1].pos.x);
        assert!(g.gods[1].hp < hp, "not struck");
        let mut g = walked(5.0);
        let x = g.gods[1].pos.x;
        let aim = g.gods[1].pos + Vec3::new(0.0, 0.0, 8.0);
        assert_eq!(g.send_back(0, aim, 16.0, 60.0, 3.0), None, "sent back from 8 u off the aim");
        assert_eq!(g.gods[1].pos.x, x);
        let mut g = walked(5.0);
        let aim = g.gods[1].pos;
        assert_eq!(g.send_back(0, aim, 1.0, 60.0, 3.0), None, "sent back out of range");
    }

    /// Wind Forward: a friend in reach has every cooldown wound on (never
    /// below 0); one out of reach, an enemy, and she herself do not.
    #[test]
    fn wind_forward_winds_on_friends_only() {
        let mut g = Game::new_teams(&[hourwatch(), 0, 1], &[2]);
        g.gods[1].pos = g.gods[0].pos + Vec3::new(3.0, 0.0, 0.0);
        g.gods[2].pos = g.gods[0].pos + Vec3::new(40.0, 0.0, 0.0);
        g.gods[3].pos = g.gods[0].pos + Vec3::new(-3.0, 0.0, 0.0);
        for k in 0..4 {
            for c in 0..4 {
                g.gods[k].cds[c] = 3.0 + c as f32 * 5.0;
            }
        }
        assert_eq!(g.wind_forward(0, 9.0, 5.0), 1);
        assert_eq!(g.gods[1].cds, [0.0, 3.0, 8.0, 13.0]);
        for k in [0usize, 2, 3] {
            assert_eq!(g.gods[k].cds, [3.0, 8.0, 13.0, 18.0], "cat {k} wound on");
        }
    }

    /// The Long Look: aloft for its flight and out of reach there; every
    /// enemy behind her line shown up (and none ahead of it), and she shown
    /// up once she is down.
    #[test]
    fn the_long_look_sees_behind_her_and_shows_her_landing() {
        let mut g = Game::new_teams(&[hourwatch()], &[1, 2]);
        let fwd = forward(g.gods[0].team);
        let me = Vec3::ZERO;
        g.gods[0].pos = me;
        g.gods[1].pos = me - fwd * 30.0 + Vec3::new(fwd.z, 0.0, -fwd.x) * 40.0; // behind her, far to the side
        g.gods[2].pos = me + fwd * 25.0; // ahead
        g.long_look(0, me + fwd * 20.0, 2.0, 6.0, 4.0);
        g.hours_tick(1.0 / 60.0);
        assert!(g.gods[1].revealed_t > 5.0, "the one behind her not shown up");
        assert_eq!(g.gods[2].revealed_t, 0.0, "the one ahead shown up");
        let hp = g.gods[0].hp;
        g.deal_damage(TargetRef::God(0), 500.0, true, Some(1), 0.0, 0.0);
        assert_eq!(g.gods[0].hp, hp, "struck in the stars");
        assert_eq!(g.gods[0].revealed_t, 0.0, "shown up before she lands");
        for _ in 0..130 {
            g.t += 1.0 / 60.0;
            g.hours_tick(1.0 / 60.0);
        }
        assert_eq!(g.gods[0].aloft_t, 0.0);
        assert!(g.gods[0].revealed_t > 3.5, "not shown up where she landed");
        assert!(g.events.iter().any(|e| matches!(e, Event::LookLand { .. })));
    }
}
