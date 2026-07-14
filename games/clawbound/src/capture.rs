//! capture.rs — "Pop Island 2 · Tails of the Wild", the team yarn-ball capture mode,
//! ported from the browser prototype as pure, deterministic, renderer-free logic.
//!
//! Two teams (Tawny vs Silver) of terrain-specialised creatures ([`crate::class`])
//! race to carry **yarn balls** back to their base. **Firecrackers** stun anyone
//! caught in the blast and make carriers drop their yarn; **power-ups** grant speed,
//! a one-hit shield, or a triple throw ([`crate::powerup`]). Two timed periods —
//! most captures wins. Movement speed comes from the world heightfield, so who's
//! quick depends on whether they're on land, in the water, or up the mountains.

use crate::class::Class;
use crate::powerup::{Buffs, Power, POWERS};
use crate::world;

pub const TAWNY: u8 = 0;
pub const SILVER: u8 = 1;
/// Nominal base anchors; the real bases are nudged to dry land near these at match start.
pub const TAWNY_ANCHOR: (f32, f32) = (0.0, -70.0);
pub const SILVER_ANCHOR: (f32, f32) = (0.0, 70.0);
pub const BASE_R: f32 = 7.0;
const ARENA: f32 = 90.0;
const PER_TEAM: usize = 3;
const PERIOD_LEN: f32 = 60.0;
const PERIODS: u8 = 2;
const OVERTIME_LEN: f32 = 30.0;   // sudden-death window after a tie
const FC_SPEED: f32 = 26.0;
const FC_FUSE: f32 = 0.9;
const FC_RADIUS: f32 = 4.0;
const FC_CD: f32 = 1.6;
const PICKUP_R: f32 = 1.6;
const PU_INTERVAL: f32 = 8.0;
const LEAP_DUR: f32 = 0.32;
const MOVE_REACH: f32 = 3.2;    // melee autoattack range
const ULT_CD_MULT: f32 = 1.8;   // stretch the shared Ability's cooldown to feel like an ultimate

const YARN_SPAWNS: [(f32, f32); 6] = [(-20.0, -18.0), (20.0, -18.0), (-20.0, 18.0), (20.0, 18.0), (0.0, -22.0), (0.0, 22.0)];
const PU_SPAWNS: [(f32, f32); 6] = [(-30.0, 0.0), (30.0, 0.0), (0.0, 0.0), (-14.0, -40.0), (14.0, 40.0), (0.0, -8.0)];

fn hashf(mut h: u32) -> f32 {
    h ^= h >> 16; h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15; h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16; (h as f32) / (u32::MAX as f32)
}
fn norm(x: f32, z: f32) -> (f32, f32) { let l = (x * x + z * z).sqrt(); if l > 1e-5 { (x / l, z / l) } else { (0.0, 0.0) } }
fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }

/// Nearest dry, reasonably flat spot to an anchor — a base you can actually reach on foot.
fn land_near(ax: f32, az: f32) -> (f32, f32) {
    let mut best = (ax, az);
    let mut best_score = f32::MAX;
    for ring in 0..16 {
        let r = ring as f32 * 4.0;
        let steps = (6 + ring * 3).max(1);
        for s in 0..steps {
            let a = s as f32 / steps as f32 * std::f32::consts::TAU;
            let (x, z) = (ax + a.cos() * r, az + a.sin() * r);
            let h = world::height_at(x, z);
            if h < world::WATER_Y + 0.3 { continue; }   // must be dry
            let slope = (world::height_at(x + 2.0, z) - h).abs() + (world::height_at(x, z + 2.0) - h).abs();
            let score = slope * 3.0 + r * 0.05;          // flat + close to the anchor
            if score < best_score { best_score = score; best = (x, z); }
        }
        if best_score < 0.5 && ring >= 2 { break; }
    }
    best
}

pub struct Mob {
    pub team: u8,
    pub class: Class,
    pub x: f32, pub z: f32, pub heading: f32,
    pub buffs: Buffs,
    pub carrying: bool,
    pub stun_t: f32,
    pub leap_t: f32,
    pub cd: f32,
    pub is_player: bool,
    pub captures: u32,
    pub stuns: u32,
}
impl Mob {
    pub fn stunned(&self) -> bool { self.stun_t > 0.0 }
}

