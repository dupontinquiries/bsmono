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

use glam::Vec3;

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
    for (i, c) in cents.iter().enumerate() {
        if prims[i].is_empty() {
            continue;
        }
        let d = ((*c - mid) / half.max(Vec3::splat(1e-4))).length();
        if d < CORE_R {
            sk.core.push(i);
        }
    }
    // a body with no middle is one piece: call the biggest prim the core
    // so the rest still has something to attach to
    if sk.core.is_empty() {
        if let Some((i, _)) = prims.iter().enumerate().max_by_key(|(_, p)| p.len()) {
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
        let centre_shift = sk.core_centre - (lo + hi) * 0.5;
        let cap = centre_shift.abs();
        shift = shift.clamp(-cap, cap);
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

#[cfg(test)]
mod tests {
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
    fn compaction_halves_the_overhang_without_detaching_it() {
        let j = jet();
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
