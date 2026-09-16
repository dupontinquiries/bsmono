//! blackstar-demo — the smallest thing that is actually a game object.
//!
//! ## Why this exists
//!
//! `blackstar-rs/src/main.rs` is 9,937 lines and the crate is 191 files. That
//! is navigable if you wrote it last week and hostile if you did not, and the
//! practical symptom is that a change cannot be checked without launching the
//! whole game — window, GPU, audio, SDF bake, 268 chassis — so every edit is
//! a guess until it is on screen.
//!
//! This crate is the counter-proposal. Everything a vehicle IS lives here, in
//! plain data with no dependencies:
//!
//!   * [`Shape`]  — the hull as control sections; geometry is derived, never stored
//!   * [`Fold`]   — vehicle <-> robot as a state machine over [`Ms`]
//!   * [`Drive`]  — throttle, steering, and where the thing ends up
//!   * [`Walk`]   — a gait with FOOT PLANTING, which is the thing the old
//!                  engine's sliding-parts walk never had
//!
//! There is no `Instant::now()` anywhere: every entry point takes a
//! timestamp, so a whole session is a fold over `Ms` values and a test is a
//! list of numbers. The viewer in `main.rs` is the only part that needs a
//! screen, and it is deliberately small enough to read in one sitting.
//!
//! The rule that keeps it that way: **nothing in this file may import a GPU
//! type.** If a feature cannot be expressed without one, it belongs in the
//! shell.

pub type Ms = u64;

// ───────────────────────────────────────────────────────────────── shape

/// One cross-section of a hull: distance along the body, half width, floor,
/// roof, and how rounded the section is (0 = box, 1 = ellipse).
///
/// Same tuple `blackstar-rs/src/meshgen.rs` uses, deliberately — a shape
/// authored here can be pasted there and vice versa.
/// Half-thickness of a tyre as a fraction of its radius.
///
/// Named here rather than left implicit. The generator builds a tyre 0.55
/// wide in unit space and scales x by `r * 0.5`, so the half-thickness is
/// `0.275 * r` -- a constant that lived in two places and agreed by luck. A
/// width test disagreeing with a rendered measurement by 7% is how it
/// surfaced.
pub const TYRE_HALF_THICK: f32 = 0.275;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    pub z: f32,
    pub half_w: f32,
    pub floor: f32,
    pub roof: f32,
    pub round: f32,
}

impl Section {
    pub const fn new(z: f32, half_w: f32, floor: f32, roof: f32, round: f32) -> Self {
        Self { z, half_w, floor, roof, round }
    }
    pub fn height(&self) -> f32 { self.roof - self.floor }
}

/// A hull. Geometry is DERIVED from this on demand and never stored, which is
/// why a model is ~200 bytes and why resolution is a read parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub name: String,
    pub sections: Vec<Section>,
    /// (x, y, z, radius) per wheel. Empty for something that does not roll.
    pub wheels: Vec<(f32, f32, f32, f32)>,
}

impl Shape {
    pub fn length(&self) -> f32 {
        match (self.sections.first(), self.sections.last()) {
            (Some(a), Some(b)) => b.z - a.z,
            _ => 0.0,
        }
    }
    /// Widest point, WHEELS INCLUDED.
    ///
    /// The hull alone gives a different number (1.98 vs 2.11 on MOIRE) and
    /// both are honest measurements of different things -- but a vehicle's
    /// width is what it will hit a wall with, so the wheels count. A test
    /// caught this disagreeing with the design note by 6%, which is exactly
    /// the size of mistake that never gets noticed by eye.
    pub fn width(&self) -> f32 {
        let hull = self.sections.iter().map(|s| s.half_w).fold(0.0, f32::max);
        let over = self.wheels.iter()
            .map(|w| w.0.abs() + w.3 * TYRE_HALF_THICK)
            .fold(0.0, f32::max);
        hull.max(over) * 2.0
    }

