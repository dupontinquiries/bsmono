//! `sim::cloth` — cloth for capes, banners and flags: position-based dynamics.
//!
//! A grid of particles held by distance constraints -- STRUCTURAL (neighbours),
//! SHEAR (diagonals) and BEND (every other particle) -- integrated with Verlet
//! and projected Gauss-Seidel, the scheme games use because it cannot blow up
//! the way force springs do at a fixed frame budget. Per step:
//!
//! 1. predict: carried velocity (damped) + gravity + wind (per triangle, the
//!    model in `crate::cloth_wind_force`: force ~ |normal . wind| x strength x
//!    area, along the normal, from the wind RELATIVE to the cloth);
//! 2. pins: pinned particles are moved to their anchors (a cape's top edge on the
//!    shoulders) -- infinite mass, so the anchors drive and the cloth follows;
//! 3. constraints, `iterations` sweeps; stiffness per sweep is corrected so the
//!    result does not depend on the iteration count;
//! 4. collision: pushed out of each capsule (the body), then velocity is what
//!    moved.
//!
//! The frame's `dt` is split into equal substeps no longer than `max_substep`,
//! so a 30 Hz and a 144 Hz frame rate settle to the same shape.
//!
//! Pure: no clock, no RNG, no GPU, no I/O. Deterministic: same inputs, same
//! floats, every run.

/// Tuning. Defaults are a light fabric (a cape) at human scale, in blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClothParams {
    pub gravity: [f32; 3],
    /// Fraction of velocity kept per SECOND (0..1): air resistance.
    pub keep_per_s: f32,
    /// Constraint sweeps per substep.
    pub iterations: u32,
    /// Longest substep (s).
    pub max_substep: f32,
    /// Stiffness of the three constraint kinds, 0..1 (1 = inextensible).
    pub stretch: f32,
    pub shear: f32,
    pub bend: f32,
    /// Wind force per unit area per (speed^2): how much the fabric catches.
    pub wind_drag: f32,
    /// Mass per particle (blocks are ~1 m; a cape is ~1 kg over its particles).
    pub particle_mass: f32,
    /// Collision margin kept outside a capsule's radius.
    pub margin: f32,
}

impl Default for ClothParams {
    fn default() -> Self {
        ClothParams {
            gravity: [0.0, -9.81, 0.0],
            keep_per_s: 0.4,
            iterations: 8,
            max_substep: 1.0 / 240.0,
            stretch: 1.0,
            shear: 0.6,
            bend: 0.15,
            wind_drag: 0.6,
            particle_mass: 0.02,
            margin: 0.01,
        }
    }
}

/// A capsule: the segment `a`-`b` swept by `radius` (a torso, an arm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Link {
    a: usize,
    b: usize,
    rest: f32,
    kind: u8, // 0 stretch, 1 shear, 2 bend
}

#[derive(Clone, Debug)]
pub struct Cloth {
    pub cols: usize,
    pub rows: usize,
    pub pos: Vec<[f32; 3]>,
    prev: Vec<[f32; 3]>,
    pinned: Vec<bool>,
    links: Vec<Link>,
    pub params: ClothParams,
}

// ── small vector helpers ──
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn mul(a: [f32; 3], s: f32) -> [f32; 3] { [a[0] * s, a[1] * s, a[2] * s] }
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn len(a: [f32; 3]) -> f32 { dot(a, a).sqrt() }

