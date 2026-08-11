//! light-kit — what the ambient light sensor is actually good for.
//!
//! Four things, and they are separable: **covered?**, **how bright?**, **what
//! colour?**, and **what should the theme do about it**. No sysfs, no
//! rendering — the same policy-only split `hover_kit` uses, so all of it is
//! testable in a dark room on a machine with no sensor.
//!
//! ## Differentials, not levels — and why that pairs with proximity
//!
//! `hover_kit`'s proximity gate answers *"is the phone face-down or in a
//! pocket"* with one saturated bit. The ALS answers a different question with
//! **range**: a hand passing over the display drops the reading sharply and
//! briefly, which a level threshold cannot distinguish from walking into a
//! darker room. The *rate* can.
//!
//! So the two combine rather than duplicate:
//!
//! ```text
//!   prox saturated  ──► pocket / face-down ──► don't look
//!   ALS step down   ──► something covered   ──► look (a hand is over the glass)
//!   ALS slow drift  ──► the room changed    ──► adapt brightness, don't react
//! ```
//!
//! ## The controller is where naive implementations hunt
//!
//! Four mistakes produce the screen that breathes, and all four are avoidable:
//!
//! 1. **Controlling in lux.** Perception of brightness is roughly logarithmic,
//!    so a controller linear in lux is aggressive in the dark and sluggish
//!    outdoors. This one works in log space.
//! 2. **Integral windup.** At the backlight's rail the error persists and a
//!    naive integrator keeps accumulating; when the light changes it unwinds
//!    into a large overshoot. Integration is conditional here.
//! 3. **Derivative kick.** Differentiating the *error* spikes on every setpoint
//!    change. This differentiates the **measurement**, which is the standard
//!    fix and costs nothing.
//! 4. **No deadband.** Below the just-noticeable difference a correction is
//!    invisible but its motion is not, so the screen shimmers while
//!    "converging". Inside the deadband the output is held exactly.
//!
//! And one asymmetry that is a UX decision rather than control theory:
//! **brighten quickly, dim slowly.** Walking into sunlight you cannot read the
//! screen and every millisecond counts; walking into shade a bright screen is
//! merely non-ideal, and a display that dims eagerly reads as broken.

// -------------------------------------------------------------- sensing ---

/// A reading in lux. Zero is legal — a covered sensor really does read zero.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Lux(pub f32);

/// Perceptual position of a lux value, 0..=1.
///
/// `log10(lux + 1)` normalised over 0..100_000 lx — moonlight to direct sun.
/// The `+1` keeps darkness finite: `log10(0)` is negative infinity, and a
/// controller fed that produces an instant rail rather than a dim screen.
pub fn perceptual(lux: f32) -> f32 {
    if !lux.is_finite() || lux <= 0.0 { return 0.0; }
    (lux.max(0.0) + 1.0).log10() / 100_000f32.log10()
}

/// What the light is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LightEvent {
    /// A fast, large drop — a hand or an object over the glass.
    Covered,
    /// A fast rise back — whatever it was has gone.
    Uncovered,
    /// Slow change; the room is different, not the display.
    Drifting,
    Steady,
}

/// Fractional drop over the window that counts as a cover.
///
/// Fractional rather than absolute, because a 50 lx fall is a cover indoors and
/// noise outdoors. The reading has to more than halve.
pub const COVER_DROP: f32 = 0.55;
/// The window a cover has to happen inside. Slower than this is the room.
pub const COVER_WINDOW_MS: f32 = 250.0;

/// Tracks the differential.
#[derive(Clone, Copy, Debug, Default)]
pub struct Differ {
    last: Option<f32>,
    /// Reference level a cover is measured against — the pre-cover brightness.
    baseline: f32,
    since_ms: f32,
    covered: bool,
}

impl Differ {
    pub fn is_covered(&self) -> bool { self.covered }
    pub fn baseline(&self) -> f32 { self.baseline }

