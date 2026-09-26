//! How a weapon moves in the hand: per-class move sets with a real wind-up, a
//! strike moment and a follow-through, strung into combos.
//!
//! Until this module every weapon played one 0.24 s arc and its hit landed on
//! the click, so a greataxe and a dagger felt the same and nothing could read
//! an attack coming. Here a swing is a small timeline:
//!
//! ```text
//!  rest ──ease──▶ cocked ──snap──▶ STRIKE ──ease──▶ follow-through ──▶ rest
//!  0              ¾·strike_at      strike_at        …                  1
//! ```
//!
//! The blow lands at `strike_at` (the caller tests for a hit then, along the
//! view at that moment, so you can still track a target during the wind-up).
//! Heavy classes strike later in a longer swing: the wind-up is the telegraph
//! a dragon (or a player) can read, and the commitment is the cost.
//!
//! One timeline, two projections: [`ArmPose`] for the third-person rig
//! (`hero_forge`) and [`ViewDelta`] for the first-person viewmodel. Both come
//! from the same keyframes, so the body and the hand can never disagree.
//!
//! Pure: no clock (time is an argument), no RNG, no allocation, no panics on
//! NaN or negative `dt`.

use crate::sim::legendary_weapon::WeaponClass;

/// The shape of one attack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// Forehand horizontal cut, right to left.
    SlashR,
    /// Backhand cut, left to right.
    SlashL,
    /// Overhead chop (axes).
    Chop,
    /// Straight stab (daggers, spears, fists).
    Thrust,
    /// Wide two-handed sweep (greatswords, staves).
    Sweep,
    /// Short wrist flick (wands).
    Flick,
    /// Over-the-shoulder throw; the strike moment is the release.
    Throw,
    /// A returning weapon slapping back into the palm.
    Catch,
    /// A quick work swing (mining, chopping a block): no combat strike.
    Work,
}

/// Additive deltas for the right arm and torso of the hero rig (radians), in
/// `hero_forge`'s convention: shoulder pitch negative raises the arm forward
/// and up (−2.6 is overhead), shoulder roll negative swings it out to the
/// right, elbow negative flexes, chest twist positive turns the torso left,
/// chest lean negative folds forward.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ArmPose {
    pub sh_pitch: f32,
    pub sh_yaw: f32,
    pub sh_roll: f32,
    pub elbow: f32,
    pub chest_twist: f32,
    pub chest_lean: f32,
    /// The grip: the item's tilt in the hand about the side axis. 0 is the
    /// rig's carry (the item upright along the forearm, as a hanging hand
    /// holds a sword); positive tips it forward over the knuckles.
    pub wrist: f32,
}

/// Additive deltas for the first-person viewmodel, in its view space (+X
/// right, +Y up, −Z forward; pitch about X, yaw about Y, roll about Z).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewDelta {
    pub offset: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    arm: ArmPose,
    view: ViewDelta,
}

const fn arm(sh_pitch: f32, sh_yaw: f32, sh_roll: f32, elbow: f32, chest_twist: f32, chest_lean: f32) -> ArmPose {
    ArmPose { sh_pitch, sh_yaw, sh_roll, elbow, chest_twist, chest_lean, wrist: 0.0 }
}

/// The grip at (cocked, strike, follow-through) for each [`Motion`], in
/// declaration order: see [`ArmPose::wrist`]. Solved offline against a target
/// direction for the item at each key (behind the head when cocked for a chop,
/// out ahead at the blow, across the body after a slash), each key within reach
/// of the one before so the grip never spins the long way round. Checked by
/// `weapon_motion_the_item_points_where_the_move_needs_it`.
const WRISTS: [[f32; 3]; 9] = [
    [-2.40, -3.11, -2.91], // SlashR
    [2.40, 3.14, 3.29],    // SlashL
    [1.75, 3.01, 3.29],    // Chop
    [-2.40, -3.14, -3.06], // Thrust
    [-2.40, -3.29, -2.92], // Sweep
    [2.40, 2.77, 2.97],    // Flick
    [2.40, 2.44, 3.11],    // Throw
    [2.33, 2.19, 0.80],    // Catch
    [2.37, 3.29, 3.29],    // Work
];

fn wrists(m: Motion) -> [f32; 3] {
    WRISTS[m as usize]
}

