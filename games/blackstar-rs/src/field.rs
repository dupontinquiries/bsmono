//! THE FIELD — the wave, standing where you can shoot it.
//!
//! `campaign::MissionRun` fights a wave as a 1-D skirmish: fighters on a
//! range line, volleys, an objective machine deciding what it was for.
//! `enemy::Enemy` is a ported, tested idle/chase/attack agent with zones
//! and standoffs — and nothing ever spawned one. The sortie's Engage
//! stage therefore had hostiles you could not see, avoid or aim at:
//! `fire_weapon` landed every trigger pull on "the hostile leading the
//! line" whether you were pointing at the sky or not. That is the kill
//! bridge both `sortie.rs` and `raidflow.rs` name as their debt.
//!
//! This module puts ONE agent in the world per living fighter of the
//! current wave, slot-indexed against `Fight.b.fighters`, and keeps the
//! two in step:
//!
//! * **The fight stays the authority on HP.** A world hit is routed to
//!   the fighter in that slot (`Fight::external_on_b_slot`) through the
//!   same shield/evasion/ring gates as everyone else's fire, and the
//!   fighter's HP is mirrored back onto the agent. The campaign's
//!   "every mission is winnable by the default squad" test is untouched
//!   because the skirmish still runs underneath; what changes is that
//!   the player's contribution now has to be AIMED.
//! * **The field owns geometry.** Where a foe stands, whether it can see
//!   you, when it fires and in what direction — and, new here, WHAT FORM
//!   IT IS IN. Same seam as `raidrun`: this layer is handed the player
//!   and a clock and hands back EVENTS; it never applies damage to the
//!   player or moves a bot in the renderer. That is what makes a wave
//!   testable at 120 Hz on a build box.
//!
//! ## Vehicle mode is the closing move
//!
//! The brief for this layer was that vehicle-mode play has to matter in a
//! fight, not just in the open. So a hostile that is far from you FOLDS
//! to its vehicle, drives — fast, silent, unable to shoot — to a FLANK
//! point beside you rather than straight at you (the wave splits around
//! the player instead of forming a queue), unfolds on arriving there
//! and fights as a robot. Passing through you in vehicle form is a RAM,
//! which is the one attack a vehicle has. The player gets the same
//! move: driving through a foe at speed is reported as a hit on it.
//! Spark-hunters (the Corrupted and the Apex) do not fold — the fiction
//! gives them no clean vehicle mode (`corruption.rs`'s continuous
//! transformation) and they were already fast on foot.
//!
//! ## Guesses, labelled
//!
//! Every distance and multiplier under `tunables` is a GUESS with a
//! reason beside it. None of this has been seen on a screen; the
//! numbers are shaped by the sandbox's own scale (the RL car at 1/22)
//! and want a controller in someone's hand before anyone trusts them.
//! Damage per shot is NOT a guess: it is the fighter's own dps times the
//! agent's cooldown, so a hostile's world fire is worth exactly what its
//! line volley was.

use glam::Vec3;

use crate::content::{Role, Troop};
use crate::enemy::{Enemy, State};
use crate::escalation::EnemyKind;
use crate::patrol::Faction;
use crate::raidgen::strhash;
use crate::skirmish::Fighter;
use crate::weapons::{Element, Verb};

// -------------------------------------------------------------- tunables ---

