//! kit -- Sproutkit's gifts: the litter, the giggles and the dance party.
//!
//! * **Litter** (`Archetype::Litter`): kitten doubles ([`Kitling`]) burst
//!   out in a fan toward the aim. Each runs for the nearest enemy cat or
//!   minion within [`SEEK`] (else on along its line); one that reaches its
//!   quarry nips it and pops, leaving it giggling for [`NIP_GIGGLE`]. Any
//!   left when their time runs out pop where they stand.
//! * **Giggles** (`GodUnit::giggle_t`, `Minion::giggle_t`): a giggling cat
//!   cannot auto-attack or cast (a wind-up under way is lost), and its paws
//!   wander -- whoever steers it, the way it goes swings to and fro
//!   ([`confused`]). A giggling minion does not attack.
//! * **Dance party** (`ZoneKind::Dance`): while the glitter-ball hangs,
//!   every enemy under it dances: no attacks, no casts, and it shuffles at
//!   [`DANCE_MOVE`] of its speed. When the music stops for it -- the party
//!   ends, or it shuffles out -- it drags its paws: the ability's `slow` for
//!   its `slow_secs`.
//!
//! Headless; the scene draws the kittens, the ball and the dancers.

use glam::Vec3;

use crate::consts::Team;
use crate::sim::{Event, Game, TargetRef, Zone, ZoneKind};

/// How far a kitten looks for someone to run at.
pub const SEEK: f32 = 8.0;
/// Close enough to nip.
pub const NIP: f32 = 1.3;
/// How long a nip leaves its victim giggling.
pub const NIP_GIGGLE: f32 = 0.8;
/// The fan the litter bursts out in, degrees either side of the aim.
pub const FAN_DEG: f32 = 32.0;
/// A dancer's share of its speed.
pub const DANCE_MOVE: f32 = 0.3;
/// How long a dancer keeps dancing after it last stood under the ball.
pub const DANCE_HOLD: f32 = 0.3;
/// How far the ball's zone lands from the caster, at most.
pub const PARTY_RANGE: f32 = 14.0;
/// Sproutkit's litter is drawn at this share of his size.
pub const KITLING_SCALE: f32 = 0.6;
/// The glitter-ball hangs this high over its dance floor.
pub const BALL_HEIGHT: f32 = 7.5;
/// How far a giggling cat's paws wander from where it means to go, at the
/// peak of the swing (radians).
pub const WANDER: f32 = 1.5;

/// One of the litter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kitling {
    pub team: Team,
    /// The Sproutkit it came from (for the kill credit and its look).
    pub owner: usize,
    pub pos: Vec3,
    /// The way it runs when there is no one to run at.
    pub dir: Vec3,
    pub speed: f32,
    pub dmg: f32,
    pub life: f32,
    /// Seconds alive, for the scene's hop.
    pub age: f32,
}

/// Where a giggling cat actually goes when it means to go `dir`: swung to
/// and fro, up to [`WANDER`] radians, on its own rhythm (cat `i`, time `t`).
pub fn confused(dir: Vec3, giggle_t: f32, t: f32, i: usize) -> Vec3 {
    if giggle_t <= 0.0 || dir.length_squared() < 1e-6 {
        return dir;
    }
    let a = WANDER * ((t * 3.1 + i as f32 * 1.7).sin() * 0.7 + (t * 7.3 + i as f32).sin() * 0.3);
    let (s, c) = a.sin_cos();
    Vec3::new(dir.x * c - dir.z * s, dir.y, dir.x * s + dir.z * c)
}

impl Game {
    /// Can cat `i` act (attack, cast) -- not giggling, not dancing?
    pub fn kit_free(&self, i: usize) -> bool {
        let g = &self.gods[i];
        g.giggle_t <= 0.0 && g.dance_t <= 0.0
    }

    /// The litter bursts out of `gi` toward `dir`.
    pub fn litter(&mut self, gi: usize, dir: Vec3, count: u32, dmg: f32, speed: f32, life: f32) {
        let (from, team) = (self.gods[gi].pos, self.gods[gi].team);
        let base = dir.z.atan2(dir.x);
        for k in 0..count {
            let u = if count > 1 { k as f32 / (count - 1) as f32 - 0.5 } else { 0.0 };
            let a = base + u * 2.0 * FAN_DEG.to_radians();
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            self.kitlings.push(Kitling { team, owner: gi, pos: from + d * 0.8, dir: d, speed, dmg, life, age: 0.0 });
        }
    }