impl Cloth {
    /// A `cols` x `rows` sheet, `spacing` apart, its first row at `origin` along
    /// `across`, rows stepping along `down` (both unit vectors; a cape: across the
    /// shoulders, down the back). Nothing is pinned yet.
    pub fn grid(cols: usize, rows: usize, spacing: f32, origin: [f32; 3], across: [f32; 3], down: [f32; 3], params: ClothParams) -> Cloth {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let mut pos = Vec::with_capacity(cols * rows);
        for r in 0..rows {
            for c in 0..cols {
                pos.push(add(origin, add(mul(across, c as f32 * spacing), mul(down, r as f32 * spacing))));
            }
        }
        let idx = |c: usize, r: usize| r * cols + c;
        let mut links = Vec::new();
        let mut link = |a: usize, b: usize, kind: u8, pos: &Vec<[f32; 3]>| links.push(Link { a, b, rest: len(sub(pos[a], pos[b])), kind });
        for r in 0..rows {
            for c in 0..cols {
                if c + 1 < cols { link(idx(c, r), idx(c + 1, r), 0, &pos); }
                if r + 1 < rows { link(idx(c, r), idx(c, r + 1), 0, &pos); }
                if c + 1 < cols && r + 1 < rows {
                    link(idx(c, r), idx(c + 1, r + 1), 1, &pos);
                    link(idx(c + 1, r), idx(c, r + 1), 1, &pos);
                }
                if c + 2 < cols { link(idx(c, r), idx(c + 2, r), 2, &pos); }
                if r + 2 < rows { link(idx(c, r), idx(c, r + 2), 2, &pos); }
            }
        }
        Cloth { cols, rows, prev: pos.clone(), pinned: vec![false; pos.len()], pos, links, params }
    }

    pub fn index(&self, col: usize, row: usize) -> usize { row * self.cols + col }

    /// Pin a particle: from now on it goes wherever `set_pin` puts it.
    pub fn pin(&mut self, i: usize) { self.pinned[i] = true; }

    /// Move a pinned particle to its anchor for the next step. Its previous
    /// position moves with it, so a pin carries no velocity of its own into the
    /// cloth except through the constraints (a sprinting body drags its cape; it
    /// does not fling it).
    pub fn set_pin(&mut self, i: usize, p: [f32; 3]) {
        self.pos[i] = p;
        self.prev[i] = p;
    }

    pub fn is_pinned(&self, i: usize) -> bool { self.pinned[i] }

    /// Advance by `dt` seconds in `wind` (world velocity of the air, blocks/s),
    /// pushed out of `colliders`.
    pub fn step(&mut self, dt: f32, wind: [f32; 3], colliders: &[Capsule]) {
        if !(dt > 0.0) || !dt.is_finite() { return; }
        let n = ((dt / self.params.max_substep.max(1e-4)).ceil() as u32).clamp(1, 64);
        let h = dt / n as f32;
        for _ in 0..n {
            self.substep(h, wind, colliders);
        }
    }

