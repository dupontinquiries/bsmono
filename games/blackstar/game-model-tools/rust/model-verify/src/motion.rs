//! motion — a transformer as a joint web, and how it moves.
//!
//! ## The model this replaces, and why it was wrong
//!
//! The first attempt translated regions along axes in parallel, which reads as
//! an explosion diagram. The second parented them in a tree and hinged them,
//! which reads as small parts being torn off big ones. Both were wrong in the
//! same way: they treated the body plan as the *thing being animated*.
//!
//! It is not. **Every part — every missile, every finger — has a joint to
//! something.** The character is a WEB, and the transformation is that web
//! rearranging itself. What makes a screen transformation look designed rather
//! than chaotic is that the order is *optimised*: certain things rotate first
//! so that everything else has somewhere to go.
//!
//! So this module does not try to derive a bot from a vehicle. It solves the
//! separate, smaller problem: **given a joint web and a target pose, move every
//! part there smoothly, quickly, and without parts passing through each other.**
//!
//! ## The constraint that makes it tractable
//!
//! A part is attached to exactly one thing for ~90% of the transform. The
//! remaining cases re-attach elsewhere — but only in the **first or last 5%** of
//! the timeline. That means the graph is a forest for the entire middle of the
//! motion, and re-parenting is two discrete events at the edges rather than a
//! continuously changing topology. A solver for the general case would be hard;
//! a solver for this one is a scheduler.
//!
//! ## Emotion is a profile, not a rewrite
//!
//! The same schedule played with a different easing profile is the same
//! transformation performed in a different mood: [`Profile::Combat`] reaches
//! peak velocity sooner and stops harder, [`Profile::Anger`] overshoots and
//! rebounds. Keeping that as a curve means the choreography is authored once.

/// One joint in the web.
#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    pub part: usize,
    /// What it hangs from for the bulk of the motion. `None` = anchored.
    pub parent: Option<usize>,
    /// Pivot in model space — the seam, not the centroid.
    pub pivot: [f64; 3],
    /// Rotation axis (unit).
    pub axis: [f64; 3],
    /// Total swing, radians.
    pub angle: f64,
    /// Translation along the axis, for parts that slide as well as swing.
    pub slide: f64,
    /// A re-clip: at `t`, the part changes parent. Only legal in the first or
    /// last 5%, which is what keeps the topology static through the middle.
    pub reclip: Option<Reclip>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reclip {
    pub at: f64,
    pub new_parent: Option<usize>,
}

/// Where in the timeline a re-clip may legally happen.
pub const RECLIP_EARLY: f64 = 0.05;
pub const RECLIP_LATE: f64 = 0.95;

pub fn reclip_legal(at: f64) -> bool { at <= RECLIP_EARLY || at >= RECLIP_LATE }

/// Motion character. The choreography is authored once; this is the performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Smooth ease-in-out. The default read: mechanical and unhurried.
    Fluid,
    /// Reaches peak velocity sooner and arrives hard — parts snap into place.
    Combat,
    /// Overshoots the target and rebounds into it. Reads as force applied by
    /// something that does not care about its own hardware.
    Anger,
}

impl Profile {
    /// Eased position 0..1. May exceed 1 for [`Profile::Anger`] — that is the
    /// overshoot, and clamping it here would remove the entire effect.
    pub fn ease(self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Profile::Fluid => u * u * (3.0 - 2.0 * u),
            Profile::Combat => {
                // fast out of the gate, hard stop: quintic ease-out
                let v = 1.0 - u;
                1.0 - v * v * v * v * v
            }
            Profile::Anger => {
                // damped spring past the target, settling by u = 1
                if u >= 1.0 { return 1.0; }
                // Tuned down from w=14, z=0.42, which overshot handsomely and
                // also covered 17% of the path in a single frame at 240 Hz -
                // that strobes. Anger should hit harder than Fluid, not
                // teleport; the continuity test is what caught it.
                let w: f64 = 9.0;
                let z: f64 = 0.55;
                let d = (1.0 - z * z).sqrt();
                1.0 - (-z * w * u).exp() * ((d * w * u).cos() + z / d * (d * w * u).sin())
            }
        }
    }

    /// Peak overshoot, for a caller sizing clearances so an angry transform does
    /// not punch a limb through the floor.
    pub fn max_overshoot(self) -> f64 {
        match self {
            Profile::Anger => {
                let mut m: f64 = 1.0;
                for i in 0..=200 { m = m.max(self.ease(i as f64 / 200.0)); }
                m
            }
            _ => 1.0,
        }
    }
}

