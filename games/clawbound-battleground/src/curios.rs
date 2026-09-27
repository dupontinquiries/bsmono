//! curios -- the Curio shelf's passives, worked (see `items::Passive`).
//!
//! Each hook is called from the one place in the sim where its moment
//! happens, and does nothing for a cat that holds no such Curio:
//!
//! | Curio                | hook                  | called from                         |
//! |----------------------|-----------------------|-------------------------------------|
//! | Ninth Life           | [`Game::ninth_life`]  | `deal_damage`, before a cat dies    |
//! | Catnip Censer        | [`Game::censer_at`]   | `move_speed`, `auto_interval`       |
//! | Thrice-Pounce Claws  | [`Game::pounce_bonus`]| `god_autoattacks`, as a blow fires  |
//! | Moonmilk Flask       | [`Game::mending`]     | `timers_tick`, the regen            |
//! | Thornpelt Mantle     | [`Game::curios_on_hit`]| `deal_damage`, a cat hit           |
//! | Wanderer's Whiskers  | [`Game::curio_move_bonus`] | `move_speed`                   |
//! | Tidecaller Pearl     | [`Game::curios_on_hit`]| `deal_damage`, a cat hit           |
//! | Bramble Lash         | [`Game::curios_on_hit`]| `deal_damage`, a basic attack      |
//!
//! Plus [`Game::curios_tick`]: the Ninth Life's recharge and the trophies
//! that grow with time (`StackKind::Time`), once a tick per living cat.

use crate::items::{self, Passive, StackKind};
use crate::sim::{Game, TargetRef};

/// A cat's Curio state.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CurioState {
    /// Seconds until the Ninth Life can save the cat again.
    pub ninth_cd: f32,
    /// Basic attacks since the last pounce.
    pub pounces: u32,
}

