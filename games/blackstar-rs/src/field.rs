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
//! ## Two registers, and being caught between forms
//!
//! The brief is a mix: Dark of the Moon's slow, heavy, consequential
//! hits and War for Cybertron's stream of fast rounds. `weapons::
//! Register` names which a form is; the field answers each differently.
//! A HEAVY hit worth a real fraction of the body's health STAGGERS it —
//! still, silent, exposed for `STAGGER_S`. FAST fire SUPPRESSES: every
//! hit adds to a pressure that widens the foe's spread and slows its
//! advance, and drains when the fire stops. And a body hit hard while
//! it is FOLDING is caught between forms — the fold stalls, and a shot
//! into the exposed works costs it `MIDFOLD_EXPOSURE` times more. That
//! last one is the reward for the still window every fold telegraphs.
//!
//! ## The cable
//!
//! `cable.rs` is a tether machine; this is where it becomes moves. A
//! hostile from the rails (`cable_troop`) fires one at a robot-form
//! player in the open and REELS — the pull, which drags you out of
//! cover toward it unless you are heavier than it is, in which case it
//! drags itself to you. The player in vehicle form hooks a foe and it
//! becomes a TOW: dragged behind the car, unable to act, wearing road
//! damage by the second. Turn hard and the body on the end swings —
//! `Cable::swing_ms` — and letting go at speed is the WHIP: the body
//! flies, and whatever it lands on takes the impact too. A foe heavier
//! than the car reverses the trade and yanks the car instead.
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

use crate::cable::{self, Cable, End, Fate, Mode};
use crate::content::{Role, Troop};
use crate::enemy::{Enemy, State};
use crate::escalation::EnemyKind;
use crate::patrol::Faction;
use crate::raidgen::strhash;
use crate::skirmish::Fighter;
use crate::weapons::{Element, Register, Verb};

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

// -- the two registers --
/// A heavy hit worth at least this fraction of max hp staggers. GUESS:
/// two railshots on a grunt, one on a swarmer.
pub const STAGGER_FRAC: f32 = 0.18;
/// How long a stagger holds a body still. GUESS: long enough to walk
/// around it, short enough that it is not a stun-lock.
pub const STAGGER_S: f32 = 1.4;
/// Suppression added per fast hit, and its decay per second. GUESS:
/// four hits in a second pins a grunt, a second of silence frees it.
pub const SUPPRESS_PER_HIT: f32 = 0.28;
pub const SUPPRESS_DECAY: f32 = 0.9;
/// A hit while folding stalls the fold by this many seconds, capped at
/// one whole extra fold. GUESS.
pub const FOLD_STALL_S: f32 = 0.45;
/// Damage multiplier on a body caught mid-fold. GUESS: the reward for
/// hitting the window.
pub const MIDFOLD_EXPOSURE: f32 = 1.6;
/// Damage multiplier on a body being towed — dragged, upside down, not
/// shooting back. GUESS.
pub const TOWED_EXPOSURE: f32 = 1.3;

// -- the cable --
/// A cable troop reaches for the player between these ranges, robot
/// form, line of sight. GUESSES.
pub const CABLE_MIN_M: f32 = 8.0;
pub const CABLE_MAX_M: f32 = 42.0;
pub const CABLE_CD_S: f32 = 7.0;
/// A pull that has not brought its target home in this long lets go.
pub const CABLE_LIFE_S: f32 = 4.5;
/// The player's vehicle-form hook reaches this far. GUESS: the Watch's
/// GRAPPLE reach, so the two feel like one tool.
pub const HOOK_REACH_M: f32 = 55.0;
/// A tow costs the towed body this much per second per m/s of speed —
/// road damage. GUESS sized so a ten-second drag at speed is a kill on
/// a grunt and a scratch on a juggernaut.
pub const TOW_DMG_PER_MS_S: f32 = 0.35;
/// Road damage is reported in lumps this often so the fight is not asked
/// 120 times a second for a rounding error.
pub const TOW_REPORT_S: f32 = 0.25;
/// A release with at least this much swing is a whip; less is a drop.
pub const WHIP_MIN_MS: f32 = 9.0;
/// A flung body lands its speed times this on whatever it hits, and on
/// itself when it lands. GUESS: the ram's number, because it is a ram.
pub const THROW_DMG_PER_MS: f32 = RAM_DMG_PER_MS;
/// How heavy a chassis is next to the car, per unit of `Chassis::bulk`.
/// GUESS: a standard bot is a little heavier than the car that stands
/// in for it, a juggernaut is a truck.
pub const BOT_MASS_PER_BULK: f32 = 1.3;

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
    /// relative mass, `cable::CAR_MASS` units, from the chassis bulk
    pub mass: f32,
    /// seconds of stagger left; a staggered body does nothing
    pub stagger_t: f32,
    /// suppression pressure, 0..1+; widens spread, slows the advance
    pub suppress: f32,
    /// this foe's cable on the player (the pull), while it holds
    cable: Option<Cable>,
    cable_cd: f32,
    /// airborne after a whip: velocity, integrated under gravity until
    /// it lands or hits someone
    fling: Option<Vec3>,
    /// road damage owed to the fight, reported in lumps
    tow_owed: f32,
    tow_t: f32,
}

