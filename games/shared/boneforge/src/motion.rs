//! Robot-form motion: (clip, time, physical parameters, standing pose) ->
//! a pose, an airborne height, travel, and effects.
//!
//! A port of the viewer's headless motion core, with the same seam: the core
//! never owns a skeleton, it is handed one through [`Rig`] -- the solver it
//! measures against is always the rig's own. Every number is derived (from
//! gravity, leg length, mass and thrust) and reported in [`Out::info`], so a
//! caller can show the loop and steer it.
//!
//! Units: 1 unit ~ 1 cm (a 0.65 m wheel is ~64 units), so g = 981 u/s^2.
//!
//! Conventions were MEASURED on the rig, never assumed (the tests re-derive
//! them on boneforge's skeleton): the robot faces -x; thigh +z swings the knee
//! back; shin +z bends the knee (0 is straight); foot +z points the toes down;
//! spine +z leans forward, +y twists. Right mirrors left as (-x, -y, +z).

use crate::rig::{Joint, Pose};

/// The skeleton the core measures against: its own forward kinematics.
pub trait Rig {
    fn solve(&self, p: &Pose) -> Vec<Joint>;
    fn index(&self, name: &str) -> usize;
}

/// boneforge's own skeleton is a [`Rig`].
pub struct Skeleton(pub Vec<crate::rig::Bone>);

