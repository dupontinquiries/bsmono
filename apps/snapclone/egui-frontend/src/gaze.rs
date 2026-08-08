//! gaze — turning a gaze vector into hands-free pointing.
//!
//! `gaze-core` is a headless selfie-camera engine: frame in, `TrackingResult`
//! out, carrying a **unit gaze vector in camera space and a confidence**. It
//! owns no camera and no thread. What it does not carry — deliberately — is a
//! screen coordinate, because that depends on where the screen is relative to
//! the eye, which is a per-session calibration rather than a property of the
//! tracker.
//!
//! This module is the consumption layer: vector → screen point → target →
//! activation.
//!
//! ## It is NOT a second cursor
//!
//! Glove mode already has targets, a generous snap radius, a spoken vocabulary
//! and a confirmation type that refuses "yes". Building a parallel gaze cursor
//! would duplicate all of it and the two would drift — the same failure this
//! programme keeps finding in the Qt code. So gaze produces a **point**, and
//! `glove::GloveCursor`'s target machinery does the rest. Everything below is
//! about producing that point honestly.
//!
//! ## Three things stand between a gaze vector and a usable pointer
//!
//! **1. Saccades.** The eye does not glide; it jumps, several times a second,
//! and between jumps it never truly stops. A cursor that follows raw gaze is
//! unusable — it is not jitter to be smoothed but structure to be recognised.
//! `Fixation` uses dispersion over a window: while the samples stay inside a
//! small box the eye is *looking at* something, and only then is there a point
//! worth reporting.
//!
//! **2. Blinks and glare.** A blink is not a gaze at the floor; it is the
//! absence of a measurement, and the tracker reports it as low confidence.
//! Feeding that through as a position throws the cursor across the screen every
//! few seconds. Low confidence FREEZES the pointer rather than moving it.
//!
//! **3. The Midas touch.** If looking at a thing activates it, you cannot look
//! at anything. This is the oldest problem in gaze interaction and it has no
//! clever solution — only an explicit one: activation requires a **dwell**, the
//! dwell belongs to a **target** rather than a pixel, and there is always a way
//! to look at something without arming it.
//!
//! ## Dwell belongs to the target, not the pixel
//!
//! This is the difference between a gaze UI that works and one that does not.
//! Human fixation drifts by a degree or so even when "still", so a dwell timer
//! keyed to a screen position restarts constantly and never completes. Keyed to
//! the snapped TARGET, the drift is irrelevant: the timer runs as long as the
//! eye stays on the same button, however much it wanders inside it.

use uikit_egui::glove::Target;

/// How long a dwell must hold before it activates.
///
/// 600ms is the usual floor in the literature: below about 400ms people trigger
/// things by reading, above about a second the interface feels like wading.
pub const DWELL_MS: f32 = 600.0;

/// Confidence below which the tracker's output is not a measurement.
pub const MIN_CONFIDENCE: f64 = 0.35;

/// Dispersion window for fixation detection, in milliseconds.
pub const FIXATION_WINDOW_MS: f32 = 120.0;

/// How far samples may spread and still count as one fixation, in pixels.
///
/// Generous, because gaze accuracy is about a degree of visual angle — roughly
/// 40px at phone distance on a phone screen. A tighter box would never register
/// a fixation at all, which looks like the tracker being broken.
pub const FIXATION_DISPERSION_PX: f32 = 60.0;

/// One sample from `gaze-core`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GazeSample {
    /// Unit gaze in camera space: +x right, +y up, +z toward the camera.
    pub gaze: [f64; 3],
    pub confidence: f64,
    pub t_ms: f64,
}

/// Maps a gaze vector to a screen point.
///
/// An affine fit rather than anything cleverer, and the reason is honesty: with
/// five to nine calibration points, a full projective model overfits the noise
/// and behaves worse away from the points. Affine degrades gracefully — it is
/// wrong by a slowly-varying amount rather than wildly wrong in the corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    /// x_screen = ax * gx + bx * gy + cx
    pub ax: f64, pub bx: f64, pub cx: f64,
    /// y_screen = ay * gx + by * gy + cy
    pub ay: f64, pub by: f64, pub cy: f64,
    /// Mean residual of the fit, in pixels. Reported, not hidden.
    pub residual_px: f64,
}