/// A per-joint time window: when this joint moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot { pub start: f64, pub end: f64 }

impl Slot {
    pub fn local(&self, t: f64) -> f64 {
        if t <= self.start { 0.0 }
        else if t >= self.end { 1.0 }
        else { (t - self.start) / (self.end - self.start).max(1e-9) }
    }
}

/// Order the joints so the web can actually move.
///
/// Two rules, and they are the whole "optimiser":
///
/// 1. **A parent moves before its children.** Not because it must finish first,
///    but because a child swinging while its parent is still folded sweeps
///    through space the parent is about to occupy. Overlapping windows are fine;
///    starting a child *earlier* than its parent is what looks broken.
/// 2. **Wide swings go early.** A part with a large angle needs the most room,
///    and room is most available at the start when everything is still stowed.
///    This is the part that makes a transformation look *designed* — the big
///    panels commit first and the detail follows into the space they opened.
///
/// The result is a schedule, not a simulation: cheap, deterministic, and
/// authorable over by hand where an artist disagrees.
pub fn schedule(joints: &[Joint], overlap: f64) -> Vec<Slot> {
    let n = joints.len();
    let mut depth = vec![0usize; n];
    for i in 0..n {
        let mut d = 0;
        let mut p = joints[i].parent;
        let mut guard = 0;
        while let Some(pi) = p {
            d += 1;
            p = joints.get(pi).and_then(|j| j.parent);
            guard += 1;
            if guard > n { break; }   // a cycle in the web: stop rather than hang
        }
        depth[i] = d;
    }
    let maxd = depth.iter().copied().max().unwrap_or(0);
    // rank within a depth band by swing size, widest first
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        depth[a].cmp(&depth[b])
            .then(joints[b].angle.abs().partial_cmp(&joints[a].angle.abs()).unwrap())
    });
    let mut slots = vec![Slot { start: 0.0, end: 1.0 }; n];
    for (rank, &i) in order.iter().enumerate() {
        let band = if maxd == 0 { 0.0 } else { depth[i] as f64 / (maxd + 1) as f64 };
        // widest-first nudge within the band, so big panels lead
        let within = rank as f64 / n.max(1) as f64 * 0.12;
        let start = (band + within).min(1.0 - overlap);
        slots[i] = Slot { start, end: (start + overlap).min(1.0) };
    }
    slots
}

/// The effective parent of `j` at time `t`, honouring a legal re-clip.
pub fn parent_at(j: &Joint, t: f64) -> Option<usize> {
    match j.reclip {
        Some(rc) if reclip_legal(rc.at) && t >= rc.at => rc.new_parent,
        _ => j.parent,
    }
}

/// Rotation matrix for `angle` about a unit `axis` (Rodrigues).
pub fn axis_angle(axis: [f64; 3], angle: f64) -> [[f64; 3]; 3] {
    let l = (axis[0]*axis[0] + axis[1]*axis[1] + axis[2]*axis[2]).sqrt().max(1e-12);
    let (x, y, z) = (axis[0]/l, axis[1]/l, axis[2]/l);
    let (c, s) = (angle.cos(), angle.sin());
    let t = 1.0 - c;
    [[t*x*x + c,   t*x*y - s*z, t*x*z + s*y],
     [t*x*y + s*z, t*y*y + c,   t*y*z - s*x],
     [t*x*z - s*y, t*y*z + s*x, t*z*z + c]]
}

