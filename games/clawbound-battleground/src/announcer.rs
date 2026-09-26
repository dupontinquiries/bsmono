//! announcer -- the voice of the battleground calling the game's state.
//!
//! Pure: fed the sim's events (and the game, for who is dead), it decides
//! what is worth calling, and [`Announcer::next`] hands out one line at a
//! time so calls never talk over each other. The app plays the line (its
//! key is `announcer/<key>`; the words are in `voicescript`) and tells the
//! announcer how long it lasts.
//!
//! What it calls, per kill of an ancestor, the single most notable of: an
//! ace (a whole team down) > a shutdown (the victim was on a spree) >
//! rampage / triple / double kill (kills by one cat within
//! [`MULTI_WINDOW`]) > unstoppable / on the prowl (kills without dying) >
//! first blood > a plain kill. Plus towers, the Elders unshielded, great
//! beasts, disasters, the start and the end.

use std::collections::{HashMap, VecDeque};

use crate::consts::Team;
use crate::sim::{Event, Game};

/// Kills by one cat within this many seconds of each other chain.
pub const MULTI_WINDOW: f32 = 10.0;
/// A queued call older than this is dropped, not played late.
pub const STALE: f32 = 4.0;
/// Silence between two calls.
pub const GAP: f32 = 0.35;

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// `announcer/<key>` in the bank; a line in `voicescript`.
    pub key: &'static str,
    pub priority: u8,
    pub at: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Announcer {
    first_blood: bool,
    /// Kills since last death, per cat (by name: that is what Kill carries).
    streak: HashMap<String, u32>,
    /// (kills in the current chain, time of the last).
    chain: HashMap<String, (u32, f32)>,
    queue: VecDeque<Call>,
    busy_until: f32,
    ended: bool,
}

fn team_key(t: Team, blue: &'static str, red: &'static str) -> &'static str {
    if t == Team::Blue {
        blue
    } else {
        red
    }
}

impl Announcer {
    fn push(&mut self, key: &'static str, priority: u8, now: f32) {
        self.queue.push_back(Call { key, priority, at: now });
    }

    /// The intro has ended: play begins.
    pub fn start(&mut self, now: f32) {
        self.push("match_start", 9, now);
    }

    /// Take in one event.
    pub fn hear(&mut self, ev: &Event, game: &Game) {
        let now = game.t;
        match ev {
            Event::Kill { killer, victim, victim_team, is_god: true } => {
                let mut best: Option<(&'static str, u8)> = None;
                let mut offer = |k: &'static str, p: u8| {
                    if best.map_or(true, |b| p > b.1) {
                        best = Some((k, p));
                    }
                };
                // ace: every cat of the victim's team is down (the victim
                // counts as down: its respawn timer starts this tick)
                let team_down = game.gods.iter().enumerate().filter(|(_, g)| g.team == *victim_team).all(|(i, g)| g.respawn_t > 0.0 || game.god_name(i) == victim.as_str());
                if team_down && game.gods.iter().filter(|g| g.team == *victim_team).count() > 1 {
                    offer("ace", 10);
                }
                if self.streak.get(victim).copied().unwrap_or(0) >= 3 {
                    offer("shutdown", 8);
                }
                self.streak.insert(victim.clone(), 0);
                self.chain.remove(victim);
                let s = self.streak.entry(killer.clone()).or_insert(0);
                *s += 1;
                let streak = *s;
                let c = self.chain.entry(killer.clone()).or_insert((0, -100.0));
                if now - c.1 <= MULTI_WINDOW {
                    c.0 += 1;
                } else {
                    c.0 = 1;
                }
                c.1 = now;
                match c.0 {
                    2 => offer("double_kill", 6),
                    3 => offer("triple_kill", 7),
                    n if n >= 4 => offer("rampage", 9),
                    _ => {}
                }
                match streak {
                    3 => offer("spree", 5),
                    6 => offer("unstoppable", 7),
                    _ => {}
                }
                if !self.first_blood {
                    self.first_blood = true;
                    offer("first_blood", 6);
                }
                offer("kill", 1);
                let (k, p) = best.expect("offered");
                self.push(k, p, now);
            }
            Event::TowerDown { team, .. } => self.push(team_key(*team, "tower_down_frostfang", "tower_down_emberclaw"), 5, now),
            Event::TitanVulnerable { team } => self.push(team_key(*team, "titan_frostfang", "titan_emberclaw"), 8, now),
            Event::BossSpawn { .. } => self.push("boss_spawn", 4, now),
            Event::BossDown { .. } => self.push("boss_down", 6, now),
            Event::DisasterWarning { .. } => self.push("disaster", 7, now),
            _ => {}
        }
    }

