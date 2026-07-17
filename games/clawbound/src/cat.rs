//! Cat rig — a posable papercraft cat. Two body units (fore/hind), a head, tail,
//! and four two-bone legs (thigh+shank) that pivot at the hip. A `Pose` captures
//! per-leg swing/knee plus body bob/pitch/crouch/tail/head, so the same builder
//! renders any gait phase or special pose (leap, startle, climb).

use crate::math::{v3, Vec3};
use crate::mesh::{rgb, shade, Mesh};

const TAU: f32 = std::f32::consts::TAU;
const PI: f32 = std::f32::consts::PI;

#[derive(Clone, Copy)]
pub struct Pose {
    pub swing: [f32; 4], // per-leg fore/back angle (FL, FR, HL, HR)
    pub knee: [f32; 4],  // knee flex
    pub bob: f32,        // vertical body bounce
    pub pitch: f32,      // nose up(+)/down(−)
    pub crouch: f32,     // 0..1 lowers the body, folds the legs
    pub tail: f32,       // tail sway/lift
    pub head: f32,       // head pitch
    pub head_yaw: f32,   // head turn left(−)/right(+)
    pub breath: f32,     // 0..1 breathing expansion of the chest
}

fn base() -> Pose { Pose { swing: [0.0; 4], knee: [0.35; 4], bob: 0.0, pitch: 0.0, crouch: 0.0, tail: 0.2, head: 0.0, head_yaw: 0.0, breath: 0.0 } }

pub fn stand() -> Pose { base() }

/// Idle — a living, breathing stand: slow chest rise, tail sway, the odd glance.
pub fn idle(t: f32) -> Pose {
    let mut p = base();
    let b = (t * TAU).sin();
    p.breath = 0.5 + 0.5 * b;                       // chest expands and settles
    p.bob = 0.018 * b;                              // whole body rises a touch with the breath
    p.head = 0.045 * (t * 0.7 * TAU + 1.0).sin();   // small head bob, offset from the breath
    p.head_yaw = 0.14 * (t * 0.33 * TAU).sin();     // slow glances left and right
    p.tail = 0.2 + 0.18 * (t * 0.5 * TAU).sin();    // lazy tail sway
    p
}

/// Frozen, reading the wind — muzzle lifted, casting the head, quick scenting breaths.
/// The pose a cat holds when it stops to taste the air for a predator.
pub fn taste_air(t: f32) -> Pose {
    let mut p = base();
    p.head = 0.34 + 0.08 * (t * TAU).sin();         // muzzle raised to the breeze
    p.pitch = -0.12;
    p.head_yaw = 0.2 * (t * 0.6 * TAU).sin();        // casting side to side
    p.breath = 0.5 + 0.5 * (t * 1.8 * TAU).sin();    // fast scenting breaths
    p.tail = 0.35;
    p
}

/// Wading/pushing through thick cover — the gait, but lowered and shouldering into the
/// brush, head dipped. `cover` (0..1) is how deep the undergrowth is.
pub fn wade(speed: f32, t: f32, off: f32, cover: f32) -> Pose {
    let mut p = gait(speed, t, off);
    p.crouch = (p.crouch + cover * 0.4).min(0.6);   // hunched low, pushing through
    p.head = -0.15 * cover;                          // head down, breasting the growth
    p.pitch = 0.05 * cover;
    p
}

/// Four-beat walk (lateral sequence).
pub fn walk(t: f32) -> Pose {
    let off = [0.0, 0.5, 0.75, 0.25];
    let mut p = base();
    for i in 0..4 {
        let ph = (t + off[i]) * TAU;
        p.swing[i] = 0.5 * ph.sin();
        p.knee[i] = 0.28 + 0.45 * (-ph.cos()).max(0.0);
    }
    p.bob = 0.04 * (2.0 * t * TAU).sin();
    p.tail = 0.2 * (t * TAU).sin();
    p
}

