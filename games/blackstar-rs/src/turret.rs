//! TURRETS AND TREADS, found in the geometry.
//!
//! A tank whose gun points wherever the hull points is a hull with a
//! decoration on it. The thing that makes armour read as armour is that
//! the turret tracks where you are LOOKING while the hull goes where you
//! are driving — two independent orientations on one body, which no
//! chassis in this game has ever had.
//!
//! Nothing in the pipeline could express it, either: `model::Tag` knows
//! `Body`, `WheelFront`, `WheelBack`, and `split_wheels` finds round
//! things at the corners. A tracked vehicle has no round things at the
//! corners, so it gets no wheels, nothing spins, and the whole hull is one
//! rigid `Body` — which is why the treads are still and the turret is
//! welded.
//!
//! This module measures both from the prims the loader already built:
//!
//! * **A TURRET** is a compact cluster sitting ABOVE the hull's shoulder
//!   line and near the middle in plan — a thing on top, not a thing at the
//!   end. Its pivot is that cluster's own centroid projected onto the hull
//!   deck, because a turret rotates about its ring, not about its mass.
//! * **A TREAD** is a long, low, flat slab running most of the body's
//!   length, one per side. Two of them mirrored is a tracked vehicle. The
//!   flag matters beyond looks: `locomotion` counts treads as running
//!   gear, so a tank is Wheeled (grip, grounded) rather than falling
//!   through the skirt test into Hover.
//!
//! WHAT IS WIRED AND WHAT IS NOT. Detection is here and the turret's yaw
//! is applied in the draw path — that half is real. Tread SCROLLING is
//! not: the meshes are static and rolling them needs either a UV scroll in
//! the shader or per-link geometry, neither of which is a measurement
//! problem. The tread indices are exposed so that work has somewhere to
//! start, and calling it detected-but-not-animated is the honest label.
//!
//! Thresholds here are reasoned from shapes, like `partdetect`'s and
//! `locomotion`'s, and inherit the same caveat: no labelled set exists in
//! this repo to fit them against.

use glam::Vec3;

/// What was found on a hull.
#[derive(Clone, Debug, Default)]
pub struct Fittings {
    /// prim indices that make up the turret, and the ring it turns on
    pub turret: Vec<usize>,
    /// the pivot, in model space — on the deck, under the turret's mass
    pub ring: Vec3,
    /// prim indices that read as treads (detected, not yet animated)
    pub treads: Vec<usize>,
    /// round low outboard prims with their spin pivots, for hulls whose
    /// wheels `model::split_wheels` never found. Imported bots mostly fall
    /// in that gap: their wheels are not at tidy corners with a shared
    /// radius, so nothing was tagged and nothing turned.
    pub rollers: Vec<(usize, Vec3)>,
    /// the turret prim read as the barrel/mantlet — long and thin
    /// relative to the rest of the turret assembly. `None` when the
    /// turret has no single prim shaped like a gun (a rounded cupola
    /// with no protruding barrel, for instance).
    pub barrel: Option<usize>,
    /// where the barrel meets the turret body — the mantlet axis point,
    /// taken as the barrel's own endpoint nearest the ring rather than
    /// its centroid, so elevating about it pitches the muzzle and not
    /// the breech through the turret roof.
    pub elevation_pivot: Vec3,
    /// the horizontal axis to pitch the barrel about: perpendicular to
    /// both world-up and the barrel's own root-to-tip direction, so a
    /// positive rotation about it raises the muzzle regardless of which
    /// way the turret is currently yawed.
    pub elevation_axis: Vec3,
}

impl Fittings {
    pub fn has_turret(&self) -> bool {
        !self.turret.is_empty()
    }
    pub fn tracked(&self) -> bool {
        self.treads.len() >= 2
    }
    /// A turret that can pitch its gun, not just yaw the whole assembly.
    pub fn has_elevation(&self) -> bool {
        self.barrel.is_some()
    }
    pub fn report(&self) -> String {
        format!(
            "fittings: {} turret prims (ring {:.2},{:.2},{:.2}), {} tread prims{}{}",
            self.turret.len(),
            self.ring.x,
            self.ring.y,
            self.ring.z,
            self.treads.len(),
            if self.tracked() { " — tracked" } else { "" },
            if self.has_elevation() { " — elevating" } else { "" }
        )
    }
}

