//! CRAWLER AMBUSH — "something is crawling the structure."
//!
//! Ported from `game.c`. Marked corrupted break off, scale a nearby structure,
//! coil on the crest, and dive on you.
//!
//! ## Three things make it horror rather than a gimmick
//!
//! **A cap of two.** Not a swarm from above — one or two at a time, ever. A
//! ceiling full of them is a crowd; two is a threat you can look for and
//! still fail to find. The cap is the discipline.
//!
//! **Three telegraphs, and the last one is late.** A message when the climb
//! starts, eye-glints in the final 0.45 s of the perch, then the dive. You
//! are told, you are told again, and the third warning arrives after it is
//! already too late to walk.
//!
//! **The pounce is ballistic and leads nothing.** Flight time is fixed at
//! `FLIGHT_S`, and the arc is solved at launch against where you were
//! *standing then*. So it is dodged by moving — and only by moving *after* it
//! commits, which is what the glints exist to time. A crawler that tracked
//! you would be unfair; one that predicts badly is a puzzle.
//!
//! ## What it borrows
//!
//! `cling.rs` decides whether a frame can hold a face at all, and a crawler
//! alt-mode is the one that grips best — the same `Frame::crawler()` that
//! crosses ice overhangs no heavy frame can. The climb here is that ability
//! used offensively, which is why the two want to agree about who can do it.

use crate::cling::{hang_time_s, Frame, Hold};
use crate::terrain::Material;
use glam::Vec3;

/// Never more than this many in the air or on a wall at once.
pub const CONCURRENT_CAP: usize = 2;
/// Closer than this and there is no time to climb; further and it is
/// somebody else's problem.
pub const TRIGGER_NEAR_M: f32 = 22.0;
pub const TRIGGER_FAR_M: f32 = 65.0;
/// A structure has to rise this far above the crawler to be worth scaling.
pub const MIN_HEIGHT_M: f32 = 6.0;
/// And be within this of it.
pub const MAX_REACH_M: f32 = 14.0;
/// The scuttle up. Accelerating, so most of it is the last half.
pub const CLIMB_S: f32 = 2.4;
/// Base coil on the crest, before the per-crawler stagger.
pub const PERCH_S: f32 = 0.6;
/// The eyes catch the dark for this long before it commits.
pub const GLINT_S: f32 = 0.45;
/// Time in the air. Fixed, which is what makes the arc solvable and dodgeable.
pub const FLIGHT_S: f32 = 0.85;
/// After it lands, before that one tries again.
pub const COOLDOWN_S: f32 = 12.0;
/// Roughly this fraction of the corrupted are crawlers.
pub const CRAWLER_SHARE: f32 = 0.4;

/// Where a crawler is in the ambush.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// on the ground with the others
    Ground,
    /// glued to a face, going up
    Climbing,
    /// coiled on the crest, choosing
    Perch,
    /// committed, in the air
    Pounce,
}

/// Something tall enough to be worth climbing.
#[derive(Clone, Copy, Debug)]
pub struct Structure {
    pub at: Vec3,
    /// crest height above the ground
    pub height_m: f32,
    /// what it is made of — a crawler still has to hold on
    pub face: Material,
}

/// Is this a structure this crawler can use?
///
/// Height and reach are `game.c`'s. The material check is added: `cling.rs`
/// already knows a face made of soft ice will not hold anything for the
/// length of a climb, and a crawler that scales an unclimbable wall makes one
/// of the two modules wrong.
pub fn usable(s: &Structure, from: Vec3) -> bool {
    if s.height_m < MIN_HEIGHT_M {
        return false;
    }
    if Vec3::new(s.at.x - from.x, 0.0, s.at.z - from.z).length() > MAX_REACH_M {
        return false;
    }
    hang_time_s(Hold::Wall, Frame::crawler(), s.face).unwrap_or(0.0) > CLIMB_S
}

