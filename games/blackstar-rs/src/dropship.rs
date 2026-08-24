//! THE DROPSHIP PASS — one flyover, every flow.
//!
//! Extracted from `sortie.rs` the day the raid flow needed the same twenty
//! seconds: a `BigBody` hull on a deterministic approach line, banking into
//! its escape turn, throwing the jumper out of the belly bay with
//! `Frame::carry_at` — velocity plus the rotational term through the real
//! lever arm. A sortie flies one of these; a raid flies one PER PHASE,
//! because a phase swaps the arena and the way onto an arena is out the
//! door.
//!
//! Still canned, by name: the hull's dimensions and the fact that it flies
//! at all (nobody owns a live flying ship yet — when the parked hull can
//! launch, this becomes `carry_at` on THAT body and the constants retire).
//! Hull WEAR dirties the bank: a ship that sat pinned on hostile decks
//! rolls harder on the run-in, and the door throws accordingly.

use glam::{Quat, Vec3};

use crate::bigbody::{BigBody, Shape};
use crate::drop::Drop;

/// Altitude of the door above the LZ's ground, metres.
pub const APPROACH_ALT: f32 = 600.0;
/// How far up-track the door opens. Far enough that a vehicle-mode dart
/// overshoots without correction — the drop should be a decision, not a
/// formality.
pub const APPROACH_BACK: f32 = 900.0;
/// The ship's pass speed at the door, m/s, straight down-track.
pub const APPROACH_CARRY: f32 = 62.0;
/// How hard the flyover banks into its escape turn, rad/s. The lateral
/// throw at the door is carry_at's answer to this — computed through the
/// lever arm, not injected.
pub const BANK_RATE: f32 = 0.5;

/// Fly the pass and step out. Builds the hull on the approach line,
/// asks the drop's own gate (`can_jump`), and returns the hull mid-pass
/// plus the drop already falling — or the gate's refusal.
pub fn launch(
    lz: Vec3,
    defended: bool,
    wear: f32,
    ground_y: f32,
    hash: u64,
) -> Result<(BigBody, Drop), &'static str> {
    let dir = (lz * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let sign = if hash & 1 == 0 { 1.0 } else { -1.0 };
    // a praetor-ish box with an aft belly bay — dimensions canned, the
    // mechanism not
    let shape = Shape {
        half: Vec3::new(9.0, 4.0, 22.0),
        round: 1.5,
        bay: Some((Vec3::new(0.0, -2.5, 8.0), Vec3::new(3.0, 2.0, 5.0))),
    };
    let door_local = Vec3::new(0.0, -4.5, 8.0); // the bay's open floor
    let mut hull = BigBody::new(
        shape,
        lz - dir * APPROACH_BACK + Vec3::new(0.0, ground_y + APPROACH_ALT - door_local.y, 0.0),
    );
    hull.frame.rot = Quat::from_rotation_arc(Vec3::NEG_Z, dir);
    hull.frame.vel = dir * APPROACH_CARRY;
    let bank = BANK_RATE * (1.0 + 0.35 * wear);
    hull.frame.ang = dir * (bank * sign) + Vec3::Y * (0.15 * sign);
    let door = hull.frame.to_world(door_local);
    let carry = hull.frame.carry_at(door);

    let mut drop = Drop::new(lz, defended);
    // the drop's own gate, asked for real: the bay is opened for the run
    // (canned), the rest is measured
    let alt = door.y - ground_y;
    drop.can_jump(true, alt > 0.0, alt)?;
    drop.jump(door, carry);
    Ok((hull, drop))
}