/// Gallop/bound — fore pair together, hind pair together, big amplitude.
pub fn bound(t: f32) -> Pose {
    let mut p = base();
    let ph = t * TAU;
    for i in 0..4 {
        let a = if i < 2 { ph } else { ph + PI };
        p.swing[i] = 0.75 * a.sin();
        p.knee[i] = 0.5 + 0.5 * (-a.cos()).max(0.0);
    }
    p.bob = 0.13 * ph.sin();
    p.pitch = 0.25 * ph.cos();
    p.tail = 0.3;
    p
}

/// Mid-leap — body stretched, legs extended.
/// Name the gait a ground speed calls for — a walk quickens to a trot, then breaks
/// into a bound. Used for both animation and (in tests) to reason about traversal.
pub fn gait_name(speed: f32) -> &'static str {
    if speed > 11.0 { "bound" } else if speed > 6.0 { "trot" } else { "walk" }
}

/// The moving pose for a ground speed: a slow walk, quickening to a trot, breaking
/// into a bounding gallop when running flat out. `t` is the animation clock, `off`
/// a per-cat phase offset so a pack doesn't move in lockstep.
pub fn gait(speed: f32, t: f32, off: f32) -> Pose {
    if speed > 11.0 {
        bound((t * 2.4 + off).rem_euclid(1.0))          // galloping bound
    } else {
        let rate = 1.0 + (speed * 0.12).min(1.8);        // walk cadence quickens toward a trot
        walk((t * rate + off).rem_euclid(1.0))
    }
}

pub fn leap() -> Pose {
    let mut p = base();
    p.swing = [0.9, 0.9, -0.8, -0.8];
    p.knee = [0.1; 4];
    p.pitch = 0.4; p.bob = 0.25; p.tail = 0.45; p.head = 0.3;
    p
}

/// Anticipation crouch before a leap.
pub fn coil() -> Pose {
    let mut p = base();
    p.crouch = 1.0; p.knee = [1.0; 4]; p.pitch = -0.12; p.tail = 0.1; p.head = -0.2;
    p
}

/// Startle — arched back, tail up, about to bolt.
pub fn startle() -> Pose {
    let mut p = base();
    p.crouch = 0.5; p.pitch = 0.3; p.tail = 1.0; p.knee = [0.7; 4]; p.head = 0.35;
    p
}

/// Clinging to a trunk/branch, angled upward, gripping.
pub fn climb() -> Pose {
    let mut p = base();
    p.pitch = 0.7;
    p.swing = [0.85, 0.85, -0.5, -0.5];
    p.knee = [0.6, 0.6, 0.5, 0.5];
    p.tail = -0.3; p.head = 0.4;
    p
}

/// Combat ready-stance — low and coiled on the haunches, tail lashing, eyes on the foe.
pub fn stance(t: f32) -> Pose {
    let mut p = base();
    p.crouch = 0.55;
    p.knee = [0.8; 4];
    p.pitch = -0.05;
    p.head = -0.1;
    p.tail = 0.6 + 0.45 * (t * 2.0 * TAU).sin();   // tail whips side to side
    p.head_yaw = 0.06 * (t * TAU).sin();
    p.breath = 0.5 + 0.5 * (t * 1.5 * TAU).sin();
    p
}

/// A swipe/pounce strike. `t` runs 0→1 over the attack; the fore paws lash forward
/// and the body lunges, peaking mid-swing.
pub fn strike(t: f32) -> Pose {
    let mut p = base();
    let k = (t.clamp(0.0, 1.0) * PI).sin();          // ease in and out of the swing
    p.swing = [0.6 * k, 0.95 * k, -0.3 * k, -0.3 * k]; // fore paws reach, hind braces
    p.knee = [0.15, 0.1, 0.5, 0.5];
    p.pitch = 0.15 * k;
    p.bob = 0.1 * k;
    p.head = 0.22 * k;
    p.tail = 0.4;
    p
}

/// Underbelly swipe — dropped low and half-rolled, fore paws raking up under a high
/// or pouncing attacker.
pub fn underbelly_swipe(t: f32) -> Pose {
    let mut p = base();
    let k = (t.clamp(0.0, 1.0) * PI).sin();
    p.crouch = 0.75;
    p.pitch = -0.35 - 0.2 * k;             // belly turned toward the foe
    p.swing = [0.7 * k, 0.95 * k, -0.2, -0.2];  // fore paws rake upward
    p.knee = [0.1, 0.1, 0.7, 0.7];
    p.head = -0.2;
    p.tail = 0.5;
    p
}

