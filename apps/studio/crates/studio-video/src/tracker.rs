//! tracker — pin layers to something in the shot: a rig of track points.
//!
//! The opposite of "track in frame" (where the FRAME follows a subject, see
//! `action::follow_path`): here the subject stays where it is and later
//! layers follow IT -- dust around a figure, a glow on a face, a patch over a
//! logo. The layers live in the rig's own space, which is unbounded: they can
//! reach past the frame edge and come back in as the subject moves.
//!
//! **A shared chain.** Several points are tracked, but they are not separate
//! answers: they share one rigid transform (translation, rotation, uniform
//! scale), solved each frame by least squares from whichever points are
//! visible. That is what makes the rig survive points DISAPPEARING -- a hand
//! crossing a face, a head turning away. A point whose match goes bad is
//! dropped for that frame; the rig is solved from the rest; the lost point's
//! position is then PREDICTED from the rig and searched for there, so it is
//! re-acquired when it reappears instead of being lost for good.
//!
//! With fewer than two points visible the rig holds its last transform and
//! the frame is marked, rather than inventing motion.

use crate::slate_ocr::Gray;

/// A similarity transform: reference-frame point -> this frame's point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rig { pub tx: f32, pub ty: f32, pub rot: f32, pub scale: f32 }

impl Rig {
    pub const IDENTITY: Rig = Rig { tx: 0.0, ty: 0.0, rot: 0.0, scale: 1.0 };
    /// Where a point given in the REFERENCE frame is in this frame.
    pub fn apply(&self, p: (f32, f32)) -> (f32, f32) {
        let (c, s) = (self.rot.cos() * self.scale, self.rot.sin() * self.scale);
        (c * p.0 - s * p.1 + self.tx, s * p.0 + c * p.1 + self.ty)
    }
}

/// Least-squares similarity from reference points to observed points
/// (Umeyama, 2D). `None` with fewer than two pairs.
pub fn solve(pairs: &[((f32, f32), (f32, f32))]) -> Option<Rig> {
    if pairs.len() < 2 { return None; }
    let n = pairs.len() as f32;
    let (mut ax, mut ay, mut bx, mut by) = (0.0, 0.0, 0.0, 0.0);
    for (a, b) in pairs { ax += a.0; ay += a.1; bx += b.0; by += b.1; }
    let (ax, ay, bx, by) = (ax / n, ay / n, bx / n, by / n);
    let (mut sxx, mut sxy, mut va) = (0.0f32, 0.0f32, 0.0f32);
    for (a, b) in pairs {
        let (pa, qa) = (a.0 - ax, a.1 - ay);
        let (pb, qb) = (b.0 - bx, b.1 - by);
        sxx += pa * pb + qa * qb;       // cos component
        sxy += pa * qb - qa * pb;       // sin component
        va += pa * pa + qa * qa;
    }
    if va < 1e-6 { return None; }
    let rot = sxy.atan2(sxx);
    let scale = (sxx * sxx + sxy * sxy).sqrt() / va;
    let (c, s) = (rot.cos() * scale, rot.sin() * scale);
    Some(Rig { tx: bx - (c * ax - s * ay), ty: by - (s * ax + c * ay), rot, scale })
}

/// Sum of absolute differences between a patch around `a` in `fa` and around
/// `b` in `fb` (mean per pixel); `None` when either patch leaves the frame.
fn sad(fa: &Gray, a: (i32, i32), fb: &Gray, b: (i32, i32), r: i32) -> Option<f32> {
    let inside = |g: &Gray, p: (i32, i32)| p.0 - r >= 0 && p.1 - r >= 0 && p.0 + r < g.w as i32 && p.1 + r < g.h as i32;
    if !inside(fa, a) || !inside(fb, b) { return None; }
    let mut s = 0u32;
    for dy in -r..=r { for dx in -r..=r {
        let va = fa.px[((a.1 + dy) as usize) * fa.w + (a.0 + dx) as usize] as i32;
        let vb = fb.px[((b.1 + dy) as usize) * fb.w + (b.0 + dx) as usize] as i32;
        s += (va - vb).unsigned_abs();
    }}
    Some(s as f32 / ((2 * r + 1) * (2 * r + 1)) as f32)
}