    /// Everyone within `radius` of `gi` giggles for `secs` (and takes `dmg`).
    pub fn giggle(&mut self, gi: usize, radius: f32, dmg: f32, secs: f32) {
        let g = &self.gods[gi];
        let (from, team, pen_pct, pen_flat) = (g.pos, g.team, g.pen_pct, g.pen_flat);
        // Everyone giggles first, then the damage lands, minions from the
        // highest index down: a minion the damage kills is REMOVED, shifting
        // every later index, and the loop that set the giggle as it dealt
        // the damage indexed past the end (a 3v3 on the Drowned Halls found
        // it: "index out of bounds: the len is 2 but the index is 2").
        let mut targets = self.aoe_targets(from, radius, team);
        for &t in &targets {
            match t {
                TargetRef::God(j) => {
                    self.gods[j].giggle_t = self.gods[j].giggle_t.max(secs);
                    // a laugh breaks a wind-up
                    self.gods[j].casting = None;
                }
                TargetRef::Minion(m) if m < self.minions.len() => self.minions[m].giggle_t = self.minions[m].giggle_t.max(secs),
                _ => {}
            }
        }
        if dmg > 0.0 {
            targets.sort_by_key(|t| std::cmp::Reverse(match t {
                TargetRef::Minion(m) => *m as i64,
                _ => -1,
            }));
            for t in targets {
                self.deal_damage(t, dmg, true, Some(gi), pen_pct, pen_flat);
            }
        }
    }

    /// The glitter-ball goes up over `at`.
    pub fn dance_party(&mut self, gi: usize, at: Vec3, radius: f32, duration: f32, slow: f32, slow_secs: f32, color: [f32; 3]) {
        let team = self.gods[gi].team;
        self.zones.push(Zone { team, kind: ZoneKind::Dance { until: duration, slow, slow_secs, owner: gi }, pos: at, radius, color });
    }

    /// Each tick: the litter runs and nips, dancers under a ball dance, and
    /// giggles and dances wear off (the long drag after a dance begins).
    pub fn kit_tick(&mut self, dt: f32) {
        self.kitlings_tick(dt);
        // under a ball: dance (both cats and minions)
        let parties: Vec<(Team, Vec3, f32, f32, f32)> = self
            .zones
            .iter()
            .filter_map(|z| match z.kind {
                ZoneKind::Dance { slow, slow_secs, .. } => Some((z.team, z.pos, z.radius, slow, slow_secs)),
                _ => None,
            })
            .collect();
        for (team, pos, radius, slow, slow_secs) in parties {
            for g in self.gods.iter_mut() {
                if g.team != team && g.respawn_t <= 0.0 && flat(g.pos - pos) <= radius {
                    g.dance_t = g.dance_t.max(DANCE_HOLD);
                    g.dance_after = (slow, slow_secs);
                    g.casting = None;
                }
            }
            for m in self.minions.iter_mut() {
                if m.team != team && flat(m.pos - pos) <= radius {
                    m.dance_t = m.dance_t.max(DANCE_HOLD);
                }
            }
        }
        for g in self.gods.iter_mut() {
            if g.giggle_t > 0.0 {
                g.giggle_t = (g.giggle_t - dt).max(0.0);
            }
            if g.dance_t > 0.0 {
                g.dance_t -= dt;
                if g.dance_t <= 0.0 {
                    // the music stopped: the long drag home
                    g.dance_t = 0.0;
                    let (slow, secs) = g.dance_after;
                    g.slow = g.slow.max(slow);
                    g.slow_t = g.slow_t.max(secs);
                }
            }
        }
        for m in self.minions.iter_mut() {
            m.giggle_t = (m.giggle_t - dt).max(0.0);
            m.dance_t = (m.dance_t - dt).max(0.0);
        }
    }