impl Default for Calibration {
    /// The identity-ish default is deliberately USELESS rather than plausible.
    ///
    /// An uncalibrated gaze pointer that lands somewhere reasonable is worse
    /// than one that obviously does not work: the user tries to compensate,
    /// fails, and blames themselves. `residual_px` is infinite so
    /// `is_usable()` refuses it.
    fn default() -> Self {
        Calibration { ax: 0.0, bx: 0.0, cx: 0.0, ay: 0.0, by: 0.0, cy: 0.0,
                      residual_px: f64::INFINITY }
    }
}

/// Above this mean residual the calibration is not fit to point with.
///
/// Half a snap radius: past that, the pointer lands on the wrong target often
/// enough that dwell activates things the user was not looking at, which is
/// worse than telling them to calibrate again.
pub const MAX_RESIDUAL_PX: f64 = 36.0;

impl Calibration {
    pub fn is_usable(&self) -> bool { self.residual_px <= MAX_RESIDUAL_PX }

    /// Least-squares affine fit from (gaze, screen point) pairs.
    ///
    /// Needs at least three non-degenerate points. Returns `None` rather than a
    /// wild extrapolation when the points are collinear — a user who stared at
    /// one row of dots deserves "that did not work", not a pointer that only
    /// moves horizontally.
    pub fn fit(samples: &[([f64; 3], (f32, f32))]) -> Option<Calibration> {
        if samples.len() < 3 { return None; }
        // Solve the 3x3 normal equations for each axis.
        let (mut sxx, mut sxy, mut sx) = (0.0, 0.0, 0.0);
        let (mut syy, mut sy, mut n) = (0.0, 0.0, 0.0);
        for (g, _) in samples {
            sxx += g[0] * g[0]; sxy += g[0] * g[1]; sx += g[0];
            syy += g[1] * g[1]; sy += g[1]; n += 1.0;
        }
        let m = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]];
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        // Collinear or coincident points make this singular. Refusing is the
        // whole point: a near-singular solve produces enormous coefficients
        // that look like a working calibration until the eye moves.
        if det.abs() < 1e-9 { return None; }

        let solve = |rhs: [f64; 3]| -> [f64; 3] {
            let mut out = [0.0; 3];
            for i in 0..3 {
                let mut mi = m;
                for r in 0..3 { mi[r][i] = rhs[r]; }
                let d = mi[0][0] * (mi[1][1] * mi[2][2] - mi[1][2] * mi[2][1])
                      - mi[0][1] * (mi[1][0] * mi[2][2] - mi[1][2] * mi[2][0])
                      + mi[0][2] * (mi[1][0] * mi[2][1] - mi[1][1] * mi[2][0]);
                out[i] = d / det;
            }
            out
        };

        let mut rx = [0.0; 3];
        let mut ry = [0.0; 3];
        for (g, p) in samples {
            rx[0] += g[0] * p.0 as f64; rx[1] += g[1] * p.0 as f64; rx[2] += p.0 as f64;
            ry[0] += g[0] * p.1 as f64; ry[1] += g[1] * p.1 as f64; ry[2] += p.1 as f64;
        }
        let cx = solve(rx);
        let cy = solve(ry);
        let mut cal = Calibration { ax: cx[0], bx: cx[1], cx: cx[2],
                                    ay: cy[0], by: cy[1], cy: cy[2],
                                    residual_px: 0.0 };
        let mut total = 0.0;
        for (g, p) in samples {
            let (px, py) = cal.project(*g);
            total += (((px - p.0) as f64).powi(2) + ((py - p.1) as f64).powi(2)).sqrt();
        }
        cal.residual_px = total / samples.len() as f64;
        Some(cal)
    }

    pub fn project(&self, g: [f64; 3]) -> (f32, f32) {
        ((self.ax * g[0] + self.bx * g[1] + self.cx) as f32,
         (self.ay * g[0] + self.by * g[1] + self.cy) as f32)
    }
}

