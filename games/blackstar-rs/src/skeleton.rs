//! A BODY AS A SKELETON, for anything — bot or vehicle.
//!
//! `partdetect` classifies one prim at a time: is this a wheel, a leg, a
//! nozzle? That answers "what is this part" and never "what is this part
//! ATTACHED TO", so nothing in the pipeline knows a body has a middle and
//! things sticking out of it. Three separate problems all come from that
//! gap, and they are all solved here:
//!
//! 1. **Vehicles have no skeleton at all.** A bot gets a torso and
//!    appendages by eye; a car gets a bounding box. But a car IS a core
//!    with appendages — cabin and chassis in the middle, wheels and wings
//!    and nacelles hanging off — and the decomposition is the same
//!    measurement. `skeletonize` does both: it does not know or care which
//!    it was handed.
//!
//! 2. **Wings obscure the robot.** A jet's wing is correct at vehicle
//!    scale and absurd when the same geometry is standing up as a torso
//!    ornament — a two-metre bot inside a six-metre wingspan. Nothing in
//!    the fold changes it, because the fold interpolates vertices and the
//!    wing is where the artist left it. `compact` pulls each appendage in
//!    ALONG ITS OWN AXIS, so a wing shortens toward the shoulder it grows
//!    from rather than being uniformly scaled into a stub.
//!
//! 3. **Wonky flat pieces.** Thin slabs — often a vehicle's flat
//!    underside — survive into robot mode as loose plates floating near
//!    the body, because they are too big for `massfilter` to call debris
//!    and too disconnected to read as structure. `join_orphans` runs a
//!    RECURSIVE pass: a tiny or flat segment adopts the nearest group,
//!    that group's reach grows to include it, which may bring a further
//!    segment into range, and so on to a fixpoint. Voxel-grid neighbour
//!    lookup rather than all-pairs, so a 400-prim body does not cost
//!    160,000 distance tests per iteration.
//!
//! The thresholds are reasoned from shapes and carry the same caveat every
//! detector in this repo carries: no labelled set exists to fit against.
//! `Skeleton::report` prints what was decided so it can be judged on real
//! assets rather than argued about.

use glam::{Quat, Vec3};

/// One connected group: a core, or something hanging off it.
#[derive(Clone, Debug)]
pub struct Limb {
    /// prim indices belonging to this group
    pub prims: Vec<usize>,
    /// where it attaches — the point on the core nearest its mass
    pub root: Vec3,
    /// its far end
    pub tip: Vec3,
    /// how far the tip is from the root
    pub reach: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    /// the prims that make up the middle of the body
    pub core: Vec<usize>,
    pub core_centre: Vec3,
    pub core_half: Vec3,
    pub limbs: Vec<Limb>,
    /// how many segments were adopted by a neighbour rather than left
    /// floating (`join_orphans`)
    pub joined: usize,
}

impl Skeleton {
    pub fn report(&self) -> String {
        let reaches: Vec<String> =
            self.limbs.iter().map(|l| format!("{:.2}", l.reach)).collect();
        format!(
            "skeleton: core {} prims, {} limbs (reach {}), {} orphans joined",
            self.core.len(),
            self.limbs.len(),
            reaches.join("/"),
            self.joined
        )
    }
    /// The longest thing sticking out — the number that decides whether a
    /// body reads as itself or as a wingspan with a pilot in it.
    pub fn max_reach(&self) -> f32 {
        self.limbs.iter().map(|l| l.reach).fold(0.0, f32::max)
    }
}

/// A prim is a core candidate when its centroid sits within this fraction
/// of the body's half-extent of the middle.
const CORE_R: f32 = 0.42;
/// A segment holding less than this share of the body's points is small
/// enough to be adopted rather than stand alone.
const ORPHAN_SHARE: f32 = 0.035;
/// ...or is flat enough: thinnest axis under this fraction of its longest.
const FLAT: f32 = 0.14;
/// How much of a flat group's offset from the core must run along its own
/// LONGEST axis for it to count as reaching outward (a wing) rather than
/// lying against the body (an underside plate). Measured on the fixtures:
/// 1.00 for a wing, 0.00 for a plate, so half-way is a real gap.
const REACHES_OUT: f32 = 0.5;
/// Adoption range, in multiples of the adopting group's own size.
const ADOPT_R: f32 = 1.35;
/// Give up after this many passes. The recursion converges quickly (each
/// pass either adopts something or stops), and a cap means a pathological
/// body cannot spin.
const MAX_PASSES: usize = 8;

fn bounds(pts: &[Vec3]) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    (lo, hi)
}

fn centroid(pts: &[Vec3]) -> Vec3 {
    if pts.is_empty() {
        return Vec3::ZERO;
    }
    pts.iter().copied().sum::<Vec3>() / pts.len() as f32
}

/// Is this segment small or flat enough to be somebody's else's problem?
///
/// SHARE OUTRANKS FLATNESS. The flatness rule is for debris — a panel, a
/// skirt, a splinter — but "thin in one axis" is also the definition of a
/// WING, a fin, a blade and a rotor, which are appendages and the whole
/// reason this module exists. Measured on the test jet: each wing is 21%
/// of the model by point count and has flatness 0.031 against FLAT 0.14,
/// so the flatness rule adopted both wings away and a two-winged jet
/// reported ONE limb ("the wings did not read as limbs"). `compact` then
/// looked like a no-op for the same reason — with the wings absorbed into
/// their neighbour there was no overhang left to shorten.
///
/// So a group carrying a real share of the body is never orphaned on
/// flatness alone. It can still be orphaned by being small, which is what
/// the share test is for, and that is the test debris actually fails.
fn orphanish(pts: &[Vec3], share: f32, core_c: Vec3) -> bool {
    if share < ORPHAN_SHARE {
        return true;
    }
    let (lo, hi) = bounds(pts);
    let e = hi - lo;
    let long = e.max_element().max(1e-5);
    if e.min_element() / long >= FLAT {
        return false; // chunky: nobody's debris
    }
    // FLAT, so ask WHICH WAY IT LIES rather than how thin it is. A wing
    // and an underside plate are both thin, both ~20% of the body, and
    // both similarly elongated — share and shape cannot tell them apart
    // (measured: 0.194 vs 0.214 share, 0.011 vs 0.031 flatness). What
    // separates them is DIRECTION. A wing reaches away from the core
    // along its own long axis; a plate lies flat against the core, so its
    // offset runs along its THIN axis. Measured on the two fixtures the
    // tests use, this fraction is 1.00 for a wing and 0.00 for a plate —
    // a clean split, not a threaded needle.
    let c = centroid(pts);
    let off = c - core_c;
    let d = off.length();
    if d < 1e-4 {
        return true; // sitting on the core's own centre: not reaching anywhere
    }
    let long_axis = if e.x >= e.y && e.x >= e.z {
        Vec3::X
    } else if e.y >= e.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let along = (off / d).dot(long_axis).abs();
    along < REACHES_OUT
}

