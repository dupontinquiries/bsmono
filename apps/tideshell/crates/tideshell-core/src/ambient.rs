//! Auto-brightness from ambient light and time of day. Pure: callers feed
//! lux readings, the clock (minute of the local day) and the user's slider,
//! and apply the brightness values it returns.
//!
//! The pipeline, each stage chosen against a specific failure:
//!
//! 1. **Lux → brightness curve**, piecewise-linear in `log10(lux + 1)`:
//!    perceived brightness is roughly logarithmic in lux, so 10 → 100 lux
//!    (dim room → lit room) is as big a step as 1 000 → 10 000 (overcast →
//!    daylight).
//! 2. **Asymmetric smoothing** of log-lux: fast when it gets brighter (walking
//!    outside: the screen must become readable now), slow when it gets darker,
//!    and only after the darker reading has persisted for DARKEN_DELAY_MS. A
//!    hand or passing shadow over the sensor must not dim the screen; slow
//!    smoothing alone still dimmed it visibly (0.57 -> 0.48 for a one-second
//!    shadow, caught by a test), so darkening waits first.
//! 3. **Night window**: a multiplier that dims the result between `start` and
//!    `end` (local wall clock, fading over `fade` minutes each side), for
//!    dark-adapted eyes. With no sensor, time of day alone drives brightness.
//! 4. **Learned offset**: when the user moves the brightness slider while auto
//!    is on, the difference from the curve becomes an offset applied from then
//!    on, so auto does not immediately undo the user's choice.
//! 5. **Dead band** on the target, then a **rate-limited ramp** toward it: no
//!    visible steps, and no stream of tiny adjustments from sensor noise.

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

pub const MIN_LEVEL: f32 = 0.02;
/// Seconds for the smoothed log-lux to cover ~63% of a step, per direction.
pub const TAU_UP: f32 = 1.2;
pub const TAU_DOWN: f32 = 6.0;
/// A darker reading must persist this long before the screen starts dimming.
pub const DARKEN_DELAY_MS: Millis = 3000;
/// Target changes smaller than this are ignored (sensor noise, flicker).
pub const DEAD_BAND: f32 = 0.03;
/// Maximum ramp speed, brightness units per second (full range in ~2.5 s).
pub const RAMP_PER_S: f32 = 0.4;
/// No reading for this long: treat the sensor as gone, use time of day.
pub const SENSOR_STALE_MS: Millis = 15_000;
/// Brightness with no sensor, before the night window applies.
pub const DAY_LEVEL_NO_SENSOR: f32 = 0.6;

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

#[derive(Debug, Clone, PartialEq)]
pub struct AutoBrightness {
    pub enabled: bool,
    /// Learned from the user's slider; added after the curve and night factor.
    pub offset: f32,
    pub night: NightWindow,
    /// Smoothed log10(lux + 1); None until the first reading.
    log_lux: Option<f32>,
    last_reading: Option<Millis>,
    /// When readings started being darker than the smoothed value.
    darker_since: Option<Millis>,
    target: Option<f32>,
    /// Current level as last applied (from us, or from the user's slider).
    pub applied: Option<f32>,
    last_emitted: Option<f32>,
    last_tick: Option<Millis>,
}

impl Default for AutoBrightness {
    fn default() -> Self {
        AutoBrightness {
            enabled: false,
            offset: 0.0,
            night: NightWindow::default(),
            log_lux: None,
            last_reading: None,
            darker_since: None,
            target: None,
            applied: None,
            last_emitted: None,
            last_tick: None,
        }
    }
}

impl AutoBrightness {
    /// The level the backlight had when we started (so the first ramp starts
    /// from reality instead of jumping).
    pub fn set_current(&mut self, level: f32) {
        self.applied = Some(level.clamp(0.0, 1.0));
        self.last_emitted = self.applied;
    }

    pub fn has_sensor(&self, now: Millis) -> bool {
        self.last_reading.is_some_and(|t| now.saturating_sub(t) < SENSOR_STALE_MS)
    }