    fn kitlings_tick(&mut self, dt: f32) {
        let mut pops: Vec<(usize, Option<TargetRef>)> = Vec::new();
        for k in 0..self.kitlings.len() {
            let kl = self.kitlings[k];
            // whom to run at: the nearest enemy cat or minion in sight
            let mut best: Option<(TargetRef, Vec3, f32)> = None;
            for (j, g) in self.gods.iter().enumerate() {
                if g.team != kl.team && g.respawn_t <= 0.0 && self.sees(kl.team, j) {
                    let d = flat(g.pos - kl.pos);
                    if d < SEEK && best.map_or(true, |b| d < b.2) {
                        best = Some((TargetRef::God(j), g.pos, d));
                    }
                }
            }
            for (j, m) in self.minions.iter().enumerate() {
                let d = flat(m.pos - kl.pos);
                if m.team != kl.team && d < SEEK && best.map_or(true, |b| d < b.2) {
                    best = Some((TargetRef::Minion(j), m.pos, d));
                }
            }
            let kl = &mut self.kitlings[k];
            kl.age += dt;
            kl.life -= dt;
            if let Some((t, _, d)) = best {
                if d <= NIP {
                    pops.push((k, Some(t)));
                    continue;
                }
            }
            let dir = best.map_or(kl.dir, |(_, p, _)| Vec3::new(p.x - kl.pos.x, 0.0, p.z - kl.pos.z).normalize_or(kl.dir));
            kl.dir = dir;
            let next = crate::map::onto_floor(kl.pos + dir * kl.speed * dt, 0.3);
            kl.pos = Vec3::new(next.x, crate::map::ground_y(next.x, next.z), next.z);
            if kl.life <= 0.0 {
                pops.push((k, None));
            }
        }
        // pop, highest index first; a nip lands after (damage may remove a
        // minion and shift the rest)
        pops.sort_by(|a, b| b.0.cmp(&a.0));
        let mut nips = Vec::new();
        for (k, t) in pops {
            let kl = self.kitlings.remove(k);
            self.events.push(Event::KitlingPop { pos: kl.pos });
            if let Some(t) = t {
                nips.push((kl, t));
            }
        }
        for (kl, t) in nips {
            match t {
                TargetRef::God(j) => self.gods[j].giggle_t = self.gods[j].giggle_t.max(NIP_GIGGLE),
                TargetRef::Minion(m) if m < self.minions.len() => self.minions[m].giggle_t = self.minions[m].giggle_t.max(NIP_GIGGLE),
                _ => {}
            }
            let (pp, pf) = self.gods.get(kl.owner).map_or((0.0, 0.0), |g| (g.pen_pct, g.pen_flat));
            if matches!(t, TargetRef::Minion(m) if m >= self.minions.len()) {
                continue;
            }
            self.deal_damage(t, kl.dmg, true, Some(kl.owner), pp, pf);
        }
    }
}

fn flat(v: Vec3) -> f32 {
    Vec3::new(v.x, 0.0, v.z).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A giggle that kills some of the minions it catches: the dead are
    /// removed mid-loop, and the rest still giggle (it indexed past the end).
    #[test]
    fn a_killing_giggle_does_not_lose_count_of_the_minions() {
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        g.next_wave_t = 0.0;
        g.tick(1.0 / 60.0);
        g.next_wave_t = 1e9;
        let red: Vec<usize> = (0..g.minions.len()).filter(|&m| g.minions[m].team == Team::Red).collect();
        assert!(red.len() >= 3);
        let at = g.gods[0].pos;
        for (k, &m) in red.iter().enumerate() {
            g.minions[m].pos = at + Vec3::new(1.0 + k as f32 * 0.3, 0.0, 0.0);
            // every other one dies to the laugh
            g.minions[m].hp = if k % 2 == 0 { 1.0 } else { 1e6 };
        }
        g.giggle(0, 6.0, 50.0, 2.0);
        let survivors: Vec<_> = g.minions.iter().filter(|m| m.team == Team::Red && m.pos.distance(at) < 6.0).collect();
        assert!(!survivors.is_empty());
        assert!(survivors.iter().all(|m| m.giggle_t > 0.0 && m.hp < 1e6), "every survivor giggles and was hurt");
    }

    #[test]
    fn a_giggling_cat_wanders_both_ways_and_a_sober_one_goes_straight() {
        let d = Vec3::X;
        assert_eq!(confused(d, 0.0, 3.0, 2), d);
        let (mut left, mut right) = (0, 0);
        for k in 0..200 {
            let v = confused(d, 1.0, k as f32 * 0.05, 3);
            assert!((v.length() - 1.0).abs() < 1e-4, "same speed, only a new heading");
            let a = v.z.atan2(v.x);
            assert!(a.abs() <= WANDER + 1e-3);
            if a > 0.4 {
                left += 1;
            }
            if a < -0.4 {
                right += 1;
            }
        }
        assert!(left > 20 && right > 20, "swings to and fro: {left} / {right}");
    }
}