/// One frame of the rig's solve.
#[derive(Debug, Clone, PartialEq)]
pub struct Solved {
    pub rig: Rig,
    /// Each point's position this frame, or `None` if it was not seen.
    pub points: Vec<Option<(f32, f32)>>,
    /// Fewer than two points were visible: the rig was held, not solved.
    pub held: bool,
}

/// Track `points` (given in frame 0) through `frames` as one rig.
///
/// Each point is matched against its ORIGINAL appearance in frame 0 (no
/// drift from re-templating), searched within `search` px of where the rig
/// predicts it; a match worse than `max_error` (mean abs difference per
/// pixel) means the point is not visible this frame.
pub fn track(frames: &[Gray], points: &[(f32, f32)], patch: i32, search: i32, max_error: f32) -> Vec<Solved> {
    let mut out = Vec::with_capacity(frames.len());
    if frames.is_empty() { return out; }
    out.push(Solved { rig: Rig::IDENTITY, points: points.iter().map(|p| Some(*p)).collect(), held: false });
    let mut rig = Rig::IDENTITY;
    let mut velocity = (0.0f32, 0.0f32);
    for f in &frames[1..] {
        // predict: last rig plus its last step, so a quick move is not outrun
        let predicted = Rig { tx: rig.tx + velocity.0, ty: rig.ty + velocity.1, ..rig };
        let mut seen = Vec::with_capacity(points.len());
        for &p in points {
            let guess = predicted.apply(p);
            let (gx, gy) = (guess.0.round() as i32, guess.1.round() as i32);
            let a = (p.0.round() as i32, p.1.round() as i32);
            let mut best: Option<(f32, (i32, i32))> = None;
            for dy in -search..=search { for dx in -search..=search {
                if let Some(e) = sad(&frames[0], a, f, (gx + dx, gy + dy), patch) {
                    if best.map(|b| e < b.0).unwrap_or(true) { best = Some((e, (gx + dx, gy + dy))); }
                }
            }}
            seen.push(best.filter(|b| b.0 <= max_error).map(|b| (b.1 .0 as f32, b.1 .1 as f32)));
        }
        let pairs: Vec<_> = points.iter().zip(&seen).filter_map(|(p, s)| s.map(|s| (*p, s))).collect();
        // reject a point that disagrees with the rest: a match that is
        // "good" but in the wrong place (a similar texture elsewhere)
        let solved = solve(&pairs).map(|first| {
            let keep: Vec<_> = pairs.iter().copied().filter(|(p, s)| {
                let q = first.apply(*p); (q.0 - s.0).hypot(q.1 - s.1) < patch as f32
            }).collect();
            if keep.len() >= 2 && keep.len() < pairs.len() { solve(&keep).unwrap_or(first) } else { first }
        });
        let held = solved.is_none();
        let new = solved.unwrap_or(predicted);
        velocity = (new.tx - rig.tx, new.ty - rig.ty);
        if held { velocity = (0.0, 0.0); }
        rig = if held { rig } else { new };
        let points_now = points.iter().zip(&seen).map(|(p, s)| s.map(|_| rig.apply(*p))).collect();
        out.push(Solved { rig, points: points_now, held });
    }
    out
}

