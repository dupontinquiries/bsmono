//! imager — stereo width, per band.
//!
//! ## Why mid/side, and the invariant it buys
//!
//! Width is done by scaling the SIDE signal (L−R) while leaving the MID (L+R)
//! alone. That single choice gives the property that makes a widener safe to
//! use on a master:
//!
//! > **The mono fold-down does not change, whatever you do to the width.**
//!
//! Because mono is exactly the mid, and width never touches it. So you can widen
//! the top end without the phone-speaker version of your mix quietly losing the
//! hi-hats — which is precisely what happens with widening tricks based on
//! delays or phase rotation, where the fold-down cancels material you can still
//! hear in stereo. The test asserts that invariant directly, since it is the
//! whole reason to build it this way.
//!
//! ## Why per band
//!
//! The two moves every mastering engineer makes are opposite and both standard:
//! **narrow the bass** (below ~120 Hz, where wide low end smears the mix and
//! wrecks vinyl cutting) and **widen the air** (above ~8 kHz, where it costs
//! nothing and buys space). One width knob cannot do both, so the imager rides
//! the same Linkwitz–Riley splitter the multiband uses — which already
//! reconstructs to within half a dB, so an imager set to unity everywhere is
//! genuinely transparent.

use crate::multiband::Splitter;

/// Per-band width. 0 = mono, 1 = unchanged, >1 = wider.
#[derive(Debug, Clone)]
pub struct Imager {
    pub(crate) left: Splitter,
    pub(crate) right: Splitter,
    pub widths: Vec<f32>,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
}

impl Imager {
    pub fn new(rate: f32, freqs: &[f32]) -> Imager {
        let n = freqs.len() + 1;
        Imager {
            left: Splitter::new(rate, freqs),
            right: Splitter::new(rate, freqs),
            widths: vec![1.0; n],
            scratch_l: vec![0.0; n],
            scratch_r: vec![0.0; n],
        }
    }

    pub fn bands(&self) -> usize { self.widths.len() }

    /// The classic mastering starting point: bass in mono, air widened.
    pub fn mastering_default(&mut self) {
        let n = self.bands();
        if n >= 1 { self.widths[0] = 0.0; }        // bass to mono
        if n >= 3 { self.widths[n - 1] = 1.35; }   // air wider
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let n = self.bands();
        self.left.split(l, &mut self.scratch_l);
        self.right.split(r, &mut self.scratch_r);
        let (mut ol, mut or) = (0.0f32, 0.0f32);
        for i in 0..n {
            let (bl, br) = (self.scratch_l[i], self.scratch_r[i]);
            // mid is left alone; only side scales. This is the whole design.
            let mid = (bl + br) * 0.5;
            let side = (bl - br) * 0.5 * self.widths[i].max(0.0);
            ol += mid + side;
            or += mid - side;
        }
        (ol, or)
    }

    pub fn reset(&mut self) { self.left.reset(); self.right.reset(); }
}

/// A goniometer (Lissajous) point: the mid/side pair rotated 45° so a mono
/// signal draws a vertical line and an out-of-phase one draws a horizontal
/// one — the orientation every hardware vectorscope uses, and the reason
/// engineers can read it at a glance.
#[inline]
pub fn goniometer_point(l: f32, r: f32) -> (f32, f32) {
    let mid = (l + r) * std::f32::consts::FRAC_1_SQRT_2;
    let side = (l - r) * std::f32::consts::FRAC_1_SQRT_2;
    (side, mid)
}

/// Decimate a stereo buffer into goniometer points for the display. Drawing
/// every sample of a 48 kHz buffer is both illegible and wasteful; a few
/// thousand points is the same picture.
pub fn goniometer(left: &[f32], right: &[f32], max_points: usize) -> Vec<(f32, f32)> {
    let n = left.len().min(right.len());
    if n == 0 || max_points == 0 { return Vec::new(); }
    let step = (n / max_points).max(1);
    (0..n).step_by(step).map(|i| goniometer_point(left[i], right[i])).collect()
}