/// Resolve a part's world transform at time `t`, walking up the web.
pub fn pose_at(joints: &[Joint], slots: &[Slot], i: usize, t: f64, profile: Profile,
               p: [f64; 3]) -> [f64; 3] {
    let mut q = p;
    let mut cur = Some(i);
    let mut guard = 0;
    while let Some(k) = cur {
        let j = &joints[k];
        let e = profile.ease(slots[k].local(t));
        let r = axis_angle(j.axis, j.angle * e);
        let d = [q[0]-j.pivot[0], q[1]-j.pivot[1], q[2]-j.pivot[2]];
        for a in 0..3 {
            q[a] = j.pivot[a] + r[a][0]*d[0] + r[a][1]*d[1] + r[a][2]*d[2]
                 + j.axis[a] * j.slide * e;
        }
        cur = parent_at(j, t);
        guard += 1;
        if guard > joints.len() { break; }
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(part: usize, parent: Option<usize>, angle: f64) -> Joint {
        Joint { part, parent, pivot: [0.0, 0.0, 0.0], axis: [0.0, 0.0, 1.0],
                angle, slide: 0.0, reclip: None }
    }

    #[test]
    fn every_profile_starts_at_rest_and_arrives() {
        // A part that has not finished moving by the end of the transform is a
        // part sticking out of the finished robot.
        for p in [Profile::Fluid, Profile::Combat, Profile::Anger] {
            assert!(p.ease(0.0).abs() < 1e-9, "{p:?} must start at rest");
            assert!((p.ease(1.0) - 1.0).abs() < 1e-6, "{p:?} must arrive: {}", p.ease(1.0));
        }
    }

    #[test]
    fn combat_gets_there_sooner_and_anger_overshoots() {
        // The emotions have to be measurably different, or they are a label.
        let (f, c, a) = (Profile::Fluid, Profile::Combat, Profile::Anger);
        assert!(c.ease(0.3) > f.ease(0.3), "combat is ahead early: {} vs {}", c.ease(0.3), f.ease(0.3));
        assert!(a.max_overshoot() > 1.05, "anger must overshoot, got {}", a.max_overshoot());
        assert_eq!(f.max_overshoot(), 1.0, "fluid never exceeds its target");
        assert_eq!(c.max_overshoot(), 1.0);
        // and anger settles rather than oscillating forever
        assert!((a.ease(1.0) - 1.0).abs() < 1e-6);
        assert!((a.ease(0.98) - 1.0).abs() < 0.08, "nearly settled just before the end");
    }

    #[test]
    fn a_parent_never_starts_after_its_child() {
        // A child swinging while its parent is still folded sweeps through space
        // the parent is about to occupy — the thing that looks broken.
        let joints = vec![
            j(0, None, 1.2), j(1, Some(0), 0.8), j(2, Some(1), 0.4),
            j(3, Some(0), 1.5), j(4, Some(3), 0.2),
        ];
        let s = schedule(&joints, 0.5);
        for (i, jt) in joints.iter().enumerate() {
            if let Some(p) = jt.parent {
                assert!(s[p].start <= s[i].start + 1e-9,
                        "joint {i} starts at {} before its parent {p} at {}", s[i].start, s[p].start);
            }
        }
    }

    #[test]
    fn wide_swings_lead_within_their_band() {
        // Big panels commit first and detail follows into the space they open —
        // the thing that makes a transformation look designed.
        let joints = vec![j(0, None, 0.1), j(1, None, 2.0), j(2, None, 0.5)];
        let s = schedule(&joints, 0.4);
        assert!(s[1].start <= s[2].start && s[2].start <= s[0].start,
                "widest first: {:?}", s.iter().map(|x| x.start).collect::<Vec<_>>());
    }

    #[test]
    fn every_joint_finishes_by_the_end() {
        let joints: Vec<Joint> = (0..12).map(|i| j(i, if i == 0 { None } else { Some(i-1) }, 0.5)).collect();
        let s = schedule(&joints, 0.45);
        for (i, sl) in s.iter().enumerate() {
            assert!(sl.end <= 1.0 + 1e-9, "joint {i} ends past the transform");
            assert!(sl.local(1.0) >= 1.0 - 1e-9, "joint {i} has not arrived at t=1");
        }
    }

    #[test]
    fn a_cycle_in_the_web_is_survived_rather_than_hung() {
        // Real data will contain one eventually, and an animation tool that
        // hangs on load is worse than one that mis-orders a joint.
        let joints = vec![j(0, Some(1), 0.5), j(1, Some(0), 0.5)];
        let s = schedule(&joints, 0.4);
        assert_eq!(s.len(), 2);
        let _ = pose_at(&joints, &s, 0, 0.5, Profile::Fluid, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn reclipping_is_confined_to_the_edges_of_the_transform() {
        // The constraint that makes this tractable: the web is a fixed forest
        // for the whole middle, so re-parenting is two discrete events.
        assert!(reclip_legal(0.0) && reclip_legal(0.04) && reclip_legal(0.99));
        assert!(!reclip_legal(0.2), "mid-transform re-clip changes the topology under the solver");
        assert!(!reclip_legal(0.5));

        let mut jt = j(1, Some(0), 0.5);
        jt.reclip = Some(Reclip { at: 0.96, new_parent: Some(2) });
        assert_eq!(parent_at(&jt, 0.5), Some(0), "before the re-clip, the original parent");
        assert_eq!(parent_at(&jt, 0.97), Some(2), "after it, the new one");

        // an ILLEGAL re-clip is ignored rather than honoured
        let mut bad = j(1, Some(0), 0.5);
        bad.reclip = Some(Reclip { at: 0.5, new_parent: Some(2) });
        assert_eq!(parent_at(&bad, 0.9), Some(0), "mid-transform re-clip must not take effect");
    }

    #[test]
    fn a_part_actually_moves_and_ends_where_the_joint_says() {
        // A quarter turn about Z: (1,0,0) -> (0,1,0).
        let joints = vec![j(0, None, std::f64::consts::FRAC_PI_2)];
        let s = vec![Slot { start: 0.0, end: 1.0 }];
        let start = pose_at(&joints, &s, 0, 0.0, Profile::Fluid, [1.0, 0.0, 0.0]);
        let end = pose_at(&joints, &s, 0, 1.0, Profile::Fluid, [1.0, 0.0, 0.0]);
        assert!((start[0] - 1.0).abs() < 1e-9, "unmoved at t=0");
        assert!((end[0]).abs() < 1e-6 && (end[1] - 1.0).abs() < 1e-6, "quarter turn: {end:?}");
    }

    #[test]
    fn motion_is_continuous_so_nothing_teleports() {
        // Fluid motion means no frame-to-frame jump larger than the whole path
        // divided by a sane frame count — a teleport is the one artefact an
        // audience always sees.
        let joints = vec![j(0, None, 1.4), j(1, Some(0), 2.2), j(2, Some(1), 0.9)];
        let s = schedule(&joints, 0.5);
        for prof in [Profile::Fluid, Profile::Combat, Profile::Anger] {
            let mut prev = pose_at(&joints, &s, 2, 0.0, prof, [1.0, 0.5, 0.0]);
            let mut worst: f64 = 0.0;
            for k in 1..=240 {
                let t = k as f64 / 240.0;
                let p = pose_at(&joints, &s, 2, t, prof, [1.0, 0.5, 0.0]);
                let d = ((p[0]-prev[0]).powi(2) + (p[1]-prev[1]).powi(2) + (p[2]-prev[2]).powi(2)).sqrt();
                worst = worst.max(d);
                prev = p;
            }
            assert!(worst < 0.15, "{prof:?} jumped {worst} in one frame");
        }
    }
}