const fn view(x: f32, y: f32, z: f32, pitch: f32, yaw: f32, roll: f32) -> ViewDelta {
    ViewDelta { offset: [x, y, z], pitch, yaw, roll }
}

/// (cocked, strike, follow-through) keys of a motion.
fn keys(m: Motion) -> [Key; 3] {
    let k = |a: ArmPose, v: ViewDelta| Key { arm: a, view: v };
    match m {
        Motion::Chop => [
            k(arm(-2.75, 0.0, -0.15, -1.50, -0.10, 0.18), view(0.02, 0.16, 0.06, -1.00, 0.0, 0.05)),
            k(arm(-1.00, 0.0, 0.05, -0.15, 0.20, -0.28), view(-0.06, -0.04, -0.14, 0.60, 0.0, 0.10)),
            k(arm(-0.45, 0.0, 0.10, -0.35, 0.12, -0.18), view(-0.04, -0.10, -0.08, 0.95, 0.0, 0.08)),
        ],
        Motion::SlashR => [
            k(arm(-1.35, 0.35, -1.10, -1.00, -0.55, 0.0), view(0.18, 0.06, 0.04, 0.10, 0.70, -0.55)),
            k(arm(-1.40, -0.30, 0.25, -0.20, 0.45, -0.12), view(-0.20, -0.02, -0.12, 0.20, -0.80, 0.45)),
            k(arm(-1.10, -0.55, 0.45, -0.45, 0.60, -0.08), view(-0.30, -0.06, -0.06, 0.25, -1.05, 0.55)),
        ],
        Motion::SlashL => [
            k(arm(-1.30, -0.45, 0.40, -1.35, 0.45, 0.0), view(-0.22, 0.04, 0.02, 0.10, -0.80, 0.60)),
            k(arm(-1.40, 0.30, -0.80, -0.20, -0.40, -0.12), view(0.16, -0.02, -0.12, 0.20, 0.75, -0.40)),
            k(arm(-1.05, 0.45, -1.05, -0.40, -0.55, -0.06), view(0.24, -0.06, -0.05, 0.25, 1.00, -0.50)),
        ],
        Motion::Thrust => [
            k(arm(-0.55, 0.0, -0.20, -1.70, -0.35, 0.10), view(0.04, -0.02, 0.14, -0.15, 0.0, 0.0)),
            k(arm(-1.55, 0.0, 0.05, -0.02, 0.25, -0.22), view(-0.06, 0.04, -0.30, 0.05, -0.05, 0.0)),
            k(arm(-1.45, 0.0, 0.0, -0.20, 0.20, -0.18), view(-0.05, 0.02, -0.24, 0.10, -0.05, 0.0)),
        ],
        Motion::Sweep => [
            k(arm(-1.10, 0.50, -1.30, -0.70, -0.80, 0.08), view(0.26, 0.02, 0.06, 0.25, 0.95, -0.75)),
            k(arm(-1.25, -0.40, 0.40, -0.15, 0.70, -0.20), view(-0.26, -0.06, -0.10, 0.30, -0.95, 0.60)),
            k(arm(-0.90, -0.70, 0.60, -0.40, 0.90, -0.12), view(-0.38, -0.10, -0.02, 0.35, -1.30, 0.70)),
        ],
        Motion::Flick => [
            k(arm(-1.90, 0.0, -0.10, -1.30, -0.10, 0.0), view(0.02, 0.08, 0.05, -0.55, 0.0, 0.0)),
            k(arm(-1.25, 0.0, 0.05, -0.15, 0.10, -0.06), view(-0.02, 0.00, -0.12, 0.55, 0.0, 0.05)),
            k(arm(-1.10, 0.0, 0.05, -0.30, 0.08, -0.04), view(-0.02, -0.02, -0.08, 0.45, 0.0, 0.05)),
        ],
        Motion::Throw => [
            k(arm(-2.90, 0.25, -0.35, -1.85, -0.55, 0.22), view(0.10, 0.18, 0.06, -1.25, 0.25, -0.20)),
            k(arm(-1.20, -0.10, 0.10, -0.10, 0.45, -0.30), view(-0.06, 0.02, -0.36, 0.70, -0.10, 0.10)),
            k(arm(-0.70, -0.30, 0.20, -0.20, 0.55, -0.25), view(-0.10, -0.20, -0.20, 1.20, -0.20, 0.10)),
        ],
        Motion::Catch => [
            // Starts reaching (the Reach pose) and absorbs the weight.
            k(arm(-1.50, 0.0, -0.05, -0.05, 0.10, -0.08), view(-0.02, 0.06, -0.14, 0.10, 0.0, 0.0)),
            k(arm(-1.05, 0.0, -0.05, -0.95, -0.15, 0.10), view(0.02, -0.08, 0.10, -0.35, 0.0, 0.12)),
            k(arm(-0.40, 0.0, 0.0, -0.40, -0.05, 0.04), view(0.01, -0.03, 0.04, -0.10, 0.0, 0.04)),
        ],
        Motion::Work => [
            k(arm(-1.90, 0.0, -0.10, -1.00, -0.05, 0.05), view(0.0, 0.08, 0.04, -0.45, 0.0, 0.0)),
            k(arm(-1.00, 0.0, 0.0, -0.40, 0.10, -0.12), view(0.0, -0.10, -0.06, 0.95, 0.0, 0.15)),
            k(arm(-0.60, 0.0, 0.0, -0.40, 0.05, -0.06), view(0.0, -0.08, -0.04, 0.70, 0.0, 0.10)),
        ],
    }
}

