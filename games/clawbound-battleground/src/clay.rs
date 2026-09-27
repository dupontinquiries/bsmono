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

/// The mudslide's kittens: real fighters, and the price of the gamble --
/// each one broken is SLIDE_KIT_GOLD to whoever breaks it.
pub const SLIDE_KITS: u32 = 8;
pub const SLIDE_KIT_HP: f32 = 190.0;
pub const SLIDE_KIT_HP_LEVEL: f32 = 14.0;
pub const SLIDE_KIT_DMG: f32 = 30.0;
pub const SLIDE_KIT_GOLD: f32 = 40.0;
pub const SLIDE_KIT_LIFE: f32 = 20.0;
pub const SLIDE_KIT_SPEED: f32 = 9.0;
/// For this long after he brings the bank down, his kills shape more: a
/// god KITS_PER_GOD_KILL, a minion or a jungle beast KITS_PER_SMALL_KILL.
pub const SURGE_SECS: f32 = 6.0;
pub const KITS_PER_GOD_KILL: u32 = 3;
pub const KITS_PER_SMALL_KILL: u32 = 1;
/// The wave: each plant it crushes through takes PLANT_MUL of its strength
/// (never below PLANT_FLOOR); each wall it sloshes off, REBOUND_MUL; after
/// MAX_REBOUNDS it spends itself.
pub const PLANT_MUL: f32 = 0.85;
pub const PLANT_FLOOR: f32 = 0.35;
pub const REBOUND_MUL: f32 = 0.6;
pub const MAX_REBOUNDS: u32 = 3;
/// How deep the front is (what it strikes and carries).
pub const FRONT_DEPTH: f32 = 2.5;

/// A mudslide kitten's making (on its `Minion`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClayKit {
    pub owner: usize,
    pub life: f32,
    pub heading: Vec3,
    pub dmg: f32,
}