/// Dispersion-based fixation detection.
///
/// The eye jumps between fixations several times a second. Smoothing a saccade
/// produces a cursor that lags and then overshoots; recognising it produces a
/// cursor that simply does not move until the eye has landed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fixation {
    samples: Vec<((f32, f32), f64)>,
    /// The current fixation point, once one exists.
    point: Option<(f32, f32)>,
}

impl Fixation {
    /// Feed a projected point. Returns the fixation point, if the eye is on one.
    pub fn push(&mut self, p: (f32, f32), t_ms: f64) -> Option<(f32, f32)> {
        self.samples.push((p, t_ms));
        self.samples.retain(|(_, t)| t_ms - t <= FIXATION_WINDOW_MS as f64);
        if self.samples.len() < 3 { return self.point; }

        let xs: Vec<f32> = self.samples.iter().map(|(p, _)| p.0).collect();
        let ys: Vec<f32> = self.samples.iter().map(|(p, _)| p.1).collect();
        let spread = |v: &[f32]| {
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for x in v { lo = lo.min(*x); hi = hi.max(*x); }
            hi - lo
        };
        // Dispersion as the sum of the ranges — the classic I-DT measure. Cheap
        // and, more importantly, it does not average across a saccade the way a
        // variance-based test can.
        if spread(&xs) + spread(&ys) <= FIXATION_DISPERSION_PX {
            let n = self.samples.len() as f32;
            self.point = Some((xs.iter().sum::<f32>() / n, ys.iter().sum::<f32>() / n));
        } else {
            // Mid-saccade: no fixation. The pointer holds its last position
            // rather than smearing across the screen.
            self.point = None;
        }
        self.point
    }

    pub fn point(&self) -> Option<(f32, f32)> { self.point }
    pub fn reset(&mut self) { self.samples.clear(); self.point = None; }
}

/// What the gaze layer is asking for.
#[derive(Debug, Clone, PartialEq)]
pub enum GazeEvent {
    /// The pointer moved to a target. Not an activation.
    Hover(String),
    /// A dwell completed on this target.
    Activate(String),
    /// Nothing is being looked at.
    Idle,
}

/// Gaze pointing with dwell activation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GazePointer {
    pub calibration: Calibration,
    fixation: Fixation,
    /// Last good screen point. Held through blinks rather than reset.
    at: Option<(f32, f32)>,
    /// The target the dwell timer belongs to, and how long it has run.
    dwell_on: Option<String>,
    dwell_ms: f32,
    /// True once a dwell has fired on the current target, so it fires ONCE.
    fired: bool,
    /// Dwell activation is off until explicitly armed — see `set_dwell_armed`.
    dwell_armed: bool,
}

impl GazePointer {
    pub fn at(&self) -> Option<(f32, f32)> { self.at }
    pub fn dwell_progress(&self) -> f32 { (self.dwell_ms / DWELL_MS).clamp(0.0, 1.0) }
    pub fn dwell_target(&self) -> Option<&str> { self.dwell_on.as_deref() }

    /// Arm or disarm dwell activation.
    ///
    /// The answer to the Midas touch. With dwell disarmed the pointer still
    /// tracks and still highlights, so the user can SEE where the system thinks
    /// they are looking without anything being pressed — which is also how you
    /// read a screen. Arming is an explicit act (a switch, a word, a held key).
    pub fn set_dwell_armed(&mut self, armed: bool) {
        if self.dwell_armed == armed { return; }
        self.dwell_armed = armed;
        self.dwell_ms = 0.0;
        self.fired = false;
    }

    pub fn dwell_armed(&self) -> bool { self.dwell_armed }

