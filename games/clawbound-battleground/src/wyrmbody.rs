//! wyrmbody -- the Moonwyrm's body: 400 u of sky-serpent, and where each
//! part of it is.
//!
//! The body is a rope of [`BODY_LEN`] laid along a track, head first.
//!
//! * **In the well** the track is the rest spine ([`rest_spine`]): the head
//!   held low over the water, the neck curving back and up into a great
//!   coil that climbs round the well, narrowing, into the sky -- a tower of
//!   serpent ~90 u tall with the tail's tip at the top. Cats fight the head;
//!   the coil passes far over theirs.
//! * **Taking wing**, the head flies its loop (`wyrmflight::pose`) and the
//!   body pays out of the coil behind it: the track is the path the head has
//!   flown, then what is left of the rest spine. So the coil unwinds from
//!   the neck end, and the body streams along the loop through the sky.
//! * **Coming home**, the head dives straight down into the well at
//!   [`DIVE_SPEED`] and the body follows it in; what has gone under the
//!   water is not drawn. Then the coil rises back out of the well over
//!   [`RISE_SECS`].
//!
//! Pure geometry: a function of the flight clock and the match clock. The
//! scene draws a segment mesh at each [`Segment`] and the head at the front.

use glam::Vec3;

/// The body's length, head to tail tip, u.
pub const BODY_LEN: f32 = 400.0;
/// Its radius at its thickest, u.
pub const BODY_R: f32 = 3.4;
/// Segments are laid this far apart along it.
pub const SEG_STEP: f32 = 2.6;
/// Where the head rests, above the well's centre.
pub const REST_HEAD: Vec3 = Vec3::new(0.0, 7.0, 0.0);
/// Diving home, the head goes down this fast (u/s).
pub const DIVE_SPEED: f32 = 40.0;
/// Rising back out of the well, coiled, takes this long.
pub const RISE_SECS: f32 = 4.0;
/// The coil: its radius at the bottom and at the top, the height its first
/// turn starts at, and how much it climbs per unit of body.
pub const COIL_R0: f32 = 21.0;
pub const COIL_R1: f32 = 9.0;
pub const COIL_Y0: f32 = 15.0;
pub const COIL_CLIMB: f32 = 0.2;
/// Where round the well the coil's first turn begins (radians, from +x).
pub const COIL_START: f32 = 2.4;
/// Length of the neck, from the head back to the coil's first turn.
pub const NECK_LEN: f32 = 34.0;

/// One piece of the body: where it is, the way it runs (toward the head),
/// how thick it is (x `BODY_R`), and whether it is out of the water.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub pos: Vec3,
    pub fwd: Vec3,
    pub r: f32,
    pub shown: bool,
}

/// Thickness along the body, `s` from the head: a slimmer neck, full through
/// the middle, tapering to a fine tail.
pub fn girth(s: f32) -> f32 {
    let u = (s / BODY_LEN).clamp(0.0, 1.0);
    let neck = 0.72 + 0.28 * (u / 0.08).min(1.0);
    let tail = 1.0 - 0.85 * ((u - 0.6) / 0.4).clamp(0.0, 1.0).powf(1.3);
    neck * tail
}

fn cubic(a: Vec3, b: Vec3, c: Vec3, d: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    a * (u * u * u) + b * (3.0 * u * u * t) + c * (3.0 * u * t * t) + d * (t * t * t)
}

