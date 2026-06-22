//! spatial.rs — a 3D layout + orbit-camera projection for the conversation graph, so an "aggressive"
//! spatial view can render messages as nodes in space (projected to 2D in egui's painter; true GPU 3D
//! via a glow callback can come later). Pure math, no egui: `layout` assigns each node a 3D position
//! along a descending helix with branches fanning out by angle; `OrbitCam::project` maps world→screen
//! with yaw/pitch/distance + perspective so nearer nodes draw larger. The point is spatial relevance —
//! the conversation's shape becomes navigable space instead of a scrolling list.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
    fn add(self, o: Vec3) -> Vec3 { Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z) }
    fn sub(self, o: Vec3) -> Vec3 { Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z) }
    fn scale(self, s: f32) -> Vec3 { Vec3::new(self.x * s, self.y * s, self.z * s) }
    fn dot(self, o: Vec3) -> f32 { self.x * o.x + self.y * o.y + self.z * o.z }
    fn norm(self) -> f32 { self.dot(self).sqrt() }
}

/// 3D positions from a parent list. Each node sits one "generation" deeper than its parent, spiralling
/// down a helix (angle ∝ generation) — the organic "noise" that keeps the graph from looking like a
/// ladder. On top of that, each node carries an accumulated angular *lane*: a fork's children fan over
/// an arc and their descendants INHERIT the divergence, so sub-branches keep to their own sector
/// instead of overlapping. Many siblings (e.g. a fuzz fan-out) pack into a capped arc. A linear spine
/// collapses to a single clean helix (all lanes 0). Deterministic, no force iteration.
pub fn layout(parents: &[Option<usize>]) -> Vec<Vec3> {
    let n = parents.len();
    let mut pos = vec![Vec3::new(0.0, 0.0, 0.0); n];
    let mut gen = vec![0usize; n];          // chain length to root
    let mut child_count = vec![0usize; n];  // total children per node
    let mut sib = vec![0usize; n];          // this node's rank among its parent's children
    for i in 0..n {
        if let Some(p) = parents[i] {
            gen[i] = gen[p] + 1;
            sib[i] = child_count[p];
            child_count[p] += 1;
        }
    }
    let turn = 0.62_f32;   // helix radians per generation
    let drop = 1.0_f32;    // y descent per generation
    let base_r = 2.2_f32;  // helix radius
    let max_arc = 2.6_f32; // total angular fan a set of siblings may occupy
    // accumulate the lane (parents precede children, so lane[p] is ready when we reach i)
    let mut lane = vec![0.0_f32; n];
    for i in 0..n {
        if let Some(p) = parents[i] {
            let m = child_count[p].max(1);
            let spread = if m > 1 { (max_arc / (m as f32 - 1.0)).min(0.7) } else { 0.0 };
            let offset = (sib[i] as f32 - (m as f32 - 1.0) / 2.0) * spread; // centered fan
            lane[i] = lane[p] + offset;
        }
    }
    for i in 0..n {
        let ang = gen[i] as f32 * turn + lane[i];
        let r = base_r + lane[i].abs() * 0.35; // diverged branches also push out radially
        pos[i] = Vec3::new(r * ang.cos(), -(gen[i] as f32) * drop, r * ang.sin());
    }
    pos
}

/// Bond rest length from a time gap: logarithmic, so a 1-second gap and a 1-hour gap both read on
/// screen (a long pause stretches the backbone; a flurry of quick turns sits tight).
pub fn bond_rest(dt_secs: f64) -> f32 { 1.3 + (1.0 + dt_secs.max(0.0)).ln() as f32 * 1.25 }