/// A foe further than this folds down and drives. GUESS: about the grunt's
/// attack range, so a robot that could already shoot you stays a robot.
pub const FOLD_OUT_M: f32 = 40.0;
/// A driving foe unfolds when it is within this of its FLANK POINT, which
/// sits `FLANK_M` beside the player — so it stands up about that far from
/// you, inside every ranged verb's reach. GUESS. Larger than `RAM_R` on
/// purpose: a lane that ends on the player is a ram, not a parking spot.
pub const ARRIVE_M: f32 = 3.0;
/// Seconds a fold takes, and the foe is still for it — the same window the
/// player's own `xform` pays, and the moment to shoot a folding target.
pub const FOLD_S: f32 = 0.9;
/// Vehicle speed over the agent's robot `move_speed`. GUESS: the drop's
/// "vehicle is a dart" ratio, so the two modes feel like the same trade.
pub const VEH_MUL: f32 = 3.2;
/// How far beside the player a vehicle aims its approach. GUESS.
pub const FLANK_M: f32 = 14.0;
/// A vehicle within this of the player has rammed it.
pub const RAM_R: f32 = 2.0;
/// Seconds between rams from one foe — one pass, one hit.
pub const RAM_CD_S: f32 = 0.6;
/// A ram lands the vehicle's speed times this as damage. GUESS sized so a
/// full-speed pass is worth about three of its shots.
pub const RAM_DMG_PER_MS: f32 = 1.4;
/// The player's ram needs at least this much speed to count, m/s. GUESS:
/// a rolling nudge is not an attack.
pub const PLAYER_RAM_MIN_MS: f32 = 12.0;
/// How far past the LZ, along the drop bearing, the wave musters. GUESS:
/// far enough that you land and see them ahead, close enough to drive to.
pub const SITE_AHEAD_M: f32 = 70.0;
/// Line-of-sight is re-asked this often per foe, staggered by slot, so a
/// full wave costs a few SDF marches a second rather than a few hundred.
pub const LOS_EVERY_S: f32 = 0.25;
/// Base aim spread, radians, at `SPREAD_REF_M`; scales with distance so a
/// hostile misses at range and lands up close, which is the Destiny feel
/// the zones exist for. GUESS.
pub const SPREAD_RAD: f32 = 0.05;
pub const SPREAD_REF_M: f32 = 30.0;
/// Chest height above the ground contact, in the sandbox's car units.
pub const CHEST_H: f32 = 0.8;

/// Standoff per zone, metres: front pushes close, middle holds mid, back
/// anchors. GUESS shaped by `enemy.rs`'s zone comments.
const STANDOFF: [f32; 3] = [6.0, 18.0, 34.0];
/// Muster depth per zone, metres behind the anchor toward the objective.
const DEPTH: [(f32, f32); 3] = [(0.0, 8.0), (12.0, 22.0), (28.0, 40.0)];
/// Lateral muster spread, metres.
const LATERAL: f32 = 24.0;

// ------------------------------------------------------------------ types ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Form {
    Robot,
    Vehicle,
}

/// One hostile, standing in the world.
#[derive(Clone)]
pub struct Foe {
    /// index into the fight's side-B roster
    pub slot: usize,
    pub troop: &'static Troop,
    pub agent: Enemy,
    pub form: Form,
    /// seconds left in the current fold; the foe is still while it runs
    pub fold_t: f32,
    /// what it is folding INTO, valid while `fold_t > 0`
    pub fold_to: Form,
    /// unit XZ facing, for the shell's rotation and the muzzle
    pub facing: Vec3,
    /// this tick's velocity, for the shell's shot marks and wheel spin
    pub vel: Vec3,
    /// which side of the player this one flanks on, ±1
    pub flank: f32,
    /// the flank point this drive is aimed at, and where the player was
    /// when it was chosen. LOCKED rather than re-derived each tick: a
    /// point defined as "beside the player from where I am now" rotates
    /// as I move, and chasing it is a spiral that never arrives and
    /// clips the player on the way. Re-chosen only when the player has
    /// moved most of a flank width from where the lock was taken.
    goal: Option<(Vec3, Vec3)>,
    /// the fighter's line dps, taken at muster: one shot is worth this
    /// times the agent's cooldown
    pub dps: f32,
    /// line-of-sight cache and its refresh clock
    los: bool,
    los_t: f32,
    ram_cd: f32,
    shots: u32,
    /// the fighter's last mirrored HP, so a death seen through the fight
    /// (a squad-ghost volley) is reported once
    seen_alive: bool,
}

impl Foe {
    pub fn alive(&self) -> bool {
        self.agent.state != State::Dead
    }
    /// 0..1 through the fold, 0 = fully the form it is leaving.
    pub fn fold_frac(&self) -> f32 {
        if self.fold_t <= 0.0 {
            return 1.0;
        }
        1.0 - (self.fold_t / FOLD_S).clamp(0.0, 1.0)
    }
    pub fn pos(&self) -> Vec3 {
        self.agent.position
    }
    /// Is it in vehicle form right now, for the renderer's pose pick.
    pub fn is_vehicle(&self) -> bool {
        match (self.form, self.fold_t > 0.0) {
            (Form::Vehicle, false) => true,
            (_, true) => self.fold_to == Form::Vehicle && self.fold_frac() > 0.5,
            _ => false,
        }
    }
}