/// The rest spine as a polyline, head end first: the neck (a curve from
/// the head, facing `facing`, back and up to the coil) then the coil. A
/// slow ripple runs along it (`t`, match seconds), so the resting wyrm
/// breathes and stirs.
pub fn rest_spine(home: Vec3, facing: Vec3, t: f32) -> Vec<Vec3> {
    let head = home + REST_HEAD;
    // the coil stays put (a 90 u tower cannot swing round every time the
    // head turns); the neck bends to join it
    let a0 = COIL_START;
    let coil = |sc: f32, a: f32| {
        let r = COIL_R0 + (COIL_R1 - COIL_R0) * (sc / (BODY_LEN - NECK_LEN)).clamp(0.0, 1.0);
        home + Vec3::new(a.cos() * r, COIL_Y0 + sc * COIL_CLIMB, a.sin() * r)
    };
    let c0 = coil(0.0, a0);
    let tangent0 = Vec3::new(-a0.sin(), COIL_CLIMB, a0.cos()).normalize();
    let mut pts = Vec::with_capacity(400);
    // the neck: out of the back of the head, arching up to meet the coil
    // running along it
    let (b, c) = (head - facing.normalize_or(Vec3::Z) * 12.0 + Vec3::Y * 2.0, c0 - tangent0 * 14.0);
    for k in 0..=40 {
        pts.push(cubic(head, b, c, c0, k as f32 / 40.0));
    }
    // the coil, climbing and narrowing, in steps of about a unit
    let (mut a, mut sc) = (a0, 0.0f32);
    while sc < BODY_LEN - NECK_LEN + 20.0 {
        let r = COIL_R0 + (COIL_R1 - COIL_R0) * (sc / (BODY_LEN - NECK_LEN)).clamp(0.0, 1.0);
        sc += 1.0;
        a += 1.0 / r;
        pts.push(coil(sc, a));
    }
    // the ripple: a slow wave running tailward, fading in past the neck
    let mut s = 0.0;
    let mut out = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            s += (pts[i] - pts[i - 1]).length();
        }
        let k = (s / 20.0).min(1.0);
        let w = s * 0.07 - t * 0.8;
        out.push(*p + Vec3::new(w.sin() * 0.9, (w * 0.7).cos() * 0.7, w.cos() * 0.9) * k);
    }
    out
}

/// Walk `track` (a polyline) and lay segments every `SEG_STEP` from its
/// start, for the body's length. `hidden` says which points are under the
/// water.
fn lay(track: &[Vec3], hidden: &dyn Fn(Vec3) -> bool) -> Vec<Segment> {
    let mut out = Vec::new();
    if track.len() < 2 {
        return out;
    }
    let mut i = 0usize;
    let mut into = 0.0f32; // distance past track[i]
    let mut s = 0.0f32;
    while s <= BODY_LEN {
        // advance to the point at distance s
        while i + 1 < track.len() && into > (track[i + 1] - track[i]).length() {
            into -= (track[i + 1] - track[i]).length();
            i += 1;
        }
        if i + 1 >= track.len() {
            break;
        }
        let seg = track[i + 1] - track[i];
        let len = seg.length().max(1e-4);
        let p = track[i] + seg * (into / len).min(1.0);
        out.push(Segment { pos: p, fwd: -seg / len, r: girth(s), shown: !hidden(p) });
        s += SEG_STEP;
        into += SEG_STEP;
    }
    out
}

