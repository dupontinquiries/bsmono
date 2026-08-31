//! IS THIS BODY STANDING, OR JUST POSITIONED?
//!
//! Two questions the rest of the pipeline assumes the answer to, and one
//! reported symptom that is what happens when the assumption is wrong.
//!
//! **What is the body resting ON?** `ride_h` is the distance from the origin
//! to the lowest point, and physics seats the chassis at exactly that height.
//! `massfilter` already removes *debris* from that measurement, which fixes
//! the flung-panel case. It does not — and should not — remove a legitimately
//! modelled arm that happens to hang lower than the feet. When that happens
//! the body is seated on its knuckles: the feet float, or the fingers go
//! through the floor, depending on which way the error runs. Naming the limbs
//! (`skeleton::limb_roles`) is what makes this checkable at last — before it,
//! the lowest point was just a number with no idea which part produced it.
//!
//! **Is the mass over the base?** A standing figure keeps its centre of mass
//! inside the polygon its feet make. Outside it, the body is falling — and a
//! rig that is outside it while standing still is the definition of reading as
//! floating, because the pose asserts a balance the geometry does not support.
//! This is diagnostic rather than uniform: a four-legged crawler should pass
//! trivially and a one-legged pose should not, so a result of "everything is
//! fine" means the measure is broken.
//!
//! ## A limitation this module found in its own inputs
//!
//! `resting_on` is only as good as the limb names, and `skeleton::limb_roles`
//! decides a role from where a limb ATTACHES. `skeletonize` puts that root at
//! the core point nearest the limb's mass — so a long arm reaching to the
//! floor has its root dragged to the bottom of the core and is named a LEG.
//! Root position alone cannot separate a knuckle-dragging arm from a leg;
//! distinguishing them needs the limb's own extent, or its attachment height
//! measured before the mass pulls it down. Found while trying to build a
//! fixture for exactly that case and discovering it could not be expressed.
//! Recorded rather than papered over, because the classifier is confident and
//! silent about it.
//!
//! Pure: points in, numbers out. No renderer, no physics, no clock. The
//! `Vec3`s are in the body's own oriented frame (Y up, +X right).

use glam::{Vec2, Vec3};

use crate::skeleton::{LimbRole, Skeleton};

/// How thick a slice off the bottom of the body counts as touching down.
/// A fraction of the body's total height: feet are not points, and a rig
/// with any ankle at all has its contact spread over a band.
pub const CONTACT_BAND: f32 = 0.08;

/// What the body is standing on and whether it is over it.
#[derive(Clone, Debug)]
pub struct Stance {
    /// Vertex-weighted centroid — a proxy for the volume-weighted centre of
    /// mass, and the right one here: the prims are shells, so vertex density
    /// tracks surface area, which tracks mass for a body built from panels.
    pub com: Vec3,
    /// The contact points, projected to the floor plane.
    pub support: Vec<Vec2>,
    /// Which limb owns the lowest point. `Other` covers the core and any
    /// unnamed appendage.
    pub resting_on: LimbRole,
    /// Signed distance from the CoM to the support polygon's edge, in world
    /// units. NEGATIVE is inside (stable); positive is the distance by which
    /// the body is past its own base.
    pub balance: f32,
}

impl Stance {
    /// The mass is over the base.
    pub fn balanced(&self) -> bool {
        self.balance <= 0.0
    }
    /// A legged body that is not standing on a leg. This is the one that
    /// produces the reported symptom directly: physics seats the chassis on
    /// whatever is lowest, so if that is a hand the feet do not reach the
    /// floor.
    pub fn resting_wrong(&self, roles: &[LimbRole]) -> bool {
        roles.iter().any(|r| r.is_leg()) && !self.resting_on.is_leg()
    }
    pub fn report(&self) -> String {
        format!(
            "com ({:.2},{:.2},{:.2})  on {}  support {} pts  balance {:+.3} {}",
            self.com.x,
            self.com.y,
            self.com.z,
            self.resting_on.label(),
            self.support.len(),
            self.balance,
            if self.balanced() { "ok" } else { "FALLING" },
        )
    }
}

