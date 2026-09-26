//! A weapon out of the hand: thrown axes that lodge and come back when called,
//! daggers and spears that stay where they land, the chain-knife that the
//! chain yanks home, and the Flying Sword, which comes back when it chooses.
//!
//! Lore it implements (`docs/design/legendary_weapons_and_bestiary_addendum.md`):
//! - **Flying Sword** (a Storm blade): "thrown, it does not return until it
//!   chooses to; recall is a request, not a command." On release it arcs
//!   lightning to the two nearest hostiles, then strikes, then returns to a
//!   *guard position* at its owner's shoulder rather than the hand. From guard
//!   it strikes anything that comes close, and turns a recall down while a
//!   hostile is near.
//! - **Tideward Chainknife** (a Water dagger): "thrown, it returns to hand on
//!   the WaterJet proc rather than needing to be walked back for" — it comes
//!   back the moment it hits, and a miss can be taken back.
//! - Axes are the thrown-and-recalled weapon: they spin end over end, lodge in
//!   what they hit (a dragon included, and ride along in it), and on recall rip
//!   free and fly home on a curve, cutting whatever is in the way.
//!
//! The world is behind the [`Arena`] trait (voxels, creatures, who is hostile),
//! so this module knows nothing about chunks or the roamer list and the whole
//! flight is testable with a mock. The caller turns [`Event`]s into damage,
//! procs, sound and animation.
//!
//! Pure and deterministic: no clock, no RNG (the curve side comes from a seed),
//! allocation only in the events the caller collects. NaN-safe inputs.
//!
//! Not `sim::item_entity`: that is the fixed-point, rollback-safe chassis for
//! autonomous items (per-axis homing in blocks per tick), and nothing in the
//! game loop drives it yet. A thrown weapon needs gravity, spin, a curved
//! return and voxel/body hits at frame rate, so it is its own f32 module; if
//! the sim ever needs thrown weapons to roll back, this is what to port.

use crate::sim::legendary_weapon::{Weapon, WeaponClass};

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn norm_or(a: V3, fallback: V3) -> V3 {
    let l = len(a);
    if l > 1e-6 && l.is_finite() {
        mul(a, 1.0 / l)
    } else {
        fallback
    }
}
fn finite3(a: V3) -> bool {
    a.iter().all(|x| x.is_finite())
}
const UP: V3 = [0.0, 1.0, 0.0];

/// How a thrown weapon gets back to its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Homecoming {
    /// It stays where it lands; walk over to pick it up.
    Stays,
    /// It comes when called (and can always be walked back for).
    OnRecall,
    /// It decides. A recall is a request (the Flying Sword).
    ByChoice,
}

/// How one kind of weapon flies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThrowProfile {
    /// Launch speed (m/s).
    pub speed: f32,
    /// Downward acceleration (m/s²); 0 flies level.
    pub gravity: f32,
    /// End-over-end spin (rad/s); 0 flies point first.
    pub spin: f32,
    /// Seconds before a gravity-free weapon gives up the flight.
    pub max_flight: f32,
    pub homecoming: Homecoming,
    /// Comes straight back after striking (the chain-knife).
    pub return_on_hit: bool,
    /// Speed on the way home (m/s).
    pub return_speed: f32,
    /// Damage of the outgoing blow, × the weapon's base damage.
    pub damage_mult: f32,
    /// Damage of a blow on the way home, × base damage.
    pub return_mult: f32,
    /// Lodges in what it hits (else it strikes and moves on).
    pub lodges: bool,
    /// Lightning links arced to the nearest hostiles on release.
    pub release_arcs: u8,
    /// Reach of those links (m).
    pub arc_range: f32,
}

/// The Flying Sword is a Storm blade.
pub fn is_flying_sword(w: &Weapon) -> bool {
    w.class == WeaponClass::Blade && w.element_idx % 6 == 5
}

/// The chain-knife is a Water dagger.
pub fn is_chainknife(w: &Weapon) -> bool {
    w.class == WeaponClass::Dagger && w.element_idx % 6 == 1
}

/// How `w` flies, or `None` if it is not a throwing weapon (greatswords,
/// greataxes, staves, wands, and every blade that is not the Flying Sword).
pub fn profile(w: &Weapon) -> Option<ThrowProfile> {
    let base = ThrowProfile {
        speed: 22.0,
        gravity: 12.0,
        spin: 13.0,
        max_flight: 6.0,
        homecoming: Homecoming::Stays,
        return_on_hit: false,
        return_speed: 26.0,
        damage_mult: 1.0,
        return_mult: 0.0,
        lodges: true,
        release_arcs: 0,
        arc_range: 0.0,
    };
    match w.class {
        WeaponClass::Axe => Some(ThrowProfile {
            homecoming: Homecoming::OnRecall,
            damage_mult: 1.15,
            return_mult: 0.8,
            ..base
        }),
        WeaponClass::Dagger if is_chainknife(w) => Some(ThrowProfile {
            speed: 28.0,
            gravity: 9.0,
            spin: 20.0,
            homecoming: Homecoming::OnRecall,
            return_on_hit: true,
            return_speed: 32.0,
            damage_mult: 0.9,
            return_mult: 0.3,
            lodges: false,
            ..base
        }),
        WeaponClass::Dagger => Some(ThrowProfile { speed: 28.0, gravity: 9.0, spin: 20.0, damage_mult: 0.9, ..base }),
        WeaponClass::Spear => Some(ThrowProfile { speed: 30.0, gravity: 9.0, spin: 0.0, damage_mult: 1.4, ..base }),
        WeaponClass::Blade if is_flying_sword(w) => Some(ThrowProfile {
            speed: 24.0,
            gravity: 0.0,
            spin: 0.0,
            max_flight: 1.6,
            homecoming: Homecoming::ByChoice,
            return_speed: 18.0,
            damage_mult: 1.0,
            return_mult: 0.0,
            lodges: false,
            release_arcs: 2,
            arc_range: 14.0,
            ..base
        }),
        _ => None,
    }
}