    /// Feed one tracker sample.
    pub fn feed(&mut self, s: GazeSample, dt_ms: f32, targets: &[Target]) -> GazeEvent {
        // A calibration that does not fit is refused outright. A pointer that
        // lands plausibly-but-wrongly is worse than one that visibly does not
        // work: the user tries to compensate and blames themselves.
        if !self.calibration.is_usable() { return GazeEvent::Idle; }

        // A blink is the ABSENCE of a measurement, not a look at the floor.
        // Freeze rather than move, and stop the dwell — you cannot dwell on
        // something with your eyes closed.
        if s.confidence < MIN_CONFIDENCE {
            self.dwell_ms = 0.0;
            return match &self.dwell_on {
                Some(id) => GazeEvent::Hover(id.clone()),
                None => GazeEvent::Idle,
            };
        }

        let raw = self.calibration.project(s.gaze);
        let Some(p) = self.fixation.push(raw, s.t_ms) else {
            // Mid-saccade. Hold position and hold the dwell timer — restarting
            // it on every flick of the eye means dwell never completes.
            return match &self.dwell_on {
                Some(id) => GazeEvent::Hover(id.clone()),
                None => GazeEvent::Idle,
            };
        };
        self.at = Some(p);

        // Nearest target, using glove mode's own radius so gaze and the tilt
        // cursor agree about what "on" means.
        let hit = targets.iter()
            .map(|t| (t, (t.at.0 - p.0).hypot(t.at.1 - p.1)))
            .filter(|(_, d)| *d <= uikit_egui::glove::SNAP_RADIUS_PX)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(t, _)| t);

        let Some(t) = hit else {
            self.dwell_on = None;
            self.dwell_ms = 0.0;
            self.fired = false;
            return GazeEvent::Idle;
        };

        // THE rule: the dwell belongs to the TARGET, not the pixel. Fixation
        // drifts by a degree even when "still", so a timer keyed to position
        // restarts constantly and never completes.
        if self.dwell_on.as_deref() != Some(t.id.as_str()) {
            self.dwell_on = Some(t.id.clone());
            self.dwell_ms = 0.0;
            self.fired = false;
        }

        if !self.dwell_armed || self.fired { return GazeEvent::Hover(t.id.clone()); }

