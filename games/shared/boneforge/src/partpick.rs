//! Decide a new vehicle's parts by the NEAREST KNOWN PART.
//!
//! The generalisation experiment (partnet/) trained networks on orion, the
//! Falcon GT and the flyer and scored them on a vehicle they had not seen. The
//! winner was not a network: copying the decision of the most similar known
//! part -- by geometry alone, never by name -- placed parts best (96 units off
//! in robot form, against 131 for the average and 142 for the structured net).
//! A network should overtake it once the library holds enough vehicles; until
//! then this is the decider, and every decision says which part it came from.

/// A part's geometry: its box centre and half-extents, triangle count, and
/// its vehicle's bounds.
#[derive(Debug, Clone)]
pub struct Geom { pub c: [f32; 3], pub r: [f32; 3], pub nt: u32, pub lo: [f32; 3], pub hi: [f32; 3] }

#[derive(Debug, Clone)]
pub struct Decision {
    pub keep: bool,
    /// canonical bone ("uparm"), made sided ("uparm_l") by [`pick`]
    pub bone: String,
    pub seat: [f32; 3],
    pub scale: f32,
    /// 0 none, 1 spike, 2 trim
    pub morph: u8,
    pub dissolve: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct Known { pub vehicle: String, pub name: String, pub geom: Geom, pub decision: Decision }

/// -1 left, +1 right, 0 on the centreline (within 8% of the half-width).
pub fn side_of(g: &Geom) -> f32 {
    let half_w = (g.hi[2] - g.lo[2]) / 2.0;
    if g.c[2].abs() < 0.08 * half_w { 0.0 } else { g.c[2].signum() }
}

/// Geometry only, and mirror-folded so left and right parts share what they
/// teach. NOT the vehicle's own proportions: constant per vehicle, they acted
/// as a vehicle ID and wrecked generalisation in the experiment.
pub fn features(g: &Geom) -> [f32; 13] {
    let mut f = [0.0f32; 13];
    let ctr = [(g.lo[0] + g.hi[0]) / 2.0, (g.lo[1] + g.hi[1]) / 2.0, (g.lo[2] + g.hi[2]) / 2.0];
    let half = [(g.hi[0] - g.lo[0]) / 2.0, (g.hi[1] - g.lo[1]) / 2.0, (g.hi[2] - g.lo[2]) / 2.0];
    for k in 0..3 { f[k] = (g.c[k] - ctr[k]) / half[k]; }
    f[2] = f[2].abs();
    for k in 0..3 { f[3 + k] = g.r[k] / half[0]; }
    let mut ext = [g.r[0] / half[0], g.r[1] / half[0], g.r[2] / half[0]];
    ext.sort_by(|a, b| b.partial_cmp(a).unwrap());
    f[6] = ext[0]; f[7] = ext[1]; f[8] = ext[2];
    f[9] = ext[2] / ext[0].max(1e-6); f[10] = ext[1] / ext[0].max(1e-6);
    f[11] = (1.0 + g.nt as f32).ln() / 10.0;
    f[12] = side_of(g).abs();
    f
}

/// Parse the library (assets/partlib.csv): one known part per line.
pub fn library(csv: &str) -> Vec<Known> {
    csv.lines().skip(1).filter(|l| !l.trim().is_empty()).map(|l| {
        let c: Vec<&str> = l.split(',').collect();
        let n = |i: usize| c[i].parse::<f32>().unwrap();
        Known {
            vehicle: c[0].into(), name: c[1].into(),
            geom: Geom { c: [n(2), n(3), n(4)], r: [n(5), n(6), n(7)], nt: n(8) as u32,
                         lo: [n(9), n(10), n(11)], hi: [n(12), n(13), n(14)] },
            decision: Decision { keep: n(15) > 0.5, bone: c[16].into(), seat: [n(17), n(18), n(19)],
                                 scale: n(20), morph: n(21) as u8, dissolve: [n(22), n(23)] },
        }
    }).collect()
}

const SIDED: [&str; 8] = ["clav", "uparm", "forearm", "hand", "hip", "thigh", "shin", "foot"];

/// The nearest known part's decision, moved onto this part's side. Features
/// are standardised over the library and clipped to +-3 sd, so a vehicle of
/// a new shape cannot push the distance into extrapolation.
pub fn pick<'a>(lib: &'a [Known], g: &Geom) -> (Decision, &'a Known) {
    let fs: Vec<[f32; 13]> = lib.iter().map(|k| features(&k.geom)).collect();
    let n = fs.len() as f32;
    let mut mu = [0.0f32; 13]; let mut sd = [0.0f32; 13];
    for f in &fs { for i in 0..13 { mu[i] += f[i] / n; } }
    for f in &fs { for i in 0..13 { sd[i] += (f[i] - mu[i]).powi(2) / n; } }
    for s in sd.iter_mut() { *s = s.sqrt() + 1e-6; }
    let z = |f: &[f32; 13]| -> [f32; 13] { let mut o = [0.0; 13]; for i in 0..13 { o[i] = ((f[i] - mu[i]) / sd[i]).clamp(-3.0, 3.0); } o };
    let q = z(&features(g));
    let (best, _) = fs.iter().enumerate()
        .map(|(i, f)| { let d = z(f); (i, (0..13).map(|k| (d[k] - q[k]).powi(2)).sum::<f32>()) })
        .fold((0, f32::MAX), |a, b| if b.1 < a.1 { b } else { a });
    let src = &lib[best];
    let mut d = src.decision.clone();
    let (side, src_side) = (side_of(g), side_of(&src.geom));
    if src_side * if side == 0.0 { 1.0 } else { side } < 0.0 { d.seat[2] = -d.seat[2]; }
    if SIDED.contains(&d.bone.as_str()) { d.bone = format!("{}{}", d.bone, if side < 0.0 { "_l" } else { "_r" }); }
    (d, src)
}

