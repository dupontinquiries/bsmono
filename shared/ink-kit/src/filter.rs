//! Point-stream conditioning. Two stages, both optional and both pure:
//!
//! * [`OneEuro`] — live smoothing while the finger is down (removes touch
//!   sensor jitter without the lag of a plain low-pass; Casiez et al. 2012).
//!   Run it in *screen* space before the camera transform, because sensor
//!   noise is a screen-space phenomenon.
//! * [`rdp`] — Ramer–Douglas–Peucker simplification at stroke end (storage,
//!   recognition, and export all want fewer points).

use crate::stroke::InkPoint;
use crate::Vec2d;

/// One-euro filter over a scalar channel.
#[derive(Clone, Debug)]
struct OneEuroChan {
    min_cutoff: f64,
    beta: f64,
    d_cutoff: f64,
    prev: Option<(f64, f64, f64)>, // (t_s, x, dx)
}

impl OneEuroChan {
    fn alpha(cutoff: f64, dt: f64) -> f64 {
        let tau = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
        1.0 / (1.0 + tau / dt)
    }
    fn filter(&mut self, t_s: f64, x: f64) -> f64 {
        match self.prev {
            None => {
                self.prev = Some((t_s, x, 0.0));
                x
            }
            Some((pt, px, pdx)) => {
                let dt = (t_s - pt).max(1e-4);
                let dx = (x - px) / dt;
                let a_d = Self::alpha(self.d_cutoff, dt);
                let dx_hat = a_d * dx + (1.0 - a_d) * pdx;
                let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
                let a = Self::alpha(cutoff, dt);
                let x_hat = a * x + (1.0 - a) * px;
                self.prev = Some((t_s, x_hat, dx_hat));
                x_hat
            }
        }
    }
}

/// 2D one-euro filter for a live stroke. One instance per stroke; feed points
/// in capture order.
#[derive(Clone, Debug)]
pub struct OneEuro {
    x: OneEuroChan,
    y: OneEuroChan,
}

impl OneEuro {
    /// Defaults tuned for finger input at typical touch sample rates
    /// (60–120 Hz): `min_cutoff` 1.0 Hz, `beta` 0.02.
    pub fn new() -> Self {
        Self::with_params(1.0, 0.02)
    }
    pub fn with_params(min_cutoff: f64, beta: f64) -> Self {
        let mk = || OneEuroChan {
            min_cutoff,
            beta,
            d_cutoff: 1.0,
            prev: None,
        };
        OneEuro { x: mk(), y: mk() }
    }
    /// Filter one sample. `t_ms` is the capture timestamp in milliseconds.
    pub fn filter(&mut self, t_ms: f64, p: Vec2d) -> Vec2d {
        let t_s = t_ms / 1000.0;
        Vec2d::new(self.x.filter(t_s, p.x), self.y.filter(t_s, p.y))
    }
}

impl Default for OneEuro {
    fn default() -> Self {
        OneEuro::new()
    }
}

/// Ramer–Douglas–Peucker on InkPoints. Keeps the original samples (pressure
/// and timestamps ride along); `epsilon` is the max perpendicular deviation
/// in the points' own units (world units, normally).
pub fn rdp(points: &[InkPoint], epsilon: f64) -> Vec<InkPoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    rdp_rec(points, 0, points.len() - 1, epsilon, &mut keep);
    points
        .iter()
        .zip(keep.iter())
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

fn rdp_rec(pts: &[InkPoint], a: usize, b: usize, eps: f64, keep: &mut [bool]) {
    if b <= a + 1 {
        return;
    }
    let (pa, pb) = (pts[a].pos, pts[b].pos);
    let mut worst = 0.0f64;
    let mut wi = a;
    for i in (a + 1)..b {
        let d = perp_dist(pa, pb, pts[i].pos);
        if d > worst {
            worst = d;
            wi = i;
        }
    }
    if worst > eps {
        keep[wi] = true;
        rdp_rec(pts, a, wi, eps, keep);
        rdp_rec(pts, wi, b, eps, keep);
    }
}

fn perp_dist(a: Vec2d, b: Vec2d, p: Vec2d) -> f64 {
    let ab = b - a;
    let len = (ab.x * ab.x + ab.y * ab.y).sqrt();
    if len <= f64::EPSILON {
        return a.dist(p);
    }
    ((p.x - a.x) * ab.y - (p.y - a.y) * ab.x).abs() / len
}

/// Resample a polyline to (roughly) equidistant points, `step` apart in the
/// points' units. Pressure and time are linearly interpolated. Recognition
/// and rasterizing both prefer even spacing over raw sensor cadence.
pub fn resample(points: &[InkPoint], step: f64) -> Vec<InkPoint> {
    if points.len() < 2 || step <= 0.0 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    let mut carried = 0.0f64;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = a.pos.dist(b.pos);
        if seg <= f64::EPSILON {
            continue;
        }
        let mut d = step - carried;
        while d <= seg {
            let t = d / seg;
            out.push(InkPoint {
                pos: Vec2d::new(
                    a.pos.x + (b.pos.x - a.pos.x) * t,
                    a.pos.y + (b.pos.y - a.pos.y) * t,
                ),
                pressure: a.pressure + (b.pressure - a.pressure) * t as f32,
                t_ms: a.t_ms + (b.t_ms - a.t_ms) * t,
            });
            d += step;
        }
        carried = seg - (d - step);
    }
    let last = *points.last().unwrap();
    if out.last().map(|p| p.pos.dist(last.pos)).unwrap_or(f64::MAX) > step * 0.25 {
        out.push(last);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdp_collapses_a_straight_run() {
        let pts: Vec<InkPoint> = (0..50).map(|i| InkPoint::xy(i as f64, 0.0)).collect();
        let simplified = rdp(&pts, 0.5);
        assert_eq!(simplified.len(), 2);
        assert_eq!(simplified[0].pos.x, 0.0);
        assert_eq!(simplified[1].pos.x, 49.0);
    }

    #[test]
    fn rdp_keeps_a_corner() {
        let mut pts: Vec<InkPoint> = (0..20).map(|i| InkPoint::xy(i as f64, 0.0)).collect();
        pts.extend((1..20).map(|i| InkPoint::xy(19.0, i as f64)));
        let simplified = rdp(&pts, 0.5);
        assert_eq!(simplified.len(), 3); // start, corner, end
        assert_eq!(simplified[1].pos, Vec2d::new(19.0, 0.0));
    }

    #[test]
    fn one_euro_converges_and_smooths() {
        let mut f = OneEuro::new();
        // constant position with alternating noise: filtered stays near 10
        let mut last = Vec2d::ZERO;
        for i in 0..200 {
            let noise = if i % 2 == 0 { 0.6 } else { -0.6 };
            last = f.filter(i as f64 * 8.0, Vec2d::new(10.0 + noise, 5.0));
        }
        assert!((last.x - 10.0).abs() < 0.5, "x={}", last.x);
        assert!((last.y - 5.0).abs() < 1e-6);
    }

    #[test]
    fn resample_spacing() {
        let pts = vec![InkPoint::xy(0.0, 0.0), InkPoint::xy(10.0, 0.0)];
        let r = resample(&pts, 2.0);
        assert_eq!(r.len(), 6); // 0,2,4,6,8,10
        for w in r.windows(2) {
            assert!((w[0].pos.dist(w[1].pos) - 2.0).abs() < 1e-9);
        }
    }
}