    pub fn update(&mut self, lux: Lux, dt_ms: f32) -> LightEvent {
        let v = if lux.0.is_finite() { lux.0.max(0.0) } else { return LightEvent::Steady };
        let Some(prev) = self.last else {
            self.last = Some(v);
            self.baseline = v;
            return LightEvent::Steady;
        };
        self.last = Some(v);
        self.since_ms += dt_ms.max(0.0);

        if self.covered {
            // Uncovered when the reading comes back toward the baseline it fell
            // from. Compared against the BASELINE rather than the covered
            // level, or a slow brightening under a hand reads as uncovering.
            if v >= self.baseline * (1.0 - COVER_DROP * 0.6) {
                self.covered = false;
                self.since_ms = 0.0;
                self.baseline = v;
                return LightEvent::Uncovered;
            }
            return LightEvent::Steady;
        }

        let dropped = prev > 0.0 && v <= prev * (1.0 - COVER_DROP);
        if dropped && self.since_ms <= COVER_WINDOW_MS {
            self.covered = true;
            self.since_ms = 0.0;
            return LightEvent::Covered;
        }
        // Not a cover: track the baseline so the next one is measured against
        // where the room actually is.
        let rel = if prev > 0.0 { (v - prev).abs() / prev } else { 0.0 };
        self.baseline = self.baseline * 0.9 + v * 0.1;
        if rel > 0.05 { self.since_ms = 0.0; LightEvent::Drifting } else { LightEvent::Steady }
    }
}

// ----------------------------------------------------------- controller ---

/// Backlight controller: PID in perceptual space, with the four fixes.
#[derive(Clone, Copy, Debug)]
pub struct Backlight {
    kp: f32,
    ki: f32,
    kd: f32,
    integral: f32,
    /// Previous MEASUREMENT, for derivative-on-measurement.
    last_meas: Option<f32>,
    output: f32,
    /// Corrections smaller than this are not made at all.
    deadband: f32,
    /// Maximum output change per second, brightening then dimming.
    slew_up: f32,
    slew_down: f32,
}

/// Just-noticeable difference in backlight, roughly. Below it a correction is
/// invisible but its motion is not.
pub const DEADBAND: f32 = 0.015;

impl Default for Backlight {
    fn default() -> Self {
        Backlight {
            kp: 0.9, ki: 0.25, kd: 0.06,
            integral: 0.0, last_meas: None, output: 0.5,
            deadband: DEADBAND,
            // Brighten in ~1.5 s, dim over ~5 s. See the module docs.
            slew_up: 0.65, slew_down: 0.2,
        }
    }
}

impl Backlight {
    pub fn output(&self) -> f32 { self.output }
    pub fn integral(&self) -> f32 { self.integral }

    /// Step the controller. `target` and `measured` are perceptual 0..=1.
    pub fn step(&mut self, target: f32, measured: f32, dt_s: f32) -> f32 {
        if dt_s <= 0.0 || !dt_s.is_finite() { return self.output; }
        let target = target.clamp(0.0, 1.0);
        let measured = measured.clamp(0.0, 1.0);
        let error = target - measured;

        // Deadband: hold exactly. A correction below the JND is invisible, and
        // making it anyway is what makes a screen shimmer while "converging".
        if error.abs() < self.deadband {
            self.last_meas = Some(measured);
            return self.output;
        }

        // Derivative on MEASUREMENT, not error — differentiating the error
        // spikes on every setpoint change for no physical reason.
        let d = match self.last_meas {
            Some(prev) => -(measured - prev) / dt_s,
            None => 0.0,
        };
        self.last_meas = Some(measured);

        // INCREMENTAL, not absolute. A PID computes a CORRECTION; treating its
        // output as the level itself means that as the error shrinks the
        // commanded level shrinks with it, and the controller drives the screen
        // back down toward zero. My first version did exactly that and settled
        // at 0.51 against a target of 0.80 — the test caught it, and the shape
        // of the failure (stuck partway, not oscillating) is what named the
        // cause.
        let correction = self.kp * error + self.ki * (self.integral + error * dt_s) + self.kd * d;
        let unclamped = self.output + correction;
        // CONDITIONAL INTEGRATION: only accumulate when the result is not
        // pinned at a rail. At the rail the error persists and a naive
        // integrator winds up, then unwinds into a large overshoot when the
        // light finally changes.
        let railed = (unclamped >= 1.0 && error > 0.0) || (unclamped <= 0.0 && error < 0.0);
        if !railed { self.integral += error * dt_s; }
        // Bounded regardless, so a long spell near a rail cannot store an
        // arbitrary amount of history.
        self.integral = self.integral.clamp(-1.0, 1.0);

        let want = (self.output + self.kp * error + self.ki * self.integral + self.kd * d)
            .clamp(0.0, 1.0);

        // Asymmetric slew: brighten fast, dim slow.
        let max_step = if want > self.output { self.slew_up } else { self.slew_down } * dt_s;
        let delta = (want - self.output).clamp(-max_step, max_step);
        self.output = (self.output + delta).clamp(0.0, 1.0);
        self.output
    }

