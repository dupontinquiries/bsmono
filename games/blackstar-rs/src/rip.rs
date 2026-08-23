//! THE RIP — the style of the kill belongs to WHO YOU ARE.
//!
//! Ported from `game.c`. Low-health enemies stagger; close in and execute,
//! and their actual body parts go ballistic. The C's own table:
//!
//! > prime → MERCIFUL: one clean strike, the frame folds, parts drop soft
//! > reaver/pyre → BRUTAL: torn apart, everything flies hard, oil smoke
//! > scout/interceptor → PRECISION: a flank-shift, one tight pop, neat ring
//! > specialist/warden → SURGICAL: parts lift in ordered anti-grav float
//! > vanguard/titan/siege → HEAVY: a straight power-rip, radial burst
//! > dino/dragon alt-mode → THRASH override: savage double-fling
//!
//! Six characters expressed entirely in the velocities of somebody else's
//! parts, which is a lovely way to say something about a player without a
//! line of dialogue.
//!
//! ## The one that is hidden
//!
//! There is a seventh, and `game.c` only mentions it in a comment next to the
//! condition: a big frame that gets its hands on a small enemy
//! **does not scatter it — it takes it in both hands and tears it in TWO**,
//! along your own shoulder line. Prime will not (mercy holds) and the beast
//! will not (it thrashes). Everyone else will.
//!
//! What makes it read is coherence: the two halves are given near-identical
//! velocities, so each moves as ONE piece rather than a cloud. Every other
//! style is a scatter; this one is the absence of scatter, and that is why it
//! looks deliberate.

use glam::Vec3;

/// The parts that fly. Enough of the C's set to tell upper from lower, which
/// is the only distinction any style actually makes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Head,
    Visor,
    Chest,
    Abdomen,
    ShoulderL,
    ShoulderR,
    ArmL,
    ArmR,
    HandL,
    HandR,
    Back,
    HipL,
    HipR,
    LegL,
    LegR,
    FootL,
    FootR,
}

impl Part {
    pub fn all() -> [Part; 17] {
        use Part::*;
        [
            Head, Visor, Chest, Abdomen, ShoulderL, ShoulderR, ArmL, ArmR, HandL, HandR, Back,
            HipL, HipR, LegL, LegR, FootL, FootR,
        ]
    }
    /// Above the tear line. The C lists these explicitly; everything else
    /// goes with the legs.
    pub fn upper(self) -> bool {
        use Part::*;
        matches!(
            self,
            Chest | Abdomen | Head | Visor | ShoulderL | ShoulderR | ArmL | ArmR | Back | HandL
                | HandR
        )
    }
}

/// Who is doing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// one clean strike; the frame folds and parts drop soft
    Merciful,
    /// torn apart, everything flies hard
    Brutal,
    /// a flank-shift, one tight pop, a neat radial ring
    Precision,
    /// parts lift in an ordered anti-grav float, then drop
    Surgical,
    /// a straight power-rip
    Heavy,
    /// savage double-fling, huge scatter
    Thrash,
    /// torn in two along your shoulder line
    Jazz,
}

/// The player's frame class, as the styles care about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Frame {
    Prime,
    Reaver,
    Scout,
    Specialist,
    Vanguard,
    /// dino / dragon alt-mode
    Beast,
}

/// The victim, as the override cares about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Victim {
    /// grunt, hijack — small enough to pick up
    Small,
    Standard,
    // No `Apex`. I added one out of habit from `stealth.rs`, where an apex
    // frame is a real case because it changes an assassination into a wound.
    // Here it changes nothing: the override asks only whether the victim is
    // SMALL ENOUGH TO PICK UP, and an apex is `Standard`'s answer — no — by
    // the same logic. A variant that only ever takes the other branch is a
    // distinction the code does not make.
}

/// Bulk above which you count as big enough to tear something in half.
pub const BIG_BULK: f32 = 0.72;

/// Which rip happens.
///
/// The override is the interesting line and it is a THREE-way condition: big
/// enough, small enough victim, and not one of the two frames that will not
/// do it. Prime's refusal is mercy; the beast's is that it has its own idea.
pub fn style_for(frame: Frame, victim: Victim, bulk: f32) -> Style {
    let base = match frame {
        Frame::Prime => Style::Merciful,
        Frame::Reaver => Style::Brutal,
        Frame::Scout => Style::Precision,
        Frame::Specialist => Style::Surgical,
        Frame::Vanguard => Style::Heavy,
        Frame::Beast => Style::Thrash,
    };
    let big = bulk > BIG_BULK;
    let refuses = matches!(frame, Frame::Prime | Frame::Beast);
    if big && victim == Victim::Small && !refuses {
        return Style::Jazz;
    }
    base
}