/// What the thrown weapon can sense of the world. The shell implements this
/// over voxels and creatures; tests implement it over a few boxes.
pub trait Arena {
    /// First solid voxel on the segment `a→b`: (fraction along it, surface normal).
    fn solid(&self, a: V3, b: V3) -> Option<(f32, V3)>;
    /// First creature the segment `a→b` passes within `pad` of, skipping
    /// `skip`: (its id, fraction along the segment).
    fn creature(&self, a: V3, b: V3, pad: f32, skip: &[u64]) -> Option<(u64, f32)>;
    /// Where creature `id` is now (its centre), if it still exists.
    fn position_of(&self, id: u64) -> Option<V3>;
    /// Hostile creatures within `r` of `p`, nearest first.
    fn hostiles_near(&self, p: V3, r: f32) -> Vec<(u64, V3)>;
}

/// Where the owner is this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Owner {
    pub hand: V3,
    pub eye: V3,
    pub forward: V3,
    pub right: V3,
}

/// What the weapon is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// In flight, outbound (or dropping).
    Flying,
    /// Stuck: in the terrain (`target: None`) or in a creature, at `offset`
    /// from its centre, riding along.
    Lodged { target: Option<u64>, offset: V3 },
    /// Called home, on a curve `from → ctrl → hand`.
    Returning { from: V3, ctrl: V3, t: f32, dur: f32 },
    /// The Flying Sword standing guard at the owner's shoulder.
    Guarding,
    /// Back in hand; the caller drops this `Thrown`.
    Home,
}

/// Which leg of the flight a blow came on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// Thrown, on the way out (or the sword's own strike).
    Out,
    /// Torn free of the creature it was lodged in, on recall.
    RipOut,
    /// Cutting through something on the way home.
    Home,
}

impl Leg {
    /// Coming back toward the owner (a rip-out or a pass on the way home).
    pub fn returning(self) -> bool {
        self != Leg::Out
    }
}

/// What happened this tick, for the caller to resolve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// It struck creature `target` at `at`, travelling along `dir`, for
    /// `mult` × the weapon's base damage.
    Hit { target: u64, at: V3, dir: V3, mult: f32, leg: Leg },
    /// A lightning link from `from` to creature `target` at `to`.
    Arc { from: V3, target: u64, to: V3 },
    /// It stuck fast — in the ground or in a creature.
    Lodged { at: V3, in_creature: Option<u64> },
    /// It glanced off stone (the sword does not stick).
    Glanced { at: V3 },
    /// The Flying Sword turned a recall down.
    Refused { hostile_near: bool },
    /// The Flying Sword took up its guard.
    Guarding,
    /// It arrived in the owner's hand.
    Caught,
    /// The owner walked over and picked it up.
    PickedUp,
}

/// The answer to a recall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallAnswer {
    Coming,
    /// The Flying Sword will come when it is done.
    Refused,
    /// This weapon has to be walked back for.
    CannotRecall,
    /// Already home / on its way.
    Already,
}

/// Distance at which a lodged weapon is picked up by walking to it (m).
pub const PICKUP_RANGE: f32 = 1.8;
/// Hostiles within this of the owner keep the Flying Sword on guard (m).
pub const GUARD_RADIUS: f32 = 6.0;
/// Seconds between the guarding sword's strikes.
pub const GUARD_COOLDOWN: f32 = 2.2;
/// Longest a gravity weapon may fall before it is simply left where it is.
const MAX_FALL: f32 = 8.0;
/// Integration substep (s).
const SUB: f32 = 1.0 / 120.0;
/// Most distinct creatures one leg of a flight can strike.
const MAX_HITS: usize = 8;

/// A weapon in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Thrown {
    pub weapon: Weapon,
    pub profile: ThrowProfile,
    /// The grip (the model's origin), world space.
    pub pos: V3,
    pub vel: V3,
    /// Grip-to-tip length (m): the head is `length` along the item axis.
    pub length: f32,
    /// Current end-over-end angle (rad).
    pub spin_angle: f32,
    /// The travel direction the orientation follows.
    pub facing: V3,
    /// Seconds in the current phase.
    pub age: f32,
    pub phase: Phase,
    /// The owner asked for it back (the sword remembers the request).
    pub recall_asked: bool,
    /// The sword's chosen quarry.
    pub quarry: Option<u64>,
    pub guard_cd: f32,
    hits: [u64; MAX_HITS],
    hit_n: usize,
    seed: u64,
}