/// Lay the conversation out as a river flowing through the scene: the thread advances along +X by a
/// time-stretched step (a long pause reaches further), gently meandering in Y and Z so it snakes through
/// space, while a fork sends its children into separate lanes that flow alongside — and descendants inherit
/// the lane, so each sub-branch keeps its own channel. This spreads the graph across the room instead of
/// coiling it into a tight radial helix, yet preserves every relation: depth → distance along the flow,
/// branches → lanes. Deterministic; O(n).
pub fn flow(parents: &[Option<usize>], times_secs: &[f64]) -> Vec<Vec3> {
    let n = parents.len();
    let mut pos = vec![Vec3::new(0.0, 0.0, 0.0); n];
    if n == 0 { return pos; }
    let (mut child_count, mut sib) = (vec![0usize; n], vec![0usize; n]);
    for i in 0..n {
        if let Some(p) = parents[i] { sib[i] = child_count[p]; child_count[p] += 1; }
    }
    const SEP: f32 = 2.4; // lateral spacing between sibling channels
    let (mut x, mut ly, mut lz) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
    let mut root = 0usize;
    for i in 0..n {
        match parents[i] {
            None => { x[i] = 0.0; lz[i] = root as f32 * (SEP * 2.0); root += 1; } // stagger independent threads
            Some(p) => {
                let dt = (times_secs.get(i).copied().unwrap_or(0.0) - times_secs.get(p).copied().unwrap_or(0.0)).abs();
                x[i] = x[p] + 0.9 + (1.0 + dt.max(0.0)).ln() as f32 * 0.7; // time-stretched reach forward
                let m = child_count[p].max(1);
                if m > 1 {
                    let k = sib[i] as f32 - (m as f32 - 1.0) / 2.0; // centered rank among siblings
                    ly[i] = ly[p] + ((sib[i] % 2) as f32 - 0.5) * SEP * 0.7; // alternate a touch up/down
                    lz[i] = lz[p] + k * SEP; // fan siblings sideways into their own lanes
                } else { ly[i] = ly[p]; lz[i] = lz[p]; } // a linear spine flows straight in its lane
            }
        }
    }
    let xc = x.iter().copied().sum::<f32>() / n as f32; // center the flow on the origin
    let th = std::env::var("LOGSCOPE_TUNE_HEIGHT").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0);
    for i in 0..n {
        let xi = x[i] - xc;
        let amp = 1.0 + (xi.abs() * 0.03).min(0.8); // opens up a little as it flows outward
        let zc = (xi * 0.16).sin() * 2.2 * amp + (xi * 0.41 + 1.0).sin() * 1.0; // two-frequency lateral meander
        let yc = ((xi * 0.11 + 0.6).sin() * 2.4 + (xi * 0.05 - 0.3).sin() * 1.0) * th;  // vertical roll, tunable
        pos[i] = Vec3::new(xi, yc + ly[i], zc + lz[i]);
    }
    pos
}


#[derive(Clone, Copy, Debug)]
pub struct FoldParams {
    pub iters: usize,      // relaxation steps
    pub edge_k: f32,       // bond spring stiffness (pull toward the time-based rest length)
    pub bend_k: f32,       // backbone rigidity (resist bending — the "physical rigidity")
    pub repel_k: f32,      // non-bonded repulsion (drives the folding / prevents overlap)
    pub repel_radius: f32, // repulsion cutoff distance
    pub dt: f32,           // integration step
    pub damping: f32,      // velocity retained per step (<1 = settles)
}
impl Default for FoldParams {
    fn default() -> Self { Self { iters: 90, edge_k: 0.45, bend_k: 0.20, repel_k: 0.9, repel_radius: 3.0, dt: 0.5, damping: 0.82 } }
}