/// Does this style close the distance by lunging, or by blinking to the flank?
///
/// Scouts do not charge. The C moves them straight to a flank position at
/// 2.2 m instead of adding velocity, which is the same idea their whole
/// archetype is: arrive, do not travel.
pub fn blinks(style: Style) -> bool {
    style == Style::Precision
}

/// Deterministic per-part jitter, the C's hash. Same part on the same victim
/// always flies the same way, so a replay looks like a replay.
fn jitter(part_index: usize, victim_id: u32) -> (f32, f32) {
    let h = ((part_index as u32 + 1).wrapping_mul(2_654_435_761))
        .wrapping_add(victim_id.wrapping_mul(97_561));
    let angle = ((h >> 3) & 0xFF) as f32 / 255.0 * std::f32::consts::TAU;
    let radius = 0.4 + ((h >> 11) & 0xFF) as f32 / 255.0;
    (angle, radius)
}

/// The launch velocity for one part.
///
/// `radial` is the horizontal direction from the victim's centre to that
/// part — so a scatter throws each piece outward from where it already was,
/// which is what makes a radial burst read as a burst rather than an
/// explosion at a point.
pub fn part_velocity(
    style: Style,
    part: Part,
    part_index: usize,
    victim_id: u32,
    radial: Vec3,
    your_yaw: f32,
) -> Vec3 {
    let (ja, jr) = jitter(part_index, victim_id);
    let (rx, rz) = (radial.x, radial.z);
    match style {
        Style::Merciful => Vec3::new(rx * 1.5, 1.2 + jr, rz * 1.5),
        Style::Brutal => Vec3::new(
            (rx + ja.cos() * 0.7) * 13.0,
            7.0 + jr * 8.0,
            (rz + ja.sin() * 0.7) * 13.0,
        ),
        Style::Precision => Vec3::new(rx * 7.5, 3.0 + jr * 1.5, rz * 7.5),
        Style::Surgical => Vec3::new(rx * 0.8, 5.5 + jr * 1.2, rz * 0.8),
        Style::Heavy => Vec3::new(rx * 9.0, 5.0 + jr * 4.0, rz * 9.0),
        Style::Thrash => Vec3::new(ja.cos() * 17.0, 5.0 + jr * 10.0, ja.sin() * 17.0),
        Style::Jazz => {
            // TORN ALONG YOUR SHOULDER LINE, and each half moves as ONE.
            // Coherence near 1 is the whole effect: every other style is a
            // scatter, and this one is the absence of one.
            let (hrx, hrz) = (your_yaw.cos(), -your_yaw.sin());
            let coh = 0.90 + jr * 0.20;
            if part.upper() {
                Vec3::new(hrx * 8.5 * coh, 7.5 * coh, hrz * 8.5 * coh)
            } else {
                Vec3::new(-hrx * 6.0 * coh, 3.0 * coh, -hrz * 6.0 * coh)
            }
        }
    }
}

/// What a style looks like, measured rather than described.
#[derive(Clone, Copy, Debug)]
pub struct Signature {
    /// mean launch speed
    pub speed: f32,
    /// mean upward component
    pub lift: f32,
    /// How alike the HORIZONTAL directions of one half are, 0..1.
    ///
    /// Horizontal, and one half, for a reason. Measured over the whole body
    /// in three dimensions, `Merciful` scored as coherent as a jazz rip —
    /// because its parts barely move sideways, so they all point straight up
    /// and "all going up" reads as "all going the same way". And measured
    /// over the whole body horizontally, a jazz rip scores ZERO, because its
    /// two halves cancel.
    ///
    /// What actually distinguishes the tear is that each half moves as one
    /// piece. So: the upper half, in the ground plane.
    pub coherence: f32,
}