/// What the field did this tick that the shell has to act on. Nothing
/// here has been applied: damage to the player, shots into a pool, a
/// bot's pose — all the shell's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// a hostile fired: spawn this into the hostile shot pool
    Fire { slot: usize, from: Vec3, dir: Vec3, verb: Verb, element: Element, dmg: f32 },
    /// a melee foe connected with the player
    Melee { slot: usize, dmg: f32 },
    /// a vehicle-form foe drove through the player
    Ram { slot: usize, dv: Vec3, dmg: f32 },
    /// the player, in vehicle form, drove through a foe: land `dmg` on
    /// that slot through the fight, shove the foe by `dv`
    Rammed { slot: usize, dv: Vec3, dmg: f32 },
    /// a fold completed
    Folded { slot: usize, form: Form },
    /// the foe in this slot is dead (by any hand) — fx and drop the bot
    Down { slot: usize },
}

/// The wave, in the world.
pub struct Field {
    pub foes: Vec<Foe>,
    /// where the wave musters around
    pub anchor: Vec3,
    /// unit XZ vector from the anchor toward where the player arrives
    pub facing: Vec3,
    seed: u64,
    ride_h: f32,
    pending: Vec<Event>,
}

// ---------------------------------------------------------------- muster ---

/// Which of `enemy.rs`'s four minds a troop gets. The Corrupted are the
/// spark-hunters; a Corrupted champion is the Apex; the cheap Hijack
/// scout body is the swarm unit; everything else is a ranged trooper.
pub fn kind_for(t: &Troop, champion: bool) -> EnemyKind {
    match t.faction {
        Faction::Corrupted if champion => EnemyKind::Apex,
        Faction::Corrupted => EnemyKind::Corrupted,
        _ if t.id.contains("swarmer") => EnemyKind::Hijack,
        _ => EnemyKind::Grunt,
    }
}

/// Engagement range from how the troop shoots, so a flak body closes and
/// a beam body holds. Melee verbs keep the agent kind's own reach.
fn attack_range_for(verb: Verb, kind: EnemyKind) -> Option<f32> {
    use crate::ballistics::Delivery as D;
    if matches!(kind, EnemyKind::Corrupted | EnemyKind::Apex) {
        return None;
    }
    Some(match verb.delivery() {
        D::Melee => 2.4,
        D::Blast => 45.0,
        D::Kinetic => 70.0,
        D::Beam => 90.0,
    })
}

/// Can this troop fold at all. A Variant with `Element::None` is either
/// the Corrupted apex or a Hijack elite carrying its signal — neither is
/// drawn with a vehicle in the treatment.
fn can_fold(t: &Troop, kind: EnemyKind) -> bool {
    !matches!(kind, EnemyKind::Corrupted | EnemyKind::Apex)
        && !(t.role == Role::Variant && t.element == Element::None)
}

fn unit_xz(v: Vec3) -> Vec3 {
    let f = Vec3::new(v.x, 0.0, v.z);
    if f.length_squared() < 1e-6 {
        Vec3::Z
    } else {
        f.normalize()
    }
}

fn hash_unit(h: u64, lane: u32) -> f32 {
    let x = h.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add((lane as u64).wrapping_mul(0x632BE59BD9B4E019));
    let x = (x ^ (x >> 29)).wrapping_mul(0xBF58476D1CE4E5B9);
    ((x >> 40) & 0xFFFFFF) as f32 / 16_777_215.0
}

impl Field {
    /// An empty field around an anchor. `facing` points from the anchor
    /// toward where the player will come from (the LZ); `ride_h` is the
    /// shell's ground clearance for a standing body.
    pub fn new(anchor: Vec3, facing: Vec3, seed: u64, ride_h: f32) -> Field {
        Field { foes: Vec::new(), anchor, facing: unit_xz(facing), seed, ride_h, pending: Vec::new() }
    }

    /// The conventional anchor for a sortie: `SITE_AHEAD_M` past the LZ
    /// along the drop bearing, facing back at it.
    pub fn for_sortie(lz: Vec3, lz_dir: Vec3, mission_id: &str, ride_h: f32) -> Field {
        let dir = unit_xz(lz_dir);
        Field::new(lz + dir * SITE_AHEAD_M, -dir, strhash(mission_id), ride_h)
    }

    pub fn alive(&self) -> usize {
        self.foes.iter().filter(|f| f.alive()).count()
    }

    pub fn foe(&self, slot: usize) -> Option<&Foe> {
        self.foes.iter().find(|f| f.slot == slot)
    }

