//! clay -- Claypaw, the potter of the river mud: a fishing cat who is
//! made of the riverbank, and makes more of it.
//!
//! * **His kittens** (passive): every [`KIT_EVERY`] seconds he shapes a
//!   clay kitten, up to [`KIT_MAX`] of his own at once. Each runs at the
//!   nearest enemy it can see and nips it (`kit`'s kitlings, `clay: true`:
//!   no giggle, a crumble).
//! * **His mana** does not come back by itself: each of his basic attacks
//!   that lands gives him [`MANA_PER_HIT`] (+[`MANA_PER_HIT_LEVEL`] a
//!   level). And healing from anything else only half takes on him
//!   ([`HEAL_MUL`], `GodUnit::heal_mul`).
//! * **He dries**: his clay dries over [`DRY_SECS`], down to [`DRY_SPEED`]
//!   of his speed (`GodUnit::clay_wet`, 1 wet .. 0 dry).
//! * **Remold** (`Archetype::Remold`): he reshapes himself, `heal` of his
//!   health back (his own clay: not halved).
//! * **Quicken** (`Archetype::Quicken`): wet again at once, and for `secs`
//!   faster by `speed` and shaping kittens every [`KIT_QUICK`] s.
//! * **Spin** (`Archetype::Spin`): he spins along the aim, `dist`,
//!   striking what he passes (`dmg`), and leaves the line he spun down
//!   a MUD SLICK `radius` either side for `secs` ([`Slick`]).
//! * **Mudslide** (`Archetype::Mudslide`, the ultimate): a slick `len`
//!   long and `width` across along the aim that CARRIES everything in it
//!   along the aim at `push` u/s for `secs`; and `kits` kittens tumble out.
//!
//! A slick is anyone's: friend or foe, a cat on it keeps going the way it
//! was going, [`SLICK_SPEED`] faster, and turns only slowly
//! ([`SLICK_GRIP`]) -- a friend following his line comes on fast, an enemy
//! trying to turn on it slides past where it meant to go.
//! (`systems::move_gods` reads [`Game::slick_at`].)

use glam::Vec3;

use crate::sim::{Event, Game};

pub const KIT_EVERY: f32 = 6.0;
pub const KIT_QUICK: f32 = 2.5;
pub const KIT_MAX: usize = 4;
/// A kitten: its bite, its run, how long it holds together.
pub const KIT_DMG: f32 = 22.0;
pub const KIT_SPEED: f32 = 12.5;
pub const KIT_LIFE: f32 = 9.0;
pub const MANA_PER_HIT: f32 = 14.0;
pub const MANA_PER_HIT_LEVEL: f32 = 1.0;
pub const HEAL_MUL: f32 = 0.5;
pub const DRY_SECS: f32 = 20.0;
pub const DRY_SPEED: f32 = 0.7;
/// Quick: this much faster.
pub const QUICK_SPEED: f32 = 0.2;
/// On a slick: this much faster, and this quick (1/s) to change heading.
pub const SLICK_SPEED: f32 = 1.3;
pub const SLICK_GRIP: f32 = 2.2;

/// A mud slick: a capsule round the line `a`-`b`, `r` across, for `left`
/// seconds; `push` (u/s) carries whatever is on it (a mudslide's), or zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slick {
    pub a: Vec3,
    pub b: Vec3,
    pub r: f32,
    pub left: f32,
    pub secs: f32,
    pub push: Vec3,
}

impl Slick {
    pub fn covers(&self, p: Vec3) -> bool {
        let f = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
        crate::abilities::point_segment_dist(f(p), f(self.a), f(self.b)) <= self.r
    }
}

/// Is this cat made of clay (Claypaw's kind)?
pub fn is_clay(def: &crate::gods::GodDef) -> bool {
    matches!(def.pelt, crate::gods::PeltKind::Clay)
}

/// His speed as his clay dries: all of it wet, [`DRY_SPEED`] of it dry.
pub fn dry_mul(wet: f32) -> f32 {
    DRY_SPEED + (1.0 - DRY_SPEED) * wet.clamp(0.0, 1.0)
}

impl Game {
    /// The slick under `p`, if any (the newest).
    pub fn slick_at(&self, p: Vec3) -> Option<&Slick> {
        self.slicks.iter().rev().find(|s| s.covers(p))
    }