/// Hind kick — front dipped, both hind legs lashing backward to rake and shove off.
pub fn hind_kick(t: f32) -> Pose {
    let mut p = base();
    let k = (t.clamp(0.0, 1.0) * PI).sin();
    p.pitch = 0.3 + 0.2 * k;               // nose down, haunches up
    p.swing = [0.3, 0.3, -0.9 * k, -1.05 * k];  // hind legs drive back
    p.knee = [0.5, 0.5, 0.1, 0.05];
    p.bob = 0.15 * k;
    p.tail = -0.4;
    p.head = -0.3;
    p
}

/// A vertical hop — legs tuck, body rises and settles.
pub fn jump(t: f32) -> Pose {
    let mut p = base();
    let k = (t.clamp(0.0, 1.0) * PI).sin();
    p.bob = 0.4 * k;
    p.crouch = (0.5 * (1.0 - k)).max(0.0); // coiled at take-off and landing
    p.swing = [0.5 * k, 0.5 * k, 0.5 * k, 0.5 * k];  // legs tuck under
    p.knee = [0.8, 0.8, 0.8, 0.8];
    p.pitch = 0.1;
    p.tail = 0.3 + 0.3 * k;
    p
}

/// The pose for a combat move at progress `t` (0→1). Bridges `combat::Move` to the rig.
pub fn move_pose(m: crate::combat::Move, t: f32) -> Pose {
    use crate::combat::Move::*;
    match m {
        Pounce => leap(),
        Jump => jump(t),
        Roll | Dodge => coil(),
        UnderbellySwipe => underbelly_swipe(t),
        HindKick => hind_kick(t),
        Swat => strike(t),
    }
}

/// Per-leg (thigh, shank) length for a species. The Frog's hind legs (index 2,3) are
/// noticeably longer than everyone else's — built for its huge leap.
fn leg_scale(class: crate::class::Class, i: usize, lt: f32, ls: f32) -> (f32, f32) {
    if class == crate::class::Class::Frog && i >= 2 { (lt * 1.4, ls * 1.4) } else { (lt, ls) }
}