    /// Stand a wave up. One foe per living fighter, placed by zone in an
    /// arc facing the LZ, deterministic per (seed, wave, slot). The
    /// champion, if it is in this wave, anchors the back.
    pub fn muster(&mut self, wave: usize, fighters: &[Fighter], champion: Option<&str>) {
        self.foes.clear();
        let right = self.facing.cross(Vec3::Y).normalize_or(Vec3::X);
        for (slot, f) in fighters.iter().enumerate() {
            if !f.alive() {
                continue;
            }
            let t = f.troop;
            let is_champ = champion == Some(t.id);
            let kind = kind_for(t, is_champ);
            let h = self.seed ^ strhash(t.id) ^ ((wave as u64) << 32) ^ (slot as u64 * 0x1234_5678_9ABC_DEF1);
            // zone: the champion anchors the back; otherwise cycle
            let zone = if is_champ { 2 } else { slot % 3 };
            let (d0, d1) = DEPTH[zone];
            let depth = d0 + (d1 - d0) * hash_unit(h, 1);
            let lat = (hash_unit(h, 2) * 2.0 - 1.0) * LATERAL;
            let pos = self.anchor - self.facing * depth + right * lat;
            let mut agent = Enemy::new_kind(slot as i32, pos, f.hp, kind, t.element);
            // the fight's HP is the truth, whatever the kind's default said
            agent.hp = f.hp;
            agent.max_hp = f.hp_max.max(f.hp).max(1.0);
            if let Some(r) = attack_range_for(t.fire, kind) {
                agent.attack_range = r;
            }
            agent.zone = zone as i32;
            // a body that fights at arm's length cannot hold a standoff
            // wider than its arm — `Enemy::update` stops advancing at the
            // hold, so a melee lunger with one would never connect
            agent.standoff = if agent.attack_range < STANDOFF[zone] { 0.0 } else { STANDOFF[zone] };
            agent.is_commander = is_champ;
            agent.position.y = self.anchor.y + self.ride_h;
            self.foes.push(Foe {
                slot,
                troop: t,
                agent,
                form: Form::Robot,
                fold_t: 0.0,
                fold_to: Form::Robot,
                facing: self.facing,
                vel: Vec3::ZERO,
                flank: if slot % 2 == 0 { 1.0 } else { -1.0 },
                dps: f.dps,
                goal: None,
                los: false,
                // stagger the first LOS ask so a wave does not march in
                // lockstep on frame one
                los_t: LOS_EVERY_S * (slot as f32 % 4.0) / 4.0,
                ram_cd: 0.0,
                shots: 0,
                seen_alive: true,
            });
        }
    }

    /// The wave is over: nobody left to stand.
    pub fn clear(&mut self) {
        for f in &self.foes {
            if f.alive() {
                self.pending.push(Event::Down { slot: f.slot });
            }
        }
        self.foes.clear();
    }

    /// Mirror the fight's HP onto the agents. A fighter the fight killed
    /// (a squad-ghost volley, a burn tick) dies here too, once.
    pub fn sync(&mut self, fighters: &[Fighter]) {
        for foe in self.foes.iter_mut() {
            let Some(f) = fighters.get(foe.slot) else {
                if foe.alive() {
                    foe.agent.state = State::Dead;
                    foe.seen_alive = false;
                    self.pending.push(Event::Down { slot: foe.slot });
                }
                continue;
            };
            foe.agent.hp = f.hp.max(0.0);
            if !f.alive() && foe.seen_alive {
                foe.agent.state = State::Dead;
                foe.seen_alive = false;
                self.pending.push(Event::Down { slot: foe.slot });
            }
        }
    }

    /// The player's fire landed on a slot. The FIGHT decides the damage;
    /// this only wakes the agent, so a foe shot from cover turns to face
    /// the shooter the way `Enemy::damage` intends.
    pub fn poke(&mut self, slot: usize) {
        if let Some(f) = self.foes.iter_mut().find(|f| f.slot == slot) {
            if f.alive() && f.agent.state == State::Idle {
                f.agent.state = State::Chase;
            }
        }
    }