/// Decompose a body into a core and appendages, joining orphan segments
/// into whichever group is nearest as it goes.
///
/// `prims` is each prim's points, in model space and already oriented.
pub fn skeletonize(prims: &[&[Vec3]]) -> Skeleton {
    let mut sk = Skeleton::default();
    let n = prims.len();
    if n == 0 {
        return sk;
    }
    let all: Vec<Vec3> = prims.iter().flat_map(|p| p.iter().copied()).collect();
    if all.is_empty() {
        return sk;
    }
    let total = all.len() as f32;
    let (lo, hi) = bounds(&all);
    let half = (hi - lo) * 0.5;
    // THE MIDDLE IS WHERE THE MASS IS, not the centre of the bounding box.
    // A bbox centre is dragged by whatever sticks out furthest, however
    // little of the body that is. Measured on the test jet: a 90-point
    // tail fin at y=0.9 pulls mid.y to 0.55, so the 500-point fuselage —
    // dead centre in x and z, half the model by point count — scores
    // d_norm 0.524 against CORE_R 0.42 and is NOT core. Nothing is, so
    // the "biggest prim is the core" fallback fires, both wings are then
    // adopted as orphans of that one prim, and a jet with two wings
    // reports ONE limb. That is the whole of "the wings did not read as
    // limbs" — and it is why `compact` looked like a no-op too: with the
    // wings swallowed into the core there was no overhang left to shorten.
    //
    // The centroid of all points moves with the bulk instead of with the
    // extremities, which is what "prims whose mass sits near the middle"
    // meant in the first place. `half` stays bbox-derived: it is the
    // SCALE the distance is normalised by, and the body's true extent is
    // exactly what that should be.
    let mid = centroid(&all);
    let scale = half.max_element().max(1e-4);

    // THE CORE: prims whose mass sits near the middle. Everything else is
    // a candidate appendage. Deliberately not "the biggest prim" — a
    // vehicle's core is often several pieces (floor, cabin, engine cover)
    // and a bot's torso nearly always is.
    let cents: Vec<Vec3> = prims.iter().map(|p| centroid(p)).collect();
    let half_safe = half.max(Vec3::splat(1e-4));
    for (i, c) in cents.iter().enumerate() {
        if prims[i].is_empty() {
            continue;
        }
        let d = ((*c - mid) / half_safe).length();
        if d >= CORE_R {
            continue;
        }
        // CENTROID-CENTRAL IS NOT THE SAME AS SMALL. A prim can sit
        // dead centre by that measure and still reach far in one
        // direction — e.g. an oversized wing whose root is planted at
        // the body's middle. Checked on `starlord`: prim 34 (an
        // oversized Wing) passes the centroid check outright (23 of 37
        // prims do) and becomes core, so it is never evaluated as a
        // limb at all (`trace_compact assets/bots starlord 34`).
        // Fix: also require the prim's FARTHEST point — not just its
        // centroid — to sit within CORE_R, using the same threshold
        // rather than inventing a second one. A prim that is genuinely
        // core-sized clears this trivially (its own extent is small
        // relative to the body); one that merely roots centrally but
        // reaches outward does not.
        let reaches_out = prims[i].iter().any(|p| ((*p - mid) / half_safe).length() >= CORE_R);
        if !reaches_out {
            sk.core.push(i);
        }
    }
    // a body with no middle is one piece: fall back to whichever SINGLE
    // prim's own centroid sits closest to the whole body's mass-weighted
    // middle, so the rest still has something to attach to.
    //
    // This used to be "the biggest prim by point count", and that is a
    // real bug, not a hypothetical: reproduced with a body of 25 small
    // detail prims plus one large off-center appendage (25 x 8-point
    // debris + 1 x 200-point offender) — the offender IS the biggest
    // prim, so it was picked as the core itself, every debris prim was
    // then adopted as ITS orphan, and the appendage the whole system
    // exists to shorten never became a limb at all. Confirmed the same
    // failure on five real chassis (`examples/trace_compact.rs`):
    // hardboil, tank, jet_alphastar, starlord, void all reported ZERO
    // limbs despite `starlord` and `void` visibly having an oversized
    // Wing/Wheel/Thruster respectively (`silhouette_audit.rs`) that
    // `compact` should have been shortening every frame.
    //
    // Closest-to-middle is what "the core" is supposed to mean in the
    // first place — point count is a proxy for "big", and the biggest
    // thing in a body is not reliably the thing nearest its middle.
    if sk.core.is_empty() {
        if let Some((i, _)) = cents
            .iter()
            .enumerate()
            .filter(|(i, _)| !prims[*i].is_empty())
            .min_by(|(_, a), (_, b)| {
                let da = ((**a - mid) / half.max(Vec3::splat(1e-4))).length_squared();
                let db = ((**b - mid) / half.max(Vec3::splat(1e-4))).length_squared();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
        {
            sk.core.push(i);
        }
    }
    let core_pts: Vec<Vec3> =
        sk.core.iter().flat_map(|i| prims[*i].iter().copied()).collect();
    let (clo, chi) = bounds(&core_pts);
    sk.core_centre = (clo + chi) * 0.5;
    sk.core_half = (chi - clo) * 0.5;

    // Each non-core prim starts as its own group; the recursive join then
    // merges the small and flat ones into their neighbours.
    let mut groups: Vec<Vec<usize>> =
        (0..n).filter(|i| !sk.core.contains(i) && !prims[*i].is_empty()).map(|i| vec![i]).collect();
    sk.joined = join_orphans(prims, &mut groups, &sk.core, total, scale);

    for g in groups {
        let pts: Vec<Vec3> = g.iter().flat_map(|i| prims[*i].iter().copied()).collect();
        if pts.is_empty() {
            continue;
        }
        // the ROOT is where this thing meets the core: the core-surface
        // point nearest its mass, clamped into the core's box rather than
        // taken as the core centre — a wing roots at the shoulder, not at
        // the sternum, and pulling it toward the sternum is what makes a
        // compacted body look melted.
        let c = centroid(&pts);
        let root = c.clamp(sk.core_centre - sk.core_half, sk.core_centre + sk.core_half);
        // the TIP is the point furthest from the root, which is the real
        // extent of the appendage along its own axis
        let mut tip = c;
        let mut far = 0.0f32;
        for p in &pts {
            let d = (*p - root).length();
            if d > far {
                far = d;
                tip = *p;
            }
        }
        sk.limbs.push(Limb { prims: g, root, tip, reach: far });
    }
    sk.limbs.sort_by(|a, b| b.reach.partial_cmp(&a.reach).unwrap_or(std::cmp::Ordering::Equal));
    sk
}

/// THE RECURSIVE JOIN. Each pass, every group that is still small or flat
/// looks for the nearest group (or the core) within reach and is adopted
/// by it. Adoption grows the adopter, which can bring a further segment
/// into range next pass — so this runs to a fixpoint rather than once.
///
/// Neighbour lookup is by voxel bucket: groups are hashed into a grid at
/// the adoption radius, and a candidate only tests the 27 cells around it.
/// All-pairs would be 160k tests per pass on a 400-prim body.
fn join_orphans(
    prims: &[&[Vec3]],
    groups: &mut Vec<Vec<usize>>,
    core: &[usize],
    total: f32,
    scale: f32,
) -> usize {
    use std::collections::HashMap;
    let cell = (scale * 0.25).max(1e-3);
    let key = |p: Vec3| -> (i32, i32, i32) {
        ((p.x / cell) as i32, (p.y / cell) as i32, (p.z / cell) as i32)
    };
    let core_pts: Vec<Vec3> = core.iter().flat_map(|i| prims[*i].iter().copied()).collect();
    let core_c = centroid(&core_pts);
    let (clo, chi) = bounds(&core_pts);
    let core_half = (chi - clo) * 0.5;
    let mut joined = 0;

    for _ in 0..MAX_PASSES {
        // measure every group once per pass
        let mut info: Vec<(Vec3, f32, f32)> = Vec::with_capacity(groups.len());
        for g in groups.iter() {
            let pts: Vec<Vec3> = g.iter().flat_map(|i| prims[*i].iter().copied()).collect();
            let c = centroid(&pts);
            let (lo, hi) = bounds(&pts);
            let size = ((hi - lo) * 0.5).max_element().max(1e-4);
            info.push((c, size, pts.len() as f32 / total));
        }
        // voxel buckets over group centroids
        let mut grid: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
        for (i, (c, _, _)) in info.iter().enumerate() {
            grid.entry(key(*c)).or_default().push(i);
        }
        // find one adoption per pass per orphan, then apply
        let mut adopt: Vec<(usize, usize)> = Vec::new(); // (orphan, parent)
        let mut into_core: Vec<usize> = Vec::new();
        for (i, (c, _, share)) in info.iter().enumerate() {
            let pts: Vec<Vec3> = groups[i].iter().flat_map(|k| prims[*k].iter().copied()).collect();
            if !orphanish(&pts, *share, core_c) {
                continue;
            }
            // the core adopts first when it is closest — a flat underside
            // belongs to the body, not to the nearest wheel
            let to_core = (*c - c.clamp(core_c - core_half, core_c + core_half)).length();
            let mut best: Option<(f32, usize)> = None;
            let (kx, ky, kz) = key(*c);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(bucket) = grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                            continue;
                        };
                        for j in bucket {
                            if *j == i {
                                continue;
                            }
                            let (jc, jsize, jshare) = info[*j];
                            // an orphan does not adopt an orphan: that
                            // makes drifting clumps instead of a body
                            let jpts: Vec<Vec3> =
                                groups[*j].iter().flat_map(|k| prims[*k].iter().copied()).collect();
                            if orphanish(&jpts, jshare, core_c) {
                                continue;
                            }
                            let d = (jc - *c).length();
                            if d < jsize * ADOPT_R && best.map(|b| d < b.0).unwrap_or(true) {
                                best = Some((d, *j));
                            }
                        }
                    }
                }
            }
            match best {
                Some((d, j)) if d <= to_core => adopt.push((i, j)),
                _ if to_core < core_half.max_element() * ADOPT_R => into_core.push(i),
                Some((_, j)) => adopt.push((i, j)),
                None => {}
            }
        }
        if adopt.is_empty() && into_core.is_empty() {
            break; // fixpoint
        }
        // apply, highest index first so removal does not shift the rest
        let mut moved: Vec<usize> = Vec::new();
        for (orphan, parent) in &adopt {
            let taken = groups[*orphan].clone();
            groups[*parent].extend(taken);
            moved.push(*orphan);
        }
        // (core adoption drops the group; the core's own prim list is not
        // extended, because the caller's `core` is borrowed — the effect
        // that matters is the same: it stops being a floating limb)
        moved.extend(into_core);
        moved.sort_unstable();
        moved.dedup();
        joined += moved.len();
        for i in moved.into_iter().rev() {
            groups.remove(i);
        }
    }
    joined
}