impl Game {
    /// The passives cat `i` holds.
    pub fn passives(&self, i: usize) -> impl Iterator<Item = Passive> + '_ {
        self.gods[i].items.iter().filter_map(|&id| items::all()[id as usize].passive)
    }

    fn enemy_cat_within(&self, i: usize, radius: f32) -> bool {
        let g = &self.gods[i];
        self.gods.iter().any(|o| o.team != g.team && o.respawn_t <= 0.0 && o.pos.distance(g.pos) <= radius)
    }

    /// The Catnip Censer's air where cat `i` stands: (move, attack speed)
    /// fractions, from the strongest censer held by it or an ally in reach.
    pub fn censer_at(&self, i: usize) -> (f32, f32) {
        let g = &self.gods[i];
        let mut best = (0.0f32, 0.0f32);
        for (j, o) in self.gods.iter().enumerate() {
            if o.team != g.team || o.respawn_t > 0.0 {
                continue;
            }
            for p in self.passives(j) {
                if let Passive::Censer { radius, ms, atk } = p {
                    if o.pos.distance(g.pos) <= radius {
                        best = (best.0.max(ms), best.1.max(atk));
                    }
                }
            }
        }
        best
    }

    /// Movement from Curios: the censer's air, and the Wanderer's Whiskers
    /// while no enemy cat is near.
    pub fn curio_move_bonus(&self, i: usize) -> f32 {
        let mut ms = self.censer_at(i).0;
        for p in self.passives(i) {
            if let Passive::Wander { radius, ms: w } = p {
                if !self.enemy_cat_within(i, radius) {
                    ms += w;
                }
            }
        }
        ms
    }

    /// A basic attack fires: count it, and on every `every`th one return the
    /// Thrice-Pounce Claws' extra damage.
    pub fn pounce_bonus(&mut self, i: usize) -> f32 {
        let Some((every, bonus, scale)) = self.passives(i).find_map(|p| match p {
            Passive::Pounce { every, bonus, scale } => Some((every, bonus, scale)),
            _ => None,
        }) else {
            return 0.0;
        };
        let g = &mut self.gods[i];
        g.curio.pounces += 1;
        if g.curio.pounces >= every {
            g.curio.pounces = 0;
            bonus + scale * g.power
        } else {
            0.0
        }
    }

    /// Out-of-combat mending for cat `i`: (seconds untouched first, share
    /// of max health a second).
    pub fn mending(&self, i: usize) -> (f32, f32) {
        self.passives(i)
            .find_map(|p| match p {
                Passive::Moonmilk { after, pct } => Some((after, pct)),
                _ => None,
            })
            .unwrap_or((6.0, 0.012))
    }

    /// Cat `ti` has just taken `dmg` from `source`: the Thornpelt Mantle
    /// throws a share back, the Tidecaller Pearl heals an ability's caster,
    /// the Bramble Lash slows on a basic attack.
    pub fn curios_on_hit(&mut self, ti: usize, source: Option<usize>, dmg: f32) {
        let Some(s) = source.filter(|&s| s < self.gods.len() && s != ti && self.gods[s].team != self.gods[ti].team) else {
            return;
        };
        if dmg <= 0.0 {
            return;
        }
        let basic = self.basic_attack;
        for p in self.passives(s).collect::<Vec<_>>() {
            match p {
                Passive::Tide { pct } if !basic && !self.reflecting => {
                    let g = &mut self.gods[s];
                    if g.respawn_t <= 0.0 {
                        g.hp = (g.hp + dmg * pct).min(g.max_hp);
                    }
                }
                Passive::Bramble { slow, secs } if basic => {
                    let g = &mut self.gods[ti];
                    g.slow = g.slow.max(slow);
                    g.slow_t = g.slow_t.max(secs);
                }
                _ => {}
            }
        }
        if !self.reflecting && self.gods[s].respawn_t <= 0.0 {
            let back: f32 = self.passives(ti).map(|p| if let Passive::Thorns { pct } = p { pct } else { 0.0 }).sum();
            if back > 0.0 {
                // back as pure damage: full penetration, and not a basic attack
                self.reflecting = true;
                let was_basic = std::mem::replace(&mut self.basic_attack, false);
                self.deal_damage(TargetRef::God(s), dmg * back, true, Some(ti), 1.0, 0.0);
                self.basic_attack = was_basic;
                self.reflecting = false;
            }
        }
    }

    /// Cat `i` has taken a blow that would kill it: does a Ninth Life catch
    /// it? If so it stands at the Curio's share of its health and the bell
    /// starts its recharge.
    pub fn ninth_life(&mut self, i: usize) -> bool {
        let Some((hp, cd)) = self.passives(i).find_map(|p| match p {
            Passive::NinthLife { hp, cd } => Some((hp, cd)),
            _ => None,
        }) else {
            return false;
        };
        let g = &mut self.gods[i];
        if g.curio.ninth_cd > 0.0 {
            return false;
        }
        g.hp = g.max_hp * hp;
        g.curio.ninth_cd = cd;
        let pos = g.pos;
        self.events.push(crate::sim::Event::Explosion { pos: pos + glam::Vec3::Y * 1.2, radius: 2.2, color: [1.0, 0.9, 0.55] });
        true
    }

    /// Once a tick for a living cat: the bell recharges, and the trophies
    /// that grow with the match gain their time.
    pub fn curios_tick(&mut self, i: usize, dt: f32) {
        let g = &mut self.gods[i];
        g.curio.ninth_cd = (g.curio.ninth_cd - dt).max(0.0);
        let cat = items::all();
        let mut changed = false;
        for k in 0..g.items.len() {
            if let Some(st) = cat[g.items[k] as usize].stack.filter(|s| s.kind == StackKind::Time) {
                // a Time trophy counts the intervals it has been held
                let cur = g.stacks.get(k).copied().unwrap_or(0.0);
                let next = (cur + dt / st.interval.max(1.0)).min(st.cap());
                changed |= st.gained(next) != st.gained(cur);
                if k < g.stacks.len() {
                    g.stacks[k] = next;
                }
            }
        }
        if changed {
            self.recalc(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn curio(name: &str) -> u16 {
        items::all().iter().find(|i| i.name == name).unwrap_or_else(|| panic!("no {name}")).id
    }

    /// Blue 0 and 1 side by side at the origin, red 5 beside them; every
    /// other cat far away.
    fn game() -> Game {
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        for (i, c) in g.gods.iter_mut().enumerate() {
            c.pos = Vec3::new(200.0 + i as f32 * 10.0, 0.0, 200.0);
            c.gold = 1e6;
        }
        g.gods[0].pos = Vec3::ZERO;
        g.gods[1].pos = Vec3::new(3.0, 0.0, 0.0);
        g.gods[5].pos = Vec3::new(0.0, 0.0, 4.0);
        g
    }

    fn give(g: &mut Game, i: usize, name: &str) {
        assert!(g.buy_item(i, curio(name)));
    }

    #[test]
    fn eight_curios_each_with_a_trick() {
        let c: Vec<_> = items::all().iter().filter(|i| i.cat == items::Cat::Curio).collect();
        assert_eq!(c.len(), 8);
        assert!(c.iter().all(|i| i.passive.is_some() && i.stack.is_none()));
        assert!(items::all().iter().filter(|i| i.cat != items::Cat::Curio).all(|i| i.passive.is_none()));
    }

    #[test]
    fn the_ninth_life_catches_one_death_then_recharges() {
        let mut g = game();
        give(&mut g, 0, "Ninth Life");
        let max = g.gods[0].max_hp;
        g.deal_damage(TargetRef::God(0), 1e6, true, Some(5), 1.0, 0.0);
        assert!(g.gods[0].respawn_t <= 0.0, "saved");
        assert!((g.gods[0].hp - max * 0.3).abs() < 1.0, "at 30%: {}", g.gods[0].hp);
        g.deal_damage(TargetRef::God(0), 1e6, true, Some(5), 1.0, 0.0);
        assert!(g.gods[0].respawn_t > 0.0, "the bell is not ready: dead");
        // a cat without it simply dies
        g.deal_damage(TargetRef::God(1), 1e6, true, Some(5), 1.0, 0.0);
        assert!(g.gods[1].respawn_t > 0.0);
        // and the bell comes back
        let mut h = game();
        give(&mut h, 0, "Ninth Life");
        h.deal_damage(TargetRef::God(0), 1e6, true, Some(5), 1.0, 0.0);
        for _ in 0..1510 {
            h.curios_tick(0, 0.1);
        }
        assert_eq!(h.gods[0].curio.ninth_cd, 0.0);
        h.deal_damage(TargetRef::God(0), 1e6, true, Some(5), 1.0, 0.0);
        assert!(h.gods[0].respawn_t <= 0.0, "saved again after 150 s");
    }

    #[test]
    fn the_censer_quickens_allies_near_it_once() {
        let mut g = game();
        let (ms0, ai0) = (g.move_speed(1), g.auto_interval(1));
        give(&mut g, 0, "Catnip Censer");
        assert!(g.move_speed(1) > ms0 * 1.07 && g.auto_interval(1) < ai0 / 1.11, "the ally beside it");
        assert_eq!(g.censer_at(5), (0.0, 0.0), "not the enemy");
        g.gods[1].pos = Vec3::new(30.0, 0.0, 0.0);
        assert_eq!(g.censer_at(1), (0.0, 0.0), "not out of reach");
        g.gods[1].pos = Vec3::new(3.0, 0.0, 0.0);
        let one = g.censer_at(1);
        give(&mut g, 1, "Catnip Censer");
        assert_eq!(g.censer_at(1), one, "two do not add up");
    }

    #[test]
    fn every_third_pounce_lands_harder() {
        let mut g = game();
        assert_eq!(g.pounce_bonus(0), 0.0);
        give(&mut g, 0, "Thrice-Pounce Claws");
        let b: Vec<f32> = (0..6).map(|_| g.pounce_bonus(0)).collect();
        assert_eq!((b[0], b[1], b[3], b[4]), (0.0, 0.0, 0.0, 0.0));
        let want = 60.0 + 0.35 * g.gods[0].power;
        assert!((b[2] - want).abs() < 1e-3 && (b[5] - want).abs() < 1e-3, "{b:?}");
    }

    #[test]
    fn moonmilk_mends_sooner_and_faster() {
        let mut g = game();
        assert_eq!(g.mending(0), (6.0, 0.012));
        give(&mut g, 0, "Moonmilk Flask");
        assert_eq!(g.mending(0), (3.5, 0.035));
    }

    #[test]
    fn the_thornpelt_bites_back_but_not_forever() {
        let mut g = game();
        give(&mut g, 0, "Thornpelt Mantle");
        give(&mut g, 5, "Thornpelt Mantle");
        let hp5 = g.gods[5].hp;
        let hp0 = g.gods[0].hp;
        g.deal_damage(TargetRef::God(0), 100.0, false, Some(5), 0.0, 0.0);
        let taken = hp0 - g.gods[0].hp;
        let back = hp5 - g.gods[5].hp;
        // the blow through 0's protection, and nothing more: the
        // reflection was not thrown back at it by 5's own mantle
        let blow = 100.0 * 100.0 / (100.0 + g.gods[0].prot);
        assert!((taken - blow).abs() < 1e-3, "took {taken}, the blow was {blow}");
        assert!((back - blow * 0.18).abs() < 1e-3, "gave back {back}");
        // an ally's blow is not an enemy's
        let hp1 = g.gods[1].hp;
        g.deal_damage(TargetRef::God(0), 100.0, false, Some(1), 0.0, 0.0);
        assert_eq!(g.gods[1].hp, hp1);
    }

    #[test]
    fn the_whiskers_speed_a_lone_cat_only() {
        let mut g = game();
        g.gods[5].pos = Vec3::new(100.0, 0.0, 100.0);
        g.gods[0].pos = Vec3::new(-100.0, 0.0, -100.0);
        let lone = g.move_speed(0);
        give(&mut g, 0, "Wanderer's Whiskers");
        assert!(g.move_speed(0) > lone * 1.13, "alone: faster");
        g.gods[5].pos = g.gods[0].pos + Vec3::new(8.0, 0.0, 0.0);
        assert!((g.move_speed(0) - lone).abs() < 1e-3, "an enemy near: no bonus");
    }

    #[test]
    fn the_pearl_heals_on_abilities_and_the_lash_slows_on_blows() {
        let mut g = game();
        give(&mut g, 5, "Tidecaller Pearl");
        give(&mut g, 5, "Bramble Lash");
        g.gods[5].hp = 100.0;
        // an ability: heals, no slow
        g.deal_damage(TargetRef::God(0), 200.0, true, Some(5), 0.0, 0.0);
        let healed = g.gods[5].hp - 100.0;
        assert!(healed > 1.0, "healed {healed}");
        assert_eq!(g.gods[0].slow_t, 0.0);
        // a basic attack: slows, no heal
        let hp = g.gods[5].hp;
        g.basic_attack = true;
        g.deal_damage(TargetRef::God(0), 200.0, false, Some(5), 0.0, 0.0);
        g.basic_attack = false;
        assert_eq!(g.gods[5].hp, hp);
        assert!((g.gods[0].slow - 0.18).abs() < 1e-6 && g.gods[0].slow_t > 1.0);
    }
}