/// Lay the conversation out as a semi-rigid backbone that FOLDS under force, like a protein. Bonds
/// (parent→child) want a length set by the time gap between the turns (`bond_rest`); the backbone
/// resists bending (angular rigidity toward the grandparent→parent line); every node repels every
/// other, so a long chain folds back on itself and branches push apart instead of overlapping. Seeded
/// from the helix so it starts untangled. Roots are pinned. Deterministic for given inputs/params.
pub fn fold(parents: &[Option<usize>], times_secs: &[f64], pa: &FoldParams) -> Vec<Vec3> {
    let n = parents.len();
    let mut pos = layout(parents); // helix seed
    if n <= 1 { return pos; }
    let rest: Vec<f32> = (0..n).map(|i| match parents[i] {
        Some(p) => bond_rest((times_secs.get(i).copied().unwrap_or(0.0) - times_secs.get(p).copied().unwrap_or(0.0)).abs()),
        None => 0.0,
    }).collect();
    let mut vel = vec![Vec3::new(0.0, 0.0, 0.0); n];
    for _ in 0..pa.iters {
        let mut force = vec![Vec3::new(0.0, 0.0, 0.0); n];
        // bonds → spring toward rest length
        for i in 0..n {
            if let Some(p) = parents[i] {
                let d = pos[i].sub(pos[p]);
                let len = d.norm().max(1e-4);
                let dir = d.scale(1.0 / len);
                let f = pa.edge_k * (len - rest[i]); // >0 when too long → pull together
                force[i] = force[i].sub(dir.scale(f));
                force[p] = force[p].add(dir.scale(f));
            }
        }
        // bending rigidity → push each node toward where a straight backbone (grandparent→parent
        // extended by the bond length) would put it
        for i in 0..n {
            if let Some(p) = parents[i] {
                if let Some(gp) = parents[p] {
                    let back = pos[p].sub(pos[gp]);
                    let bl = back.norm().max(1e-4);
                    let ideal = pos[p].add(back.scale(rest[i] / bl));
                    force[i] = force[i].add(ideal.sub(pos[i]).scale(pa.bend_k));
                }
            }
        }
        // repulsion → all pairs within the cutoff push apart (O(n^2); conversation-sized n is fine)
        for i in 0..n {
            for j in (i + 1)..n {
                let d = pos[i].sub(pos[j]);
                let l2 = d.dot(d);
                if l2 > 1e-6 && l2 < pa.repel_radius * pa.repel_radius {
                    let len = l2.sqrt();
                    let dir = d.scale(1.0 / len);
                    let f = pa.repel_k * (pa.repel_radius - len) / pa.repel_radius;
                    force[i] = force[i].add(dir.scale(f));
                    force[j] = force[j].sub(dir.scale(f));
                }
            }
        }
        // integrate (damped); pin roots so the structure doesn't drift
        for i in 0..n {
            if parents[i].is_none() { vel[i] = Vec3::new(0.0, 0.0, 0.0); continue; }
            vel[i] = vel[i].add(force[i].scale(pa.dt)).scale(pa.damping);
            pos[i] = pos[i].add(vel[i].scale(pa.dt));
        }
    }
    pos
}

#[derive(Clone, Debug)]
pub struct OrbitCam {
    pub yaw: f32,    // rotation around world y
    pub pitch: f32,  // rotation around camera x
    pub dist: f32,   // camera distance from target
    pub target: Vec3,
}

impl Default for OrbitCam {
    fn default() -> Self { Self { yaw: 0.5, pitch: 0.42, dist: 15.0, target: Vec3::new(0.0, -3.0, 0.0) } }
}

impl OrbitCam {
    pub fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.45, 1.45); // avoid flipping over the poles
    }
    pub fn zoom(&mut self, factor: f32) { self.dist = (self.dist * factor).clamp(3.0, 80.0); }

    /// A gently drifting copy for idle "breathing": a small parallax sway in yaw/pitch and a faint dolly,
    /// driven by time `t`. It's additive on top of the user's framing and never accumulates, so manual
    /// orbit/zoom stay in control — this is the slight parallax + rotation, not a full orbit.
    pub fn drifted(&self, t: f32) -> OrbitCam {
        let mut c = self.clone();
        c.yaw += (t * 0.16).sin() * 0.05 + (t * 0.057 + 1.3).sin() * 0.025; // ≈ ±4° lazy sway, two rates
        c.pitch = (c.pitch + (t * 0.13 + 0.6).sin() * 0.022).clamp(-1.45, 1.45);
        c.dist *= 1.0 + (t * 0.09).sin() * 0.012; // a faint dolly in/out
        c
    }

    /// Project a world point to screen. Returns (sx, sy, depth) where depth is distance in front of the
    /// camera (>0). None if behind the camera. (cx,cy)=viewport center; `scale` ties world units to px
    /// (and is divided by depth for perspective, so nearer points project larger and farther smaller).
    pub fn project(&self, p: Vec3, cx: f32, cy: f32, scale: f32) -> Option<(f32, f32, f32)> {
        let (dx, dy, dz) = (p.x - self.target.x, p.y - self.target.y, p.z - self.target.z);
        // yaw around y
        let (cyaw, syaw) = (self.yaw.cos(), self.yaw.sin());
        let x1 = dx * cyaw + dz * syaw;
        let z1 = -dx * syaw + dz * cyaw;
        // pitch around x
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        let y2 = dy * cp - z1 * sp;
        let z2 = dy * sp + z1 * cp;
        // camera sits back along +z at `dist`, looking at the target; depth = how far in front
        let depth = self.dist - z2;
        if depth <= 0.1 { return None; }
        let persp = scale / depth;
        Some((cx + x1 * persp, cy - y2 * persp, depth))
    }
}