/// PULL THE APPENDAGES IN. Returns a per-prim translation that shortens
/// every limb toward its root so the longest reach becomes `factor` of
/// what it was — half, by the request that prompted this.
///
/// Along the limb's OWN axis, and proportional to how far out each point
/// already is, so a wing shortens from the tip and stays attached at the
/// shoulder. A uniform scale would shrink the whole appendage including
/// its root joint and leave a visible gap where it met the body.
///
/// Only limbs longer than the core's own half-extent are touched: a
/// wheel is an appendage too, and pulling wheels into the chassis is not
/// what anybody asked for.
pub fn compact(sk: &Skeleton, prim_count: usize, factor: f32) -> Vec<Vec3> {
    let mut out = vec![Vec3::ZERO; prim_count];
    let keep = sk.core_half.max_element().max(1e-3);
    for limb in &sk.limbs {
        if limb.reach <= keep {
            continue; // short enough to be part of the silhouette already
        }
        // how much of the overhang to remove
        let target = (limb.reach * factor).max(keep);
        let cut = limb.reach - target;
        if cut <= 1e-4 {
            continue;
        }
        let axis = (limb.tip - limb.root).normalize_or(Vec3::ZERO);
        if axis == Vec3::ZERO {
            continue;
        }
        for i in &limb.prims {
            if let Some(slot) = out.get_mut(*i) {
                *slot = -axis * cut;
            }
        }
    }
    out
}