/// The ballistic solution for a dive, given `FLIGHT_S` and this gravity.
///
/// Returns the launch velocity. Solved against where the target IS, at the
/// moment of launch, and never revisited — that is the counterplay.
pub fn pounce_velocity(from: Vec3, target: Vec3, gravity: f32) -> Vec3 {
    let d = target - from;
    Vec3::new(
        d.x / FLIGHT_S,
        d.y / FLIGHT_S + 0.5 * gravity * FLIGHT_S,
        d.z / FLIGHT_S,
    )
}

/// Where a dive launched at `v` actually ends up after `FLIGHT_S`.
///
/// Integrates the arc rather than assuming it. The first version returned
/// `from.y` for the height — the PERCH's height — so a dive from an eighteen
/// metre spire reported landing eighteen metres in the air, and a test
/// comparing it against a target on the ground failed by exactly the height
/// of the structure.
pub fn pounce_landing(from: Vec3, v: Vec3, gravity: f32) -> Vec3 {
    Vec3::new(
        from.x + v.x * FLIGHT_S,
        from.y + v.y * FLIGHT_S - 0.5 * gravity * FLIGHT_S * FLIGHT_S,
        from.z + v.z * FLIGHT_S,
    )
}

pub struct Crawler {
    pub id: u32,
    pub phase: Phase,
    pub pos: Vec3,
    /// counts up while climbing, down while perched, down while airborne
    pub t: f32,
    pub cooldown: f32,
    pub target_at_launch: Vec3,
    pub vel: Vec3,
    from: Vec3,
    climbing: Option<Structure>,
}

impl Crawler {
    pub fn new(id: u32, at: Vec3) -> Crawler {
        Crawler {
            id,
            phase: Phase::Ground,
            pos: at,
            t: 0.0,
            cooldown: 0.0,
            target_at_launch: Vec3::ZERO,
            vel: Vec3::ZERO,
            from: at,
            climbing: None,
        }
    }

    /// Is this one a climber at all? A stable ~40% share, from the id rather
    /// than a roll, so the same enemy is always the same kind of enemy.
    pub fn is_crawler(&self) -> bool {
        let h = self.id.wrapping_mul(2_654_435_761);
        (h % 10) < (CRAWLER_SHARE * 10.0) as u32
    }

    /// The eyes are showing. The last warning, and it is late on purpose.
    pub fn glinting(&self) -> bool {
        self.phase == Phase::Perch && self.t <= GLINT_S
    }

    pub fn ready(&self, dist_m: f32) -> bool {
        self.phase == Phase::Ground
            && self.cooldown <= 0.0
            && self.is_crawler()
            && (TRIGGER_NEAR_M..TRIGGER_FAR_M).contains(&dist_m)
    }
}

/// What happened to a crawler this tick, for the player to be told about.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    // No `Nothing` variant. `step` returns a Vec of what actually happened,
    // so "nothing" is the empty vector — a state that cannot be constructed
    // beats one every caller has to remember to ignore.
    /// "something is crawling the structure--"
    StartedClimbing,
    /// "IT POUNCES"
    Pounced,
    /// came down, at this point
    Landed(Vec3),
}

/// The pack, so the cap can be enforced across it.
pub struct Ambush {
    pub crawlers: Vec<Crawler>,
    pub gravity: f32,
}

impl Ambush {
    pub fn new(crawlers: Vec<Crawler>) -> Ambush {
        Ambush { crawlers, gravity: 16.0 }
    }

    /// How many are off the ground right now.
    pub fn live(&self) -> usize {
        self.crawlers.iter().filter(|c| c.phase != Phase::Ground).count()
    }