/// The pose held while recalling a weapon: arm out, palm open toward it.
pub const REACH_ARM: ArmPose = ArmPose { wrist: 1.3, ..arm(-1.50, 0.0, -0.05, -0.05, 0.10, -0.08) };
/// First-person counterpart of [`REACH_ARM`].
pub const REACH_VIEW: ViewDelta = view(-0.02, 0.06, -0.14, 0.10, 0.0, 0.0);

/// The attacks a class strings together, in combo order. `None` = bare hands.
pub fn combo(class: Option<WeaponClass>) -> &'static [Motion] {
    match class {
        None => &[Motion::Thrust, Motion::Thrust, Motion::SlashR],
        Some(WeaponClass::Dagger) => &[Motion::Thrust, Motion::SlashR, Motion::Thrust],
        Some(WeaponClass::Blade) => &[Motion::SlashR, Motion::SlashL, Motion::Thrust],
        Some(WeaponClass::Greatsword) => &[Motion::Sweep, Motion::SlashL, Motion::Chop],
        Some(WeaponClass::Axe) => &[Motion::Chop, Motion::SlashR],
        Some(WeaponClass::Greataxe) => &[Motion::Chop, Motion::Sweep],
        Some(WeaponClass::Spear) => &[Motion::Thrust, Motion::Thrust, Motion::Sweep],
        Some(WeaponClass::Staff) => &[Motion::Sweep, Motion::Chop],
        Some(WeaponClass::Wand) => &[Motion::Flick],
    }
}

/// Fraction of the swing at which the blow lands: later for heavier classes.
pub fn strike_at(class: Option<WeaponClass>, m: Motion) -> f32 {
    match m {
        Motion::Throw => return 0.50,
        Motion::Catch => return 0.0,
        Motion::Work => return 0.45,
        _ => {}
    }
    match class {
        None => 0.35,
        Some(WeaponClass::Dagger) => 0.35,
        Some(WeaponClass::Wand) => 0.30,
        Some(WeaponClass::Blade) => 0.45,
        Some(WeaponClass::Spear) => 0.42,
        Some(WeaponClass::Staff) => 0.45,
        Some(WeaponClass::Axe) => 0.50,
        Some(WeaponClass::Greatsword) => 0.55,
        Some(WeaponClass::Greataxe) => 0.60,
    }
}

/// Seconds one swing takes, from the weapon's `swing_time` (legendary_weapon's
/// archetype stat, 0.3 s for a wand .. 2.2 s for a greataxe). Bare hands and a
/// missing stat get a jab's 0.35 s.
pub fn duration(class: Option<WeaponClass>, swing_time: f32, m: Motion) -> f32 {
    match m {
        Motion::Catch => return 0.32,
        Motion::Work => return 0.26,
        Motion::Throw => {
            let heavy = matches!(class, Some(WeaponClass::Greataxe | WeaponClass::Greatsword | WeaponClass::Spear));
            return if heavy { 0.75 } else { 0.55 };
        }
        _ => {}
    }
    if class.is_none() || !swing_time.is_finite() {
        return 0.35;
    }
    (swing_time * 0.8).clamp(0.28, 1.8)
}

