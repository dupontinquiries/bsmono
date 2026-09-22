//! What the parts ARE, derived from where they sit.
//!
//! Reading a car's parts off renders got left and right backwards on 19 of 72
//! of them -- systematically, because a three-quarter view puts -z on
//! whichever side the camera happens to be. Position is not ambiguous. The
//! nose is the end the bonnet is at and the left is -z; everything else
//! follows.

use crate::island::Island;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Wheel,
    Panel,
    Beam,
    Glass,
    Greeble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side { L, C, R }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End { Front, Mid, Rear }

#[derive(Debug, Clone, Copy)]
pub struct Part {
    pub kind: Kind,
    pub side: Side,
    pub end: End,
    /// How far the surface bows across its width, 0 flat.
    pub crown: f32,
    /// Narrowest width over widest along its length, 1 parallel-sided.
    pub taper: f32,
}

pub struct Body {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    /// Which axis is the vehicle's length, and whether the nose is +.
    pub long_axis: usize,
    pub nose_positive: bool,
    /// Which axis separates left from right.
    pub lat_axis: usize,
}

impl Body {
    pub fn size(&self) -> [f32; 3] {
        [self.hi[0] - self.lo[0], self.hi[1] - self.lo[1], self.hi[2] - self.lo[2]]
    }
}

/// Work out the vehicle's own axes before classifying anything on it.
///
/// Length is the longest extent and height is the one with the ground at its
/// bottom, so lateral is whatever is left. Which end is the nose is the one
/// question geometry alone cannot answer, so it is taken from where the mass
/// sits low and wide: a car's rear axle carries more structure than its nose.
pub fn frame(islands: &[Island]) -> Body {
    // ROBUST EXTENT: per-island trimmed boxes, then their union.
    //
    // Three versions, each failing a different way:
    //
    //   min/max           one stray vertex made a car 12,774 units tall.
    //   percentile of island CENTRES
    //                     cannot trim an outlier that lives inside an island,
    //                     and a two-island model has nothing to trim anyway.
    //   percentile of all VERTICES
    //                     weights every part by how finely it is tessellated.
    //                     A dense wheel beside a low-poly body swamps it and
    //                     the body is trimmed away as an outlier -- the
    //                     vehicle comes out the size of its wheels.
    //
    // Trimming INSIDE each island removes a stray vertex where it lives, and
    // taking the union of the trimmed boxes gives every part an equal say
    // whatever its triangle count.
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for i in islands {
        for a in 0..3 {
            let mut vs: Vec<f32> = i.verts.iter().map(|v| v[a]).collect();
            if vs.is_empty() { continue; }
            vs.sort_by(|x, y| x.partial_cmp(y).unwrap());
            // only trim when there is something to trim from: a box with
            // eight corners has no outliers, it has corners
            let k = if vs.len() >= 200 { vs.len() / 100 } else { 0 };
            lo[a] = lo[a].min(vs[k]);
            hi[a] = hi[a].max(vs[vs.len() - 1 - k]);
        }
    }
    for a in 0..3 {
        if !(lo[a].is_finite() && hi[a].is_finite()) { lo[a] = 0.0; hi[a] = 1.0; }
        if hi[a] - lo[a] < 1e-6 { hi[a] = lo[a] + 1.0; }
    }
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let mut order: Vec<usize> = (0..3).collect();
    order.sort_by(|&a, &b| size[b].partial_cmp(&size[a]).unwrap());
    let long_axis = order[0];
    // LATERAL is the axis of MIRROR SYMMETRY; up is whatever is left.
    //
    // Two rules failed first, and how they failed is the argument for this
    // one:
    //
    //   "the axis whose low end is nearest zero" -- a fact about how one
    //   exporter placed one car. A model centred on the origin has every
    //   axis's low end near minus half its size, so the rule picked at random
    //   and called a T-rex's vertical its lateral.
    //
    //   "the axis whose bottom is sparsest", because things touch the ground
    //   at small contacts -- but a lateral axis is sparse at BOTH edges, so
    //   it won as often as the real vertical did, and fixing the dinosaur
    //   broke a car.
    //
    // What is true of every vehicle and every animal: they are bilaterally
    // symmetric. Left mirrors right. Top does not mirror bottom -- wheels are
    // not a roof, feet are not a spine. So compare each candidate axis's
    // distribution to its own reflection, and the one that matches best is
    // left-right.
    let lat_axis = {
        let cand: Vec<usize> = (0..3).filter(|&a| a != long_axis).collect();
        let mut best = cand[0];
        let mut best_err = f32::MAX;
        for &a in &cand {
            let span = size[a].max(1e-6);
            let mid = (lo[a] + hi[a]) * 0.5;
            const N: usize = 16;
            let mut bins = [0f32; N];
            let mut tot = 0f32;
            for i in islands {
                for v in &i.verts {
                    let k = (((v[a] - lo[a]) / span) * N as f32) as isize;
                    if (0..N as isize).contains(&k) { bins[k as usize] += 1.0; tot += 1.0; }
                }
            }
            if tot == 0.0 { continue; }
            // difference between the histogram and its mirror image
            let err: f32 = (0..N).map(|k| (bins[k] - bins[N - 1 - k]).abs()).sum::<f32>() / tot;
            let _ = mid;
            if err < best_err { best_err = err; best = a; }
        }
        best
    };
    let up = (0..3).find(|&a| a != long_axis && a != lat_axis).unwrap_or(1);
    let _ = up;
    // the heavier half along the length is the rear
    let mid = (lo[long_axis] + hi[long_axis]) * 0.5;
    let (mut a_side, mut b_side) = (0usize, 0usize);
    for i in islands {
        if i.centre()[long_axis] < mid { a_side += i.verts.len() } else { b_side += i.verts.len() }
    }
    Body { lo, hi, long_axis, nose_positive: a_side > b_side, lat_axis }
}