/// Above this fraction of the body's height, measured from the middle, a
/// cluster is "on top of" the hull rather than part of it.
const SHOULDER: f32 = 0.15;
/// A turret sits near the middle in plan: within this fraction of the
/// half-length of centre. Beyond it, a raised cluster is a cab or a
/// tailfin.
const CENTRED: f32 = 0.55;
/// A turret is compact — no longer than this share of the hull.
const COMPACT: f32 = 0.62;
/// A tread runs at least this share of the body's length...
const TREAD_LONG: f32 = 0.55;
/// ...sits entirely below this height...
const TREAD_LOW: f32 = 0.10;
/// ...and is at least this far off the centreline.
const TREAD_OUT: f32 = 0.25;
/// A barrel is long relative to its OWN other two axes, not the hull's —
/// this compares a candidate prim's longest extent to its second-longest.
/// A turret roof or cupola is roughly as wide as it is deep and fails
/// this outright; a gun tube is several times longer than it is thick.
const BARREL_ASPECT: f32 = 1.8;

/// Measure a hull. `prims` is each prim's points in MODEL space, already
/// oriented (Y up, -Z forward) — the frame everything downstream assumes.
pub fn find(prims: &[&[Vec3]]) -> Fittings {
    let mut out = Fittings::default();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in prims.iter().flat_map(|p| p.iter()) {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    if !(hi.x > lo.x) {
        return out;
    }
    let half = (hi - lo) * 0.5;
    let mid = (lo + hi) * 0.5;
    if half.y < 1e-4 || half.z < 1e-4 {
        return out;
    }

    let mut turret_pts = 0.0f32;
    let mut turret_sum = Vec3::ZERO;
    for (i, p) in prims.iter().enumerate() {
        if p.len() < 8 {
            continue;
        }
        let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut sum = Vec3::ZERO;
        for v in p.iter() {
            plo = plo.min(*v);
            phi = phi.max(*v);
            sum += *v;
        }
        let c = sum / p.len() as f32;
        let ph = (phi - plo) * 0.5;

        // TURRET: centre of mass above the shoulder, near the middle in
        // plan, and compact for the hull it sits on.
        let above = (c.y - mid.y) / half.y > SHOULDER;
        let centred = ((c.z - mid.z).abs() / half.z) < CENTRED;
        let compact = ph.z / half.z < COMPACT && ph.x / half.x < COMPACT;
        if above && centred && compact {
            out.turret.push(i);
            turret_sum += c * p.len() as f32;
            turret_pts += p.len() as f32;
            continue;
        }

        // TREAD: long, low, outboard, and flat.
        let long = ph.z / half.z > TREAD_LONG;
        let low = (phi.y - mid.y) / half.y < TREAD_LOW;
        let outboard = (c.x - mid.x).abs() / half.x > TREAD_OUT;
        let flat = ph.y < ph.x * 2.2;
        if long && low && outboard && flat {
            out.treads.push(i);
        }
    }

    // GROW THE TURRET INTO ITS OWN ASSEMBLY.
    //
    // The seed test finds prims above the shoulder line, and on a real
    // tank that is often just the BARREL — long, thin, high — while the
    // turret body it swings from sits lower and fails "above the
    // shoulder". Rotating the seed alone is exactly the reported symptom:
    // the barrel moves and the turret does not follow. So the seed is
    // grown outward to anything overlapping its box, once, which picks up
    // the mantlet, the basket and the cupola without walking down the
    // hull.
    if !out.turret.is_empty() {
        let (mut tlo, mut thi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in &out.turret {
            for v in prims[*i].iter() {
                tlo = tlo.min(*v);
                thi = thi.max(*v);
            }
        }
        // a little slack, so a part touching the ring counts as attached
        let grow = ((thi - tlo) * 0.22).max(Vec3::splat(1e-3));
        let (glo, ghi) = (tlo - grow, thi + grow);
        for (i, p) in prims.iter().enumerate() {
            if out.turret.contains(&i) || out.treads.contains(&i) || p.len() < 6 {
                continue;
            }
            let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in p.iter() {
                plo = plo.min(*v);
                phi = phi.max(*v);
            }
            // must OVERLAP the turret box and sit at or above its base:
            // a part hanging below the ring is hull, not turret
            let overlaps = plo.x <= ghi.x && phi.x >= glo.x && plo.z <= ghi.z && phi.z >= glo.z;
            // MOSTLY above the ring, not merely poking over it. `phi.y >
            // tlo.y` passes on a sliver: the test tank's hull roof clears
            // the turret base by 0.05 and was adopted on that alone. A
            // part belongs to the turret when the bulk of it rides above
            // the ring, so compare its CENTRE.
            let high_enough = (plo.y + phi.y) * 0.5 > tlo.y;
            // and it must not be most of the body: that is the hull.
            //
            // Compare against the turret's WIDTH, not its longest axis. A
            // barrel is long by design and stretches the seed box down z,
            // which inflates this threshold until the hull slips under
            // it — measured on the test tank: the barrel makes the seed
            // 3.40 long, so the limit becomes 6.80 and a 4.80 hull reads
            // as "not big". The turret's x/z footprint is what a part has
            // to be comparable to; length is the barrel's business.
            let seed_span = thi - tlo;
            let seed_w = seed_span.x.max(seed_span.z).max(1e-3);
            let big = (phi - plo).max_element() > seed_w * 2.0;
            if overlaps && high_enough && !big {
                out.turret.push(i);
            }
        }
    }

    // ROLLERS: round, low, outboard prims — the wheels `split_wheels` did
    // not tag because they are not at tidy corners with a matched radius.
    // Detected here so the draw path has something to spin; the pivot is
    // the prim's own centre, which is where a wheel turns.
    for (i, p) in prims.iter().enumerate() {
        if out.turret.contains(&i) || out.treads.contains(&i) || p.len() < 12 {
            continue;
        }
        let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut sum = Vec3::ZERO;
        for v in p.iter() {
            plo = plo.min(*v);
            phi = phi.max(*v);
            sum += *v;
        }
        let c = sum / p.len() as f32;
        let e = (phi - plo) * 0.5;
        // round in the side view: height and length within a quarter of
        // each other, and narrow across the hull
        let round = (e.y - e.z).abs() < e.y.max(e.z) * 0.28;
        let narrow = e.x < e.y.max(e.z) * 0.9;
        let low = (c.y - mid.y) / half.y < -0.1;
        let outboard = (c.x - mid.x).abs() / half.x > 0.25;
        let small = e.max_element() < half.max_element() * 0.5;
        if round && narrow && low && outboard && small {
            out.rollers.push((i, c));
        }
    }

    // A turret is a thing, not a scattering: a "turret" made of a dozen
    // separate small prims is more likely to be roof detail, and rotating
    // roof detail away from the hull it belongs to looks far worse than
    // leaving a real turret still.
    if out.turret.len() > 6 || turret_pts <= 0.0 {
        out.turret.clear();
        turret_pts = 0.0;
    }
    if turret_pts > 0.0 {
        let c = turret_sum / turret_pts;
        // the RING, not the mass: a turret turns about its base, so drop
        // the pivot to the deck under it. Rotating about the centroid
        // makes a long gun swing its breech through the hull.
        out.ring = Vec3::new(c.x, lo.y + half.y * 0.9, c.z);
    }

    // THE BARREL: whichever turret prim is shaped like a gun rather than
    // a body — long and thin relative to its own other two axes, not
    // relative to the hull. `COMPACT` already keeps genuinely wide things
    // (the turret roof, a cupola) out of contention on its own, so this
    // only has to ask each candidate's OWN aspect ratio, the same way
    // `orphanish` in `skeleton.rs` asks a group's shape rather than the
    // body's. The most elongated qualifying prim wins; a turret with
    // nothing shaped like a barrel (a rounded MG cupola, say) correctly
    // reports `barrel: None` rather than guessing.
    if !out.turret.is_empty() {
        let mut best: Option<(usize, f32)> = None;
        for &i in &out.turret {
            let p = prims[i];
            if p.len() < 6 {
                continue;
            }
            let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in p.iter() {
                plo = plo.min(*v);
                phi = phi.max(*v);
            }
            let e = phi - plo;
            let longest = e.max_element().max(1e-5);
            let second = {
                let mut vals = [e.x, e.y, e.z];
                vals.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                vals[1].max(1e-5)
            };
            let aspect = longest / second;
            if aspect < BARREL_ASPECT {
                continue;
            }
            if best.map(|(_, a)| aspect > a).unwrap_or(true) {
                best = Some((i, aspect));
            }
        }
        if let Some((i, _)) = best {
            let p = prims[i];
            // ROOT is the barrel's own point nearest the ring — where it
            // meets the mantlet — not its centroid, which sits halfway
            // down the tube. TIP is the point farthest from that root,
            // the muzzle end. Same root/tip shape as `skeleton::Limb`,
            // for the same reason: a pivot at the joint, not the middle.
            let root = p.iter().copied().min_by(|a, b| {
                (*a - out.ring).length_squared().partial_cmp(&(*b - out.ring).length_squared()).unwrap_or(std::cmp::Ordering::Equal)
            }).unwrap_or(out.ring);
            let tip = p.iter().copied().max_by(|a, b| {
                (*a - root).length_squared().partial_cmp(&(*b - root).length_squared()).unwrap_or(std::cmp::Ordering::Equal)
            }).unwrap_or(root);
            let aim = tip - root;
            // the axis to pitch about: horizontal, perpendicular to the
            // barrel's own current heading, so raising the muzzle stays
            // correct no matter which way the turret is yawed. Falls
            // back to X for the (degenerate) case of a dead-vertical
            // barrel, where up x aim is zero.
            let axis = Vec3::Y.cross(aim);
            out.elevation_axis = if axis.length_squared() > 1e-8 { axis.normalize() } else { Vec3::X };
            out.elevation_pivot = root;
            out.barrel = Some(i);
        }
    }

    // treads come in pairs; one is a running board
    if out.treads.len() == 1 {
        out.treads.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slab(c: Vec3, h: Vec3, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let t = i as f32 * 0.7;
                c + Vec3::new(t.sin() * h.x, (t * 1.7).cos() * h.y, (t * 0.3).sin() * h.z)
            })
            .collect()
    }

    /// A tank: wide low hull, two side treads, a compact turret on top.
    fn tank() -> Vec<Vec<Vec3>> {
        vec![
            slab(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.5, 2.4), 400),
            slab(Vec3::new(-1.1, -0.45, 0.0), Vec3::new(0.18, 0.22, 2.2), 200),
            slab(Vec3::new(1.1, -0.45, 0.0), Vec3::new(0.18, 0.22, 2.2), 200),
            slab(Vec3::new(0.0, 0.75, -0.2), Vec3::new(0.6, 0.3, 0.8), 260),
        ]
    }

    #[test]
    fn a_tank_has_a_turret_and_two_treads() {
        let t = tank();
        let refs: Vec<&[Vec3]> = t.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        assert!(f.has_turret(), "no turret: {}", f.report());
        assert!(f.turret.contains(&3), "the turret is not the thing on top: {}", f.report());
        assert!(f.tracked(), "not tracked: {}", f.report());
        assert!(f.treads.contains(&1) && f.treads.contains(&2), "{}", f.report());
    }

    #[test]
    fn the_ring_sits_under_the_turret_not_in_it() {
        let t = tank();
        let refs: Vec<&[Vec3]> = t.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        // a long gun rotating about its own centroid swings the breech
        // through the hull; the ring has to be down on the deck
        assert!(f.ring.y < 0.75, "the pivot is up in the turret's mass: {:?}", f.ring);
        assert!((f.ring.x - 0.0).abs() < 0.5, "the ring drifted off the hull: {:?}", f.ring);
    }

    #[test]
    fn the_whole_turret_follows_the_barrel() {
        // the reported symptom: only the barrel turned. The barrel is what
        // clears the shoulder test; the turret body it swings from sits
        // lower and has to be grown into.
        let mut t = tank();
        // a low, wide turret body under a high thin barrel
        t.push(slab(Vec3::new(0.0, 0.55, -0.2), Vec3::new(0.8, 0.18, 0.9), 300));
        t.push(slab(Vec3::new(0.0, 0.8, 1.1), Vec3::new(0.07, 0.07, 1.3), 120));
        let refs: Vec<&[Vec3]> = t.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        assert!(f.turret.contains(&5), "the barrel is not in the turret: {}", f.report());
        assert!(
            f.turret.contains(&4),
            "the barrel turns but the turret body does not follow: {}",
            f.report()
        );
        assert!(!f.turret.contains(&0), "the hull was swept into the turret: {}", f.report());
    }

    #[test]
    fn the_barrel_is_found_among_the_turret_and_gets_a_horizontal_elevation_axis() {
        let mut t = tank();
        // same shape as `the_whole_turret_follows_the_barrel`: a wide low
        // turret body plus a long thin barrel protruding forward
        t.push(slab(Vec3::new(0.0, 0.55, -0.2), Vec3::new(0.8, 0.18, 0.9), 300));
        t.push(slab(Vec3::new(0.0, 0.8, 1.1), Vec3::new(0.07, 0.07, 1.3), 120));
        let refs: Vec<&[Vec3]> = t.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        assert!(f.has_elevation(), "no barrel found on a turret that has one: {}", f.report());
        assert_eq!(f.barrel, Some(5), "wrong prim called the barrel: {}", f.report());
        // pitching about a vertical or forward-pointing axis would swing
        // the muzzle sideways, not up; the axis has to be mostly
        // horizontal and mostly across the hull (X-ish), not along it
        assert!(f.elevation_axis.y.abs() < 0.3, "the elevation axis is not horizontal: {:?}", f.elevation_axis);
        assert!(f.elevation_axis.x.abs() > 0.7, "the elevation axis does not run across the hull: {:?}", f.elevation_axis);
        // the pivot is where the barrel meets the mantlet (near the
        // turret body), not out at the muzzle or buried at the ring
        assert!(f.elevation_pivot.z < 1.1, "the pivot drifted out to the muzzle: {:?}", f.elevation_pivot);
    }

    #[test]
    fn a_turret_with_no_barrel_shaped_prim_reports_no_elevation() {
        // a rounded cupola with no protruding gun tube: nothing here is
        // shaped like a barrel, so this should not guess one
        let t = tank();
        let refs: Vec<&[Vec3]> = t.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        assert!(f.has_turret(), "{}", f.report());
        assert!(!f.has_elevation(), "a barrel was found where there is none: {}", f.report());
        assert_eq!(f.barrel, None);
    }

    #[test]
    fn wheels_that_split_wheels_would_miss_are_still_found() {
        // round, low, outboard, narrow: spinnable even when they are not
        // at tidy corners with a matched radius
        let mut all = vec![slab(Vec3::ZERO, Vec3::new(0.9, 0.5, 2.2), 500)];
        for (x, z) in [(-1.0f32, -1.4f32), (1.0, -1.4), (-1.0, 1.5), (1.0, 1.5)] {
            all.push(slab(Vec3::new(x, -0.42, z), Vec3::new(0.09, 0.32, 0.32), 90));
        }
        let refs: Vec<&[Vec3]> = all.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        assert_eq!(f.rollers.len(), 4, "found {} rollers: {}", f.rollers.len(), f.report());
        for (i, pivot) in &f.rollers {
            assert!(*i > 0, "the hull was called a wheel");
            assert!(pivot.y < 0.0, "the spin pivot is not at the wheel: {pivot:?}");
        }
    }

    #[test]
    fn a_plain_car_has_neither() {
        let body = slab(Vec3::ZERO, Vec3::new(0.9, 0.45, 2.2), 500);
        let wheels: Vec<Vec<Vec3>> = [(-1.0f32, -1.6f32), (1.0, -1.6), (-1.0, 1.6), (1.0, 1.6)]
            .iter()
            .map(|(x, z)| slab(Vec3::new(*x, -0.42, *z), Vec3::splat(0.3), 60))
            .collect();
        let mut all = vec![body];
        all.extend(wheels);
        let refs: Vec<&[Vec3]> = all.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        assert!(!f.has_turret(), "a car grew a turret: {}", f.report());
        assert!(!f.tracked(), "wheels were called treads: {}", f.report());
    }

    #[test]
    fn roof_clutter_is_not_a_turret() {
        // a dozen small raised prims is detail, and swinging detail away
        // from the hull looks worse than leaving a real turret still
        let mut all = vec![slab(Vec3::ZERO, Vec3::new(0.9, 0.45, 2.2), 500)];
        for i in 0..10 {
            let z = -1.5 + i as f32 * 0.3;
            all.push(slab(Vec3::new(0.0, 0.6, z), Vec3::splat(0.08), 20));
        }
        let refs: Vec<&[Vec3]> = all.iter().map(|v| v.as_slice()).collect();
        let f = find(&refs);
        assert!(!f.has_turret(), "roof clutter was called a turret: {}", f.report());
    }
}