pub struct Cracker { pub x: f32, pub z: f32, vx: f32, vz: f32, fuse: f32, owner: usize }
pub struct Pickup { pub x: f32, pub z: f32, pub kind: Power }

pub struct Capture {
    pub mobs: Vec<Mob>,
    pub yarn_free: Vec<(f32, f32)>,
    pub crackers: Vec<Cracker>,
    pub powerups: Vec<Pickup>,
    pub score: [u32; 2],
    pub period: u8,
    pub period_t: f32,
    pub time: f32,
    pub outcome: Option<u8>,     // winning team (TAWNY/SILVER) or 2 = draw
    pub overtime: bool,          // in sudden-death after a tie
    pub bases: [(f32, f32); 2],
    pu_spawn_t: f32,
    tick: u32,
    input: Option<(f32, f32, bool, bool)>,   // mx, mz, throw, jump
    pub player_move: Option<(crate::combat::Move, f32)>,   // player's in-progress autoattack
    pub combo_last: Option<(crate::combat::Move, f32)>,
    pub combo_flash: f32,
    pub combo_name: &'static str,
    pub ability_cd: f32,      // the player's ultimate (class.ability())
    pub ability_t: f32,
    ability_want: bool,
}

impl Capture {
    pub fn base(&self, team: u8) -> (f32, f32) { self.bases[team as usize] }

    /// New match. The player is mob 0 on Tawny with the chosen class; the rest of
    /// each team is filled with a spread of classes.
    pub fn new(player_class: Class) -> Capture {
        world::init_world();
        let bases = [land_near(TAWNY_ANCHOR.0, TAWNY_ANCHOR.1), land_near(SILVER_ANCHOR.0, SILVER_ANCHOR.1)];
        let fill = [Class::Squirrel, Class::Otter, Class::Frog, Class::Goat, Class::Hawk, Class::Cat];
        let mut mobs = Vec::new();
        for team in [TAWNY, SILVER] {
            let base = bases[team as usize];
            for i in 0..PER_TEAM {
                let is_player = team == TAWNY && i == 0;
                let class = if is_player { player_class } else { fill[(team as usize * PER_TEAM + i) % fill.len()] };
                let a = i as f32 / PER_TEAM as f32 * std::f32::consts::TAU;
                let (x, z) = (base.0 + a.cos() * 8.0, base.1 + a.sin() * 8.0 * 0.3);
                mobs.push(Mob { team, class, x, z, heading: 0.0, buffs: Buffs::new(), carrying: false, stun_t: 0.0, leap_t: 0.0, cd: 0.0, is_player, captures: 0, stuns: 0 });
            }
        }
        let yarn_free = YARN_SPAWNS.to_vec();
        Capture { mobs, yarn_free, crackers: Vec::new(), powerups: Vec::new(), score: [0, 0], period: 1, period_t: PERIOD_LEN, time: 0.0, outcome: None, overtime: false, bases, pu_spawn_t: PU_INTERVAL, tick: 0, input: None,
            player_move: None, combo_last: None, combo_flash: 0.0, combo_name: "", ability_cd: 0.0, ability_t: 0.0, ability_want: false }
    }

    pub fn set_input(&mut self, mx: f32, mz: f32, throw: bool, jump: bool) { self.input = Some((mx, mz, throw, jump)); }

    /// Perform a flick — the same gesture-driven autoattack as Prowl/the 6v1 mode
    /// (ported from `combat.rs`). `dir_local` is the flick in the player's local frame
    /// (`.1` forward, `.0` right). Chains combos, and a landed hit stuns the nearest
    /// enemy in range and makes it drop any yarn it's carrying — a melee option
    /// alongside the firecracker throw.
    pub fn player_flick(&mut self, dir_local: (f32, f32), mag: f32, low: bool) {
        use crate::combat::{combo, resolve_flick, COMBO_WINDOW};
        if self.player_move.is_some() { return; }
        let mut mv = match resolve_flick(dir_local, mag, low) { Some(m) => m, None => return };
        if let Some((prev, since)) = self.combo_last {
            if since < COMBO_WINDOW {
                if let Some((name, finisher)) = combo(prev, mv) {
                    mv = finisher; self.combo_name = name; self.combo_flash = 1.0;
                }
            }
        }
        self.player_move = Some((mv, 0.0));
    }