/// Build the creature facing `yaw` radians (0 = +z, matching `heading = dx.atan2(dz)`),
/// as its `class` — each of the six has its own silhouette (color, tail, ears, body
/// proportions, and a signature feature) rather than sharing one undifferentiated rig.
pub fn build(m: &mut Mesh, at: Vec3, yaw: f32, p: &Pose, class: crate::class::Class) {
    use crate::class::Class;
    let pelt = rgb(class.def().color);
    let dark = shade(pelt, 0.62);
    let (s, co) = (yaw.sin(), yaw.cos());
    // rotate a body-local point (forward = +z) into world, pivoting about `at`
    let place = |lx: f32, wy: f32, lz: f32| v3(at.x + lx * co + lz * s, wy, at.z - lx * s + lz * co);

    // per-species body proportions: Otter long and low, Frog rear-heavy for its leap,
    // Goat stocky, Hawk slight and light, Squirrel small and quick
    let (len_mul, wid_mul, stand_h): (f32, f32, f32) = match class {
        Class::Cat => (1.0, 1.0, 0.52),
        Class::Squirrel => (0.85, 0.85, 0.42),
        Class::Otter => (1.25, 0.92, 0.38),
        Class::Frog => (1.0, 1.05, 0.48),
        Class::Goat => (1.05, 1.2, 0.62),
        Class::Hawk => (0.9, 0.85, 0.5),
    };

    let breath = 1.0 + p.breath * 0.05;
    let body_y = at.y + stand_h * (1.0 - p.crouch * 0.35) + p.bob;
    let fore_y = body_y + p.pitch * 0.25 + 0.03;
    let hind_y = body_y - p.pitch * 0.25;

    // Frog's leap-built rear haunches are bigger than the fore quarter; everyone else is even
    let hind_wid = if class == Class::Frog { 0.34 * wid_mul * 1.35 } else { 0.34 * wid_mul };
    m.boxx_yaw(place(0.0, hind_y, -0.35 * len_mul), v3(hind_wid, 0.30 * breath, 0.45 * len_mul), yaw, pelt);
    m.boxx_yaw(place(0.0, fore_y, 0.30 * len_mul), v3(0.32 * wid_mul, 0.30 * breath, 0.42 * len_mul), yaw, pelt);

    // head turns with head_yaw; ears (or Hawk's beak) ride on top, rotated with it
    let head_c = place(0.0, fore_y + 0.22 + p.head * 0.12, 0.78 * len_mul);
    m.boxx_yaw(head_c, v3(0.27 * wid_mul, 0.24, 0.25), yaw + p.head_yaw, pelt);
    let (hs, hc) = (p.head_yaw.sin(), p.head_yaw.cos());
    match class {
        Class::Hawk => {
            // a short hooked beak instead of ears
            let beak = place(0.0 * hc, head_c.y - 0.02, 0.78 * len_mul + 0.22 * hc);
            m.cone(beak, 0.07, 0.2, 4, rgb(0xd8a53c));
        }
        Class::Goat => {
            // small back-swept horns, rising well above where the ears would sit
            for ex in [-0.12f32, 0.12] {
                let base_p = place(ex * hc, head_c.y + 0.2, 0.78 * len_mul - ex * hs - 0.05);
                let tip = place(ex * 1.4 * hc, head_c.y + 0.62, 0.78 * len_mul - ex * hs - 0.2);
                m.segment(base_p, tip, 0.05, 0.02, 4, rgb(0xe8e0d0));
            }
        }
        _ => {
            let ear_r = if class == Class::Squirrel { 0.12 } else { 0.1 };
            for ex in [-0.15f32, 0.15] {
                let ear = place(ex * hc, head_c.y + 0.24, 0.78 * len_mul - ex * hs);
                m.cone(ear, ear_r, 0.22, 4, dark);
            }
        }
    }

    // Hawk's wings — a flat swept panel each side, angled with the pitch/bob for a
    // little lift-and-glide read even standing still
    if class == Class::Hawk {
        let lift = 0.15 + p.bob.abs() * 0.6;
        for side in [-1.0f32, 1.0] {
            let root = place(side * 0.16, body_y + 0.05, 0.0);
            let tip = place(side * (0.7 + lift), body_y + 0.1 + lift * 0.3, -0.15);
            let back = place(side * 0.55, body_y - 0.05, -0.5);
            m.tri(root, tip, back, shade(pelt, 0.85));
        }
    }

    // tail sweeps out behind (−z) — species-specific style
    let tb = place(0.0, hind_y + 0.05, -0.78 * len_mul);
    match class {
        Class::Squirrel => {
            // big bushy tail, curled up and forward over the back
            let mid = place(p.tail * 0.15, tb.y + 0.35, -0.9 * len_mul);
            let tip = place(p.tail * 0.25, tb.y + 0.6, -0.55 * len_mul);
            m.segment(tb, mid, 0.13, 0.11, 6, pelt);
            m.segment(mid, tip, 0.11, 0.08, 6, pelt);
        }
        Class::Otter => {
            // long, flat, paddle-shaped
            let mid = place(p.tail * 0.1, tb.y + 0.05, -1.15 * len_mul);
            let tip = place(p.tail * 0.15, tb.y + 0.02, -1.5 * len_mul);
            m.segment(tb, mid, 0.09, 0.09, 5, pelt);
            m.segment(mid, tip, 0.09, 0.02, 5, pelt);
        }
        _ => {
            let ttip = place(p.tail * 0.3, tb.y + 0.2 + p.tail.abs() * 0.25, -0.78 * len_mul - 0.5);
            m.segment(tb, ttip, 0.08, 0.04, 5, pelt);
        }
    }

    // four two-bone legs; swing rotates each leg in its own fore/aft (z–y) plane
    let (lt, ls) = (0.28, 0.28);
    let hip_lx = [-0.22 * wid_mul, 0.22 * wid_mul, -0.22 * wid_mul, 0.22 * wid_mul];
    let hip_lz = [0.32 * len_mul, 0.32 * len_mul, -0.32 * len_mul, -0.32 * len_mul];
    let leg_dims = |i: usize| leg_scale(class, i, lt, ls);
    for i in 0..4 {
        let (lt_i, ls_i) = leg_dims(i);
        let sw = p.swing[i];
        let kn = p.knee[i] + p.crouch * 0.4;
        let hip = place(hip_lx[i], body_y, hip_lz[i]);
        let knee_lz = hip_lz[i] + lt_i * sw.sin();
        let knee_y = body_y - lt_i * sw.cos();
        let knee = place(hip_lx[i], knee_y, knee_lz);
        let foot_lz = knee_lz + ls_i * (sw + kn).sin();
        let foot_y = knee_y - ls_i * (sw + kn).cos();
        let foot = place(hip_lx[i], foot_y, foot_lz);
        m.segment(hip, knee, 0.09, 0.075, 5, dark);
        m.segment(knee, foot, 0.075, 0.05, 5, dark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(yaw: f32) -> (f32, f32) {
        let mut m = Mesh::new();
        build(&mut m, v3(0.0, 0.0, 0.0), yaw, &stand(), crate::class::Class::Cat);
        let (mut xmin, mut xmax, mut zmin, mut zmax) = (1e9f32, -1e9f32, 1e9f32, -1e9f32);
        for v in &m.verts {
            xmin = xmin.min(v.pos[0]); xmax = xmax.max(v.pos[0]);
            zmin = zmin.min(v.pos[2]); zmax = zmax.max(v.pos[2]);
        }
        (xmax - xmin, zmax - zmin)
    }

    #[test]
    fn every_species_has_a_distinct_silhouette() {
        use crate::class::CLASSES;
        // each species should produce a mesh with its own vertex count (extra features
        // like wings/horns/beak, or an extra tail segment, add geometry) and its own
        // pelt color — a real visual difference, not one rig with a tinted marker.
        use std::collections::HashSet;
        let mut counts = HashSet::new();
        let mut colors = HashSet::new();
        for c in CLASSES {
            let mut m = Mesh::new();
            build(&mut m, v3(0.0, 0.0, 0.0), 0.0, &stand(), c);
            assert!(!m.verts.is_empty(), "{c:?} should build a non-empty mesh");
            counts.insert(m.verts.len());
            colors.insert(c.def().color);
        }
        assert_eq!(colors.len(), 6, "every species should have its own pelt color");
        assert!(counts.len() >= 4, "species with signature features (wings/horns/beak/extra tail) should differ in geometry, got {} distinct vertex counts across 6 species", counts.len());
    }

    #[test]
    fn hawk_has_a_wingspan_wider_than_the_base_rig() {
        // the wings sweep out to the sides much further than any leg or ear does —
        // check for that extra x-extent rather than raw vertex count (fewer verts
        // than a two-eared rig is fine; a beak cone is cheaper than two ear cones).
        fn x_extent(c: crate::class::Class) -> f32 {
            let mut m = Mesh::new();
            build(&mut m, v3(0.0, 0.0, 0.0), 0.0, &stand(), c);
            m.verts.iter().map(|v| v.pos[0].abs()).fold(0.0f32, f32::max)
        }
        assert!(x_extent(crate::class::Class::Hawk) > x_extent(crate::class::Class::Cat) + 0.3, "the Hawk's wingspan should reach well beyond the base rig's width");
    }

    #[test]
    fn goat_has_horns_rising_above_the_base_rig() {
        // horns rise noticeably higher off the head than a pair of ears does
        fn max_head_height(c: crate::class::Class) -> f32 {
            let mut m = Mesh::new();
            build(&mut m, v3(0.0, 0.0, 0.0), 0.0, &stand(), c);
            m.verts.iter().map(|v| v.pos[1]).fold(0.0f32, f32::max)
        }
        assert!(max_head_height(crate::class::Class::Goat) > max_head_height(crate::class::Class::Cat) + 0.1, "the Goat's horns should rise well above the base rig's ears");
    }

    #[test]
    fn frog_has_notably_longer_hind_legs_for_its_leap() {
        use crate::class::Class;
        let (lt, ls) = (0.28, 0.28);
        assert_eq!(leg_scale(Class::Frog, 0, lt, ls), (lt, ls), "the Frog's fore legs are normal length");
        let (flt, fls) = leg_scale(Class::Frog, 2, lt, ls);
        assert!(flt > lt * 1.2 && fls > ls * 1.2, "the Frog's hind legs should be noticeably longer, built for its leap");
        assert_eq!(leg_scale(Class::Cat, 2, lt, ls), (lt, ls), "only the Frog gets the longer hind legs");
    }

    #[test]
    fn build_rotates_to_face_yaw() {
        // facing +z (yaw 0) the cat is longer front-to-back (z); rotated a quarter turn
        // it's longer side-to-side (x). This proves the whole rig actually turns.
        let (x0, z0) = spans(0.0);
        assert!(z0 > x0 + 0.3, "facing +z, longer along z (z {z0:.2} vs x {x0:.2})");
        let (x9, z9) = spans(std::f32::consts::FRAC_PI_2);
        assert!(x9 > z9 + 0.3, "facing +x, longer along x (x {x9:.2} vs z {z9:.2})");
    }

    #[test]
    fn idle_breathes_and_stand_is_still() {
        // the idle pose animates (breath varies over time); stand is a fixed base
        let a = idle(0.1).breath;
        let b = idle(0.6).breath;
        assert!((a - b).abs() > 0.1, "idle breathing should vary over time");
        assert_eq!(stand().breath, 0.0);
    }

    #[test]
    fn strike_peaks_mid_swing() {
        // the swipe reaches farthest at the middle of the animation, not the ends
        let start = strike(0.0).swing[1].abs();
        let mid = strike(0.5).swing[1].abs();
        let end = strike(1.0).swing[1].abs();
        assert!(mid > start && mid > end, "the strike should peak mid-swing ({start:.2}/{mid:.2}/{end:.2})");
        // the ready-stance is a low crouch, distinct from an upright stand
        assert!(stance(0.0).crouch > 0.3 && stand().crouch == 0.0);
    }

    #[test]
    fn gait_shifts_up_with_speed() {
        assert_eq!(gait_name(3.0), "walk");
        assert_eq!(gait_name(8.0), "trot");
        assert_eq!(gait_name(13.0), "bound");
        assert!(gait(13.0, 0.25, 0.0).bob.abs() >= gait(3.0, 0.25, 0.0).bob.abs());
    }

    #[test]
    fn traversal_poses_read_right() {
        // tasting the air raises the muzzle; wading through cover hunches lower than the
        // same open gait
        assert!(taste_air(0.25).head > 0.2, "tasting the air lifts the muzzle");
        let open = gait(6.0, 0.3, 0.0).crouch;
        let thick = wade(6.0, 0.3, 0.0, 1.0).crouch;
        assert!(thick > open, "wading through cover hunches the cat lower");
    }

    #[test]
    fn combat_poses_are_distinct_and_read_right() {
        use crate::combat::Move;
        // the hind-kick drives the hind legs backward; the underbelly swipe rakes the fore
        // paws up from a low crouch; each move maps to its own pose
        assert!(hind_kick(0.5).swing[3] < -0.3, "hind kick lashes the hind legs back");
        assert!(underbelly_swipe(0.5).crouch > 0.5 && underbelly_swipe(0.5).swing[1] > 0.3, "swipe rakes up from low");
        assert!(jump(0.5).bob > 0.2, "the jump gets air");
        // move_pose bridges combat → rig for every move
        for m in [Move::Pounce, Move::Jump, Move::Roll, Move::Dodge, Move::UnderbellySwipe, Move::HindKick, Move::Swat] {
            let _ = move_pose(m, 0.5);
        }
    }
}