impl Rig for Skeleton {
    fn solve(&self, p: &Pose) -> Vec<Joint> { crate::rig::solve(&self.0, p) }
    fn index(&self, name: &str) -> usize {
        self.0.iter().position(|b| b.name == name).unwrap_or_else(|| panic!("no bone {name}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side { L, R }

impl Side {
    fn tag(self) -> &'static str { if self == Side::L { "_l" } else { "_r" } }
    /// outward along z: left is -z
    fn out(self) -> f32 { if self == Side::L { -1.0 } else { 1.0 } }
}

#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub g: f32,
    pub leg_len: f32,
    pub mass: f32,
    /// foot thrusters, as a multiple of the robot's weight AT 1 g
    pub thrust: f32,
    /// leg force, as a multiple of the robot's weight AT 1 g
    pub leg_force: f32,
    /// damage: a hyperextending knee on its stance (0..1) -- the limp, kept as
    /// a deliberate delta for a damage system to drive
    pub limp: (Side, f32),
    pub weapon: WeaponMode,
    /// walk (0) .. run (1): the Froude number, log-spaced 0.25..1.6 -- the
    /// gait's SHAPE. Gravity sets its speed.
    pub gait: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params { g: 981.0, leg_len: 185.0, mass: 1.0, thrust: 0.0, leg_force: 2.6, limp: (Side::L, 0.0), weapon: WeaponMode::Saw, gait: 0.3 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clip { Idle, Walk, Run, Locomote, Jump, Dash, Punch, Float, Glide, Cast, Weapon, Ranged }

/// What a weapon hand becomes: a wrist wheel spun up into a saw, forearm
/// cannons slid past the fist, or a staff extended from the hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeaponMode { Saw, Cannon, Staff }

#[derive(Debug, Clone)]
pub struct Out {
    pub pose: Pose,
    /// height above the grounded pose: a flight arc, a hop, a hover bob
    pub air: f32,
    /// distance travelled forward (-x)
    pub travel: f32,
    pub fx_feet: f32,
    pub fx_back: f32,
    /// how far the weapon hand has deployed (0 stowed .. 1), and the attack
    /// pulse -- a renderer slides the part, retracts the hand, spins or kicks
    pub weapon: f32,
    pub attack: f32,
    /// ranged: barrels deployed (0..1), shots in flight, recoil per source
    pub ranged: f32,
    pub shots: Vec<Shot>,
    pub kick: (f32, f32),
    pub info: String,
}

// ── the physics, on its own ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Gait { pub v: f32, pub step: f32, pub cadence: f32, pub hip_deg: f32, pub cycle: f32, pub duty: f32, pub flight: f32 }

/// Gait from the Froude number Fr = v^2/(gL): walking tops out near 0.5,
/// running sits around 1-3 for any legged animal.
///
/// NO-SLIP STANCE: while a foot is planted the body travels
/// v * duty * cycle = 2 * duty * step, and the foot sweeps 2 L sin(hip) under
/// it, so hip = asin(duty * step / L). (Matching the sweep to ONE step left
/// the planted foot sliding at 75% of walking speed.)
pub fn gait(p: &Params, froude: f32, duty: f32) -> Gait {
    let v = (froude * p.g * p.leg_len).sqrt();
    let step = p.leg_len * if froude < 0.5 { 0.75 } else { 1.25 };
    let cadence = v / step;
    let hip_deg = (duty * step / p.leg_len).min(0.9).asin().to_degrees();
    let cycle = 2.0 / cadence;
    Gait { v, step, cadence, hip_deg, cycle, duty, flight: (cycle * (0.5 - duty)).max(0.0) }
}

#[derive(Debug, Clone, Copy)]
pub struct Jump { pub apex: f32, pub v0: f32, pub t_up: f32, pub t_down: f32 }

impl Jump { pub fn airtime(&self) -> f32 { self.t_up + self.t_down } }

/// Pushing through a crouch of depth d with force F gives h = d (F/mg - 1).
/// Leg force and thrust are ABSOLUTE (quoted at 1 g): halve gravity and the
/// same legs jump higher. The legs set take-off speed under plain gravity;
/// foot thrusters soften the deceleration on the way UP only.
pub fn jump(p: &Params) -> Jump {
    let d = 0.35 * p.leg_len;
    let h0 = d * (p.leg_force * (981.0 / p.g) / p.mass - 1.0).max(0.05);
    let g_up = (p.g - p.thrust * 981.0 / p.mass).max(0.05 * p.g);
    let v0 = (2.0 * p.g * h0).sqrt();
    let apex = v0 * v0 / (2.0 * g_up);
    Jump { apex, v0, t_up: v0 / g_up, t_down: (2.0 * apex / p.g).sqrt() }
}

/// Tilting hover thrust by theta accelerates at g tan(theta) against drag k v.
pub fn glide_terminal(p: &Params, tilt_deg: f32, drag: f32) -> f32 {
    p.g * tilt_deg.to_radians().tan() / drag
}

// ── ranged fire ─────────────────────────────────────────────────────────

/// Where a shot comes from: a barrel out of the mounted part (a wheel's hub,
/// a cannon's or staff's tip), or a blaster out of the hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source { Part, Palm }

#[derive(Debug, Clone, Copy)]
pub struct Shot { pub src: Source, pub age: f32 }

/// First shot once deployed, then alternating part / hand every `EVERY`
/// seconds; a shot lives `LIFE` seconds. Slugs are ballistic, bolts straight.
pub const FIRST: f32 = 0.6;
pub const EVERY: f32 = 0.4;
pub const LIFE: f32 = 0.6;
pub const SLUG_V: f32 = 3500.0;
pub const BOLT_V: f32 = 4500.0;

/// The shots in flight at time `t` -- a pure function of time, so any frame
/// stands alone (no projectile state to carry).
pub fn shots(t: f32) -> Vec<Shot> {
    if t < FIRST { return vec![]; }
    let n = ((t - FIRST) / EVERY).floor() as i32;
    ((n - 3).max(0)..=n).filter_map(|k| {
        let age = t - (FIRST + k as f32 * EVERY);
        (age >= 0.0 && age < LIFE).then_some(Shot { src: if k % 2 == 1 { Source::Palm } else { Source::Part }, age })
    }).collect()
}

/// Where a shot is: from its muzzle, along the aim at its speed; a slug also
/// falls under gravity, a bolt does not.
pub fn shot_position(sh: &Shot, muzzle: [f32; 3], aim: [f32; 3], g: f32) -> [f32; 3] {
    let v = if sh.src == Source::Part { SLUG_V } else { BOLT_V };
    let drop = if sh.src == Source::Part { 0.5 * g * sh.age * sh.age } else { 0.0 };
    [muzzle[0] + aim[0] * v * sh.age, muzzle[1] + aim[1] * v * sh.age - drop, muzzle[2] + aim[2] * v * sh.age]
}

// ── inverse kinematics ──────────────────────────────────────────────────

/// Analytic two-bone IK in the leg's side-on (x, y) plane, always taking the
/// knee-FORWARD solution (a Newton solve flipped branch mid-stance). Angles
/// from straight down, +x (backward) positive -- the way thigh +z and shin +z
/// both rotate on this rig. Returns (thigh, bend) in degrees.
pub fn leg_angles(tx: f32, ty: f32, a: f32, b: f32) -> (f32, f32) {
    let d = (tx * tx + ty * ty).sqrt().min((a + b) * 0.999);
    let gam = ((a * a + b * b - d * d) / (2.0 * a * b)).clamp(-1.0, 1.0).acos();
    let alp = ((a * a + d * d - b * b) / (2.0 * a * d)).clamp(-1.0, 1.0).acos();
    let th = tx.atan2(-ty);
    ((th - alp).to_degrees(), (std::f32::consts::PI - gam).to_degrees())
}

/// The robot's facing, read off BOTH feet (the toes point forward; each foot
/// splays ~21 degrees, so one foot alone skews it). A robot that spins in its
/// transform ends facing elsewhere, and aiming, leg IK and travel all follow.
pub fn facing(rig: &dyn Rig, sol: &[Joint]) -> [f32; 3] {
    let (mut x, mut z) = (0.0, 0.0);
    for n in ["foot_l", "foot_r"] { let f = &sol[rig.index(n)]; x += f.tail[0] - f.head[0]; z += f.tail[2] - f.head[2]; }
    let l = (x * x + z * z).sqrt().max(1e-6);
    [x / l, 0.0, z / l]
}

fn add(p: &mut Pose, i: usize, d: [f32; 3]) {
    let a = &mut p.angles[i];
    a[0] += d[0]; a[1] += d[1]; a[2] += d[2];
}

fn both(rig: &dyn Rig, p: &mut Pose, base: &str, d: [f32; 3]) {
    add(p, rig.index(&format!("{base}_l")), d);
    add(p, rig.index(&format!("{base}_r")), [-d[0], -d[1], d[2]]);
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Move one ankle by (dx, dy) in the side-on plane, from its standing place.
/// Applied as CHANGES from the standing leg's own angles; the reach limit is
/// the STANDING leg's side-on length (its knee is straight, and a sideways
/// kink makes that shorter than thigh + shin); and a knee never bends past
/// straight (shin z = 0) -- asking for more is how the IK found a limp.
pub fn ik_leg(rig: &dyn Rig, p: &mut Pose, side: Side, dx: f32, dy: f32, s0: &[Joint]) {
    let fwd = facing(rig, s0);
    let (it, is, ia) = (rig.index(&format!("thigh{}", side.tag())), rig.index(&format!("shin{}", side.tag())),
                        rig.index(&format!("foot{}", side.tag())));
    let (h, k, an) = (s0[it].head, s0[is].head, s0[ia].head);
    let (a, b) = (dist(h, k), dist(k, an));
    // side-on in the robot's own frame: +s is BACKWARD, against its facing
    let (sx, sy) = (-((an[0] - h[0]) * fwd[0] + (an[2] - h[2]) * fwd[2]), an[1] - h[1]);
    let (tx, mut ty) = (sx + dx, sy + dy);
    let reach = (sx * sx + sy * sy).sqrt() * 0.995;
    if (tx * tx + ty * ty).sqrt() > reach { ty = -(reach * reach - tx * tx).max(1.0).sqrt(); }
    let (s_th, s_b) = leg_angles(sx, sy, a, b);
    let (g_th, g_b) = leg_angles(tx, ty, a, b);
    add(p, it, [0.0, 0.0, g_th - s_th]);
    add(p, is, [0.0, 0.0, g_b - s_b]);
    if p.angles[is][2] < 0.0 { p.angles[is][2] = 0.0; }
}

fn inv3(m: [[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let [a, b, c] = m[0]; let [d, e, f] = m[1]; let [g, h, i] = m[2];
    let (aa, bb, cc) = (e * i - f * h, -(d * i - f * g), d * h - e * g);
    let det = a * aa + b * bb + c * cc;
    if det.abs() < 1e-12 { return None; }
    Some([[aa / det, -(b * i - c * h) / det, (b * f - c * e) / det],
          [bb / det, (a * i - c * g) / det, -(a * f - c * d) / det],
          [cc / det, -(a * h - b * g) / det, (a * e - b * d) / det]])
}

/// ARMS ARE AIMED, not rotated about an axis: at the standing pose the upper
/// arms point well behind vertical and one axis lifts them mostly UP, so an
/// angle "swing" flailed (the zombie run). The motion says where the ELBOW
/// should be -- a direction from the shoulder, `fwd_deg` forward of straight
/// down and `outward` out to the side -- and a damped least-squares solve on
/// the three upper-arm angles gets it there. Twisting the arm about its own
/// length barely moves the elbow, so the damping matters.
pub fn aim_arm(rig: &dyn Rig, p: &mut Pose, side: Side, fwd_deg: f32, outward: f32) {
    let k = rig.index(&format!("uparm{}", side.tag()));
    let e_i = rig.index(&format!("forearm{}", side.tag()));
    let th = fwd_deg.to_radians();
    let f = facing(rig, &rig.solve(p));
    let right = [-f[2], 0.0, f[0]];                 // up x forward
    let o = side.out() * outward;
    let want = [f[0] * th.sin() + right[0] * o, -th.cos(), f[2] * th.sin() + right[2] * o];
    let wl = (want[0] * want[0] + want[1] * want[1] + want[2] * want[2]).sqrt();
    for _ in 0..6 {
        let s0 = rig.solve(p);
        let (sh, el) = (s0[k].head, s0[e_i].head);
        let l = dist(sh, el);
        let e = [sh[0] + want[0] / wl * l - el[0], sh[1] + want[1] / wl * l - el[1], sh[2] + want[2] / wl * l - el[2]];
        if (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt() < 0.8 { break; }
        let mut j = [[0.0f32; 3]; 3];              // j[ax] = elbow move per degree about axis ax
        for ax in 0..3 {
            let mut q = p.clone(); q.angles[k][ax] += 1.0;
            let ee = rig.solve(&q)[e_i].head;
            j[ax] = [ee[0] - el[0], ee[1] - el[1], ee[2] - el[2]];
        }
        let a = [[j[0][0], j[1][0], j[2][0]], [j[0][1], j[1][1], j[2][1]], [j[0][2], j[1][2], j[2][2]]];
        let lam = 0.08;
        let mut m = [[0.0f32; 3]; 3];
        for r in 0..3 { for c in 0..3 {
            m[r][c] = a[r][0] * a[c][0] + a[r][1] * a[c][1] + a[r][2] * a[c][2] + if r == c { lam } else { 0.0 };
        } }
        let Some(inv) = inv3(m) else { break };
        let y = [0, 1, 2].map(|r| inv[r][0] * e[0] + inv[r][1] * e[1] + inv[r][2] * e[2]);
        for ax in 0..3 {
            let d = (a[0][ax] * y[0] + a[1][ax] * y[1] + a[2][ax] * y[2]).clamp(-20.0, 20.0);
            p.angles[k][ax] += d;
        }
    }
}

fn arms(rig: &dyn Rig, p: &mut Pose, fwd: (f32, f32), bend: (f32, f32), outward: f32) {
    aim_arm(rig, p, Side::L, fwd.0, outward);
    aim_arm(rig, p, Side::R, fwd.1, outward);
    add(p, rig.index("forearm_l"), [0.0, 0.0, bend.0]);
    add(p, rig.index("forearm_r"), [0.0, 0.0, bend.1]);
}

fn sm(x: f32) -> f32 { let x = x.clamp(0.0, 1.0); x * x * (3.0 - 2.0 * x) }
const TAU: f32 = std::f32::consts::TAU;

/// FOOT IK: each ankle has a target held still in the world while planted and
/// carried on a lifted arc to its next landing.
fn legs(rig: &dyn Rig, p: &mut Pose, par: &Params, phase: f32, g: &Gait, lift: f32, comp: f32) {
    let s0 = rig.solve(p);
    let reach = g.duty * g.step;
    for (side, off) in [(Side::L, 0.0), (Side::R, 0.5)] {
        let c = (phase + off).rem_euclid(1.0);
        let (dx, dy) = if c < g.duty {
            let u = c / g.duty;                              // planted: sliding back at body speed
            (-reach * (1.0 - 2.0 * u), comp * (std::f32::consts::PI * u).sin())
        } else {
            let u = (c - g.duty) / (1.0 - g.duty);           // swing: eased forward, lifted
            (reach - 2.0 * reach * sm(u), lift * (std::f32::consts::PI * u).sin())
        };
        ik_leg(rig, p, side, dx, dy, &s0);
        if par.limp.1 > 0.0 && par.limp.0 == side && c < g.duty {
            add(p, rig.index(&format!("shin{}", side.tag())),
                [0.0, 0.0, -22.0 * par.limp.1 * (std::f32::consts::PI * c / g.duty).sin()]);
        }
    }
}

/// One frame of a clip. `stand` is the transformed robot's standing pose.
pub fn pose(clip: Clip, t: f32, stand: &Pose, rig: &dyn Rig, par: &Params) -> Out {
    let mut p = stand.clone();
    let (mut air, mut travel, mut feet, mut back) = (0.0, 0.0, 0.0, 0.0);
    let (mut weapon, mut attack) = (0.0, 0.0);
    let (mut ranged, mut in_flight, mut kick) = (0.0, Vec::new(), (0.0f32, 0.0f32));
    let spine = rig.index("spine");
    let info = match clip {
        Clip::Idle => {
            let b = (TAU * t / 3.2).sin();
            add(&mut p, spine, [0.0, 0.0, 1.2 * b]);
            arms(rig, &mut p, (6.0 + 2.0 * b, 6.0 + 2.0 * b), (18.0 + 3.0 * b, 18.0 + 3.0 * b), 0.14);
            "idle  breathing, 3.2 s".to_string()
        }
        Clip::Walk => {
            let g = gait(par, 0.25, 0.6); let ph = t / g.cycle; let sw = (TAU * ph).cos();
            add(&mut p, spine, [0.0, 0.0, 3.0]);
            arms(rig, &mut p, (6.0 - 16.0 * sw, 6.0 + 16.0 * sw), (22.0, 22.0), 0.14);
            legs(rig, &mut p, par, ph, &g, 0.12 * par.leg_len, 0.03 * par.leg_len);
            travel = g.v * t;
            format!("walk  Fr 0.25  {:.2} m/s  {:.2} steps/s", g.v / 100.0, g.cadence)
        }
        Clip::Run => {
            let g = gait(par, 1.6, 0.34); let ph = t / g.cycle; let sw = (TAU * ph).cos();
            add(&mut p, spine, [0.0, 0.0, 10.0]);
            arms(rig, &mut p, (12.0 - 38.0 * sw, 12.0 + 38.0 * sw), (85.0, 85.0), 0.10);
            legs(rig, &mut p, par, ph, &g, 0.28 * par.leg_len, 0.10 * par.leg_len);
            let in_step = (ph * 2.0).rem_euclid(1.0); let f0 = g.duty * 2.0;
            let fr = (in_step - f0) / (1.0 - f0);
            if fr > 0.0 { air = 0.5 * par.g * (fr * g.flight) * (g.flight - fr * g.flight); }
            travel = g.v * t;
            format!("run  Fr 1.6  {:.2} m/s  flight {:.0} ms/step", g.v / 100.0, g.flight * 1000.0)
        }
        // WALK <-> RUN, continuously: the slider is the gait's shape (Froude),
        // gravity its speed -- the same setting is a slower, floatier lope on
        // a low-gravity world.
        Clip::Locomote => {
            let u = par.gait.clamp(0.0, 1.0); let e = sm(u);
            let froude = 0.25 * (1.6f32 / 0.25).powf(u);
            let g = gait(par, froude, 0.6 + (0.34 - 0.6) * e);
            let ph = t / g.cycle; let sw = (TAU * ph).cos();
            add(&mut p, spine, [0.0, 0.0, 3.0 + 7.0 * e]);
            let (amp, base) = (16.0 + 22.0 * e, 6.0 + 6.0 * e);
            arms(rig, &mut p, (base - amp * sw, base + amp * sw), (22.0 + 63.0 * e, 22.0 + 63.0 * e), 0.14 - 0.04 * e);
            legs(rig, &mut p, par, ph, &g, (0.12 + 0.16 * e) * par.leg_len, (0.03 + 0.07 * e) * par.leg_len);
            let in_step = (ph * 2.0).rem_euclid(1.0); let f0 = g.duty * 2.0; let fr = (in_step - f0) / (1.0 - f0);
            if g.flight > 0.0 && fr > 0.0 { air = 0.5 * par.g * (fr * g.flight) * (g.flight - fr * g.flight); }
            travel = g.v * t;
            format!("locomote  Fr {froude:.2}  {:.2} m/s at {:.2} g", g.v / 100.0, par.g / 981.0)
        }
        Clip::Jump => {
            let j = jump(par); let tt = j.airtime();
            let (tc, tp, ta, tr) = (0.35, 0.12, 0.28, 0.45);
            let u = t.rem_euclid(tc + tp + tt + ta + tr + 0.6);
            let (mut crouch, mut tuck) = (0.0, 0.0);
            let seg = if u < tc { crouch = sm(u / tc); "crouch" }
                else if u < tc + tp { crouch = 1.0 - sm((u - tc) / tp); "push" }
                else if u < tc + tp + tt {
                    let a = u - tc - tp; let g_up = j.v0 / j.t_up;
                    air = if a < j.t_up { j.v0 * a - 0.5 * g_up * a * a } else { j.apex - 0.5 * par.g * (a - j.t_up).powi(2) };
                    tuck = (std::f32::consts::PI * a / tt).sin();
                    if a < j.t_up { feet = par.thrust; }
                    if a < j.t_up { "rise" } else { "fall" }
                }
                else if u < tc + tp + tt + ta { crouch = (std::f32::consts::PI * (u - tc - tp - tt) / ta).sin() * (j.v0 / 400.0).min(1.0); "land" }
                else { "recover" };
            both(rig, &mut p, "thigh", [0.0, 0.0, -38.0 * crouch - 30.0 * tuck]);
            both(rig, &mut p, "shin", [0.0, 0.0, 72.0 * crouch + 55.0 * tuck]);
            both(rig, &mut p, "foot", [0.0, 0.0, -30.0 * crouch]);
            add(&mut p, spine, [0.0, 0.0, 14.0 * crouch]);
            let a_f = 6.0 - 40.0 * crouch + 35.0 * tuck;
            arms(rig, &mut p, (a_f, a_f), (25.0 + 20.0 * crouch, 25.0 + 20.0 * crouch), 0.14);
            format!("jump  {seg}  apex {:.2} m  airtime {:.2} s", j.apex / 100.0, tt)
        }
        Clip::Dash => {
            let acc = 0.9 * 981.0 / par.mass; let (tl, tf, tb) = (0.25, 0.6, 0.5);
            let period = tl + tf + tb + 0.9; let u = t.rem_euclid(period); let n = (t / period).floor();
            let vmax = acc * tf; let per = 0.5 * acc * tf * tf + vmax * tb * 0.5;
            let (lean, x) = if u < tl { (sm(u / tl), 0.0) }
                else if u < tl + tf { let a = u - tl; back = 1.0; (1.0, 0.5 * acc * a * a) }
                else if u < tl + tf + tb { let a = u - tl - tf; (1.0 - 2.0 * sm(a / tb), 0.5 * acc * tf * tf + vmax * a - 0.5 * (vmax / tb) * a * a) }
                else { (-1.0 + sm((u - tl - tf - tb) / 0.9), per) };
            add(&mut p, spine, [0.0, 0.0, 22.0 * lean]);
            let a_d = 6.0 - 50.0 * lean.max(0.0);
            arms(rig, &mut p, (a_d, a_d), (25.0, 25.0), 0.14);
            both(rig, &mut p, "thigh", [0.0, 0.0, 14.0 * lean.max(0.0)]);
            both(rig, &mut p, "shin", [0.0, 0.0, 20.0 * lean.max(0.0)]);
            if back > 0.0 { air = 10.0; }
            travel = n * per + x;
            format!("dash  top {:.1} m/s", vmax / 100.0)
        }
        Clip::Punch => {
            let (tw, ts, tr) = (0.30, 0.12, 0.40); let u = t.rem_euclid(tw + ts + tr + 0.35);
            let k = if u < tw { -sm(u / tw) } else if u < tw + ts { -1.0 + 2.0 * sm((u - tw) / ts) }
                    else if u < tw + ts + tr { 1.0 - sm((u - tw - ts) / tr) } else { 0.0 };
            add(&mut p, spine, [0.0, -22.0 * k, 5.0]);
            // the striking elbow is AIMED: back in the wind-up, straight ahead at full extension
            let fwd_r = if k < 0.0 { 6.0 - 30.0 * (-k) } else { 6.0 + 84.0 * k };
            let bend_r = if k < 0.0 { 20.0 + 70.0 * (-k) } else { 20.0 * (1.0 - k) };
            arms(rig, &mut p, (45.0, fwd_r), (105.0, bend_r), 0.14);
            both(rig, &mut p, "thigh", [0.0, 0.0, -8.0]);
            both(rig, &mut p, "shin", [0.0, 0.0, 14.0]);
            "punch".to_string()
        }
        // WEAPON HAND: the arm rises to aim straight ahead while the hand
        // retracts and the mounted part deploys; then the attack loops.
        Clip::Weapon => {
            weapon = sm(t / 0.6);
            let k = if t < 0.6 { 0.0 } else { ((t - 0.6) % 1.2) / 1.2 };
            let (mut fwd_r, mut bend_r, mut out) = (6.0 + 84.0 * weapon, 5.0 + 60.0 * (1.0 - weapon), 0.14);
            if t >= 0.6 {
                match par.weapon {
                    WeaponMode::Saw => { let sw = (TAU * k).sin(); add(&mut p, spine, [0.0, 28.0 * sw, 4.0]); out = 0.14 + 0.35 * sw.max(0.0); attack = 1.0; }
                    WeaponMode::Cannon => { attack = (-((k - 0.1) / 0.04).powi(2)).exp(); fwd_r = 90.0 - 9.0 * attack; bend_r = 5.0 + 14.0 * attack; add(&mut p, spine, [0.0, 0.0, -4.0 * attack]); }
                    WeaponMode::Staff => { let r = (std::f32::consts::PI * (k / 0.5).min(1.0)).sin(); fwd_r = 90.0 + 25.0 * r; attack = (TAU * k).sin().max(0.0); }
                }
            }
            arms(rig, &mut p, (45.0, fwd_r), (105.0, bend_r), out);
            format!("weapon  {:?}  deployed {:.0}%", par.weapon, weapon * 100.0)
        }
        // RANGED: aim, pop the barrels, fire alternately part / hand.
        Clip::Ranged => {
            ranged = sm(t / 0.5);
            in_flight = shots(t);
            for s in &in_flight {
                let k = (-s.age / 0.06).exp();
                if s.src == Source::Part { kick.0 = kick.0.max(k) } else { kick.1 = kick.1.max(k) }
            }
            let k = kick.0.max(0.5 * kick.1);
            arms(rig, &mut p, (45.0, 6.0 + 84.0 * ranged - 7.0 * k), (105.0, 5.0 + 50.0 * (1.0 - ranged) + 10.0 * k), 0.14);
            add(&mut p, spine, [0.0, 0.0, -3.0 * k]);
            format!("ranged  {} in flight", in_flight.len())
        }
        // THE FLOATING MAGE: no floor to plant on, so the legs are posed.
        Clip::Float => {
            let b = (TAU * t / 2.4).sin(); let w = (TAU * t / 3.7).sin();
            add(&mut p, spine, [0.0, 2.0 * w, -2.0 + 1.5 * b]);
            arms(rig, &mut p, (52.0 + 6.0 * w, 52.0 - 6.0 * w), (38.0 + 8.0 * b, 38.0 - 8.0 * b), 0.55);
            feet = 0.28 + 0.06 * b; air = 3.5 * (1.0 + b);   // above its base: never into the floor
            "float  hover bob 7 u, period 2.4 s".to_string()
        }
        Clip::Glide => {
            let v = glide_terminal(par, 14.0, 4.5);
            add(&mut p, spine, [0.0, 0.0, 14.0]);
            both(rig, &mut p, "thigh", [0.0, 0.0, 16.0]);
            both(rig, &mut p, "shin", [0.0, 0.0, 10.0]);
            arms(rig, &mut p, (-28.0, -28.0), (30.0, 30.0), 0.35);
            feet = 0.35; back = 0.5; air = 1.5 * (1.0 + (TAU * t / 1.9).sin()); travel = v * t;
            format!("glide  thrust tilted 14 deg  {:.1} m/s terminal", v / 100.0)
        }
        Clip::Cast => {
            let (tg, tt, th, tr) = (0.45, 0.14, 0.35, 0.5); let u = t.rem_euclid(tg + tt + th + tr);
            let k = if u < tg { -sm(u / tg) } else if u < tg + tt { -1.0 + 2.0 * sm((u - tg) / tt) }
                    else if u < tg + tt + th { 1.0 } else { 1.0 - sm((u - tg - tt - th) / tr) };
            let fwd = if k < 0.0 { 40.0 - 15.0 * (-k) } else { 40.0 + 48.0 * k };
            let bend = if k < 0.0 { 40.0 + 55.0 * (-k) } else { 40.0 * (1.0 - k) };
            arms(rig, &mut p, (fwd, fwd), (bend, bend), if k > 0.0 { 0.12 } else { 0.35 });
            add(&mut p, spine, [0.0, 0.0, if k > 0.0 { -6.0 * k } else { 4.0 * (-k) }]);
            feet = 0.3 + 0.4 * k.max(0.0); travel = -8.0 * k.max(0.0);
            "cast".to_string()
        }
    };
    // travel along the robot's OWN facing (it was always -x)
    let f = facing(rig, &rig.solve(stand));
    p.root_pos[0] += f[0] * travel; p.root_pos[2] += f[2] * travel;
    Out { pose: p, air, travel, fx_feet: feet, fx_back: back, weapon, attack, ranged, shots: in_flight, kick, info }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot;

    fn rig() -> (Skeleton, Pose) {
        let bones = bot::skeleton(390.0);
        let stand = bot::stand(&bones, 390.0);
        (Skeleton(bones), stand)
    }

    #[test]
    fn froude_speed_and_no_slip_hip() {
        let p = Params::default();
        let w = gait(&p, 0.25, 0.6);
        assert!((w.v - (0.25f32 * 981.0 * 185.0).sqrt()).abs() < 1e-3);
        // the stance sweep equals the body's travel while planted
        let sweep = 2.0 * p.leg_len * w.hip_deg.to_radians().sin();
        assert!((sweep - 2.0 * w.duty * w.step).abs() < 0.5, "sweep {sweep} vs {}", 2.0 * w.duty * w.step);
    }

    #[test]
    fn jump_physics_matches_the_viewer() {
        let mut p = Params::default();
        let j = jump(&p);
        assert!((j.apex / 100.0 - 1.04).abs() < 0.01, "apex {}", j.apex);
        assert!((j.airtime() - 0.92).abs() < 0.01, "airtime {}", j.airtime());
        p.thrust = 0.4; assert!((jump(&p).apex / j.apex - 1.0 / 0.6).abs() < 0.01, "thrust raises apex by 1/(1-T)");
        p.thrust = 0.0; p.g = 981.0 * 0.5; assert!(jump(&p).apex > 2.5 * j.apex, "half gravity: the same legs jump higher");
        p.g = 981.0; p.mass = 2.0; assert!(jump(&p).apex < 0.25 * j.apex, "double mass barely leaves the ground");
    }

    #[test]
    fn leg_angles_reach_their_target() {
        let (a, b) = (95.0, 90.0);
        for (tx, ty) in [(0.0, -170.0), (-60.0, -150.0), (40.0, -140.0)] {
            let (th, bend) = leg_angles(tx, ty, a, b);
            let (t1, t2) = (th.to_radians(), (th + bend).to_radians());
            let (x, y) = (a * t1.sin() + b * t2.sin(), -a * t1.cos() - b * t2.cos());
            assert!((x - tx).abs() < 0.5 && (y - ty).abs() < 0.5, "({x},{y}) vs ({tx},{ty})");
            assert!(bend >= 0.0, "the knee bends forward");
        }
    }

    /// The conventions the core relies on, re-derived on boneforge's skeleton.
    #[test]
    fn measured_conventions_hold_on_this_rig() {
        let (r, stand) = rig();
        let s0 = r.solve(&stand);
        let (thigh, shin, foot) = (r.index("thigh_l"), r.index("shin_l"), r.index("foot_l"));
        let mut q = stand.clone(); q.angles[thigh][2] += 25.0;
        assert!(r.solve(&q)[shin].head[0] > s0[shin].head[0] + 10.0, "thigh +z swings the knee back (+x)");
        let mut q = stand.clone(); q.angles[shin][2] += 25.0;
        assert!(r.solve(&q)[foot].head[0] > s0[foot].head[0] + 10.0, "shin +z bends the knee (foot goes back)");
        let f = &s0[foot]; assert!(f.tail[0] < f.head[0], "the feet point -x: the robot faces -x");
    }

    #[test]
    fn walking_and_running_feet_stay_planted_and_knees_never_bend_back() {
        let (r, stand) = rig();
        let par = Params::default();
        for (clip, fr, duty) in [(Clip::Walk, 0.25, 0.6), (Clip::Run, 1.6, 0.34)] {
            let g = gait(&par, fr, duty);
            let (t0, t1) = (0.15 * duty * g.cycle, 0.85 * duty * g.cycle);   // inside the left stance
            let ank = |t: f32| r.solve(&pose(clip, t, &stand, &r, &par).pose)[r.index("foot_l")].head[0];
            let slip = (ank(t1) - ank(t0)).abs() / (g.v * (t1 - t0));
            assert!(slip < 0.08, "{clip:?}: planted foot slips {:.0}% of body travel", slip * 100.0);
            for k in 0..=60 {
                let s = r.solve(&pose(clip, k as f32 / 60.0 * g.cycle, &stand, &r, &par).pose);
                for sd in ["_l", "_r"] {
                    let (h, kn, a) = (s[r.index(&format!("thigh{sd}"))].head, s[r.index(&format!("shin{sd}"))].head, s[r.index(&format!("foot{sd}"))].head);
                    let d = [a[0] - h[0], a[1] - h[1], a[2] - h[2]];
                    let f = ((kn[0] - h[0]) * d[0] + (kn[1] - h[1]) * d[1] + (kn[2] - h[2]) * d[2]) / (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                    let off = -(kn[0] - (h[0] + d[0] * f));
                    assert!(off > -2.5, "{clip:?} {sd}: knee {off:.1} behind the leg line");
                }
            }
        }
    }

    #[test]
    fn the_limp_is_there_only_when_asked() {
        let (r, stand) = rig();
        let mut par = Params::default();
        let g = gait(&par, 1.6, 0.34);
        // the left knee's lowest bend angle over a run cycle: 0 is straight
        let knee = |par: &Params| -> f32 {
            (0..=60).map(|k| pose(Clip::Run, k as f32 / 60.0 * g.cycle, &stand, &r, par).pose.angles[r.index("shin_l")][2])
                .fold(f32::MAX, f32::min)
        };
        assert!(knee(&par) >= 0.0, "healthy: the knee never passes straight");
        par.limp = (Side::L, 1.0);
        assert!(knee(&par) < -10.0, "damaged: the knee hyperextends on its stance");
    }

    #[test]
    fn airborne_offsets_never_go_below_the_grounded_pose() {
        let (r, stand) = rig();
        let par = Params::default();
        for clip in [Clip::Idle, Clip::Walk, Clip::Run, Clip::Locomote, Clip::Jump, Clip::Dash, Clip::Punch, Clip::Float, Clip::Glide, Clip::Cast, Clip::Weapon, Clip::Ranged] {
            for k in 0..=80 { let o = pose(clip, k as f32 * 0.05, &stand, &r, &par); assert!(o.air >= -1e-3, "{clip:?} air {} at {}", o.air, k as f32 * 0.05); }
        }
    }

    #[test]
    fn a_weapon_hand_deploys_aims_and_attacks_by_its_kind() {
        let (r, stand) = rig();
        for mode in [WeaponMode::Saw, WeaponMode::Cannon, WeaponMode::Staff] {
            let par = Params { weapon: mode, ..Params::default() };
            let o0 = pose(Clip::Weapon, 0.0, &stand, &r, &par);
            let o1 = pose(Clip::Weapon, 0.62, &stand, &r, &par);
            assert!(o0.weapon < 0.01 && o1.weapon > 0.99, "{mode:?}: stowed at 0, deployed by 0.6 s");
            let s = r.solve(&o1.pose);
            let (a, b) = (s[r.index("uparm_r")].head, s[r.index("forearm_r")].head);
            let l = dist(a, b);
            assert!((b[0] - a[0]) / l < -0.85, "{mode:?}: the weapon arm aims straight ahead");
            // the attack pulses by kind: a saw runs continuously, a cannon kicks once a cycle
            // 10 ms steps: a cannon's kick is ~50 ms wide, and 50 ms sampling missed its peak
            let pulses: Vec<f32> = (0..120).map(|i| pose(Clip::Weapon, 0.6 + i as f32 * 0.01, &stand, &r, &par).attack).collect();
            let hi = pulses.iter().cloned().fold(0.0, f32::max);
            let lo = pulses.iter().cloned().fold(1.0, f32::min);
            match mode {
                WeaponMode::Saw => assert!(lo > 0.99, "a saw spins hard the whole time"),
                WeaponMode::Cannon => assert!(hi > 0.9 && lo < 0.05, "a cannon fires in pulses"),
                WeaponMode::Staff => assert!(hi > 0.9 && lo < 0.05, "a staff thrusts in pulses"),
            }
        }
    }

    #[test]
    fn ranged_fire_alternates_and_slugs_arc_while_bolts_fly_straight() {
        assert!(shots(0.5).is_empty(), "nothing before deployment");
        let s = shots(0.61); assert_eq!(s.len(), 1); assert_eq!(s[0].src, Source::Part);
        let s = shots(1.05); assert_eq!(s.len(), 2, "a slug and a bolt in flight");
        assert!(s.iter().any(|x| x.src == Source::Palm) && s.iter().any(|x| x.src == Source::Part), "alternating sources");
        for t in [0.7, 1.3, 2.9, 7.7] { assert!(shots(t).iter().all(|x| x.age >= 0.0 && x.age < LIFE)); }
        // kinematics: level aim, from the origin
        let aim = [-1.0, 0.0, 0.0];
        let slug = shot_position(&Shot { src: Source::Part, age: 0.5 }, [0.0; 3], aim, 981.0);
        let bolt = shot_position(&Shot { src: Source::Palm, age: 0.5 }, [0.0; 3], aim, 981.0);
        assert!((slug[0] + SLUG_V * 0.5).abs() < 0.01 && (slug[1] + 0.5 * 981.0 * 0.25).abs() < 0.01, "a slug drops 1/2 g t^2");
        assert!((bolt[1]).abs() < 1e-6 && (bolt[0] + BOLT_V * 0.5).abs() < 0.01, "a bolt flies straight");
        let low_g = shot_position(&Shot { src: Source::Part, age: 0.5 }, [0.0; 3], aim, 981.0 * 0.5);
        assert!(low_g[1] > slug[1], "half gravity: the slug drops less");
    }

    #[test]
    fn ranged_holds_the_arm_on_aim() {
        let (r, stand) = rig();
        let o = pose(Clip::Ranged, 1.3, &stand, &r, &Params::default());
        assert!(o.ranged > 0.99);
        let s = r.solve(&o.pose);
        let (a, b) = (s[r.index("uparm_r")].head, s[r.index("forearm_r")].head);
        assert!((b[0] - a[0]) / dist(a, b) < -0.85, "the firing arm aims straight ahead");
    }

    /// A robot that spun half a turn in its transform: aiming, planted feet and
    /// travel all work in ITS frame.
    #[test]
    fn everything_follows_the_robots_own_facing() {
        let (r, mut stand) = rig();
        let par = Params::default();
        let f0 = facing(&r, &r.solve(&stand));
        assert!(f0[0] < -0.99, "unturned, it faces -x: {f0:?}");
        stand.root_rot[1] += 180.0;
        let f = facing(&r, &r.solve(&stand));
        assert!(f[0] > 0.99, "turned, it faces +x: {f:?}");
        // the punch lands along the new facing
        let s = r.solve(&pose(Clip::Punch, 0.42, &stand, &r, &par).pose);
        let (a, b) = (s[r.index("uparm_r")].head, s[r.index("forearm_r")].head);
        assert!((b[0] - a[0]) / dist(a, b) > 0.9, "punches along its facing");
        // walking carries it along its facing, feet still planted
        let o = pose(Clip::Walk, 1.0, &stand, &r, &par);
        assert!(o.pose.root_pos[0] > stand.root_pos[0] + 100.0, "walks forward (+x)");
        let g = gait(&par, 0.25, 0.6); let (t0, t1) = (0.15 * 0.6 * g.cycle, 0.85 * 0.6 * g.cycle);
        let ank = |t: f32| r.solve(&pose(Clip::Walk, t, &stand, &r, &par).pose)[r.index("foot_l")].head[0];
        assert!((ank(t1) - ank(t0)).abs() / (g.v * (t1 - t0)) < 0.08, "planted foot still planted, turned");
    }

    #[test]
    fn the_gait_slider_runs_walk_to_run_and_gravity_sets_the_speed() {
        let (r, stand) = rig();
        let speed = |u: f32, gm: f32| { let par = Params { gait: u, g: 981.0 * gm, ..Params::default() };
            let o = pose(Clip::Locomote, 1.0, &stand, &r, &par); o.travel };
        assert!(speed(0.0, 1.0) < speed(0.5, 1.0) && speed(0.5, 1.0) < speed(1.0, 1.0), "slider: walk -> jog -> run");
        let ratio = speed(1.0, 0.17) / speed(1.0, 1.0);
        assert!((ratio - 0.17f32.sqrt()).abs() < 0.01, "same gait at 0.17 g is sqrt(0.17) as fast: {ratio}");
        let (earth, moon) = (gait(&Params::default(), 1.6, 0.34), gait(&Params { g: 981.0 * 0.17, ..Params::default() }, 1.6, 0.34));
        assert!(moon.flight > 2.0 * earth.flight, "a longer flight phase on the Moon");
    }

    #[test]
    fn a_punch_ends_straight_ahead_and_the_mage_casts_forward() {
        let (r, stand) = rig();
        let par = Params::default();
        let elbow_dir = |clip, t, side: &str| {
            let s = r.solve(&pose(clip, t, &stand, &r, &par).pose);
            let (a, b) = (s[r.index(&format!("uparm{side}"))].head, s[r.index(&format!("forearm{side}"))].head);
            let l = dist(a, b); [(b[0] - a[0]) / l, (b[1] - a[1]) / l, (b[2] - a[2]) / l]
        };
        let d = elbow_dir(Clip::Punch, 0.42, "_r");
        assert!(d[0] < -0.9 && d[1].abs() < 0.3, "punch: elbow straight ahead, got {d:?}");
        let d = elbow_dir(Clip::Cast, 0.75, "_l");
        assert!(d[0] < -0.7, "cast: arms thrust forward, got {d:?}");
        let d = elbow_dir(Clip::Run, 0.0, "_l");
        assert!(d[1] < -0.8, "running arms hang and pump, not flail up: {d:?}");
    }
}