        self.dwell_ms += dt_ms;
        if self.dwell_ms >= DWELL_MS {
            // Fires once. A dwell that repeated would activate a button
            // continuously for as long as the user kept reading it.
            self.fired = true;
            return GazeEvent::Activate(t.id.clone());
        }
        GazeEvent::Hover(t.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uikit_egui::glove::TargetKind;

    fn target(id: &str, x: f32, y: f32) -> Target {
        Target { id: id.into(), kind: TargetKind::Button, at: (x, y), label: id.into() }
    }

    /// A calibration that maps gaze x/y straight to pixels, for testing.
    fn cal() -> Calibration {
        Calibration { ax: 1000.0, bx: 0.0, cx: 200.0,
                      ay: 0.0, by: -1000.0, cy: 400.0, residual_px: 4.0 }
    }

    fn armed() -> GazePointer {
        let mut g = GazePointer { calibration: cal(), ..Default::default() };
        g.set_dwell_armed(true);
        g
    }

    fn sample(gx: f64, gy: f64, t: f64) -> GazeSample {
        GazeSample { gaze: [gx, gy, 0.9], confidence: 0.9, t_ms: t }
    }

    /// Hold the eye at one spot long enough to fixate and dwell.
    fn stare(g: &mut GazePointer, gx: f64, gy: f64, ms: f64, ts: &[Target]) -> GazeEvent {
        let mut ev = GazeEvent::Idle;
        let mut t = 0.0;
        while t < ms {
            t += 16.0;
            ev = g.feed(sample(gx, gy, t), 16.0, ts);
            if matches!(ev, GazeEvent::Activate(_)) { return ev; }
        }
        ev
    }

    #[test]
    fn an_unfitted_calibration_refuses_to_point_at_all() {
        // A pointer that lands plausibly but wrongly is worse than one that
        // visibly does not work: the user tries to compensate and blames
        // themselves.
        let mut g = GazePointer::default();
        g.set_dwell_armed(true);
        let ts = vec![target("a", 200.0, 400.0)];
        assert_eq!(g.feed(sample(0.0, 0.0, 16.0), 16.0, &ts), GazeEvent::Idle);
        assert!(!GazePointer::default().calibration.is_usable());
    }

    #[test]
    fn a_calibration_worse_than_half_a_snap_radius_is_refused() {
        // Past that the pointer lands on the wrong target often enough that
        // dwell activates things nobody looked at.
        let mut c = cal();
        c.residual_px = MAX_RESIDUAL_PX + 1.0;
        assert!(!c.is_usable());
        c.residual_px = MAX_RESIDUAL_PX;
        assert!(c.is_usable());
    }

    #[test]
    fn calibration_needs_three_points_and_refuses_collinear_ones() {
        // A near-singular solve produces enormous coefficients that look like a
        // working calibration until the eye moves.
        assert!(Calibration::fit(&[([0.0, 0.0, 1.0], (0.0, 0.0))]).is_none());
        let collinear = [([0.0, 0.0, 1.0], (0.0, 0.0)),
                         ([0.1, 0.0, 1.0], (100.0, 0.0)),
                         ([0.2, 0.0, 1.0], (200.0, 0.0))];
        assert!(Calibration::fit(&collinear).is_none(), "one row of dots is not a calibration");
    }

    #[test]
    fn a_good_calibration_fits_and_reports_a_small_residual() {
        let pts = [([-0.1, 0.1, 1.0], (100.0, 300.0)),
                   ([0.1, 0.1, 1.0], (300.0, 300.0)),
                   ([-0.1, -0.1, 1.0], (100.0, 500.0)),
                   ([0.1, -0.1, 1.0], (300.0, 500.0))];
        let c = Calibration::fit(&pts).expect("fits");
        assert!(c.residual_px < 1.0, "residual {}", c.residual_px);
        assert!(c.is_usable());
        let (x, y) = c.project([0.1, 0.1, 1.0]);
        assert!((x - 300.0).abs() < 1.0 && (y - 300.0).abs() < 1.0);
    }

    #[test]
    fn the_residual_is_reported_rather_than_hidden() {
        // A calibration that silently fitted badly is how a gaze UI becomes
        // "it just doesn't work for me".
        let pts = [([-0.1, 0.1, 1.0], (100.0, 300.0)),
                   ([0.1, 0.1, 1.0], (300.0, 300.0)),
                   ([-0.1, -0.1, 1.0], (100.0, 500.0)),
                   ([0.1, -0.1, 1.0], (300.0, 900.0))];   // one bad point
        let c = Calibration::fit(&pts).expect("fits");
        assert!(c.residual_px > 10.0, "a bad point must show up: {}", c.residual_px);
    }

    #[test]
    fn a_saccade_produces_no_fixation() {
        // The eye jumps several times a second. A cursor that follows raw gaze
        // is unusable — this is structure to recognise, not jitter to smooth.
        let mut f = Fixation::default();
        f.push((100.0, 100.0), 0.0);
        f.push((400.0, 100.0), 16.0);
        assert_eq!(f.push((700.0, 100.0), 32.0), None);
    }

    #[test]
    fn a_steady_eye_produces_a_fixation_despite_drift() {
        // Human fixation is never perfectly still; a tight box would never
        // register one, which looks like a broken tracker.
        let mut f = Fixation::default();
        f.push((300.0, 300.0), 0.0);
        f.push((312.0, 296.0), 16.0);
        let p = f.push((305.0, 308.0), 32.0).expect("that is a fixation");
        assert!((p.0 - 305.0).abs() < 15.0 && (p.1 - 301.0).abs() < 15.0);
    }

    #[test]
    fn a_blink_freezes_the_pointer_instead_of_flinging_it() {
        // A blink is the absence of a measurement, not a look at the floor.
        let ts = vec![target("a", 200.0, 400.0)];
        let mut g = armed();
        stare(&mut g, 0.0, 0.0, 100.0, &ts);
        let before = g.at();
        let blink = GazeSample { gaze: [0.9, -0.9, 0.1], confidence: 0.05, t_ms: 200.0 };
        g.feed(blink, 16.0, &ts);
        assert_eq!(g.at(), before, "the pointer did not move");
    }

    #[test]
    fn a_blink_stops_the_dwell_because_you_cannot_dwell_with_your_eyes_shut() {
        let ts = vec![target("a", 200.0, 400.0)];
        let mut g = armed();
        stare(&mut g, 0.0, 0.0, DWELL_MS as f64 * 0.7, &ts);
        assert!(g.dwell_progress() > 0.0);
        let blink = GazeSample { gaze: [0.0, 0.0, 1.0], confidence: 0.05, t_ms: 999.0 };
        g.feed(blink, 16.0, &ts);
        assert_eq!(g.dwell_progress(), 0.0);
    }

    #[test]
    fn a_dwell_activates_the_target_it_belongs_to() {
        let ts = vec![target("send", 200.0, 400.0)];
        let mut g = armed();
        let ev = stare(&mut g, 0.0, 0.0, DWELL_MS as f64 + 200.0, &ts);
        assert_eq!(ev, GazeEvent::Activate("send".into()));
    }

    #[test]
    fn the_dwell_belongs_to_the_target_not_the_pixel() {
        // THE rule. Fixation drifts by a degree even when still, so a timer
        // keyed to a screen position restarts constantly and never completes.
        let ts = vec![target("send", 200.0, 400.0)];
        let mut g = armed();
        let mut t = 0.0;
        let mut fired = false;
        // Wander around inside the target the whole time.
        for i in 0..80 {
            t += 16.0;
            let jitter = (i % 5) as f64 * 0.004 - 0.008;
            if matches!(g.feed(sample(jitter, jitter, t), 16.0, &ts), GazeEvent::Activate(_)) {
                fired = true;
                break;
            }
        }
        assert!(fired, "drift inside one target must not restart the timer");
    }

    #[test]
    fn looking_away_to_another_target_restarts_the_dwell() {
        let ts = vec![target("a", 200.0, 400.0), target("b", 200.0, 200.0)];
        let mut g = armed();
        stare(&mut g, 0.0, 0.0, DWELL_MS as f64 * 0.8, &ts);
        assert!(g.dwell_progress() > 0.5);
        // Look at the other one.
        g.fixation.reset();
        stare(&mut g, 0.0, 0.2, 60.0, &ts);
        assert_eq!(g.dwell_target(), Some("b"));
        assert!(g.dwell_progress() < 0.5, "the timer started again");
    }

    #[test]
    fn a_dwell_fires_once_however_long_you_keep_looking() {
        // Otherwise a button activates continuously for as long as the user
        // keeps reading it.
        let ts = vec![target("send", 200.0, 400.0)];
        let mut g = armed();
        assert!(matches!(stare(&mut g, 0.0, 0.0, DWELL_MS as f64 + 100.0, &ts),
                         GazeEvent::Activate(_)));
        let after = stare(&mut g, 0.0, 0.0, DWELL_MS as f64 * 2.0, &ts);
        assert_eq!(after, GazeEvent::Hover("send".into()), "not a second activation");
    }

    #[test]
    fn disarmed_dwell_still_tracks_and_highlights_but_never_activates() {
        // The answer to the Midas touch: you must be able to LOOK at a screen
        // without pressing it, and to see where the system thinks you are
        // looking while doing so.
        let ts = vec![target("send", 200.0, 400.0)];
        let mut g = GazePointer { calibration: cal(), ..Default::default() };
        let ev = stare(&mut g, 0.0, 0.0, DWELL_MS as f64 * 3.0, &ts);
        assert_eq!(ev, GazeEvent::Hover("send".into()));
        assert_eq!(g.dwell_progress(), 0.0);
        assert!(g.at().is_some(), "but it is still tracking");
    }

    #[test]
    fn arming_resets_any_dwell_in_progress() {
        // Arming must not instantly fire on whatever the user happened to be
        // reading when they armed it.
        let ts = vec![target("send", 200.0, 400.0)];
        let mut g = armed();
        stare(&mut g, 0.0, 0.0, DWELL_MS as f64 * 0.9, &ts);
        g.set_dwell_armed(false);
        g.set_dwell_armed(true);
        assert_eq!(g.dwell_progress(), 0.0);
    }

    #[test]
    fn looking_at_nothing_is_idle_rather_than_the_nearest_thing() {
        // A pointer that always snaps somewhere means there is nowhere safe to
        // rest your eyes.
        let ts = vec![target("send", 200.0, 400.0)];
        let mut g = armed();
        let ev = stare(&mut g, 0.6, 0.0, 200.0, &ts);
        assert_eq!(ev, GazeEvent::Idle);
        assert_eq!(g.dwell_target(), None);
    }

    #[test]
    fn gaze_uses_the_same_snap_radius_as_the_tilt_cursor() {
        // Two pointing methods that disagree about what "on a button" means
        // would need two sets of targets and would drift apart.
        assert!(uikit_egui::glove::SNAP_RADIUS_PX > 0.0);
    }
}

// ── attention ───────────────────────────────────────────────────────────────

/// "Is the user looking at this screen?" — the cheap half of gaze.
///
/// Everything above this line is POINTING: calibrated, per-session, accurate to
/// about a degree, and — honestly — a rare interaction. Dwell-to-click is slower
/// than touch and tiring to sustain, so it earns its place for hands-free
/// operation and nothing else.
///
/// This is the half that gets used every day, and it is cheap in every
/// dimension pointing is expensive:
///
/// * **No calibration.** It asks whether a face is oriented at the screen, not
///   where on the screen. There is nothing to fit and nothing to drift.
/// * **No accuracy budget.** A degree of error is irrelevant to a yes/no.
/// * **Low rate.** Two or three samples a second is plenty, which matters
///   because the alternative is a selfie camera at 30fps eating the battery of
///   the phone it is trying to keep awake.
/// * **Cheap to be wrong.** The worst outcome is a screen that stays on a few
///   seconds longer.
///
/// ## What it is actually for
///
/// `PostureEngine` dims and then blanks the screen from proximity and
/// stillness. Sitting still, reading a long message, is indistinguishable from
/// not using the phone — so the screensaver fires while you are reading. Every
/// phone has this bug and everyone works around it by jabbing the screen.
/// Attention is the missing signal, and it is a VETO rather than a trigger: it
/// can keep a screen awake, and it can never dim one.
///
/// That asymmetry is deliberate. A false "looking" costs a few seconds of
/// backlight. A false "not looking" blanks the screen in someone's face, and
/// that is the failure people actually notice.
#[derive(Debug, Clone, PartialEq)]
pub struct Attention {
    /// Seconds of grace after the last confident look.
    ///
    /// Generous, because glancing away — at a keyboard, at someone talking to
    /// you — is not leaving. Shorter than the dim timeout so it never fights it.
    pub grace_ms: f32,
    since_seen_ms: f32,
    /// True once a look has been seen at all.
    ever: bool,
}

impl Default for Attention {
    fn default() -> Self {
        Attention { grace_ms: 4_000.0, since_seen_ms: f32::INFINITY, ever: false }
    }
}

/// Confidence floor for "a face is oriented at the screen".
///
/// Lower than `MIN_CONFIDENCE` for pointing on purpose: a blurry, half-turned,
/// badly-lit face is poor evidence of WHERE someone is looking and perfectly
/// good evidence THAT they are.
pub const ATTENTION_CONFIDENCE: f64 = 0.15;

/// How far off-axis the gaze may be and still count as at-screen.
///
/// The z component of the unit gaze points toward the camera. 0.5 is roughly
/// sixty degrees off-axis — deliberately loose, because someone reading the
/// bottom of a tall phone screen is genuinely looking well off the camera's
/// axis and must not be treated as having left.
pub const AT_SCREEN_Z: f64 = 0.5;

impl Attention {
    /// Feed a sample. Cheap enough to call at 2-3Hz.
    pub fn feed(&mut self, s: GazeSample, dt_ms: f32) {
        self.since_seen_ms = self.since_seen_ms.saturating_add_f32(dt_ms);
        if s.confidence >= ATTENTION_CONFIDENCE && s.gaze[2] >= AT_SCREEN_Z {
            self.since_seen_ms = 0.0;
            self.ever = true;
        }
    }