    /// Hull width with nothing bolted on. For anything that must sit ON the
    /// bodywork rather than clear it.
    pub fn hull_width(&self) -> f32 {
        self.sections.iter().map(|s| s.half_w).fold(0.0, f32::max) * 2.0
    }
    pub fn height(&self) -> f32 {
        let roof = self.sections.iter().map(|s| s.roof).fold(f32::MIN, f32::max);
        let floor = self.sections.iter().map(|s| s.floor).fold(f32::MAX, f32::min);
        roof - floor
    }
    /// Half width at an arbitrary z, linear between control sections. Used to
    /// place anything that must sit ON the bodywork — arches, sills, lights —
    /// so it cannot poke through where the hull tapers.
    pub fn half_w_at(&self, z: f32) -> f32 {
        let s = &self.sections;
        if s.is_empty() { return 0.0; }
        if z <= s[0].z { return s[0].half_w; }
        if z >= s[s.len() - 1].z { return s[s.len() - 1].half_w; }
        for w in s.windows(2) {
            if w[0].z <= z && z <= w[1].z {
                let t = (z - w[0].z) / (w[1].z - w[0].z).max(1e-6);
                return w[0].half_w * (1.0 - t) + w[1].half_w * t;
            }
        }
        s[s.len() - 1].half_w
    }
    /// Wheelbase: front axle to rear axle. Zero when it does not roll.
    pub fn wheelbase(&self) -> f32 {
        let zs: Vec<f32> = self.wheels.iter().map(|w| w.2).collect();
        match (zs.iter().cloned().fold(f32::MAX, f32::min),
               zs.iter().cloned().fold(f32::MIN, f32::max)) {
            (a, b) if a <= b => b - a,
            _ => 0.0,
        }
    }
}

/// MOIRE — the default. Mid-set hitbox, a waist, a two-stage roof.
pub fn moire() -> Shape {
    Shape {
        name: "MOIRE".into(),
        sections: vec![
            Section::new(-2.05, 0.44, 0.10, 0.34, 0.30),
            Section::new(-1.55, 0.72, 0.07, 0.46, 0.34),
            Section::new(-1.05, 0.88, 0.05, 0.52, 0.32),
            Section::new(-0.55, 0.80, 0.05, 0.60, 0.30),   // waist
            Section::new(-0.05, 0.86, 0.05, 0.74, 0.32),
            Section::new( 0.50, 0.92, 0.05, 0.78, 0.32),
            Section::new( 1.05, 0.99, 0.06, 0.70, 0.30),   // haunch
            Section::new( 1.60, 0.86, 0.09, 0.60, 0.32),
            Section::new( 1.92, 0.76, 0.10, 0.58, 0.30),
        ],
        wheels: vec![
            (-0.86, 0.31, -1.42, 0.31), (0.86, 0.31, -1.42, 0.31),
            (-0.96, 0.34,  1.30, 0.34), (0.96, 0.34,  1.30, 0.34),
        ],
    }
}

// ────────────────────────────────────────────────────────────────── fold

/// Where a chassis is between its two forms.
///
/// A fold is NOT a lerp between two baked poses. `blackstar-rs/src/actuator.rs`
/// already names why that fails: "a `cyber::Muscle` that only responds to
/// torque is decoration, because the fold is still a scripted lerp underneath
/// it." So this is a state machine with a direction, a rate and a LATCH — the
/// latch is what makes it feel mechanical rather than animated, because a
/// half-folded machine is not a valid state to drive in and the model says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Vehicle,
    Folding,
    Unfolding,
    Robot,
}

#[derive(Clone, Debug)]
pub struct Fold {
    /// 0.0 = vehicle, 1.0 = robot. Only meaningful with `form`.
    t: f32,
    form: Form,
    /// Seconds for a full fold. From chassis.json's `xform_dur`.
    dur_ms: Ms,
    last: Ms,
}

impl Fold {
    pub fn new(dur_ms: Ms) -> Self {
        Self { t: 0.0, form: Form::Vehicle, dur_ms: dur_ms.max(1), last: 0 }
    }
    pub fn t(&self) -> f32 { self.t }
    pub fn form(&self) -> Form { self.form }
    /// Can this thing be driven right now? Only at the ends — a machine
    /// mid-fold has its wheels somewhere else.
    pub fn drivable(&self) -> bool { self.form == Form::Vehicle }
    pub fn walkable(&self) -> bool { self.form == Form::Robot }
    pub fn busy(&self) -> bool {
        matches!(self.form, Form::Folding | Form::Unfolding)
    }