impl Thrown {
    /// Release `weapon` from `from` along `dir`. `None` if it is not a
    /// throwing weapon. Release effects (the sword's lightning) go to `out`.
    pub fn launch(
        weapon: Weapon,
        from: V3,
        dir: V3,
        length: f32,
        seed: u64,
        arena: &impl Arena,
        out: &mut Vec<Event>,
    ) -> Option<Thrown> {
        let profile = profile(&weapon)?;
        if !finite3(from) {
            return None;
        }
        let d = norm_or(dir, [0.0, 0.0, 1.0]);
        let mut t = Thrown {
            weapon,
            profile,
            pos: from,
            vel: mul(d, profile.speed),
            length: if length.is_finite() { length.clamp(0.1, 3.0) } else { 0.8 },
            spin_angle: 0.0,
            facing: d,
            age: 0.0,
            phase: Phase::Flying,
            recall_asked: false,
            quarry: None,
            guard_cd: 0.0,
            hits: [0; MAX_HITS],
            hit_n: 0,
            seed,
        };
        t.release(d, true, arena, out);
        Some(t)
    }

    /// Start (or restart, from guard) an outbound flight along `d`.
    fn release(&mut self, d: V3, arcs: bool, arena: &impl Arena, out: &mut Vec<Event>) {
        self.phase = Phase::Flying;
        self.age = 0.0;
        self.vel = mul(d, self.profile.speed);
        self.facing = d;
        self.hit_n = 0;
        self.quarry = None;
        if self.profile.homecoming == Homecoming::ByChoice {
            // It picks its quarry: the hostile nearest the line it was sent along.
            let mut best: Option<(u64, f32)> = None;
            for (id, p) in arena.hostiles_near(self.pos, self.profile.speed * self.profile.max_flight) {
                let to = sub(p, self.pos);
                let dist = len(to);
                if dist < 1e-3 {
                    continue;
                }
                let c = dot(to, d) / dist;
                if c > 0.94 && best.map_or(true, |(_, bc)| c > bc) {
                    best = Some((id, c));
                }
            }
            self.quarry = best.map(|b| b.0);
        }
        if arcs && self.profile.release_arcs > 0 {
            for (id, p) in arena.hostiles_near(self.pos, self.profile.arc_range).into_iter().take(self.profile.release_arcs as usize) {
                out.push(Event::Arc { from: self.pos, target: id, to: p });
            }
        }
    }

    /// The Flying Sword, from guard, sent along `dir` (no wind-up needed: it is
    /// already in the air). Returns false if it is not guarding.
    pub fn send_from_guard(&mut self, dir: V3, arena: &impl Arena, out: &mut Vec<Event>) -> bool {
        if self.phase != Phase::Guarding {
            return false;
        }
        self.recall_asked = false;
        let d = norm_or(dir, self.facing);
        self.release(d, true, arena, out);
        true
    }

    /// The owner calls it back.
    pub fn recall(&mut self, owner: &Owner, arena: &impl Arena, out: &mut Vec<Event>) -> RecallAnswer {
        match self.phase {
            Phase::Home | Phase::Returning { .. } => return RecallAnswer::Already,
            _ => {}
        }
        match self.profile.homecoming {
            Homecoming::Stays => RecallAnswer::CannotRecall,
            Homecoming::ByChoice => {
                self.recall_asked = true;
                let near = !arena.hostiles_near(owner.eye, GUARD_RADIUS).is_empty();
                if self.phase == Phase::Guarding && !near {
                    self.start_return(owner, arena, out);
                    RecallAnswer::Coming
                } else {
                    out.push(Event::Refused { hostile_near: near });
                    RecallAnswer::Refused
                }
            }
            Homecoming::OnRecall => {
                self.start_return(owner, arena, out);
                RecallAnswer::Coming
            }
        }
    }

    fn start_return(&mut self, owner: &Owner, arena: &impl Arena, out: &mut Vec<Event>) {
        // Ripped out of a creature: that hurts on the way out -- and that is
        // its one blow on this leg (the path home starts inside it).
        let ripped_from = if let Phase::Lodged { target: Some(id), .. } = self.phase { Some(id) } else { None };
        if let Phase::Lodged { target: Some(id), .. } = self.phase {
            if self.profile.return_mult > 0.0 {
                let dir = norm_or(sub(owner.hand, self.pos), UP);
                out.push(Event::Hit { target: id, at: self.tip(), dir, mult: self.profile.return_mult * 0.4, leg: Leg::RipOut });
            }
            let _ = arena;
        }
        let from = self.pos;
        let to = owner.hand;
        let d = len(sub(to, from));
        // Curve in from one side and a little above: it reads as a weapon
        // answering, not a projectile rewinding. The side is fixed per throw.
        let side = if self.seed & 1 == 0 { 1.0 } else { -1.0 };
        let right = norm_or(cross(norm_or(sub(to, from), [0.0, 0.0, 1.0]), UP), owner.right);
        let mid = mul(add(from, to), 0.5);
        let ctrl = add(add(mid, mul(right, side * d * 0.35)), mul(UP, d * 0.2));
        let dur = (d / self.profile.return_speed.max(1.0)).clamp(0.2, 2.5);
        self.phase = Phase::Returning { from, ctrl, t: 0.0, dur };
        self.age = 0.0;
        self.hit_n = 0;
        if let Some(id) = ripped_from {
            self.note_hit(id);
        }
    }