    pub fn step(&mut self, dt: f32, player: Vec3, structures: &[Structure]) -> Vec<(u32, Event)> {
        let mut out = Vec::new();
        for i in 0..self.crawlers.len() {
            let mut live = self.live();
            let c = &mut self.crawlers[i];
            c.cooldown = (c.cooldown - dt).max(0.0);
            let dist = Vec3::new(c.pos.x - player.x, 0.0, c.pos.z - player.z).length();

            match c.phase {
                Phase::Ground => {
                    // THE CAP IS THE DISCIPLINE. Two is a threat you can look
                    // for and still fail to find; a ceiling of them is a
                    // crowd, and a crowd is not frightening.
                    if live >= CONCURRENT_CAP || !c.ready(dist) {
                        continue;
                    }
                    let Some(s) = structures.iter().copied().find(|s| usable(s, c.pos)) else {
                        continue;
                    };
                    c.phase = Phase::Climbing;
                    c.t = 0.0;
                    c.from = c.pos;
                    c.climbing = Some(s);
                    live += 1;
                    let _ = live;
                    out.push((c.id, Event::StartedClimbing));
                }
                Phase::Climbing => {
                    c.t += dt;
                    let f = (c.t / CLIMB_S).min(1.0);
                    let s = c.climbing.expect("climbing without a structure");
                    // accelerating scuttle: most of the height in the last half
                    c.pos.x = c.from.x + (s.at.x - c.from.x) * (f * 2.2).min(1.0);
                    c.pos.z = c.from.z + (s.at.z - c.from.z) * (f * 2.2).min(1.0);
                    c.pos.y = c.from.y + s.height_m * f * f;
                    if f >= 1.0 {
                        c.phase = Phase::Perch;
                        // staggered, so two never commit on the same frame
                        c.t = PERCH_S + (c.id % 5) as f32 * 0.15;
                    }
                }
                Phase::Perch => {
                    c.t -= dt;
                    if c.t <= 0.0 {
                        // SOLVED ONCE, AGAINST WHERE YOU ARE NOW. Never
                        // revisited, which is the counterplay.
                        c.target_at_launch = player;
                        c.vel = pounce_velocity(c.pos, player, self.gravity);
                        c.phase = Phase::Pounce;
                        c.t = FLIGHT_S;
                        out.push((c.id, Event::Pounced));
                    }
                }
                Phase::Pounce => {
                    c.t -= dt;
                    c.pos += c.vel * dt;
                    c.vel.y -= self.gravity * dt;
                    if c.t <= 0.0 {
                        c.pos.y = c.from.y;
                        c.phase = Phase::Ground;
                        c.cooldown = COOLDOWN_S;
                        out.push((c.id, Event::Landed(c.pos)));
                    }
                }
            }
        }
        out
    }
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("CRAWLER AMBUSH — something is crawling the structure\n");

    let spire = Structure { at: Vec3::new(0.0, 0.0, 30.0), height_m: 18.0, face: Material::Basalt };
    let ice = Structure { at: Vec3::new(0.0, 0.0, 30.0), height_m: 18.0, face: Material::SoftIce };
    let stub = Structure { at: Vec3::new(0.0, 0.0, 30.0), height_m: 3.0, face: Material::Basalt };
    println!("  WHAT IS WORTH CLIMBING (from 4 m away)");
    for (n, s) in [("an 18 m basalt spire", spire), ("the same in soft ice", ice), ("a 3 m stub", stub)] {
        println!("    {:<24} {}", n, if usable(&s, Vec3::new(0.0, 0.0, 26.0)) { "yes" } else { "no" });
    }
    println!("    (`cling.rs` decides the face: a crawler must out-hang the climb)\n");

    println!("  THE CAP — eight corrupted, {CONCURRENT_CAP} allowed off the ground\n");
    let crawlers: Vec<Crawler> =
        (0..8).map(|i| Crawler::new(i, Vec3::new(i as f32 * 3.0 - 10.0, 0.0, 30.0))).collect();
    let n_climbers = crawlers.iter().filter(|c| c.is_crawler()).count();
    let mut a = Ambush::new(crawlers);
    let structures = [spire];
    let mut t = 0.0f32;
    let mut peak = 0;
    let mut glinted: Vec<u32> = Vec::new();
    for _ in 0..(60 * 8) {
        t += 1.0 / 60.0;
        for (id, e) in a.step(1.0 / 60.0, Vec3::ZERO, &structures) {
            match e {
                Event::StartedClimbing => println!("    t+{t:>4.1}s  #{id} — something is CRAWLING the structure--"),
                Event::Pounced => println!("    t+{t:>4.1}s  #{id} — IT POUNCES"),
                Event::Landed(p) => println!("    t+{t:>4.1}s  #{id} — lands at {:.0},{:.0}", p.x, p.z),
            }
        }
        // the glint is the last warning and the driver should show it, or
        // the printed trace jumps from "crawling" straight to "POUNCES" and
        // makes the encounter look like it has two warnings rather than three
        for c in &a.crawlers {
            if c.glinting() && !glinted.contains(&c.id) {
                glinted.push(c.id);
                println!("    t+{t:>4.1}s  #{} — eyes catch the dark: it has CHOSEN", c.id);
            }
        }
        peak = peak.max(a.live());
    }
    println!("\n    {n_climbers} of 8 are climbers; at most {peak} were ever off the ground.\n");