    fn substep(&mut self, h: f32, wind: [f32; 3], colliders: &[Capsule]) {
        let p = self.params;
        let keep = p.keep_per_s.clamp(0.0, 1.0).powf(h);
        // Wind: per triangle, along its normal, from the air speed relative to
        // the triangle, split over its corners.
        let mut accel = vec![p.gravity; self.pos.len()];
        let inv_m = 1.0 / p.particle_mass.max(1e-6);
        for r in 0..self.rows - 1 {
            for c in 0..self.cols - 1 {
                let (i00, i10, i01, i11) = (self.index(c, r), self.index(c + 1, r), self.index(c, r + 1), self.index(c + 1, r + 1));
                for tri in [[i00, i10, i11], [i00, i11, i01]] {
                    let (a, b, cc) = (self.pos[tri[0]], self.pos[tri[1]], self.pos[tri[2]]);
                    let nrm = cross(sub(b, a), sub(cc, a));
                    let area2 = len(nrm);
                    if area2 < 1e-9 { continue; }
                    let nhat = mul(nrm, 1.0 / area2);
                    let vel = mul(add(add(sub(a, self.prev[tri[0]]), sub(b, self.prev[tri[1]])), sub(cc, self.prev[tri[2]])), 1.0 / (3.0 * h));
                    let rel = sub(wind, vel);
                    let speed = len(rel);
                    if speed < 1e-6 { continue; }
                    let facing = dot(nhat, mul(rel, 1.0 / speed));
                    let f = crate::cloth_wind_force::force(facing, p.wind_drag * speed * speed, area2 * 0.5);
                    let fvec = mul(nhat, f * facing.signum() / 3.0);
                    for &i in &tri {
                        accel[i] = add(accel[i], mul(fvec, inv_m));
                    }
                }
            }
        }
        let old = self.pos.clone();
        for i in 0..self.pos.len() {
            if self.pinned[i] { continue; }
            let v = sub(self.pos[i], self.prev[i]);
            self.pos[i] = add(add(self.pos[i], mul(v, keep)), mul(accel[i], h * h));
        }
        // Stiffness per sweep so that `iterations` sweeps give the set stiffness.
        let iters = p.iterations.max(1);
        let k_of = |k: f32| 1.0 - (1.0 - k.clamp(0.0, 1.0)).powf(1.0 / iters as f32);
        let ks = [k_of(p.stretch), k_of(p.shear), k_of(p.bend)];
        for _ in 0..iters {
            for l in &self.links {
                let (wa, wb) = (if self.pinned[l.a] { 0.0 } else { 1.0 }, if self.pinned[l.b] { 0.0 } else { 1.0 });
                let w = wa + wb;
                if w == 0.0 { continue; }
                let d = sub(self.pos[l.b], self.pos[l.a]);
                let dl = len(d);
                if dl < 1e-9 { continue; }
                let corr = mul(d, ks[l.kind as usize] * (dl - l.rest) / (dl * w));
                self.pos[l.a] = add(self.pos[l.a], mul(corr, wa));
                self.pos[l.b] = sub(self.pos[l.b], mul(corr, wb));
            }
            for i in 0..self.pos.len() {
                if self.pinned[i] { continue; }
                for cap in colliders {
                    self.pos[i] = push_out(self.pos[i], cap, p.margin);
                }
            }
        }
        self.prev = old;
        // A pin's previous position is its current one (see `set_pin`).
        for i in 0..self.pos.len() {
            if self.pinned[i] { self.prev[i] = self.pos[i]; }
        }
    }

    /// Largest relative stretch of any structural link (0 = at rest length).
    pub fn max_stretch(&self) -> f32 {
        self.links.iter().filter(|l| l.kind == 0).map(|l| (len(sub(self.pos[l.b], self.pos[l.a])) - l.rest).abs() / l.rest.max(1e-9)).fold(0.0, f32::max)
    }

    /// Mean particle speed over the last step (blocks per second, given its dt).
    pub fn mean_speed(&self, dt: f32) -> f32 {
        let s: f32 = self.pos.iter().zip(&self.prev).map(|(a, b)| len(sub(*a, *b))).sum();
        s / (self.pos.len() as f32 * dt.max(1e-6))
    }

    /// Two triangles per grid cell, for drawing.
    pub fn triangles(&self) -> Vec<[usize; 3]> {
        let mut t = Vec::with_capacity((self.cols - 1) * (self.rows - 1) * 2);
        for r in 0..self.rows - 1 {
            for c in 0..self.cols - 1 {
                let (i00, i10, i01, i11) = (self.index(c, r), self.index(c + 1, r), self.index(c, r + 1), self.index(c + 1, r + 1));
                t.push([i00, i10, i11]);
                t.push([i00, i11, i01]);
            }
        }
        t
    }
}