/// A mudslide running: its front's middle, the way it runs, how far it has
/// left to go, and what it has done so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Wave {
    pub owner: usize,
    pub team: crate::consts::Team,
    pub pos: Vec3,
    pub dir: Vec3,
    pub width: f32,
    pub speed: f32,
    pub left: f32,
    pub dmg: f32,
    /// Its strength now (plants and walls take it down).
    pub mul: f32,
    pub rebounds: u32,
    /// Where this leg began (a slick is laid down each leg when it ends).
    pub leg_from: Vec3,
    /// Struck this leg (gods by index, minions by id + 1_000_000, beasts
    /// + 2_000_000), and plants crushed.
    pub hit: Vec<u64>,
    pub smushed: Vec<u32>,
}

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
        self.waves_tick(dt);
        // his mudslide kittens run out of shape (no bounty for that); his
        // surge runs down
        for m in self.minions.iter_mut() {
            if let Some(c) = &mut m.clay {
                c.life -= dt;
            }
        }
        self.minions.retain(|m| m.clay.is_none_or(|c| c.life > 0.0));
        for g in self.gods.iter_mut() {
            g.clay_surge_t = (g.clay_surge_t - dt).max(0.0);
        }
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

    /// The Mudslide: the bank gives way from where he stands, a wave `width`
    /// across running `len` along `dir` at `speed`, striking for `dmg`; `kits`
    /// kittens tumble out of it; and for SURGE_SECS his kills shape more.
    #[allow(clippy::too_many_arguments)]
    pub fn mudslide(&mut self, gi: usize, dir: Vec3, len: f32, width: f32, speed: f32, dmg: f32, kits: u32) {
        let (pos, team) = (self.gods[gi].pos, self.gods[gi].team);
        let from = Vec3::new(pos.x, crate::map::ground_y(pos.x, pos.z), pos.z);
        self.mudslides.push(Wave { owner: gi, team, pos: from + dir * 1.5, dir, width, speed, left: len, dmg, mul: 1.0, rebounds: 0, leg_from: from, hit: Vec::new(), smushed: Vec::new() });
        self.gods[gi].clay_surge_t = SURGE_SECS + len / speed.max(1.0);
        self.events.push(Event::MudslideFx { pos: from, dir });
        self.slide_kittens(gi, from + dir * 2.0, kits, dir);
    }

    /// `n` of his mudslide kittens at `at`, fanned round `heading`.
    pub fn slide_kittens(&mut self, gi: usize, at: Vec3, n: u32, heading: Vec3) {
        let g = &self.gods[gi];
        let (team, level, power) = (g.team, g.level, self.def(gi).power + self.def(gi).power_growth * (g.level.max(1) - 1) as f32);
        let hp = SLIDE_KIT_HP + SLIDE_KIT_HP_LEVEL * level as f32;
        let dmg = SLIDE_KIT_DMG + power * 0.6;
        let base = heading.z.atan2(heading.x);
        for k in 0..n {
            let u = if n > 1 { k as f32 / (n - 1) as f32 - 0.5 } else { 0.0 };
            let a = base + u * 1.6;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            let p = crate::map::onto_floor(at + d * (0.6 + (k % 3) as f32 * 0.5), 0.4);
            let id = self.next_minion_id;
            self.next_minion_id += 1;
            self.minions.push(crate::sim::Minion {
                team,
                lane: crate::consts::Lane::Mid,
                caster: false,
                pos: Vec3::new(p.x, crate::map::ground_y(p.x, p.z), p.z),
                hp,
                max_hp: hp,
                atk_timer: 0.0,
                atk_anim: 0.0,
                walk_anim: k as f32,
                wp: 0,
                last_hit: None,
                yaw: a,
                id,
                foe: None,
                think_t: 0.0,
                left_at: None,
                lane_off: 0.0,
                giggle_t: 0.0,
                dance_t: 0.0,
                clay: Some(ClayKit { owner: gi, life: SLIDE_KIT_LIFE, heading: d, dmg }),
            });
        }
        if n > 0 {
            self.events.push(Event::ClayShaped { pos: at, n });
        }
    }

    /// `killer` killed something at `pos` worth `n` kittens: if he is
    /// Claypaw with his mudslide's surge on, they are shaped there.
    pub fn clay_kill_credit(&mut self, killer: usize, n: u32, pos: Vec3) {
        if killer >= self.gods.len() || self.gods[killer].clay_surge_t <= 0.0 || !is_clay(self.def(killer)) {
            return;
        }
        let heading = crate::hours::forward(self.gods[killer].team);
        self.slide_kittens(killer, pos, n, heading);
    }

    /// The mudslides run: each front advances a unit at a time; crushes
    /// through plants (weaker for each), sloshes back off anything else
    /// (weaker again, and able to strike what it struck before); strikes
    /// every enemy in its front once a leg, and carries every cat in it,
    /// friend or foe, along. Where a leg ends, it leaves a slick.
    pub fn waves_tick(&mut self, dt: f32) {
        let floor = self.blockers.floor();
        let mut done = Vec::new();
        for wi in 0..self.mudslides.len() {
            let mut w = self.mudslides[wi].clone();
            let mut travel = w.speed * dt;
            while travel > 0.0 && w.left > 0.0 {
                let stepd = travel.min(1.0);
                travel -= stepd;
                let next = w.pos + w.dir * stepd;
                // a wall ahead (the floor's edge, or something that is not
                // a plant): slosh back off it
                let side = Vec3::new(-w.dir.z, 0.0, w.dir.x);
                let mut normal: Option<Vec3> = None;
                if let Some(f) = floor {
                    if f.sdf(next.x, next.z) > -1.0 {
                        let e = 0.5;
                        let gx = f.sdf(next.x + e, next.z) - f.sdf(next.x - e, next.z);
                        let gz = f.sdf(next.x, next.z + e) - f.sdf(next.x, next.z - e);
                        normal = Some(-Vec3::new(gx, 0.0, gz).normalize_or(-w.dir));
                    }
                }
                let (plants, wall) = self.blockers.front_contacts(next, w.width * 0.25);
                // (the front is wide: its flanks crush plants too)
                let mut all_plants = plants;
                for s in [-0.35f32, 0.35] {
                    let (p2, _) = self.blockers.front_contacts(next + side * (w.width * s), w.width * 0.2);
                    all_plants.extend(p2);
                }
                if normal.is_none() {
                    normal = wall;
                }
                // the arena's rim, where no floor plan says (the jungle)
                let rim = crate::consts::arena_radius() - 2.0;
                if normal.is_none() && Vec3::new(next.x, 0.0, next.z).length() > rim {
                    normal = Some(-Vec3::new(next.x, 0.0, next.z).normalize_or(w.dir));
                }
                if let Some(n) = normal {
                    if w.dir.dot(n) < 0.0 {
                        if w.rebounds >= MAX_REBOUNDS {
                            w.left = 0.0;
                            break;
                        }
                        let from = w.leg_from;
                        self.lay_slick(from, w.pos, w.width * 0.5, 5.0, Vec3::ZERO);
                        w.dir = (w.dir - n * 2.0 * w.dir.dot(n)).normalize_or(-w.dir);
                        w.dir.y = 0.0;
                        w.mul *= REBOUND_MUL;
                        w.rebounds += 1;
                        w.hit.clear();
                        w.leg_from = w.pos;
                        self.events.push(Event::Slosh { pos: w.pos });
                        continue;
                    }
                }
                for p in all_plants {
                    if !w.smushed.contains(&p) {
                        w.smushed.push(p);
                        w.mul = (w.mul * PLANT_MUL).max(PLANT_FLOOR);
                        self.events.push(Event::Smush { pos: next });
                    }
                }
                w.pos = Vec3::new(next.x, crate::map::ground_y(next.x, next.z), next.z);
                w.left -= stepd;
            }
            // strike and carry what is in the front
            let side = Vec3::new(-w.dir.z, 0.0, w.dir.x);
            let in_front = |p: Vec3| {
                let d = Vec3::new(p.x - w.pos.x, 0.0, p.z - w.pos.z);
                d.dot(side).abs() <= w.width * 0.5 && d.dot(w.dir) <= 0.8 && d.dot(w.dir) >= -FRONT_DEPTH
            };
            let targets = self.aoe_targets(w.pos, w.width * 0.5 + FRONT_DEPTH, w.team);
            let mut strikes = Vec::new();
            for t in targets {
                let (key, at) = match t {
                    crate::sim::TargetRef::God(j) => (j as u64, self.gods[j].pos),
                    crate::sim::TargetRef::Minion(m) => (1_000_000 + self.minions[m].id as u64, self.minions[m].pos),
                    crate::sim::TargetRef::Boss(b) => (2_000_000 + b as u64, self.bosses.list[b].pos),
                    _ => continue,
                };
                if in_front(at) && !w.hit.contains(&key) {
                    w.hit.push(key);
                    strikes.push(t);
                }
            }
            for i in 0..self.gods.len() {
                let g = &mut self.gods[i];
                if g.respawn_t <= 0.0 && g.leap.is_none() && g.dash.is_none() && g.aloft_t <= 0.0 && in_front(g.pos) {
                    let to = g.pos + w.dir * w.speed * dt;
                    g.slide = w.dir * w.speed;
                    g.pos = self.blockers.resolve(Vec3::new(to.x, crate::map::ground_y(to.x, to.z), to.z), crate::consts::GOD_RADIUS);
                }
            }
            let dmg = w.dmg * w.mul;
            // minions by index shift as they die: strike from the highest
            strikes.sort_by_key(|t| std::cmp::Reverse(match t {
                crate::sim::TargetRef::Minion(m) => *m,
                _ => 0,
            }));
            let owner = w.owner;
            for t in strikes {
                self.deal_damage(t, dmg, true, Some(owner), 0.0, 0.0);
            }
            if w.left <= 0.0 {
                self.lay_slick(w.leg_from, w.pos, w.width * 0.5, 5.0, Vec3::ZERO);
                self.events.push(Event::Slosh { pos: w.pos });
                done.push(wi);
            }
            self.mudslides[wi] = w;
        }
        for wi in done.into_iter().rev() {
            self.mudslides.remove(wi);
        }
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

    /// A world with a wall across the way at x = `wall_x` (if any) and a
    /// jungle tree on the line at each of `trees`.
    fn world(wall_x: Option<f32>, trees: &[f32]) -> std::sync::Arc<crate::blockers::Blockers> {
        let mut b = crate::blockers::Blockers::new(300.0, 6.0);
        if let Some(x) = wall_x {
            b.push(crate::blockers::Blocker { shape: crate::blockers::Shape::Segment { x0: x, z0: -30.0, x1: x, z1: 30.0, r: 1.0 }, top: 6.0, cover: crate::blockers::Cover::NONE });
        }
        for &x in trees {
            b.push_prop(crate::blockers::PropKind::JungleTree, x, 3.0, 0.0, 1.0);
        }
        std::sync::Arc::new(b)
    }

    /// Claypaw (blue, 0) at the origin facing +x, a friend (1) and an enemy
    /// (2) placed; no minions; the mudslide cast along +x.
    fn slide(blockers: std::sync::Arc<crate::blockers::Blockers>, friend: Vec3, enemy: Vec3) -> Game {
        let mut g = Game::new_teams(&[claypaw(), 0], &[1]);
        for c in g.gods.iter_mut() {
            c.ai = None;
            c.max_hp = 1e5;
            c.hp = 1e5;
        }
        g.minions.clear();
        g.next_wave_t = 1e9;
        g.blockers = blockers;
        g.gods[0].pos = Vec3::ZERO;
        g.gods[1].pos = friend;
        g.gods[2].pos = enemy;
        g
    }

    fn run_out(g: &mut Game) {
        for _ in 0..(8 * 60) {
            g.t += 1.0 / 60.0;
            g.clay_tick(1.0 / 60.0);
        }
    }

    /// The front strikes an enemy in its way once (not every tick it is in
    /// it), spares a friend, and carries both along the way it runs.
    #[test]
    fn the_mudslide_strikes_once_and_carries_friend_and_foe() {
        let mut g = slide(world(None, &[]), Vec3::new(8.0, 0.0, 4.0), Vec3::new(12.0, 0.0, -3.0));
        g.mudslide(0, Vec3::X, 40.0, 16.0, 16.0, 280.0, 0);
        let (f0, e0) = (g.gods[1].pos.x, g.gods[2].pos.x);
        // until it spends itself: then there is mud where it ran
        let mut n = 0;
        while !g.mudslides.is_empty() && n < 600 {
            g.t += 1.0 / 60.0;
            g.clay_tick(1.0 / 60.0);
            n += 1;
        }
        assert!(!g.slicks.is_empty(), "no mud left behind");
        run_out(&mut g);
        let lost = 1e5 - g.gods[2].hp;
        assert!(lost > 150.0 && lost < 290.0, "the enemy lost {lost} (once is at most 280)");
        assert_eq!(g.gods[1].hp, 1e5, "the friend was struck");
        assert!(g.gods[1].pos.x - f0 > 4.0 && g.gods[2].pos.x - e0 > 4.0, "not carried: {} and {}", g.gods[1].pos.x - f0, g.gods[2].pos.x - e0);
        assert!(g.mudslides.is_empty(), "still running");
    }

    /// Plants: the front crushes through them (it does not stop), weaker
    /// for each -- an enemy behind two trees takes less than one in the
    /// open.
    #[test]
    fn plants_are_crushed_through_but_weaken_it() {
        let hit = |trees: &[f32]| {
            let mut g = slide(world(None, trees), Vec3::new(0.0, 0.0, 60.0), Vec3::new(20.0, 0.0, 0.0));
            g.mudslide(0, Vec3::X, 40.0, 16.0, 16.0, 280.0, 0);
            run_out(&mut g);
            (1e5 - g.gods[2].hp, g.events.iter().filter(|e| matches!(e, Event::Smush { .. })).count())
        };
        let (open, _) = hit(&[]);
        let (through, smushed) = hit(&[6.0, 12.0]);
        assert!(smushed >= 2, "{smushed} plants crushed");
        assert!(through < open * 0.9 && through > open * PLANT_FLOOR - 1.0, "through two trees {through}, in the open {open}");
    }

    /// A wall: the front sloshes back off it, weaker, and can strike again
    /// what it struck on the way in.
    #[test]
    fn a_wall_sends_it_back_weaker() {
        let mut g = slide(world(Some(20.0), &[]), Vec3::new(0.0, 0.0, 60.0), Vec3::new(10.0, 0.0, 0.0));
        g.gods[2].pos = Vec3::new(10.0, 0.0, 0.0);
        g.mudslide(0, Vec3::X, 40.0, 16.0, 16.0, 280.0, 0);
        // hold the enemy where it stands, to be met twice
        let mut struck = Vec::new();
        let mut last = g.gods[2].hp;
        for _ in 0..(8 * 60) {
            g.t += 1.0 / 60.0;
            g.gods[2].pos = Vec3::new(10.0, 0.0, 0.0);
            g.clay_tick(1.0 / 60.0);
            if g.gods[2].hp < last {
                struck.push(last - g.gods[2].hp);
                last = g.gods[2].hp;
            }
        }
        assert!(g.events.iter().any(|e| matches!(e, Event::Slosh { .. })), "no slosh");
        assert_eq!(struck.len(), 2, "struck {struck:?}");
        assert!((struck[1] / struck[0] - REBOUND_MUL).abs() < 0.05, "the rebound struck {:.0} after {:.0}", struck[1], struck[0]);
    }

    /// Eight kittens from the bank; during the surge a god kill shapes
    /// three more and a minion or beast one; after it, none.
    #[test]
    fn kills_in_the_surge_shape_more_kittens() {
        let mut g = slide(world(None, &[]), Vec3::new(0.0, 0.0, 60.0), Vec3::new(0.0, 0.0, -60.0));
        let kits = |g: &Game| g.minions.iter().filter(|m| m.clay.is_some()).count();
        g.mudslide(0, Vec3::X, 40.0, 16.0, 16.0, 280.0, SLIDE_KITS);
        assert_eq!(kits(&g), 8);
        g.clay_kill_credit(0, KITS_PER_GOD_KILL, Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(kits(&g), 11);
        g.clay_kill_credit(0, KITS_PER_SMALL_KILL, Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(kits(&g), 12);
        g.clay_kill_credit(1, KITS_PER_GOD_KILL, Vec3::ZERO);
        assert_eq!(kits(&g), 12, "someone else's kill shaped kittens");
        // a real kill, through the game's own death: three more
        g.gods[2].hp = 10.0;
        g.deal_damage(crate::sim::TargetRef::God(2), 100.0, true, Some(0), 0.0, 0.0);
        assert!(g.gods[2].respawn_t > 0.0, "the enemy did not die");
        assert_eq!(kits(&g), 15, "a god slain in the surge");
        g.gods[0].clay_surge_t = 0.0;
        g.clay_kill_credit(0, KITS_PER_GOD_KILL, Vec3::ZERO);
        assert_eq!(kits(&g), 12, "kittens after the surge");
    }

    /// Each kitten broken is SLIDE_KIT_GOLD to its breaker; one that runs
    /// out of shape is nobody's gold.
    #[test]
    fn a_broken_kitten_is_gold_to_its_breaker() {
        let mut g = slide(world(None, &[]), Vec3::new(0.0, 0.0, 60.0), Vec3::new(0.0, 0.0, -60.0));
        g.slide_kittens(0, Vec3::new(5.0, 0.0, 0.0), 2, Vec3::X);
        let gold = g.gods[2].gold;
        g.deal_damage(crate::sim::TargetRef::Minion(0), 1e4, false, Some(2), 0.0, 0.0);
        let got = g.gods[2].gold - gold;
        assert!(got >= SLIDE_KIT_GOLD * 0.99, "breaking a kitten paid {got}");
        let gold = g.gods[2].gold;
        for _ in 0..((SLIDE_KIT_LIFE + 1.0) * 10.0) as usize {
            g.clay_tick(0.1);
        }
        assert!(g.minions.iter().all(|m| m.clay.is_none()), "a kitten outlived its shape");
        assert_eq!(g.gods[2].gold, gold, "gold for a kitten that fell apart");
    }

}
