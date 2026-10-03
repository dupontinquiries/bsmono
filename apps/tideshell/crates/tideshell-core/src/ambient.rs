//! Auto-brightness from ambient light and time of day. Pure: callers feed
//! lux readings, the clock (minute of the local day) and the slider, and
//! apply the brightness values it returns (at up to 90 Hz while moving).
//!
//! ```text
//!  lux ─► darkening gate ─► Holt filter (level L, trend T, residual σ²)
//!                                   │
//!             ┌─────────────────────┼──────────────────────────┐
//!   centre c = curve(L)·night   band half-width w          math offset o1
//!                               (widens with σ and |T|,    (forward prediction
//!                                fast; contracts slowly)    curve(L+T·H) − curve(L),
//!                                                           weighted by instability,
//!                                                           decays once stable)
//!   target = c + o1 + u·w        u = slider position in the band, −1..1
//!   output: critically damped spring toward target, speed-limited
//! ```
//!
//! * **Centre**: lux → brightness is piecewise-linear in `log10(lux + 1)`
//!   (perception is ~logarithmic), times the night-window factor.
//! * **Input**: brightening is followed fast (TAU_UP); a darker reading must
//!   persist DARKEN_DELAY_MS before it counts, then is followed slowly
//!   (TAU_DOWN), so a hand or shadow over the sensor does not dim the screen.
//! * **Holt filter** (level + trend) on log-lux, dt-aware so irregular sensor
//!   rates give the same result; residual variance tracks noise.
//! * **Stability** s = exp(−|T|/T_REF − σ/S_REF): 1 when light is steady.
//! * **Offset 1, from the maths**: while light is changing, lean toward where
//!   it is heading (forward prediction over HORIZON_S) in proportion to the
//!   instability; engages quickly, and decays back to 0 as stability returns
//!   (its time constant lengthens with stability: decay tracking).
//! * **Offset 2, from the slider**: the slider is relative. Its travel spans
//!   the adaptive band, centre = auto; the position u ∈ −1..1 is stored, so
//!   the offset in brightness units (u·w) scales with the band.
//! * **Band** half-width w: W_MIN plus terms for noise (σ) and trend, mapped
//!   into brightness units by the curve's local slope; expands fast,
//!   contracts slowly.
//! * **Output**: a critically damped spring (exact closed form, frame-rate
//!   independent, no overshoot) with a speed limit, emitting every visible
//!   step (EMIT_STEP) — ~90 small steps a second during a change instead of
//!   a few large ones. There is no dead band: it made steps.

use crate::Millis;

/// (log10(lux + 1), brightness) anchor points.
const CURVE: [(f32, f32); 6] = [
    (0.0, 0.03), // pitch dark
    (1.0, 0.12), // ~10 lux: dim room at night
    (2.0, 0.32), // ~100 lux: living room
    (2.7, 0.50), // ~500 lux: office
    (3.5, 0.78), // ~3 000 lux: overcast outdoors
    (4.3, 1.00), // ~20 000 lux: daylight
];

pub fn lux_curve(lux: f32) -> f32 {
    let x = (lux.max(0.0) + 1.0).log10();
    if x <= CURVE[0].0 {
        return CURVE[0].1;
    }
    for w in CURVE.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    CURVE[CURVE.len() - 1].1
}

/// Wall-clock night window, minutes after local midnight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NightWindow {
    pub start: u32,
    pub end: u32,
    /// Fade length on each side, minutes.
    pub fade: u32,
    /// Multiplier at full night.
    pub dim: f32,
}

impl Default for NightWindow {
    fn default() -> Self {
        NightWindow { start: 22 * 60, end: 7 * 60, fade: 60, dim: 0.6 }
    }
}

impl NightWindow {
    /// Minutes from `a` forward to `b` on a 24 h clock.
    fn fwd(a: u32, b: u32) -> u32 {
        (b + 1440 - a) % 1440
    }

    /// 1.0 by day, `dim` at night, linear fades of `fade` minutes before
    /// `start` and after `end`. Works across midnight.
    pub fn factor(&self, minute: u32) -> f32 {
        let m = minute % 1440;
        let night_len = Self::fwd(self.start, self.end);
        let into_night = Self::fwd(self.start, m);
        if into_night < night_len {
            return self.dim;
        }
        let f = self.fade.max(1) as f32;
        let to_start = Self::fwd(m, self.start) as f32; // fading in
        let since_end = Self::fwd(self.end, m) as f32; // fading out
        if to_start < f {
            return 1.0 + (self.dim - 1.0) * (1.0 - to_start / f);
        }
        if since_end < f {
            return self.dim + (1.0 - self.dim) * (since_end / f);
        }
        1.0
    }

    /// Parse `HH:MM-HH:MM`.
    pub fn parse_span(s: &str) -> Option<(u32, u32)> {
        let (a, b) = s.trim().split_once('-')?;
        let hm = |t: &str| -> Option<u32> {
            let (h, m) = t.trim().split_once(':')?;
            let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
            (h < 24 && m < 60).then_some(h * 60 + m)
        };
        Some((hm(a)?, hm(b)?))
    }

    pub fn format_span(&self) -> String {
        format!("{:02}:{:02}-{:02}:{:02}", self.start / 60, self.start % 60, self.end / 60, self.end % 60)
    }
}

pub const MIN_LEVEL: f32 = 0.02;
pub const TAU_UP: f32 = 1.2;
pub const TAU_DOWN: f32 = 6.0;
pub const DARKEN_DELAY_MS: Millis = 3000;
/// Trend and residual-variance smoothing, seconds.
pub const TAU_TREND: f32 = 2.0;
pub const TAU_VAR: f32 = 4.0;
/// A trend overtaken by the input decays with this; any trend with the other.
pub const TREND_SPENT_TAU: f32 = 0.3;
pub const TREND_DAMP_TAU: f32 = 6.0;
/// Forward-prediction horizon, seconds.
pub const HORIZON_S: f32 = 1.5;
/// Stability scales: trend (log-lux/s) and noise (log-lux σ) that count as "changing".
pub const T_REF: f32 = 0.08;
pub const S_REF: f32 = 0.08;
/// Band half-width limits, and its expand / contract time constants.
pub const W_MIN: f32 = 0.06;
pub const W_MAX: f32 = 0.35;
pub const TAU_W_EXPAND: f32 = 0.4;
pub const TAU_W_CONTRACT: f32 = 5.0;
/// Output spring (rad/s) and speed limit (brightness units per second).
pub const OMEGA: f32 = 7.0;
pub const MAX_SPEED: f32 = 0.8;
/// Emit when the output moved at least this much (~one step of a 12-bit backlight x 6).
pub const EMIT_STEP: f32 = 0.0015;
pub const SENSOR_STALE_MS: Millis = 15_000;