    /// Press the player's ultimate (class.ability()), consumed on the next `step`.
    pub fn set_ability(&mut self, on: bool) { if on { self.ability_want = true; } }
    pub fn ability_ready(&self) -> bool { self.ability_cd <= 0.0 && self.ability_t <= 0.0 }

    /// Advance the player's in-progress autoattack, land a hit on the nearest stunnable
    /// enemy on the frame it turns active, and tick the combo window.
    fn step_combat(&mut self, dt: f32) {
        self.combo_flash = (self.combo_flash - dt * 1.5).max(0.0);
        if let Some((m, s)) = self.combo_last { let ns = s + dt; self.combo_last = if ns > 1.0 { None } else { Some((m, ns)) }; }
        if let Some((mv, t)) = self.player_move {
            let sp = crate::combat::spec(mv);
            let nt = t + dt;
            if mv.is_attack() && t < sp.windup && nt >= sp.windup {
                let power = if self.combo_flash > 0.0 { sp.power * crate::combat::COMBO_POWER_BONUS } else { sp.power };
                let (px, pz, pteam) = { let p = &self.mobs[0]; (p.x, p.z, p.team) };
                if let Some(m) = self.mobs.iter_mut().skip(1)
                    .filter(|m| m.team != pteam && !m.stunned())
                    .find(|m| dist(px, pz, m.x, m.z) < MOVE_REACH)
                {
                    if m.buffs.absorb_hit() {
                        m.stun_t = m.class.def().stun * (0.5 + power * 0.35);
                        if m.carrying { m.carrying = false; let (mx, mz) = (m.x, m.z); self.yarn_free.push((mx, mz)); }
                        self.mobs[0].stuns += 1;
                    }
                }
            }
            self.player_move = if nt >= mv.total() { self.combo_last = Some((mv, 0.0)); None } else { Some((mv, nt)) };
        }
    }

    fn spawn_powerup(&mut self) {
        let i = (self.tick.wrapping_mul(2_654_435_761) >> 8) as usize % PU_SPAWNS.len();
        let k = POWERS[(hashf(self.tick ^ 0x1234) * 3.0) as usize % 3];
        let (x, z) = PU_SPAWNS[i];
        // don't stack two power-ups on the same spot
        if !self.powerups.iter().any(|p| dist(p.x, p.z, x, z) < 2.0) {
            self.powerups.push(Pickup { x, z, kind: k });
        }
    }

    /// Throw `n` firecrackers from mob `i` in its facing direction (spread for triples).
    fn throw_from(&mut self, i: usize, n: u32) {
        let (mx, mz, heading) = { let m = &self.mobs[i]; (m.x, m.z, m.heading) };
        for k in 0..n {
            let spread = if n > 1 { (k as f32 - (n as f32 - 1.0) * 0.5) * 0.35 } else { 0.0 };
            let a = heading + spread;
            self.crackers.push(Cracker { x: mx, z: mz, vx: a.sin() * FC_SPEED, vz: a.cos() * FC_SPEED, fuse: FC_FUSE, owner: i });
        }
        self.mobs[i].cd = FC_CD;
    }

    fn explode(&mut self, cx: f32, cz: f32, owner: usize) {
        let owner_team = self.mobs[owner].team;
        let player_invuln = self.ability_t > 0.0 && self.mobs[0].class.ability().invuln();
        let mut drops: Vec<(f32, f32)> = Vec::new();
        let mut credited = 0u32;
        for (i, m) in self.mobs.iter_mut().enumerate() {
            if m.stunned() || (i == 0 && player_invuln) { continue; }
            if dist(m.x, m.z, cx, cz) < FC_RADIUS {
                if !m.buffs.absorb_hit() { continue; }       // shield soaks the blast
                m.stun_t = m.class.def().stun;
                if m.carrying { m.carrying = false; drops.push((m.x, m.z)); }
                if m.team != owner_team { credited += 1; }
            }
        }
        for d in drops { self.yarn_free.push(d); }
        self.mobs[owner].stuns += credited;
    }