/// Convex hull of the contact points, in the floor plane. Andrew's monotone
/// chain: the support base is the hull of the feet, not the feet themselves —
/// a body is stable anywhere between its soles, which is the whole reason two
/// feet are more stable than one.
fn hull(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts;
    }
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| (*a - *b).length() < 1e-6);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len() * 2);
    for &p in &pts {
        while out.len() >= 2 && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
            out.pop();
        }
        out.push(p);
    }
    let lower = out.len() + 1;
    for &p in pts.iter().rev() {
        while out.len() >= lower && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
            out.pop();
        }
        out.push(p);
    }
    out.pop();
    out
}

/// Signed distance from `p` to a convex polygon: negative inside.
///
/// Degenerate bases are handled rather than special-cased away, because they
/// are the interesting ones: a single contact point is a body balanced on a
/// pin, and the honest answer is the distance to that point, not zero.
fn signed_distance(poly: &[Vec2], p: Vec2) -> f32 {
    match poly.len() {
        0 => f32::MAX,
        1 => (p - poly[0]).length(),
        2 => {
            let (a, b) = (poly[0], poly[1]);
            let ab = b - a;
            let t = if ab.length_squared() < 1e-12 {
                0.0
            } else {
                ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
            };
            (p - (a + ab * t)).length()
        }
        n => {
            let mut inside = true;
            let mut best = f32::MAX;
            for i in 0..n {
                let a = poly[i];
                let b = poly[(i + 1) % n];
                let ab = b - a;
                // hull() emits counter-clockwise, so a point to the RIGHT of
                // any edge is outside
                if ab.x * (p.y - a.y) - ab.y * (p.x - a.x) < 0.0 {
                    inside = false;
                }
                let t = if ab.length_squared() < 1e-12 {
                    0.0
                } else {
                    ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
                };
                best = best.min((p - (a + ab * t)).length());
            }
            if inside {
                -best
            } else {
                best
            }
        }
    }
}