impl Foe {
    /// Is it doing anything at all this tick — no stagger, no tow, no
    /// flight, no fold in progress.
    pub fn free(&self) -> bool {
        self.alive() && self.stagger_t <= 0.0 && self.fling.is_none() && self.fold_t <= 0.0
    }
    /// What a hit on this body is worth right now, over its base: caught
    /// mid-fold or on the end of a tow, it is exposed.
    pub fn exposure(&self, towed: bool) -> f32 {
        let mut m = 1.0;
        if self.fold_t > 0.0 {
            m *= MIDFOLD_EXPOSURE;
        }
        if towed {
            m *= TOWED_EXPOSURE;
        }
        m
    }
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
    /// a heavy hit put this body down for `STAGGER_S`
    Staggered { slot: usize },
    /// a hostile fired a cable at the player: draw it from `from`
    CableOut { slot: usize, from: Vec3 },
    /// the cable is hauling: apply `dv` to the player this tick
    Pulled { slot: usize, dv: Vec3 },
    /// a cable parted or was let go
    CableOff { slot: usize },
    /// the player's hook took hold of this body
    Hooked { slot: usize },
    /// the tow line is pulling the CAR: a heavier body on the end
    Yanked { slot: usize, dv: Vec3 },
    /// road damage owed to this body from being dragged (`Element::None`)
    Towed { slot: usize, dmg: f32 },
    /// the whip let go: the body is flying at `vel`
    Whipped { slot: usize, vel: Vec3 },
    /// a flung body landed — on the ground (`into: None`) or on another
    /// foe. `dmg` is what the flung body takes; the one it landed on
    /// takes the same
    Thrown { slot: usize, into: Option<usize>, dmg: f32 },
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
    /// the player's tow line: which slot is on the end, and the cable
    tow: Option<(usize, Cable)>,
    /// gravity for flung bodies, m/s². Set by the shell for the map;
    /// the default is Earth so headless tests fly a sensible arc.
    pub g: f32,
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

/// Who carries a cable: Blackstar's rail ranks ("raiders on the rails"
/// — the Dreadwake roster in `content.rs`) and the Hijack elite that is
/// starting to field recovered tech. Everyone else shoots.
pub fn cable_troop(t: &Troop) -> bool {
    t.faction == Faction::BlackstarClans || t.id.contains("harvester")
}

/// A chassis's mass next to the car, from its bulk.
fn mass_for(t: &Troop) -> f32 {
    let bulk = crate::content::chassis(t.chassis).map(|c| c.bulk).unwrap_or(1.0);
    cable::CAR_MASS * BOT_MASS_PER_BULK * bulk
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
        Field { foes: Vec::new(), anchor, facing: unit_xz(facing), seed, ride_h, pending: Vec::new(), tow: None, g: 9.81 }
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
                mass: mass_for(t),
                stagger_t: 0.0,
                suppress: 0.0,
                cable: None,
                cable_cd: CABLE_CD_S * 0.5 + hash_unit(h, 5) * CABLE_CD_S * 0.5,
                fling: None,
                tow_owed: 0.0,
                tow_t: 0.0,
            });
        }
    }

    /// The wave is over: nobody left to stand.
    pub fn clear(&mut self) {
        for f in &self.foes {
            if f.alive() {
                self.pending.push(Event::Down { slot: f.slot });
            }
            if f.cable.is_some() {
                self.pending.push(Event::CableOff { slot: f.slot });
            }
        }
        if let Some((slot, _)) = self.tow.take() {
            self.pending.push(Event::CableOff { slot });
        }
        self.foes.clear();
    }

    /// Is this slot on the end of the player's tow line.
    pub fn towed(&self, slot: usize) -> bool {
        self.tow.as_ref().map(|t| t.0 == slot).unwrap_or(false)
    }

    /// The player's hook, from vehicle form: the nearest living foe inside
    /// `HOOK_REACH_M` and within the aim cone takes the line. Returns the
    /// slot hooked. One tow at a time; a second hook while one holds is
    /// refused rather than silently swapping.
    pub fn hook(&mut self, from: Vec3, aim: Vec3, player_mass: f32) -> Option<usize> {
        if self.tow.is_some() {
            return None;
        }
        let aim = aim.normalize_or(Vec3::Z);
        let mut best: Option<(f32, usize)> = None;
        for f in self.foes.iter().filter(|f| f.alive() && f.fling.is_none()) {
            let to = f.pos() - from;
            let d = to.length();
            if d > HOOK_REACH_M || d < 1.0 {
                continue;
            }
            if to.normalize_or(aim).dot(aim) < 0.94 {
                continue;
            }
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, f.slot));
            }
        }
        let (_, slot) = best?;
        let foe = self.foes.iter_mut().find(|f| f.slot == slot)?;
        let a = End { pos: from, vel: Vec3::ZERO, mass: player_mass };
        let b = End { pos: foe.pos(), vel: Vec3::ZERO, mass: foe.mass };
        self.tow = Some((slot, Cable::between(&a, &b)));
        foe.goal = None;
        self.pending.push(Event::Hooked { slot });
        Some(slot)
    }

    /// Let the tow line go. At speed this is the whip: the body on the
    /// end flies with the swing it had. Below `WHIP_MIN_MS` it is just
    /// dropped where it is.
    pub fn release(&mut self) {
        let Some((slot, mut c)) = self.tow.take() else { return };
        let swing = c.swing_ms;
        if let Some(foe) = self.foes.iter_mut().find(|f| f.slot == slot) {
            let b = End { pos: foe.pos(), vel: foe.vel, mass: foe.mass };
            let v = c.release(&b);
            if swing >= WHIP_MIN_MS && foe.alive() {
                // carry the swing, and a little lift so it is a throw
                let vel = v + Vec3::Y * (v.length() * 0.25);
                foe.fling = Some(vel);
                self.pending.push(Event::Whipped { slot, vel });
            }
        }
        self.pending.push(Event::CableOff { slot });
    }

    /// A hit on this body, in a register. The FIGHT took the damage;
    /// this is the field's answer to how it was delivered: heavy and
    /// large enough staggers, fast suppresses, anything mid-fold stalls
    /// the fold. `dmg` is what the fight was told (after exposure).
    pub fn hit(&mut self, slot: usize, dmg: f32, reg: Register) {
        let Some(foe) = self.foes.iter_mut().find(|f| f.slot == slot) else { return };
        if !foe.alive() {
            return;
        }
        if foe.agent.state == State::Idle {
            foe.agent.state = State::Chase;
        }
        match reg {
            Register::Heavy => {
                if foe.fold_t > 0.0 {
                    foe.fold_t = (foe.fold_t + FOLD_STALL_S).min(FOLD_S * 2.0);
                }
                if dmg >= STAGGER_FRAC * foe.agent.max_hp && foe.stagger_t <= 0.0 {
                    foe.stagger_t = STAGGER_S;
                    // a stagger drops whatever it was hauling
                    if foe.cable.take().is_some() {
                        self.pending.push(Event::CableOff { slot });
                    }
                    self.pending.push(Event::Staggered { slot });
                }
            }
            Register::Fast => {
                foe.suppress = (foe.suppress + SUPPRESS_PER_HIT).min(1.5);
                if foe.fold_t > 0.0 {
                    foe.fold_t = (foe.fold_t + FOLD_STALL_S * 0.3).min(FOLD_S * 2.0);
                }
            }
            Register::Medium | Register::Utility => {
                if foe.fold_t > 0.0 {
                    foe.fold_t = (foe.fold_t + FOLD_STALL_S * 0.6).min(FOLD_S * 2.0);
                }
            }
        }
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

    /// One fixed step. The field is handed the player and hands back
    /// events; `ground` is the height field; `los` the shell's own
    /// line-of-sight answer, asked at most every `LOS_EVERY_S` per foe.
    pub fn step(
        &mut self,
        dt: f32,
        player: &Player,
        ground: &dyn Fn(f32, f32) -> f32,
        los: &dyn Fn(Vec3, Vec3) -> bool,
    ) -> Vec<Event> {
        let mut ev: Vec<Event> = std::mem::take(&mut self.pending);
        let ride_h = self.ride_h;
        let g = self.g;
        let player_pos = player.pos;
        let towed_slot = self.tow.as_ref().map(|t| t.0);
        let seed = self.seed;

        // positions of the living, for a flung body to land on
        let others: Vec<(usize, Vec3)> = self.foes.iter().filter(|f| f.alive()).map(|f| (f.slot, f.pos())).collect();

        for foe in self.foes.iter_mut() {
            if !foe.alive() {
                foe.vel = Vec3::ZERO;
                continue;
            }
            let before = foe.agent.position;
            foe.ram_cd = (foe.ram_cd - dt).max(0.0);
            foe.cable_cd = (foe.cable_cd - dt).max(0.0);
            foe.suppress = (foe.suppress - SUPPRESS_DECAY * dt).max(0.0);

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

            // -- airborne after a whip: a ballistic arc, then a landing --
            if let Some(mut v) = foe.fling {
                v.y -= g * dt;
                foe.agent.position += v * dt;
                foe.fling = Some(v);
                let p = foe.agent.position;
                let floor = ground(p.x, p.z) + ride_h;
                // landed on someone: both wear it
                let hit = others
                    .iter()
                    .filter(|(sl, _)| *sl != foe.slot)
                    .find(|(_, op)| (*op - p).length() <= RAM_R)
                    .map(|(sl, _)| *sl);
                if p.y <= floor || hit.is_some() {
                    let speed = v.length();
                    foe.agent.position.y = floor;
                    foe.fling = None;
                    foe.stagger_t = STAGGER_S;
                    ev.push(Event::Thrown { slot: foe.slot, into: hit, dmg: speed * THROW_DMG_PER_MS });
                }
                foe.vel = v;
                continue;
            }

            // -- on the end of the player's tow: dragged, not acting --
            if towed_slot == Some(foe.slot) {
                // the tow physics runs below, once, with the cable
                if dist > 1e-3 {
                    foe.facing = dir_xz;
                }
                continue;
            }

            // -- staggered: still, and the fold does not advance --
            if foe.stagger_t > 0.0 {
                foe.stagger_t -= dt;
                foe.vel = Vec3::ZERO;
                continue;
            }

            // -- a fold in progress: still, then done --
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

            // -- hauling on the player: the pull --
            if let Some(c) = foe.cable.as_mut() {
                let a = End { pos: foe.agent.position, vel: Vec3::ZERO, mass: foe.mass };
                let b = End { pos: player.pos, vel: player.vel, mass: player.mass };
                let (pull, fate) = c.step(dt, &a, &b, Mode::Reel);
                let age = c.age;
                match fate {
                    Fate::Holding => {
                        // a heavier player drags the hauler toward them
                        foe.agent.position += pull.a * dt * 4.0;
                        if pull.b.length_squared() > 0.0 {
                            ev.push(Event::Pulled { slot: foe.slot, dv: pull.b });
                        }
                        if age > CABLE_LIFE_S || player.vehicle {
                            // a car does not stay on a line; a long haul
                            // that never landed lets go
                            foe.cable = None;
                            ev.push(Event::CableOff { slot: foe.slot });
                        }
                    }
                    Fate::Reeled => {
                        foe.cable = None;
                        ev.push(Event::CableOff { slot: foe.slot });
                        // that was the point of dragging you in
                        ev.push(Event::Melee { slot: foe.slot, dmg: foe.agent.melee_damage().max(fighter_dmg_per_shot(foe)) });
                    }
                    Fate::Snapped { .. } => {
                        foe.cable = None;
                        ev.push(Event::CableOff { slot: foe.slot });
                    }
                }
                if dist > 1e-3 {
                    foe.facing = dir_xz;
                }
                foe.vel = (foe.agent.position - before) / dt.max(1e-4);
                let p = &mut foe.agent.position;
                p.y = ground(p.x, p.z) + ride_h;
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
                    // suppressed: slower on the approach
                    let los_now = foe.los;
                    let sup = foe.suppress.min(1.0);
                    let base_speed = foe.agent.move_speed;
                    foe.agent.move_speed = base_speed * (1.0 - 0.5 * sup);
                    foe.agent.update(los_now, player.cloaked, player_pos, dt);
                    foe.agent.move_speed = base_speed;

                    // the cable, on its own clock, whenever it fits: a
                    // rail rank does not wait to be in its standoff to
                    // throw a line — the line is how it closes the gap
                    let reaches = cable_troop(foe.troop)
                        && foe.cable_cd <= 0.0
                        && foe.los
                        && !player.vehicle
                        && dist >= CABLE_MIN_M
                        && dist <= CABLE_MAX_M
                        && matches!(foe.agent.state, State::Chase | State::Attack);
                    if reaches {
                        foe.agent.wants_to_fire = false;
                        foe.cable_cd = CABLE_CD_S;
                        let a = End { pos: foe.agent.position, vel: Vec3::ZERO, mass: foe.mass };
                        let b = End { pos: player.pos, vel: player.vel, mass: player.mass };
                        foe.cable = Some(Cable::between(&a, &b));
                        ev.push(Event::CableOut { slot: foe.slot, from: foe.agent.position + Vec3::Y * CHEST_H });
                    }

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
                            // deterministic scatter, wider with range and
                            // wider again under fire
                            let h = strhash(foe.troop.id) ^ (foe.shots as u64) ^ seed;
                            let spread = SPREAD_RAD * (dist / SPREAD_REF_M).clamp(0.4, 2.0) * (1.0 + 2.0 * sup);
                            let dir = scatter(aim, spread, h);
                            ev.push(Event::Fire { slot: foe.slot, from, dir, verb: foe.troop.fire, element: foe.troop.element, dmg });
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

                    // the swarm shoots from the saddle: a Hijack in
                    // vehicle form takes wild pot-shots while closing
                    if foe.agent.kind == EnemyKind::Hijack {
                        foe.agent.fire_timer -= dt;
                        if foe.agent.fire_timer <= 0.0 && foe.los && dist <= foe.agent.attack_range {
                            foe.agent.fire_timer = foe.agent.fire_cooldown * 1.5;
                            foe.shots += 1;
                            let from = foe.agent.position + Vec3::Y * (CHEST_H * 0.5);
                            let aim = (player_pos + Vec3::Y * CHEST_H - from).normalize_or(dir_xz);
                            let h = strhash(foe.troop.id) ^ (foe.shots as u64) ^ seed;
                            let dir = scatter(aim, SPREAD_RAD * 3.0, h);
                            ev.push(Event::Fire { slot: foe.slot, from, dir, verb: foe.troop.fire, element: foe.troop.element, dmg: fighter_dmg_per_shot(foe) * 0.6 });
                        }
                    }

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
            if player.vehicle && d_after <= RAM_R && foe.ram_cd <= 0.0 {
                let v = player.vel.length();
                if v >= PLAYER_RAM_MIN_MS {
                    foe.ram_cd = RAM_CD_S;
                    let dv = unit_xz(player.vel) * v * 0.5 + Vec3::Y * (v * 0.12);
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

        // -- THE TOW: the player's line, one cable, physics once --
        if let Some((slot, c)) = self.tow.as_mut() {
            let slot = *slot;
            let mut drop = false;
            if let Some(foe) = self.foes.iter_mut().find(|f| f.slot == slot) {
                if !foe.alive() || !player.vehicle {
                    // a dead body slips the hook; a robot cannot tow
                    drop = true;
                } else {
                    let a = End { pos: player.pos, vel: player.vel, mass: player.mass };
                    let b = End { pos: foe.agent.position, vel: foe.vel, mass: foe.mass };
                    let (pull, fate) = c.step(dt, &a, &b, Mode::Hold);
                    match fate {
                        Fate::Holding => {
                            if pull.a.length_squared() > 0.0 {
                                ev.push(Event::Yanked { slot, dv: pull.a });
                            }
                            // the towed body: a velocity, ground drag, no will
                            foe.vel += pull.b;
                            foe.vel *= 1.0 - (2.5 * dt).min(0.5);
                            foe.agent.position += foe.vel * dt;
                            let p = &mut foe.agent.position;
                            p.y = ground(p.x, p.z) + ride_h;
                            let speed = foe.vel.length();
                            foe.tow_owed += TOW_DMG_PER_MS_S * speed * dt;
                            foe.tow_t += dt;
                            if foe.tow_t >= TOW_REPORT_S && foe.tow_owed > 0.0 {
                                ev.push(Event::Towed { slot, dmg: foe.tow_owed });
                                foe.tow_owed = 0.0;
                                foe.tow_t = 0.0;
                            }
                        }
                        Fate::Snapped { .. } | Fate::Reeled => drop = true,
                    }
                }
            } else {
                drop = true;
            }
            if drop {
                self.tow = None;
                ev.push(Event::CableOff { slot });
            }
        }
        ev
    }
}

/// A deterministic cone scatter around `aim`: two hashed lanes, one per
/// axis, scaled by `spread` in radians.
fn scatter(aim: Vec3, spread: f32, h: u64) -> Vec3 {
    let side = aim.cross(Vec3::Y).normalize_or(Vec3::X);
    let upv = side.cross(aim).normalize_or(Vec3::Y);
    let jx = hash_unit(h, 3) * 2.0 - 1.0;
    let jy = hash_unit(h, 4) * 2.0 - 1.0;
    (aim + (side * jx + upv * jy) * spread).normalize_or(aim)
}

/// The player, as the field sees them this tick.
#[derive(Clone, Copy, Debug)]
pub struct Player {
    pub pos: Vec3,
    pub vel: Vec3,
    /// vehicle form: rams, tows, cannot be pulled
    pub vehicle: bool,
    /// the cloak ability, when the shell wires it
    pub cloaked: bool,
    /// relative mass, `cable::CAR_MASS` units
    pub mass: f32,
}

impl Player {
    pub fn standing(pos: Vec3) -> Player {
        Player { pos, vel: Vec3::ZERO, vehicle: false, cloaked: false, mass: cable::CAR_MASS }
    }
    pub fn driving(pos: Vec3, vel: Vec3) -> Player {
        Player { pos, vel, vehicle: true, cloaked: false, mass: cable::CAR_MASS }
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
            for e in f.step(1.0 / 120.0, &Player::standing(player), &flat, &open) {
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
            for e in f.step(1.0 / 120.0, &Player::standing(player), &flat, &wall) {
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
            f.step(1.0 / 120.0, &Player::standing(player), &flat, &open);
            ticks += 1;
            assert!(ticks < 120 * 5, "never got rolling");
        }
        // the player drives INTO its path: one metre ahead of the nose,
        // every tick, for half a second — inside RAM_R the whole time
        let mut rams = 0;
        for _ in 0..60 {
            let foe = &f.foes[0];
            player = foe.pos() + foe.facing * 1.0;
            for e in f.step(1.0 / 120.0, &Player::standing(player), &flat, &open) {
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
            for e in f.step(1.0 / 120.0, &Player::standing(player), &flat, &open) {
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
        let ev = f.step(1.0 / 120.0, &Player { pos: at, vel: fast, vehicle: false, cloaked: false, mass: cable::CAR_MASS }, &flat, &open);
        assert!(!ev.iter().any(|e| matches!(e, Event::Rammed { .. })), "rammed in robot form");
        // vehicle form, slow: not a ram
        let ev = f.step(1.0 / 120.0, &Player::driving(at, Vec3::new(0.0, 0.0, 3.0)), &flat, &open);
        assert!(!ev.iter().any(|e| matches!(e, Event::Rammed { .. })), "a rolling nudge counted");
        // vehicle form, fast: one ram, then the cooldown holds
        let ev = f.step(1.0 / 120.0, &Player::driving(at, fast), &flat, &open);
        let n = ev.iter().filter(|e| matches!(e, Event::Rammed { .. })).count();
        assert_eq!(n, 1);
        let ev = f.step(1.0 / 120.0, &Player::driving(at, fast), &flat, &open);
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
        let ev = f.step(1.0 / 120.0, &Player::standing(Vec3::new(0.0, 0.19, -500.0)), &flat, &open);
        let downs: Vec<_> = ev.iter().filter(|e| matches!(e, Event::Down { slot: 1 })).collect();
        assert_eq!(downs.len(), 1, "a fight kill reported {} times", downs.len());
        assert!(!f.foes[1].alive());
        assert_eq!(f.alive(), 2);
        f.clear();
        let ev = f.step(1.0 / 120.0, &Player::standing(Vec3::ZERO), &flat, &open);
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
            f.step(1.0 / 120.0, &Player::standing(player), &flat, &counting);
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
            f.step(1.0 / 120.0, &Player::standing(player), &flat, &wall);
        }
        let after = f.foes[0].pos();
        assert!((player - after).length() < (player - before).length(), "did not close on the shooter");
    }

    // ---- the two registers ----

    #[test]
    fn a_heavy_hit_staggers_and_a_stream_of_fast_ones_suppresses() {
        let s = wave(&["grunt", "grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let max = f.foes[0].agent.max_hp;
        // heavy but small: no stagger
        f.hit(0, max * STAGGER_FRAC * 0.5, Register::Heavy);
        assert_eq!(f.foes[0].stagger_t, 0.0);
        // heavy and large: staggered, and the field says so
        f.hit(0, max * STAGGER_FRAC, Register::Heavy);
        assert!(f.foes[0].stagger_t > 0.0);
        let ev = f.step(1.0 / 120.0, &Player::standing(Vec3::new(0.0, 0.19, 40.0)), &flat, &open);
        assert!(ev.iter().any(|e| matches!(e, Event::Staggered { slot: 0 })));
        // a staggered body does not move for STAGGER_S
        let before = f.foes[0].pos();
        for _ in 0..(120 as f32 * STAGGER_S * 0.9) as usize {
            f.step(1.0 / 120.0, &Player::standing(Vec3::new(0.0, 0.19, 40.0)), &flat, &open);
        }
        assert_eq!(f.foes[0].pos(), before, "moved while staggered");
        assert!(!f.foes[0].free());
        // fast fire never staggers, however much of it
        for _ in 0..20 {
            f.hit(1, max, Register::Fast);
        }
        assert_eq!(f.foes[1].stagger_t, 0.0);
        assert!(f.foes[1].suppress >= 1.0, "twenty fast hits did not pin it: {}", f.foes[1].suppress);
        // and suppression drains when the fire stops
        for _ in 0..(120 * 3) {
            f.step(1.0 / 120.0, &Player::standing(Vec3::new(0.0, 0.19, 400.0)), &flat, &wall);
        }
        assert_eq!(f.foes[1].suppress, 0.0);
    }

    #[test]
    fn a_body_hit_mid_fold_is_caught_between_forms_and_exposed() {
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        f.foes[0].agent.state = State::Chase;
        let player = Player::standing(Vec3::new(0.0, 0.19, -90.0));
        let mut n = 0;
        while f.foes[0].fold_t <= 0.0 {
            f.step(1.0 / 120.0, &player, &flat, &open);
            n += 1;
            assert!(n < 600, "never started folding");
        }
        assert!((f.foes[0].exposure(false) - MIDFOLD_EXPOSURE).abs() < 1e-6);
        let left = f.foes[0].fold_t;
        f.hit(0, 1.0, Register::Heavy);
        assert!(f.foes[0].fold_t > left, "the fold did not stall");
        assert!(f.foes[0].fold_t <= FOLD_S * 2.0, "stall past the cap");
        for _ in 0..(120 * 3) {
            f.step(1.0 / 120.0, &player, &flat, &open);
        }
        assert_eq!(f.foes[0].fold_t, 0.0);
        assert_eq!(f.foes[0].exposure(false), 1.0);
    }

    // ---- the cable ----

    fn rail_troop() -> &'static str {
        crate::content::hostiles()
            .find(|t| cable_troop(t))
            .map(|t| t.id)
            .expect("no cable troop in content")
    }

    #[test]
    fn a_rail_rank_reaches_for_a_standing_player_and_hauls() {
        let id = rail_troop();
        let s = wave(&[id]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        f.foes[0].cable_cd = 0.0;
        let at = f.foes[0].pos() + Vec3::new(0.0, 0.0, -30.0);
        let mut player = Player::standing(at);
        let mut out = false;
        let mut pulled = Vec3::ZERO;
        for _ in 0..(120 * 8) {
            for e in f.step(1.0 / 120.0, &player, &flat, &open) {
                match e {
                    Event::CableOut { slot: 0, .. } => out = true,
                    Event::Pulled { dv, .. } => {
                        pulled += dv;
                        player.vel += dv;
                    }
                    _ => {}
                }
            }
            player.vel *= 0.96;
            player.pos += player.vel * (1.0 / 120.0);
            if pulled.length() > 3.0 {
                break;
            }
        }
        assert!(out, "the rail rank never threw a cable");
        assert!(pulled.length() > 3.0, "the cable never hauled: {pulled:?}");
        let toward = (f.foes[0].pos() - at).normalize_or(Vec3::Z);
        assert!(pulled.normalize_or(Vec3::Z).dot(toward) > 0.8, "pulled the wrong way");
    }

    #[test]
    fn a_car_cannot_be_pulled_and_a_line_on_one_is_let_go() {
        let id = rail_troop();
        let s = wave(&[id]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        f.foes[0].cable_cd = 0.0;
        let at = f.foes[0].pos() + Vec3::new(0.0, 0.0, -30.0);
        let mut got = false;
        for _ in 0..(120 * 6) {
            for e in f.step(1.0 / 120.0, &Player::standing(at), &flat, &open) {
                if let Event::CableOut { .. } = e {
                    got = true;
                }
            }
            if got {
                break;
            }
        }
        assert!(got);
        let ev = f.step(1.0 / 120.0, &Player::driving(at, Vec3::ZERO), &flat, &open);
        assert!(ev.iter().any(|e| matches!(e, Event::CableOff { slot: 0 })), "a car stayed on the line");
        assert!(f.foes[0].cable.is_none());
    }

    #[test]
    fn the_hook_takes_the_nearest_body_in_the_cone_and_only_one_at_a_time() {
        let s = wave(&["grunt", "grunt", "grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let from = f.anchor + f.facing * 30.0;
        let aim = -f.facing;
        let slot = f.hook(from, aim, cable::CAR_MASS).expect("nothing hooked");
        let d = (f.foes.iter().find(|x| x.slot == slot).unwrap().pos() - from).length();
        for foe in &f.foes {
            let od = (foe.pos() - from).length();
            let to = (foe.pos() - from).normalize_or(aim);
            if to.dot(aim) >= 0.94 {
                assert!(od >= d - 1e-3, "hooked slot {slot} at {d:.1} with slot {} nearer at {od:.1}", foe.slot);
            }
        }
        assert!(f.towed(slot));
        assert!(f.hook(from, aim, cable::CAR_MASS).is_none());
        f.release();
        assert!(f.hook(from + f.facing * 200.0, aim, cable::CAR_MASS).is_none());
    }

    #[test]
    fn a_towed_grunt_is_dragged_wears_road_damage_and_cannot_act() {
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let start = f.foes[0].pos();
        let mut car = Player::driving(start + Vec3::new(0.0, 0.0, -10.0), Vec3::new(0.0, 0.0, -18.0));
        f.hook(car.pos, (start - car.pos).normalize(), car.mass).expect("hook");
        let mut owed = 0.0;
        let mut fired = false;
        for _ in 0..(120 * 4) {
            for e in f.step(1.0 / 120.0, &car, &flat, &open) {
                match e {
                    Event::Towed { dmg, .. } => owed += dmg,
                    Event::Fire { .. } | Event::Melee { .. } => fired = true,
                    Event::Yanked { dv, .. } => car.vel += dv,
                    _ => {}
                }
            }
            car.vel = car.vel.normalize_or(Vec3::NEG_Z) * 18.0;
            car.pos += car.vel * (1.0 / 120.0);
        }
        assert!(f.towed(0), "the line did not hold a grunt behind a car");
        let moved = (f.foes[0].pos() - start).length();
        assert!(moved > 30.0, "dragged only {moved:.1} m in four seconds");
        assert!(owed > 0.0, "no road damage owed");
        assert!(!fired, "a towed body shot back");
        assert!((f.foes[0].exposure(true) - TOWED_EXPOSURE).abs() < 1e-6);
    }

    #[test]
    fn a_juggernaut_on_the_line_yanks_the_car_not_the_other_way() {
        let s = wave(&["corrupted_apex"]);
        let mut f = field();
        f.muster(1, &s.fighters, Some("corrupted_apex"));
        let start = f.foes[0].pos();
        assert!(f.foes[0].mass > cable::CAR_MASS * 1.5, "a juggernaut should outweigh the car");
        let mut car = Player::driving(start + Vec3::new(0.0, 0.0, -10.0), Vec3::new(0.0, 0.0, -18.0));
        f.hook(car.pos, (start - car.pos).normalize(), car.mass).expect("hook");
        let mut yank = Vec3::ZERO;
        for _ in 0..(120 * 2) {
            for e in f.step(1.0 / 120.0, &car, &flat, &open) {
                if let Event::Yanked { dv, .. } = e {
                    yank += dv;
                    car.vel += dv;
                }
            }
            car.pos += car.vel * (1.0 / 120.0);
        }
        let heavy_moved = (f.foes[0].pos() - start).length();
        assert!(yank.length() > heavy_moved, "the car was not the one yanked: car {:.1} vs body {heavy_moved:.1}", yank.length());
    }

    #[test]
    fn the_whip_flings_the_body_and_it_lands_on_a_neighbour() {
        let s = wave(&["grunt", "grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let p0 = f.foes[0].pos();
        let target = p0 + Vec3::new(12.0, 0.0, 0.0);
        f.foes[1].agent.position = target;
        let mut car = Player::driving(p0 + Vec3::new(0.0, 0.0, -6.0), Vec3::ZERO);
        f.hook(car.pos, (p0 - car.pos).normalize(), car.mass).expect("hook");
        let mut swing: f32 = 0.0;
        for i in 0..(120 * 2) {
            let t = i as f32 / 120.0;
            let omega = 3.0;
            let r = 6.0;
            car.pos = p0 + Vec3::new(r * (omega * t).sin(), 0.0, -r * (omega * t).cos());
            car.vel = Vec3::new(r * omega * (omega * t).cos(), 0.0, r * omega * (omega * t).sin());
            f.step(1.0 / 120.0, &car, &flat, &open);
            if let Some((_, c)) = f.tow.as_ref() {
                swing = f32::max(swing, c.swing_ms);
            } else {
                break;
            }
        }
        assert!(f.towed(0), "the line parted while loading the whip (swing {swing:.1})");
        assert!(swing >= WHIP_MIN_MS, "never loaded a whip: {swing:.1} m/s");
        f.foes[0].vel = Vec3::new(swing, 0.0, 0.0);
        f.release();
        let mut whipped = false;
        let mut thrown: Option<Event> = None;
        for _ in 0..(120 * 6) {
            for e in f.step(1.0 / 120.0, &Player::driving(car.pos, Vec3::ZERO), &flat, &open) {
                match e {
                    Event::Whipped { slot: 0, .. } => whipped = true,
                    Event::Thrown { slot: 0, .. } => thrown = Some(e),
                    _ => {}
                }
            }
            if thrown.is_some() {
                break;
            }
        }
        assert!(whipped, "no whip on release at {swing:.1} m/s");
        match thrown {
            Some(Event::Thrown { into, dmg, .. }) => {
                assert!(dmg > 0.0);
                assert_eq!(into, Some(1), "flew past the neighbour it was aimed at");
            }
            _ => panic!("never landed"),
        }
        assert!(f.foes[0].stagger_t > 0.0, "landed and got straight up");
    }

    #[test]
    fn a_slow_release_is_a_drop_not_a_throw() {
        let s = wave(&["grunt"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        let p0 = f.foes[0].pos();
        let car = Player::driving(p0 + Vec3::new(0.0, 0.0, -6.0), Vec3::ZERO);
        f.hook(car.pos, (p0 - car.pos).normalize(), car.mass).expect("hook");
        f.step(1.0 / 120.0, &car, &flat, &open);
        f.release();
        let ev = f.step(1.0 / 120.0, &car, &flat, &open);
        assert!(!ev.iter().any(|e| matches!(e, Event::Whipped { .. })));
        assert!(ev.iter().any(|e| matches!(e, Event::CableOff { slot: 0 })));
        assert!(f.foes[0].fling.is_none());
        assert!(!f.towed(0));
    }

    #[test]
    fn the_swarm_shoots_from_the_saddle() {
        let s = wave(&["hijack_swarmer"]);
        let mut f = field();
        f.muster(1, &s.fighters, None);
        f.foes[0].agent.state = State::Chase;
        let player = Player::standing(f.foes[0].pos() + Vec3::new(0.0, 0.0, -100.0));
        let mut saddle_shots = 0;
        for _ in 0..(120 * 12) {
            let veh = f.foes[0].form == Form::Vehicle && f.foes[0].fold_t <= 0.0;
            for e in f.step(1.0 / 120.0, &player, &flat, &open) {
                if let Event::Fire { .. } = e {
                    if veh {
                        saddle_shots += 1;
                    }
                }
            }
            if f.foes[0].form == Form::Robot && f.foes[0].fold_t <= 0.0 && saddle_shots > 0 {
                break;
            }
        }
        assert!(saddle_shots > 0, "a swarmer drove the whole way in silence");
    }
}