    /// Where the head/tip is (for hits and sparks).
    pub fn tip(&self) -> V3 {
        add(self.pos, mul(self.basis()[1], self.length))
    }

    /// Orientation as columns (item +X thickness, +Y along the item toward
    /// the head, +Z the edge), matching `item_model`'s grip-local frame.
    pub fn basis(&self) -> [V3; 3] {
        let f = norm_or(self.facing, [0.0, 0.0, 1.0]);
        let side = norm_or(cross(f, UP), [1.0, 0.0, 0.0]);
        let up2 = cross(side, f);
        match self.phase {
            Phase::Guarding => {
                // Tip up, edge toward where it faces.
                let z = norm_or([f[0], 0.0, f[2]], [0.0, 0.0, 1.0]);
                let x = norm_or(cross(UP, z), side);
                [x, UP, z]
            }
            _ if self.profile.spin == 0.0 => [side, f, up2],
            _ => {
                let (s, c) = self.spin_angle.sin_cos();
                let y = add(mul(up2, c), mul(f, s));
                let z = sub(mul(f, c), mul(up2, s));
                [side, y, z]
            }
        }
    }

    fn note_hit(&mut self, id: u64) {
        if self.hit_n < MAX_HITS {
            self.hits[self.hit_n] = id;
            self.hit_n += 1;
        }
    }