    pub fn step(&mut self, dt: f32) {
        if self.outcome.is_some() { return; }
        self.time += dt;
        let input = self.input.take();

        // ── period clock ──
        self.period_t -= dt;
        if self.period_t <= 0.0 {
            if self.overtime {
                // sudden-death ran its course with no golden goal → it's a draw
                self.outcome = Some(2);
                return;
            }
            if self.period >= PERIODS {
                if self.score[0] == self.score[1] {
                    self.overtime = true;            // tied after two periods → golden goal
                    self.period_t = OVERTIME_LEN;
                } else {
                    self.outcome = Some(if self.score[0] > self.score[1] { TAWNY } else { SILVER });
                    return;
                }
            } else {
                self.period += 1; self.period_t = PERIOD_LEN;
            }
        }

        // ── power-up spawns ──
        self.pu_spawn_t -= dt;
        if self.pu_spawn_t <= 0.0 { self.spawn_powerup(); self.pu_spawn_t = PU_INTERVAL; }

        // ── the player's ultimate (class.ability()) ──
        self.ability_cd = (self.ability_cd - dt).max(0.0);
        self.ability_t = (self.ability_t - dt).max(0.0);
        if self.ability_want && self.ability_cd <= 0.0 && self.ability_t <= 0.0 && !self.mobs[0].stunned() {
            let ab = self.mobs[0].class.ability();
            self.ability_t = ab.duration();
            self.ability_cd = ab.cooldown() * ULT_CD_MULT;
        }
        self.ability_want = false;
        if self.ability_t > 0.0 && self.mobs[0].class.ability().invuln() { self.mobs[0].stun_t = 0.0; }   // untouchable — shrug off any stun this frame

        // ── decide each mob's move, then integrate ──
        let n = self.mobs.len();
        let mut throws: Vec<(usize, u32)> = Vec::new();
        for i in 0..n {
            self.mobs[i].cd = (self.mobs[i].cd - dt).max(0.0);
            self.mobs[i].stun_t = (self.mobs[i].stun_t - dt).max(0.0);
            self.mobs[i].leap_t = (self.mobs[i].leap_t - dt).max(0.0);
            self.mobs[i].buffs.tick(dt);
            if self.mobs[i].stunned() { continue; }         // stunned: no action

            let (mx, mz, want_throw, want_jump);
            if self.mobs[i].is_player {
                let (ix, iz, th, jp) = input.unwrap_or((0.0, 0.0, false, false));
                mx = ix; mz = iz; want_throw = th; want_jump = jp;
            } else {
                let (ix, iz, th) = self.ai_intent(i);
                mx = ix; mz = iz; want_throw = th; want_jump = false;
            }

            // heading + throw
            let (dx, dz) = norm(mx, mz);
            if dx.abs() + dz.abs() > 1e-4 { self.mobs[i].heading = dx.atan2(dz); }
            if want_throw && self.mobs[i].cd <= 0.0 {
                let cnt = self.mobs[i].buffs.throw_count();
                throws.push((i, cnt));
            }
            if want_jump && self.mobs[i].leap_t <= 0.0 { self.mobs[i].leap_t = LEAP_DUR; }

            // integrate movement at the terrain-and-buff-scaled speed
            let base = self.mobs[i].class.speed_at(self.mobs[i].x, self.mobs[i].z);
            let leap_mult = if self.mobs[i].leap_t > 0.0 { 1.0 + self.mobs[i].class.def().jump * 0.06 } else { 1.0 };
            let ult_mult = if i == 0 && self.ability_t > 0.0 { self.mobs[0].class.ability().speed_mult() } else { 1.0 };
            let speed = base * self.mobs[i].buffs.move_mult() * leap_mult * ult_mult;
            let m = &mut self.mobs[i];
            m.x = (m.x + dx * speed * dt).clamp(-ARENA, ARENA);
            m.z = (m.z + dz * speed * dt).clamp(-ARENA, ARENA);
        }
        for (i, cnt) in throws { self.throw_from(i, cnt); }

        // ── the player's flick autoattack: advance the move, land a stun on a nearby enemy ──
        self.step_combat(dt);

        // ── yarn pickup + capture at base ──
        for i in 0..n {
            if self.mobs[i].stunned() { continue; }
            let (mx, mz, team, carrying) = { let m = &self.mobs[i]; (m.x, m.z, m.team, m.carrying) };
            if !carrying {
                if let Some(j) = self.yarn_free.iter().position(|&(yx, yz)| dist(mx, mz, yx, yz) < PICKUP_R) {
                    self.yarn_free.swap_remove(j);
                    self.mobs[i].carrying = true;
                }
            } else {
                let base = self.base(team);
                if dist(mx, mz, base.0, base.1) < BASE_R {
                    self.mobs[i].carrying = false;
                    self.mobs[i].captures += 1;
                    self.score[team as usize] += 1;
                    self.respawn_yarn();
                    if self.overtime { self.outcome = Some(team); }   // golden goal
                }
            }
        }

        // ── power-up pickup ──
        for i in 0..n {
            if self.mobs[i].stunned() { continue; }
            let (mx, mz) = (self.mobs[i].x, self.mobs[i].z);
            if let Some(j) = self.powerups.iter().position(|p| dist(mx, mz, p.x, p.z) < PICKUP_R) {
                let k = self.powerups.swap_remove(j).kind;
                self.mobs[i].buffs.grant(k);
            }
        }

        // ── firecrackers fly + explode ──
        let mut booms: Vec<(f32, f32, usize)> = Vec::new();
        self.crackers.retain_mut(|c| {
            c.fuse -= dt;
            c.x += c.vx * dt; c.z += c.vz * dt;
            if c.fuse <= 0.0 { booms.push((c.x, c.z, c.owner)); false } else { true }
        });
        for (bx, bz, owner) in booms { self.explode(bx, bz, owner); }

        self.tick += 1;
    }