/// Seconds after a swing ends during which the next click continues the combo.
pub const COMBO_WINDOW: f32 = 0.45;

/// What a tick of a swing produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Before the blow.
    Winding,
    /// The blow lands this tick (exactly once per swing).
    Strike,
    /// After the blow.
    Recovering,
    /// The swing is over.
    Done,
}

/// One swing in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swing {
    pub class: Option<WeaponClass>,
    pub motion: Motion,
    /// Seconds elapsed.
    pub t: f32,
    pub dur: f32,
    /// Fraction of `dur` at which the blow lands.
    pub strike_at: f32,
    pub struck: bool,
    /// Position in the class's combo.
    pub step: u8,
}

impl Swing {
    /// A combat swing: combo step `step` of `class`'s move set.
    pub fn attack(class: Option<WeaponClass>, swing_time: f32, step: u8) -> Swing {
        let set = combo(class);
        let motion = set[step as usize % set.len()];
        Swing::of(class, motion, swing_time, step % set.len() as u8)
    }

    /// Any single motion (a throw, a catch, a work swing).
    pub fn of(class: Option<WeaponClass>, motion: Motion, swing_time: f32, step: u8) -> Swing {
        Swing {
            class,
            motion,
            t: 0.0,
            dur: duration(class, swing_time, motion),
            strike_at: strike_at(class, motion),
            struck: false,
            step,
        }
    }

    /// Advance by `dt` seconds. Reports [`Tick::Strike`] exactly once, on the
    /// tick that crosses the strike moment, however coarse `dt` is (a single
    /// huge step that crosses both the strike and the end still strikes).
    pub fn tick(&mut self, dt: f32) -> Tick {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.t += dt;
        if !self.struck && self.t >= self.strike_at * self.dur {
            self.struck = true;
            return Tick::Strike;
        }
        if self.t >= self.dur {
            Tick::Done
        } else if self.struck {
            Tick::Recovering
        } else {
            Tick::Winding
        }
    }

    pub fn done(&self) -> bool {
        self.struck && self.t >= self.dur
    }

    /// 0..1 through the swing.
    pub fn progress(&self) -> f32 {
        if self.dur > 0.0 {
            (self.t / self.dur).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Seconds until the blow lands (0 once it has).
    pub fn strike_in(&self) -> f32 {
        (self.strike_at * self.dur - self.t).max(0.0)
    }

    pub fn arm(&self) -> ArmPose {
        arm_at(self.motion, self.strike_at, self.progress())
    }

    pub fn view(&self) -> ViewDelta {
        view_at(self.motion, self.strike_at, self.progress())
    }
}

/// Chooses the next combo step: continue if the last swing of the same class
/// ended less than [`COMBO_WINDOW`] ago, else start over.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Combo {
    last: Option<(Option<WeaponClass>, u8)>,
    since: f32,
}

impl Combo {
    /// Time passes with no swing in progress.
    pub fn idle(&mut self, dt: f32) {
        if dt.is_finite() && dt > 0.0 {
            self.since += dt;
        }
    }