    /// Advance `dt` seconds.
    pub fn step(&mut self, dt: f32, owner: &Owner, arena: &impl Arena, out: &mut Vec<Event>) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        self.guard_cd = (self.guard_cd - dt).max(0.0);
        let mut left = dt;
        while left > 1e-6 && self.phase != Phase::Home {
            let h = left.min(SUB);
            left -= h;
            self.substep(h, owner, arena, out);
        }
    }

    fn substep(&mut self, h: f32, owner: &Owner, arena: &impl Arena, out: &mut Vec<Event>) {
        self.age += h;
        match self.phase {
            Phase::Home => {}
            Phase::Flying => self.fly(h, owner, arena, out),
            Phase::Lodged { target, offset } => {
                if let Some(id) = target {
                    match arena.position_of(id) {
                        Some(c) => self.pos = add(c, offset),
                        None => {
                            // What it was stuck in is gone: it drops.
                            self.phase = Phase::Flying;
                            self.vel = [0.0, 0.0, 0.0];
                            self.age = 0.0;
                            return;
                        }
                    }
                }
                let to = sub(owner.hand, self.pos);
                if target.is_none() && len(to) < PICKUP_RANGE {
                    self.phase = Phase::Home;
                    out.push(Event::PickedUp);
                }
            }
            Phase::Returning { from, ctrl, t, dur } => {
                let t2 = (t + h / dur.max(1e-3)).min(1.0);
                let u = 1.0 - t2;
                let p = add(add(mul(from, u * u), mul(ctrl, 2.0 * u * t2)), mul(owner.hand, t2 * t2));
                let seg = sub(p, self.pos);
                if len(seg) > 1e-5 {
                    self.facing = norm_or(seg, self.facing);
                    let skip: Vec<u64> = self.hits[..self.hit_n].to_vec();
                    if let Some((id, f)) = arena.creature(self.pos, p, 0.35, &skip) {
                        if self.profile.return_mult > 0.0 {
                            let at = add(self.pos, mul(seg, f));
                            out.push(Event::Hit { target: id, at, dir: self.facing, mult: self.profile.return_mult, leg: Leg::Home });
                        }
                        self.note_hit(id);
                    }
                }
                self.pos = p;
                // It spins backwards on the way home.
                self.spin_angle -= self.profile.spin * h;
                if t2 >= 1.0 {
                    self.phase = Phase::Home;
                    self.vel = [0.0, 0.0, 0.0];
                    out.push(Event::Caught);
                } else {
                    self.phase = Phase::Returning { from, ctrl, t: t2, dur };
                }
            }
            Phase::Guarding => self.guard(h, owner, arena, out),
        }
    }

    fn fly(&mut self, h: f32, owner: &Owner, arena: &impl Arena, out: &mut Vec<Event>) {
        // The sword steers toward its quarry.
        if let Some(q) = self.quarry.and_then(|id| arena.position_of(id)) {
            let want = norm_or(sub(q, self.pos), self.facing);
            let cur = norm_or(self.vel, want);
            let turn = (3.5 * h).min(1.0);
            let d = norm_or(add(mul(cur, 1.0 - turn), mul(want, turn)), cur);
            self.vel = mul(d, self.profile.speed);
        }
        self.vel[1] -= self.profile.gravity * h;
        let next = add(self.pos, mul(self.vel, h));
        self.facing = norm_or(self.vel, self.facing);
        self.spin_angle += self.profile.spin * h;
        // Test from the grip to where the head will be: the head is what hits.
        let reach = mul(self.facing, self.length * 0.6);
        let (a, b) = (self.pos, add(next, reach));
        let skip: Vec<u64> = self.hits[..self.hit_n].to_vec();
        let wall = arena.solid(a, b);
        let body = arena.creature(a, b, 0.3, &skip);
        let wall_first = match (wall, body) {
            (Some((fw, _)), Some((_, fc))) => fw < fc,
            (Some(_), None) => true,
            _ => false,
        };
        if let (Some((id, f)), false) = (body, wall_first) {
            let at = add(a, mul(sub(b, a), f));
            out.push(Event::Hit { target: id, at, dir: self.facing, mult: self.profile.damage_mult, leg: Leg::Out });
            self.note_hit(id);
            if self.profile.return_on_hit {
                self.start_return(owner, arena, out);
            } else if self.profile.lodges {
                // Buried head-first: the item axis along the flight.
                self.spin_angle = std::f32::consts::FRAC_PI_2;
                let centre = arena.position_of(id).unwrap_or(at);
                let grip = sub(at, mul(self.facing, self.length * 0.8));
                self.pos = grip;
                self.phase = Phase::Lodged { target: Some(id), offset: sub(grip, centre) };
                self.vel = [0.0, 0.0, 0.0];
                self.age = 0.0;
                out.push(Event::Lodged { at, in_creature: Some(id) });
            } else {
                // The sword strikes and chooses: back to guard.
                self.take_guard(out);
            }
            return;
        }
        if let (Some((f, _n)), true) = (wall, wall_first) {
            let at = add(a, mul(sub(b, a), f));
            if self.profile.lodges {
                self.spin_angle = std::f32::consts::FRAC_PI_2;
                self.pos = sub(at, mul(self.facing, self.length * 0.8));
                self.phase = Phase::Lodged { target: None, offset: [0.0, 0.0, 0.0] };
                self.vel = [0.0, 0.0, 0.0];
                self.age = 0.0;
                out.push(Event::Lodged { at, in_creature: None });
            } else if self.profile.homecoming == Homecoming::ByChoice {
                out.push(Event::Glanced { at });
                self.take_guard(out);
            } else {
                // The chain-knife missed: it drops where it struck, and the
                // chain can take the mistake back.
                self.pos = sub(at, mul(self.facing, self.length * 0.8));
                self.spin_angle = std::f32::consts::FRAC_PI_2;
                self.phase = Phase::Lodged { target: None, offset: [0.0, 0.0, 0.0] };
                self.vel = [0.0, 0.0, 0.0];
                self.age = 0.0;
                out.push(Event::Lodged { at, in_creature: None });
            }
            return;
        }
        self.pos = next;
        let give_up = if self.profile.gravity == 0.0 { self.profile.max_flight } else { MAX_FALL };
        if self.age > give_up {
            if self.profile.homecoming == Homecoming::ByChoice {
                self.take_guard(out);
            } else {
                // Fell out of anything to hit: it stays where it is.
                self.phase = Phase::Lodged { target: None, offset: [0.0, 0.0, 0.0] };
                self.vel = [0.0, 0.0, 0.0];
                self.age = 0.0;
                out.push(Event::Lodged { at: self.pos, in_creature: None });
            }
        }
    }

    fn take_guard(&mut self, out: &mut Vec<Event>) {
        self.phase = Phase::Guarding;
        self.vel = [0.0, 0.0, 0.0];
        self.age = 0.0;
        self.quarry = None;
        self.guard_cd = self.guard_cd.max(0.6);
        out.push(Event::Guarding);
    }

    /// The guard station at the owner's shoulder, with a slow bob.
    pub fn guard_station(&self, owner: &Owner) -> V3 {
        let bob = (self.age * 2.1).sin() * 0.08;
        let back = norm_or([owner.forward[0], 0.0, owner.forward[2]], [0.0, 0.0, 1.0]);
        add(add(add(owner.eye, mul(owner.right, 0.75)), [0.0, 0.25 + bob, 0.0]), mul(back, -0.35))
    }

    fn guard(&mut self, h: f32, owner: &Owner, arena: &impl Arena, out: &mut Vec<Event>) {
        let station = self.guard_station(owner);
        let to = sub(station, self.pos);
        let d = len(to);
        let step = (self.profile.return_speed * h).min(d);
        // Glide there, easing in for the last metre.
        let ease = if d < 1.0 { (d * 8.0 * h).min(d) } else { step };
        self.pos = add(self.pos, mul(norm_or(to, [0.0; 3]), ease.max(0.0)));
        self.facing = norm_or([owner.forward[0], 0.0, owner.forward[2]], self.facing);
        let near = arena.hostiles_near(owner.eye, GUARD_RADIUS);
        if let Some(&(id, p)) = near.first() {
            if self.guard_cd <= 0.0 && d < 1.5 {
                // It strikes what comes close, and comes back to guard.
                self.guard_cd = GUARD_COOLDOWN;
                let dir = norm_or(sub(p, self.pos), self.facing);
                self.release(dir, false, arena, out);
                self.quarry = Some(id);
            }
            return;
        }
        // Nothing near and it was asked home: now it chooses to come.
        if self.recall_asked && self.age > 0.4 {
            self.start_return(owner, arena, out);
        }
    }

    /// True while the weapon is not in the owner's hand.
    pub fn out(&self) -> bool {
        self.phase != Phase::Home
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::legendary_weapon::forge;

    /// A flat floor at y=0, an optional wall at x = `wall_x`, and spheres.
    struct Mock {
        wall_x: Option<f32>,
        beasts: Vec<(u64, V3, f32, bool)>, // id, centre, radius, hostile
    }

    impl Arena for Mock {
        fn solid(&self, a: V3, b: V3) -> Option<(f32, V3)> {
            let mut best: Option<(f32, V3)> = None;
            if a[1] > 0.0 && b[1] <= 0.0 {
                best = Some((a[1] / (a[1] - b[1]), UP));
            }
            if let Some(w) = self.wall_x {
                if a[0] < w && b[0] >= w {
                    let f = (w - a[0]) / (b[0] - a[0]);
                    if best.map_or(true, |(bf, _)| f < bf) {
                        best = Some((f, [-1.0, 0.0, 0.0]));
                    }
                }
            }
            best
        }
        fn creature(&self, a: V3, b: V3, pad: f32, skip: &[u64]) -> Option<(u64, f32)> {
            let ab = sub(b, a);
            let l2 = dot(ab, ab).max(1e-9);
            let mut best: Option<(u64, f32)> = None;
            for &(id, c, r, _) in &self.beasts {
                if skip.contains(&id) {
                    continue;
                }
                let f = (dot(sub(c, a), ab) / l2).clamp(0.0, 1.0);
                let p = add(a, mul(ab, f));
                if len(sub(p, c)) <= r + pad && best.map_or(true, |(_, bf)| f < bf) {
                    best = Some((id, f));
                }
            }
            best
        }
        fn position_of(&self, id: u64) -> Option<V3> {
            self.beasts.iter().find(|b| b.0 == id).map(|b| b.1)
        }
        fn hostiles_near(&self, p: V3, r: f32) -> Vec<(u64, V3)> {
            let mut v: Vec<(u64, V3, f32)> = self
                .beasts
                .iter()
                .filter(|b| b.3)
                .map(|b| (b.0, b.1, len(sub(b.1, p))))
                .filter(|b| b.2 <= r)
                .collect();
            v.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
            v.into_iter().map(|b| (b.0, b.1)).collect()
        }
    }

    fn owner() -> Owner {
        Owner { hand: [0.0, 1.4, 0.0], eye: [0.0, 1.6, 0.0], forward: [1.0, 0.0, 0.0], right: [0.0, 0.0, 1.0] }
    }
    fn weapon(class: WeaponClass, el: u8) -> Weapon {
        forge(7, class, el, 1)
    }
    fn run(t: &mut Thrown, o: &Owner, a: &Mock, secs: f32) -> Vec<Event> {
        let mut ev = Vec::new();
        let mut s = 0.0;
        while s < secs && t.out() {
            t.step(1.0 / 60.0, o, a, &mut ev);
            s += 1.0 / 60.0;
        }
        ev
    }

    #[test]
    fn thrown_only_throwing_weapons_fly() {
        for c in [WeaponClass::Greatsword, WeaponClass::Greataxe, WeaponClass::Staff, WeaponClass::Wand] {
            assert!(profile(&weapon(c, 5)).is_none(), "{c:?}");
        }
        assert!(profile(&weapon(WeaponClass::Blade, 4)).is_none(), "only the Storm blade flies");
        assert!(profile(&weapon(WeaponClass::Blade, 5)).is_some());
        assert!(profile(&weapon(WeaponClass::Axe, 0)).is_some());
        assert!(profile(&weapon(WeaponClass::Spear, 3)).is_some());
    }

    #[test]
    fn thrown_axe_arcs_down_and_lodges_in_the_ground() {
        let a = Mock { wall_x: None, beasts: vec![] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Axe, 3), [0.0, 1.4, 0.0], [1.0, 0.1, 0.0], 0.8, 1, &a, &mut ev).unwrap();
        let ev = run(&mut t, &owner(), &a, 4.0);
        assert!(matches!(t.phase, Phase::Lodged { target: None, .. }), "{:?}", t.phase);
        let at = ev.iter().find_map(|e| if let Event::Lodged { at, .. } = e { Some(*at) } else { None }).unwrap();
        assert!(at[1].abs() < 0.05, "stuck in the floor: {at:?}");
        assert!(at[0] > 5.0 && at[0] < 25.0, "a real throw's distance: {at:?}");
        assert!(t.spin_angle != 0.0);
    }

    #[test]
    fn thrown_axe_lodges_in_a_beast_rides_along_and_recall_rips_it_out() {
        let mut a = Mock { wall_x: None, beasts: vec![(42, [8.0, 1.5, 0.0], 1.0, true)] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Axe, 3), [0.0, 1.5, 0.0], [1.0, 0.05, 0.0], 0.8, 2, &a, &mut ev).unwrap();
        let ev = run(&mut t, &owner(), &a, 2.0);
        assert!(ev.iter().any(|e| matches!(e, Event::Hit { target: 42, leg: Leg::Out, .. })));
        assert!(matches!(t.phase, Phase::Lodged { target: Some(42), .. }));
        // The beast moves; the axe goes with it.
        let before = t.pos;
        a.beasts[0].1 = [10.0, 1.5, 3.0];
        let mut ev = Vec::new();
        t.step(1.0 / 60.0, &owner(), &a, &mut ev);
        assert!((t.pos[2] - before[2] - 3.0).abs() < 1e-3 && (t.pos[0] - before[0] - 2.0).abs() < 1e-3);
        // Sunk deep (the grip inside the body, as in a big dragon).
        t.phase = Phase::Lodged { target: Some(42), offset: [0.0; 3] };
        t.step(1.0 / 60.0, &owner(), &a, &mut ev);
        // Recall: ripped out (a returning hit), home, caught.
        assert_eq!(t.recall(&owner(), &a, &mut ev), RecallAnswer::Coming);
        assert!(ev.iter().any(|e| matches!(e, Event::Hit { target: 42, leg: Leg::RipOut, .. })), "ripping it out hurts");
        let ev = run(&mut t, &owner(), &a, 3.0);
        assert!(
            !ev.iter().any(|e| matches!(e, Event::Hit { target: 42, .. })),
            "the rip-out is its one blow: the way home starts inside it, and does not cut it again"
        );
        assert_eq!(t.phase, Phase::Home);
        assert!(ev.contains(&Event::Caught));
    }

    #[test]
    fn thrown_recalled_axe_cuts_what_is_in_its_way_once() {
        // Lodged in the ground far out; a beast stands between it and the owner.
        // A dragon-sized body across the curve home (the curve bows ~0.35·d
        // sideways, so a small one on the straight line would be missed).
        let a = Mock { wall_x: None, beasts: vec![(7, [7.0, 1.4, 0.0], 2.5, true)] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Axe, 0), [0.0, 1.4, 0.0], [1.0, 0.0, 0.0], 0.8, 4, &a, &mut ev).unwrap();
        t.phase = Phase::Lodged { target: None, offset: [0.0; 3] };
        t.pos = [14.0, 0.2, 0.0];
        t.recall(&owner(), &a, &mut ev);
        let ev = run(&mut t, &owner(), &a, 3.0);
        let hits = ev.iter().filter(|e| matches!(e, Event::Hit { target: 7, leg: Leg::Home, .. })).count();
        assert_eq!(hits, 1, "struck on the way home, exactly once: {ev:?}");
        assert_eq!(t.phase, Phase::Home);
    }

    #[test]
    fn thrown_a_wall_shields_what_stands_behind_it() {
        let a = Mock { wall_x: Some(5.0), beasts: vec![(6, [5.5, 1.15, 0.0], 0.15, true)] }; // just behind it
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Axe, 3), [0.0, 1.4, 0.0], [1.0, 0.0, 0.0], 0.8, 1, &a, &mut ev).unwrap();
        let ev = run(&mut t, &owner(), &a, 2.0);
        assert!(!ev.iter().any(|e| matches!(e, Event::Hit { .. })), "{ev:?}");
        assert!(matches!(t.phase, Phase::Lodged { target: None, .. }));
    }

    #[test]
    fn thrown_dagger_must_be_walked_back_for() {
        let a = Mock { wall_x: Some(6.0), beasts: vec![] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Dagger, 4), [0.0, 1.4, 0.0], [1.0, 0.0, 0.0], 0.4, 3, &a, &mut ev).unwrap();
        run(&mut t, &owner(), &a, 1.0);
        assert!(matches!(t.phase, Phase::Lodged { target: None, .. }), "stuck in the wall");
        assert!(t.pos[0] < 6.0);
        assert_eq!(t.recall(&owner(), &a, &mut ev), RecallAnswer::CannotRecall);
        let mut near = owner();
        near.hand = [5.0, 1.4, 0.0];
        let ev = run(&mut t, &near, &a, 0.1);
        assert!(ev.contains(&Event::PickedUp));
        assert_eq!(t.phase, Phase::Home);
    }

    #[test]
    fn thrown_chainknife_comes_back_the_moment_it_hits() {
        let a = Mock { wall_x: None, beasts: vec![(9, [7.0, 1.4, 0.0], 0.8, true)] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Dagger, 1), [0.0, 1.4, 0.0], [1.0, 0.0, 0.0], 0.4, 3, &a, &mut ev).unwrap();
        let ev = run(&mut t, &owner(), &a, 3.0);
        assert!(ev.iter().any(|e| matches!(e, Event::Hit { target: 9, leg: Leg::Out, .. })));
        assert!(!ev.iter().any(|e| matches!(e, Event::Lodged { .. })), "never sticks in what it hits");
        assert!(!ev.iter().any(|e| matches!(e, Event::Hit { leg: Leg::RipOut, .. })), "so nothing is ever ripped out");
        assert_eq!(t.phase, Phase::Home);
        assert!(ev.contains(&Event::Caught));
    }

    #[test]
    fn thrown_spear_flies_point_first() {
        let a = Mock { wall_x: None, beasts: vec![] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Spear, 2), [0.0, 1.4, 0.0], [1.0, 0.3, 0.0], 1.6, 3, &a, &mut ev).unwrap();
        for _ in 0..30 {
            t.step(1.0 / 60.0, &owner(), &a, &mut ev);
        }
        let axis = t.basis()[1];
        let v = norm_or(t.vel, [1.0, 0.0, 0.0]);
        assert!(dot(axis, v) > 0.999, "shaft along the flight");
        assert!(t.vel[1] < 0.3 * 30.0, "gravity bends it");
    }

    #[test]
    fn thrown_flying_sword_arcs_on_release_to_the_two_nearest_hostiles() {
        let a = Mock {
            wall_x: None,
            beasts: vec![
                (1, [5.0, 1.5, 4.0], 0.8, true),
                (2, [3.0, 1.5, -3.0], 0.8, true),
                (3, [9.0, 1.5, 9.0], 0.8, true),  // third nearest: no link
                (4, [2.0, 1.5, 2.0], 0.8, false), // not hostile
                (5, [30.0, 1.5, 0.0], 0.8, true), // out of range
            ],
        };
        let mut ev = Vec::new();
        let t = Thrown::launch(weapon(WeaponClass::Blade, 5), [0.0, 1.5, 0.0], [0.0, 0.0, 1.0], 0.8, 5, &a, &mut ev).unwrap();
        let linked: Vec<u64> = ev.iter().filter_map(|e| if let Event::Arc { target, .. } = e { Some(*target) } else { None }).collect();
        assert_eq!(linked, vec![2, 1]);
        assert_eq!(t.vel[1], 0.0);
    }

    #[test]
    fn thrown_flying_sword_refuses_while_out_then_guards_then_chooses_to_come() {
        let mut a = Mock { wall_x: None, beasts: vec![(8, [12.0, 1.6, 0.0], 1.0, true)] };
        let o = owner();
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Blade, 5), [0.0, 1.6, 0.0], [1.0, 0.0, 0.0], 0.8, 5, &a, &mut ev).unwrap();
        assert_eq!(t.quarry, Some(8), "it picked the hostile on its line");
        assert_eq!(t.recall(&o, &a, &mut ev), RecallAnswer::Refused, "a request, not a command");
        let mut ev = Vec::new();
        for _ in 0..120 {
            t.step(1.0 / 60.0, &o, &a, &mut ev);
            if t.phase == Phase::Guarding {
                break;
            }
        }
        assert!(ev.iter().any(|e| matches!(e, Event::Hit { target: 8, .. })));
        assert!(ev.contains(&Event::Guarding));
        // The beast charges the owner: the sword stays on guard and strikes it.
        a.beasts[0].1 = [3.0, 1.6, 0.0];
        let ev = run(&mut t, &o, &a, 3.0);
        assert!(ev.iter().any(|e| matches!(e, Event::Hit { target: 8, .. })), "guard strike: {ev:?}");
        assert!(t.out(), "it will not come home with a hostile this close");
        let mut ev = Vec::new();
        assert_eq!(t.recall(&o, &a, &mut ev), RecallAnswer::Refused);
        // The beast leaves: the remembered request is honoured.
        a.beasts.clear();
        let ev = run(&mut t, &o, &a, 4.0);
        assert!(ev.contains(&Event::Caught), "{ev:?}");
    }

    #[test]
    fn thrown_flying_sword_glances_off_stone() {
        let a = Mock { wall_x: Some(5.0), beasts: vec![] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Blade, 5), [0.0, 1.6, 0.0], [1.0, 0.0, 0.0], 0.8, 5, &a, &mut ev).unwrap();
        let ev = run(&mut t, &owner(), &a, 1.0);
        assert!(ev.iter().any(|e| matches!(e, Event::Glanced { .. })));
        assert_eq!(t.phase, Phase::Guarding);
    }

    #[test]
    fn thrown_lodged_in_something_that_vanishes_drops() {
        let mut a = Mock { wall_x: None, beasts: vec![(3, [6.0, 3.0, 0.0], 1.0, true)] };
        let mut ev = Vec::new();
        let mut t = Thrown::launch(weapon(WeaponClass::Axe, 3), [0.0, 3.0, 0.0], [1.0, 0.1, 0.0], 0.8, 2, &a, &mut ev).unwrap();
        run(&mut t, &owner(), &a, 1.0);
        assert!(matches!(t.phase, Phase::Lodged { target: Some(3), .. }));
        a.beasts.clear();
        let ev = run(&mut t, &owner(), &a, 3.0);
        assert!(matches!(t.phase, Phase::Lodged { target: None, .. }), "fell to the ground");
        assert!(ev.iter().any(|e| matches!(e, Event::Lodged { in_creature: None, .. })));
    }

    #[test]
    fn thrown_is_deterministic_and_nan_safe() {
        let a = Mock { wall_x: None, beasts: vec![(1, [8.0, 1.5, 0.5], 1.0, true)] };
        let go = || {
            let mut ev = Vec::new();
            let mut t = Thrown::launch(weapon(WeaponClass::Axe, 2), [0.0, 1.5, 0.0], [1.0, 0.1, 0.05], 0.8, 9, &a, &mut ev).unwrap();
            ev.extend(run(&mut t, &owner(), &a, 2.0));
            (t, ev)
        };
        assert_eq!(go(), go());
        let mut ev = Vec::new();
        assert!(Thrown::launch(weapon(WeaponClass::Axe, 2), [f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], 0.8, 1, &a, &mut ev).is_none());
        let mut t = Thrown::launch(weapon(WeaponClass::Axe, 2), [0.0, 1.5, 0.0], [f32::NAN, 0.0, 0.0], f32::NAN, 1, &a, &mut ev).unwrap();
        t.step(f32::NAN, &owner(), &a, &mut ev);
        t.step(0.1, &owner(), &a, &mut ev);
        assert!(finite3(t.pos));
    }
}