    /// A user moved the brightness slider. Adopt it and forget the history —
    /// an integral accumulated against the old target would drag the screen
    /// back off the value they just chose.
    pub fn adopt(&mut self, output: f32) {
        self.output = output.clamp(0.0, 1.0);
        self.integral = 0.0;
        self.last_meas = None;
    }
}

// ---------------------------------------------------------- white point ---

/// Ambient colour, from an RGB-capable sensor such as the APDS9960.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AmbientRgb { pub r: f32, pub g: f32, pub b: f32 }

/// Correlated colour temperature in kelvin, McCamy's approximation.
///
/// Returns `None` when the channels carry no usable chroma — a dark or
/// saturated sensor gives a ratio that is noise, and a confident 2700 K from
/// noise would tint the screen for no reason.
pub fn cct_kelvin(c: AmbientRgb) -> Option<f32> {
    let (r, g, b) = (c.r.max(0.0), c.g.max(0.0), c.b.max(0.0));
    let sum = r + g + b;
    if sum <= 1e-3 { return None; }
    // sRGB-ish to XYZ, then to xy chromaticity.
    let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
    let denom = x + y + z;
    if denom <= 1e-6 { return None; }
    let (cx, cy) = (x / denom, y / denom);
    if (cy - 0.1858).abs() < 1e-4 { return None; }
    let n = (cx - 0.3320) / (0.1858 - cy);
    let cct = 449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33;
    if cct.is_finite() && (1000.0..=25_000.0).contains(&cct) { Some(cct) } else { None }
}

/// Warmest and coolest the display will be driven.
///
/// Bounded because a screen matched exactly to a 2000 K room is unusably
/// orange, and one matched to overcast daylight looks blue and clinical. The
/// aim is to reduce the mismatch, not to erase it.
pub const CCT_MIN_K: f32 = 3400.0;
pub const CCT_MAX_K: f32 = 6900.0;

/// How far toward the room's temperature the display moves.
///
/// Partial on purpose: a display is a light source in the room, not a surface
/// lit by it, and driving it all the way makes whites read as coloured paper.
pub const CCT_BLEND: f32 = 0.6;

/// The white point to render at, given the room.
pub fn target_cct(room_k: Option<f32>, display_native_k: f32) -> f32 {
    let Some(room) = room_k else { return display_native_k };
    let room = room.clamp(CCT_MIN_K, CCT_MAX_K);
    (display_native_k + (room - display_native_k) * CCT_BLEND).clamp(CCT_MIN_K, CCT_MAX_K)
}

// --------------------------------------------------------- theme tint ---

/// Maximum saturation a room may push the theme to.
///
/// **The same 0.18 `ambient_palette` and `ambient_blobs` already enforce.** A
/// third feature arriving at one number independently is the sign it is a real
/// contrast constraint rather than a preference, so it is shared rather than
/// re-derived.
pub const TINT_MAX_SAT: f32 = 0.18;

/// How much of the room's hue reaches the theme per step.
///
/// Slow by design: walking past a blue-lit shopfront should not repaint the
/// app. The tint should be something you notice having happened, not something
/// you watch happen.
pub const TINT_RATE_PER_S: f32 = 0.08;

/// A theme tint, tracked toward the room.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tint {
    r: f32,
    g: f32,
    b: f32,
    started: bool,
}

impl Tint {
    pub fn rgb(&self) -> (f32, f32, f32) { (self.r, self.g, self.b) }

    /// Move toward the room's colour. `None` leaves the tint where it is
    /// rather than decaying to neutral — an unreadable sample is not evidence
    /// that the room went grey.
    pub fn update(&mut self, room: Option<AmbientRgb>, dt_s: f32) -> (f32, f32, f32) {
        let Some(c) = room else { return self.rgb() };
        let sum = c.r + c.g + c.b;
        if sum <= 1e-3 { return self.rgb(); }
        let (tr, tg, tb) = (c.r / sum, c.g / sum, c.b / sum);
        if !self.started {
            self.started = true;
            self.r = tr; self.g = tg; self.b = tb;
        } else {
            let k = (TINT_RATE_PER_S * dt_s.max(0.0)).clamp(0.0, 1.0);
            self.r += (tr - self.r) * k;
            self.g += (tg - self.g) * k;
            self.b += (tb - self.b) * k;
        }
        self.rgb()
    }