/// Correlation per band — the meter that tells you *where* a mix will collapse
/// on a phone, rather than just that it will.
pub fn band_correlation(imager: &mut Imager, left: &[f32], right: &[f32]) -> Vec<f32> {
    let n = imager.bands();
    let mut lr = vec![0.0f64; n];
    let mut ll = vec![0.0f64; n];
    let mut rr = vec![0.0f64; n];
    let (mut sl, mut sr) = (vec![0.0f32; n], vec![0.0f32; n]);
    imager.reset();
    for i in 0..left.len().min(right.len()) {
        imager.left.split(left[i], &mut sl);
        imager.right.split(right[i], &mut sr);
        for b in 0..n {
            lr[b] += (sl[b] * sr[b]) as f64;
            ll[b] += (sl[b] * sl[b]) as f64;
            rr[b] += (sr[b] * sr[b]) as f64;
        }
    }
    (0..n).map(|b| {
        let d = (ll[b] * rr[b]).sqrt();
        if d < 1e-12 { 1.0 } else { (lr[b] / d).clamp(-1.0, 1.0) as f32 }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, n: usize, amp: f32, phase: f32) -> Vec<f32> {
        (0..n).map(|i| (TAU * hz * i as f32 / RATE + phase).sin() * amp).collect()
    }

    fn rms(v: &[f32], from: usize) -> f64 {
        (v[from..].iter().map(|x| (x * x) as f64).sum::<f64>() / (v.len() - from) as f64).sqrt()
    }

    /// A stereo pair with genuinely different content per side.
    fn stereo(n: usize) -> (Vec<f32>, Vec<f32>) {
        let l: Vec<f32> = tone(220.0, n, 0.4, 0.0).iter()
            .zip(tone(3000.0, n, 0.2, 0.0).iter()).map(|(a, b)| a + b).collect();
        let r: Vec<f32> = tone(220.0, n, 0.4, 0.0).iter()
            .zip(tone(3000.0, n, 0.2, 1.1).iter()).map(|(a, b)| a + b).collect();
        (l, r)
    }

    #[test]
    fn the_mono_fold_down_never_changes_however_wide_you_go() {
        // THE invariant, and the reason to do width in mid/side at all: mono IS
        // the mid, and width only scales the side. So you can widen the top end
        // without the phone-speaker version of the mix quietly losing material —
        // which is exactly what delay- or phase-based widening does.
        let n = 12_288;
        let (l, r) = stereo(n);
        let mut flat = Imager::new(RATE, &[120.0, 8000.0]);
        let mut wide = Imager::new(RATE, &[120.0, 8000.0]);
        wide.widths = vec![0.0, 1.8, 2.5];

        let mut mono_flat = Vec::with_capacity(n);
        let mut mono_wide = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = flat.process(l[i], r[i]);
            mono_flat.push(a + b);
            let (c, d) = wide.process(l[i], r[i]);
            mono_wide.push(c + d);
        }
        for i in 8192..n {
            assert!((mono_flat[i] - mono_wide[i]).abs() < 1e-4,
                    "the mono sum moved at sample {i}: {} vs {}", mono_flat[i], mono_wide[i]);
        }
    }

    #[test]
    fn unity_width_everywhere_is_transparent() {
        // An imager that colours at rest cannot be left in a chain — and this
        // only holds because the splitter underneath reconstructs.
        let n = 12_288;
        let (l, r) = stereo(n);
        let mut im = Imager::new(RATE, &[120.0, 8000.0]);
        let (mut ol, mut or) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..n {
            let (a, b) = im.process(l[i], r[i]);
            ol.push(a); or.push(b);
        }
        let dl = 20.0 * (rms(&ol, 8192) / rms(&l, 8192)).max(1e-9).log10();
        let dr = 20.0 * (rms(&or, 8192) / rms(&r, 8192)).max(1e-9).log10();
        assert!(dl.abs() < 0.5 && dr.abs() < 0.5, "left {dl:+.2} dB, right {dr:+.2} dB");
    }

    #[test]
    fn zero_width_on_a_band_puts_that_band_in_mono() {
        // The bass move: below the crossover the two channels become identical,
        // and above it they do not.
        let n = 12_288;
        let (l, r) = stereo(n);
        let mut im = Imager::new(RATE, &[1000.0]);
        im.widths = vec![0.0, 1.0]; // low mono, high untouched

        let (mut ol, mut or) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..n {
            let (a, b) = im.process(l[i], r[i]);
            ol.push(a); or.push(b);
        }
        // the side signal below the crossover should be gone
        let side: Vec<f32> = ol.iter().zip(&or).map(|(a, b)| a - b).collect();
        let s = crate::analysis::Spectrum::analyze(&side[8192..12_288], RATE);
        let low_side = s.band_db(180.0, 260.0);
        let high_side = s.band_db(2800.0, 3200.0);
        assert!(high_side > low_side + 20.0,
                "the low band should be mono, the high untouched: low={low_side:.1} high={high_side:.1}");
    }

    #[test]
    fn widening_a_band_only_widens_that_band() {
        let n = 12_288;
        let (l, r) = stereo(n);
        let measure = |widths: Vec<f32>| -> (f32, f32) {
            let mut im = Imager::new(RATE, &[1000.0]);
            im.widths = widths;
            let (mut ol, mut or) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for i in 0..n {
                let (a, b) = im.process(l[i], r[i]);
                ol.push(a); or.push(b);
            }
            let side: Vec<f32> = ol.iter().zip(&or).map(|(a, b)| a - b).collect();
            let s = crate::analysis::Spectrum::analyze(&side[8192..12_288], RATE);
            (s.band_db(180.0, 260.0), s.band_db(2800.0, 3200.0))
        };
        let (low_a, high_a) = measure(vec![1.0, 1.0]);
        let (low_b, high_b) = measure(vec![1.0, 2.0]);
        assert!(high_b > high_a + 4.0, "the high band widened: {high_a:.1} → {high_b:.1}");
        assert!((low_b - low_a).abs() < 1.5, "the low band did not: {low_a:.1} → {low_b:.1}");
    }

    #[test]
    fn the_goniometer_orients_the_way_engineers_read_it() {
        // Mono draws a VERTICAL line and out-of-phase a horizontal one — the
        // orientation every hardware vectorscope uses, which is why it can be
        // read at a glance.
        let (x, y) = goniometer_point(0.5, 0.5);
        assert!(x.abs() < 1e-6, "mono has no side component");
        assert!(y.abs() > 0.5, "and lands up the vertical axis");
        let (x, y) = goniometer_point(0.5, -0.5);
        assert!(y.abs() < 1e-6, "out of phase has no mid");
        assert!(x.abs() > 0.5, "and lands on the horizontal");
        // decimation keeps the picture without drawing every sample
        let n = 48_000;
        let pts = goniometer(&tone(100.0, n, 0.5, 0.0), &tone(100.0, n, 0.5, 0.7), 2000);
        assert!(pts.len() <= 2000 && pts.len() > 1500, "decimated to {} points", pts.len());
        assert!(goniometer(&[], &[], 100).is_empty());
    }

    #[test]
    fn per_band_correlation_finds_where_a_mix_will_collapse() {
        // Not "this mix has phase problems" but "your 200 Hz is out of phase",
        // which is the difference between a meter you act on and one you ignore.
        let n = 12_288;
        // low band in phase, high band inverted
        let low = tone(200.0, n, 0.5, 0.0);
        let high = tone(4000.0, n, 0.3, 0.0);
        let l: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a + b).collect();
        let r: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a - b).collect();
        let mut im = Imager::new(RATE, &[1000.0]);
        let c = band_correlation(&mut im, &l, &r);
        assert_eq!(c.len(), 2);
        assert!(c[0] > 0.9, "the low band is in phase: {}", c[0]);
        assert!(c[1] < -0.9, "the high band is inverted: {}", c[1]);
    }

    #[test]
    fn the_mastering_default_is_the_move_everyone_actually_makes() {
        let mut im = Imager::new(RATE, &[120.0, 8000.0]);
        im.mastering_default();
        assert_eq!(im.widths[0], 0.0, "bass to mono");
        assert!(im.widths[2] > 1.0, "air widened");
        assert_eq!(im.widths[1], 1.0, "and the midrange left alone");
    }
}