    /// Ask to transform. Ignored mid-fold: a fold that can be interrupted
    /// halfway needs every downstream system to cope with an arbitrary t,
    /// and nothing does.
    pub fn toggle(&mut self, now: Ms) -> bool {
        match self.form {
            Form::Vehicle => { self.form = Form::Folding; self.last = now; true }
            Form::Robot => { self.form = Form::Unfolding; self.last = now; true }
            _ => false,
        }
    }

    /// Advance. `now` is a parameter, never read from a clock, so a test is a
    /// list of timestamps and a replay is exact.
    pub fn tick(&mut self, now: Ms) {
        if !self.busy() { self.last = now; return; }
        let dt = now.saturating_sub(self.last);
        self.last = now;
        let step = dt as f32 / self.dur_ms as f32;
        match self.form {
            Form::Folding => {
                self.t += step;
                if self.t >= 1.0 { self.t = 1.0; self.form = Form::Robot; }
            }
            Form::Unfolding => {
                self.t -= step;
                if self.t <= 0.0 { self.t = 0.0; self.form = Form::Vehicle; }
            }
            _ => {}
        }
    }

    /// Eased fold fraction for display. Smoothstep, because a linear fold
    /// reads as a rig test rather than a mechanism settling into a detent.
    pub fn eased(&self) -> f32 {
        let t = self.t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

// ────────────────────────────────────────────────────────────────── drive

/// Where the thing is and how fast. Bicycle model: two virtual wheels on the
/// centreline, steering at the front. Enough to be right about turning radius
/// and understeer without pretending to be a tyre model.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub x: f32,
    pub z: f32,
    /// Heading in radians. 0 = facing -z, which is the nose direction every
    /// `Shape` is authored in.
    pub yaw: f32,
}

#[derive(Clone, Debug)]
pub struct Drive {
    pub pose: Pose,
    pub speed: f32,
    wheelbase: f32,
    max_speed: f32,
    accel: f32,
    /// How hard it can turn the front wheels, radians.
    max_steer: f32,
    last: Ms,
}

impl Drive {
    /// Built FROM the shape, so a longer car turns wider without anyone
    /// tuning a second number. That coupling is the reason shape and physics
    /// live in the same crate.
    pub fn for_shape(s: &Shape) -> Self {
        Self {
            pose: Pose::default(),
            speed: 0.0,
            wheelbase: s.wheelbase().max(0.5),
            max_speed: 26.0,
            accel: 14.0,
            max_steer: 0.62,
            last: 0,
        }
    }

    /// `throttle` and `steer` both in -1..1.
    pub fn tick(&mut self, now: Ms, throttle: f32, steer: f32) {
        let dt = now.saturating_sub(self.last) as f32 / 1000.0;
        self.last = now;
        if dt <= 0.0 { return; }
        let throttle = throttle.clamp(-1.0, 1.0);
        let steer = steer.clamp(-1.0, 1.0);

        self.speed += throttle * self.accel * dt;
        // drag, so coasting settles instead of running forever
        self.speed -= self.speed * 0.8 * dt;
        self.speed = self.speed.clamp(-self.max_speed * 0.4, self.max_speed);

        // Steering authority FALLS with speed. Without this a car turns as
        // sharply at 26 m/s as at 2, which is the single thing that makes
        // arcade driving feel like sliding a sprite around.
        let grip = 1.0 / (1.0 + (self.speed.abs() / 12.0).powi(2));
        let angle = steer * self.max_steer * grip;
        let yaw_rate = if angle.abs() > 1e-5 {
            self.speed * angle.tan() / self.wheelbase
        } else {
            0.0
        };
        self.pose.yaw += yaw_rate * dt;
        self.pose.x += self.speed * self.pose.yaw.sin() * dt;
        self.pose.z -= self.speed * self.pose.yaw.cos() * dt;
    }