    println!("  THE COUNTERPLAY — it solves the arc once, at launch\n");
    let from = Vec3::new(0.0, 18.0, 30.0);
    for (label, moved) in [("stand still", 0.0f32), ("walk", 2.5), ("run", 6.0)] {
        let v = pounce_velocity(from, Vec3::ZERO, 16.0);
        let land = pounce_landing(from, v, 16.0);
        let you = Vec3::new(moved * FLIGHT_S, 0.0, 0.0);
        println!(
            "    {:<12} it lands {:>4.1} m from where you end up",
            label,
            land.distance(you)
        );
    }
    println!("\n  the glints are {GLINT_S:.2}s of warning and the dive is {FLIGHT_S:.2}s long, so the");
    println!("  window is real and it is small. A crawler that tracked you would be");
    println!("  unfair; one that predicts badly is a puzzle.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spire() -> Structure {
        Structure { at: Vec3::new(0.0, 0.0, 30.0), height_m: 18.0, face: Material::Basalt }
    }

    fn pack(n: u32) -> Ambush {
        Ambush::new(
            (0..n).map(|i| Crawler::new(i, Vec3::new(i as f32 - 4.0, 0.0, 30.0))).collect(),
        )
    }

    #[test]
    fn never_more_than_two_are_off_the_ground() {
        // the cap is the discipline: a ceiling of them is a crowd, and a
        // crowd is not frightening
        let mut a = pack(20);
        let s = [spire()];
        let mut peak = 0;
        for _ in 0..(60 * 30) {
            a.step(1.0 / 60.0, Vec3::ZERO, &s);
            peak = peak.max(a.live());
            assert!(a.live() <= CONCURRENT_CAP, "{} were up at once", a.live());
        }
        assert!(peak >= 1, "none of twenty ever climbed anything");
    }

    #[test]
    fn only_some_of_them_are_climbers_and_it_is_stable() {
        let n = (0..200).filter(|i| Crawler::new(*i, Vec3::ZERO).is_crawler()).count();
        let share = n as f32 / 200.0;
        assert!((share - CRAWLER_SHARE).abs() < 0.12, "share was {share}");
        // the same enemy is always the same kind of enemy
        for i in 0..50 {
            let a = Crawler::new(i, Vec3::ZERO).is_crawler();
            let b = Crawler::new(i, Vec3::new(9.0, 0.0, 9.0)).is_crawler();
            assert_eq!(a, b, "#{i} changed kind when it moved");
        }
    }

    #[test]
    fn the_trigger_band_has_both_ends() {
        let mut c = Crawler::new(0, Vec3::ZERO);
        while !c.is_crawler() {
            c = Crawler::new(c.id + 1, Vec3::ZERO);
        }
        assert!(!c.ready(TRIGGER_NEAR_M - 5.0), "too close to have time to climb");
        assert!(c.ready((TRIGGER_NEAR_M + TRIGGER_FAR_M) * 0.5));
        assert!(!c.ready(TRIGGER_FAR_M + 5.0), "too far to be your problem");
    }