/// `p` moved to the surface of `cap` (+ margin) if it is inside.
pub fn push_out(p: [f32; 3], cap: &Capsule, margin: f32) -> [f32; 3] {
    let ab = sub(cap.b, cap.a);
    let t = if dot(ab, ab) > 1e-12 { (dot(sub(p, cap.a), ab) / dot(ab, ab)).clamp(0.0, 1.0) } else { 0.0 };
    let closest = add(cap.a, mul(ab, t));
    let d = sub(p, closest);
    let dl = len(d);
    let r = cap.radius + margin;
    if dl >= r { return p; }
    if dl < 1e-9 { return add(closest, [0.0, 0.0, r]); } // exactly on the axis: any outward direction
    add(closest, mul(d, r / dl))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOWN: [f32; 3] = [0.0, -1.0, 0.0];
    const ACROSS: [f32; 3] = [1.0, 0.0, 0.0];

    /// A 10 x 14 cape, 0.07 apart (~0.65 x 0.9 blocks), its top row pinned.
    fn cape() -> Cloth {
        let mut c = Cloth::grid(10, 14, 0.07, [0.0, 2.0, 0.0], ACROSS, [0.0, 0.0, -1.0], ClothParams::default());
        for col in 0..c.cols { let i = c.index(col, 0); c.pin(i); }
        c
    }
    fn centroid(c: &Cloth) -> [f32; 3] {
        let n = c.pos.len() as f32;
        c.pos.iter().fold([0.0; 3], |a, p| add(a, mul(*p, 1.0 / n)))
    }
    fn finite(c: &Cloth) -> bool { c.pos.iter().all(|p| p.iter().all(|x| x.is_finite())) }

    #[test]
    fn it_hangs_under_gravity_and_settles_without_stretching() {
        let mut c = cape(); // starts horizontal, pinned along one edge
        for _ in 0..(6 * 60) { c.step(1.0 / 60.0, [0.0; 3], &[]); }
        assert!(finite(&c));
        let bottom = c.pos[c.index(5, c.rows - 1)];
        let hang = (c.rows - 1) as f32 * 0.07;
        assert!(bottom[1] < 2.0 - 0.9 * hang, "the free edge hangs down: y {:.3}", bottom[1]);
        assert!(c.max_stretch() < 0.03, "stretch {:.3}", c.max_stretch());
        assert!(c.mean_speed(1.0 / 60.0) < 0.02, "at rest: {:.4} blocks/s", c.mean_speed(1.0 / 60.0));
    }

    #[test]
    fn wind_pushes_it_downwind_and_strong_gusts_never_blow_it_up() {
        for hz in [30.0f32, 144.0] {
            let mut c = cape();
            let start_z = { for _ in 0..(3.0 * hz) as usize { c.step(1.0 / hz, [0.0; 3], &[]); } centroid(&c)[2] };
            // 20 s of gusting wind along -z, up to 25 blocks/s.
            for i in 0..(20.0 * hz) as usize {
                let t = i as f32 / hz;
                let gust = 15.0 + 10.0 * (t * 1.7).sin();
                c.step(1.0 / hz, [0.0, 0.0, -gust], &[]);
                assert!(finite(&c), "{hz} Hz: NaN at {t:.2} s");
                assert!(c.max_stretch() < 0.15, "{hz} Hz: stretch {:.3} at {t:.2} s", c.max_stretch());
            }
            let z = centroid(&c)[2];
            assert!(z < start_z - 0.2, "{hz} Hz: blown downwind ({start_z:.2} -> {z:.2})");
            assert!(c.pos.iter().all(|p| len(sub(*p, [0.3, 2.0, 0.0])) < 2.0), "{hz} Hz: stays attached, nothing flies off");
        }
    }

    #[test]
    fn pins_follow_their_anchors_exactly_at_a_sprint() {
        let mut c = cape();
        let pins: Vec<usize> = (0..c.cols).map(|col| c.index(col, 0)).collect();
        for i in 0..240 {
            let off = [i as f32 * 0.1, 0.02 * (i as f32).sin(), 0.0]; // 6 blocks/s
            for (k, &p) in pins.iter().enumerate() {
                c.set_pin(p, add([k as f32 * 0.07, 2.0, 0.0], off));
            }
            c.step(1.0 / 60.0, [0.0; 3], &[]);
            for (k, &p) in pins.iter().enumerate() {
                assert_eq!(c.pos[p], add([k as f32 * 0.07, 2.0, 0.0], off), "pin {k} at step {i}");
            }
        }
        assert!(finite(&c) && c.max_stretch() < 0.2, "stretch {:.3}", c.max_stretch());
        // The cape trails behind the direction of travel (+x).
        let free = c.pos[c.index(5, c.rows - 1)];
        assert!(free[0] < c.pos[pins[5]][0], "the free edge trails the runner");
    }

    #[test]
    fn it_drapes_over_a_body_instead_of_passing_through() {
        let body = Capsule { a: [0.32, 0.8, 0.0], b: [0.32, 1.9, 0.0], radius: 0.25 };
        let mut c = Cloth::grid(10, 14, 0.07, [0.0, 2.0, 0.0], ACROSS, DOWN, ClothParams::default());
        for col in 0..c.cols { let i = c.index(col, 0); c.pin(i); c.set_pin(i, [col as f32 * 0.07, 2.0, 0.26]); }
        // Start the sheet in front of the body, then let the wind press it back onto it.
        for _ in 0..600 { c.step(1.0 / 60.0, [0.0, 0.0, -6.0], &[body]); }
        let inside = c.pos.iter().enumerate().filter(|(i, p)| !c.is_pinned(*i) && {
            let q = push_out(**p, &body, 0.0);
            len(sub(q, **p)) > 0.005
        }).count();
        assert_eq!(inside, 0, "no particle inside the body");
        // ...and not because it went THROUGH: without collision the wind blows the
        // sheet straight past the body, where no particle is "inside" either. The
        // part of the sheet level with the body is held in front of it, pressed
        // against it.
        let beside: Vec<[f32; 3]> = c.pos.iter().enumerate().filter(|(i, p)| !c.is_pinned(*i) && p[1] > 0.9 && p[1] < 1.8 && (p[0] - 0.32).abs() < 0.12).map(|(_, p)| *p).collect();
        assert!(!beside.is_empty());
        assert!(beside.iter().all(|p| p[2] > 0.0), "held in front of the body (z > 0), not blown through it: min z {:.3}", beside.iter().map(|p| p[2]).fold(f32::MAX, f32::min));
        let touching = beside.iter().filter(|p| len(sub(**p, push_out(**p, &body, 0.02))) > 0.0).count();
        assert!(touching > 0, "pressed against the body");
        assert!(finite(&c));
    }

    #[test]
    fn it_is_deterministic_and_frame_rate_does_not_change_the_settled_shape() {
        let run = |hz: f32| { let mut c = cape(); for _ in 0..(8.0 * hz) as usize { c.step(1.0 / hz, [0.0, 0.0, -4.0], &[]); } c };
        let (a, b) = (run(60.0), run(60.0));
        assert_eq!(a.pos, b.pos, "bit-identical");
        let (s60, s144) = (run(60.0), run(144.0));
        let worst = s60.pos.iter().zip(&s144.pos).map(|(p, q)| len(sub(*p, *q))).fold(0.0, f32::max);
        assert!(worst < 0.05, "60 vs 144 Hz settled shapes differ by {worst:.3} blocks");
    }

    #[test]
    fn push_out_puts_a_point_on_the_capsule_surface() {
        let cap = Capsule { a: [0.0, 0.0, 0.0], b: [0.0, 1.0, 0.0], radius: 0.5 };
        let q = push_out([0.1, 0.5, 0.0], &cap, 0.0);
        assert!((len(sub(q, [0.0, 0.5, 0.0])) - 0.5).abs() < 1e-5);
        assert_eq!(push_out([2.0, 0.5, 0.0], &cap, 0.0), [2.0, 0.5, 0.0], "outside is untouched");
        let end = push_out([0.0, 1.2, 0.0], &cap, 0.0);
        assert!((len(sub(end, [0.0, 1.0, 0.0])) - 0.5).abs() < 1e-5, "the rounded end");
    }
}
