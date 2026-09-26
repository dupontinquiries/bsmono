// The living jungle — headless nature systems owned by the sim.
//
//   Heal Trees   Six sanctuary trees from Clawbound's lore, three a side,
//                off-lane in the jungle. An ancestor resting beneath one
//                (no damage taken for 4 s) slowly mends; Sweetrain doubles it.
//   Prey         Hares and stags graze the jungle in small groups. They
//                flee any ancestor that comes close; an ancestor that gets
//                within pounce range of a fleeing animal *catches* it — gold,
//                a mouthful of health, and a kill-feed line. AI ancestors
//                catch prey they blunder into; the player hunts on purpose.
//   Prey Run     A map event: a great herd of stags pours down the rivers.
//   Glowfish     One per river, rare and quick, swimming the channel. Catch
//                it (dash in, or wade fast) for the Riverblessing — the
//                jungle-camp buff of this map: speed, attack speed, mana
//                regen, and the rivers stop slowing you for 45 s.
//
// Everything here is data + arithmetic on the game rng — no render types —
// so it runs under `cargo test`. Rendering reads `Game::wild` directly.

use crate::consts::{lane_clearance, river_dist, river_x, water_depth, Team, ARENA_RADIUS, BLESSING_SECS, MAP_SCALE};
use crate::gods::GodDef;

/// Rainspots and Riptail were born to the rivers: no wading penalty.
pub fn is_riverborn(def: &GodDef) -> bool {
    matches!(def.name, "Rainspots" | "Riptail")
}
use crate::sim::{Event, Game, Rng};
use glam::Vec3;

// --- heal trees ------------------------------------------------------------

pub const HEAL_TREE_RADIUS: f32 = 4.8;
pub const HEAL_TREE_HP_PER_S: f32 = 0.018; // fraction of max hp
pub const HEAL_TREE_MP_PER_S: f32 = 0.012;
pub const REST_SECS: f32 = 4.0;

#[derive(Clone, Copy, Debug)]
pub struct HealTree {
    pub pos: Vec3,
    /// 0..1: how many ancestors it is currently mending, smoothed (for VFX).
    pub glow: f32,
}

/// Sanctuary positions: three in the Blue half — one west jungle, one east
/// jungle, one near the base — found deterministically (fixed seed) in
/// pockets clear of lanes, rivers and bases, then mirrored through the
/// centre so both sides have the same walk.
pub fn heal_tree_positions() -> [Vec3; 6] {
    match crate::map::active() {
        crate::map::MapId::Pass => return crate::pass::heal_tree_positions(),
        crate::map::MapId::Sky => return crate::sky::heal_tree_positions(),
        crate::map::MapId::Jungle => {}
    }
    let mut rng = Rng::new(0x7EE5);
    let s = MAP_SCALE;
    // (x range, z range) in unscaled map units, Blue half
    let bands: [((f32, f32), (f32, f32)); 3] = [((-70.0, -25.0), (-55.0, -15.0)), ((25.0, 70.0), (-55.0, -15.0)), ((-30.0, 30.0), (-80.0, -50.0))];
    let mut blue = [Vec3::ZERO; 3];
    for (i, ((x0, x1), (z0, z1))) in bands.iter().enumerate() {
        let mut best = Vec3::new((x0 + x1) * 0.5 * s, 0.0, (z0 + z1) * 0.5 * s);
        let mut best_clear = f32::MIN;
        for _ in 0..200 {
            let x = rng.range(*x0, *x1) * s;
            let z = rng.range(*z0, *z1) * s;
            let clear = lane_clearance(x, z).min(river_dist(x, z) - 2.0);
            if clear > best_clear && Vec3::new(x, 0.0, z).length() < ARENA_RADIUS - 10.0 {
                best_clear = clear;
                best = Vec3::new(x, 0.0, z);
            }
        }
        blue[i] = best;
    }
    let mut out = [Vec3::ZERO; 6];
    for (i, b) in blue.iter().enumerate() {
        out[i] = *b;
        out[i + 3] = Vec3::new(-b.x, 0.0, -b.z);
    }
    out
}

// --- prey ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreyKind {
    Hare,
    Stag,
    Glowfish,
}