    /// One fixed step. `player_vehicle` and `player_vel` feed the ram
    /// checks; `ground` is the height field; `los` the shell's own
    /// line-of-sight answer, asked at most every `LOS_EVERY_S` per foe.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        dt: f32,
        player_pos: Vec3,
        player_vel: Vec3,
        player_vehicle: bool,
        player_cloaked: bool,
        ground: &dyn Fn(f32, f32) -> f32,
        los: &dyn Fn(Vec3, Vec3) -> bool,
    ) -> Vec<Event> {
        let mut ev: Vec<Event> = std::mem::take(&mut self.pending);
        let ride_h = self.ride_h;
        for foe in self.foes.iter_mut() {
            if !foe.alive() {
                foe.vel = Vec3::ZERO;
                continue;
            }
            let before = foe.agent.position;
            foe.ram_cd = (foe.ram_cd - dt).max(0.0);

            // line of sight, throttled
            foe.los_t -= dt;
            if foe.los_t <= 0.0 {
                foe.los_t = LOS_EVERY_S;
                let eye = foe.agent.position + Vec3::Y * CHEST_H;
                foe.los = los(eye, player_pos + Vec3::Y * CHEST_H);
            }

            let to = player_pos - foe.agent.position;
            let dist = to.length();
            let dir_xz = unit_xz(to);

            // a fold in progress: still, then done
            if foe.fold_t > 0.0 {
                foe.fold_t -= dt;
                if foe.fold_t <= 0.0 {
                    foe.fold_t = 0.0;
                    foe.form = foe.fold_to;
                    ev.push(Event::Folded { slot: foe.slot, form: foe.form });
                }
                foe.vel = Vec3::ZERO;
                continue;
            }

            let folds = can_fold(foe.troop, foe.agent.kind);
            match foe.form {
                Form::Robot => {
                    // far, awake, and able to: fold down and drive
                    // — but a back-zone body whose job is to hold at
                    // range walks its last stretch rather than folding
                    // inside its own standoff
                    let fold_out = FOLD_OUT_M.max(foe.agent.standoff + 12.0);
                    if folds && dist > fold_out && foe.agent.state == State::Chase && foe.agent.scatter_t <= 0.0 {
                        foe.fold_t = FOLD_S;
                        foe.fold_to = Form::Vehicle;
                        foe.vel = Vec3::ZERO;
                        continue;
                    }
                    let los = foe.los;
                    foe.agent.update(los, player_cloaked, player_pos, dt);
                    if foe.agent.wants_to_fire {
                        foe.shots += 1;
                        let from = foe.agent.position + Vec3::Y * CHEST_H;
                        let aim = (player_pos + Vec3::Y * CHEST_H - from).normalize_or(dir_xz);
                        let dmg = fighter_dmg_per_shot(foe);
                        if foe.troop.fire.delivery() == crate::ballistics::Delivery::Melee
                            || matches!(foe.agent.kind, EnemyKind::Corrupted | EnemyKind::Apex)
                        {
                            ev.push(Event::Melee { slot: foe.slot, dmg: foe.agent.melee_damage().max(dmg) });
                        } else {
                            // deterministic scatter, wider with range
                            let h = strhash(foe.troop.id) ^ (foe.shots as u64) ^ self.seed;
                            let spread = SPREAD_RAD * (dist / SPREAD_REF_M).clamp(0.4, 2.0);
                            let side = aim.cross(Vec3::Y).normalize_or(Vec3::X);
                            let upv = side.cross(aim).normalize_or(Vec3::Y);
                            let jx = hash_unit(h, 3) * 2.0 - 1.0;
                            let jy = hash_unit(h, 4) * 2.0 - 1.0;
                            let dir = (aim + (side * jx + upv * jy) * spread).normalize_or(aim);
                            ev.push(Event::Fire {
                                slot: foe.slot,
                                from,
                                dir,
                                verb: foe.troop.fire,
                                element: foe.troop.element,
                                dmg,
                            });
                        }
                    }
                    if dist > 1e-3 {
                        foe.facing = dir_xz;
                    }
                }
                Form::Vehicle => {
                    // drive at a flank point beside the player, not at
                    // them: the wave splits around you instead of queueing
                    let stale = match foe.goal {
                        Some((_, at)) => (player_pos - at).length() > FLANK_M * 0.75,
                        None => true,
                    };
                    if stale {
                        let right = dir_xz.cross(Vec3::Y).normalize_or(Vec3::X);
                        foe.goal = Some((player_pos + right * (foe.flank * FLANK_M), player_pos));
                    }
                    let goal = foe.goal.map(|g| g.0).unwrap_or(player_pos);
                    let to_goal = goal - foe.agent.position;
                    let gd = unit_xz(to_goal);
                    let speed = foe.agent.move_speed * VEH_MUL;
                    foe.agent.position += gd * speed * dt;
                    foe.facing = gd;
                    // the ram: through the player at speed. Checked BEFORE
                    // the unfold so a player standing in the lane is hit
                    // rather than politely stopped for.
                    let d_now = (player_pos - foe.agent.position).length();
                    if d_now <= RAM_R && foe.ram_cd <= 0.0 {
                        foe.ram_cd = RAM_CD_S;
                        let dv = gd * speed * 0.6 + Vec3::Y * (speed * 0.15);
                        ev.push(Event::Ram { slot: foe.slot, dv, dmg: speed * RAM_DMG_PER_MS });
                    }
                    // arrived at the flank point: stop, unfold, fight
                    if Vec3::new(to_goal.x, 0.0, to_goal.z).length() <= ARRIVE_M {
                        foe.goal = None;
                        foe.fold_t = FOLD_S;
                        foe.fold_to = Form::Robot;
                        foe.vel = Vec3::ZERO;
                        continue;
                    }
                }
            }

            // the player's ram: vehicle form, real speed, contact
            let d_after = (player_pos - foe.agent.position).length();
            if player_vehicle && d_after <= RAM_R && foe.ram_cd <= 0.0 {
                let v = player_vel.length();
                if v >= PLAYER_RAM_MIN_MS {
                    foe.ram_cd = RAM_CD_S;
                    let dv = unit_xz(player_vel) * v * 0.5 + Vec3::Y * (v * 0.12);
                    // the body is knocked along the pass; the shell's bot
                    // follows the agent, so the shove has to land here
                    foe.agent.position += unit_xz(dv) * (v * 0.08);
                    ev.push(Event::Rammed { slot: foe.slot, dv, dmg: v * RAM_DMG_PER_MS });
                }
            }

            // ground snap and this tick's velocity
            let p = &mut foe.agent.position;
            p.y = ground(p.x, p.z) + ride_h;
            foe.vel = if dt > 0.0 { (*p - before) / dt } else { Vec3::ZERO };
        }
        ev
    }
}