/// PACK THE APPENDAGES INSIDE THE HULL, for vehicle mode.
///
/// `compact` shortens a limb by half so a standing bot is not swallowed by
/// its own wingspan. Vehicle mode wants something different and stricter:
/// a folded car should read as a solid object, and instead the skeleton
/// shows — limbs hanging outside the silhouette, so the fold looks like a
/// bounding box with parts stuck to it rather than a vehicle.
///
/// So this does not shorten by a ratio, it CONTAINS: every prim is pulled
/// as far inside the hull envelope as TRANSLATION can put it, the
/// envelope being the core's box grown by `slack` and no further. A prim
/// already inside does not move at all, which is the important half — a
/// car whose parts are where they belong is untouched, and only what
/// pokes out gets pulled in.
///
/// WHAT THIS CANNOT DO, because it only moves things: a limb WIDER than
/// the envelope cannot be made to fit inside it. The jet's wing spans 5.2
/// against a 1.25-wide envelope, so the best any translation achieves is
/// centring it — 1.98 of overhang on each side instead of 3.95 out one
/// side. Fitting that wing inside the silhouette needs it ROTATED (a real
/// fold: swing-wings hinge, they do not slide), which is a different
/// operation on a different data model and is not what this function
/// promises.
///
/// The envelope is the CORE's box rather than the whole body's, because
/// the whole body's box is exactly the thing the appendages defined: using
/// it would contain nothing.
/// FOLD a limb toward the body by HINGING it at its root, which is the
/// thing `contain` cannot do.
///
/// `contain` translates, and translation cannot make an oversized limb
/// fit: a 5.2 m wing does not go inside a 1.25 m envelope by sliding,
/// only by swinging. Real transforming vehicles agree — swing-wings
/// hinge, undercarriage retracts on an arm, a robot's forearm folds at
/// the elbow. Nothing was rotating anywhere in this codebase's fold path,
/// so a "vehicle mode" was a body with its limbs shoved as far inboard as
/// translation allowed and still sticking out.
///
/// Returns a per-limb rotation about `root`, as an axis-angle `Quat`, at
/// fold parameter `t` (0 = robot/extended, 1 = vehicle/stowed). The
/// caller applies it to that limb's prims — this returns the transform
/// rather than moving points, so the same answer drives the renderer, a
/// collision rebuild and a test without three copies of the arithmetic.
///
/// The hinge axis is the limb's own reach crossed with the body's up:
/// a wing sticking out sideways swings BACKWARD (its axis is vertical),
/// a leg hanging down swings UP AND FORWARD. That falls out of the
/// geometry rather than needing a per-part authored axis, which matters
/// because these skeletons are DERIVED from arbitrary meshes and nobody
/// is there to author one.
pub fn fold_rotation(limb: &Limb, up: Vec3, t: f32) -> Quat {
    // NON-FINITE GEOMETRY IS A LIMB WITH NOTHING TO SWING, not a crash.
    //
    // These skeletons come from arbitrary third-party meshes, and a NaN
    // vertex reaching here used to PANIC — `min_by` below compares
    // `dir.dot(axis)` with `partial_cmp().unwrap()`, and NaN compares to
    // nothing, so the unwrap fires. A corrupt model should cost one limb,
    // not the process.
    let reach = limb.tip - limb.root;
    let len = reach.length();
    if !len.is_finite() || len < 1e-4 {
        return Quat::IDENTITY; // a limb with no extent has nothing to swing
    }
    // A NaN `t` produced a NaN quaternion, which does not fold a limb
    // wrong — it deletes it, because every point it transforms becomes
    // NaN. Unfolded is the safe reading of "no usable amount".
    let t = if t.is_finite() { t } else { 0.0 };
    let dir = reach / len;
    let up = up.normalize_or(Vec3::Y);
    // The axis is perpendicular to the limb AND TO WHERE IT IS GOING, not
    // to the limb and up — that was the first version's mistake and it
    // swung a sideways wing straight upward (measured: tip (5.2,0,0)
    // landed at (0.6,4.6,0), a wing standing on end). A limb stows ALONG
    // the body, so the destination is the body's long axis: the component
    // of "not up" that the limb is not already using.
    //
    // `along` is the body direction most orthogonal to the limb, chosen
    // from the two non-up axes so a wing (out along x) swings into z and a
    // limb already along z swings into x. Deriving it beats authoring it:
    // these skeletons come from arbitrary meshes with nobody to annotate
    // them.
    let cands = [Vec3::X, Vec3::Y, Vec3::Z];
    let along = cands
        .iter()
        .copied()
        .filter(|c| c.dot(up).abs() < 0.9) // not the up axis: stow flat, not vertical
        // `unwrap` here was the panic path above; total ordering keeps it
        // total even if a NaN ever reaches this point by another route.
        .min_by(|a, b| {
            dir.dot(*a)
                .abs()
                .partial_cmp(&dir.dot(*b).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(Vec3::Z);
    let axis = {
        let c = dir.cross(along);
        // Limb already lying along its destination, or straight up a
        // degenerate cross: any perpendicular will do, and picking one
        // beats returning identity — the part most in need of stowing
        // would otherwise be the one part that never moves.
        if c.length() > 1e-3 { c.normalize() } else { dir.any_orthonormal_vector() }
    };
    // A quarter turn stows a limb alongside the body; that is the whole
    // useful range, and going further swings it out the other side.
    Quat::from_axis_angle(axis, t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2)
}

/// Where a point ends up after its limb folds — the caller-side half of
/// [`fold_rotation`], kept here so the rotate-about-root convention lives
/// in one place. Rotating about the ORIGIN instead is the obvious mistake
/// and detaches the limb from the body.
pub fn fold_point(p: Vec3, limb: &Limb, up: Vec3, t: f32) -> Vec3 {
    limb.root + fold_rotation(limb, up, t) * (p - limb.root)
}

/// The per-limb containment shift, given the limb's bounds and the
/// envelope. Extracted from [`contain`] so the hinged fold can run the
/// SAME arithmetic on rotated bounds — two copies of the centring cap
/// would drift.
fn contain_shift(lo: Vec3, hi: Vec3, elo: Vec3, ehi: Vec3, core_centre: Vec3) -> Vec3 {
    // how far each axis pokes out, in the direction it pokes
    let over_hi = (hi - ehi).max(Vec3::ZERO);
    let over_lo = (elo - lo).max(Vec3::ZERO);
    let mut shift = over_lo - over_hi;
    // Never push a limb THROUGH the body — but the cap has to be the
    // shift that CENTRES it, not `envelope - span`. For any limb WIDER
    // than the envelope that formula is zero, so the pull is clamped
    // to nothing and the limb does not move at all: on the test jet
    // the envelope is 1.25 across and a wing spans 5.2, so cap.x = 0
    // and the widest limbs — exactly the ones containment is for —
    // were the only ones it could never touch.
    //
    // Centring is the real limit. It is the furthest a limb can travel
    // while still being pulled toward the body rather than past it, it
    // is never zero, and for an oversized limb it splits the overhang
    // evenly instead of driving one end out the far side (measured on
    // the jet's wing: 1.98 each side, against 3.95 out one side if the
    // raw shift is applied uncapped).
    let centre_shift = core_centre - (lo + hi) * 0.5;
    let cap = centre_shift.abs();
    shift.clamp(-cap, cap)
}

pub fn contain(sk: &Skeleton, prims: &[&[Vec3]], slack: f32) -> Vec<Vec3> {
    let mut out = vec![Vec3::ZERO; prims.len()];
    if sk.core_half.max_element() <= 1e-4 {
        return out;
    }
    let env = sk.core_half * (1.0 + slack.max(0.0));
    let (elo, ehi) = (sk.core_centre - env, sk.core_centre + env);
    for limb in &sk.limbs {
        // measure the limb as a whole, so its pieces move together and it
        // does not come apart on the way in
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in &limb.prims {
            for p in prims.get(*i).copied().unwrap_or(&[]) {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        if !(hi.x >= lo.x) {
            continue;
        }
        let shift = contain_shift(lo, hi, elo, ehi, sk.core_centre);
        if shift.length_squared() < 1e-8 {
            continue;
        }
        for i in &limb.prims {
            if let Some(slot) = out.get_mut(*i) {
                *slot = shift;
            }
        }
    }
    out
}

/// One prim's share of the VEHICLE-MODE fold: hinge about the limb root,
/// then a containment translation computed on the ROTATED bounds.
///
/// This is the answer to the composition question `fold_rotation` left
/// open ("how a limb's rotation should COMPOSE with the containment
/// translation"): rotate FIRST, contain what is left. The hinge is the
/// physically-real mechanism — swing-wings hinge, undercarriage folds on
/// an arm — so it does what a hinge can, and translation finishes the
/// job on the silhouette the hinge produced. The other order (contain
/// then rotate) measures the overhang of geometry that is about to move,
/// which is answering the wrong question.
///
/// Identity for core prims and for every limb that translation ALONE can
/// fit — a bonnet stays a bonnet. The hinge is reserved for limbs that
/// are WIDER than the envelope on an axis they poke out of, which is
/// precisely the case `contain`'s own doc names as impossible for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoldStep {
    pub rot: Quat,
    /// hinge point, meaningful only when `rot != IDENTITY`
    pub pivot: Vec3,
    pub shift: Vec3,
}

impl Default for FoldStep {
    fn default() -> Self {
        FoldStep { rot: Quat::IDENTITY, pivot: Vec3::ZERO, shift: Vec3::ZERO }
    }
}

pub fn fold_pack(sk: &Skeleton, prims: &[&[Vec3]], slack: f32, up: Vec3) -> Vec<FoldStep> {
    let mut out = vec![FoldStep::default(); prims.len()];
    if sk.core_half.max_element() <= 1e-4 {
        return out;
    }
    let env = sk.core_half * (1.0 + slack.max(0.0));
    let (elo, ehi) = (sk.core_centre - env, sk.core_centre + env);
    for limb in &sk.limbs {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in &limb.prims {
            for p in prims.get(*i).copied().unwrap_or(&[]) {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        if !(hi.x >= lo.x) {
            continue;
        }
        // translation suffices unless the limb is wider than the whole
        // envelope on an axis it pokes out of — the jet-wing case
        let span = hi - lo;
        let pokes = (hi - ehi).max(Vec3::ZERO) + (elo - lo).max(Vec3::ZERO);
        let needs_hinge = (0..3).any(|a| pokes[a] > 1e-4 && span[a] > 2.0 * env[a]);
        // KEEP THE HINGE ONLY IF IT HELPS.
        //
        // `needs_hinge` fires when a limb pokes out and spans more than
        // twice the envelope on that axis, which is the wing case the
        // rotation exists for. But a limb oversized on EVERY axis cannot
        // be helped by rotating: turning it only changes which axis pokes
        // furthest. Measured on truck_lynx_dominante_av_445 — a limb
        // spanning 2.00 x 1.61 x 1.33 inside a 0.72 x 0.72 x 0.33
        // envelope, hinged, overhang 0.639 before and 0.702 after. The
        // hinge made containment WORSE and nothing checked.
        //
        // Both candidates are now built and the better one wins, measured
        // the same way the caller cares about: the furthest any point
        // still pokes out of the envelope. A rotation that does not
        // improve that is not worth the risk of decoupling a limb.
        let overhang_of = |step: &FoldStep| -> f32 {
            let mut worst = 0.0f32;
            for i in &limb.prims {
                for p in prims.get(*i).copied().unwrap_or(&[]) {
                    let q = step.pivot + step.rot * (*p - step.pivot) + step.shift;
                    let out = (elo - q).max(q - ehi).max(Vec3::ZERO);
                    worst = worst.max(out.max_element());
                }
            }
            worst
        };
        let flat_step = FoldStep {
            shift: contain_shift(lo, hi, elo, ehi, sk.core_centre),
            ..FoldStep::default()
        };
        let step = if needs_hinge {
            let rot = fold_rotation(limb, up, 1.0);
            // bounds of the limb AFTER the hinge — the silhouette the
            // containment translation actually has to deal with
            let (mut rlo, mut rhi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for i in &limb.prims {
                for p in prims.get(*i).copied().unwrap_or(&[]) {
                    let q = limb.root + rot * (*p - limb.root);
                    rlo = rlo.min(q);
                    rhi = rhi.max(q);
                }
            }
            let hinged = FoldStep {
                rot,
                pivot: limb.root,
                shift: contain_shift(rlo, rhi, elo, ehi, sk.core_centre),
            };
            if overhang_of(&hinged) <= overhang_of(&flat_step) {
                hinged
            } else {
                flat_step
            }
        } else {
            flat_step
        };
        if step == FoldStep::default() {
            continue;
        }
        for i in &limb.prims {
            if let Some(slot) = out.get_mut(*i) {
                *slot = step;
            }
        }
    }
    out
}

// ------------------------------------------------------------- limb roles --
//
// WHAT `anim::Pose` HAS BEEN COMPUTING FOR.
//
// `Pose` produces `leg_l`, `leg_r`, `arm_l`, `arm_r` every frame and its own
// comment says nobody consumes them: *"the bodies are pose strips, not rigs —
// but they are what a rigged bot would be driven by."* The reason is that
// `skeletonize` finds appendages and names them nothing: a limb is a bag of
// prims with a root, a tip and a reach, so "swing the left leg" has no
// address to send the rotation to.
//
// The shapes are already the evidence. What is missing is the inference from
// shape to role, and it is a small extension of a decomposition that exists
// rather than a new system. Given the canonical frame (Y up, -Z forward, so
// +X is the body's right), a limb's ROOT relative to the core says which it
// is: hanging below the core is a leg, reaching outboard near the top is an
// arm, and the sign of its x picks the side.
//
// This is a measurement, so it can be wrong, and being wrong here means a
// walk cycle driving the wrong part. `Other` is therefore the default and
// the classifier is deliberately reluctant: a limb that is not clearly one
// thing gets no channel and keeps whatever the fold gives it.

/// Which animation channel a limb answers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimbRole {
    LegL,
    LegR,
    ArmL,
    ArmR,
    /// A wing, a nacelle, a gun, a tail — real geometry with no channel in
    /// `anim::Pose`. Not a failure: most of a jet is this.
    Other,
}

impl LimbRole {
    pub fn label(self) -> &'static str {
        match self {
            LimbRole::LegL => "leg.l",
            LimbRole::LegR => "leg.r",
            LimbRole::ArmL => "arm.l",
            LimbRole::ArmR => "arm.r",
            LimbRole::Other => "other",
        }
    }
    pub fn is_leg(self) -> bool {
        matches!(self, LimbRole::LegL | LimbRole::LegR)
    }
    pub fn is_arm(self) -> bool {
        matches!(self, LimbRole::ArmL | LimbRole::ArmR)
    }
    /// The swing this role takes from a pose, in radians. The one place the
    /// four channels are addressed, so a renderer cannot pair them up wrong.
    pub fn swing(self, pose: &crate::anim::Pose) -> f32 {
        match self {
            LimbRole::LegL => pose.leg_l,
            LimbRole::LegR => pose.leg_r,
            LimbRole::ArmL => pose.arm_l,
            LimbRole::ArmR => pose.arm_r,
            LimbRole::Other => 0.0,
        }
    }
}

/// How far below the core's centre a root must sit, as a fraction of the
/// core's own half-height, before it counts as hanging underneath.
const LEG_DROP: f32 = 0.30;
/// How far out from the centreline, as a fraction of the core's half-width,
/// before a root counts as outboard rather than part of the trunk.
const ARM_OUT: f32 = 0.55;
/// A root this close to the centreline has no side. Fraction of half-width.
const SIDE_DEAD: f32 = 0.12;

/// Name each limb, in the same order as `Skeleton::limbs`.
///
/// Measured against the CORE, not the whole body: a body's overall bounding
/// box includes the limbs being classified, so using it would let a long leg
/// move the reference that decides whether it is a leg.
pub fn limb_roles(sk: &Skeleton) -> Vec<LimbRole> {
    let c = sk.core_centre;
    let h = sk.core_half;
    let hy = h.y.max(1e-4);
    let hx = h.x.max(1e-4);
    let mut roles: Vec<LimbRole> = Vec::with_capacity(sk.limbs.len());
    for l in &sk.limbs {
        let dy = (l.root.y - c.y) / hy;
        let dx = (l.root.x - c.x) / hx;
        let side_known = dx.abs() >= SIDE_DEAD;
        let right = dx > 0.0;
        let role = if dy <= -LEG_DROP && side_known {
            // hangs below the trunk: a leg, and it is the leg on the side
            // its root sits on
            if right { LimbRole::LegR } else { LimbRole::LegL }
        } else if dx.abs() >= ARM_OUT && side_known {
            // reaches out from the trunk without hanging below it: an arm.
            //
            // ORDERING IS THE GUARD, not a second condition. This branch used
            // to also test `dy > -LEG_DROP`, which can never be false here —
            // the leg arm above has already taken everything below the line.
            // A redundant condition reads like a check that is happening, and
            // the car test passed against its removal, which is how it was
            // found: the pass was guaranteed by the branch order, not by the
            // clause. A wide-track car's wheels ARE outboard enough to match
            // `ARM_OUT`; what stops them being arms is that they hang low and
            // the leg branch claims them first.
            if right { LimbRole::ArmR } else { LimbRole::ArmL }
        } else {
            LimbRole::Other
        };
        roles.push(role);
    }
    roles
}

/// `limb_roles`, with unpaired limbs demoted to `Other`.
///
/// A walk cycle is a pair of legs in counter-phase. One leg is not half a
/// walk — it is a misclassification, or a body that does not walk, and
/// swinging it alone reads as a twitch rather than a stride. Same for arms.
/// Called instead of `limb_roles` wherever the roles will actually DRIVE
/// something; the raw version stays for diagnostics, which want to see the
/// unpaired call rather than have it hidden.
pub fn limb_roles_paired(sk: &Skeleton) -> Vec<LimbRole> {
    let mut roles = limb_roles(sk);
    let count = |r: LimbRole, v: &[LimbRole]| v.iter().filter(|x| **x == r).count();
    for (a, b) in [(LimbRole::LegL, LimbRole::LegR), (LimbRole::ArmL, LimbRole::ArmR)] {
        if count(a, &roles) == 0 || count(b, &roles) == 0 {
            for r in roles.iter_mut() {
                if *r == a || *r == b {
                    *r = LimbRole::Other;
                }
            }
        }
    }
    roles
}

#[cfg(test)]
mod role_tests {
    use super::*;

    /// A box of points, so a synthetic body is readable as geometry rather
    /// than as a wall of coordinates.
    fn slab(c: Vec3, half: Vec3, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let t = i as f32 * 2.399;
                let u = (i as f32 / n as f32) * 2.0 - 1.0;
                c + Vec3::new(t.cos() * half.x, u * half.y, t.sin() * half.z)
            })
            .collect()
    }

    /// torso + two legs below + two arms outboard, in the canonical frame
    /// (Y up, -Z forward, +X the body's right).
    fn humanoid() -> Vec<Vec<Vec3>> {
        vec![
            slab(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.45, 0.55, 0.30), 400), // torso
            slab(Vec3::new(-0.25, 0.5, 0.0), Vec3::new(0.14, 0.55, 0.14), 90), // left leg
            slab(Vec3::new(0.25, 0.5, 0.0), Vec3::new(0.14, 0.55, 0.14), 90),  // right leg
            slab(Vec3::new(-0.72, 1.7, 0.0), Vec3::new(0.12, 0.45, 0.12), 80), // left arm
            slab(Vec3::new(0.72, 1.7, 0.0), Vec3::new(0.12, 0.45, 0.12), 80),  // right arm
        ]
    }

    fn roles_of(prims: &[Vec<Vec3>]) -> (Skeleton, Vec<LimbRole>) {
        let refs: Vec<&[Vec3]> = prims.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let roles = limb_roles_paired(&sk);
        (sk, roles)
    }

    #[test]
    fn a_humanoid_gets_two_legs_and_two_arms_on_the_right_sides() {
        let body = humanoid();
        let (sk, roles) = roles_of(&body);
        assert!(sk.limbs.len() >= 4, "the skeleton found {} limbs: {}", sk.limbs.len(), sk.report());
        let has = |r: LimbRole| roles.contains(&r);
        assert!(has(LimbRole::LegL) && has(LimbRole::LegR), "legs not found: {roles:?}");
        assert!(has(LimbRole::ArmL) && has(LimbRole::ArmR), "arms not found: {roles:?}");
        // and the sides are not swapped: +X is the body's right
        for (l, r) in sk.limbs.iter().zip(&roles) {
            match r {
                LimbRole::LegR | LimbRole::ArmR => {
                    assert!(l.root.x > sk.core_centre.x, "{} rooted on the left", r.label())
                }
                LimbRole::LegL | LimbRole::ArmL => {
                    assert!(l.root.x < sk.core_centre.x, "{} rooted on the right", r.label())
                }
                LimbRole::Other => {}
            }
        }
    }

    #[test]
    fn a_car_gets_no_arms() {
        // four wheels under a body. Every appendage hangs below the core, so
        // the arm rule must not fire — a car waving is the failure this
        // guards, and it is the shape most of the roster actually is.
        let mut prims = vec![slab(Vec3::new(0.0, 0.8, 0.0), Vec3::new(0.9, 0.35, 2.0), 500)];
        for (x, z) in [(-0.8f32, 1.4f32), (0.8, 1.4), (-0.8, -1.4), (0.8, -1.4)] {
            prims.push(slab(Vec3::new(x, 0.3, z), Vec3::new(0.16, 0.30, 0.30), 70));
        }
        let (sk, roles) = roles_of(&prims);
        // THE TEST MUST EXERCISE THE ARM RULE, not miss it. These wheels sit
        // outboard past ARM_OUT, so they reach the arm branch's width test
        // and are rejected only because the leg branch claimed them first.
        // Without this assertion the test passes on any fixture whose
        // appendages are simply too narrow to be arms — which is what it was
        // doing when a regression removing the guard went unnoticed.
        let hx = sk.core_half.x.max(1e-4);
        let outboard = sk
            .limbs
            .iter()
            .filter(|l| ((l.root.x - sk.core_centre.x) / hx).abs() >= ARM_OUT)
            .count();
        assert!(
            outboard >= 2,
            "the fixture's wheels are not outboard enough to test the arm rule ({outboard} of {})",
            sk.limbs.len()
        );
        assert!(
            !roles.iter().any(|r| r.is_arm()),
            "a car was given arms: {roles:?}"
        );
    }

    #[test]
    fn an_unpaired_limb_is_demoted_rather_than_swung_alone() {
        // CLAUDE.md §5c — the complement. One leg is not half a walk; it is a
        // misclassification, and swinging it alone reads as a twitch. The raw
        // classifier may still call it a leg; the paired one must not.
        let prims = vec![
            slab(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.45, 0.55, 0.30), 400),
            slab(Vec3::new(-0.25, 0.5, 0.0), Vec3::new(0.14, 0.55, 0.14), 90),
        ];
        let refs: Vec<&[Vec3]> = prims.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let paired = limb_roles_paired(&sk);
        assert!(
            !paired.iter().any(|r| r.is_leg()),
            "a lone leg survived pairing: {paired:?}"
        );
    }

    #[test]
    fn every_role_addresses_a_distinct_pose_channel() {
        // The binding is the point of the whole module: four roles, four
        // channels, no two reading the same one. A copy-paste that made both
        // arms read arm_l would pass any test that only checks a limb moved.
        let pose = crate::anim::Pose {
            leg_l: 0.11,
            leg_r: 0.22,
            arm_l: 0.33,
            arm_r: 0.44,
            ..Default::default()
        };
        let got = [
            LimbRole::LegL.swing(&pose),
            LimbRole::LegR.swing(&pose),
            LimbRole::ArmL.swing(&pose),
            LimbRole::ArmR.swing(&pose),
        ];
        assert_eq!(got, [0.11, 0.22, 0.33, 0.44], "a role read the wrong channel");
        assert_eq!(LimbRole::Other.swing(&pose), 0.0, "Other must take no channel");
        // distinctness, stated as its own assertion so a future edit that
        // aliases two channels fails here rather than somewhere visual
        for i in 0..got.len() {
            for j in (i + 1)..got.len() {
                assert_ne!(got[i], got[j], "roles {i} and {j} share a channel");
            }
        }
    }

    #[test]
    fn a_body_with_no_limbs_names_nothing_and_does_not_panic() {
        let one = vec![slab(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0), 200)];
        let refs: Vec<&[Vec3]> = one.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let roles = limb_roles_paired(&sk);
        assert_eq!(roles.len(), sk.limbs.len(), "roles and limbs disagree in length");
        assert!(!roles.iter().any(|r| r.is_leg() || r.is_arm()));
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_hinge_is_kept_only_when_it_improves_containment() {
        // A limb oversized on every axis cannot be helped by rotating:
        // turning it only changes which axis pokes furthest. `needs_hinge`
        // fires on span alone, so it used to hinge anyway and could leave
        // the limb further outside than it started. Measured on
        // truck_lynx_dominante_av_445: overhang 0.639 -> 0.702.
        //
        // The property that must hold for every limb and every chassis:
        // fold_pack never makes the overhang worse than doing nothing.
        let core: Vec<Vec3> = cube(Vec3::ZERO, Vec3::splat(0.5));
        // A limb far larger than the envelope in all three axes.
        let blob: Vec<Vec3> = cube(Vec3::new(1.4, 0.3, 0.2), Vec3::new(1.0, 0.8, 0.7));
        let prims: Vec<&[Vec3]> = vec![&core, &blob];
        let sk = skeletonize(&prims);
        if sk.limbs.is_empty() {
            return; // the classifier did not see a limb; nothing to assert
        }
        let slack = 0.25;
        let steps = fold_pack(&sk, &prims, slack, Vec3::Y);
        let env = sk.core_half * (1.0 + slack);
        let (elo, ehi) = (sk.core_centre - env, sk.core_centre + env);
        let overhang = |pts: &[Vec3], st: &FoldStep| -> f32 {
            let mut w = 0.0f32;
            for p in pts {
                let q = st.pivot + st.rot * (*p - st.pivot) + st.shift;
                let out = (elo - q).max(q - ehi).max(Vec3::ZERO);
                w = w.max(out.max_element());
            }
            w
        };
        for limb in &sk.limbs {
            for i in &limb.prims {
                let pts = prims[*i];
                let before = overhang(pts, &FoldStep::default());
                let after = overhang(pts, &steps[*i]);
                assert!(
                    after <= before + 1e-4,
                    "containment made it worse: {before:.3} -> {after:.3}"
                );
            }
        }
    }

    /// Eight corners of a box, enough for the skeletonizer to see a shape.
    fn cube(centre: Vec3, half: Vec3) -> Vec<Vec3> {
        let mut v = Vec::with_capacity(8);
        for sx in [-1.0f32, 1.0] {
            for sy in [-1.0f32, 1.0] {
                for sz in [-1.0f32, 1.0] {
                    v.push(centre + Vec3::new(half.x * sx, half.y * sy, half.z * sz));
                }
            }
        }
        v
    }

    /// `fold_rotation` runs on skeletons derived from arbitrary
    /// third-party meshes, so it meets geometry nobody authored.
    #[test]
    fn a_non_finite_limb_costs_one_limb_not_the_process() {
        // This PANICKED: `min_by` compares `dir.dot(axis)` through
        // `partial_cmp().unwrap()`, and NaN compares to nothing.
        let bad = Limb {
            prims: vec![0],
            root: Vec3::ZERO,
            tip: Vec3::splat(f32::NAN),
            reach: f32::NAN,
        };
        let q = fold_rotation(&bad, Vec3::Y, 1.0);
        assert_eq!(q, Quat::IDENTITY, "a limb with no usable extent stays put");
    }

    #[test]
    fn a_non_finite_fold_amount_leaves_the_limb_unfolded() {
        // A NaN quaternion does not fold a limb wrong, it deletes it:
        // every point it transforms becomes NaN.
        let l = Limb {
            prims: vec![0],
            root: Vec3::ZERO,
            tip: Vec3::new(5.0, 0.0, 0.0),
            reach: 5.0,
        };
        for t in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let q = fold_rotation(&l, Vec3::Y, t);
            assert!(q.is_finite(), "NaN quaternion from t={t}");
        }
        assert_eq!(fold_rotation(&l, Vec3::Y, f32::NAN), Quat::IDENTITY);
    }

    #[test]
    fn a_non_finite_point_does_not_spread_through_fold_point() {
        // fold_point is the caller-side half; a bad limb must not turn a
        // good point into NaN.
        let bad = Limb {
            prims: vec![0],
            root: Vec3::ZERO,
            tip: Vec3::splat(f32::NAN),
            reach: f32::NAN,
        };
        let p = fold_point(Vec3::new(1.0, 2.0, 3.0), &bad, Vec3::Y, 1.0);
        assert!(p.is_finite(), "a bad limb leaked NaN into a good point: {p:?}");
    }
    use super::*;

    fn blob(c: Vec3, r: Vec3, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let t = i as f32 * 0.7;
                c + Vec3::new(t.sin() * r.x, (t * 1.7).cos() * r.y, (t * 0.4).sin() * r.z)
            })
            .collect()
    }

    /// A jet: fuselage core, two long wings, a tail.
    fn jet() -> Vec<Vec<Vec3>> {
        vec![
            blob(Vec3::ZERO, Vec3::new(0.5, 0.5, 3.0), 500),
            blob(Vec3::new(-3.4, 0.0, 0.0), Vec3::new(2.6, 0.08, 0.9), 220),
            blob(Vec3::new(3.4, 0.0, 0.0), Vec3::new(2.6, 0.08, 0.9), 220),
            blob(Vec3::new(0.0, 0.9, 2.6), Vec3::new(0.1, 0.7, 0.5), 90),
        ]
    }

    /// A wing rooted at the body's own centre — its CENTROID sits dead
    /// central (passing the old, single-property check) but it reaches
    /// far out along its own long axis, same shape of bug as `starlord`
    /// prim 34.  Regression test for the "centroid-central AND still
    /// reaching outward" gap: a fuselage-sized blob at the origin plus
    /// one long, thin blob whose points run from the origin outward
    /// (so its centroid is near-central even though its tip is not).
    fn rooted_wing(len: f32, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1).max(1) as f32; // 0..1 along the wing
                Vec3::new(t * len, 0.05 * (i as f32 * 1.3).sin(), 0.05 * (i as f32 * 0.7).cos())
            })
            .collect()
    }

    #[test]
    fn a_centrally_rooted_prim_that_reaches_out_is_not_core() {
        let core = blob(Vec3::ZERO, Vec3::new(0.6, 0.6, 0.6), 400);
        let wing = rooted_wing(6.0, 200);
        let bodies = vec![core, wing];
        let refs: Vec<&[Vec3]> = bodies.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        assert!(!sk.core.contains(&1), "a prim reaching far out was still classified as core: {}", sk.report());
        assert!(sk.limbs.iter().any(|l| l.prims.contains(&1)), "the reaching prim was not detected as a limb: {}", sk.report());
    }

    #[test]
    fn a_vehicle_decomposes_the_same_way_a_body_does() {
        let j = jet();
        let refs: Vec<&[Vec3]> = j.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        assert!(sk.core.contains(&0), "the fuselage is not the core: {}", sk.report());
        assert!(sk.limbs.len() >= 2, "the wings did not read as limbs: {}", sk.report());
        assert!(sk.max_reach() > 2.0, "no long appendage found: {}", sk.report());
    }

    #[test]
    fn compaction_halves_the_overhang_without_detaching_it() {        let j = jet();
        let refs: Vec<&[Vec3]> = j.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let before = sk.max_reach();
        let off = compact(&sk, refs.len(), 0.5);
        // apply and re-measure
        let moved: Vec<Vec<Vec3>> = j
            .iter()
            .enumerate()
            .map(|(i, p)| p.iter().map(|v| *v + off[i]).collect())
            .collect();
        let mrefs: Vec<&[Vec3]> = moved.iter().map(|v| v.as_slice()).collect();
        let after = skeletonize(&mrefs).max_reach();
        assert!(after < before * 0.72, "the wing barely moved: {before} -> {after}");
        // and the wing prims moved toward the body, not away
        let wing_before = centroid(&j[1]).length();
        let wing_after = centroid(&moved[1]).length();
        assert!(wing_after < wing_before, "the wing was pushed outward");
    }

    #[test]
    fn a_hinged_wing_fits_where_a_slid_one_could_not() {
        // The point of the whole thing. `contain` can only translate, and
        // a 5.2-long wing cannot be translated inside a narrow envelope —
        // the containment test had to be relaxed to "centred" for exactly
        // this reason. Hinging it removes the excess instead of splitting
        // it: a wing swung a quarter turn occupies its own LENGTH along
        // the body rather than across it.
        let limb = Limb {
            prims: vec![1],
            root: Vec3::new(0.6, 0.0, 0.0),
            tip: Vec3::new(5.2, 0.0, 0.0),
            reach: 4.6,
        };
        let tip_out = limb.tip;
        let tip_folded = fold_point(tip_out, &limb, Vec3::Y, 1.0);
        assert!(
            tip_folded.x.abs() < tip_out.x.abs(),
            "the wingtip did not come inboard: {tip_out:?} -> {tip_folded:?}"
        );
        // It swung about the vertical axis, so it moved in Z (along the
        // body) and stayed level — a wing folding DOWN through the hull
        // would be worse than not folding.
        assert!(tip_folded.z.abs() > 1.0, "it did not swing back: {tip_folded:?}");
        assert!(tip_folded.y.abs() < 1e-3, "it dropped out of plane: {tip_folded:?}");
    }

    #[test]
    fn the_root_never_moves_and_t_zero_changes_nothing() {
        // A fold that detaches the limb from the body is the obvious
        // failure of rotating about the origin instead of the root.
        let limb = Limb {
            prims: vec![0],
            root: Vec3::new(1.0, 2.0, 3.0),
            tip: Vec3::new(1.0, 2.0, 8.0),
            reach: 5.0,
        };
        for t in [0.0, 0.25, 0.5, 1.0] {
            let r = fold_point(limb.root, &limb, Vec3::Y, t);
            assert!((r - limb.root).length() < 1e-4, "the root moved at t={t}: {r:?}");
        }
        // Robot mode is the identity: nothing should shift before the
        // transformation starts.
        let p = Vec3::new(1.0, 2.0, 8.0);
        assert!((fold_point(p, &limb, Vec3::Y, 0.0) - p).length() < 1e-4);
    }

    #[test]
    fn folding_preserves_the_limb_length() {
        // A hinge rotates; it does not stretch or crush. If this drifts,
        // the fold is a scale in disguise and the model will visibly
        // deform mid-transformation.
        let limb = Limb {
            prims: vec![0],
            root: Vec3::ZERO,
            tip: Vec3::new(3.0, 1.0, 0.0),
            reach: 3.16,
        };
        let before = (limb.tip - limb.root).length();
        for t in [0.1, 0.5, 0.9, 1.0] {
            let after = (fold_point(limb.tip, &limb, Vec3::Y, t) - limb.root).length();
            assert!((after - before).abs() < 1e-3, "length drifted at t={t}: {before} -> {after}");
        }
    }

    #[test]
    fn a_limb_lying_along_up_still_folds() {
        // A mast or a dorsal fin points the same way as `up`, so the
        // obvious axis (reach x up) vanishes. Returning identity there
        // would mean the one part most obviously in need of stowing is
        // the one part that never moves.
        let limb = Limb {
            prims: vec![0],
            root: Vec3::ZERO,
            tip: Vec3::new(0.0, 4.0, 0.0),
            reach: 4.0,
        };
        let folded = fold_point(limb.tip, &limb, Vec3::Y, 1.0);
        assert!(folded.y.abs() < 3.9, "the mast never came down: {folded:?}");
        assert!((folded.length() - 4.0).abs() < 1e-3, "and it kept its length");
    }

    #[test]
    fn a_degenerate_limb_is_not_a_panic() {
        let limb = Limb { prims: vec![], root: Vec3::ONE, tip: Vec3::ONE, reach: 0.0 };
        assert_eq!(fold_rotation(&limb, Vec3::Y, 1.0), Quat::IDENTITY);
        assert!((fold_point(Vec3::ONE, &limb, Vec3::Y, 1.0) - Vec3::ONE).length() < 1e-4);
    }

    #[test]
    fn vehicle_mode_packs_the_limbs_inside_the_hull() {
        let j = jet();
        let refs: Vec<&[Vec3]> = j.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let off = contain(&sk, &refs, 0.25);
        let env = sk.core_half * 1.25;
        for (i, o) in off.iter().enumerate() {
            if sk.core.contains(&i) {
                continue;
            }
            let (lo, hi) = bounds(&j[i].iter().map(|p| *p + *o).collect::<Vec<_>>());
            let out_hi = (hi - (sk.core_centre + env)).max_element();
            let out_lo 
                = ((sk.core_centre - env) - lo).max_element();
            // What containment can promise for an oversized limb is
            // CENTRING, not fitting: a 5.2-wide wing does not go inside a
            // 1.25-wide envelope by being moved, only by being rotated,
            // which `contain` does not do and does not claim to. So the
            // check is that the limb is centred — the overhang is even on
            // both sides and no worse than half its excess width — rather
            // than a fixed slack that no translation could ever reach.
            // PER-AXIS. The wing is wide in x while the envelope is
            // largest in z, so comparing the two maxima compares
            // different axes and reports no excess at all — my first
            // version did exactly that and computed 0.00 for a limb
            // overhanging by 1.96.
            let span = hi - lo;
            let excess = ((span - env * 2.0) * 0.5).max(Vec3::ZERO).max_element();
            let allow = excess + 0.35;
            assert!(
                out_hi < allow && out_lo < allow,
                "prim {i} pokes out by {:.2}/{:.2}, more than its {:.2} excess allows",
                out_hi,
                out_lo,
                excess
            );
            if excess > 0.35 {
                assert!(
                    (out_hi - out_lo).abs() < 0.35,
                    "prim {i} is not centred: {:.2} one side, {:.2} the other",
                    out_hi,
                    out_lo
                );
            }
        }
    }

    #[test]
    fn containment_leaves_a_tidy_body_alone() {
        // the important half: a vehicle whose parts are already inside its
        // own hull must not be nudged at all
        let car = vec![
            blob(Vec3::ZERO, Vec3::new(1.2, 0.7, 2.4), 600),
            blob(Vec3::new(0.5, 0.2, 0.4), Vec3::splat(0.25), 80),
        ];
        let refs: Vec<&[Vec3]> = car.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let off = contain(&sk, &refs, 0.25);
        for (i, o) in off.iter().enumerate() {
            assert!(o.length() < 1e-3, "prim {i} moved on a tidy body: {o:?}");
        }
    }

    #[test]
    fn the_oversized_wing_hinges_and_then_fits_where_translation_could_not() {
        // the exact case contain's doc names as impossible for it: a
        // wing wider than the whole envelope. The hinge swings it along
        // the body FIRST; containment then translates the rotated
        // silhouette. Judge the composition by the overhang it leaves.
        let j = jet();
        let refs: Vec<&[Vec3]> = j.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let env = sk.core_half * 1.25;
        let (elo, ehi) = (sk.core_centre - env, sk.core_centre + env);
        let overhang = |pts: &[Vec3]| {
            pts.iter()
                .map(|p| {
                    let over = (*p - ehi).max(Vec3::ZERO) + (elo - *p).max(Vec3::ZERO);
                    over.max_element()
                })
                .fold(0.0f32, f32::max)
        };
        let steps = fold_pack(&sk, &refs, 0.25, Vec3::Y);
        // translation-only, for comparison — the measured 1.98 per side
        let slide = contain(&sk, &refs, 0.25);
        let mut hinged = 0;
        let (mut worst_rotated, mut worst_slid) = (0.0f32, 0.0f32);
        for limb in &sk.limbs {
            let i0 = limb.prims[0];
            if steps[i0].rot == Quat::IDENTITY {
                continue;
            }
            hinged += 1;
            let s = steps[i0];
            for i in &limb.prims {
                let rot: Vec<Vec3> =
                    refs[*i].iter().map(|p| s.pivot + s.rot * (*p - s.pivot) + s.shift).collect();
                let sld: Vec<Vec3> = refs[*i].iter().map(|p| *p + slide[*i]).collect();
                worst_rotated = worst_rotated.max(overhang(&rot));
                worst_slid = worst_slid.max(overhang(&sld));
            }
        }
        assert!(hinged >= 2, "the wings did not hinge: {}", sk.report());
        // both directions pinned: the hinge must beat translation by a
        // wide margin AND land genuinely close to the envelope — a floor
        // alone would pass a hinge that stopped discriminating
        assert!(
            worst_rotated < worst_slid * 0.5,
            "hinging did not beat sliding: {worst_rotated} vs {worst_slid}"
        );
        assert!(
            worst_rotated < 1.0,
            "the hinged wing still hangs far outside the hull: {worst_rotated}"
        );
    }

    #[test]
    fn a_tidy_body_gets_identity_fold_steps() {
        // same fixture and same promise as containment's tidy-body test:
        // parts already inside their hull are not rotated OR nudged
        let car = vec![
            blob(Vec3::ZERO, Vec3::new(1.2, 0.7, 2.4), 600),
            blob(Vec3::new(0.5, 0.2, 0.4), Vec3::splat(0.25), 80),
        ];
        let refs: Vec<&[Vec3]> = car.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        for (i, s) in fold_pack(&sk, &refs, 0.25, Vec3::Y).iter().enumerate() {
            assert!(s.rot == Quat::IDENTITY, "prim {i} rotated on a tidy body");
            assert!(s.shift.length() < 1e-3, "prim {i} moved on a tidy body: {:?}", s.shift);
        }
    }

    #[test]
    fn a_short_appendage_is_left_where_it_is() {
        // wheels are appendages too; nobody asked for them inside the hull
        let car = vec![
            blob(Vec3::ZERO, Vec3::new(0.9, 0.5, 2.2), 500),
            blob(Vec3::new(-1.0, -0.4, -1.5), Vec3::splat(0.3), 60),
            blob(Vec3::new(1.0, -0.4, -1.5), Vec3::splat(0.3), 60),
        ];
        let refs: Vec<&[Vec3]> = car.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        let off = compact(&sk, refs.len(), 0.5);
        for (i, o) in off.iter().enumerate() {
            assert!(o.length() < 0.05, "prim {i} was dragged in: {o:?}");
        }
    }

    #[test]
    fn a_flat_plate_is_adopted_rather_than_left_floating() {
        // the wonky flat underside: too big for massfilter, too loose to
        // read as structure
        let body = vec![
            blob(Vec3::ZERO, Vec3::new(0.8, 0.8, 2.0), 500),
            blob(Vec3::new(0.0, -0.95, 0.0), Vec3::new(0.75, 0.02, 1.9), 120),
        ];
        let refs: Vec<&[Vec3]> = body.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        assert!(
            sk.joined > 0 || sk.core.contains(&1),
            "the flat plate is still a free-floating limb: {}",
            sk.report()
        );
    }

    #[test]
    fn the_join_reaches_a_fixpoint_and_does_not_spin() {
        // a chain of small pieces, each near the next: the recursion should
        // walk it inward and then stop
        let mut all = vec![blob(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0), 600)];
        for i in 1..7 {
            let x = 1.1 + i as f32 * 0.5;
            all.push(blob(Vec3::new(x, 0.0, 0.0), Vec3::splat(0.18), 20));
        }
        let refs: Vec<&[Vec3]> = all.iter().map(|v| v.as_slice()).collect();
        let sk = skeletonize(&refs);
        assert!(sk.joined > 0, "the chain was not walked in: {}", sk.report());
        assert!(sk.limbs.len() < 6, "every link stayed its own limb: {}", sk.report());
    }
}

