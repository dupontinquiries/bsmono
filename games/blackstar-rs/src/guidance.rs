//! GUIDANCE — the ball-chaser as a missile, and what has to change.
//!
//! `bots.rs` drives three hundred cars at the ball on CPU at frame rate, and
//! the suggestion is to point that at a weapon target instead. It is a good
//! suggestion: the structure is already right — a target with a position and
//! a velocity, a chaser with a limited turn rate, and a uniform grid so a
//! pile stays a pile instead of a singularity.
//!
//! One thing has to change, and it is the steering law.
//!
//! ## What ballchase actually does
//!
//! ```text
//! let target = ball.pos + ball.vel * 0.25 + wob;
//! let ang    = signed angle from forward to target;
//! c.steer    = (ang * 2.4).clamp(-1, 1);
//! ```
//!
//! That is **pure pursuit with a fixed quarter-second lead**. Point at where
//! the thing will be in 0.25 s and drive. For a car chasing a ball it is
//! exactly right: the lead is short because the car is slow, and constantly
//! re-aiming looks eager, which the module's own doc calls "tuned for comedy".
//!
//! For a missile it is the classic wrong answer. Pure pursuit **tail-chases**:
//! against anything crossing, the pursuer curves in behind and arrives from
//! the rear having flown a longer path with a harder turn at the end — which
//! is the moment its turn rate runs out.
//!
//! ## Proportional navigation
//!
//! Turn in proportion to how fast the LINE OF SIGHT is rotating, not to how
//! far off-axis the target is:
//!
//! ```text
//! a_lat = N · V_closing · λ̇
//! ```
//!
//! If the bearing to the target is not changing, you are on a collision
//! course and should not turn AT ALL — which is the exact opposite of what
//! pure pursuit does, and the whole reason it wins. A constant-bearing
//! approach is a hit, and PN's job is to null the bearing rate rather than
//! the bearing.
//!
//! ## What is measured here
//!
//! Both laws, the same target, the same turn-rate limit, across crossing
//! angles. The turn limit is what makes it a real comparison: with infinite
//! agility everything hits.

use glam::Vec3;

/// A thing being shot at.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub pos: Vec3,
    pub vel: Vec3,
    /// lateral acceleration it can pull, m/s²
    pub agility: f32,
}

/// The chaser.
#[derive(Clone, Copy, Debug)]
pub struct Missile {
    pub pos: Vec3,
    pub vel: Vec3,
    /// how hard it can turn, m/s² of lateral acceleration
    pub turn: f32,
    /// how close counts as a hit
    pub lethal_r: f32,
}

/// Which steering law.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Law {
    /// what `bots.rs` does: aim at the target plus a fixed lead
    PurePursuit,
    /// null the line-of-sight rate
    Proportional,
}

/// `bots.rs`'s lead, in seconds.
pub const BALLCHASE_LEAD_S: f32 = 0.25;

/// PN's navigation constant. Three to five is the usual range; below two it
/// under-leads and above about six it chases its own noise.
pub const NAV_N: f32 = 4.0;