/// Choose `n` points worth tracking within `radius` of `around`: the most
/// textured patches (highest local variance -- a flat wall cannot be matched,
/// an eye or a collar can), kept at least `2 * patch` apart so the rig is
/// spread across the subject rather than clustered on one feature.
pub fn pick_points(frame: &Gray, around: (f32, f32), radius: f32, n: usize, patch: i32) -> Vec<(f32, f32)> {
    let (w, h) = (frame.w as i32, frame.h as i32);
    let step = (patch / 2).max(2);
    let mut cand: Vec<(f32, (f32, f32))> = Vec::new();
    let (cx, cy) = (around.0 as i32, around.1 as i32);
    let r = radius as i32;
    let mut y = (cy - r).max(patch + 1);
    while y < (cy + r).min(h - patch - 1) {
        let mut x = (cx - r).max(patch + 1);
        while x < (cx + r).min(w - patch - 1) {
            if ((x - cx) as f32).hypot((y - cy) as f32) <= radius {
                let (mut s, mut s2, mut k) = (0.0f32, 0.0f32, 0.0f32);
                for dy in -patch..=patch { for dx in -patch..=patch {
                    let v = frame.px[((y + dy) as usize) * frame.w + (x + dx) as usize] as f32;
                    s += v; s2 += v * v; k += 1.0;
                }}
                cand.push((s2 / k - (s / k).powi(2), (x as f32, y as f32)));
            }
            x += step;
        }
        y += step;
    }
    cand.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(f32, f32)> = Vec::new();
    for (var, p) in cand {
        if var < 60.0 { break; }   // too flat to match reliably
        if out.iter().all(|q| (q.0 - p.0).hypot(q.1 - p.1) >= 2.0 * patch as f32) { out.push(p); }
        if out.len() == n { break; }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured "face" drawn at a rigid transform over a plain background,
    /// with an optional occluder (a flat block) covering part of it.
    fn scene(w: usize, h: usize, rig: Rig, occluder: Option<(i32, i32, i32, i32)>) -> Gray {
        let mut g = Gray::new(w, h);
        let inv = |x: f32, y: f32| {
            let (dx, dy) = (x - rig.tx, y - rig.ty);
            let (c, s) = (rig.rot.cos() / rig.scale, rig.rot.sin() / rig.scale);
            (c * dx + s * dy, -s * dx + c * dy)
        };
        for y in 0..h { for x in 0..w {
            let (u, v) = inv(x as f32, y as f32);
            let on_face = (u - 80.0).hypot(v - 60.0) < 40.0;
            let val = if on_face { 128.0 + 90.0 * ((u / 5.0).sin() * (v / 4.0).cos()) } else { 40.0 };
            g.px[y * w + x] = val.clamp(0.0, 255.0) as u8;
        }}
        if let Some((x0, y0, x1, y1)) = occluder {
            for y in y0.max(0)..y1.min(h as i32) { for x in x0.max(0)..x1.min(w as i32) { g.px[y as usize * w + x as usize] = 200; } }
        }
        g
    }

    #[test]
    fn the_rig_is_solved_exactly_from_clean_points() {
        let truth = Rig { tx: 12.0, ty: -7.0, rot: 0.3, scale: 1.1 };
        let refs = [(10.0, 10.0), (60.0, 15.0), (30.0, 50.0), (70.0, 70.0)];
        let pairs: Vec<_> = refs.iter().map(|p| (*p, truth.apply(*p))).collect();
        let r = solve(&pairs).unwrap();
        assert!((r.tx - truth.tx).abs() < 1e-3 && (r.rot - truth.rot).abs() < 1e-4 && (r.scale - truth.scale).abs() < 1e-4, "{r:?}");
        assert!(solve(&pairs[..1]).is_none(), "one point cannot fix a rotation");
    }

    #[test]
    fn a_rig_follows_a_moving_turning_face() {
        let frames: Vec<Gray> = (0..30).map(|i| scene(200, 140, Rig { tx: i as f32 * 1.5, ty: i as f32 * 0.5,
            rot: (i as f32 * 0.012), scale: 1.0 }, None)).collect();
        let pts = [(60.0, 45.0), (100.0, 45.0), (70.0, 75.0), (95.0, 72.0)];
        let t = track(&frames, &pts, 6, 6, 18.0);
        let last = &t[29].rig;
        assert!((last.tx - 43.5).abs() < 6.0 || (last.apply((80.0, 60.0)).0 - (80.0 + 43.5)).abs() < 3.0, "{last:?}");
        let centre = last.apply((80.0, 60.0));
        let truth = Rig { tx: 29.0 * 1.5, ty: 29.0 * 0.5, rot: 29.0 * 0.012, scale: 1.0 }.apply((80.0, 60.0));
        assert!((centre.0 - truth.0).hypot(centre.1 - truth.1) < 2.5, "face centre {centre:?} vs {truth:?}");
        assert!((last.rot - 29.0 * 0.012).abs() < 0.05, "rotation followed: {}", last.rot);
    }

    #[test]
    fn points_that_disappear_are_carried_by_the_others_and_come_back() {
        // frames 10-19: a hand covers the left half of the face
        let frames: Vec<Gray> = (0..30).map(|i| {
            let rig = Rig { tx: i as f32 * 1.2, ty: 0.0, rot: 0.0, scale: 1.0 };
            let occ = if (10..20).contains(&i) { Some((0, 0, 72 + (i as f32 * 1.2) as i32, 140)) } else { None };
            scene(200, 140, rig, occ)
        }).collect();
        let pts = [(55.0, 45.0), (60.0, 75.0), (100.0, 45.0), (104.0, 72.0)];
        let t = track(&frames, &pts, 6, 6, 18.0);
        // while covered: the left points are not seen, the rig still tracks
        assert!(t[15].points[0].is_none() && t[15].points[1].is_none(), "covered points are reported missing: {:?}", t[15].points);
        assert!(!t[15].held, "two points are enough to carry the rig");
        let c15 = t[15].rig.apply((80.0, 60.0));
        assert!((c15.0 - (80.0 + 15.0 * 1.2)).abs() < 2.5, "the face centre is still right: {c15:?}");
        // after: the lost points are found again where the rig said they would be
        assert!(t[25].points[0].is_some() && t[25].points[1].is_some(), "re-acquired: {:?}", t[25].points);
    }

    #[test]
    fn points_are_picked_on_texture_and_spread_out() {
        let g = scene(200, 140, Rig::IDENTITY, None);
        let pts = pick_points(&g, (80.0, 60.0), 45.0, 5, 5);
        assert!(pts.len() >= 4, "{pts:?}");
        // all on the textured face (within its radius of 40 around 80,60), none on the flat background
        assert!(pts.iter().all(|p| (p.0 - 80.0).hypot(p.1 - 60.0) < 42.0), "{pts:?}");
        for (i, a) in pts.iter().enumerate() { for b in &pts[i + 1..] {
            assert!((a.0 - b.0).hypot(a.1 - b.1) >= 10.0, "spread apart: {a:?} {b:?}");
        }}
        // a flat region offers nothing to track
        assert!(pick_points(&g, (180.0, 20.0), 12.0, 5, 5).is_empty(), "flat background: no points");
    }

    #[test]
    fn with_everything_covered_the_rig_holds_instead_of_inventing_motion() {
        let frames: Vec<Gray> = (0..12).map(|i| scene(200, 140, Rig { tx: i as f32, ty: 0.0, rot: 0.0, scale: 1.0 },
            if i >= 6 { Some((0, 0, 200, 140)) } else { None })).collect();
        let t = track(&frames, &[(60.0, 45.0), (100.0, 45.0), (80.0, 75.0)], 6, 6, 18.0);
        assert!(t[6].held && t[8].held, "nothing visible: held");
        assert!(!t[5].held);
        // against the last SOLVED frame: comparing two held frames let a
        // one-step drift on entering the hold pass unnoticed
        assert_eq!(t[8].rig, t[5].rig, "the rig does not drift while held");
    }
}