    /// No camera, or the tracker stopped. Attention becomes UNKNOWN, and
    /// unknown must not hold a screen awake forever.
    pub fn lost(&mut self) { self.since_seen_ms = f32::INFINITY; }

    /// Is the user looking, within the grace window?
    pub fn present(&self) -> bool { self.ever && self.since_seen_ms <= self.grace_ms }

    /// Time since the last confident look, for a UI that wants to explain
    /// itself rather than just behaving differently.
    pub fn since_seen_ms(&self) -> f32 { self.since_seen_ms }
}

trait SatAdd { fn saturating_add_f32(self, d: f32) -> f32; }
impl SatAdd for f32 {
    /// Infinity plus anything is infinity, and that is the wanted behaviour —
    /// but so is not overflowing a long session into NaN.
    fn saturating_add_f32(self, d: f32) -> f32 {
        if self.is_infinite() { self } else { (self + d).min(1.0e9) }
    }
}

#[cfg(test)]
mod attention_tests {
    use super::*;

    fn looking(t: f64) -> GazeSample {
        GazeSample { gaze: [0.1, -0.1, 0.95], confidence: 0.8, t_ms: t }
    }
    fn away(t: f64) -> GazeSample {
        GazeSample { gaze: [0.9, 0.1, 0.2], confidence: 0.8, t_ms: t }
    }