    /// Once a tick: the slicks dry up; each clay cat dries, and shapes a
    /// kitten when it is time.
    pub fn clay_tick(&mut self, dt: f32) {
        for s in self.slicks.iter_mut() {
            s.left -= dt;
        }
        self.slicks.retain(|s| s.left > 0.0);
        for i in 0..self.gods.len() {
            if !is_clay(self.def(i)) || self.gods[i].respawn_t > 0.0 {
                continue;
            }
            let own = self.kitlings.iter().filter(|k| k.clay && k.owner == i).count();
            let g = &mut self.gods[i];
            g.clay_wet = (g.clay_wet - dt / DRY_SECS).max(0.0);
            g.clay_quick_t = (g.clay_quick_t - dt).max(0.0);
            g.clay_kit_t -= dt;
            if g.clay_kit_t <= 0.0 {
                g.clay_kit_t = if g.clay_quick_t > 0.0 { KIT_QUICK } else { KIT_EVERY };
                if own < KIT_MAX {
                    let facing = crate::abilities::facing_of(g.yaw);
                    self.shape_kittens(i, facing, 1);
                }
            }
        }
    }

    /// `n` clay kittens out of him, fanned round `dir`.
    pub fn shape_kittens(&mut self, gi: usize, dir: Vec3, n: u32) {
        let before = self.kitlings.len();
        self.litter(gi, dir, n, KIT_DMG, KIT_SPEED, KIT_LIFE);
        for k in &mut self.kitlings[before..] {
            k.clay = true;
        }
        let pos = self.gods[gi].pos;
        self.events.push(Event::ClayShaped { pos, n });
    }

    /// A basic attack of hers landed: mana for it.
    pub fn clay_on_hit(&mut self, s: usize) {
        if s < self.gods.len() && is_clay(self.def(s)) {
            let g = &mut self.gods[s];
            g.mana = (g.mana + MANA_PER_HIT + MANA_PER_HIT_LEVEL * g.level as f32).min(g.max_mana);
        }
    }

    /// Remold: `heal` of his health back, not halved (his own clay).
    pub fn remold(&mut self, gi: usize, heal: f32) {
        let g = &mut self.gods[gi];
        g.hp = (g.hp + g.max_hp * heal).min(g.max_hp);
        let pos = g.pos;
        self.events.push(Event::ClayRemold { pos });
    }

    /// Quicken: wet again, and quick for `secs`.
    pub fn quicken(&mut self, gi: usize, secs: f32) {
        let g = &mut self.gods[gi];
        g.clay_wet = 1.0;
        g.clay_quick_t = secs;
        g.clay_kit_t = g.clay_kit_t.min(KIT_QUICK);
        let pos = g.pos;
        self.events.push(Event::ClayRemold { pos });
    }

    /// Lay a slick.
    pub fn lay_slick(&mut self, a: Vec3, b: Vec3, r: f32, secs: f32, push: Vec3) {
        self.slicks.push(Slick { a, b, r, left: secs, secs, push });
        self.events.push(Event::SlickFx { a, b, r });
    }
}

/// A cat's step on a slick: the way it slides, from the way it slid
/// (`slide`) and the way it wants to go at `speed` (`want`, unit or zero),
/// over `dt`. (A mudslide's carry is added on top, by the mover.)
pub fn slide_step(slide: Vec3, want: Vec3, speed: f32, dt: f32) -> Vec3 {
    let target = want * speed * SLICK_SPEED;
    slide + (target - slide) * (SLICK_GRIP * dt).min(1.0)
}