    /// Radius of the circle it is currently describing. Infinite when straight.
    pub fn turn_radius(&self, steer: f32) -> f32 {
        let grip = 1.0 / (1.0 + (self.speed.abs() / 12.0).powi(2));
        let a = (steer.clamp(-1.0, 1.0) * self.max_steer * grip).abs();
        if a < 1e-5 { f32::INFINITY } else { self.wheelbase / a.tan() }
    }
}

// ─────────────────────────────────────────────────────────────────── walk

/// A two-legged gait where the stance foot DOES NOT MOVE.
///
/// This is the whole point of the module. The exengine walk slid the model's
/// parts along the ground — the legs animated and the feet travelled with the
/// body, so nothing ever pushed against anything. Here the stance foot is
/// pinned in WORLD space for the whole stance phase, and the body is moved by
/// how far that foot is behind the hip. The body follows the feet rather than
/// the feet following the body, which is the entire difference between a walk
/// and a slide.
#[derive(Clone, Debug)]
pub struct Walk {
    /// World z of each foot. Index 0 = left, 1 = right.
    foot_z: [f32; 2],
    /// Which foot is planted.
    stance: usize,
    /// 0..1 through the current step.
    phase: f32,
    step_len: f32,
    step_ms: Ms,
    /// Body position, DERIVED from the planted foot.
    pub body_z: f32,
    last: Ms,
}

impl Walk {
    pub fn for_shape(s: &Shape) -> Self {
        // Stride from the machine's own size: a tall chassis takes long
        // strides, which is why `raptor` and `bruiser` should not share a
        // cadence.
        let step_len = (s.height() * 0.55).max(0.25);
        Self {
            foot_z: [-step_len * 0.5, step_len * 0.5],
            stance: 0,
            phase: 0.0,
            step_len,
            step_ms: 520,
            body_z: 0.0,
            last: 0,
        }
    }

    pub fn step_len(&self) -> f32 { self.step_len }
    pub fn stance_foot(&self) -> usize { self.stance }
    pub fn foot_z(&self, i: usize) -> f32 { self.foot_z[i] }
    pub fn swing_foot(&self) -> usize { 1 - self.stance }

    /// Advance the gait. Returns how far the body moved this tick.
    pub fn tick(&mut self, now: Ms, walking: bool) -> f32 {
        let dt = now.saturating_sub(self.last);
        self.last = now;
        if !walking || dt == 0 {
            return 0.0;
        }
        let before = self.body_z;
        self.phase += dt as f32 / self.step_ms as f32;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            // The swing foot lands one stride ahead of the planted one, and
            // THEN becomes the stance. The planted foot never moved.
            let swing = self.swing_foot();
            self.foot_z[swing] = self.foot_z[self.stance] - self.step_len;
            self.stance = swing;
        }
        // Body rides between the feet, biased toward the stance foot as the
        // step progresses. It is a consequence of foot positions, never an
        // input.
        let a = self.foot_z[self.stance];
        let b = self.foot_z[self.swing_foot()];
        self.body_z = a + (b - a) * (0.5 - 0.5 * (self.phase * std::f32::consts::TAU).cos()) * 0.5;
        self.body_z - before
    }
}

// ─────────────────────────────────────────────────────────────────── tests

#[cfg(test)]
mod tests {
    use super::*;

    // ---- shape ----------------------------------------------------------

    #[test]
    fn moire_is_the_proportions_it_claims() {
        let c = moire();
        let lw = c.length() / c.width();
        assert!((1.85..1.95).contains(&lw), "l/w drifted to {lw:.2}");
        // and the hull alone is narrower than the track, i.e. the wheels
        // stand proud, which is what the arches exist to cover
        assert!(c.hull_width() < c.width());
        // A default car must not be a wedge or a brick.
        assert!(c.height() > 0.6 && c.height() < 0.9, "height {}", c.height());
    }

    #[test]
    fn the_waist_is_real() {
        // The design claim is that the hull pinches behind the front axle and
        // flares again. If someone smooths that out the car becomes a loaf
        // and nothing else in the file would notice.
        let c = moire();
        let w: Vec<f32> = c.sections.iter().map(|s| s.half_w).collect();
        let peak = w[2];
        let waist = w[3];
        let haunch = w[6];
        assert!(waist < peak, "no waist: {waist} >= {peak}");
        assert!(haunch > peak, "rear is not the widest: {haunch} <= {peak}");
    }

