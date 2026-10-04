//! The One Euro filter (Casiez, Roussel and Vogel, CHI 2012) on a 2D point: heavy smoothing while the point is nearly
//! still, so a fixation does not shake, and light smoothing while it moves fast, so a saccade is not late. The cutoff
//! rises with the point's SPEED (`cutoff = min_cutoff + beta * speed`); the speed is the vector's, so both axes share one
//! cutoff and a diagonal move is not smoothed differently from a straight one.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OneEuroParams {
    /// Cutoff in Hz when still: lower is steadier and later.
    pub min_cutoff: f64,
    /// How fast the cutoff rises with speed (Hz per unit/s).
    pub beta: f64,
    /// Cutoff for the speed estimate itself.
    pub d_cutoff: f64,
}

impl Default for OneEuroParams {
    /// For a gaze point in screen fractions (0..1): a fixation's jitter is a few thousandths, a saccade crosses
    /// a good part of the screen in a tenth of a second. Swept against the tests below: min_cutoff 0.6 / beta 6 and
    /// 0.5 / 8 leave the fixation at 0.35-0.37 of its jitter (the noise itself raises the cutoff through the speed
    /// term); 0.5 / 5 keeps it under 0.35 with the saccade still inside 6 frames at 30 Hz.
    fn default() -> Self { OneEuroParams { min_cutoff: 0.5, beta: 5.0, d_cutoff: 3.0 } }
}

fn alpha(cutoff: f64, dt: f64) -> f64 {
    let tau = 1.0 / (2.0 * PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

#[derive(Debug, Clone, PartialEq)]
pub struct OneEuro2D {
    pub p: OneEuroParams,
    x: Option<(f64, f64)>,
    dx: (f64, f64),
    t: Option<f64>,
}

impl OneEuro2D {
    pub fn new(p: OneEuroParams) -> OneEuro2D { OneEuro2D { p, x: None, dx: (0.0, 0.0), t: None } }

    pub fn reset(&mut self) { self.x = None; self.dx = (0.0, 0.0); self.t = None; }

    /// The filtered point for a measurement at time `t` seconds. A non-finite measurement, or a time that did not
    /// advance, returns the last output unchanged (or the measurement itself the first time).
    pub fn step(&mut self, t: f64, m: (f64, f64)) -> (f64, f64) {
        if !(m.0.is_finite() && m.1.is_finite() && t.is_finite()) { return self.x.unwrap_or((0.0, 0.0)); }
        let (Some(prev), Some(pt)) = (self.x, self.t) else {
            self.x = Some(m); self.t = Some(t);
            return m;
        };
        let dt = t - pt;
        if dt <= 1e-6 { return prev; }
        let raw_d = ((m.0 - prev.0) / dt, (m.1 - prev.1) / dt);
        let ad = alpha(self.p.d_cutoff, dt);
        self.dx = (self.dx.0 + ad * (raw_d.0 - self.dx.0), self.dx.1 + ad * (raw_d.1 - self.dx.1));
        let speed = self.dx.0.hypot(self.dx.1);
        let a = alpha(self.p.min_cutoff + self.p.beta * speed, dt);
        let out = (prev.0 + a * (m.0 - prev.0), prev.1 + a * (m.1 - prev.1));
        self.x = Some(out);
        self.t = Some(t);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic noise source in [-1, 1].
    fn noise(i: usize) -> f64 {
        let mut z = (i as u64).wrapping_add(0x9e3779b97f4a7c15).wrapping_mul(0xbf58476d1ce4e5b9);
        z ^= z >> 29;
        z = z.wrapping_mul(0x94d049bb133111eb);
        ((z >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }

    fn std_of(v: &[f64]) -> f64 {
        let m = v.iter().sum::<f64>() / v.len() as f64;
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
    }

    #[test]
    fn a_fixation_is_steadied_to_a_third_of_its_jitter() {
        let mut f = OneEuro2D::new(OneEuroParams::default());
        let (mut raw, mut out) = (Vec::new(), Vec::new());
        for i in 0..300 {
            let m = (0.5 + 0.01 * noise(2 * i), 0.5 + 0.01 * noise(2 * i + 1));
            let o = f.step(i as f64 / 30.0, m);
            if i >= 60 { raw.push(m.0); out.push(o.0); }
        }
        assert!(std_of(&out) < 0.35 * std_of(&raw), "raw {:.5} out {:.5}", std_of(&raw), std_of(&out));
    }

    #[test]
    fn a_saccade_is_not_late() {
        let mut f = OneEuro2D::new(OneEuroParams::default());
        for i in 0..60 { f.step(i as f64 / 30.0, (0.2, 0.5)); }
        // the gaze jumps to 0.8 and stays
        let mut frames_to_90 = None;
        for k in 1..=30 {
            let o = f.step((59 + k) as f64 / 30.0, (0.8, 0.5));
            if frames_to_90.is_none() && o.0 >= 0.2 + 0.9 * 0.6 { frames_to_90 = Some(k); }
        }
        let k = frames_to_90.expect("it arrives");
        assert!(k <= 6, "90% of a 0.6 jump took {k} frames at 30 Hz ({} ms)", k * 33);
    }

    #[test]
    fn steady_pursuit_lags_by_a_few_hundredths_not_more() {
        let mut f = OneEuro2D::new(OneEuroParams::default());
        let mut lag = 0.0f64;
        for i in 0..120 {
            let x = 0.1 + 0.5 * i as f64 / 120.0 * 4.0 / 4.0; // 0.5 screen per 4 s ... slow sweep
            let o = f.step(i as f64 / 30.0, (x, 0.5));
            if i > 60 { lag = lag.max(x - o.0); }
        }
        assert!(lag < 0.03, "lag {lag}");
    }

    #[test]
    fn bad_input_and_repeated_timestamps_change_nothing() {
        let mut f = OneEuro2D::new(OneEuroParams::default());
        let a = f.step(0.0, (0.3, 0.4));
        assert_eq!(a, (0.3, 0.4), "the first point passes through");
        let b = f.step(0.033, (0.31, 0.4));
        assert_eq!(f.step(0.033, (0.9, 0.9)), b, "the clock did not advance");
        assert_eq!(f.step(0.066, (f64::NAN, 0.4)), b, "NaN is ignored");
        assert_eq!(f.step(f64::INFINITY, (0.5, 0.5)), b);
        assert!(f.step(0.066, (0.32, 0.4)).0 > b.0);
    }
}