/// A puddle of wet mud, radius 1 (the scene scales it): flat, ragged at
/// the rim, darker in the middle, a few glints of wet. Centred, y from 0.
pub fn puddle_mesh() -> crate::voxel::VoxelMesh {
    const V: f32 = 0.1;
    let n = 22i32;
    let mut g = crate::voxel::Grid::new((n * 2 + 1) as usize, 2, (n * 2 + 1) as usize);
    for x in -n..=n {
        for z in -n..=n {
            let d = ((x * x + z * z) as f32).sqrt() / n as f32;
            let a = (z as f32).atan2(x as f32);
            let rim = 0.88 + 0.12 * (a * 5.0).sin() * (a * 3.0 + 1.0).cos();
            if d > rim {
                continue;
            }
            let k = ((x * 13 + z * 7) as u32).wrapping_mul(2654435761) >> 27;
            let c = if k == 0 { [0.62, 0.52, 0.42, 1.0] } else { let m = 0.30 - 0.10 * (1.0 - d); [m + 0.10, m, m * 0.7, 1.0] };
            g.set(x + n, 0, z + n, c);
        }
    }
    g.mesh(V, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::ROSTER;

    fn claypaw() -> u32 {
        ROSTER.iter().find(|d| is_clay(d)).expect("Claypaw on the roster").id
    }

    fn solo() -> Game {
        let mut g = Game::new_teams(&[claypaw()], &[1]);
        g.gods[1].pos = Vec3::new(0.0, 0.0, 400.0);
        g.gods[0].ai = None;
        g.minions.clear();
        g.next_wave_t = 1e9;
        g
    }

    /// Kittens: one every KIT_EVERY, never more than KIT_MAX of hers; and
    /// faster while he is quick.
    #[test]
    fn he_shapes_kittens_and_quicker_when_quick() {
        let mut g = solo();
        let count = |g: &Game| g.kitlings.iter().filter(|k| k.clay && k.owner == 0).count();
        for _ in 0..(KIT_EVERY * 2.5 * 60.0) as usize {
            g.t += 1.0 / 60.0;
            g.clay_tick(1.0 / 60.0);
        }
        assert!((2..=3).contains(&count(&g)), "{} kittens in {} s", count(&g), KIT_EVERY * 2.5);
        for _ in 0..(KIT_EVERY * 10.0 * 60.0) as usize {
            g.clay_tick(1.0 / 60.0);
        }
        assert_eq!(count(&g), KIT_MAX, "more than KIT_MAX (they are not running off: nothing ticks them)");
        // in the same 6 s: one kitten slow, two quick (at 2.5 and 5 s)
        let in_six = |quick: bool| {
            let mut g = solo();
            if quick {
                g.quicken(0, 6.0);
            }
            for _ in 0..(6.0 * 60.0) as usize - 6 {
                g.clay_tick(1.0 / 60.0);
            }
            count(&g)
        };
        assert_eq!((in_six(false), in_six(true)), (0, 2), "kittens in 6 s, slow and quick");
    }

    /// His mana: none back by itself; a landed basic attack gives it.
    #[test]
    fn his_mana_comes_from_his_claws() {
        let mut g = solo();
        // (away from the fountain, which fills anyone's)
        g.gods[0].pos = Vec3::ZERO;
        g.gods[0].mana = 0.0;
        for _ in 0..(5 * 60) {
            g.tick(1.0 / 60.0);
        }
        assert!(g.gods[0].mana < 1.0, "mana came back by itself: {}", g.gods[0].mana);
        g.clay_on_hit(0);
        assert!(g.gods[0].mana >= MANA_PER_HIT, "{}", g.gods[0].mana);
        // not for anyone else's hits
        let m = g.gods[1].mana;
        g.gods[1].mana = 0.0;
        g.clay_on_hit(1);
        assert_eq!(g.gods[1].mana, 0.0);
        let _ = m;
    }

    /// Healing halves on him -- a heal ability's, the springs' -- but his
    /// own Remold does not.
    #[test]
    fn other_healing_halves_on_him_but_remold_does_not() {
        let mut g = Game::new_teams(&[claypaw(), 0], &[1]);
        assert_eq!(g.gods[0].heal_mul, HEAL_MUL);
        assert_eq!(g.gods[1].heal_mul, 1.0);
        let max = g.gods[0].max_hp;
        g.gods[0].hp = max * 0.3;
        g.remold(0, 0.22);
        assert!((g.gods[0].hp - max * 0.52).abs() < 1.0, "remold halved: {}", g.gods[0].hp / max);
    }

    /// He dries to DRY_SPEED of his speed; Quicken wets him again and more.
    #[test]
    fn he_dries_and_quickens() {
        let mut g = solo();
        let wet = g.move_speed(0);
        for _ in 0..(DRY_SECS * 60.0 + 60.0) as usize {
            g.clay_tick(1.0 / 60.0);
        }
        let dry = g.move_speed(0);
        assert!((dry / wet - DRY_SPEED).abs() < 0.02, "dry at {}", dry / wet);
        g.quicken(0, 6.0);
        assert!(g.move_speed(0) > wet * 1.1, "quick at {}", g.move_speed(0) / wet);
    }

    /// On a slick a cat keeps sliding the way it went, faster, and turns
    /// slowly: asked to turn round, it is still going forward a tick later,
    /// and has turned a few tenths of a second on.
    #[test]
    fn a_slick_keeps_you_going() {
        let speed = 10.0;
        let mut v = Vec3::X * speed;
        for _ in 0..60 {
            v = slide_step(v, Vec3::X, speed, 1.0 / 60.0);
        }
        assert!((v.length() - speed * SLICK_SPEED).abs() < 1.0, "on a slick at {}", v.length());
        let v1 = slide_step(v, -Vec3::X, speed, 1.0 / 60.0);
        assert!(v1.x > speed, "turned at once: {v1}");
        let mut v2 = v;
        for _ in 0..60 {
            v2 = slide_step(v2, -Vec3::X, speed, 1.0 / 60.0);
        }
        assert!(v2.x < 0.0, "never turned: {v2}");
    }
}
