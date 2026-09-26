//! brawl -- what makes a fight close: bodies that collide, melee that
//! differs by cat, area damage that thins in a crowd, a defence that pays
//! back what a crowd soaks, and an escape that costs something once you
//! are engaged.
//!
//! The movement already makes a fight of circling and diving in when a
//! teammate gives you the edge; these rules give the dive its risk and its
//! reward. The real-world patterns they follow:
//! - **Body collision** (Dota 2 has unit collision: body-blocking, trapping
//!   and surrounding are skills): cats shove each other; a dash into an
//!   enemy stops dead on the body and knocks it back -- a tackle.
//! - **Melee identity** (fighting games' and MOBAs' hitbox differences):
//!   guardians sweep wide and hit several, warriors cleave two, assassins
//!   bite one -- hard from behind, which rewards the circling.
//! - **Multi-target falloff** (common to MOBAs: area spells are tuned so
//!   stacking many targets does not multiply their worth): each extra cat
//!   an area hit catches thins it, to half at nine -- a 15-v-15 choke is no
//!   longer a wall of damage. Minions take full damage: wave-clearing is
//!   untouched.
//! - **Siphon** (spell-vamp and mana-on-hit, turned defensive): every cat
//!   regains a share of the ability damage enemy cats deal it as mana --
//!   soaking a barrage fuels the answer. Two items raise it.
//! - **Winded** (Dota 2's Blink Dagger is disabled for 3 s after hero
//!   damage): hit in the last 2.5 s, a cat cannot chain-bound from perch to
//!   perch and its next leap cools twice as long. You can still leap IN, or
//!   reposition; you cannot hop away forever once engaged.

use glam::Vec3;

use crate::gods::{GodDef, Role};
use crate::sim::{Game, TargetRef};

/// How a cat's basic attack lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Melee {
    /// Full width of the cone, radians, about the line to the main target.
    pub arc: f32,
    /// Most targets one swing can hit (the main one included).
    pub targets: usize,
    /// Damage to each extra target, of the main hit's.
    pub splash: f32,
    /// Damage multiplier striking a cat from behind.
    pub backstab: f32,
}

/// A melee cat's attack by role; None for the ranged (their basic attack
/// is a projectile or a snap, unchanged).
pub fn melee_for(def: &GodDef) -> Option<Melee> {
    if def.auto_range >= 4.5 {
        return None;
    }
    Some(match def.role {
        Role::Guardian => Melee { arc: 150f32.to_radians(), targets: 3, splash: 0.6, backstab: 1.0 },
        Role::Warrior => Melee { arc: 100f32.to_radians(), targets: 2, splash: 0.75, backstab: 1.1 },
        Role::Assassin => Melee { arc: 50f32.to_radians(), targets: 1, splash: 0.0, backstab: 1.35 },
        _ => Melee { arc: 80f32.to_radians(), targets: 1, splash: 0.0, backstab: 1.15 },
    })
}

/// Per-target damage when an area hit catches `n` enemy cats.
pub fn aoe_falloff(n: usize) -> f32 {
    if n <= 1 {
        return 1.0;
    }
    (1.0 / (1.0 + 0.12 * (n as f32 - 1.0))).max(0.5)
}

/// An area hit reaches a target this far above or below its centre.
pub const AOE_REACH_Y: f32 = 3.5;

/// The base share of enemy ability damage a cat regains as mana.
pub const SIPHON_BASE: f32 = 0.10;
/// Items that raise it (by name), and by how much.
pub const SIPHON_ITEMS: [(&str, f32); 2] = [("Siphon Charm", 0.08), ("Soulwell Talisman", 0.20)];

pub fn siphon_pct(items: &[u16]) -> f32 {
    let cat = crate::items::all();
    let bonus: f32 = items
        .iter()
        .filter_map(|id| cat.iter().find(|i| i.id == *id))
        .filter_map(|i| SIPHON_ITEMS.iter().find(|(n, _)| *n == i.name).map(|x| x.1))
        .sum();
    (SIPHON_BASE + bonus).min(0.4)
}

/// Seconds after taking damage that a cat is winded.
pub const WINDED_SECS: f32 = 2.5;
/// A winded cat's leap cooldown, times the usual.
pub const WINDED_COOLDOWN: f32 = 2.0;