/// Bow and taper, the two numbers that let armour inherit a panel's section.
pub fn profile(is: &Island) -> (f32, f32) {
    let s = is.size();
    let mut ord: Vec<usize> = (0..3).collect();
    ord.sort_by(|&a, &b| s[a].partial_cmp(&s[b]).unwrap());
    let (thin, mid, long_) = (ord[0], ord[1], ord[2]);
    if s[mid] < 1e-6 || s[long_] < 1e-6 { return (0.15, 0.7); }

    let mut bins = vec![(0.0f32, 0u32); 12];
    for v in &is.verts {
        let k = (((v[mid] - is.lo[mid]) / s[mid]) * 11.0).round() as usize;
        let k = k.min(11);
        bins[k].0 += v[thin];
        bins[k].1 += 1;
    }
    let prof: Vec<f32> = bins.iter().filter(|b| b.1 > 0)
        .map(|b| b.0 / b.1 as f32).collect();
    let crown = if prof.len() > 2 {
        let mx = prof.iter().cloned().fold(f32::MIN, f32::max);
        let mn = prof.iter().cloned().fold(f32::MAX, f32::min);
        (mx - mn) / s[mid]
    } else { 0.15 };

    let mut wb: Vec<(f32, f32, u32)> = vec![(f32::MAX, f32::MIN, 0); 10];
    for v in &is.verts {
        let k = (((v[long_] - is.lo[long_]) / s[long_]) * 9.0).round() as usize;
        let k = k.min(9);
        wb[k].0 = wb[k].0.min(v[mid]);
        wb[k].1 = wb[k].1.max(v[mid]);
        wb[k].2 += 1;
    }
    let ws: Vec<f32> = wb.iter().filter(|b| b.2 > 2).map(|b| b.1 - b.0).collect();
    let taper = if ws.len() > 1 {
        let mx = ws.iter().cloned().fold(f32::MIN, f32::max).max(1e-6);
        ws.iter().cloned().fold(f32::MAX, f32::min) / mx
    } else { 0.7 };
    (crown.clamp(0.02, 0.6), taper.clamp(0.2, 0.98))
}