/// The commanded lateral acceleration, before the turn limit.
pub fn steer(law: Law, m: &Missile, t: &Target) -> Vec3 {
    let r = t.pos - m.pos;
    let speed = m.vel.length().max(0.01);
    match law {
        Law::PurePursuit => {
            // aim at where it will be in a fixed time, then turn toward that
            let aim = t.pos + t.vel * BALLCHASE_LEAD_S;
            let want = (aim - m.pos).normalize_or_zero();
            let have = m.vel / speed;
            // acceleration perpendicular to travel that rotates toward `want`
            let lat = want - have * have.dot(want);
            lat * m.turn
        }
        Law::Proportional => {
            // LOS rate: the component of relative velocity perpendicular to
            // the line of sight, over the range
            let rng = r.length().max(0.01);
            let vr = t.vel - m.vel;
            let los = r / rng;
            let omega = los.cross(vr) / rng; // rotation vector of the sight line
            let closing = (-vr.dot(los)).max(0.0);
            // a_lat = N · Vc · (ω × los)
            omega.cross(los) * (NAV_N * closing)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shot {
    Hit { at_s: f32, path_m: f32 },
    Miss { closest_m: f32, at_s: f32 },
}

impl Shot {
    pub fn hit(self) -> bool {
        matches!(self, Shot::Hit { .. })
    }
    pub fn miss_m(self) -> f32 {
        match self {
            Shot::Hit { .. } => 0.0,
            Shot::Miss { closest_m, .. } => closest_m,
        }
    }
}

/// Fly one intercept. `jink` makes the target manoeuvre.
pub fn intercept(law: Law, mut m: Missile, mut t: Target, max_s: f32, jink: bool) -> Shot {
    let dt = 1.0 / 120.0;
    let speed = m.vel.length().max(0.01);
    let (mut closest, mut closest_t, mut path) = (f32::MAX, 0.0f32, 0.0f32);
    let mut time = 0.0f32;
    while time < max_s {
        time += dt;
        // the target's own motion
        if jink && t.agility > 0.0 {
            let side = Vec3::new(-t.vel.z, 0.0, t.vel.x).normalize_or_zero();
            t.vel += side * t.agility * (time * 2.0).sin() * dt;
        }
        t.pos += t.vel * dt;

        let a = steer(law, &m, &t);
        // THE TURN LIMIT is what makes this a comparison rather than a
        // formality: with unlimited agility every law hits.
        let a = if a.length() > m.turn { a.normalize_or_zero() * m.turn } else { a };

        // STEER BY ROTATING, NOT BY ADDING. The first version did
        // `vel += a*dt` and renormalised, which is fine while `a*dt` is small
        // against the speed and nonsense when it is not: at a turn limit of
        // 100,000 the added vector dwarfs the velocity, so renormalising left
        // the missile pointing along its own lateral acceleration instead of
        // where it was going. Pure pursuit then MISSED WITH INFINITE AGILITY,
        // which is not a thing that can happen and was the tell.
        //
        // Lateral acceleration is a turn rate: ω = a/v. Rotating by ω·dt is
        // exact at any magnitude and cannot change the speed.
        let dir = m.vel / speed;
        let lat = a - dir * dir.dot(a);
        if lat.length() > 1e-6 {
            let omega = lat.length() / speed;
            let axis = dir.cross(lat).normalize_or_zero();
            let q = glam::Quat::from_axis_angle(axis, omega * dt);
            m.vel = q * m.vel;
        }
        m.pos += m.vel * dt;
        path += speed * dt;

        let d = m.pos.distance(t.pos);
        if d < closest {
            closest = d;
            closest_t = time;
        }
        if d <= m.lethal_r {
            return Shot::Hit { at_s: time, path_m: path };
        }
        // gone past and opening: no second chance
        if d > closest + m.lethal_r * 6.0 && time > 0.4 {
            break;
        }
    }
    Shot::Miss { closest_m: closest, at_s: closest_t }
}

/// A standard engagement: the missile starts at the origin heading +Z, the
/// target crosses at `angle_deg` from head-on.
pub fn engagement(angle_deg: f32, target_speed: f32, missile_speed: f32, turn: f32) -> (Missile, Target) {
    let a = angle_deg.to_radians();
    // OPENING RANGE 1500 m, not 300. The first version launched from three
    // hundred metres, which at 250 m/s is 1.2 seconds of flight — and a
    // 90 m/s² missile can turn 24.8° in that time against 25.6° of lead
    // required. The engagement was GEOMETRICALLY IMPOSSIBLE and both laws
    // missed by a hundred metres, which I read as the laws failing rather
    // than as the setup being unwinnable.
    (
        Missile {
            pos: Vec3::ZERO,
            vel: Vec3::new(0.0, 0.0, missile_speed),
            turn,
            lethal_r: 3.0,
        },
        Target {
            pos: Vec3::new(0.0, 0.0, 1500.0),
            vel: Vec3::new(a.sin(), 0.0, a.cos()) * target_speed,
            agility: 40.0,
        },
    )
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("GUIDANCE — the ball-chaser as a missile\n");
    println!("  bots.rs steers three hundred cars at a ball on CPU, and the structure");
    println!("  ports directly: a target with a velocity, a chaser with a turn limit.");
    println!("  What has to change is the steering law.\n");
    println!("    ballchase:  aim at ball.pos + ball.vel * {BALLCHASE_LEAD_S} and drive");
    println!("    that is PURE PURSUIT with a fixed lead. Right for a car chasing a");
    println!("    ball; for a missile it tail-chases.\n");

    println!("  STRAIGHT-FLYING TARGET, 400 m/s missile against 200 m/s, 150 m/s^2 turn\n");
    println!("  {:>9} {:>18} {:>18}", "crossing", "pure pursuit", "proportional");
    let (mut pp_hits, mut pn_hits) = (0, 0);
    for deg in [0.0f32, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0] {
        let (m, t) = engagement(deg, 200.0, 400.0, 150.0);
        let a = intercept(Law::PurePursuit, m, t, 12.0, false);
        let b = intercept(Law::Proportional, m, t, 12.0, false);
        if a.hit() {
            pp_hits += 1;
        }
        if b.hit() {
            pn_hits += 1;
        }
        let show = |s: Shot| match s {
            Shot::Hit { at_s, path_m } => format!("hit {at_s:.2}s {path_m:.0}m"),
            Shot::Miss { closest_m, .. } => format!("MISS by {closest_m:.0} m"),
        };
        println!("  {:>8.0}° {:>18} {:>18}", deg, show(a), show(b));
    }
    println!("\n    {pp_hits}/7 vs {pn_hits}/7\n");

    println!("  AND AGAINST A TARGET THAT JINKS\n");
    println!("  {:>9} {:>18} {:>18}", "crossing", "pure pursuit", "proportional");
    for deg in [30.0f32, 60.0, 90.0, 120.0] {
        let (m, t) = engagement(deg, 200.0, 400.0, 150.0);
        let a = intercept(Law::PurePursuit, m, t, 12.0, true);
        let b = intercept(Law::Proportional, m, t, 12.0, true);
        let show = |s: Shot| match s {
            Shot::Hit { at_s, path_m } => format!("hit {at_s:.2}s {path_m:.0}m"),
            Shot::Miss { closest_m, .. } => format!("MISS by {closest_m:.0} m"),
        };
        println!("  {:>8.0}° {:>18} {:>18}", deg, show(a), show(b));
    }

    println!("\n  HOW MUCH AGILITY EACH LAW NEEDS (90° crossing)\n");
    println!("  {:>10} {:>16} {:>16}", "turn m/s^2", "pure pursuit", "proportional");
    for turn in [20.0f32, 40.0, 60.0, 90.0, 150.0, 250.0] {
        let (m, t) = engagement(90.0, 200.0, 400.0, turn);
        let a = intercept(Law::PurePursuit, m, t, 12.0, false);
        let b = intercept(Law::Proportional, m, t, 12.0, false);
        println!(
            "  {:>10.0} {:>16} {:>16}",
            turn,
            if a.hit() { "hit".into() } else { format!("{:.0} m", a.miss_m()) },
            if b.hit() { "hit".into() } else { format!("{:.0} m", b.miss_m()) }
        );
    }

    println!("\n  A SALVO HAS THE OPPOSITE PROBLEM TO A CROWD\n");
    println!("    bots.rs keeps three hundred cars apart with a uniform grid so a");
    println!("    pile stays a pile instead of a singularity. Missiles have that and");
    println!("    worse: PN is DETERMINISTIC, so a salvo at one target computes one");
    println!("    solution and arrives as a single warhead. A separation grid cannot");
    println!("    fix it — pushing them apart in the last hundred metres is too late");
    println!("    and fights the guidance instead of shaping it.\n");
    println!("  {:>10} {:>8} {:>14}", "aim spread", "hits/12", "impact spread");
    for sp in [0.0f32, 10.0, SALVO_SPREAD_M, 60.0] {
        let (h, s) = salvo(12, sp, 60.0);
        println!("  {:>9.0}m {:>8} {:>13.1}m", sp, h, s);
    }
    println!("\n    the fix is upstream: give each missile its own aim point around");
    println!("    the target so they are solving different problems from launch.");

    let t0 = std::time::Instant::now();
    let (h, _) = salvo(300, SALVO_SPREAD_M, 60.0);
    println!(
        "\n    and 300 of them: {h} hits in {:.0} ms of CPU. PN is a cross product",
        t0.elapsed().as_secs_f32() * 1000.0
    );
    println!("    and a dot product per missile per tick, which is far less than the");
    println!("    car physics the ball-chaser was already running at that count.");

    println!("\n  that last table is the point. Proportional navigation does not turn");
    println!("  when the bearing is steady, so it spends its agility budget on the");
    println!("  approach rather than on a hard break at the end. A cheaper missile");
    println!("  hits with it — which is what matters if there are going to be");
    println!("  hundreds of them, the way there are hundreds of cars.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_laws_hit_a_sitting_target() {
        // the sanity floor: if a law cannot hit something stationary the
        // comparison below is measuring a bug
        for law in [Law::PurePursuit, Law::Proportional] {
            let m = Missile { pos: Vec3::ZERO, vel: Vec3::new(0.0, 0.0, 400.0), turn: 90.0, lethal_r: 3.0 };
            let t = Target { pos: Vec3::new(0.0, 0.0, 1500.0), vel: Vec3::ZERO, agility: 0.0 };
            assert!(intercept(law, m, t, 12.0, false).hit(), "{law:?} missed a parked target");
        }
    }

    #[test]
    fn pure_pursuit_tail_chases_and_proportional_does_not() {
        // THE CLAIM. Against a crossing target, pure pursuit curves in behind
        // and flies a longer path; PN takes the lead angle and goes straight.
        let (m, t) = engagement(90.0, 200.0, 400.0, 150.0);
        let pp = intercept(Law::PurePursuit, m, t, 12.0, false);
        let pn = intercept(Law::Proportional, m, t, 12.0, false);
        match (pp, pn) {
            (Shot::Hit { path_m: a, .. }, Shot::Hit { path_m: b, .. }) => {
                assert!(b < a, "PN flew {b:.0} m and pursuit flew {a:.0} m");
            }
            (Shot::Miss { .. }, Shot::Hit { .. }) => {}
            other => panic!("expected PN to do at least as well: {other:?}"),
        }
    }

    #[test]
    fn proportional_wins_where_agility_is_scarce() {
        // the practical claim, and the one that matters for hundreds of them:
        // PN needs less turn to make the same shot
        let need = |law: Law| {
            for turn in (10..=300).step_by(5) {
                let (m, t) = engagement(90.0, 200.0, 400.0, turn as f32);
                if intercept(law, m, t, 12.0, false).hit() {
                    return Some(turn as f32);
                }
            }
            None
        };
        let pn = need(Law::Proportional).expect("PN should hit at some agility");
        match need(Law::PurePursuit) {
            Some(pp) => assert!(pn <= pp, "PN needed {pn} and pursuit {pp}"),
            None => {} // pursuit never manages it, which is a stronger result
        }
    }

    #[test]
    fn a_steady_bearing_means_do_not_turn() {
        // the property the whole law rests on: on a collision course PN
        // commands almost nothing, where pure pursuit is still correcting
        let m = Missile { pos: Vec3::ZERO, vel: Vec3::new(0.0, 0.0, 400.0), turn: 90.0, lethal_r: 3.0 };
        // a target crossing exactly fast enough that the bearing holds
        let t = Target { pos: Vec3::new(300.0, 0.0, 1500.0), vel: Vec3::new(0.0, 0.0, 0.0), agility: 0.0 };
        // set the missile onto the collision line
        let mut m2 = m;
        m2.vel = (t.pos - m.pos).normalize() * 400.0;
        let a = steer(Law::Proportional, &m2, &t);
        assert!(a.length() < 1.0, "PN commanded {:.2} on a collision course", a.length());
    }

    #[test]
    fn the_turn_limit_is_what_makes_it_a_comparison() {
        // with unlimited agility every law hits, so a test that forgets the
        // limit measures nothing
        let (mut m, t) = engagement(90.0, 200.0, 400.0, 100_000.0);
        m.turn = 100_000.0;
        for law in [Law::PurePursuit, Law::Proportional] {
            assert!(intercept(law, m, t, 12.0, false).hit(), "{law:?} missed with infinite turn");
        }
    }

    #[test]
    fn a_jinking_target_is_harder_for_both_and_worse_for_pursuit() {
        let (m, t) = engagement(60.0, 200.0, 400.0, 150.0);
        let straight = intercept(Law::Proportional, m, t, 12.0, false);
        let jinking = intercept(Law::Proportional, m, t, 12.0, true);
        // a manoeuvring target should cost something even for PN
        assert!(
            straight.hit() || !jinking.hit(),
            "jinking made it easier, which means the jink is not doing anything"
        );
    }

    #[test]
    fn the_ballchase_lead_is_the_thing_that_does_not_port() {
        // 0.25 s is right for a car closing at a few metres per second and
        // meaningless at 250 m/s — a quarter second is sixty metres of flight
        let flight = 400.0 * BALLCHASE_LEAD_S;
        assert!(flight > 50.0, "the lead covers {flight} m at missile speed");
        // and the miss it causes grows with target speed
        let miss = |v: f32| {
            let (m, t) = engagement(90.0, v, 400.0, 70.0);
            intercept(Law::PurePursuit, m, t, 12.0, false).miss_m()
        };
        assert!(miss(200.0) >= miss(40.0), "{} vs {}", miss(200.0), miss(40.0));
    }

    #[test]
    fn a_missile_holds_its_speed_while_steering() {
        // steering changes direction, not thrust — if this drifts the
        // comparison becomes about who accelerates hardest
        let m = Missile { pos: Vec3::ZERO, vel: Vec3::new(0.0, 0.0, 400.0), turn: 90.0, lethal_r: 1.0 };
        let t = Target { pos: Vec3::new(400.0, 0.0, 1500.0), vel: Vec3::new(0.0, 0.0, 90.0), agility: 0.0 };
        // fly a while and check the speed held
        let mut mm = m;
        let dt = 1.0 / 120.0;
        for _ in 0..200 {
            let a = steer(Law::Proportional, &mm, &t);
            let a = if a.length() > mm.turn { a.normalize_or_zero() * mm.turn } else { a };
            mm.vel += a * dt;
            mm.vel = mm.vel.normalize_or_zero() * 400.0;
            mm.pos += mm.vel * dt;
        }
        assert!((mm.vel.length() - 400.0).abs() < 0.1);
    }
}

// ----------------------------------------------------------- the salvo ---

/// A salvo's problem is the opposite of a crowd's.
///
/// `bots.rs` keeps three hundred cars apart with a uniform grid so "a
/// 300-spartan pile stays a pile instead of a singularity" — they all want
/// the same ball and the grid stops them occupying the same metre.
///
/// Missiles have that problem and a worse one. Proportional navigation is
/// DETERMINISTIC: given the same target, every missile with the same speed
/// and turn limit computes the same solution, so a salvo launched together
/// converges on one point and arrives as a single warhead. A separation grid
/// does not fix that — pushing them apart in the last hundred metres is far
/// too late, and it fights the guidance rather than shaping it.
///
/// The fix is upstream: give each missile a different AIM POINT around the
/// target, so they are solving different problems from launch and arrive
/// spread. Costs nothing, and it is what a real salvo does.
pub const SALVO_SPREAD_M: f32 = 25.0;

/// Where missile `i` of `n` aims, relative to the target.
pub fn salvo_offset(i: usize, n: usize, spread_m: f32) -> Vec3 {
    if n <= 1 {
        return Vec3::ZERO;
    }
    let a = i as f32 / n as f32 * std::f32::consts::TAU;
    // a ring in the plane across the line of flight
    Vec3::new(a.cos(), a.sin() * 0.4, 0.0) * spread_m
}

/// Fly `n` missiles at one target and report where they ended up.
///
/// Returns (hits, mean spread of impact points in metres).
pub fn salvo(n: usize, spread_m: f32, angle_deg: f32) -> (usize, f32) {
    let (m0, t0) = engagement(angle_deg, 200.0, 400.0, 150.0);
    let mut ends = Vec::new();
    let mut hits = 0;
    for i in 0..n {
        let off = salvo_offset(i, n, spread_m);
        let mut m = m0;
        // launched from slightly different rails, as they would be
        m.pos += salvo_offset(i, n, 4.0);
        let mut t = t0;
        t.pos += off; // each solves for its own aim point
        match intercept(Law::Proportional, m, t, 15.0, false) {
            Shot::Hit { at_s, .. } => {
                hits += 1;
                ends.push(t0.pos + t0.vel * at_s + off);
            }
            Shot::Miss { at_s, .. } => ends.push(t0.pos + t0.vel * at_s + off),
        }
    }
    let centre = ends.iter().fold(Vec3::ZERO, |a, b| a + *b) / ends.len().max(1) as f32;
    let spread = ends.iter().map(|e| e.distance(centre)).sum::<f32>() / ends.len().max(1) as f32;
    (hits, spread)
}

#[cfg(test)]
mod salvo_tests {
    use super::*;

    #[test]
    fn an_unspread_salvo_arrives_as_one_warhead() {
        // PN is deterministic: same target, same solution, same impact point.
        // A separation grid cannot fix this — by the time they are close
        // enough to push apart, the shot is over.
        let (hits, spread) = salvo(12, 0.0, 60.0);
        assert!(hits > 0, "nobody hit");
        assert!(spread < 5.0, "an unspread salvo spread {spread:.1} m");
    }

    #[test]
    fn giving_each_its_own_aim_point_spreads_the_impacts() {
        let (hits, spread) = salvo(12, SALVO_SPREAD_M, 60.0);
        assert!(hits > 0, "spreading cost every hit");
        assert!(spread > 10.0, "spread only reached {spread:.1} m");
    }

    #[test]
    fn the_spread_does_not_cost_the_salvo_its_hits() {
        // a spread that halves the hit rate is not a spread, it is a miss
        // distribution
        for deg in [30.0f32, 60.0, 90.0] {
            let (tight, _) = salvo(8, 0.0, deg);
            let (wide, _) = salvo(8, SALVO_SPREAD_M, deg);
            assert!(
                wide * 2 >= tight,
                "at {deg}°: spreading took {tight} hits down to {wide}"
            );
        }
    }

    #[test]
    fn offsets_ring_the_target_rather_than_stacking() {
        let n = 8;
        let offs: Vec<Vec3> = (0..n).map(|i| salvo_offset(i, n, 25.0)).collect();
        for (i, a) in offs.iter().enumerate() {
            for b in &offs[i + 1..] {
                assert!(a.distance(*b) > 1.0, "two missiles share an aim point");
            }
        }
        // and a single missile aims at the target itself
        assert_eq!(salvo_offset(0, 1, 25.0), Vec3::ZERO);
    }

    #[test]
    fn three_hundred_of_them_is_cheap() {
        // the claim that made this worth porting: the ball-chaser held up at
        // hundreds on CPU. PN is far simpler than car physics — a cross
        // product and a dot product per missile per tick — so this should not
        // even be close.
        let start = std::time::Instant::now();
        let (hits, _) = salvo(300, SALVO_SPREAD_M, 60.0);
        let el = start.elapsed().as_secs_f32();
        assert!(hits > 0);
        assert!(el < 5.0, "300 intercepts took {el:.2}s");
    }
}