// ---- v2: decide statistically, change rarely, move smoothly ---------------------------------
/// Readings from this long ago are the sample the decision is made on.
pub const V2_WINDOW_MS: Millis = 12_000;
/// Fewest readings in the window before anything is decided.
pub const V2_MIN_N: usize = 6;
/// Each tail trimmed from the window (a hand or a shadow is a tail): a trimmed mean.
pub const V2_TRIM: f32 = 0.10;
/// |t| the window's mean must reach against the last committed light to count as a real change.
pub const V2_T_CRIT: f32 = 2.5;
/// The noise floor on log10(lux), so a very steady sensor does not make every wobble "significant".
pub const V2_SIGMA_FLOOR: f32 = 0.04;
/// Least time between two commits, and the least brightness change worth making (about 2% of the range).
pub const V2_MIN_UPDATE_MS: Millis = 5_000;
pub const V2_MIN_VISIBLE: f32 = 0.02;
/// The slider band's fixed half-width in v2 (no adaptive band).
pub const V2_HALF_WIDTH: f32 = 0.12;
/// Output rate while gliding (30 updates a second) and the least brightness step worth sending.
pub const V2_EMIT_MS: Millis = 33;
pub const V2_EMIT_STEP: f32 = 0.0004;
/// Glide length: per unit of brightness distance, within these limits (seconds), smoothstep eased.
pub const V2_GLIDE_PER_UNIT_S: f32 = 4.0;
pub const V2_GLIDE_MIN_S: f32 = 0.8;
pub const V2_GLIDE_MAX_S: f32 = 3.0;
/// A new target closer than this to the one already gliding to is the same target.
pub const V2_RETARGET_EPS: f32 = 0.004;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algo {
    /// The Holt filter + adaptive band + spring (the original).
    #[default]
    V1,
    /// Window statistics + t-test + rate limit + lerp.
    V2,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct V2 {
    /// (time, log10(lux + 1)) over the last `V2_WINDOW_MS`.
    samples: std::collections::VecDeque<(Millis, f32)>,
    /// The light (log10 lux) the screen was last set for, and when.
    committed: Option<f32>,
    last_commit: Option<Millis>,
    /// The glide in progress: (from, to, start ms, duration s).
    glide: Option<(f32, f32, Millis, f32)>,
    last_user: f32,
    last_emit_ms: Option<Millis>,
}

/// Trimmed mean, standard deviation and count of `v` (sorted in place).
fn trimmed_stats(v: &mut [f32]) -> Option<(f32, f32, usize)> {
    if v.len() < V2_MIN_N {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let cut = ((v.len() as f32) * V2_TRIM).floor() as usize;
    let t = &v[cut..v.len() - cut];
    let n = t.len() as f32;
    let mean = t.iter().sum::<f32>() / n;
    let var = t.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / (n - 1.0).max(1.0);
    Some((mean, var.sqrt(), t.len()))
}

fn smoothstep(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}
/// Above this the shell switches its own surfaces to a sunlight (high
/// contrast) look; the FP5 panel is not readable outdoors on brightness alone.
pub const SUNLIGHT_LUX: f32 = 10_000.0;
pub const CONTRAST_START_LUX: f32 = 2_500.0;

/// 0 below the start, 1 at full sunlight, smoothstep between (in log lux).
pub fn contrast_for_lux(lux: f32) -> f32 {
    let (a, b) = (CONTRAST_START_LUX.ln(), SUNLIGHT_LUX.ln());
    let x = ((lux.max(1.0).ln() - a) / (b - a)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
/// Absolute-slider curve: level = pos^SLIDER_GAMMA, so the lower half of the
/// slider's travel covers the bottom ~20% of brightness (night levels get
/// the resolution; the top is where a coarse control is fine).
pub const SLIDER_GAMMA: f32 = 2.2;

pub fn slider_to_level(pos: f32) -> f32 {
    pos.clamp(0.0, 1.0).powf(SLIDER_GAMMA)
}

pub fn level_to_slider(level: f32) -> f32 {
    level.clamp(0.0, 1.0).powf(1.0 / SLIDER_GAMMA)
}
pub const DAY_LEVEL_NO_SENSOR: f32 = 0.6;

/// The curve on the log axis, and its local slope (brightness per log10-lux).
fn curve_x(x: f32) -> f32 {
    lux_curve(10f32.powf(x) - 1.0)
}
fn slope_x(x: f32) -> f32 {
    (curve_x(x + 0.05) - curve_x((x - 0.05).max(0.0))) / 0.1
}

/// One exact step of a critically damped spring. `x` is the offset from the
/// target, `v` its velocity; returns (x, v) after `dt` seconds.
pub fn spring_step(x: f32, v: f32, omega: f32, dt: f32) -> (f32, f32) {
    let e = (-omega * dt).exp();
    let k = v + omega * x;
    ((x + k * dt) * e, (v - omega * k * dt) * e)
}

/// Everything the slider draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    /// Auto level (sensor/time of day), before offsets.
    pub center: f32,
    pub low: f32,
    pub high: f32,
    /// center + offsets, clamped: where the output is heading.
    pub target: f32,
    pub applied: Option<f32>,
    pub math_offset: f32,
    /// u * w, in brightness units.
    pub user_offset: f32,
    pub stability: f32,
    /// log10-lux per second.
    pub trend: f32,
    /// Bright enough outdoors that the shell should draw for sunlight.
    pub sunlight: bool,
    /// How far into the sunlight look to blend, 0..1: smooth from
    /// CONTRAST_START_LUX to SUNLIGHT_LUX, so the shell's contrast rises
    /// with the light instead of switching.
    pub contrast: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoBrightness {
    pub enabled: bool,
    /// Slider position within the adaptive band, −1..1 (0 = the auto centre).
    pub user: f32,
    pub night: NightWindow,
    level: Option<f32>,
    trend: f32,
    var: f32,
    last_reading: Option<Millis>,
    darker_since: Option<Millis>,
    math_offset: f32,
    half_width: f32,
    pub applied: Option<f32>,
    vel: f32,
    last_emitted: Option<f32>,
    last_target: Option<f32>,
    last_tick: Option<Millis>,
    /// Which algorithm runs (`Algo`).
    pub algo: Algo,
    v2: V2,
}

impl Default for AutoBrightness {
    fn default() -> Self {
        AutoBrightness {
            enabled: false,
            user: 0.0,
            night: NightWindow::default(),
            level: None,
            trend: 0.0,
            var: 0.0,
            last_reading: None,
            darker_since: None,
            math_offset: 0.0,
            half_width: W_MIN,
            applied: None,
            vel: 0.0,
            last_emitted: None,
            last_target: None,
            last_tick: None,
            algo: Algo::V1,
            v2: V2::default(),
        }
    }
}

impl AutoBrightness {
    pub fn set_current(&mut self, level: f32) {
        self.applied = Some(level.clamp(0.0, 1.0));
        self.last_emitted = self.applied;
        self.vel = 0.0;
    }

    pub fn has_sensor(&self, now: Millis) -> bool {
        self.last_reading.is_some_and(|t| now.saturating_sub(t) < SENSOR_STALE_MS) && self.level.is_some()
    }

    pub fn smoothed_lux(&self) -> Option<f32> {
        self.level.map(|l| 10f32.powf(l) - 1.0)
    }

    pub fn stability(&self) -> f32 {
        (-(self.trend.abs() / T_REF) - (self.var.max(0.0).sqrt() / S_REF)).exp()
    }

    pub fn half_width(&self) -> f32 {
        self.half_width
    }

    pub fn math_offset(&self) -> f32 {
        self.math_offset
    }

    pub fn set_user(&mut self, u: f32) {
        self.user = u.clamp(-1.0, 1.0);
    }

    /// With auto off, the slider is absolute: just adopt the level.
    pub fn set_absolute(&mut self, level: f32) {
        self.set_current(level);
    }

    pub fn on_lux(&mut self, lux: f32, now: Millis) {
        let x = (lux.max(0.0) + 1.0).log10();
        if self.algo == Algo::V2 {
            return self.on_lux_v2(x, now);
        }
        match (self.level, self.last_reading) {
            (Some(l), Some(t)) => {
                let dt = (now.saturating_sub(t) as f32 / 1000.0).max(1e-3);
                // The noise estimate sees EVERY reading, including darker ones
                // the gate holds back: the gate stops the level following a
                // dip, but flicker must still widen the band. (With the update
                // after the gate, light flickering below its level - 480/120
                // lux - left the band at its minimum; seen in the simulator.)
                let resid0 = x - (l + self.trend * dt);
                self.var += (resid0 * resid0 - self.var) * (1.0 - (-dt / TAU_VAR).exp());
                if x < l {
                    let since = *self.darker_since.get_or_insert(now);
                    if now.saturating_sub(since) < DARKEN_DELAY_MS {
                        self.last_reading = Some(now);
                        return; // a shadow, or real darkening? wait.
                    }
                } else {
                    self.darker_since = None;
                }
                let tau = if x >= l { TAU_UP } else { TAU_DOWN };
                let mut predicted = l + self.trend * dt;
                // Never extrapolate past the reading. When the prediction has
                // overtaken the input in the trend's direction, the change is
                // over: stop at the input and spend the trend fast. Without
                // this, a rise that stopped at 316 lux carried the estimate to
                // 490, and the darkening gate then froze that overshoot (the
                // steady input looked "darker") - brightness would climb past
                // the right level and slide back (found by the trend test).
                if (self.trend > 0.0 && predicted > x) || (self.trend < 0.0 && predicted < x) {
                    predicted = if self.trend > 0.0 { l.max(x) } else { l.min(x) };
                    self.trend *= (-dt / TREND_SPENT_TAU).exp();
                }
                let resid = x - predicted;
                let a = 1.0 - (-dt / tau).exp();
                let new_l = predicted + a * resid;
                let b = 1.0 - (-dt / TAU_TREND).exp();
                self.trend += ((new_l - l) / dt - self.trend) * b;
                // Damped trend: a trend nobody renews fades on its own.
                self.trend *= (-dt / TREND_DAMP_TAU).exp();
                self.level = Some(new_l);
            }
            _ => {
                self.level = Some(x);
                self.trend = 0.0;
                self.var = 0.0;
            }
        }
        self.last_reading = Some(now);
    }

    /// v2: a reading joins the window; the window's spread is the noise, and the committed light is
    /// what the screen is set for (not the raw window), so the band and the sunlight look stay still.
    fn on_lux_v2(&mut self, x: f32, now: Millis) {
        self.v2.samples.push_back((now, x));
        while self.v2.samples.front().is_some_and(|(t, _)| now.saturating_sub(*t) > V2_WINDOW_MS) {
            self.v2.samples.pop_front();
        }
        self.last_reading = Some(now);
        self.half_width = V2_HALF_WIDTH;
        self.trend = 0.0;
        let mut v: Vec<f32> = self.v2.samples.iter().map(|(_, x)| *x).collect();
        if let Some((_, sd, _)) = trimmed_stats(&mut v) {
            self.var = sd * sd;
        }
    }

    /// What the window says now: (trimmed mean, sd, n), once there are enough readings.
    fn v2_window(&self) -> Option<(f32, f32, usize)> {
        let mut v: Vec<f32> = self.v2.samples.iter().map(|(_, x)| *x).collect();
        trimmed_stats(&mut v)
    }

    /// The t statistic of the window's mean against the light the screen was set for.
    pub fn v2_t(&self) -> Option<f32> {
        let (m, sd, n) = self.v2_window()?;
        let c0 = self.v2.committed?;
        Some((m - c0) / (sd.max(V2_SIGMA_FLOOR) / (n as f32).sqrt()))
    }

    fn tick_v2(&mut self, now: Millis, minute: u32) -> Option<f32> {
        let night = self.night.factor(minute);
        // 1. decide: commit a new light only on a real, visible change, at most every V2_MIN_UPDATE_MS
        if let Some((m, _, _)) = self.v2_window() {
            match self.v2.committed {
                None => {
                    self.v2.committed = Some(m);
                    self.v2.last_commit = Some(now);
                }
                Some(c0) => {
                    let waited = self.v2.last_commit.is_none_or(|t| now.saturating_sub(t) >= V2_MIN_UPDATE_MS);
                    let visible = (curve_x(m) - curve_x(c0)).abs() * night >= V2_MIN_VISIBLE;
                    if waited && visible && self.v2_t().is_some_and(|t| t.abs() >= V2_T_CRIT) {
                        self.v2.committed = Some(m);
                        self.v2.last_commit = Some(now);
                    }
                }
            }
        }
        self.level = self.v2.committed;
        self.half_width = V2_HALF_WIDTH;
        self.math_offset = 0.0;
        // 2. where it should be (the slider moves it at once, whatever the wait)
        let target = self.band(now, minute).target;
        self.last_target = Some(target);
        let cur = self.applied?;
        let user_moved = (self.user - self.v2.last_user).abs() > 1e-4;
        self.v2.last_user = self.user;
        let heading = self.v2.glide.map(|g| g.1).unwrap_or(cur);
        if user_moved || (target - heading).abs() >= V2_RETARGET_EPS {
            let d = (target - cur).abs();
            let dur = (d * V2_GLIDE_PER_UNIT_S).clamp(V2_GLIDE_MIN_S, V2_GLIDE_MAX_S);
            self.v2.glide = Some((cur, target, now, dur));
        }
        // 3. glide: a smoothstep lerp, one value to the next
        let mut finished = false;
        if let Some((from, to, t0, dur)) = self.v2.glide {
            let p = now.saturating_sub(t0) as f32 / 1000.0 / dur;
            self.applied = Some(from + (to - from) * smoothstep(p));
            if p >= 1.0 {
                self.applied = Some(to);
                self.v2.glide = None;
                finished = true;
            }
        }
        self.vel = 0.0;
        let next = self.applied?;
        // 4. send at 30 updates a second, and the last one exactly
        let due = match (self.last_emitted, self.v2.last_emit_ms) {
            (None, _) => true,
            (Some(e), Some(t)) => {
                (now.saturating_sub(t) >= V2_EMIT_MS && (next - e).abs() >= V2_EMIT_STEP) || (finished && (next - e).abs() > 1e-6)
            }
            (Some(e), None) => (next - e).abs() >= V2_EMIT_STEP,
        };
        if due {
            self.last_emitted = Some(next);
            self.v2.last_emit_ms = Some(now);
            Some(next)
        } else {
            None
        }
    }

    fn center(&self, now: Millis, minute: u32) -> f32 {
        let base = match self.level {
            Some(l) if self.has_sensor(now) => curve_x(l),
            _ => DAY_LEVEL_NO_SENSOR,
        };
        base * self.night.factor(minute)
    }

    pub fn band(&self, now: Millis, minute: u32) -> Band {
        let center = self.center(now, minute);
        let user_offset = self.user * self.half_width;
        let target = (center + self.math_offset + user_offset).clamp(MIN_LEVEL, 1.0);
        Band {
            center,
            low: (center + self.math_offset - self.half_width).clamp(MIN_LEVEL, 1.0),
            high: (center + self.math_offset + self.half_width).clamp(MIN_LEVEL, 1.0),
            target,
            applied: self.applied,
            math_offset: self.math_offset,
            user_offset,
            stability: if self.has_sensor(now) { self.stability() } else { 1.0 },
            trend: self.trend,
            sunlight: self.has_sensor(now) && self.smoothed_lux().is_some_and(|l| l >= SUNLIGHT_LUX),
            contrast: if self.has_sensor(now) { self.smoothed_lux().map(contrast_for_lux).unwrap_or(0.0) } else { 0.0 },
        }
    }

    /// What the band half-width and the math offset are heading toward now.
    fn targets(&self, now: Millis, minute: u32) -> (f32, f32) {
        let night = self.night.factor(minute);
        match self.level {
            Some(l) if self.has_sensor(now) => {
                let slope = slope_x(l) * night;
                let w = W_MIN + 2.0 * slope * self.var.max(0.0).sqrt() + 0.5 * slope * self.trend.abs() * HORIZON_S;
                let o1 = (curve_x(l + self.trend * HORIZON_S) - curve_x(l)) * night * (1.0 - self.stability());
                (w.clamp(W_MIN, W_MAX), o1)
            }
            _ => (W_MIN, 0.0),
        }
    }

    /// The band half-width the current light calls for (the band itself
    /// follows it: fast when widening, slowly when narrowing).
    pub fn width_target(&self, now: Millis, minute: u32) -> f32 {
        self.targets(now, minute).0
    }

    pub fn is_animating(&self) -> bool {
        if self.algo == Algo::V2 {
            return self.enabled && self.applied.is_some() && (self.v2.glide.is_some() || self.last_target.zip(self.applied).is_some_and(|(t, a)| (t - a).abs() > 5e-4));
        }
        self.enabled
            && self.applied.is_some()
            && (self.vel.abs() > 1e-3 || self.last_target.zip(self.applied).is_some_and(|(t, a)| (t - a).abs() > 5e-4))
    }

    /// Advance the band, the math offset and the output spring; returns a
    /// level to apply when it moved a visible step.
    pub fn tick(&mut self, now: Millis, minute: u32) -> Option<f32> {
        let dt = self.last_tick.map(|t| now.saturating_sub(t) as f32 / 1000.0).unwrap_or(0.0).min(0.25);
        self.last_tick = Some(now);
        if !self.enabled {
            return None;
        }
        if self.algo == Algo::V2 {
            return self.tick_v2(now, minute);
        }
        let sensor = self.has_sensor(now);
        let s = if sensor { self.stability() } else { 1.0 };
        let (w_target, o1_target) = self.targets(now, minute);
        if dt > 0.0 {
            let tw = if w_target > self.half_width { TAU_W_EXPAND } else { TAU_W_CONTRACT };
            self.half_width += (w_target - self.half_width) * (1.0 - (-dt / tw).exp());
            // Engage quickly while unstable; release slowly as it stabilizes.
            let to = 0.3 + 2.2 * s;
            self.math_offset += (o1_target - self.math_offset) * (1.0 - (-dt / to).exp());
        }
        let target = self.band(now, minute).target;
        self.last_target = Some(target);
        // Never act from an unknown level (no readable backlight): a first
        // emit would be a jump, and there is nothing to control anyway.
        let cur = self.applied?;
        let (x, mut v) = spring_step(cur - target, self.vel, OMEGA, dt);
        let mut next = target + x;
        let maxd = MAX_SPEED * dt;
        if (next - cur).abs() > maxd {
            next = cur + maxd * (next - cur).signum();
            v = MAX_SPEED * (next - cur).signum();
        }
        if (next - target).abs() < 1e-4 && v.abs() < 1e-3 {
            next = target;
            v = 0.0;
        }
        self.applied = Some(next);
        self.vel = v;
        let due = match self.last_emitted {
            None => true,
            Some(e) => (next - e).abs() >= EMIT_STEP || (next == target && e != target),
        };
        if due {
            self.last_emitted = Some(next);
            Some(next)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOON: u32 = 12 * 60;

    /// Feed `lux(t_secs)` at `hz` for `secs`, ticking each sample. Returns
    /// (every applied value, emitted values, end time).
    fn run_fn(a: &mut AutoBrightness, lux: impl Fn(f32) -> f32, secs: f32, t0: Millis, hz: u32) -> (Vec<f32>, Vec<f32>, Millis) {
        let step = 1000.0 / hz as f32;
        let n = (secs * hz as f32) as u64;
        let (mut all, mut out) = (vec![], vec![]);
        let mut t = t0;
        for i in 1..=n {
            t = t0 + (i as f32 * step) as u64;
            a.on_lux(lux(i as f32 / hz as f32), t);
            out.extend(a.tick(t, NOON));
            all.extend(a.applied);
        }
        (all, out, t)
    }

    fn run(a: &mut AutoBrightness, lux: f32, secs: f32, t0: Millis, hz: u32) -> (Vec<f32>, Vec<f32>, Millis) {
        run_fn(a, |_| lux, secs, t0, hz)
    }

    fn on() -> AutoBrightness {
        let mut a = AutoBrightness { enabled: true, ..Default::default() };
        a.set_current(0.5);
        a
    }

    fn log(l: f32) -> f32 {
        (l + 1.0).log10()
    }

    #[test]
    fn curve_is_monotonic_and_hits_its_anchors() {
        let mut prev = 0.0;
        for i in 0..=200 {
            let lux = 10f32.powf(i as f32 / 40.0) - 1.0;
            let b = lux_curve(lux);
            assert!(b >= prev - 1e-6, "not monotonic at {lux}");
            prev = b;
        }
        assert!((lux_curve(0.0) - 0.03).abs() < 1e-6);
        assert!((lux_curve(99.0) - 0.32).abs() < 1e-3);
        assert_eq!(lux_curve(1e6), 1.0);
    }

    #[test]
    fn night_window_fades_and_wraps_midnight() {
        let w = NightWindow::default();
        assert_eq!(w.factor(NOON), 1.0);
        assert_eq!(w.factor(23 * 60), 0.6);
        assert_eq!(w.factor(2 * 60), 0.6);
        assert!((w.factor(21 * 60 + 30) - 0.8).abs() < 1e-3);
        assert!((w.factor(7 * 60 + 30) - 0.8).abs() < 1e-3);
        assert_eq!(w.factor(8 * 60 + 1), 1.0);
        let day_shift = NightWindow { start: 9 * 60, end: 17 * 60, fade: 30, dim: 0.5 };
        assert_eq!(day_shift.factor(NOON), 0.5);
        assert_eq!(day_shift.factor(20 * 60), 1.0);
    }

    #[test]
    fn span_parsing() {
        assert_eq!(NightWindow::parse_span("22:30-06:15"), Some((1350, 375)));
        assert_eq!(NightWindow::parse_span("25:00-06:00"), None);
        assert_eq!(NightWindow::default().format_span(), "22:00-07:00");
    }

    #[test]
    fn brightens_fast_darkens_slow() {
        let mut a = on();
        let (_, _, t) = run(&mut a, 50.0, 30.0, 0, 10);
        let (lo, hi) = (log(50.0), log(20_000.0));
        let (_, _, t) = run(&mut a, 20_000.0, 4.0, t, 10);
        let up = log(a.smoothed_lux().unwrap());
        assert!((up - lo) / (hi - lo) > 0.9, "4 s outside: only {:.2} of the way up", (up - lo) / (hi - lo));
        let (_, _, _) = run(&mut a, 50.0, 4.0, t, 10);
        let mid = log(a.smoothed_lux().unwrap());
        assert!((mid - lo) / (hi - lo) > 0.5, "4 s back inside: already {:.2} of the way down", 1.0 - (mid - lo) / (hi - lo));
    }

    #[test]
    fn darkening_is_gradual_once_the_delay_has_passed() {
        let mut a = on();
        let (_, _, t) = run(&mut a, 20_000.0, 30.0, 0, 5);
        run(&mut a, 50.0, (DARKEN_DELAY_MS as f32 / 1000.0) + 2.0, t, 5);
        let mid_log = (log(50.0) + log(20_000.0)) / 2.0;
        assert!(log(a.smoothed_lux().unwrap()) > mid_log);
    }

    #[test]
    fn a_brief_shadow_does_not_flicker_the_screen() {
        let mut a = on();
        let (_, _, t) = run(&mut a, 800.0, 30.0, 0, 10);
        let before = a.applied.unwrap();
        let (all, _, _) = run(&mut a, 5.0, 1.0, t, 90);
        let dip = all.iter().cloned().fold(before, f32::min);
        assert!(before - dip < 0.005, "dimmed {before} -> {dip} for a one-second shadow");
    }

    #[test]
    fn input_filter_is_rate_independent() {
        let mut a = on();
        let mut b = on();
        run(&mut a, 3000.0, 3.0, 0, 5);
        run(&mut b, 3000.0, 3.0, 0, 50);
        let (la, lb) = (log(a.smoothed_lux().unwrap()), log(b.smoothed_lux().unwrap()));
        assert!((la - lb).abs() < 0.08, "{la} vs {lb}");
    }

    #[test]
    fn band_widens_fast_with_noise_and_contracts_slowly() {
        let mut a = on();
        let (_, _, t) = run(&mut a, 300.0, 40.0, 0, 10);
        assert!(a.half_width() < W_MIN + 0.02, "steady: narrow ({})", a.half_width());
        // Mild noise: widens the band without pinning it at W_MAX (strong
        // noise saturates both band and target, leaving no lag to measure).
        let (_, _, t) = run_fn(&mut a, |s| if (s * 4.0) as u32 % 2 == 0 { 200.0 } else { 450.0 }, 8.0, t, 10);
        let wide = a.half_width();
        assert!(wide > W_MIN + 0.04 && wide < W_MAX - 0.02, "noisy: widened, not saturated ({wide})");
        // Contracts slowly, tested as a STEP: clear the noise and trend state
        // so the target width drops to W_MIN at once, then see how much of
        // the band's excess remains after 1 s: exp(-1/5) ~ 82% with slow
        // contraction, exp(-2.5) ~ 8% if it contracted fast. (Measuring lag
        // against a gradually falling target was marginal: the noise
        // estimate itself falls almost as slowly as the band.)
        a.var = 0.0;
        a.trend = 0.0;
        let excess = a.half_width() - W_MIN;
        let steady = a.smoothed_lux().unwrap();
        let (_, _, t) = run(&mut a, steady, 1.0, t, 10);
        assert!(a.width_target(t, NOON) < W_MIN + 0.01, "target stepped down");
        let remain = (a.half_width() - W_MIN) / excess;
        assert!(remain > 0.7, "band narrowed too fast: {remain:.2} of the excess left after 1 s");
        run(&mut a, 300.0, 60.0, t, 10);
        assert!(a.half_width() < W_MIN + 0.03, "steady again: narrow ({})", a.half_width());
    }

    #[test]
    fn flicker_below_the_level_widens_the_band_without_dimming() {
        // Bright light with short dark flickers: the gate keeps brightness up,
        // but the band must still widen (the simulator caught it staying at
        // its minimum when the gate hid the dark samples from the variance).
        let mut a = on();
        let (_, _, t) = run(&mut a, 480.0, 40.0, 0, 20);
        let steady_w = a.half_width();
        let before = a.applied.unwrap();
        let (all, _, _) = run_fn(&mut a, |s| if (s * 4.0) as u32 % 2 == 0 { 480.0 } else { 120.0 }, 8.0, t, 20);
        assert!(a.half_width() > steady_w + 0.04, "band did not widen: {} -> {}", steady_w, a.half_width());
        let dip = all.iter().cloned().fold(before, f32::min);
        assert!(before - dip < 0.01, "brightness dipped {before} -> {dip}");
    }

    #[test]
    fn forward_prediction_leads_a_trend_then_decays_when_stable() {
        let mut a = on();
        let (_, _, t) = run(&mut a, 10.0, 20.0, 0, 10);
        // Light rising steadily: log-lux +0.1 per second (e.g. dawn, a door opening slowly).
        let (_, _, t) = run_fn(&mut a, |s| 10.0 * 10f32.powf(s * 0.1), 15.0, t, 10);
        let b = a.band(t, NOON);
        assert!(a.band(t, NOON).trend > 0.05, "trend tracked: {}", b.trend);
        assert!(b.math_offset > 0.01, "leans toward where the light is heading: {}", b.math_offset);
        assert!(b.target > b.center, "target leads the plain curve");
        // Light stops changing: the math offset decays away.
        let end_lux = 10.0 * 10f32.powf(1.5);
        run(&mut a, end_lux, 40.0, t, 10);
        assert!(a.math_offset().abs() < 0.01, "decayed once stable: {}", a.math_offset());
        assert!(a.stability() > 0.9);
    }

    #[test]
    fn no_overshoot_when_a_trend_ends() {
        // The first filter carried a rise that stopped at 316 lux on to 490
        // (then the darkening gate froze it there).
        let mut a = on();
        let (_, _, t) = run(&mut a, 10.0, 20.0, 0, 10);
        let (_, _, mut t) = run_fn(&mut a, |s| 10.0 * 10f32.powf(s * 0.1), 15.0, t, 10);
        let end = 10.0 * 10f32.powf(1.5);
        let mut peak: f32 = 0.0;
        for _ in 0..300 {
            t += 100;
            a.on_lux(end, t);
            a.tick(t, NOON);
            peak = peak.max(a.smoothed_lux().unwrap());
        }
        assert!(peak <= end * 1.05, "estimate overshot to {peak} (input {end})");
        assert!((a.smoothed_lux().unwrap() - end).abs() / end < 0.02);
    }

    #[test]
    fn slider_offset_is_relative_to_the_band_and_scales_with_it() {
        let mut a = on();
        let (_, _, t) = run(&mut a, 300.0, 40.0, 0, 10);
        a.set_user(0.5);
        let b = a.band(t, NOON);
        assert!((b.user_offset - 0.5 * a.half_width()).abs() < 1e-6);
        assert!((b.target - (b.center + b.math_offset + b.user_offset)).abs() < 1e-5);
        let narrow = b.user_offset;
        let (_, _, t) = run_fn(&mut a, |s| if (s * 4.0) as u32 % 2 == 0 { 120.0 } else { 900.0 }, 8.0, t, 10);
        assert!(a.band(t, NOON).user_offset > narrow * 1.5, "the same slider position means more when the band is wide");
        a.set_user(3.0);
        assert_eq!(a.user, 1.0, "clamped to the band");
    }

    #[test]
    fn output_is_smooth_at_90hz() {
        let mut a = on();
        a.set_current(0.2);
        let (all, emits, _) = run(&mut a, 500.0, 2.0, 0, 90);
        // Velocity continuous: acceleration bounded (no step changes in speed).
        let v: Vec<f32> = all.windows(2).map(|w| (w[1] - w[0]) * 90.0).collect();
        let max_dv = v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_dv < 0.25, "speed jumped by {max_dv}/s in one 11 ms frame");
        // Many small steps, none visible on its own.
        assert!(emits.len() > 40, "{} emits in 2 s", emits.len());
        let max_step = emits.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_step < 0.012, "largest single step {max_step}");
        assert!(all.windows(2).all(|w| w[1] >= w[0] - 1e-6), "monotonic toward the target");
    }

    #[test]
    fn output_spring_is_frame_rate_independent_and_never_overshoots() {
        let (mut a, mut b) = (on(), on());
        for x in [&mut a, &mut b] {
            x.set_current(0.35);
        }
        run(&mut a, 500.0, 1.0, 0, 90);
        run(&mut b, 500.0, 1.0, 0, 60);
        assert!((a.applied.unwrap() - b.applied.unwrap()).abs() < 0.02, "{:?} vs {:?}", a.applied, b.applied);
        let mut c = on();
        c.set_current(0.05);
        let (all, _, t) = run(&mut c, 30_000.0, 6.0, 0, 90);
        let target = c.band(t, NOON).target;
        assert!(all.iter().all(|v| *v <= target + 1e-3), "overshot {target}");
        let first_second = all[89] - 0.05;
        assert!(first_second <= MAX_SPEED + 0.01, "speed limit: moved {first_second} in 1 s");
    }

    #[test]
    fn no_known_level_no_output() {
        let mut a = AutoBrightness { enabled: true, ..Default::default() };
        a.on_lux(20_000.0, 0);
        assert_eq!(a.tick(100, NOON), None);
        a.set_current(0.3);
        a.tick(111, NOON);
        assert!(a.tick(200, NOON).is_some());
    }

    #[test]
    fn disabled_emits_nothing() {
        let mut a = AutoBrightness::default();
        a.set_current(0.5);
        let (_, emits, _) = run(&mut a, 20_000.0, 5.0, 0, 10);
        assert!(emits.is_empty());
    }

    #[test]
    fn without_a_sensor_time_of_day_drives_it() {
        let a = on();
        assert!((a.band(0, NOON).target - DAY_LEVEL_NO_SENSOR).abs() < 1e-6);
        assert!((a.band(0, 2 * 60).target - DAY_LEVEL_NO_SENSOR * 0.6).abs() < 1e-6);
        let mut a = on();
        a.on_lux(20_000.0, 0);
        assert!(a.band(1000, NOON).center > 0.95);
        assert!((a.band(SENSOR_STALE_MS + 1, NOON).center - DAY_LEVEL_NO_SENSOR).abs() < 1e-6);
    }

    #[test]
    fn night_dims_the_sensor_result_too() {
        let mut a = on();
        a.on_lux(300.0, 0);
        let (d, n) = (a.band(1, NOON).center, a.band(1, 23 * 60).center);
        assert!((n - d * 0.6).abs() < 1e-4, "{d} {n}");
    }

    #[test]
    fn slider_curve_gives_the_low_end_the_travel_and_round_trips() {
        assert!(slider_to_level(0.5) < 0.25, "half the travel is under a quarter of the brightness");
        assert!(slider_to_level(0.3) > 0.05, "but still usable");
        for i in 0..=20 {
            let p = i as f32 / 20.0;
            assert!((level_to_slider(slider_to_level(p)) - p).abs() < 1e-5);
        }
        assert_eq!((slider_to_level(0.0), slider_to_level(1.0)), (0.0, 1.0));
    }

    #[test]
    fn sunlight_flag_needs_a_live_sensor_and_real_daylight() {
        let mut a = on();
        assert!(!a.band(0, NOON).sunlight);
        run(&mut a, 30_000.0, 20.0, 0, 10);
        assert!(a.band(20_100, NOON).sunlight);
        assert!(!a.band(20_100 + SENSOR_STALE_MS, NOON).sunlight, "stale sensor: no");
        let mut b = on();
        run(&mut b, 800.0, 20.0, 0, 10);
        assert!(!b.band(20_100, NOON).sunlight, "indoors: no");
    }

    #[test]
    fn contrast_blends_in_with_the_light() {
        assert_eq!(contrast_for_lux(500.0), 0.0);
        assert_eq!(contrast_for_lux(30_000.0), 1.0);
        let mid = contrast_for_lux((CONTRAST_START_LUX * SUNLIGHT_LUX).sqrt());
        assert!((mid - 0.5).abs() < 1e-3, "half-way in log lux: {mid}");
        let mut prev = 0.0;
        for i in 0..50 {
            let c = contrast_for_lux(1000.0 * 1.1f32.powi(i));
            assert!(c >= prev);
            prev = c;
        }
    }

    #[test]
    fn never_fully_dark() {
        let mut a = on();
        a.on_lux(0.0, 0);
        a.set_user(-1.0);
        assert_eq!(a.band(1, 2 * 60).target, MIN_LEVEL);
    }

    // ---- v2 ----------------------------------------------------------------------------

    fn v2() -> AutoBrightness {
        let mut a = AutoBrightness { algo: Algo::V2, enabled: true, ..Default::default() };
        a.set_current(0.3);
        a
    }

    /// Deterministic noise in -1..1.
    fn wob(i: u64) -> f32 {
        let x = (i as f32 * 12.9898).sin() * 43758.5453;
        (x - x.floor()) * 2.0 - 1.0
    }

    /// Run `lux(t)` at 10 Hz, ticking at 60 Hz in between; returns (time, emitted level) for every emission.
    fn drive(a: &mut AutoBrightness, lux: impl Fn(f32) -> f32, from_ms: Millis, secs: f32) -> Vec<(Millis, f32)> {
        let mut out = vec![];
        let mut i = 0u64;
        let end = from_ms + (secs * 1000.0) as u64;
        let mut t = from_ms;
        while t < end {
            if t % 100 == 0 {
                a.on_lux(lux((t - from_ms) as f32 / 1000.0) * (1.0 + 0.04 * wob(i)), t);
                i += 1;
            }
            if let Some(v) = a.tick(t, NOON) {
                out.push((t, v));
            }
            t += 16;
        }
        out
    }

    /// Steady light: it settles on the right level once, then stops sending anything at all.
    #[test]
    fn v2_settles_on_steady_light_and_then_goes_quiet() {
        let mut a = v2();
        let out = drive(&mut a, |_| 300.0, 0, 40.0);
        assert!(!out.is_empty(), "it moved to the room's level");
        let want = lux_curve(300.0);
        let last = out.last().unwrap().1;
        assert!((last - want).abs() < 0.02, "settled near {want}: {last}");
        let late: Vec<_> = out.iter().filter(|(t, _)| *t > 15_000).collect();
        assert!(late.is_empty(), "nothing is sent after it settled: {} emissions after 15 s", late.len());
        assert!(!a.is_animating(), "and it is not animating");
    }

    /// A real change (dim room to daylight) is followed, but not before the window shows it and not
    /// more often than every 5 s; a second change inside the wait is folded into the next commit.
    #[test]
    fn v2_follows_a_real_change_but_never_faster_than_every_five_seconds() {
        let mut a = v2();
        drive(&mut a, |_| 100.0, 0, 30.0);
        let level_dim = a.applied.unwrap();
        let out = drive(&mut a, |_| 3000.0, 30_000, 40.0);
        let first = out.iter().find(|(_, v)| *v > level_dim + 0.02).expect("it brightened").0;
        assert!(first >= 30_000 + 1_000, "not instant: the window has to see it (first change at {} ms in)", first - 30_000);
        let reached = out.last().unwrap().1;
        assert!((reached - lux_curve(3000.0)).abs() < 0.03, "ends at the daylight level: {reached}");
        // commits: count distinct plateaus in what was sent: a single target, one glide
        let commits = {
            let mut c = 0;
            let mut prev = out[0].1;
            let mut dir = 0.0f32;
            for (_, v) in &out {
                let d = v - prev;
                if d.abs() > 1e-6 {
                    if dir != 0.0 && d.signum() != dir { c += 1; }
                    dir = d.signum();
                }
                prev = *v;
            }
            c
        };
        assert_eq!(commits, 0, "one smooth glide, no back-and-forth");
        // two changes 2 s apart: the second waits for the 5 s rule
        let mut b = v2();
        drive(&mut b, |_| 100.0, 0, 30.0);
        let out = drive(&mut b, |t| if t < 8.0 { 1000.0 } else { 20.0 }, 30_000, 30.0);
        let first_up = out.iter().position(|(_, v)| *v > lux_curve(100.0) + 0.02);
        assert!(first_up.is_some(), "it did brighten for the 8 s of light");
    }

    /// A hand or a shadow over the sensor for 2 s does not move the screen.
    #[test]
    fn v2_ignores_a_two_second_shadow() {
        let mut a = v2();
        drive(&mut a, |_| 400.0, 0, 30.0);
        let before = a.applied.unwrap();
        let out = drive(&mut a, |t| if (3.0..5.0).contains(&t) { 3.0 } else { 400.0 }, 30_000, 20.0);
        assert!(out.iter().all(|(_, v)| (v - before).abs() < 0.02), "the screen did not follow the shadow: {:?}", out.iter().map(|o| o.1).fold((9.0f32, 0.0f32), |m, v| (m.0.min(v), m.1.max(v))));
    }

    /// Flicker (+-30% swings every reading) is not a change.
    #[test]
    fn v2_ignores_flicker_around_a_steady_level() {
        let mut a = v2();
        drive(&mut a, |_| 500.0, 0, 30.0);
        let before = a.applied.unwrap();
        let mut i = 0u64;
        let mut moved = 0;
        for k in 0..600u64 {
            let t = 30_000 + k * 100;
            i += 1;
            a.on_lux(500.0 * (1.0 + 0.3 * wob(i + 99)), t);
            if let Some(v) = a.tick(t, NOON) {
                if (v - before).abs() > 0.02 { moved += 1; }
            }
        }
        assert_eq!(moved, 0, "flicker moved the screen");
    }

    /// The glide is smooth: updates at about 30 a second, each small, never out of order, never overshooting.
    #[test]
    fn v2_glides_at_thirty_updates_a_second_in_small_steps() {
        let mut a = v2();
        drive(&mut a, |_| 100.0, 0, 30.0);
        let out = drive(&mut a, |_| 8000.0, 30_000, 30.0);
        let start = lux_curve(100.0);
        let end = lux_curve(8000.0);
        assert!(out.iter().all(|(_, v)| *v >= start - 0.02 && *v <= end + 0.005), "no overshoot");
        let glide: Vec<_> = out.windows(2).filter(|w| w[1].1 - w[0].1 > 1e-4).collect();
        assert!(glide.len() > 20, "many small steps: {}", glide.len());
        let max_step = glide.iter().map(|w| w[1].1 - w[0].1).fold(0.0f32, f32::max);
        assert!(max_step < 0.03, "no big jumps: {max_step}");
        assert!(out.windows(2).all(|w| w[1].0 - w[0].0 >= 16), "never faster than the frame");
        let mut gaps: Vec<Millis> = glide.iter().map(|w| w[1].0 - w[0].0).collect();
        gaps.sort();
        let median_gap = gaps[gaps.len() / 2];
        // the test ticks at 60 Hz, so a 33 ms rule lands on every third tick (48 ms); the real loop runs faster
        assert!((32..=50).contains(&median_gap), "about 30 updates a second (every {median_gap} ms, median)");
        assert!(out.windows(2).all(|w| w[1].1 >= w[0].1 - 1e-6), "monotonic on the way up");
    }

    /// The slider acts at once, ignoring the 5 s wait.
    #[test]
    fn v2_slider_moves_the_screen_immediately() {
        let mut a = v2();
        drive(&mut a, |_| 300.0, 0, 30.0);
        let before = a.applied.unwrap();
        a.set_user(0.8);
        let out = drive(&mut a, |_| 300.0, 30_000, 4.0);
        let after = out.last().expect("the slider's change was sent").1;
        assert!(after > before + 0.05, "{before} -> {after}");
        assert!(out.first().unwrap().0 < 30_000 + 200, "started within a frame or two");
    }

    /// No sensor, nothing decided: it does not invent a level, and a dead sensor is noticed.
    #[test]
    fn v2_without_readings_does_nothing() {
        let mut a = v2();
        assert_eq!(a.tick(1000, NOON), Some(0.3).filter(|_| false), "nothing to say before any reading");
        assert_eq!(a.applied, Some(0.3), "and the level it started at is untouched");
        assert!(!a.has_sensor(1000));
        a.on_lux(300.0, 100);
        assert!(!a.has_sensor(100), "one reading is not a decision");
    }


    /// However the light jumps about, commits are at least five seconds apart (and do happen).
    #[test]
    fn v2_commits_are_never_closer_than_the_minimum_interval() {
        let mut a = v2();
        let mut commits: Vec<Millis> = vec![];
        let mut i = 0u64;
        for k in 0..1000u64 {
            let t = k * 100;
            // 100 lux and 5000 lux, switching every 7 s
            let lux = if (t / 7000) % 2 == 0 { 100.0 } else { 5000.0 };
            i += 1;
            a.on_lux(lux * (1.0 + 0.04 * wob(i)), t);
            a.tick(t, NOON);
            if let Some(c) = a.v2.last_commit {
                if commits.last() != Some(&c) { commits.push(c); }
            }
        }
        assert!(commits.len() >= 3, "it followed the changes: {commits:?}");
        for w in commits.windows(2) {
            assert!(w[1] - w[0] >= V2_MIN_UPDATE_MS, "commits too close: {} then {}", w[0], w[1]);
        }
    }

}