    #[test]
    fn half_w_at_never_exceeds_the_control_polygon() {
        // Anything mounted on the flank is placed with this. If it can
        // overshoot, sills and arches poke through the bodywork -- which is
        // exactly the bug that got three attempts in the generator.
        let c = moire();
        let max = c.sections.iter().map(|s| s.half_w).fold(0.0, f32::max);
        let mut z = -3.0;
        while z < 3.0 {
            let hw = c.half_w_at(z);
            assert!(hw <= max + 1e-5, "half_w_at({z}) = {hw} > {max}");
            assert!(hw > 0.0);
            z += 0.01;
        }
    }

    // ---- fold -----------------------------------------------------------

    #[test]
    fn a_fold_completes_and_latches() {
        let mut f = Fold::new(1000);
        assert!(f.drivable() && !f.walkable());
        assert!(f.toggle(0));
        for t in (0..=1200).step_by(50) { f.tick(t); }
        assert_eq!(f.form(), Form::Robot);
        assert!(f.walkable() && !f.drivable());
        assert_eq!(f.t(), 1.0, "t must land exactly on 1, not 0.9997");
    }

    #[test]
    fn a_fold_cannot_be_interrupted() {
        // Every downstream system would have to cope with an arbitrary t.
        let mut f = Fold::new(1000);
        f.toggle(0);
        f.tick(200);
        assert!(!f.toggle(200), "toggle mid-fold must be refused");
        assert_eq!(f.form(), Form::Folding);
    }

    #[test]
    fn folding_is_not_drivable() {
        let mut f = Fold::new(1000);
        f.toggle(0);
        f.tick(500);
        assert!(!f.drivable() && !f.walkable(),
                "a half-folded machine is neither, and must say so");
    }

    #[test]
    fn a_fold_round_trips_exactly() {
        let mut f = Fold::new(800);
        f.toggle(0);
        for t in (0..=900).step_by(25) { f.tick(t); }
        assert_eq!(f.form(), Form::Robot);
        f.toggle(900);
        for t in (900..=1900).step_by(25) { f.tick(t); }
        assert_eq!(f.form(), Form::Vehicle);
        assert_eq!(f.t(), 0.0);
    }

    #[test]
    fn fold_is_frame_rate_independent() {
        // The same elapsed time must give the same t whether it arrives in
        // one tick or forty. Without this the fold finishes at a different
        // moment on a fast machine.
        let mut coarse = Fold::new(1000);
        let mut fine = Fold::new(1000);
        coarse.toggle(0);
        fine.toggle(0);
        coarse.tick(500);
        for t in (0..=500).step_by(10) { fine.tick(t); }
        assert!((coarse.t() - fine.t()).abs() < 1e-5,
                "coarse {} vs fine {}", coarse.t(), fine.t());
    }

    // ---- drive ----------------------------------------------------------

    #[test]
    fn steering_authority_falls_with_speed() {
        // The one property that separates driving from sliding a sprite.
        let mut d = Drive::for_shape(&moire());
        d.speed = 2.0;
        let slow = d.turn_radius(1.0);
        d.speed = 24.0;
        let fast = d.turn_radius(1.0);
        assert!(fast > slow * 3.0,
                "high-speed radius {fast:.1} is not much wider than {slow:.1}");
    }

    #[test]
    fn turn_radius_scales_with_wheelbase() {
        // A longer car turns wider, with nobody tuning a second number.
        let mut short = moire();
        short.wheels = vec![(-0.8, 0.3, -0.6, 0.3), (0.8, 0.3, 0.6, 0.3)];
        let a = Drive::for_shape(&short).turn_radius(1.0);
        let b = Drive::for_shape(&moire()).turn_radius(1.0);
        assert!(b > a, "wheelbase {} should turn wider than {}",
                moire().wheelbase(), short.wheelbase());
    }