/// Measure a style across a whole body.
pub fn signature(style: Style, victim_id: u32, your_yaw: f32) -> Signature {
    let parts = Part::all();
    let mut vs = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        // a plausible radial layout: parts spread around the body
        let a = i as f32 / parts.len() as f32 * std::f32::consts::TAU;
        let radial = Vec3::new(a.cos(), 0.0, a.sin());
        vs.push(part_velocity(style, *p, i, victim_id, radial, your_yaw));
    }
    let n = vs.len() as f32;
    let speed = vs.iter().map(|v| v.length()).sum::<f32>() / n;
    let lift = vs.iter().map(|v| v.y).sum::<f32>() / n;
    let uppers: Vec<Vec3> = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.upper())
        .map(|(i, _)| Vec3::new(vs[i].x, 0.0, vs[i].z).normalize_or_zero())
        .collect();
    let mean: Vec3 = uppers.iter().copied().sum::<Vec3>() / uppers.len().max(1) as f32;
    Signature { speed, lift, coherence: mean.length() }
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("THE RIP — six characters, no dialogue\n");
    println!("  {:<12} {:>8} {:>8} {:>11}", "style", "speed", "lift", "coherence");
    for s in [
        Style::Merciful,
        Style::Surgical,
        Style::Precision,
        Style::Heavy,
        Style::Brutal,
        Style::Thrash,
        Style::Jazz,
    ] {
        let g = signature(s, 7, 0.0);
        println!("  {:<12} {:>8.1} {:>8.1} {:>11.2}", format!("{s:?}"), g.speed, g.lift, g.coherence);
    }
    println!("\n  merciful barely moves them. thrash is the widest scatter. and JAZZ");
    println!("  is the only one with high coherence — because it is not a scatter at");
    println!("  all, it is two halves each moving as one piece.\n");

    println!("  WHO DOES WHAT\n");
    println!("  {:<12} {:>8} {:>14} {:>14}", "frame", "bulk", "vs a grunt", "vs a standard");
    for (f, bulk) in [
        (Frame::Prime, 0.85),
        (Frame::Reaver, 0.80),
        (Frame::Scout, 0.30),
        (Frame::Specialist, 0.60),
        (Frame::Vanguard, 0.90),
        (Frame::Beast, 0.88),
    ] {
        println!(
            "  {:<12} {:>8.2} {:>14} {:>14}",
            format!("{f:?}"),
            bulk,
            format!("{:?}", style_for(f, Victim::Small, bulk)),
            format!("{:?}", style_for(f, Victim::Standard, bulk))
        );
    }
    println!("\n  the override needs all three: big enough, small enough victim, and");
    println!("  not one of the two who refuse. Prime's refusal is mercy. The beast's");
    println!("  is that it already has its own idea.\n");

    println!("  AND A SCOUT DOES NOT CHARGE");
    for s in [Style::Precision, Style::Heavy] {
        println!(
            "    {:<12} {}",
            format!("{s:?}"),
            if blinks(s) { "blinks to the flank" } else { "lunges in" }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_is_distinguishable_from_every_other() {
        // six characters expressed only in velocities: if two of them measure
        // the same, one of them is not a character
        let all = [
            Style::Merciful,
            Style::Brutal,
            Style::Precision,
            Style::Surgical,
            Style::Heavy,
            Style::Thrash,
            Style::Jazz,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                let (x, y) = (signature(*a, 3, 0.0), signature(*b, 3, 0.0));
                let apart = (x.speed - y.speed).abs() / x.speed.max(y.speed)
                    + (x.lift - y.lift).abs() / x.lift.max(y.lift)
                    + (x.coherence - y.coherence).abs();
                assert!(apart > 0.12, "{a:?} and {b:?} look the same: {apart}");
            }
        }
    }

    #[test]
    fn merciful_is_the_gentlest_and_thrash_the_wildest() {
        let m = signature(Style::Merciful, 1, 0.0);
        let t = signature(Style::Thrash, 1, 0.0);
        let all = [Style::Brutal, Style::Precision, Style::Surgical, Style::Heavy, Style::Thrash];
        for s in all {
            assert!(
                signature(s, 1, 0.0).speed > m.speed,
                "{s:?} is gentler than merciful"
            );
        }
        for s in [Style::Merciful, Style::Precision, Style::Surgical, Style::Heavy] {
            assert!(signature(s, 1, 0.0).speed < t.speed, "{s:?} is wilder than thrash");
        }
    }

    #[test]
    fn jazz_is_the_only_coherent_one() {
        // the effect IS the absence of scatter. Every other style throws
        // parts every direction; this one throws two halves.
        let j = signature(Style::Jazz, 5, 0.0).coherence;
        for s in [
            Style::Merciful,
            Style::Brutal,
            Style::Precision,
            Style::Surgical,
            Style::Heavy,
            Style::Thrash,
        ] {
            assert!(
                signature(s, 5, 0.0).coherence < j,
                "{s:?} is as coherent as a jazz rip"
            );
        }
    }

    #[test]
    fn jazz_throws_two_halves_in_opposite_directions() {
        let yaw = 0.0;
        let mut upper = Vec3::ZERO;
        let mut lower = Vec3::ZERO;
        let (mut nu, mut nl) = (0.0, 0.0);
        for (i, p) in Part::all().iter().enumerate() {
            let v = part_velocity(Style::Jazz, *p, i, 2, Vec3::X, yaw);
            if p.upper() {
                upper += v;
                nu += 1.0;
            } else {
                lower += v;
                nl += 1.0;
            }
        }
        assert!(nu > 0.0 && nl > 0.0, "one half was empty");
        upper /= nu;
        lower /= nl;
        let horiz = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
        assert!(
            horiz(upper).dot(horiz(lower)) < -0.9,
            "the halves should go opposite ways: {}",
            horiz(upper).dot(horiz(lower))
        );
        assert!(upper.y > lower.y, "the upper half is hoisted, the lower is dropped");
    }

    #[test]
    fn the_tear_follows_your_shoulder_line() {
        // not the victim's facing, not the world axes — YOURS
        let a = part_velocity(Style::Jazz, Part::Chest, 0, 1, Vec3::X, 0.0);
        let b = part_velocity(
            Style::Jazz,
            Part::Chest,
            0,
            1,
            Vec3::X,
            std::f32::consts::FRAC_PI_2,
        );
        assert!(
            Vec3::new(a.x, 0.0, a.z).normalize().dot(Vec3::new(b.x, 0.0, b.z).normalize()) < 0.1,
            "turning ninety degrees should turn the tear"
        );
        // the radial direction, which every other style uses, is ignored here
        let c = part_velocity(Style::Jazz, Part::Chest, 0, 1, -Vec3::X, 0.0);
        assert!((a - c).length() < 0.01, "jazz should not care where the part was");
    }

    #[test]
    fn the_override_needs_all_three_conditions() {
        // big, small victim, and not one of the two who refuse
        assert_eq!(style_for(Frame::Vanguard, Victim::Small, 0.9), Style::Jazz);
        assert_eq!(
            style_for(Frame::Vanguard, Victim::Standard, 0.9),
            Style::Heavy,
            "a standard victim is too big to pick up"
        );
        assert_eq!(
            style_for(Frame::Vanguard, Victim::Small, 0.3),
            Style::Heavy,
            "a light frame cannot tear anything in half"
        );
        assert_eq!(
            style_for(Frame::Prime, Victim::Small, 0.95),
            Style::Merciful,
            "mercy holds"
        );
        assert_eq!(
            style_for(Frame::Beast, Victim::Small, 0.95),
            Style::Thrash,
            "the beast has its own idea"
        );
    }

    #[test]
    fn every_frame_has_its_own_signature_style() {
        use std::collections::BTreeSet;
        let s: BTreeSet<String> = [
            Frame::Prime,
            Frame::Reaver,
            Frame::Scout,
            Frame::Specialist,
            Frame::Vanguard,
            Frame::Beast,
        ]
        .iter()
        .map(|f| format!("{:?}", style_for(*f, Victim::Standard, 0.5)))
        .collect();
        assert_eq!(s.len(), 6, "two frames share a rip: {s:?}");
    }

    #[test]
    fn a_scout_arrives_rather_than_travels() {
        assert!(blinks(Style::Precision));
        for s in [Style::Merciful, Style::Brutal, Style::Surgical, Style::Heavy, Style::Thrash, Style::Jazz] {
            assert!(!blinks(s), "{s:?} should lunge");
        }
    }

    #[test]
    fn the_same_part_always_flies_the_same_way() {
        // a replay has to look like a replay
        for i in 0..Part::all().len() {
            let a = part_velocity(Style::Brutal, Part::all()[i], i, 42, Vec3::X, 0.3);
            let b = part_velocity(Style::Brutal, Part::all()[i], i, 42, Vec3::X, 0.3);
            assert_eq!(a, b);
        }
        // ... and different victims scatter differently, or every kill looks
        // identical
        let a = signature(Style::Thrash, 1, 0.0);
        let b = signature(Style::Thrash, 2, 0.0);
        assert!((a.speed - b.speed).abs() > 1e-4 || (a.lift - b.lift).abs() > 1e-4);
    }

    #[test]
    fn surgical_lifts_without_throwing() {
        // "ordered anti-grav float": almost all of the motion is upward
        let g = signature(Style::Surgical, 4, 0.0);
        assert!(g.lift > 5.0, "it should lift: {}", g.lift);
        // horizontal component is small relative to the lift
        assert!(g.speed < g.lift * 1.3, "it threw them sideways: {} vs {}", g.speed, g.lift);
    }
}