/// The whole body now. `flight`: seconds into a flight, if flying
/// (`Boss::flight`); `since_land`: seconds since it last came home (for
/// the rise); `facing`: the way the resting head looks; `t`: match seconds.
pub fn segments(home: Vec3, flight: Option<f32>, since_land: f32, facing: Vec3, t: f32) -> Vec<Segment> {
    use crate::wyrmflight::{pose, FLIGHT_SECS};
    let under = |p: Vec3| p.y < home.y - 1.0 && Vec3::new(p.x - home.x, 0.0, p.z - home.z).length() < crate::sky::POOL_R;
    let rest = rest_spine(home, facing, t);
    let Some(secs) = flight else {
        // coiled; rising back out of the well for a few seconds after a dive
        let k = (since_land / RISE_SECS).clamp(0.0, 1.0);
        let sink = (1.0 - k * k * (3.0 - 2.0 * k)) * (COIL_Y0 + BODY_LEN * COIL_CLIMB + 10.0);
        let track: Vec<Vec3> = rest.iter().map(|p| *p - Vec3::Y * sink).collect();
        return lay(&track, &under);
    };
    // the path the head has flown, newest first, sampled every 0.05 s
    let mut track = Vec::with_capacity(1600);
    if secs > FLIGHT_SECS {
        // the dive: straight down from the resting point
        let head = home + REST_HEAD;
        let down = (secs - FLIGHT_SECS) * DIVE_SPEED;
        let mut d = down;
        while d > 0.0 {
            track.push(head - Vec3::Y * d);
            d -= 2.0;
        }
        track.push(head);
    }
    let mut s = secs.min(FLIGHT_SECS);
    while s > 0.0 {
        track.push(pose(home, s).0);
        s -= 0.05;
    }
    // then what is still coiled: the rest spine, from the neck end
    track.extend(rest.iter().copied());
    lay(&track, &under)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_len(v: &[Segment]) -> f32 {
        v.windows(2).map(|w| (w[1].pos - w[0].pos).length()).sum()
    }

    #[test]
    fn a_coiled_tower_over_the_well_with_the_head_low() {
        crate::map::with(crate::map::MapId::Sky, || {
            let home = crate::sky::well_pos();
            let v = segments(home, None, 100.0, Vec3::Z, 3.0);
            assert_eq!(v.len(), (BODY_LEN / SEG_STEP) as usize + 1);
            let len = body_len(&v);
            assert!((len - BODY_LEN).abs() < 12.0, "the whole body is laid: {len}");
            assert!(v[0].pos.distance(home + REST_HEAD) < 1.5, "the head rests low over the well");
            let top = v.iter().map(|s| s.pos.y).fold(f32::MIN, f32::max);
            assert!((70.0..110.0).contains(&top), "the coil climbs into the sky: {top}");
            // the coil passes over the cats' heads: nothing past the neck is
            // lower than 10 u
            assert!(v.iter().skip((NECK_LEN / SEG_STEP) as usize + 2).all(|s| s.pos.y > 10.0));
            assert!(v.iter().all(|s| s.shown));
            // thick in the middle, fine at the tail
            assert!((v[v.len() / 2].r - 1.0).abs() < 0.05 && v.last().unwrap().r < 0.25);
        });
    }

    #[test]
    fn taking_wing_it_pays_out_along_the_path() {
        crate::map::with(crate::map::MapId::Sky, || {
            let home = crate::sky::well_pos();
            let early = segments(home, Some(6.0), 0.0, Vec3::Z, 0.0);
            let mid = segments(home, Some(35.0), 0.0, Vec3::Z, 0.0);
            let (h, _, _) = crate::wyrmflight::pose(home, 35.0);
            assert!(mid[0].pos.distance(h) < 1.0, "the head leads");
            // midway, all of it is out on the loop: far from the well
            assert!(mid.iter().all(|s| Vec3::new(s.pos.x, 0.0, s.pos.z).length() > 10.0 || s.pos.y < 40.0));
            let spread = |v: &[Segment]| v.iter().map(|s| Vec3::new(s.pos.x, 0.0, s.pos.z).length()).fold(0.0f32, f32::max);
            assert!(spread(&mid) > 150.0);
            // all of it, all the time: the part not yet out is still coiled
            assert_eq!(early.len(), mid.len());
            // early, the tail is still in the coil
            let tail = early.last().unwrap().pos;
            assert!(Vec3::new(tail.x, 0.0, tail.z).length() < COIL_R0 + 3.0, "tail still coiled: {tail}");
            // no gaps anywhere along it
            for v in [&early, &mid] {
                assert!(v.windows(2).all(|w| (w[1].pos - w[0].pos).length() < SEG_STEP * 1.6));
            }
        });
    }

    #[test]
    fn it_dives_home_and_all_of_it_goes_under() {
        crate::map::with(crate::map::MapId::Sky, || {
            use crate::wyrmflight::{DIVE_SECS, FLIGHT_SECS};
            let home = crate::sky::well_pos();
            let diving = segments(home, Some(FLIGHT_SECS + 3.0), 0.0, Vec3::Z, 0.0);
            assert!(!diving[0].shown && diving.last().unwrap().shown, "the head is under, the tail still out");
            let done = segments(home, Some(FLIGHT_SECS + DIVE_SECS - 0.01), 0.0, Vec3::Z, 0.0);
            assert!(done.iter().all(|s| !s.shown), "all of it is in the well by the end of the dive");
            // and it rises coiled
            let rising = segments(home, None, RISE_SECS * 0.5, Vec3::Z, 0.0);
            let risen = segments(home, None, RISE_SECS, Vec3::Z, 0.0);
            assert!(rising[0].pos.y < risen[0].pos.y - 5.0);
        });
    }
}