    /// The match is over: `winner` won. `me`: the player's team, or None
    /// for a spectator (who hears which clan won).
    pub fn end(&mut self, winner: Team, me: Option<Team>, now: f32) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.queue.clear();
        let key = match me {
            Some(t) if t == winner => "victory",
            Some(_) => "defeat",
            None => team_key(winner, "frostfang_win", "emberclaw_win"),
        };
        self.push(key, 10, now);
    }

    /// The next call to play now, if the announcer is free: the highest
    /// priority waiting (oldest first among equals); stale ones dropped.
    pub fn next(&mut self, now: f32) -> Option<Call> {
        self.queue.retain(|c| now - c.at <= STALE);
        if now < self.busy_until || self.queue.is_empty() {
            return None;
        }
        let i = (0..self.queue.len()).max_by(|&a, &b| self.queue[a].priority.cmp(&self.queue[b].priority).then(self.queue[b].at.partial_cmp(&self.queue[a].at).unwrap()))?;
        self.queue.remove(i)
    }

    /// The call just started lasts `secs` (its clip's length; a short
    /// default when there is no clip).
    pub fn playing(&mut self, now: f32, secs: f32) {
        self.busy_until = now + secs.max(0.0) + GAP;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9])
    }
    fn kill(g: &Game, k: usize, v: usize) -> Event {
        Event::Kill { killer: g.god_name(k).to_string(), victim: g.god_name(v).to_string(), victim_team: g.gods[v].team, is_god: true }
    }
    fn drain(a: &mut Announcer, t: f32) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut now = t;
        while let Some(c) = a.next(now) {
            out.push(c.key);
            a.playing(now, 0.5);
            now += 1.0;
        }
        out
    }

    #[test]
    fn first_blood_then_plain_kills() {
        let mut g = game();
        let mut a = Announcer::default();
        a.hear(&kill(&g, 0, 5), &g);
        g.t = 30.0;
        a.hear(&kill(&g, 6, 1), &g);
        assert_eq!(drain(&mut a, 30.0), vec!["kill"], "first blood went stale; the second kill is plain");
        let mut g = game();
        let mut a = Announcer::default();
        a.hear(&kill(&g, 0, 5), &g);
        g.t = 0.1;
        assert_eq!(drain(&mut a, 0.1), vec!["first_blood"]);
    }

    #[test]
    fn chains_and_streaks_escalate_and_a_shutdown_ends_them() {
        let mut g = game();
        let mut a = Announcer::default();
        let mut heard = Vec::new();
        for (k, t) in [(5, 0.0f32), (6, 3.0), (7, 6.0), (8, 9.0)] {
            g.t = t;
            a.hear(&kill(&g, 0, k), &g);
            heard.extend(drain(&mut a, t));
        }
        assert_eq!(heard, vec!["first_blood", "double_kill", "triple_kill", "rampage"]);
        // cat 0 is on a spree of four; red cat 9 takes it down
        g.t = 40.0;
        a.hear(&kill(&g, 9, 0), &g);
        assert_eq!(drain(&mut a, 40.0), vec!["shutdown"]);
    }

    #[test]
    fn an_ace_is_the_biggest_call() {
        let mut g = game();
        let mut a = Announcer::default();
        for v in 6..10 {
            g.gods[v].respawn_t = 5.0; // four red already down
        }
        g.t = 50.0;
        a.hear(&kill(&g, 0, 5), &g); // the last one
        assert_eq!(drain(&mut a, 50.0), vec!["ace"]);
    }

    /// Calls never overlap: the next waits for the last to finish; the most
    /// important goes first; stale calls are dropped, not played late.
    #[test]
    fn one_call_at_a_time_most_important_first_none_late() {
        let g = game();
        let mut a = Announcer::default();
        a.hear(&Event::BossSpawn { kind: crate::bosses::BossKind::Boar, pos: glam::Vec3::ZERO }, &g);
        a.hear(&Event::TitanVulnerable { team: Team::Red }, &g);
        let c = a.next(0.0).unwrap();
        assert_eq!(c.key, "titan_emberclaw", "most important first");
        a.playing(0.0, 2.0);
        assert!(a.next(1.0).is_none(), "still talking");
        assert_eq!(a.next(2.5).unwrap().key, "boss_spawn");
        a.hear(&Event::TowerDown { pos: glam::Vec3::ZERO, team: Team::Blue }, &g);
        assert!(a.next(g.t + STALE + 1.0).is_none(), "stale: dropped");
    }

    #[test]
    fn the_end_is_victory_defeat_or_the_winning_clan() {
        let mut a = Announcer::default();
        a.end(Team::Blue, Some(Team::Blue), 0.0);
        assert_eq!(a.next(0.0).unwrap().key, "victory");
        let mut a = Announcer::default();
        a.end(Team::Blue, Some(Team::Red), 0.0);
        assert_eq!(a.next(0.0).unwrap().key, "defeat");
        let mut a = Announcer::default();
        a.end(Team::Red, None, 0.0);
        assert_eq!(a.next(0.0).unwrap().key, "emberclaw_win");
    }

    /// Every call the announcer can make is a line in the script.
    #[test]
    fn every_call_is_scripted() {
        let script = crate::voicescript::part("announcer").unwrap();
        for k in [
            "match_start", "first_blood", "kill", "double_kill", "triple_kill", "rampage", "spree", "unstoppable", "shutdown", "ace",
            "tower_down_frostfang", "tower_down_emberclaw", "titan_frostfang", "titan_emberclaw", "boss_spawn", "boss_down", "disaster",
            "victory", "defeat", "frostfang_win", "emberclaw_win",
        ] {
            assert!(script.lines.iter().any(|l| l.key == k), "the announcer can say '{k}' but it is not scripted");
        }
        assert_eq!(script.lines.len(), 21, "a scripted announcer line the game never calls");
    }
}