/// Decide a whole vehicle: SEATS ARE EXCLUSIVE. Deciding parts one at a
/// time let four side panels all copy the same thigh-trim source, stacking
/// two trims on one seat. The part with the closest match claims its source
/// first; a later part on the same side falls to its next-nearest free one.
/// Processed in order of match distance, so naming order does not matter.
pub fn pick_all<'a>(lib: &'a [Known], parts: &[(String, Geom)]) -> Vec<(String, Decision, &'a Known)> {
    let fs: Vec<[f32; 13]> = lib.iter().map(|k| features(&k.geom)).collect();
    let n = fs.len() as f32;
    let mut mu = [0.0f32; 13]; let mut sd = [0.0f32; 13];
    for f in &fs { for i in 0..13 { mu[i] += f[i] / n; } }
    for f in &fs { for i in 0..13 { sd[i] += (f[i] - mu[i]).powi(2) / n; } }
    for s in sd.iter_mut() { *s = s.sqrt() + 1e-6; }
    let z = |f: &[f32; 13]| -> [f32; 13] { let mut o = [0.0; 13]; for i in 0..13 { o[i] = ((f[i] - mu[i]) / sd[i]).clamp(-3.0, 3.0); } o };
    let zl: Vec<[f32; 13]> = fs.iter().map(z).collect();
    let mut cand: Vec<(usize, Vec<(usize, f32)>)> = parts.iter().enumerate().map(|(pi, (_, g))| {
        let q = z(&features(g));
        let mut d: Vec<(usize, f32)> = zl.iter().enumerate().map(|(i, l)| (i, (0..13).map(|k| (l[k] - q[k]).powi(2)).sum::<f32>())).collect();
        d.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.cmp(&b.0)));
        (pi, d)
    }).collect();
    cand.sort_by(|a, b| a.1[0].1.partial_cmp(&b.1[0].1).unwrap().then(parts[a.0].0.cmp(&parts[b.0].0)));
    // what must be exclusive is the RESULTING seat (bone + where on it), not
    // the source part: two rear quarters, mirrored, landed on one thigh
    let resulting = |src: &Known, side: f32| -> Decision {
        let mut dec = src.decision.clone();
        if side_of(&src.geom) * if side == 0.0 { 1.0 } else { side } < 0.0 { dec.seat[2] = -dec.seat[2]; }
        if SIDED.contains(&dec.bone.as_str()) { dec.bone = format!("{}{}", dec.bone, if side < 0.0 { "_l" } else { "_r" }); }
        dec
    };
    let mut taken: Vec<(String, [f32; 3])> = Vec::new();
    let mut out = Vec::new();
    for (pi, d) in cand {
        let side = side_of(&parts[pi].1);
        let k = d.iter().map(|x| x.0).find(|&i| {
            let dec = resulting(&lib[i], side);
            !dec.keep || !taken.iter().any(|(b, s)| *b == dec.bone && dist3(*s, dec.seat) < 25.0)
        }).unwrap();
        let dec = resulting(&lib[k], side);
        if dec.keep { taken.push((dec.bone.clone(), dec.seat)); }
        out.push((parts[pi].0.clone(), dec, &lib[k]));
    }
    out
}

fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> Vec<Known> { library(include_str!("../assets/partlib.csv")) }

    #[test]
    fn the_library_loads_three_vehicles() {
        let l = lib();
        assert_eq!(l.len(), 90);
        for v in ["orion", "falcon", "flyer"] { assert!(l.iter().any(|k| k.vehicle == v)); }
    }

    #[test]
    fn no_two_parts_share_a_seat_on_one_side() {
        let l = lib();
        let fixture = include_str!("../assets/hover_parts.csv");
        let parts: Vec<(String, Geom)> = fixture.lines().skip(1).filter(|s| !s.trim().is_empty()).map(|line| {
            let c: Vec<&str> = line.split(',').collect(); let f = |i: usize| c[i].parse::<f32>().unwrap();
            (c[0].to_string(), Geom { c: [f(1), f(2), f(3)], r: [f(4), f(5), f(6)], nt: f(7) as u32, lo: [f(8), f(9), f(10)], hi: [f(11), f(12), f(13)] })
        }).collect();
        let got = pick_all(&l, &parts);
        for (i, a) in got.iter().enumerate() { for b in &got[i + 1..] {
            if !(a.1.keep && b.1.keep) || a.1.bone != b.1.bone { continue; }
            assert!(dist3(a.1.seat, b.1.seat) >= 25.0, "{} and {} share a seat on {}", a.0, b.0, a.1.bone);
        } }
    }

    #[test]
    fn a_known_part_picks_itself() {
        let l = lib();
        for k in l.iter().take(20) { assert_eq!(pick(&l, &k.geom).1.name, k.name); }
    }

    /// PARITY: the Rust lookup picks what the Python one did for every hover-car
    /// part -- same features, standardising, nearest neighbour and exclusivity.
    #[test]
    fn hover_car_picks_match_the_python_lookup() {
        let l = lib();
        let fixture = include_str!("../assets/hover_parts.csv");
        let mut n = 0;
        let mut parts: Vec<(String, Geom, String)> = Vec::new();
        for line in fixture.lines().skip(1).filter(|s| !s.trim().is_empty()) {
            let c: Vec<&str> = line.split(',').collect();
            let f = |i: usize| c[i].parse::<f32>().unwrap();
            let g = Geom { c: [f(1), f(2), f(3)], r: [f(4), f(5), f(6)], nt: f(7) as u32,
                           lo: [f(8), f(9), f(10)], hi: [f(11), f(12), f(13)] };
            parts.push((c[0].to_string(), g, c[14].to_string()));
            n += 1;
        }
        let input: Vec<(String, Geom)> = parts.iter().map(|(a, g, _)| (a.clone(), g.clone())).collect();
        for (name, _, src) in pick_all(&l, &input) {
            let want = &parts.iter().find(|p| p.0 == name).unwrap().2;
            assert_eq!(&format!("{}:{}", src.vehicle, src.name), want, "{name}");
        }
        assert_eq!(n, 15);
    }
}