    #[test]
    fn a_car_driven_straight_goes_straight() {
        let mut d = Drive::for_shape(&moire());
        for t in (0..3000).step_by(16) { d.tick(t, 1.0, 0.0); }
        assert!(d.pose.x.abs() < 1e-3, "drifted sideways to {}", d.pose.x);
        assert!(d.pose.z < -10.0, "barely moved: z = {}", d.pose.z);
        assert!(d.pose.yaw.abs() < 1e-6);
    }

    #[test]
    fn coasting_settles_instead_of_running_forever() {
        let mut d = Drive::for_shape(&moire());
        for t in (0..2000).step_by(16) { d.tick(t, 1.0, 0.0); }
        let moving = d.speed;
        for t in (2000..12000).step_by(16) { d.tick(t, 0.0, 0.0); }
        assert!(moving > 5.0);
        assert!(d.speed.abs() < 0.2, "still rolling at {}", d.speed);
    }

    #[test]
    fn a_full_lock_turn_comes_back_around() {
        // Integration: hold full lock and the heading should wrap.
        let mut d = Drive::for_shape(&moire());
        let mut yaw = 0.0f32;
        for t in (0..20000).step_by(16) {
            d.tick(t, 0.35, 1.0);
            yaw = d.pose.yaw;
        }
        assert!(yaw.abs() > std::f32::consts::TAU,
                "never completed a circle: {yaw:.2} rad");
    }

    // ---- walk -----------------------------------------------------------

    #[test]
    fn the_stance_foot_does_not_move() {
        // THE test. The old walk slid its parts; if this ever fails the walk
        // has become an animation again.
        let mut w = Walk::for_shape(&moire());
        let stance = w.stance_foot();
        let pinned = w.foot_z(stance);
        for t in (0..500).step_by(10) {
            w.tick(t, true);
            if w.stance_foot() != stance { break; }
            assert_eq!(w.foot_z(stance), pinned,
                       "stance foot slid from {pinned} to {}", w.foot_z(stance));
        }
    }

    #[test]
    fn feet_alternate() {
        let mut w = Walk::for_shape(&moire());
        let mut seen = [false, false];
        for t in (0..4000).step_by(20) {
            w.tick(t, true);
            seen[w.stance_foot()] = true;
        }
        assert!(seen[0] && seen[1], "one foot never took a turn");
    }

    #[test]
    fn the_body_advances_only_when_walking() {
        let mut w = Walk::for_shape(&moire());
        for t in (0..2000).step_by(20) { w.tick(t, true); }
        let moved = w.body_z;
        assert!(moved < -0.3, "body did not advance: {moved}");
        for t in (2000..4000).step_by(20) { w.tick(t, false); }
        assert_eq!(w.body_z, moved, "body moved while standing still");
    }

    #[test]
    fn stride_comes_from_the_machines_size() {
        let small = Shape { name: "s".into(), sections: vec![
            Section::new(-0.5, 0.2, 0.0, 0.4, 0.3),
            Section::new(0.5, 0.2, 0.0, 0.4, 0.3)], wheels: vec![] };
        let big = Shape { name: "b".into(), sections: vec![
            Section::new(-0.5, 0.2, 0.0, 2.4, 0.3),
            Section::new(0.5, 0.2, 0.0, 2.4, 0.3)], wheels: vec![] };
        assert!(Walk::for_shape(&big).step_len() > Walk::for_shape(&small).step_len());
    }

    #[test]
    fn the_whole_session_is_a_fold_over_timestamps() {
        // No clock anywhere: drive, transform, walk, all from a list of Ms.
        let car = moire();
        let mut fold = Fold::new(900);
        let mut drive = Drive::for_shape(&car);
        let mut walk = Walk::for_shape(&car);
        let mut t: Ms = 0;
        while t < 2000 { drive.tick(t, 1.0, 0.2); t += 16; }
        let driven = drive.pose;
        assert!(fold.toggle(t));
        while fold.busy() { t += 16; fold.tick(t); }
        assert!(fold.walkable());
        let start = walk.body_z;
        while t < 6000 { walk.tick(t, true); t += 16; }
        assert!(walk.body_z < start);
        assert_ne!(driven, Pose::default());
    }
}