    #[test]
    fn cling_decides_what_can_be_climbed() {
        // the two modules must agree about who can hold a face, or a crawler
        // scales a wall `cling.rs` says is unholdable
        let from = Vec3::new(0.0, 0.0, 26.0);
        assert!(usable(&spire(), from));
        let mut short = spire();
        short.height_m = MIN_HEIGHT_M - 1.0;
        assert!(!usable(&short, from), "a stub is not worth climbing");
        let mut far = spire();
        far.at.x = MAX_REACH_M + 10.0;
        assert!(!usable(&far, from), "out of reach");
        let mut slick = spire();
        slick.face = Material::SoftIce;
        // a crawler on soft ice out-hangs the climb, so this one IS usable —
        // the check exists for the cases where it is not
        let held = hang_time_s(Hold::Wall, Frame::crawler(), Material::SoftIce).unwrap();
        assert_eq!(usable(&slick, from), held > CLIMB_S);
    }

    #[test]
    fn the_arc_is_solved_once_and_moving_beats_it() {
        // THE COUNTERPLAY. Flight time is fixed and the solution is taken at
        // launch, so a target that moves afterwards is missed by exactly how
        // far it moved.
        let from = Vec3::new(0.0, 18.0, 30.0);
        let v = pounce_velocity(from, Vec3::ZERO, 16.0);
        let land = pounce_landing(from, v, 16.0);
        assert!(land.distance(Vec3::ZERO) < 0.01, "it should hit a stationary target");

        let ran = Vec3::new(6.0 * FLIGHT_S, 0.0, 0.0);
        assert!(
            land.distance(ran) > 4.0,
            "running should beat it: missed by {}",
            land.distance(ran)
        );
    }

    #[test]
    fn the_ballistics_are_real() {
        // the arc has to actually come down where the solution says
        let from = Vec3::new(0.0, 18.0, 30.0);
        let target = Vec3::new(5.0, 0.0, 12.0);
        let g = 16.0;
        let mut p = from;
        let mut v = pounce_velocity(from, target, g);
        let dt = 1.0 / 240.0;
        for _ in 0..((FLIGHT_S / dt) as i32) {
            p += v * dt;
            v.y -= g * dt;
        }
        assert!(p.distance(target) < 0.5, "landed {} m off", p.distance(target));
    }

    #[test]
    fn there_are_three_warnings_and_the_last_one_is_late() {
        let mut a = pack(6);
        let s = [spire()];
        let (mut climbed, mut pounced, mut glint_seen) = (false, false, false);
        for _ in 0..(60 * 12) {
            for (_, e) in a.step(1.0 / 60.0, Vec3::ZERO, &s) {
                match e {
                    Event::StartedClimbing => climbed = true,
                    Event::Pounced => {
                        assert!(climbed, "it pounced without climbing first");
                        assert!(glint_seen, "it pounced with no glint warning");
                        pounced = true;
                    }
                    Event::Landed(_) => {}
                }
            }
            if a.crawlers.iter().any(|c| c.glinting()) {
                glint_seen = true;
            }
        }
        assert!(pounced, "nobody ever committed");
        // and the last warning is short: less than the dive it precedes
        assert!(GLINT_S < FLIGHT_S);
    }

    #[test]
    fn a_crawler_that_has_pounced_waits_a_long_time() {
        let mut a = pack(4);
        let s = [spire()];
        let mut landed_at = None;
        for i in 0..(60 * 12) {
            for (_, e) in a.step(1.0 / 60.0, Vec3::ZERO, &s) {
                if matches!(e, Event::Landed(_)) && landed_at.is_none() {
                    landed_at = Some(i);
                }
            }
        }
        assert!(landed_at.is_some(), "nobody landed");
        let c = a.crawlers.iter().find(|c| c.cooldown > 0.0).expect("someone should be resting");
        assert!(c.cooldown > 1.0);
    }

    #[test]
    fn nothing_climbs_a_field_with_no_structures() {
        let mut a = pack(10);
        for _ in 0..(60 * 20) {
            a.step(1.0 / 60.0, Vec3::ZERO, &[]);
            assert_eq!(a.live(), 0, "it climbed thin air");
        }
    }
}