/// Measure a posed body.
///
/// `roles` must be `skeleton::limb_roles*`'s output for `sk`, in the same
/// order as `sk.limbs`.
pub fn assess(prims: &[&[Vec3]], sk: &Skeleton, roles: &[LimbRole]) -> Stance {
    let mut sum = Vec3::ZERO;
    let mut n = 0usize;
    let mut lowest = f32::MAX;
    let mut hi = f32::MIN;
    for p in prims {
        for v in *p {
            sum += *v;
            n += 1;
            lowest = lowest.min(v.y);
            hi = hi.max(v.y);
        }
    }
    if n == 0 {
        return Stance {
            com: Vec3::ZERO,
            support: Vec::new(),
            resting_on: LimbRole::Other,
            balance: f32::MAX,
        };
    }
    let com = sum / n as f32;
    let band = ((hi - lowest).max(1e-4) * CONTACT_BAND).max(1e-4);

    // which limb owns the lowest point — the part physics will seat on
    let mut resting_on = LimbRole::Other;
    let mut best = f32::MAX;
    for (li, limb) in sk.limbs.iter().enumerate() {
        let role = roles.get(li).copied().unwrap_or(LimbRole::Other);
        for &pi in &limb.prims {
            if let Some(p) = prims.get(pi) {
                for v in *p {
                    if v.y < best {
                        best = v.y;
                        resting_on = role;
                    }
                }
            }
        }
    }
    // the core can be the lowest thing (a wheeled body, a tank hull); that is
    // `Other` and correct, so only override when a limb genuinely reaches
    // lower than everything else did
    if best > lowest + 1e-5 {
        resting_on = LimbRole::Other;
    }

    let support: Vec<Vec2> = prims
        .iter()
        .flat_map(|p| p.iter())
        .filter(|v| v.y <= lowest + band)
        .map(|v| Vec2::new(v.x, v.z))
        .collect();
    let poly = hull(support);
    let balance = signed_distance(&poly, Vec2::new(com.x, com.z));
    Stance { com, support: poly, resting_on, balance }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{limb_roles_paired, skeletonize};

    fn slab(c: Vec3, half: Vec3, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let t = i as f32 * 2.399;
                let u = (i as f32 / n as f32) * 2.0 - 1.0;
                c + Vec3::new(t.cos() * half.x, u * half.y, t.sin() * half.z)
            })
            .collect()
    }

    fn measure(prims: &[Vec<Vec3>]) -> (Stance, Vec<LimbRole>) {
        let refs: Vec<&[Vec3]> = prims.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let roles = limb_roles_paired(&sk);
        (assess(&refs, &sk, &roles), roles)
    }

    fn standing() -> Vec<Vec<Vec3>> {
        vec![
            slab(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.45, 0.55, 0.30), 400),
            slab(Vec3::new(-0.25, 0.5, 0.0), Vec3::new(0.14, 0.55, 0.14), 90),
            slab(Vec3::new(0.25, 0.5, 0.0), Vec3::new(0.14, 0.55, 0.14), 90),
            slab(Vec3::new(-0.72, 1.7, 0.0), Vec3::new(0.12, 0.45, 0.12), 80),
            slab(Vec3::new(0.72, 1.7, 0.0), Vec3::new(0.12, 0.45, 0.12), 80),
        ]
    }

    #[test]
    fn a_standing_figure_rests_on_a_leg_and_is_balanced() {
        let (st, roles) = measure(&standing());
        assert!(
            st.resting_on.is_leg(),
            "a standing figure is seated on its {}: {}",
            st.resting_on.label(),
            st.report()
        );
        assert!(!st.resting_wrong(&roles), "{}", st.report());
        assert!(st.balanced(), "a symmetric standing figure fell over: {}", st.report());
    }

    #[test]
    fn a_body_seated_on_something_other_than_a_leg_is_flagged() {
        // THE REPORTED SYMPTOM, as a measurement. `ride_h` is the distance to
        // the lowest point, so whatever reaches lowest is what physics rests
        // the chassis on. If that is not a foot, the feet do not reach the
        // floor. `massfilter` cannot catch this — the offending part is real
        // geometry, not debris, and deleting it would be worse.
        //
        // The skeleton is built BY HAND here rather than via `skeletonize`.
        // `assess`'s contract is "given roles, report seating", and routing
        // the fixture through the classifier tests two units at once — which
        // is how three earlier versions of this test failed: each attempt to
        // pose a body with a low non-leg part moved the core, and the core is
        // the reference the classifier measures roles against, so the roles
        // changed out from under the fixture.
        use crate::skeleton::Limb;
        let torso = slab(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.45, 0.55, 0.30), 400);
        let leg_l = slab(Vec3::new(-0.25, 0.9, 0.0), Vec3::new(0.14, 0.30, 0.14), 90);
        let leg_r = slab(Vec3::new(0.25, 0.9, 0.0), Vec3::new(0.14, 0.30, 0.14), 90);
        // a skid, nacelle or gun pod that hangs lower than the retracted legs
        let pod = slab(Vec3::new(0.0, 0.2, 0.6), Vec3::new(0.20, 0.25, 0.20), 90);
        let prims = vec![torso, leg_l, leg_r, pod];
        let refs: Vec<&[Vec3]> = prims.iter().map(|v| v.as_slice()).collect();

        let limb = |i: usize, root: Vec3, tip: Vec3| Limb {
            prims: vec![i],
            root,
            tip,
            reach: (tip - root).length(),
        };
        let sk = Skeleton {
            core: vec![0],
            core_centre: Vec3::new(0.0, 1.6, 0.0),
            core_half: Vec3::new(0.45, 0.55, 0.30),
            limbs: vec![
                limb(1, Vec3::new(-0.25, 1.05, 0.0), Vec3::new(-0.25, 0.60, 0.0)),
                limb(2, Vec3::new(0.25, 1.05, 0.0), Vec3::new(0.25, 0.60, 0.0)),
                limb(3, Vec3::new(0.0, 1.05, 0.30), Vec3::new(0.0, -0.05, 0.60)),
            ],
            joined: 0,
        };
        let roles = vec![LimbRole::LegL, LimbRole::LegR, LimbRole::Other];

        let st = assess(&refs, &sk, &roles);
        assert!(
            st.resting_wrong(&roles),
            "a body seated on a pod was not flagged: {}",
            st.report()
        );
        assert_eq!(st.resting_on, LimbRole::Other, "{}", st.report());

        // THE COMPLEMENT (CLAUDE.md §5c): retract the pod above the soles and
        // the same body must stop being flagged, or `resting_wrong` is simply
        // always true for anything with legs.
        let mut ok_prims = prims.clone();
        for p in ok_prims[3].iter_mut() {
            p.y += 1.0;
        }
        let ok_refs: Vec<&[Vec3]> = ok_prims.iter().map(|v| v.as_slice()).collect();
        let ok = assess(&ok_refs, &sk, &roles);
        assert!(
            !ok.resting_wrong(&roles),
            "a body standing on its feet was still flagged: {}",
            ok.report()
        );
        assert!(ok.resting_on.is_leg(), "{}", ok.report());
    }

    #[test]
    fn leaning_past_the_feet_reads_as_falling() {
        // The complement to "balanced" — if nothing ever reports FALLING the
        // measure is not measuring. The torso and both arms move well past
        // the feet; the feet stay put.
        let mut body = standing();
        for i in [0usize, 3, 4] {
            for p in body[i].iter_mut() {
                p.x += 3.0;
            }
        }
        let (st, _) = measure(&body);
        assert!(
            !st.balanced(),
            "a body with its mass 3 units past its feet was called balanced: {}",
            st.report()
        );
        assert!(st.balance > 0.5, "the overhang was measured as {:.2}", st.balance);
    }

    #[test]
    fn a_wide_base_is_more_stable_than_a_narrow_one() {
        // Assert a DIRECTION, not a single value: a measure that has stopped
        // discriminating satisfies any one-sided check. Same CoM, feet moved
        // apart — the balance margin must improve.
        let narrow = {
            let mut b = standing();
            b[1] = slab(Vec3::new(-0.05, 0.5, 0.0), Vec3::new(0.10, 0.55, 0.10), 90);
            b[2] = slab(Vec3::new(0.05, 0.5, 0.0), Vec3::new(0.10, 0.55, 0.10), 90);
            measure(&b).0.balance
        };
        let wide = measure(&standing()).0.balance;
        assert!(
            wide < narrow,
            "widening the stance did not improve balance (wide {wide:+.3}, narrow {narrow:+.3})"
        );
    }

    #[test]
    fn the_hull_and_the_distance_agree_on_inside_and_outside() {
        // The geometry primitives, independent of any body — a wrong winding
        // flips inside/outside everywhere and every body test above would
        // still pass if they all happened to sit on one side.
        let square = hull(vec![
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, 1.0),
            Vec2::new(0.0, 0.0), // interior point must not survive
        ]);
        assert_eq!(square.len(), 4, "the hull kept an interior point: {square:?}");
        assert!(signed_distance(&square, Vec2::ZERO) < 0.0, "centre read as outside");
        assert!((signed_distance(&square, Vec2::ZERO) + 1.0).abs() < 1e-4, "wrong inradius");
        assert!(signed_distance(&square, Vec2::new(3.0, 0.0)) > 0.0, "far point read as inside");
        assert!((signed_distance(&square, Vec2::new(3.0, 0.0)) - 2.0).abs() < 1e-4);
        // degenerate bases: one foot, and two feet in a line
        assert!((signed_distance(&[Vec2::ZERO], Vec2::new(0.0, 2.0)) - 2.0).abs() < 1e-4);
        let line = [Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0)];
        assert!((signed_distance(&line, Vec2::new(0.0, 1.0)) - 1.0).abs() < 1e-4);
    }
}