impl PreyKind {
    pub fn gold(self) -> f32 {
        match self {
            PreyKind::Hare => 22.0,
            PreyKind::Stag => 55.0,
            PreyKind::Glowfish => 40.0,
        }
    }
    pub fn heal_frac(self) -> f32 {
        match self {
            PreyKind::Hare => 0.05,
            PreyKind::Stag => 0.11,
            PreyKind::Glowfish => 0.06,
        }
    }
    pub fn speed(self) -> f32 {
        match self {
            PreyKind::Hare => 10.5,
            PreyKind::Stag => 12.5,
            PreyKind::Glowfish => 8.5,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            PreyKind::Hare => "a hare",
            PreyKind::Stag => "a stag",
            PreyKind::Glowfish => "the glowfish",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Prey {
    pub kind: PreyKind,
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    /// > 0 while bolting from a hunter.
    pub fear: f32,
    /// Grazing anchor the animal drifts back toward.
    pub home: Vec3,
    pub anim: f32,
    /// Prey Run stags follow the river south (Blue) or north (Red) and leave.
    pub run: Option<f32>,
    pub graze_t: f32,
}

pub const PREY_NOTICE: f32 = 9.0;
pub const GLOWFISH_RESPAWN: f32 = 75.0;
pub const PREY_POUNCE: f32 = 1.7;
pub const MAX_PREY: usize = 26;
pub const PREY_RUN_COUNT: usize = 14;

#[derive(Clone, Debug, Default)]
pub struct Wild {
    pub trees: Vec<HealTree>,
    pub prey: Vec<Prey>,
    pub next_spawn_t: f32,
    /// total animals caught, per team (for the scoreboard)
    pub caught: [u32; 2],
    /// per river (west, east): when the next glowfish surfaces
    pub glowfish_t: [f32; 2],
}

impl Wild {
    pub fn new(rng: &mut Rng) -> Wild {
        let mut w = Wild {
            trees: heal_tree_positions().iter().map(|&pos| HealTree { pos, glow: 0.0 }).collect(),
            prey: Vec::new(),
            next_spawn_t: 12.0,
            caught: [0, 0],
            glowfish_t: [40.0, 55.0],
        };
        // a few groups already grazing when the match opens
        for _ in 0..4 {
            w.spawn_group(rng, None);
        }
        w
    }

    fn jungle_spot(rng: &mut Rng) -> Option<Vec3> {
        for _ in 0..40 {
            let a = rng.f32() * std::f32::consts::TAU;
            let r = 26.0 + rng.f32() * (crate::consts::arena_radius() - 40.0);
            let x = a.cos() * r;
            let z = a.sin() * r;
            if lane_clearance(x, z) > 7.0 && river_dist(x, z) > 8.0 && !crate::bosses::in_pit(x, z) && crate::map::open_ground(x, z, 2.0) {
                return Some(Vec3::new(x, 0.0, z));
            }
        }
        None
    }

    pub fn spawn_group(&mut self, rng: &mut Rng, at: Option<Vec3>) {
        let Some(home) = at.or_else(|| Self::jungle_spot(rng)) else { return };
        let stags = rng.f32() < 0.3;
        let n = if stags { 2 + (rng.next_u() % 2) as usize } else { 3 + (rng.next_u() % 3) as usize };
        for _ in 0..n {
            if self.prey.len() >= MAX_PREY {
                break;
            }
            let off = Vec3::new(rng.range(-2.5, 2.5), 0.0, rng.range(-2.5, 2.5));
            self.prey.push(Prey {
                kind: if stags { PreyKind::Stag } else { PreyKind::Hare },
                pos: home + off,
                vel: Vec3::ZERO,
                yaw: rng.f32() * std::f32::consts::TAU,
                fear: 0.0,
                home,
                anim: rng.f32() * 6.0,
                run: None,
                graze_t: rng.range(1.0, 4.0),
            });
        }
    }

    /// A glowfish surfaces mid-channel on river `side` (-1 west, 1 east).
    pub fn spawn_glowfish(&mut self, rng: &mut Rng, side: f32) {
        let z = rng.range(-60.0, 60.0);
        let x = river_x(side, z);
        self.prey.push(Prey {
            kind: PreyKind::Glowfish,
            pos: Vec3::new(x, 0.0, z),
            vel: Vec3::ZERO,
            yaw: 0.0,
            fear: 0.0,
            home: Vec3::new(x, 0.0, z),
            anim: rng.f32() * 6.0,
            run: None,
            graze_t: 0.0,
        });
    }

    /// The Prey Run: a herd of stags enters at the north end of each river
    /// and runs south (side = ±1 picks the river).
    pub fn spawn_prey_run(&mut self, rng: &mut Rng) {
        for k in 0..PREY_RUN_COUNT {
            let side = if k % 2 == 0 { -1.0 } else { 1.0 };
            let z = 110.0 + rng.range(0.0, 30.0) + (k / 2) as f32 * 4.0;
            let x = river_x(side, z) + rng.range(-2.5, 2.5);
            self.prey.push(Prey {
                kind: PreyKind::Stag,
                pos: Vec3::new(x, 0.0, z),
                vel: Vec3::ZERO,
                yaw: -std::f32::consts::FRAC_PI_2,
                fear: 0.0,
                home: Vec3::new(x, 0.0, -140.0),
                anim: rng.f32() * 6.0,
                run: Some(side),
                graze_t: 0.0,
            });
        }
    }
}

impl Game {
    /// Trees + prey, one sim step. Called from `Game::tick` after the sky.
    pub fn wild_tick(&mut self, dt: f32) {
        self.heal_trees_tick(dt);
        self.prey_tick(dt);
    }

    fn heal_trees_tick(&mut self, dt: f32) {
        let now = self.t;
        let rain_mul = if self.map_events.is(crate::map_events::MapEventKind::Sweetrain) { 2.0 } else { 1.0 };
        for ti in 0..self.wild.trees.len() {
            let tp = self.wild.trees[ti].pos;
            let mut resting = 0.0f32;
            for g in self.gods.iter_mut() {
                if g.respawn_t > 0.0 || g.pos.distance(tp) > HEAL_TREE_RADIUS {
                    continue;
                }
                if now - g.last_dmg_t < REST_SECS {
                    continue;
                }
                let hp0 = g.hp;
                g.hp = (g.hp + g.max_hp * HEAL_TREE_HP_PER_S * rain_mul * dt).min(g.max_hp);
                g.mana = (g.mana + g.max_mana * HEAL_TREE_MP_PER_S * rain_mul * dt).min(g.max_mana);
                if g.hp > hp0 + 1e-3 {
                    resting += 1.0;
                }
            }
            let tree = &mut self.wild.trees[ti];
            let target = resting.min(1.0);
            tree.glow += (target - tree.glow) * (dt * 2.5).min(1.0);
        }
    }

    fn prey_tick(&mut self, dt: f32) {
        // spawn a fresh group now and then, keeping the jungle populated
        if self.t >= self.wild.next_spawn_t {
            self.wild.next_spawn_t = self.t + self.rng.range(18.0, 34.0);
            let grazing = self.wild.prey.iter().filter(|p| p.run.is_none()).count();
            if grazing < 16 {
                let mut wild = std::mem::take(&mut self.wild);
                wild.spawn_group(&mut self.rng, None);
                self.wild = wild;
            }
        }

        for (k, side) in [-1.0f32, 1.0].iter().enumerate() {
            if self.t >= self.wild.glowfish_t[k] {
                self.wild.glowfish_t[k] = self.t + GLOWFISH_RESPAWN;
                let alive = self.wild.prey.iter().any(|p| p.kind == PreyKind::Glowfish && (p.pos.x.signum() == side.signum()));
                if !alive {
                    let mut wild = std::mem::take(&mut self.wild);
                    wild.spawn_glowfish(&mut self.rng, *side);
                    self.wild = wild;
                    self.events.push(Event::Announce { text: "A GLOWFISH SURFACES — the river blesses whoever catches it".to_string() });
                }
            }
        }

        let hunters: Vec<(usize, Vec3, bool, Team)> = self
            .gods
            .iter()
            .enumerate()
            .filter(|(_, g)| g.respawn_t <= 0.0)
            .map(|(i, g)| (i, g.pos, g.move_dir.length_squared() > 0.01 || g.attack_held, g.team))
            .collect();

        let mut caught: Vec<(usize, usize)> = Vec::new(); // (prey idx, hunter idx)
        let snow = self.weather.atmosphere().snow;
        for (pi, p) in self.wild.prey.iter_mut().enumerate() {
            p.anim += dt * (if p.fear > 0.0 { 9.0 } else { 3.0 });
            p.fear = (p.fear - dt).max(0.0);

            // nearest hunter
            let mut nearest: Option<(usize, f32, Vec3)> = None;
            for (hi, hp, _, _) in &hunters {
                let d = hp.distance(p.pos);
                if nearest.map_or(true, |(_, bd, _)| d < bd) {
                    nearest = Some((*hi, d, *hp));
                }
            }

            let mut desired = Vec3::ZERO;
            if let Some((hi, d, hp)) = nearest {
                if d < PREY_POUNCE {
                    caught.push((pi, hi));
                    continue;
                }
                // snow muffles the approach: animals notice later
                let notice = PREY_NOTICE * (1.0 - 0.3 * snow);
                if d < notice {
                    p.fear = 2.5;
                    let away = (p.pos - hp).normalize_or_zero();
                    desired = away * p.kind.speed();
                }
            }
            if p.kind == PreyKind::Glowfish {
                // swim the channel: drift along the flow, dart when frightened,
                // always pulled back toward the centreline
                let side = p.home.x.signum();
                let cx = river_x(side, p.pos.z);
                let dir = if p.fear > 0.0 { desired.z.signum() } else { ((p.anim * 0.15).sin()).signum() };
                let ahead = p.pos.z + dir * 6.0;
                let target = Vec3::new(river_x(side, ahead), 0.0, ahead);
                let speed = if p.fear > 0.0 { p.kind.speed() } else { 2.2 };
                desired = (target - p.pos).normalize_or_zero() * speed + Vec3::new((cx - p.pos.x) * 0.8, 0.0, 0.0);
                if p.pos.z.abs() > 95.0 {
                    p.anim += 20.0; // turn around at the ends of the channel
                }
            } else if p.fear <= 0.0 {
                if let Some(side) = p.run {
                    // the run: hug the river, head south, leave the map
                    let ahead = p.pos.z - 14.0;
                    let target = Vec3::new(river_x(side, ahead), 0.0, ahead);
                    desired = (target - p.pos).normalize_or_zero() * p.kind.speed() * 0.85;
                } else {
                    p.graze_t -= dt;
                    let dh = p.home - p.pos;
                    if dh.length() > 6.0 {
                        desired = dh.normalize() * 2.5;
                    } else if p.graze_t <= 0.0 {
                        p.graze_t = self.rng.range(1.5, 5.0);
                        let a = self.rng.f32() * std::f32::consts::TAU;
                        p.vel = Vec3::new(a.cos(), 0.0, a.sin()) * 1.6;
                    } else {
                        desired = p.vel * 0.6;
                    }
                }
            }
            // steer + move; stay out of the rivers unless running them
            let k = if p.fear > 0.0 { 9.0 } else { 2.5 };
            p.vel += (desired - p.vel) * (k * dt).min(1.0);
            let next = p.pos + p.vel * dt;
            let blocked = match p.kind {
                PreyKind::Glowfish => water_depth(next.x, next.z) < 0.25,
                _ => p.run.is_none() && river_dist(next.x, next.z) < 6.5,
            };
            if !blocked && next.length() < crate::consts::arena_radius() + 60.0 {
                p.pos = next;
            } else {
                p.vel *= -0.5;
            }
            if p.vel.length_squared() > 0.05 {
                p.yaw = p.vel.z.atan2(p.vel.x);
            }
        }

        // the hunt
        for (pi, hi) in caught.into_iter().rev() {
            if pi >= self.wild.prey.len() {
                continue;
            }
            let p = self.wild.prey.remove(pi);
            let gold = p.kind.gold() * self.map_events.gold_mul();
            let team = self.gods[hi].team;
            let name = self.god_name(hi);
            {
                let g = &mut self.gods[hi];
                g.gold += gold;
                g.hp = (g.hp + g.max_hp * p.kind.heal_frac()).min(g.max_hp);
                if p.kind == PreyKind::Glowfish {
                    g.blessing_t = BLESSING_SECS;
                }
            }
            self.wild.caught[team as usize] += 1;
            self.events.push(Event::PreyCaught { pos: p.pos, kind: p.kind, gold });
            match p.kind {
                PreyKind::Stag => self.events.push(Event::Kill { killer: name.to_string(), victim: "a stag".to_string(), victim_team: team.other(), is_god: false }),
                PreyKind::Glowfish => {
                    self.events.push(Event::Kill { killer: name.to_string(), victim: "the glowfish".to_string(), victim_team: team.other(), is_god: false });
                    self.events.push(Event::Announce { text: format!("{} carries the RIVERBLESSING", name) });
                }
                PreyKind::Hare => {}
            }
        }

        // animals that ran off the map are gone
        self.wild.prey.retain(|p| p.run.is_none() || p.pos.z > -150.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9])
    }

    #[test]
    fn heal_trees_sit_off_lane_and_mirror() {
        let ps = heal_tree_positions();
        for p in ps {
            assert!(lane_clearance(p.x, p.z) > 5.0, "heal tree on a lane at {p}");
            assert!(p.length() < ARENA_RADIUS, "heal tree outside the arena at {p}");
        }
        for i in 0..3 {
            assert!((ps[i] + ps[i + 3]).length() < 1e-3, "trees must mirror through the centre");
        }
    }

    #[test]
    fn resting_under_a_tree_mends_and_fighting_does_not() {
        let mut g = game();
        let tp = g.wild.trees[0].pos;
        g.gods[0].ai = None;
        g.gods[0].pos = tp;
        g.gods[0].hp = 100.0;
        g.gods[0].last_dmg_t = -100.0;
        for _ in 0..60 {
            g.wild_tick(1.0 / 30.0);
        }
        assert!(g.gods[0].hp > 100.0, "should mend at rest");
        assert!(g.wild.trees[0].glow > 0.5, "the tree should glow while mending");
        let hp = g.gods[0].hp;
        g.gods[0].last_dmg_t = g.t; // just took a hit
        for _ in 0..30 {
            g.wild_tick(1.0 / 30.0);
        }
        assert!((g.gods[0].hp - hp).abs() < 1e-3, "no mending in combat");
    }

    #[test]
    fn prey_flee_and_can_be_caught() {
        let mut g = game();
        let hare_pos = g.wild.prey[0].pos;
        g.gods[0].ai = None;
        g.gods[0].pos = hare_pos + Vec3::new(6.0, 0.0, 0.0);
        let d0 = g.wild.prey[0].pos.distance(g.gods[0].pos);
        for _ in 0..20 {
            g.prey_tick(1.0 / 30.0);
        }
        let d1 = g.wild.prey[0].pos.distance(g.gods[0].pos);
        assert!(d1 > d0, "the hare should bolt: {d0} -> {d1}");

        let n = g.wild.prey.len();
        let gold = g.gods[0].gold;
        g.gods[0].pos = g.wild.prey[0].pos;
        g.events.clear();
        g.prey_tick(1.0 / 30.0);
        assert!(g.wild.prey.len() < n, "pounce range catches (a whole huddle can go at once)");
        assert!(g.gods[0].gold > gold);
        assert!(g.events.iter().any(|e| matches!(e, Event::PreyCaught { .. })));
    }

    #[test]
    fn glowfish_swims_the_river_and_blesses_its_catcher() {
        let mut g = game();
        for gg in g.gods.iter_mut() {
            gg.ai = None;
            gg.pos = Vec3::new(0.0, 0.0, 0.0);
        }
        let mut wild = std::mem::take(&mut g.wild);
        wild.spawn_glowfish(&mut g.rng, 1.0);
        g.wild = wild;
        let fi = g.wild.prey.iter().position(|p| p.kind == PreyKind::Glowfish).unwrap();
        for _ in 0..(20 * 30) {
            g.prey_tick(1.0 / 30.0);
            let p = &g.wild.prey[fi];
            assert!(water_depth(p.pos.x, p.pos.z) > 0.2, "the glowfish must stay in the water: {}", p.pos);
        }
        let p = g.wild.prey[fi].pos;
        g.gods[0].pos = p;
        g.prey_tick(1.0 / 30.0);
        assert!(g.gods[0].blessing_t > 0.0, "catching it grants the Riverblessing");
        // blessed: no wading penalty
        let slow = g.move_speed(0);
        g.gods[0].blessing_t = 0.0;
        let unblessed = g.move_speed(0);
        assert!(slow > unblessed, "the blessing should carry you through the river: {slow} vs {unblessed}");
    }

    #[test]
    fn rivers_slow_wading_but_not_the_riverborn() {
        let mut g = game();
        let z = 10.0;
        let x = river_x(1.0, z);
        assert!(water_depth(x, z) > 1.0, "mid-channel should be deep: {}", water_depth(x, z));
        let dry = g.move_speed(0);
        g.gods[0].pos = Vec3::new(x, 0.0, z);
        let wading = g.move_speed(0);
        assert!(wading < dry * 0.75, "wading should cost real speed: {wading} vs {dry}");
        // put Riptail in slot 0 (the seeded 5v5 may not have picked them)
        let rip = crate::gods::ROSTER.iter().find(|d| d.name == "Riptail").unwrap().id;
        g.gods[0].def_id = rip;
        let born = g.move_speed(0);
        assert!((born - crate::gods::god(rip).move_speed).abs() < 1e-3, "Riptail wades free: {born}");
    }

    #[test]
    fn prey_run_herd_leaves_the_map() {
        let mut g = game();
        for gg in g.gods.iter_mut() {
            gg.ai = None;
            gg.pos = Vec3::new(0.0, 0.0, 0.0);
        }
        let mut wild = std::mem::take(&mut g.wild);
        wild.spawn_prey_run(&mut g.rng);
        g.wild = wild;
        let runners = g.wild.prey.iter().filter(|p| p.run.is_some()).count();
        assert_eq!(runners, PREY_RUN_COUNT);
        for _ in 0..(60 * 30) {
            g.prey_tick(1.0 / 30.0);
        }
        let left = g.wild.prey.iter().filter(|p| p.run.is_some()).count();
        assert!(left < runners, "the herd should have started leaving: {left}/{runners}");
    }
}