/// May a cat leap now? `Some(chained)` if so -- a chained bound (from a
/// perch, or buffered in a landing) skips the cooldown -- None if not.
/// WINDED (hit less than WINDED_SECS ago): no chaining, so the cooldown
/// holds -- the endless hop away from a fight is gone.
pub fn may_leap(in_air: bool, landing: bool, perched: bool, cooldown: f32, since_hit: f32) -> Option<bool> {
    if in_air && !landing {
        return None;
    }
    let chained = (perched || landing) && since_hit >= WINDED_SECS;
    if !chained && cooldown > 0.0 {
        return None;
    }
    Some(chained)
}

pub fn winded(game: &Game, gi: usize) -> bool {
    game.t - game.gods[gi].last_dmg_t < WINDED_SECS
}

/// A cat's body, for collision: a circle this wide.
pub const BODY_R: f32 = 0.55;
/// How much of an overlap each of two ALLIES takes back per frame (softer
/// than enemies, 0.5 each, so fifteen allies do not jam a lane).
pub const ALLY_PUSH: f32 = 0.3;
/// How far a tackled cat is knocked back.
pub const TACKLE_KNOCK: f32 = 0.9;

impl Game {
    /// Bodies collide: overlapping cats on the same level are pushed apart
    /// (enemies fully, allies softly); a dash into an enemy stops dead on
    /// the body and knocks it back. Cats in the air (leaping) pass over.
    /// Run after movement; walls get the last word.
    pub fn resolve_bodies(&mut self) {
        let n = self.gods.len();
        let blockers = self.blockers.clone();
        // only the cats this pushes are re-resolved: movement resolves walls
        // with each cat's real footing (the shelf, wall tops), and re-doing
        // it here for everyone, footing-blind, broke the climbing lane
        let mut pushed = vec![false; n];
        for i in 0..n {
            for j in (i + 1)..n {
                let (a, b) = (&self.gods[i], &self.gods[j]);
                if a.respawn_t > 0.0 || b.respawn_t > 0.0 || a.leap.is_some() || b.leap.is_some() || (a.pos.y - b.pos.y).abs() > 1.2 {
                    continue;
                }
                let d = Vec3::new(b.pos.x - a.pos.x, 0.0, b.pos.z - a.pos.z);
                let dist = d.length();
                let min = BODY_R * 2.0;
                if dist >= min {
                    continue;
                }
                let nrm = if dist > 1e-4 { d / dist } else { Vec3::X };
                let overlap = min - dist;
                let enemies = a.team != b.team;
                // a dash into an enemy: stops on the body, knocks it back
                if enemies {
                    for (dasher, other, sign) in [(i, j, 1.0f32), (j, i, -1.0)] {
                        if self.gods[dasher].dash.is_some() && self.gods[other].dash.is_none() {
                            self.gods[dasher].dash = None;
                            let knock = nrm * sign * TACKLE_KNOCK;
                            self.gods[other].pos += knock;
                            pushed[other] = true;
                        }
                    }
                }
                let k = if enemies { 0.5 } else { ALLY_PUSH };
                self.gods[i].pos -= nrm * overlap * k;
                self.gods[j].pos += nrm * overlap * k;
                pushed[i] = true;
                pushed[j] = true;
            }
        }
        // the walls, then the arena's rim -- the rim has the LAST word (as in
        // movement: a prop pushing outward must not eject a cat; the walls
        // alone here let one out to r = 169, which a test caught)
        for (g, _) in self.gods.iter_mut().zip(pushed.iter()).filter(|(_, p)| **p) {
            if g.respawn_t <= 0.0 && g.leap.is_none() {
                // the footing movement uses: a perch's top, else the ground
                let feet = if g.perch.is_some() { g.pos.y } else { crate::map::ground_y(g.pos.x, g.pos.z) };
                g.pos = blockers.resolve_at(g.pos, crate::consts::GOD_RADIUS, feet);
                g.pos = crate::abilities::clamp_arena(g.pos);
            }
        }
    }