    pub fn smoothed_lux(&self) -> Option<f32> {
        self.log_lux.map(|l| 10f32.powf(l) - 1.0)
    }

    /// A sensor reading. Smoothing happens here, per reading, with the
    /// elapsed time since the previous one, so irregular sensor rates give
    /// the same curve.
    pub fn on_lux(&mut self, lux: f32, now: Millis) {
        let x = (lux.max(0.0) + 1.0).log10();
        self.log_lux = Some(match (self.log_lux, self.last_reading) {
            (Some(prev), Some(t)) => {
                let dt = now.saturating_sub(t) as f32 / 1000.0;
                if x >= prev {
                    self.darker_since = None;
                    prev + (x - prev) * (1.0 - (-dt / TAU_UP).exp())
                } else {
                    let since = *self.darker_since.get_or_insert(now);
                    if now.saturating_sub(since) < DARKEN_DELAY_MS {
                        prev // still waiting: a shadow, or real darkening?
                    } else {
                        prev + (x - prev) * (1.0 - (-dt / TAU_DOWN).exp())
                    }
                }
            }
            _ => x,
        });
        self.last_reading = Some(now);
    }

    /// What the pipeline wants right now, before dead band and ramp.
    pub fn desired(&self, now: Millis, minute: u32) -> f32 {
        let base = match self.log_lux {
            Some(l) if self.has_sensor(now) => lux_curve(10f32.powf(l) - 1.0),
            _ => DAY_LEVEL_NO_SENSOR,
        };
        (base * self.night.factor(minute) + self.offset).clamp(MIN_LEVEL, 1.0)
    }

    /// The user moved the brightness slider. With auto on, learn the offset
    /// that makes the pipeline produce exactly this level here and now.
    /// Returns true when the offset changed (worth persisting).
    pub fn on_manual(&mut self, level: f32, now: Millis, minute: u32) -> bool {
        let level = level.clamp(0.0, 1.0);
        self.applied = Some(level);
        self.last_emitted = Some(level);
        self.target = Some(level);
        if !self.enabled {
            return false;
        }
        let without_offset = self.desired(now, minute) - self.offset;
        let new = (level - without_offset).clamp(-0.6, 0.6);
        let changed = (new - self.offset).abs() > 1e-4;
        self.offset = new;
        changed
    }