    #[test]
    fn attention_needs_no_calibration_to_answer() {
        // The whole reason this half is deployable: there is nothing to fit and
        // nothing to drift, so it works the first time on any face.
        let mut a = Attention::default();
        a.feed(looking(0.0), 300.0);
        assert!(a.present());
    }

    #[test]
    fn a_glance_away_is_not_leaving() {
        // Looking at a keyboard, or at someone talking to you, must not blank
        // the screen you are working on.
        let mut a = Attention::default();
        a.feed(looking(0.0), 300.0);
        a.feed(away(300.0), 1_500.0);
        assert!(a.present(), "still within grace");
    }

    #[test]
    fn actually_leaving_ends_attention() {
        let mut a = Attention::default();
        a.feed(looking(0.0), 300.0);
        a.feed(away(300.0), a.grace_ms + 100.0);
        assert!(!a.present());
    }

    #[test]
    fn a_dim_face_still_counts_as_present() {
        // Poor evidence of WHERE someone is looking is perfectly good evidence
        // THAT they are — which is why this threshold is below the pointing one.
        let mut a = Attention::default();
        a.feed(GazeSample { gaze: [0.0, 0.0, 0.9], confidence: 0.2, t_ms: 0.0 }, 300.0);
        assert!(a.present());
        assert!(ATTENTION_CONFIDENCE < MIN_CONFIDENCE);
    }

    #[test]
    fn reading_the_bottom_of_a_tall_screen_still_counts() {
        // Someone reading the bottom of a phone is genuinely well off the
        // camera's axis. A tight cone would call that "left the room".
        let mut a = Attention::default();
        a.feed(GazeSample { gaze: [0.0, -0.75, 0.66], confidence: 0.7, t_ms: 0.0 }, 300.0);
        assert!(a.present());
    }

    #[test]
    fn a_phone_that_never_saw_anyone_is_not_present() {
        // Otherwise a device with no camera would hold its screen on forever.
        let a = Attention::default();
        assert!(!a.present());
    }

    #[test]
    fn losing_the_tracker_ends_attention_rather_than_freezing_it() {
        // Unknown must not hold a screen awake indefinitely.
        let mut a = Attention::default();
        a.feed(looking(0.0), 300.0);
        assert!(a.present());
        a.lost();
        assert!(!a.present());
    }

    #[test]
    fn a_long_session_does_not_overflow_into_nonsense() {
        let mut a = Attention::default();
        a.feed(looking(0.0), 300.0);
        for _ in 0..100_000 { a.feed(away(0.0), 1_000.0); }
        assert!(a.since_seen_ms().is_finite());
        assert!(!a.present());
    }
}