/// Charge a tick's flak to the body, low and alternating so it spreads
/// over the legs instead of milling one section. Returns a toast when the
/// body just died of it. `applied` and `note_t` are the caller's running
/// state (total charged, feed-note throttle).
pub fn charge_flak(
    flak: f32,
    dt: f32,
    applied: &mut f32,
    note_t: &mut f32,
    body: &mut crate::sections::Body,
    feed: &mut Vec<String>,
) -> Option<&'static str> {
    use crate::sections::Kind;
    if flak <= 0.0 {
        return None;
    }
    let side = if (*applied * 3.0) as i32 % 2 == 0 { -0.25 } else { 0.25 };
    *applied += flak;
    let was_alive = body.alive();
    let res = body.hit(Kind::Blast, Vec3::new(side, 0.5, 0.0), Vec3::Y, flak, 0.6);
    for sct in &res.severed {
        feed.push(format!("flak took the {}", sct.name()));
    }
    *note_t -= dt;
    if *note_t <= 0.0 {
        feed.push("flak — taking fire on the way down".into());
        *note_t = 2.0;
    }
    if was_alive && res.killed {
        Some("STRUCTURAL FAILURE — riding it down")
    } else {
        None
    }
}

/// Charge a landing's impact to the legs — half each, kinetic: wrecked at
/// zero, not severed, per sections' own rule about what cuts. Returns a
/// toast when the body just died of it.
pub fn charge_landing(
    l: &crate::drop::Landing,
    body: &mut crate::sections::Body,
    feed: &mut Vec<String>,
) -> Option<&'static str> {
    use crate::sections::Kind;
    if l.self_dmg <= 0.0 {
        return None;
    }
    let was_alive = body.alive();
    for side in [-0.25f32, 0.25] {
        let res = body.hit(Kind::Kinetic, Vec3::new(side, 0.3, 0.0), Vec3::Y, l.self_dmg * 0.5, 0.0);
        for sct in &res.severed {
            feed.push(format!("the landing took the {}", sct.name()));
        }
    }
    if was_alive && !body.alive() {
        Some("STRUCTURAL FAILURE on impact")
    } else {
        None
    }
}

/// The landing, in one line — the same phrasing wherever a drop ends.
pub fn landing_line(l: &crate::drop::Landing, penalty: f32) -> String {
    if l.hero {
        format!("HERO LANDING — {:.0} m out, the ground wore it", l.miss_distance)
    } else if l.miss_distance < 20.0 {
        format!("clean drop — {:.0} m from the mark", l.miss_distance)
    } else if penalty < 0.2 {
        format!("loose drop — {:.0} m out", l.miss_distance)
    } else {
        format!("SCATTERED — {:.0} m out; you start this badly", l.miss_distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_throw_comes_from_the_bank_not_from_a_constant() {
        // two hulls, mirrored bank signs: the lateral components of the two
        // throws must oppose, because they come from carry_at's rotational
        // term and nothing else
        let lz = Vec3::new(300.0, 0.0, 0.0);
        let (_, a) = launch(lz, false, 0.0, 0.0, 0).unwrap();
        let (_, b) = launch(lz, false, 0.0, 0.0, 1).unwrap();
        let dir = Vec3::new(1.0, 0.0, 0.0);
        let lat = |d: &Drop| d.vel - dir * d.vel.dot(dir);
        let (la, lb) = (lat(&a), lat(&b));
        assert!(la.length() > 0.5, "a banking pass threw nothing sideways: {la}");
        assert!(la.dot(lb) < 0.0, "mirrored banks threw the same way: {la} vs {lb}");
    }

    #[test]
    fn a_worn_hull_flies_a_dirtier_pass() {
        let lz = Vec3::new(300.0, 0.0, 0.0);
        let (_, clean) = launch(lz, false, 0.0, 0.0, 0).unwrap();
        let (_, worn) = launch(lz, false, 6.0, 0.0, 0).unwrap();
        let dir = Vec3::new(1.0, 0.0, 0.0);
        let lat = |d: &Drop| (d.vel - dir * d.vel.dot(dir)).length();
        assert!(
            lat(&worn) > lat(&clean) * 1.5,
            "wear did not dirty the pass: {} vs {}",
            lat(&worn),
            lat(&clean)
        );
    }
}