    /// A melee swing's extra targets: enemies within reach and inside the
    /// cone about the line to the main target, nearest first, up to the
    /// profile's count. (The main target is hit by the usual path.)
    pub fn cleave_targets(&self, gi: usize, main: TargetRef, reach: f32, m: Melee) -> Vec<TargetRef> {
        if m.targets <= 1 {
            return Vec::new();
        }
        let g = &self.gods[gi];
        let main_pos = match main {
            TargetRef::God(j) => self.gods[j].pos,
            TargetRef::Minion(j) => self.minions[j].pos,
            _ => return Vec::new(),
        };
        let axis = Vec3::new(main_pos.x - g.pos.x, 0.0, main_pos.z - g.pos.z).normalize_or_zero();
        let half = m.arc * 0.5;
        let within = |p: Vec3| {
            let d = Vec3::new(p.x - g.pos.x, 0.0, p.z - g.pos.z);
            let len = d.length();
            len <= reach && len > 1e-3 && (d / len).dot(axis) >= half.cos() && (p.y - g.pos.y).abs() < 1.5
        };
        let mut c: Vec<(f32, TargetRef)> = Vec::new();
        for (j, e) in self.gods.iter().enumerate() {
            if e.team != g.team && e.respawn_t <= 0.0 && TargetRef::God(j) != main && within(e.pos) {
                c.push((e.pos.distance(g.pos), TargetRef::God(j)));
            }
        }
        for (j, e) in self.minions.iter().enumerate() {
            if e.team != g.team && TargetRef::Minion(j) != main && within(e.pos) {
                c.push((e.pos.distance(g.pos), TargetRef::Minion(j)));
            }
        }
        c.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        c.into_iter().take(m.targets - 1).map(|x| x.1).collect()
    }

    /// Is `attacker` striking cat `target` from behind (the target facing
    /// away from it)?
    pub fn from_behind(&self, attacker: usize, target: usize) -> bool {
        let (a, t) = (&self.gods[attacker], &self.gods[target]);
        let to_t = Vec3::new(t.pos.x - a.pos.x, 0.0, t.pos.z - a.pos.z).normalize_or_zero();
        crate::abilities::facing_of(t.yaw).dot(to_t) > 0.35
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_thins_a_crowd_but_not_one() {
        assert_eq!(aoe_falloff(1), 1.0);
        assert!(aoe_falloff(3) < 1.0 && aoe_falloff(3) > 0.7);
        assert_eq!(aoe_falloff(15), 0.5);
        // total damage to a crowd still grows: it is not a nerf into uselessness
        assert!(aoe_falloff(5) * 5.0 > aoe_falloff(3) * 3.0);
    }

    #[test]
    fn roles_fight_differently() {
        let by = |r: Role| crate::gods::ROSTER.iter().find(|d| d.role == r && d.auto_range < 4.5).map(melee_for).flatten();
        let (g, w, a) = (by(Role::Guardian).unwrap(), by(Role::Warrior).unwrap(), by(Role::Assassin).unwrap());
        assert!(g.arc > w.arc && w.arc > a.arc, "guardians sweep widest, assassins narrowest");
        assert!(g.targets > w.targets && w.targets > a.targets);
        assert!(a.backstab > w.backstab && a.backstab > g.backstab, "assassins punish from behind");
        let ranged = crate::gods::ROSTER.iter().find(|d| d.auto_range >= 4.5).unwrap();
        assert!(melee_for(ranged).is_none());
    }

    #[test]
    fn the_siphon_is_upgradeable() {
        assert_eq!(siphon_pct(&[]), SIPHON_BASE);
        let cat = crate::items::all();
        let charm = cat.iter().find(|i| i.name == "Siphon Charm").expect("the item exists").id;
        let well = cat.iter().find(|i| i.name == "Soulwell Talisman").expect("the item exists").id;
        assert!((siphon_pct(&[charm]) - 0.18).abs() < 1e-5);
        assert!((siphon_pct(&[charm, well]) - 0.38).abs() < 1e-5);
    }

    /// Winded stops the chain, not the leap: a hurt cat on a perch waits
    /// out its cooldown; an unhurt one bounds on; anyone may leap once the
    /// cooldown is done; no one leaps mid-air.
    #[test]
    fn winded_stops_chaining_not_leaping() {
        assert_eq!(may_leap(false, false, true, 1.0, 10.0), Some(true), "unhurt, perched: bound on");
        assert_eq!(may_leap(false, false, true, 1.0, 0.5), None, "hurt, perched: the cooldown holds");
        assert_eq!(may_leap(false, true, false, 1.0, 0.5), None, "hurt, landing: no buffered chain");
        assert_eq!(may_leap(false, false, false, 0.0, 0.5), Some(false), "hurt but cooled: a normal leap");
        assert_eq!(may_leap(true, false, false, 0.0, 10.0), None, "mid-air: no");
    }
}