pub fn classify(is: &Island, b: &Body) -> Part {
    let s = is.sorted_size();
    let c = is.centre();
    let sz = b.size();
    let (crown, taper) = profile(is);

    let lat = c[b.lat_axis];
    let mid_lat = (b.lo[b.lat_axis] + b.hi[b.lat_axis]) * 0.5;
    let half = sz[b.lat_axis] * 0.06;
    let side = if lat < mid_lat - half { Side::L }
        else if lat > mid_lat + half { Side::R } else { Side::C };

    let t = (c[b.long_axis] - b.lo[b.long_axis]) / sz[b.long_axis].max(1e-6);
    let t = if b.nose_positive { t } else { 1.0 - t };
    let end = if t > 0.62 { End::Front } else if t < 0.38 { End::Rear } else { End::Mid };

    // A wheel is round in two axes, thinner in the third, big -- AND SIZED
    // LIKE A WHEEL. Roundness alone put a 169-unit cabin panel in a cluster
    // of its own on a car whose tyres are 60 across, giving five corners on
    // four wheels. A wheel is between 8% and 22% of the vehicle's length;
    // anything outside that is something else that happens to be round.
    let vlen = sz[b.long_axis].max(1e-6);
    let wheel_sized = s[0] > vlen * 0.08 && s[0] < vlen * 0.22;
    let roundish = s[0] > 1e-6 && (s[0] - s[1]).abs() / s[0] < 0.14
        && s[2] < s[0] * 0.75 && wheel_sized;
    let kind = if roundish && is.verts.len() > 300 {
        Kind::Wheel
    } else if is.verts.len() < 370 {
        Kind::Greeble
    } else if s[2] < s[0] * 0.05 {
        Kind::Glass          // a shell that thin is almost always a window
    } else if s[1] < s[0] * 0.30 {
        Kind::Beam
    } else {
        Kind::Panel
    };
    Part { kind, side, end, crown, taper }
}

/// A wheel is an assembly, not an island.
///
/// A modelled wheel is a tyre, a rim, a brake disc, a hub and a dozen bolts --
/// every one of them round and thin, so every one classifies as a wheel on
/// its own. Counting islands gave 25 wheels on a four-wheeled car, and worse,
/// each fragment was placed on the skeleton separately.
///
/// Corners are found by clustering: anything whose centre is within a tyre's
/// own radius of another wheel part belongs to the same corner.
pub fn corners(islands: &[Island], parts: &[Part], body: &Body) -> Vec<Vec<usize>> {
    let idx: Vec<usize> = (0..islands.len())
        .filter(|&i| parts[i].kind == Kind::Wheel).collect();
    if idx.is_empty() { return vec![]; }
    // Reach comes from the VEHICLE, not from whatever survived classification.
    //
    // Taking it from the largest recognised wheel part is circular: tightening
    // what counts as a wheel shrank the reach, which split corners that had
    // been together, and a car went from five corners to eight while getting
    // strictly more correct about what a wheel is. A corner is about a
    // tyre-radius across whatever the classifier decided, and a tyre is a
    // known fraction of the vehicle.
    let vlen = body.size()[body.long_axis].max(1e-6);
    let reach = (vlen * 0.11).max(
        idx.iter().map(|&i| islands[i].sorted_size()[0] * 0.55)
           .fold(0.0f32, f32::max));

    let mut owner: Vec<usize> = (0..idx.len()).collect();
    fn find(o: &mut Vec<usize>, mut a: usize) -> usize {
        while o[a] != a { o[a] = o[o[a]]; a = o[a]; }
        a
    }
    for a in 0..idx.len() {
        for b in (a + 1)..idx.len() {
            let (ca, cb) = (islands[idx[a]].centre(), islands[idx[b]].centre());
            let d = ((ca[0] - cb[0]).powi(2) + (ca[1] - cb[1]).powi(2)
                     + (ca[2] - cb[2]).powi(2)).sqrt();
            if d <= reach {
                let (ra, rb) = (find(&mut owner, a), find(&mut owner, b));
                if ra != rb { owner[ra] = rb; }
            }
        }
    }
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    for a in 0..idx.len() {
        let r = find(&mut owner, a);
        groups.entry(r).or_default().push(idx[a]);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    // A WHEEL IS AT A CORNER. A cluster sitting on the centreline is a fan, a
    // spare, a steering wheel or a gauge -- round, wheel-sized, and not a
    // wheel. Requiring real lateral offset is the cheapest way to say so, and
    // it is true of every road vehicle.
    let half = body.size()[body.lat_axis] * 0.5;
    let mid = (body.lo[body.lat_axis] + body.hi[body.lat_axis]) * 0.5;
    out.retain(|c| {
        let lat: f32 = c.iter().map(|&i| islands[i].centre()[body.lat_axis]).sum::<f32>()
            / c.len() as f32;
        (lat - mid).abs() > half * 0.35
    });
    // deterministic: front-to-back then left-to-right
    out.sort_by(|a, b| {
        let ca = islands[a[0]].centre();
        let cb = islands[b[0]].centre();
        ca[0].partial_cmp(&cb[0]).unwrap()
            .then(ca[2].partial_cmp(&cb[2]).unwrap())
    });
    out
}