/// Mean position of all nodes (the natural camera target so the whole graph sits centered).
pub fn centroid(p: &[Vec3]) -> Vec3 {
    if p.is_empty() { return Vec3::new(0.0, 0.0, 0.0); }
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    for v in p { x += v.x; y += v.y; z += v.z; }
    let n = p.len() as f32;
    Vec3::new(x / n, y / n, z / n)
}

/// Largest distance from `c` to any node — the radius the camera must fit in view.
pub fn bounding_radius(p: &[Vec3], c: Vec3) -> f32 {
    p.iter().map(|v| ((v.x - c.x).powi(2) + (v.y - c.y).powi(2) + (v.z - c.z).powi(2)).sqrt())
        .fold(0.0_f32, f32::max)
}

/// Camera distance that frames a graph of the given bounding radius (with `project`'s scale, the
/// projected extent ≈ scale·r/dist; this keeps it inside the viewport with margin). Clamped.
pub fn fit_distance(radius: f32) -> f32 { (2.2 * radius + 3.0).clamp(5.0, 80.0) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_spine_is_a_descending_helix() {
        let parents = vec![None, Some(0), Some(1), Some(2)];
        let p = layout(&parents);
        assert_eq!(p.len(), 4);
        // strictly descending in y (each generation drops)
        for i in 1..p.len() { assert!(p[i].y < p[i - 1].y, "generation {i} sits below {}", i - 1); }
        // root at the top, on the helix radius
        assert!((p[0].y).abs() < 0.001);
        assert!((p[0].x * p[0].x + p[0].z * p[0].z).sqrt() > 1.0, "root is out on the helix radius");
    }

    #[test]
    fn siblings_fan_apart() {
        // two children of node 0 → different angular positions, not coincident
        let parents = vec![None, Some(0), Some(0)];
        let p = layout(&parents);
        let d = ((p[1].x - p[2].x).powi(2) + (p[1].z - p[2].z).powi(2)).sqrt();
        assert!(d > 0.5, "siblings separate in space, gap {d}");
    }

    #[test]
    fn project_target_lands_at_center() {
        let c = OrbitCam::default();
        let (sx, sy, depth) = c.project(c.target, 200.0, 150.0, 300.0).unwrap();
        assert!((sx - 200.0).abs() < 0.001 && (sy - 150.0).abs() < 0.001, "target projects to viewport center");
        assert!((depth - c.dist).abs() < 0.001, "target depth == camera distance");
    }

    #[test]
    fn nearer_points_project_larger_and_yaw_moves_x() {
        let mut c = OrbitCam::default();
        c.yaw = 0.0; c.pitch = 0.0; c.target = Vec3::new(0.0, 0.0, 0.0);
        // same x offset, one nearer (z toward camera = +z) one farther (−z)
        let near = c.project(Vec3::new(1.0, 0.0, 4.0), 0.0, 0.0, 300.0).unwrap();
        let far = c.project(Vec3::new(1.0, 0.0, -4.0), 0.0, 0.0, 300.0).unwrap();
        assert!(near.0.abs() > far.0.abs(), "nearer point projects wider from center (larger): {near:?} vs {far:?}");
        assert!(near.2 < far.2, "nearer point has smaller depth");
        // yaw rotates the scene → projected x of a +x point changes
        let before = c.project(Vec3::new(2.0, 0.0, 0.0), 0.0, 0.0, 300.0).unwrap().0;
        c.orbit(0.5, 0.0);
        let after = c.project(Vec3::new(2.0, 0.0, 0.0), 0.0, 0.0, 300.0).unwrap().0;
        assert!((before - after).abs() > 1.0, "yaw changes the projected x");
    }

    #[test]
    fn deep_subbranches_stay_separated() {
        // 0→1 ; node1 forks to 2 and 3 ; 2→4, 3→5 (two descendants at the same generation)
        let parents = vec![None, Some(0), Some(1), Some(1), Some(2), Some(3)];
        let p = layout(&parents);
        let gap = |a: usize, b: usize| ((p[a].x - p[b].x).powi(2) + (p[a].y - p[b].y).powi(2) + (p[a].z - p[b].z).powi(2)).sqrt();
        assert!(gap(2, 3) > 0.5, "the immediate fork separates");
        assert!(gap(4, 5) > 0.5, "descendants of the two branches stay separated (lane inherited), gap {}", gap(4, 5));
    }

    #[test]
    fn fuzz_fan_is_all_distinct() {
        // one node with 12 children (a fuzz fan-out) — none should land on top of another
        let mut parents = vec![None];
        for _ in 0..12 { parents.push(Some(0)); }
        let p = layout(&parents);
        for a in 1..p.len() {
            for b in (a + 1)..p.len() {
                let d = ((p[a].x - p[b].x).powi(2) + (p[a].z - p[b].z).powi(2)).sqrt();
                assert!(d > 0.1, "fuzz branches {a} and {b} are distinct (d={d})");
            }
        }
    }

    #[test]
    fn fold_respects_time_based_bond_lengths() {
        // linear chain; gaps: 1s, 1s, then a long 3600s pause before the last turn
        let parents = vec![None, Some(0), Some(1), Some(2)];
        let times = vec![0.0, 1.0, 2.0, 3602.0];
        let p = fold(&parents, &times, &FoldParams::default());
        let blen = |a: usize, b: usize| p[a].sub(p[b]).norm();
        let short = blen(1, 0);
        let long = blen(3, 2); // the bond across the long pause
        assert!(long > short * 1.5, "a long time-gap stretches its bond ({long} vs {short})");
        // each bond lands near its time-based rest length (springs converged)
        for i in 1..parents.len() {
            let rest = bond_rest((times[i] - times[i - 1]).abs());
            let got = blen(i, i - 1);
            assert!((got - rest).abs() < rest * 0.6, "bond {i} near rest {rest}, got {got}");
        }
    }

    #[test]
    fn fold_separates_and_is_deterministic() {
        // a fork that, unfolded, would overlap; folding + repulsion must keep nodes apart
        let parents = vec![None, Some(0), Some(0), Some(1), Some(2)];
        let times = vec![0.0, 1.0, 1.0, 2.0, 2.0];
        let a = fold(&parents, &times, &FoldParams::default());
        let b = fold(&parents, &times, &FoldParams::default());
        assert_eq!(a, b, "deterministic for identical inputs");
        let mut min = f32::MAX;
        for i in 0..a.len() { for j in (i + 1)..a.len() { min = min.min(a[i].sub(a[j]).norm()); } }
        assert!(min > 0.4, "repulsion keeps every node apart (min pair distance {min})");
    }

    #[test]
    fn flow_advances_forward_and_spreads() {
        // a linear chain flows forward in +x (child always ahead of parent) and spreads wider as it grows
        let p4 = flow(&[None, Some(0), Some(1), Some(2)], &[0.0, 1.0, 2.0, 3.0]);
        for i in 1..p4.len() { assert!(p4[i].x > p4[i - 1].x, "node {i} flows ahead of {}", i - 1); }
        let span = |v: &[Vec3]| v.iter().map(|p| p.x).fold(f32::MIN, f32::max) - v.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        let mut long_parents = vec![None]; let mut long_times = vec![0.0];
        for i in 0..12 { long_parents.push(Some(i)); long_times.push((i + 1) as f64); }
        let p12 = flow(&long_parents, &long_times);
        assert!(span(&p12) > span(&p4), "a longer thread spreads further along the flow");
    }

    #[test]
    fn flow_siblings_take_separate_lanes() {
        let p = flow(&[None, Some(0), Some(0), Some(0)], &[0.0, 1.0, 1.0, 1.0]);
        let gap = |a: usize, b: usize| p[a].sub(p[b]).norm();
        assert!(gap(1, 2) > 1.0 && gap(1, 3) > 1.0 && gap(2, 3) > 1.0, "forked children sit in distinct lanes");
        // a deeper descendant inherits its lane, staying off the spine
        let p2 = flow(&[None, Some(0), Some(0), Some(2)], &[0.0, 1.0, 1.0, 2.0]);
        assert!((p2[3].z - p2[1].z).abs() > 1.0, "the branch's descendant keeps its own channel");
    }

    #[test]
    fn flow_deterministic_and_degenerate_safe() {
        let a = flow(&[None, Some(0), Some(0), Some(1)], &[0.0, 1.0, 1.0, 2.0]);
        let b = flow(&[None, Some(0), Some(0), Some(1)], &[0.0, 1.0, 1.0, 2.0]);
        assert_eq!(a, b, "deterministic");
        assert_eq!(flow(&[], &[]).len(), 0);
        assert_eq!(flow(&[None], &[0.0]).len(), 1);
    }

    #[test]
    fn drift_is_small_and_nonmutating() {
        let base = OrbitCam::default();
        let mut max_dyaw = 0.0_f32;
        for k in 0..200 {
            let t = k as f32 * 0.3;
            let d = base.drifted(t);
            max_dyaw = max_dyaw.max((d.yaw - base.yaw).abs());
            assert!((d.pitch - base.pitch).abs() < 0.05, "pitch sway stays tiny");
            assert!((d.dist / base.dist - 1.0).abs() < 0.02, "dolly stays faint");
        }
        assert!(max_dyaw < 0.1 && max_dyaw > 0.01, "yaw sways a little, never a full orbit (max {max_dyaw})");
        assert_eq!(base.yaw, 0.5, "drifted() does not mutate the source camera");
    }

    #[test]
    fn fold_single_node_is_safe() {
        assert_eq!(fold(&[None], &[0.0], &FoldParams::default()).len(), 1);
        assert_eq!(fold(&[], &[], &FoldParams::default()).len(), 0);
    }

    #[test]
    fn auto_frame_centers_and_fits() {
        let parents = vec![None, Some(0), Some(1), Some(2), Some(3)];
        let p = layout(&parents);
        let c = centroid(&p);
        // centroid sits inside the spread, below the root (helix descends)
        assert!(c.y < p[0].y && c.y > p[p.len() - 1].y, "centroid is mid-strand");
        let r = bounding_radius(&p, c);
        assert!(r > 0.0, "non-degenerate radius");
        let d = fit_distance(r);
        assert!(d >= 5.0 && d <= 80.0, "fit distance clamped, got {d}");
        // pointing the camera at the centroid puts it dead-center
        let mut cam = OrbitCam::default();
        cam.target = c; cam.dist = d;
        let (sx, sy, _) = cam.project(c, 300.0, 200.0, 400.0).unwrap();
        assert!((sx - 300.0).abs() < 0.001 && (sy - 200.0).abs() < 0.001);
    }

    #[test]
    fn empty_layout_is_safe() {
        assert_eq!(centroid(&[]), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(bounding_radius(&[], Vec3::new(0.0, 0.0, 0.0)), 0.0);
    }

    #[test]
    fn zoom_clamps() {
        let mut c = OrbitCam::default();
        for _ in 0..50 { c.zoom(0.5); }
        assert!(c.dist >= 3.0, "zoom-in clamped");
        for _ in 0..50 { c.zoom(2.0); }
        assert!(c.dist <= 80.0, "zoom-out clamped");
    }
}