/// What one shot from this foe is worth: its line dps over its own
/// cooldown, so the world fire and the 1-D volley agree on the number.
fn fighter_dmg_per_shot(foe: &Foe) -> f32 {
    foe.dps * foe.agent.fire_cooldown.max(0.1)
}

// ------------------------------------------------------------------ tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skirmish::{Squad, OPEN_AT};

    fn flat(_x: f32, _z: f32) -> f32 {
        0.0
    }
    fn open(_a: Vec3, _b: Vec3) -> bool {
        true
    }
    fn wall(_a: Vec3, _b: Vec3) -> bool {
        false
    }

    fn wave(ids: &[&str]) -> Squad {
        Squad::new(ids, "aggressor", OPEN_AT)
    }

    fn field() -> Field {
        Field::for_sortie(Vec3::ZERO, Vec3::Z, "the_beach_opens", 0.19)
    }

    #[test]
    fn muster_stands_one_foe_per_living_fighter_at_the_site() {
        let s = wave(&["grunt", "grunt", "ravager", "slipstream"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        assert_eq!(f.foes.len(), 4);
        for foe in &f.foes {
            let d = (foe.pos() - f.anchor).length();
            assert!(d < 60.0, "foe {} mustered {d:.1} m from the anchor", foe.slot);
            assert!(foe.pos().z > SITE_AHEAD_M * 0.5, "the wave is not past the LZ: z={}", foe.pos().z);
            assert!(foe.alive());
            assert!((foe.agent.hp - s.fighters[foe.slot].hp).abs() < 1e-3, "agent hp is not the fighter's");
        }
        // deterministic: the same wave musters the same way
        let mut g = field();
        g.muster(1, &s.fighters, None);
        for (a, b) in f.foes.iter().zip(g.foes.iter()) {
            assert_eq!(a.pos(), b.pos());
        }
        // and a different wave differently
        let mut h = field();
        h.muster(2, &s.fighters, None);
        assert!(f.foes.iter().zip(h.foes.iter()).any(|(a, b)| a.pos() != b.pos()));
    }

    #[test]
    fn kinds_follow_the_fiction() {
        let t = |id: &str| crate::content::troop(id).unwrap();
        assert_eq!(kind_for(t("corrupted_drifter"), false), EnemyKind::Corrupted);
        assert_eq!(kind_for(t("corrupted_apex"), true), EnemyKind::Apex);
        assert_eq!(kind_for(t("hijack_swarmer"), false), EnemyKind::Hijack);
        assert_eq!(kind_for(t("grunt"), false), EnemyKind::Grunt);
        assert!(can_fold(t("grunt"), EnemyKind::Grunt));
        assert!(!can_fold(t("corrupted_drifter"), EnemyKind::Corrupted));
        assert!(!can_fold(t("hijack_harvester"), EnemyKind::Grunt), "an elite carrying the signal has no vehicle");
    }

    #[test]
    fn a_far_grunt_folds_drives_flanks_and_unfolds_to_fight() {
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let player = Vec3::new(0.0, 0.19, -60.0); // 130+ m away, well past FOLD_OUT
        let mut folded_down = false;
        let mut drove = false;
        let mut folded_up = false;
        let mut fired = false;
        let mut max_lateral = 0.0f32;
        for _ in 0..(120 * 40) {
            for e in f.step(1.0 / 120.0, player, Vec3::ZERO, false, false, &flat, &open) {
                match e {
                    Event::Folded { form: Form::Vehicle, .. } => folded_down = true,
                    Event::Folded { form: Form::Robot, .. } => folded_up = true,
                    Event::Fire { .. } => fired = true,
                    _ => {}
                }
            }
            let foe = &f.foes[0];
            if foe.form == Form::Vehicle && foe.fold_t <= 0.0 {
                drove = true;
                max_lateral = max_lateral.max((foe.pos().x - player.x).abs());
            }
            if fired {
                break;
            }
        }
        assert!(folded_down, "never folded to vehicle at 130 m");
        assert!(drove, "never drove");
        assert!(max_lateral > FLANK_M * 0.5, "drove straight at the player, no flank: {max_lateral:.1}");
        assert!(folded_up, "never unfolded to fight");
        assert!(fired, "never fired after unfolding");
        let d = (f.foes[0].pos() - player).length();
        assert!(d <= 90.0, "fired from {d:.1} m, past its own attack range");
    }

    #[test]
    fn the_corrupted_do_not_fold_they_run_you_down() {
        let s = wave(&["corrupted_drifter"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let player = Vec3::new(0.0, 0.19, -60.0);
        let mut melee = false;
        for _ in 0..(120 * 60) {
            for e in f.step(1.0 / 120.0, player, Vec3::ZERO, false, false, &flat, &wall) {
                match e {
                    Event::Folded { .. } => panic!("a Corrupted folded"),
                    Event::Fire { .. } => panic!("a Corrupted shot instead of lunging"),
                    Event::Melee { dmg, .. } => {
                        assert!(dmg > 0.0);
                        melee = true;
                    }
                    _ => {}
                }
            }
            if melee {
                break;
            }
        }
        assert!(melee, "the drifter never reached the player through a wall it should not need to see through");
    }

    #[test]
    fn a_player_who_steps_into_the_lane_is_rammed_once_per_pass() {
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        f.foes[0].agent.state = State::Chase;
        let start = f.foes[0].pos();
        let mut player = start + Vec3::new(0.0, 0.0, -90.0);
        // let it fold and get rolling
        let mut ticks = 0;
        while !(f.foes[0].form == Form::Vehicle && f.foes[0].fold_t <= 0.0) {
            f.step(1.0 / 120.0, player, Vec3::ZERO, false, false, &flat, &open);
            ticks += 1;
            assert!(ticks < 120 * 5, "never got rolling");
        }
        // the player drives INTO its path: one metre ahead of the nose,
        // every tick, for half a second — inside RAM_R the whole time
        let mut rams = 0;
        for _ in 0..60 {
            let foe = &f.foes[0];
            player = foe.pos() + foe.facing * 1.0;
            for e in f.step(1.0 / 120.0, player, Vec3::ZERO, false, false, &flat, &open) {
                if let Event::Ram { dmg, dv, .. } = e {
                    assert!(dmg > 0.0 && dv.length() > 0.0);
                    rams += 1;
                }
            }
        }
        assert_eq!(rams, 1, "{rams} rams in half a second of contact; the cooldown is {RAM_CD_S} s");
        assert_eq!(f.foes[0].form, Form::Vehicle, "a foe that is ramming does not stop to unfold");
    }

    #[test]
    fn a_natural_flank_does_not_clip_the_player() {
        // the flank lane ends FLANK_M beside the player; driving it should
        // never come inside RAM_R of someone standing still
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        f.foes[0].agent.state = State::Chase;
        let player = f.foes[0].pos() + Vec3::new(0.0, 0.0, -90.0);
        let mut closest = f32::MAX;
        for _ in 0..(120 * 20) {
            for e in f.step(1.0 / 120.0, player, Vec3::ZERO, false, false, &flat, &open) {
                if let Event::Ram { .. } = e {
                    panic!("rammed a player who stood still beside the lane");
                }
            }
            if f.foes[0].form == Form::Vehicle {
                closest = closest.min((f.foes[0].pos() - player).length());
            }
            if f.foes[0].form == Form::Robot && f.foes[0].fold_t <= 0.0 && closest < f32::MAX {
                break;
            }
        }
        assert!(closest > RAM_R && closest < FLANK_M * 1.5, "closest approach {closest:.1} m");
    }

    #[test]
    fn the_player_rams_a_foe_only_in_vehicle_form_at_speed() {
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let at = f.foes[0].pos();
        let fast = Vec3::new(0.0, 0.0, 20.0);
        // robot form: a hit at speed is not a ram
        let ev = f.step(1.0 / 120.0, at, fast, false, false, &flat, &open);
        assert!(!ev.iter().any(|e| matches!(e, Event::Rammed { .. })), "rammed in robot form");
        // vehicle form, slow: not a ram
        let ev = f.step(1.0 / 120.0, at, Vec3::new(0.0, 0.0, 3.0), true, false, &flat, &open);
        assert!(!ev.iter().any(|e| matches!(e, Event::Rammed { .. })), "a rolling nudge counted");
        // vehicle form, fast: one ram, then the cooldown holds
        let ev = f.step(1.0 / 120.0, at, fast, true, false, &flat, &open);
        let n = ev.iter().filter(|e| matches!(e, Event::Rammed { .. })).count();
        assert_eq!(n, 1);
        let ev = f.step(1.0 / 120.0, at, fast, true, false, &flat, &open);
        assert!(!ev.iter().any(|e| matches!(e, Event::Rammed { .. })), "rammed twice in two ticks");
    }

    #[test]
    fn sync_kills_once_from_the_fight_and_clear_downs_the_rest() {
        let mut s = wave(&["grunt", "grunt", "grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        s.fighters[1].hp = 0.0;
        f.sync(&s.fighters);
        f.sync(&s.fighters);
        let ev = f.step(1.0 / 120.0, Vec3::new(0.0, 0.19, -500.0), Vec3::ZERO, false, false, &flat, &open);
        let downs: Vec<_> = ev.iter().filter(|e| matches!(e, Event::Down { slot: 1 })).collect();
        assert_eq!(downs.len(), 1, "a fight kill reported {} times", downs.len());
        assert!(!f.foes[1].alive());
        assert_eq!(f.alive(), 2);
        f.clear();
        let ev = f.step(1.0 / 120.0, Vec3::ZERO, Vec3::ZERO, false, false, &flat, &open);
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::Down { .. })).count(), 2);
        assert!(f.foes.is_empty());
    }

    #[test]
    fn line_of_sight_is_asked_a_few_times_a_second_not_every_tick() {
        use std::cell::Cell;
        let s = wave(&["grunt", "grunt", "grunt", "grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let asks = Cell::new(0u32);
        let counting = |_a: Vec3, _b: Vec3| {
            asks.set(asks.get() + 1);
            true
        };
        let player = Vec3::new(0.0, 0.19, 40.0); // in sight, at range
        for _ in 0..120 {
            f.step(1.0 / 120.0, player, Vec3::ZERO, false, false, &flat, &counting);
        }
        let n = asks.get();
        // 4 foes x 4 asks/s, plus the staggered first ask
        assert!(n <= 4 * 5 && n >= 4 * 3, "{n} LOS asks in one second for four foes");
    }

    #[test]
    fn a_shot_is_worth_its_line_dps_over_its_cooldown() {
        let s = wave(&["ravager"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let foe = &f.foes[0];
        let per = fighter_dmg_per_shot(foe);
        let expect = s.fighters[0].dps * foe.agent.fire_cooldown;
        assert!((per - expect).abs() < 1e-3, "per-shot {per} vs dps*cd {expect}");
        assert!(per > 0.0);
    }

    #[test]
    fn a_wounded_foe_wakes_and_faces_the_shooter() {
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        assert_eq!(f.foes[0].agent.state, State::Idle);
        f.poke(0);
        assert_eq!(f.foes[0].agent.state, State::Chase);
        // out of sight but awake: it still closes on where you are —
        // far enough that it folds and drives, so give it the fold
        let player = Vec3::new(30.0, 0.19, 30.0);
        let before = f.foes[0].pos();
        for _ in 0..240 {
            f.step(1.0 / 120.0, player, Vec3::ZERO, false, false, &flat, &wall);
        }
        let after = f.foes[0].pos();
        assert!((player - after).length() < (player - before).length(), "did not close on the shooter");
    }
}