    /// Saturation of the current tint, HSL definition.
    pub fn saturation(&self) -> f32 {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < 1e-6 { return 0.0; }
        if l <= 0.5 { (max - min) / (max + min) } else { (max - min) / (2.0 - max - min) }
    }

    /// Apply the tint to a base colour, capped so text stays readable.
    pub fn apply(&self, base: [u8; 3], strength: f32) -> [u8; 3] {
        let neutral = 1.0 / 3.0;
        let dev = |ch: f32| (ch - neutral) * 3.0;
        // Cap the CHANNEL DEVIATION directly at the ceiling, rather than
        // scaling by a saturation ratio. The ratio version let a strongly
        // coloured room through — a violent blue washed a grey surface by 63
        // units, which the test caught. Bounding the deviation bounds the
        // visible effect, which is the thing 0.18 is actually about.
        let worst = dev(self.r).abs().max(dev(self.g).abs()).max(dev(self.b).abs());
        let allowed = if worst > 1e-6 { (TINT_MAX_SAT / worst).min(1.0) } else { 1.0 };
        let k = strength.clamp(0.0, 1.0) * allowed;
        let mut out = [0u8; 3];
        for (i, ch) in [self.r, self.g, self.b].iter().enumerate() {
            // Scaled, never clipped: clipping a channel shifts the hue, so the
            // room would come out the wrong colour rather than merely faint.
            let scale = 1.0 + dev(*ch) * k;
            out[i] = (base[i] as f32 * scale).round().clamp(0.0, 255.0) as u8;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perception_is_logarithmic_not_linear() {
        // A controller linear in lux is aggressive in the dark and sluggish
        // outdoors. The step from a dim room to a lit one must matter more than
        // the same lux step outdoors.
        let indoor_step = perceptual(200.0) - perceptual(100.0);
        let outdoor_step = perceptual(50_100.0) - perceptual(50_000.0);
        assert!(indoor_step > outdoor_step * 10.0, "{indoor_step} vs {outdoor_step}");
    }

    #[test]
    fn darkness_is_finite() {
        // log10(0) is negative infinity, and a controller fed that rails
        // instantly rather than dimming.
        assert_eq!(perceptual(0.0), 0.0);
        assert!(perceptual(f32::NAN).is_finite());
        assert!((0.0..=1.01).contains(&perceptual(100_000.0)));
    }

    #[test]
    fn a_hand_over_the_glass_is_a_cover_and_a_darker_room_is_not() {
        // The differential is the whole point: a level threshold cannot tell
        // these apart, and the rate can.
        let mut d = Differ::default();
        d.update(Lux(300.0), 16.0);
        assert_eq!(d.update(Lux(20.0), 16.0), LightEvent::Covered);

        let mut room = Differ::default();
        room.update(Lux(300.0), 16.0);
        // the same total fall, taken slowly
        let mut v = 300.0;
        let mut sawcover = false;
        for _ in 0..40 {
            v *= 0.93;
            if room.update(Lux(v), 500.0) == LightEvent::Covered { sawcover = true; }
        }
        assert!(!sawcover, "walking into shade read as a cover");
    }

    #[test]
    fn uncovering_is_measured_against_the_level_it_fell_from() {
        // Comparing against the covered level would make a slow brightening
        // under a hand read as uncovering.
        let mut d = Differ::default();
        d.update(Lux(300.0), 16.0);
        d.update(Lux(20.0), 16.0);
        assert!(d.is_covered());
        assert_eq!(d.update(Lux(40.0), 16.0), LightEvent::Steady, "uncovered too eagerly");
        assert_eq!(d.update(Lux(280.0), 16.0), LightEvent::Uncovered);
    }

    #[test]
    fn the_cover_test_is_fractional_so_it_works_indoors_and_out() {
        // A 50 lx fall is a cover indoors and noise outdoors.
        let mut bright = Differ::default();
        bright.update(Lux(50_000.0), 16.0);
        assert_ne!(bright.update(Lux(49_950.0), 16.0), LightEvent::Covered);
        let mut dim = Differ::default();
        dim.update(Lux(80.0), 16.0);
        assert_eq!(dim.update(Lux(30.0), 16.0), LightEvent::Covered);
    }

    #[test]
    fn a_bad_reading_does_not_produce_an_event() {
        let mut d = Differ::default();
        d.update(Lux(200.0), 16.0);
        assert_eq!(d.update(Lux(f32::NAN), 16.0), LightEvent::Steady);
        assert!(!d.is_covered());
    }

    #[test]
    fn the_controller_settles_without_overshooting() {
        // The failure this whole module is against: a screen that hunts.
        let mut b = Backlight::default();
        let target = 0.8;
        let mut peak: f32 = 0.0;
        for _ in 0..600 {
            let out = b.step(target, b.output(), 1.0 / 60.0);
            peak = peak.max(out);
        }
        assert!((b.output() - target).abs() < 0.05, "did not settle: {}", b.output());
        assert!(peak <= target + 0.02, "overshot to {peak}");
    }

    #[test]
    fn the_integral_does_not_wind_up_at_a_rail() {
        // At the rail the error persists; a naive integrator accumulates and
        // then unwinds into a big overshoot when the light finally changes.
        let mut b = Backlight::default();
        for _ in 0..2000 { b.step(1.0, 0.0, 1.0 / 60.0); }
        assert!(b.integral().abs() <= 1.0, "unbounded: {}", b.integral());
        // and coming off the rail does not fling the output
        let mut swing: f32 = 0.0;
        let mut prev = b.output();
        for _ in 0..120 {
            let out = b.step(0.2, 0.2, 1.0 / 60.0);
            swing = swing.max((out - prev).abs());
            prev = out;
        }
        assert!(swing < 0.05, "unwound violently: {swing} per frame");
    }

    #[test]
    fn a_setpoint_change_does_not_kick_the_derivative() {
        // Differentiating the ERROR spikes on every setpoint change for no
        // physical reason; this differentiates the measurement.
        let mut b = Backlight::default();
        for _ in 0..60 { b.step(0.5, 0.5, 1.0 / 60.0); }
        let before = b.output();
        let after = b.step(0.9, 0.5, 1.0 / 60.0);
        assert!((after - before).abs() < 0.05, "kicked {:.3} in one frame", after - before);
    }

    #[test]
    fn corrections_below_the_jnd_are_not_made_at_all() {
        // Invisible correction, visible motion — this is what makes a screen
        // shimmer while it "converges".
        let mut b = Backlight::default();
        let start = b.output();
        for _ in 0..300 { b.step(start + DEADBAND * 0.5, start, 1.0 / 60.0); }
        assert_eq!(b.output(), start, "shimmered inside the deadband");
    }

    #[test]
    fn brightening_is_faster_than_dimming() {
        // Walking into sunlight you cannot read the screen; walking into shade
        // a bright screen is merely non-ideal, and eager dimming reads as
        // broken.
        let mut up = Backlight::default();
        up.adopt(0.1);
        for _ in 0..30 { up.step(1.0, 0.1, 1.0 / 60.0); }
        let risen = up.output() - 0.1;

        let mut down = Backlight::default();
        down.adopt(0.9);
        for _ in 0..30 { down.step(0.0, 0.9, 1.0 / 60.0); }
        let fallen = 0.9 - down.output();
        assert!(risen > fallen * 2.0, "risen {risen} vs fallen {fallen}");
    }

    #[test]
    fn a_user_slider_is_adopted_rather_than_fought() {
        // An integral accumulated against the old target would drag the screen
        // back off the value they just chose.
        let mut b = Backlight::default();
        for _ in 0..600 { b.step(1.0, 0.0, 1.0 / 60.0); }
        b.adopt(0.3);
        assert_eq!(b.output(), 0.3);
        assert_eq!(b.integral(), 0.0);
        let after = b.step(0.3, 0.3, 1.0 / 60.0);
        assert_eq!(after, 0.3, "fought the user's choice");
    }

    #[test]
    fn a_zero_or_absurd_timestep_is_ignored() {
        let mut b = Backlight::default();
        let o = b.output();
        assert_eq!(b.step(1.0, 0.0, 0.0), o);
        assert_eq!(b.step(1.0, 0.0, f32::NAN), o);
    }

    #[test]
    fn colour_temperature_reads_warm_and_cool_light_apart() {
        let tungsten = cct_kelvin(AmbientRgb { r: 1.0, g: 0.72, b: 0.42 }).expect("warm");
        let daylight = cct_kelvin(AmbientRgb { r: 0.95, g: 1.0, b: 1.05 }).expect("cool");
        assert!(tungsten < daylight, "{tungsten}K should be warmer than {daylight}K");
    }

    #[test]
    fn a_dark_sensor_reports_no_temperature_rather_than_a_confident_wrong_one() {
        // The ratio is noise there, and a confident 2700 K from noise tints the
        // screen for no reason.
        assert_eq!(cct_kelvin(AmbientRgb { r: 0.0, g: 0.0, b: 0.0 }), None);
        assert_eq!(cct_kelvin(AmbientRgb { r: 1e-5, g: 1e-5, b: 1e-5 }), None);
    }

    #[test]
    fn the_white_point_moves_partway_and_stays_inside_its_bounds() {
        // A display matched exactly to a 2000 K room is unusably orange; the
        // aim is to reduce the mismatch, not erase it.
        let native = 6500.0;
        let warm = target_cct(Some(2000.0), native);
        assert!(warm > CCT_MIN_K - 1.0 && warm < native, "{warm}K");
        assert!(warm > 2000.0, "went all the way to the room: {warm}K");
        assert_eq!(target_cct(None, native), native, "no reading, no shift");
        for room in [500.0, 1_000_000.0] {
            let t = target_cct(Some(room), native);
            assert!((CCT_MIN_K..=CCT_MAX_K).contains(&t), "{room} -> {t}");
        }
    }

    #[test]
    fn the_theme_tint_shares_the_fleets_contrast_ceiling() {
        // ambient_palette and ambient_blobs both cap at 0.18; a third feature
        // arriving at the same number is the sign it is a real constraint.
        assert_eq!(TINT_MAX_SAT, 0.18);
        let mut t = Tint::default();
        // a violently blue room
        for _ in 0..2000 { t.update(Some(AmbientRgb { r: 0.05, g: 0.1, b: 1.0 }), 1.0 / 60.0); }
        let base = [128u8, 128, 128];
        let out = t.apply(base, 1.0);
        // the tint leans blue but does not wash the surface
        assert!(out[2] > out[0], "no blue lean: {out:?}");
        let shift = (out[2] as i32 - out[0] as i32).abs();
        assert!(shift < 60, "washed the theme: {out:?}");
    }

    #[test]
    fn the_tint_is_scaled_rather_than_clipped() {
        // Clipping a channel shifts the hue, which is worse than a weaker
        // tint: the room would come out the wrong colour rather than faint.
        let mut t = Tint::default();
        for _ in 0..2000 { t.update(Some(AmbientRgb { r: 1.0, g: 0.0, b: 0.0 }), 1.0 / 60.0); }
        let out = t.apply([200, 200, 200], 1.0);
        assert!(out[0] >= out[1] && out[1] == out[2], "hue drifted off red: {out:?}");
        assert!(out.iter().all(|c| *c > 0), "clipped to black: {out:?}");
    }

    #[test]
    fn walking_past_a_blue_shopfront_does_not_repaint_the_app() {
        // Slow by design: the tint should be something you notice having
        // happened, not something you watch happen.
        let mut t = Tint::default();
        for _ in 0..120 { t.update(Some(AmbientRgb { r: 0.4, g: 0.35, b: 0.25 }), 1.0 / 60.0); }
        let (wr, _, wb) = t.rgb();
        // two seconds of blue light on the way past
        for _ in 0..120 { t.update(Some(AmbientRgb { r: 0.1, g: 0.2, b: 0.7 }), 1.0 / 60.0); }
        let (nr, _, nb) = t.rgb();
        assert!(nb > wb, "did not lean at all");
        assert!(nb - wb < 0.1, "repainted in two seconds: {wb} -> {nb}");
        assert!(nr < wr);
    }

    #[test]
    fn an_unreadable_sample_holds_the_tint_rather_than_decaying_it() {
        // A failed read is not evidence that the room went grey.
        let mut t = Tint::default();
        for _ in 0..600 { t.update(Some(AmbientRgb { r: 0.2, g: 0.3, b: 0.5 }), 1.0 / 60.0); }
        let before = t.rgb();
        for _ in 0..600 { t.update(None, 1.0 / 60.0); }
        assert_eq!(t.rgb(), before);
    }
}