    fn respawn_yarn(&mut self) {
        // put a fresh yarn ball on the spawn point that's farthest from any loose yarn
        let mut best = YARN_SPAWNS[0];
        let mut best_d = -1.0;
        for &s in YARN_SPAWNS.iter() {
            let d = self.yarn_free.iter().map(|&(x, z)| dist(s.0, s.1, x, z)).fold(1e9f32, f32::min);
            if d > best_d { best_d = d; best = s; }
        }
        self.yarn_free.push(best);
    }

    /// Role-based AI. One defender per team guards its base and hunts enemy carriers;
    /// the rest fetch yarn. Everyone lobs a firecracker at a nearby enemy (carriers
    /// strongly preferred) and detours for a power-up that's on the way.
    fn ai_intent(&self, i: usize) -> (f32, f32, bool) {
        let m = &self.mobs[i];
        let is_defender = (i % PER_TEAM) == 1;

        // firecracker target: nearest enemy, carriers weighted much closer
        let mut tgt = (f32::MAX, 0.0f32, 0.0f32, false); // weighted_d, x, z, is_carrier
        for (j, e) in self.mobs.iter().enumerate() {
            if j == i || e.team == m.team || e.stunned() { continue; }
            let raw = dist(m.x, m.z, e.x, e.z);
            let w = if e.carrying { raw * 0.4 } else { raw };
            if w < tgt.0 { tgt = (w, e.x, e.z, e.carrying); }
        }
        let enemy_d = if tgt.0 < f32::MAX { dist(m.x, m.z, tgt.1, tgt.2) } else { f32::MAX };
        let throw_range = if tgt.3 { 20.0 } else { 12.0 };  // carriers are worth a longer lob
        if m.cd <= 0.0 && enemy_d < throw_range {
            return (tgt.1 - m.x, tgt.2 - m.z, true);          // face the target and throw
        }

        // grab a power-up that's basically on the way
        let mut pu = (f32::MAX, 0.0f32, 0.0f32);
        for p in &self.powerups { let d = dist(m.x, m.z, p.x, p.z); if d < pu.0 { pu = (d, p.x, p.z); } }
        if pu.0 < 9.0 { return (pu.1 - m.x, pu.2 - m.z, false); }

        if m.carrying {
            let b = self.base(m.team);
            return (b.0 - m.x, b.1 - m.z, false);             // run it home
        }

        if is_defender {
            if tgt.3 { return (tgt.1 - m.x, tgt.2 - m.z, false); }   // run down the enemy carrier
            let b = self.base(m.team);                                // else orbit our base, ready to intercept
            let a = self.tick as f32 * 0.02 + i as f32;
            return (b.0 + a.cos() * 14.0 - m.x, b.1 + a.sin() * 14.0 - m.z, false);
        }

        // attacker: fetch the nearest loose yarn
        let mut best = (f32::MAX, 0.0f32, 0.0f32);
        for &(yx, yz) in &self.yarn_free {
            let d = dist(m.x, m.z, yx, yz);
            if d < best.0 { best = (d, yx, yz); }
        }
        if best.0 < f32::MAX { (best.1 - m.x, best.2 - m.z, false) }
        else { (-m.x * 0.01, -m.z * 0.01, false) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(g: &mut Capture, secs: f32) { for _ in 0..(secs * 30.0) as i32 { g.step(1.0 / 30.0); } }

    #[test]
    fn a_flick_attack_stuns_a_nearby_enemy_and_drops_their_yarn() {
        let mut g = Capture::new(Class::Cat);
        let enemy = g.mobs.iter().position(|m| m.team == SILVER).unwrap();
        g.mobs[enemy].x = g.mobs[0].x + 1.0; g.mobs[enemy].z = g.mobs[0].z;
        g.mobs[enemy].carrying = true;
        let stuns0 = g.mobs[0].stuns;
        g.player_flick((0.0, 1.0), 1.0, false);   // forward flick → Pounce
        assert_eq!(g.player_move.map(|(m, _)| m), Some(crate::combat::Move::Pounce));
        for _ in 0..10 { g.set_input(0.0, 0.0, false, false); g.step(1.0 / 30.0); if g.mobs[0].stuns > stuns0 { break; } }
        assert!(g.mobs[0].stuns > stuns0, "a connecting flick should stun the nearby enemy");
        assert!(g.mobs[enemy].stunned());
        assert!(!g.mobs[enemy].carrying, "and make it drop its yarn");
        assert!(!g.yarn_free.is_empty());
    }

    #[test]
    fn flicks_chain_into_a_named_combo_in_capture() {
        let mut g = Capture::new(Class::Cat);
        g.player_flick((1.0, 0.0), 1.0, false);   // Roll
        while g.player_move.is_some() { g.set_input(0.0, 0.0, false, false); g.step(1.0 / 30.0); }
        g.player_flick((0.0, 1.0), 1.0, false);   // Pounce within the combo window
        assert_eq!(g.player_move.map(|(m, _)| m), Some(crate::combat::Move::Pounce));
        assert!(g.combo_flash > 0.0, "Roll into Pounce should flash the Ambush Pounce combo");
    }

    #[test]
    fn the_ultimate_engages_and_speeds_the_player() {
        let mut g = Capture::new(Class::Squirrel);   // Scamper — a speed burst
        g.set_ability(true);
        g.set_input(0.0, 1.0, false, false);
        let z0 = g.mobs[0].z;
        g.step(1.0 / 30.0);
        assert!(g.ability_t > 0.0 && g.ability_cd > 0.0, "the ultimate engages and goes on cooldown");
        let with = g.mobs[0].z - z0;
        let mut h = Capture::new(Class::Squirrel);
        h.set_input(0.0, 1.0, false, false);
        let z1 = h.mobs[0].z;
        h.step(1.0 / 30.0);
        let without = h.mobs[0].z - z1;
        assert!(with > without + 1e-3, "Scamper should cover more ground than normal movement");
    }

    #[test]
    fn an_invulnerable_ultimate_shrugs_off_a_stun() {
        let mut g = Capture::new(Class::Otter);   // Dive — invuln
        g.set_ability(true);
        g.set_input(0.0, 0.0, false, false);
        g.step(1.0 / 30.0);
        assert!(g.ability_t > 0.0);
        g.mobs[0].stun_t = 1.0;   // try to stun the player while the ultimate is active
        g.set_input(0.0, 0.0, false, false);
        g.step(1.0 / 30.0);
        assert!(!g.mobs[0].stunned(), "Dive's invulnerability should shrug off a stun");
    }

    #[test]
    fn bases_are_placed_on_dry_land() {
        let g = Capture::new(Class::Cat);
        for team in [TAWNY, SILVER] {
            let (bx, bz) = g.base(team);
            assert!(world::height_at(bx, bz) >= world::WATER_Y, "team {team}'s base must be on dry land, not underwater");
        }
    }

    #[test]
    fn ai_throws_at_a_nearby_enemy_carrier() {
        let mut g = Capture::new(Class::Cat);
        let ai = 1usize;                                   // a Tawny AI (defender)
        g.mobs[ai].x = 0.0; g.mobs[ai].z = 0.0; g.mobs[ai].cd = 0.0;
        for e in 3..6 { g.mobs[e].x = 300.0; g.mobs[e].z = 300.0; g.mobs[e].carrying = false; }
        g.mobs[3].x = 5.0; g.mobs[3].z = 0.0; g.mobs[3].carrying = true;   // enemy carrier alongside
        let (dx, dz, throw) = g.ai_intent(ai);
        assert!(throw, "AI should throw at a nearby enemy carrier");
        assert!(dx > 0.0 && dz.abs() < 0.5, "and aim at it (+x)");
    }

    #[test]
    fn ai_detours_to_a_close_powerup() {
        let mut g = Capture::new(Class::Cat);
        let ai = 2usize;                                   // a Tawny attacker
        g.mobs[ai].x = 0.0; g.mobs[ai].z = 0.0; g.mobs[ai].cd = 5.0;
        for e in 3..6 { g.mobs[e].x = 300.0; g.mobs[e].z = 300.0; }
        g.powerups.clear(); g.powerups.push(Pickup { x: 4.0, z: 0.0, kind: Power::Speed });
        g.yarn_free.clear(); g.yarn_free.push((60.0, 0.0));                // yarn far away
        let (dx, dz, throw) = g.ai_intent(ai);
        assert!(!throw);
        assert!(dx > 0.0 && dz.abs() < 0.5, "AI should grab the near power-up, not the far yarn");
    }

    #[test]
    fn defender_runs_down_an_enemy_carrier() {
        let mut g = Capture::new(Class::Cat);
        let d = 1usize;                                    // defender
        g.mobs[d].x = 0.0; g.mobs[d].z = 0.0; g.mobs[d].cd = 5.0;
        for e in 3..6 { g.mobs[e].x = 300.0; g.mobs[e].z = 300.0; g.mobs[e].carrying = false; }
        g.mobs[3].x = 40.0; g.mobs[3].z = 0.0; g.mobs[3].carrying = true;  // carrier too far to throw at
        g.powerups.clear();
        let (dx, _dz, throw) = g.ai_intent(d);
        assert!(!throw, "40 units is out of firecracker range");
        assert!(dx > 0.0, "the defender should chase the enemy carrier");
    }

    #[test]
    fn carrying_to_your_base_scores() {
        let mut g = Capture::new(Class::Cat);
        // put the player right on its base, carrying yarn
        g.mobs[0].carrying = true;
        let (bx, bz) = g.base(TAWNY); g.mobs[0].x = bx; g.mobs[0].z = bz;
        let before = g.score[TAWNY as usize];
        g.set_input(0.0, 0.0, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.score[TAWNY as usize], before + 1, "delivering yarn to your base scores");
        assert!(!g.mobs[0].carrying, "the yarn is deposited");
    }

    #[test]
    fn firecracker_stuns_and_drops_yarn() {
        let mut g = Capture::new(Class::Cat);
        // an enemy (Silver) carrying yarn, sitting still
        let e = 3usize; // first Silver mob
        assert_eq!(g.mobs[e].team, SILVER);
        g.mobs[e].carrying = true;
        g.mobs[e].x = 0.0; g.mobs[e].z = 0.0;
        let loose_before = g.yarn_free.len();
        // a firecracker explodes on the enemy
        g.crackers.push(Cracker { x: 0.0, z: 0.0, vx: 0.0, vz: 0.0, fuse: 0.02, owner: 0 });
        g.step(1.0 / 30.0);
        assert!(g.mobs[e].stunned(), "the blast stuns the enemy");
        assert!(!g.mobs[e].carrying, "a stunned carrier drops its yarn");
        assert_eq!(g.yarn_free.len(), loose_before + 1, "the dropped yarn becomes loose again");
        assert_eq!(g.mobs[0].stuns, 1, "the thrower is credited with the stun");
    }

    #[test]
    fn shield_absorbs_the_blast() {
        let mut g = Capture::new(Class::Cat);
        let e = 3usize;
        g.mobs[e].x = 0.0; g.mobs[e].z = 0.0;
        g.mobs[e].buffs.grant(Power::Shield);
        g.crackers.push(Cracker { x: 0.0, z: 0.0, vx: 0.0, vz: 0.0, fuse: 0.02, owner: 0 });
        g.step(1.0 / 30.0);
        assert!(!g.mobs[e].stunned(), "the shield soaks the stun");
        assert!(!g.mobs[e].buffs.shield, "and the shield is spent");
    }

    #[test]
    fn a_stunned_mob_cannot_move() {
        let mut g = Capture::new(Class::Cat);
        g.mobs[0].stun_t = 1.0;
        g.mobs[0].x = 0.0; g.mobs[0].z = 0.0;
        g.set_input(1.0, 0.0, false, false);
        g.step(1.0 / 30.0);
        assert!(g.mobs[0].x.abs() < 1e-4, "a stunned mob holds still");
    }

    #[test]
    fn speed_boost_makes_the_player_faster() {
        let run = |boost: bool| {
            let mut g = Capture::new(Class::Cat);
            g.mobs[0].x = 0.0; g.mobs[0].z = 40.0;      // some land spot
            if boost { g.mobs[0].buffs.grant(Power::Speed); }
            let x0 = g.mobs[0].x;
            g.set_input(1.0, 0.0, false, false);
            g.step(1.0 / 30.0);
            (g.mobs[0].x - x0).abs()
        };
        assert!(run(true) > run(false) + 1e-3, "the speed boost should cover more ground");
    }

    #[test]
    fn a_match_resolves_to_a_winner_or_draw() {
        let mut g = Capture::new(Class::Squirrel);
        settle(&mut g, PERIOD_LEN * PERIODS as f32 + 2.0);
        assert!(g.outcome.is_some(), "two periods in, the match resolves");
        let o = g.outcome.unwrap();
        assert!(o == TAWNY || o == SILVER || o == 2);
        // and it never panics with the ecology-free arena running full length
    }

    #[test]
    fn a_tie_goes_to_sudden_death_golden_goal() {
        let mut g = Capture::new(Class::Cat);
        // force a tie at the end of the final period
        g.score = [3, 3];
        g.period = PERIODS;
        g.period_t = 0.001;
        g.set_input(0.0, 0.0, false, false);
        g.step(1.0 / 30.0);                       // the clock runs out tied → sudden death
        assert!(g.overtime, "a tie after the last period should enter overtime");
        assert!(g.outcome.is_none(), "overtime isn't decided yet");
        // the player delivers a yarn → golden goal
        g.mobs[0].carrying = true;
        let (bx, bz) = g.base(TAWNY);
        g.mobs[0].x = bx; g.mobs[0].z = bz;
        g.set_input(0.0, 0.0, false, false);
        g.step(1.0 / 30.0);
        assert_eq!(g.outcome, Some(TAWNY), "a capture in overtime wins it outright");
    }

    #[test]
    fn capture_invariants_hold_for_every_class() {
        // Full matches for all six classes with a live player throwing/jumping. Assert
        // the core invariants each tick: yarn is conserved (loose + carried == the six
        // balls), scores only climb, mobs stay finite and inside the arena, stun timers
        // never go negative — and it always resolves without panicking.
        use crate::class::CLASSES;
        for cls in CLASSES {
            let mut g = Capture::new(cls);
            let mut prev = [0u32, 0u32];
            let mut t = 0i32;
            while g.outcome.is_none() && t < 170 * 30 {
                g.set_input((t as f32 * 0.10).sin(), (t as f32 * 0.07).cos(), t % 20 == 0, t % 15 == 0);
                g.step(1.0 / 30.0);
                let carried = g.mobs.iter().filter(|m| m.carrying).count();
                assert_eq!(g.yarn_free.len() + carried, YARN_SPAWNS.len(), "yarn was leaked or duplicated ({cls:?})");
                assert!(g.score[0] >= prev[0] && g.score[1] >= prev[1], "a score went backwards");
                prev = g.score;
                for m in &g.mobs {
                    assert!(m.x.is_finite() && m.z.is_finite(), "mob position not finite");
                    assert!(m.x.abs() <= ARENA + 1.0 && m.z.abs() <= ARENA + 1.0, "mob left the arena");
                    assert!(m.stun_t >= 0.0 && m.cd >= 0.0, "a timer went negative");
                }
                t += 1;
            }
            assert!(g.outcome.is_some(), "the match should resolve for {cls:?}");
        }
    }
}