    /// The step for a new attack with `class`, recorded as the latest.
    pub fn next(&mut self, class: Option<WeaponClass>) -> u8 {
        let len = combo(class).len() as u8;
        let step = match self.last {
            Some((c, s)) if c == class && self.since <= COMBO_WINDOW => (s + 1) % len,
            _ => 0,
        };
        self.last = Some((class, step));
        self.since = 0.0;
        step
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

// ── interpolation ─────────────────────────────────────────────────────────

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// (segment start, end, weight shape) → blend weight of the later key.
fn blend(u: f32, strike: f32) -> (usize, usize, f32) {
    let s = if strike.is_finite() { strike.clamp(0.0, 0.95) } else { 0.5 };
    let cock = s * 0.75;
    let follow = s + (1.0 - s) * 0.35;
    // Key indices: 0 rest, 1 cocked, 2 strike, 3 follow, 4 rest.
    if s <= 0.0 {
        // No wind-up (a catch): starts at the "cocked" key and settles.
        return if u < 0.35 {
            (1, 2, smooth(u / 0.35))
        } else if u < 0.7 {
            (2, 3, smooth((u - 0.35) / 0.35))
        } else {
            (3, 4, smooth((u - 0.7) / 0.3))
        };
    }
    if u < cock {
        (0, 1, smooth(u / cock))
    } else if u < s {
        // The snap: accelerate into the blow.
        let x = ((u - cock) / (s - cock).max(1e-4)).clamp(0.0, 1.0);
        (1, 2, x * x)
    } else if u < follow {
        let x = ((u - s) / (follow - s).max(1e-4)).clamp(0.0, 1.0);
        (2, 3, 1.0 - (1.0 - x) * (1.0 - x))
    } else {
        (3, 4, smooth((u - follow) / (1.0 - follow).max(1e-4)))
    }
}

fn lerp_arm(a: ArmPose, b: ArmPose, w: f32) -> ArmPose {
    let l = |x: f32, y: f32| x + (y - x) * w;
    ArmPose {
        sh_pitch: l(a.sh_pitch, b.sh_pitch),
        sh_yaw: l(a.sh_yaw, b.sh_yaw),
        sh_roll: l(a.sh_roll, b.sh_roll),
        elbow: l(a.elbow, b.elbow),
        chest_twist: l(a.chest_twist, b.chest_twist),
        chest_lean: l(a.chest_lean, b.chest_lean),
        wrist: l(a.wrist, b.wrist),
    }
}

fn lerp_view(a: ViewDelta, b: ViewDelta, w: f32) -> ViewDelta {
    let l = |x: f32, y: f32| x + (y - x) * w;
    ViewDelta {
        offset: [l(a.offset[0], b.offset[0]), l(a.offset[1], b.offset[1]), l(a.offset[2], b.offset[2])],
        pitch: l(a.pitch, b.pitch),
        yaw: l(a.yaw, b.yaw),
        roll: l(a.roll, b.roll),
    }
}

fn key_list(m: Motion) -> [Key; 5] {
    let rest = Key { arm: ArmPose::default(), view: ViewDelta::default() };
    let [mut c, mut s, mut f] = keys(m);
    let w = wrists(m);
    c.arm.wrist = w[0];
    s.arm.wrist = w[1];
    f.arm.wrist = w[2];
    [rest, c, s, f, rest]
}

/// The arm pose `u` (0..1) through motion `m` striking at `strike`.
pub fn arm_at(m: Motion, strike: f32, u: f32) -> ArmPose {
    let u = if u.is_finite() { u.clamp(0.0, 1.0) } else { 0.0 };
    let k = key_list(m);
    let (a, b, w) = blend(u, strike);
    lerp_arm(k[a].arm, k[b].arm, w)
}

/// The first-person delta `u` (0..1) through motion `m` striking at `strike`.
pub fn view_at(m: Motion, strike: f32, u: f32) -> ViewDelta {
    let u = if u.is_finite() { u.clamp(0.0, 1.0) } else { 0.0 };
    let k = key_list(m);
    let (a, b, w) = blend(u, strike);
    lerp_view(k[a].view, k[b].view, w)
}

/// Blend a pose toward another by `w` (0..1), for layering the recall reach.
pub fn mix_arm(a: ArmPose, b: ArmPose, w: f32) -> ArmPose {
    lerp_arm(a, b, if w.is_finite() { w.clamp(0.0, 1.0) } else { 0.0 })
}

pub fn mix_view(a: ViewDelta, b: ViewDelta, w: f32) -> ViewDelta {
    lerp_view(a, b, if w.is_finite() { w.clamp(0.0, 1.0) } else { 0.0 })
}

// ── a generic arm, for bodies without a rig ─────────────────────────────

/// The right hand's place and the held item's orientation for `arm`, on a
/// simple two-segment arm hanging from a shoulder. For bodies that have no
/// arm of their own to pose (`hero_forge` bodies carry the item on their real
/// rig; a blueprint avatar does not), so the item still moves through the
/// same wind-up, blow and follow-through.
///
/// Body frame: `+X` forward, `+Y` up, `+Z` the right side. Returns the hand
/// position relative to the shoulder and the item axes as columns
/// (`item_model`'s frame: +X through the item, +Y along it, +Z the edge).
pub fn hand_frame(a: &ArmPose, upper_len: f32, fore_len: f32) -> ([f32; 3], [[f32; 3]; 3]) {
    let f = |x: f32| if x.is_finite() { x } else { 0.0 };
    // Raising forward is a rotation about the right-side axis (+Z) taking
    // "down" toward "forward"; the rig's negative pitch raises.
    let rz = |v: [f32; 3], t: f32| {
        let (s, c) = t.sin_cos();
        [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]]
    };
    let rx = |v: [f32; 3], t: f32| {
        let (s, c) = t.sin_cos();
        [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
    };
    let ry = |v: [f32; 3], t: f32| {
        let (s, c) = t.sin_cos();
        [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
    };
    // Shoulder: out to the side first (negative roll = right, +Z), then
    // raised forward, then yawed; the torso's twist turns the whole arm about
    // the spine. (Rolling after the raise would spin a forward-pointing arm
    // about itself and never move it sideways.)
    let arm = |v: [f32; 3]| ry(rz(rx(v, f(a.sh_roll)), -f(a.sh_pitch)), f(a.sh_yaw) + f(a.chest_twist));
    // The elbow flexes the forearm further the way the shoulder raises.
    let fore = |v: [f32; 3]| arm(rz(v, -f(a.elbow)));
    let down = [0.0, -1.0, 0.0];
    let elbow = arm(down);
    let wrist = fore(down);
    let hand = [
        elbow[0] * upper_len + wrist[0] * fore_len,
        elbow[1] * upper_len + wrist[1] * fore_len,
        elbow[2] * upper_len + wrist[2] * fore_len,
    ];
    // The grip: upright along the forearm at 0 (the rig's carry), tipped
    // forward over the knuckles by `wrist`; the edge faces forward at rest.
    let grip = |v: [f32; 3]| fore(rz(v, -f(a.wrist)));
    let along = grip([0.0, 1.0, 0.0]);
    let edge = grip([1.0, 0.0, 0.0]);
    let through = [
        along[1] * edge[2] - along[2] * edge[1],
        along[2] * edge[0] - along[0] * edge[2],
        along[0] * edge[1] - along[1] * edge[0],
    ];
    (hand, [through, along, edge])
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOTIONS: [Motion; 9] = [
        Motion::SlashR, Motion::SlashL, Motion::Chop, Motion::Thrust, Motion::Sweep,
        Motion::Flick, Motion::Throw, Motion::Catch, Motion::Work,
    ];

    fn mag(a: ArmPose) -> f32 {
        a.sh_pitch.abs() + a.sh_yaw.abs() + a.sh_roll.abs() + a.elbow.abs() + a.chest_twist.abs() + a.chest_lean.abs()
            + a.wrist.abs()
    }

    #[test]
    fn weapon_motion_every_motion_ends_at_rest() {
        for m in MOTIONS {
            let s = strike_at(Some(WeaponClass::Blade), m);
            assert!(mag(arm_at(m, s, 1.0)) < 1e-4, "{m:?} ends at rest");
            let v = view_at(m, s, 1.0);
            assert!(v.pitch.abs() + v.offset.iter().map(|x| x.abs()).sum::<f32>() < 1e-4);
            if m != Motion::Catch {
                assert!(mag(arm_at(m, s, 0.0)) < 1e-4, "{m:?} starts at rest");
            }
        }
    }

    #[test]
    fn weapon_motion_poses_are_continuous() {
        // No pops: a 1/1000 step never moves the joints more than 0.15 rad in
        // sum (the snap into the blow is the fastest part, by design).
        for m in MOTIONS {
            let s = strike_at(Some(WeaponClass::Axe), m);
            let mut prev = arm_at(m, s, 0.0);
            for i in 1..=1000 {
                let a = arm_at(m, s, i as f32 / 1000.0);
                let d = mag(ArmPose {
                    sh_pitch: a.sh_pitch - prev.sh_pitch,
                    sh_yaw: a.sh_yaw - prev.sh_yaw,
                    sh_roll: a.sh_roll - prev.sh_roll,
                    elbow: a.elbow - prev.elbow,
                    chest_twist: a.chest_twist - prev.chest_twist,
                    chest_lean: a.chest_lean - prev.chest_lean,
                    wrist: a.wrist - prev.wrist,
                });
                assert!(d < 0.15, "{m:?} jumps {d} at {i}");
                prev = a;
            }
        }
    }

    #[test]
    fn weapon_motion_chop_winds_up_overhead_then_comes_down() {
        let s = strike_at(Some(WeaponClass::Axe), Motion::Chop);
        let cocked = arm_at(Motion::Chop, s, s * 0.75);
        let blow = arm_at(Motion::Chop, s, s);
        assert!(cocked.sh_pitch < -2.5, "overhead at the top of the wind-up");
        assert!(blow.sh_pitch > cocked.sh_pitch + 1.5, "brought down for the blow");
        assert!(cocked.chest_lean > 0.0 && blow.chest_lean < 0.0, "leans back, then into it");
    }

    #[test]
    fn weapon_motion_slashes_cross_the_body_in_opposite_directions() {
        let sr = strike_at(Some(WeaponClass::Blade), Motion::SlashR);
        let r0 = view_at(Motion::SlashR, sr, sr * 0.75);
        let r1 = view_at(Motion::SlashR, sr, sr);
        let l0 = view_at(Motion::SlashL, sr, sr * 0.75);
        let l1 = view_at(Motion::SlashL, sr, sr);
        assert!(r0.offset[0] > 0.0 && r1.offset[0] < 0.0, "forehand goes right to left");
        assert!(l0.offset[0] < 0.0 && l1.offset[0] > 0.0, "backhand goes left to right");
    }

    #[test]
    fn weapon_motion_strike_fires_exactly_once_at_its_moment() {
        for dt in [1.0 / 144.0, 1.0 / 30.0, 0.2, 5.0] {
            let mut sw = Swing::attack(Some(WeaponClass::Greataxe), 1.8, 0);
            let mut strikes = 0;
            let mut at = -1.0;
            for _ in 0..2000 {
                match sw.tick(dt) {
                    Tick::Strike => {
                        strikes += 1;
                        at = sw.t;
                    }
                    Tick::Done => break,
                    _ => {}
                }
            }
            assert_eq!(strikes, 1, "dt {dt}");
            assert!(at >= sw.strike_at * sw.dur - 1e-5 && at <= sw.strike_at * sw.dur + dt + 1e-5, "dt {dt}: at {at}");
            // Done is reported only after the strike, even with a huge step.
            let mut left = 0;
            while !sw.done() && left < 5000 {
                sw.tick(dt);
                left += 1;
            }
            assert!(sw.done());
        }
    }

    #[test]
    fn weapon_motion_heavy_weapons_telegraph_longer() {
        let dagger = Swing::attack(Some(WeaponClass::Dagger), 0.5, 0);
        let greataxe = Swing::attack(Some(WeaponClass::Greataxe), 1.8, 0);
        assert!(greataxe.strike_in() > 3.0 * dagger.strike_in(), "{} vs {}", greataxe.strike_in(), dagger.strike_in());
        let fists = Swing::attack(None, 0.0, 0);
        assert!(fists.dur <= 0.36);
    }

    #[test]
    fn weapon_motion_combo_continues_inside_the_window_and_resets_after() {
        let mut c = Combo::default();
        let b = Some(WeaponClass::Blade);
        assert_eq!(c.next(b), 0);
        c.idle(0.2);
        assert_eq!(c.next(b), 1);
        c.idle(0.2);
        assert_eq!(c.next(b), 2);
        c.idle(0.2);
        assert_eq!(c.next(b), 0, "wraps");
        c.idle(COMBO_WINDOW + 0.1);
        assert_eq!(c.next(b), 0, "too slow: starts over");
        c.idle(0.1);
        assert_eq!(c.next(Some(WeaponClass::Axe)), 0, "a different weapon starts over");
        let s = Swing::attack(b, 0.8, 1);
        assert_eq!(s.motion, Motion::SlashL);
    }

    #[test]
    fn weapon_motion_the_item_points_where_the_move_needs_it() {
        let along = |m: Motion, c: WeaponClass, key: usize| {
            let s = strike_at(Some(c), m);
            let u = [s * 0.75, s, s + (1.0 - s) * 0.35][key];
            hand_frame(&arm_at(m, s, u), 0.3, 0.3).1[1]
        };
        let chop_cocked = along(Motion::Chop, WeaponClass::Axe, 0);
        assert!(chop_cocked[0] < -0.3 && chop_cocked[1] < -0.3, "axe head hangs behind: {chop_cocked:?}");
        assert!(along(Motion::Chop, WeaponClass::Axe, 1)[0] > 0.8, "out ahead at the blow");
        assert!(along(Motion::Chop, WeaponClass::Axe, 2)[1] < -0.5, "follows through downward");
        assert!(along(Motion::Thrust, WeaponClass::Spear, 1)[0] > 0.9, "a thrust points where it goes");
        assert!(along(Motion::Throw, WeaponClass::Axe, 0)[0] < -0.5, "cocked behind the head");
        assert!(along(Motion::Throw, WeaponClass::Axe, 1)[0] > 0.8, "released forward");
        assert!(along(Motion::SlashR, WeaponClass::Blade, 1)[0] > 0.8);
        assert!(along(Motion::SlashR, WeaponClass::Blade, 2)[2] < -0.2, "forehand finishes to the left");
        assert!(along(Motion::SlashL, WeaponClass::Blade, 2)[2] > 0.2, "backhand finishes to the right");
        // At rest the grip is the rig's carry: upright along the forearm.
        let rest = hand_frame(&ArmPose::default(), 0.3, 0.3).1[1];
        assert!(rest[1] > 0.99);
    }

    #[test]
    fn weapon_motion_hand_frame_follows_the_swing() {
        let (up, fore) = (0.3, 0.3);
        let rest = hand_frame(&ArmPose::default(), up, fore);
        assert!((rest.0[1] + 0.6).abs() < 1e-4, "hangs straight down");
        assert!(rest.1[1][1] > 0.99, "a hanging fist carries the item upright");
        let s = strike_at(Some(WeaponClass::Axe), Motion::Chop);
        let cocked = hand_frame(&arm_at(Motion::Chop, s, s * 0.75), up, fore);
        let blow = hand_frame(&arm_at(Motion::Chop, s, s), up, fore);
        assert!(cocked.0[1] > 0.1, "wound up above the shoulder: {:?}", cocked.0);
        assert!(blow.0[0] > 0.3 && blow.0[1] < cocked.0[1], "brought down in front: {:?}", blow.0);
        let sr = strike_at(Some(WeaponClass::Blade), Motion::SlashR);
        let out = hand_frame(&arm_at(Motion::SlashR, sr, sr * 0.75), up, fore);
        let across = hand_frame(&arm_at(Motion::SlashR, sr, sr), up, fore);
        assert!(out.0[2] > across.0[2] + 0.1, "forehand starts out right: {:?} -> {:?}", out.0, across.0);
        // Axes stay orthonormal.
        for (_, b) in [rest, cocked, blow, out, across] {
            for i in 0..3 {
                let n = (b[i][0].powi(2) + b[i][1].powi(2) + b[i][2].powi(2)).sqrt();
                assert!((n - 1.0).abs() < 1e-4);
                for j in (i + 1)..3 {
                    let d = b[i][0] * b[j][0] + b[i][1] * b[j][1] + b[i][2] * b[j][2];
                    assert!(d.abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn weapon_motion_is_safe_on_bad_input() {
        let mut sw = Swing::attack(Some(WeaponClass::Wand), f32::NAN, 7);
        assert!(sw.dur.is_finite() && sw.dur > 0.0);
        assert_eq!(sw.tick(f32::NAN), Tick::Winding);
        assert_eq!(sw.tick(-3.0), Tick::Winding);
        let a = arm_at(Motion::Chop, f32::NAN, f32::INFINITY);
        assert!(a.sh_pitch.is_finite());
        let v = view_at(Motion::Sweep, 2.0, -1.0);
        assert!(v.pitch.is_finite());
    }

    #[test]
    fn weapon_motion_throw_releases_mid_swing_and_catch_starts_reaching() {
        let mut th = Swing::of(Some(WeaponClass::Axe), Motion::Throw, 1.0, 0);
        let mut t_rel = 0.0;
        while th.tick(0.01) != Tick::Strike {
            t_rel = th.t;
        }
        assert!((t_rel / th.dur - 0.5).abs() < 0.05);
        let c = Swing::of(Some(WeaponClass::Axe), Motion::Catch, 1.0, 0);
        assert!((c.arm().sh_pitch - REACH_ARM.sh_pitch).abs() < 1e-4, "a catch picks up from the reach");
    }
}