    /// Advance; returns a brightness to apply when one is due.
    pub fn tick(&mut self, now: Millis, minute: u32) -> Option<f32> {
        let dt = self.last_tick.map(|t| now.saturating_sub(t) as f32 / 1000.0).unwrap_or(0.0).min(0.5);
        self.last_tick = Some(now);
        if !self.enabled {
            return None;
        }
        let want = self.desired(now, minute);
        let target = match self.target {
            Some(t) if (want - t).abs() < DEAD_BAND => t,
            _ => want,
        };
        self.target = Some(target);
        // Never act from an unknown level: without set_current (no readable
        // backlight) a first emit would be a jump, and there is nothing to
        // control anyway. Found by the shell tests: auto-on-by-default made a
        // fresh shell emit SetSlider(0.6) on its first tick.
        let Some(cur) = self.applied else { return None };
        let step = RAMP_PER_S * dt;
        let next = if (target - cur).abs() <= step { target } else { cur + step * (target - cur).signum() };
        self.applied = Some(next);
        // Emit when it moved a visible amount, or when it just arrived.
        let due = match self.last_emitted {
            None => true,
            Some(e) => (next - e).abs() >= 0.005 || (next == target && e != target),
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

    /// Feed a constant lux at `hz` for `secs`, ticking each sample; return
    /// the emitted levels.
    fn run(a: &mut AutoBrightness, lux: f32, secs: f32, t0: Millis, hz: u32) -> (Vec<f32>, Millis) {
        let step = 1000 / hz as u64;
        let n = (secs * hz as f32) as u64;
        let mut out = vec![];
        let mut t = t0;
        for _ in 0..n {
            t += step;
            a.on_lux(lux, t);
            out.extend(a.tick(t, NOON));
        }
        (out, t)
    }

    fn on() -> AutoBrightness {
        let mut a = AutoBrightness { enabled: true, ..Default::default() };
        a.set_current(0.5);
        a
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
        let w = NightWindow::default(); // 22:00-07:00, 60 min fades, dim 0.6
        assert_eq!(w.factor(NOON), 1.0);
        assert_eq!(w.factor(23 * 60), 0.6, "inside");
        assert_eq!(w.factor(2 * 60), 0.6, "after midnight");
        assert!((w.factor(21 * 60 + 30) - 0.8).abs() < 1e-3, "half-way into the evening fade");
        assert!((w.factor(7 * 60 + 30) - 0.8).abs() < 1e-3, "half-way through the morning fade");
        assert_eq!(w.factor(8 * 60 + 1), 1.0);
        let day_shift = NightWindow { start: 9 * 60, end: 17 * 60, fade: 30, dim: 0.5 };
        assert_eq!(day_shift.factor(NOON), 0.5, "non-wrapping window works too");
        assert_eq!(day_shift.factor(20 * 60), 1.0);
    }

    #[test]
    fn span_parsing() {
        assert_eq!(NightWindow::parse_span("22:30-06:15"), Some((1350, 375)));
        assert_eq!(NightWindow::parse_span("25:00-06:00"), None);
        assert_eq!(NightWindow::parse_span("nonsense"), None);
        assert_eq!(NightWindow::default().format_span(), "22:00-07:00");
    }

    #[test]
    fn brightens_fast_darkens_slow() {
        let mut a = on();
        let (_, t) = run(&mut a, 50.0, 30.0, 0, 5); // settle indoors
        let settled = a.applied.unwrap();
        // Step outside: within 4 s most of the way up.
        let (_, t) = run(&mut a, 20_000.0, 4.0, t, 5);
        let up = a.applied.unwrap();
        assert!(up > 0.85, "brightened to {up} in 4 s from {settled}");
        // Back inside: after 4 s still well above the indoor level...
        let (_, t) = run(&mut a, 50.0, 4.0, t, 5);
        let mid = a.applied.unwrap();
        assert!(mid > settled + 0.25, "darkened too fast: {mid} vs {settled}");
        // ...but it does get there.
        run(&mut a, 50.0, 40.0, t, 5);
        assert!((a.applied.unwrap() - settled).abs() < DEAD_BAND + 0.01);
    }

    #[test]
    fn darkening_is_gradual_once_the_delay_has_passed() {
        // Measured on the smoothed light itself, so neither the delay window
        // nor the ramp limit can hide fast darkening (the first test of the
        // asymmetry passed even with TAU_DOWN replaced by TAU_UP).
        let mut a = on();
        let (_, t) = run(&mut a, 20_000.0, 30.0, 0, 5);
        run(&mut a, 50.0, (DARKEN_DELAY_MS as f32 / 1000.0) + 2.0, t, 5);
        let mid_log = ((50f32 + 1.0).log10() + (20_000f32 + 1.0).log10()) / 2.0;
        let now_log = (a.smoothed_lux().unwrap() + 1.0).log10();
        assert!(now_log > mid_log, "2 s after the delay it is already past half-way down: {now_log} <= {mid_log}");
    }

    #[test]
    fn a_brief_shadow_does_not_flicker_the_screen() {
        let mut a = on();
        let (_, t) = run(&mut a, 800.0, 30.0, 0, 5);
        let before = a.applied.unwrap();
        // Hand over the sensor for one second.
        let (emits, t) = run(&mut a, 5.0, 1.0, t, 5);
        let dip = emits.iter().cloned().fold(before, f32::min);
        assert!(before - dip < 0.005, "dimmed {before} -> {dip} for a one-second shadow");
        run(&mut a, 800.0, 10.0, t, 5);
    }

    #[test]
    fn sensor_noise_inside_the_dead_band_emits_nothing() {
        let mut a = on();
        let (_, mut t) = run(&mut a, 300.0, 30.0, 0, 5);
        let mut emits = 0;
        for i in 0..100 {
            t += 200;
            a.on_lux(if i % 2 == 0 { 280.0 } else { 330.0 }, t);
            emits += a.tick(t, NOON).is_some() as u32;
        }
        assert_eq!(emits, 0);
    }

    #[test]
    fn ramp_is_rate_limited_and_monotonic() {
        let mut a = on();
        a.set_current(0.05);
        let (emits, _) = run(&mut a, 20_000.0, 1.0, 0, 20);
        assert!(emits.windows(2).all(|w| w[1] >= w[0]), "{emits:?}");
        let reached = *emits.last().unwrap();
        assert!(reached <= 0.05 + RAMP_PER_S * 1.0 + 0.02, "ramp too fast: {reached} after 1 s");
    }

    #[test]
    fn smoothing_is_rate_independent() {
        let mut a = on();
        let mut b = on();
        run(&mut a, 3000.0, 3.0, 0, 5);
        run(&mut b, 3000.0, 3.0, 0, 50);
        assert!((a.smoothed_lux().unwrap() - b.smoothed_lux().unwrap()).abs() < 1.0);
    }

    #[test]
    fn manual_slider_teaches_an_offset_and_auto_keeps_it() {
        let mut a = on();
        let (_, t) = run(&mut a, 300.0, 30.0, 0, 5);
        let auto_level = a.applied.unwrap();
        assert!(a.on_manual(auto_level + 0.2, t, NOON), "offset changed");
        // Same light: auto must not pull it back.
        let (emits, t) = run(&mut a, 300.0, 10.0, t, 5);
        assert!(emits.is_empty(), "fought the user: {emits:?}");
        assert!((a.applied.unwrap() - (auto_level + 0.2)).abs() < 1e-3);
        // Brighter light: still brighter than the plain curve by ~the offset.
        run(&mut a, 3000.0, 30.0, t, 5);
        let plain = lux_curve(3000.0);
        assert!((a.applied.unwrap() - (plain + 0.2).min(1.0)).abs() < DEAD_BAND + 0.01, "{} vs {}", a.applied.unwrap(), plain + 0.2);
    }

    #[test]
    fn disabled_emits_nothing_and_manual_learns_nothing() {
        let mut a = AutoBrightness::default();
        let (emits, t) = run(&mut a, 20_000.0, 5.0, 0, 5);
        assert!(emits.is_empty());
        assert!(!a.on_manual(0.9, t, NOON));
        assert_eq!(a.offset, 0.0);
    }

    #[test]
    fn without_a_sensor_time_of_day_drives_it() {
        let mut a = on();
        let day = a.desired(0, NOON);
        let night = a.desired(0, 2 * 60);
        assert!((day - DAY_LEVEL_NO_SENSOR).abs() < 1e-6 && (night - DAY_LEVEL_NO_SENSOR * 0.6).abs() < 1e-6);
        // A sensor that stops reporting falls back to the clock.
        a.on_lux(20_000.0, 0);
        assert!(a.desired(1000, NOON) > 0.95);
        assert!((a.desired(SENSOR_STALE_MS + 1, NOON) - DAY_LEVEL_NO_SENSOR).abs() < 1e-6);
    }

    #[test]
    fn night_dims_the_sensor_result_too() {
        let mut a = on();
        a.on_lux(300.0, 0);
        let (d, n) = (a.desired(1, NOON), a.desired(1, 23 * 60));
        assert!((n - d * 0.6).abs() < 1e-4, "{d} {n}");
    }

    #[test]
    fn no_known_level_no_output() {
        let mut a = AutoBrightness { enabled: true, ..Default::default() };
        a.on_lux(20_000.0, 0);
        assert_eq!(a.tick(100, NOON), None);
        a.set_current(0.3);
        assert!(a.tick(200, NOON).is_some());
    }

    #[test]
    fn never_fully_dark() {
        let mut a = on();
        a.offset = -0.6;
        a.on_lux(0.0, 0);
        assert_eq!(a.desired(1, 2 * 60), MIN_LEVEL);
    }
}
