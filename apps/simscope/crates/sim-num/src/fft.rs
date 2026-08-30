//! fft.rs — radix-2 Cooley–Tukey, 1D and 2D.
//!
//! Written rather than imported for the same reason as everything else in `sim-num`: the transform
//! sits directly under an optical model where a sign convention or a normalisation error produces
//! a mirrored or rescaled image that still looks entirely plausible. Having it here means it is
//! tested against a naive DFT rather than trusted.
//!
//! Convention, stated because it is the thing that goes wrong: the forward transform carries
//! `e^{-2πi k n / N}` and no scaling; the inverse carries `e^{+2πi k n / N}` and divides by `N`.
//! Round-tripping is therefore the identity, which the tests assert directly.
//!
//! Sizes must be powers of two. That is a real restriction and it is the right one here: an
//! optical simulation grid is chosen by the modeller, so padding to a power of two costs nothing
//! and a mixed-radix implementation would be code with no test that needs it.

use crate::complex::C64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Forward,
    Inverse,
}

fn bit_reverse(data: &mut [C64]) {
    let n = data.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            data.swap(i, j);
        }
    }
}

/// In-place 1D transform. `data.len()` must be a power of two.
pub fn fft(data: &mut [C64], dir: Dir) {
    let n = data.len();
    assert!(n.is_power_of_two(), "fft length {n} is not a power of two");
    if n <= 1 {
        return;
    }
    bit_reverse(data);
    let sign = if dir == Dir::Forward { -1.0 } else { 1.0 };
    let mut len = 2usize;
    while len <= n {
        let theta = sign * std::f64::consts::TAU / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                // Recomputing the twiddle from the angle rather than accumulating it: an
                // accumulated rotation drifts as O(n·eps) in phase, which shows up as a slow
                // spurious tilt across the spectrum.
                let w = C64::expi(theta * k as f64);
                let u = data[start + k];
                let v = data[start + k + len / 2] * w;
                data[start + k] = u + v;
                data[start + k + len / 2] = u - v;
            }
        }
        len <<= 1;
    }
    if dir == Dir::Inverse {
        let s = 1.0 / n as f64;
        for v in data.iter_mut() {
            *v = v.scale(s);
        }
    }
}

/// In-place 2D transform on a row-major `n × n` grid.
pub fn fft2(data: &mut [C64], n: usize, dir: Dir) {
    assert_eq!(data.len(), n * n, "fft2 expects a square grid");
    let mut row = vec![C64::default(); n];
    for r in 0..n {
        row.copy_from_slice(&data[r * n..(r + 1) * n]);
        fft(&mut row, dir);
        data[r * n..(r + 1) * n].copy_from_slice(&row);
    }
    let mut col = vec![C64::default(); n];
    for c in 0..n {
        for r in 0..n {
            col[r] = data[r * n + c];
        }
        fft(&mut col, dir);
        for r in 0..n {
            data[r * n + c] = col[r];
        }
    }
}

/// Move the zero frequency from the corner to the centre, which is where an optical pupil is
/// naturally described. Its own inverse for even `n`.
pub fn fftshift(data: &mut [C64], n: usize) {
    let h = n / 2;
    for r in 0..h {
        for c in 0..n {
            data.swap(r * n + c, (r + h) * n + c);
        }
    }
    for c in 0..h {
        for r in 0..n {
            data.swap(r * n + c, r * n + c + h);
        }
    }
}

/// Frequency of index `k` on an `n`-point grid of spacing `dx`, in cycles per unit length, with
/// indices above `n/2` folded to negative frequencies.
#[inline]
pub fn freq(k: usize, n: usize, dx: f64) -> f64 {
    let kk = if k <= n / 2 { k as isize } else { k as isize - n as isize };
    kk as f64 / (n as f64 * dx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::complex::c64;

    fn naive_dft(x: &[C64], dir: Dir) -> Vec<C64> {
        let n = x.len();
        let sign = if dir == Dir::Forward { -1.0 } else { 1.0 };
        let mut out = vec![C64::default(); n];
        for k in 0..n {
            let mut acc = C64::default();
            for (i, v) in x.iter().enumerate() {
                acc += *v * C64::expi(sign * std::f64::consts::TAU * (k * i) as f64 / n as f64);
            }
            out[k] = if dir == Dir::Inverse { acc.scale(1.0 / n as f64) } else { acc };
        }
        out
    }

    #[test]
    fn matches_a_naive_dft() {
        for n in [2usize, 4, 8, 64] {
            let x: Vec<C64> = (0..n)
                .map(|i| c64((i as f64 * 0.7).sin(), (i as f64 * 0.31).cos()))
                .collect();
            let want = naive_dft(&x, Dir::Forward);
            let mut got = x.clone();
            fft(&mut got, Dir::Forward);
            for k in 0..n {
                assert!((got[k] - want[k]).abs() < 1e-11, "n={n} k={k}");
            }
        }
    }

    #[test]
    fn a_round_trip_is_the_identity() {
        let n = 128;
        let x: Vec<C64> = (0..n).map(|i| c64(i as f64 % 7.0, (i as f64).sqrt())).collect();
        let mut y = x.clone();
        fft(&mut y, Dir::Forward);
        fft(&mut y, Dir::Inverse);
        for i in 0..n {
            assert!((y[i] - x[i]).abs() < 1e-12, "index {i}");
        }
    }

    #[test]
    fn a_pure_tone_transforms_to_a_single_bin() {
        // The sharpest structural check: a complex exponential at bin m must put all its energy in
        // bin m and nothing anywhere else. A sign error in the twiddle puts it in bin n-m.
        let (n, m) = (64usize, 5);
        let x: Vec<C64> = (0..n)
            .map(|i| C64::expi(std::f64::consts::TAU * (m * i) as f64 / n as f64))
            .collect();
        let mut y = x.clone();
        fft(&mut y, Dir::Forward);
        for k in 0..n {
            let mag = y[k].abs();
            if k == m {
                assert!((mag - n as f64).abs() < 1e-9, "bin {m} should hold all the energy");
            } else {
                assert!(mag < 1e-9, "bin {k} should be empty, has {mag}");
            }
        }
    }

    #[test]
    fn parseval_holds() {
        let n = 256;
        let x: Vec<C64> = (0..n).map(|i| c64((i as f64 * 0.13).cos(), 0.0)).collect();
        let energy_x: f64 = x.iter().map(|v| v.norm_sq()).sum();
        let mut y = x.clone();
        fft(&mut y, Dir::Forward);
        let energy_y: f64 = y.iter().map(|v| v.norm_sq()).sum::<f64>() / n as f64;
        assert!((energy_x - energy_y).abs() / energy_x < 1e-12);
    }

    #[test]
    fn the_two_dimensional_transform_round_trips_and_separates() {
        let n = 32;
        let mut a: Vec<C64> = (0..n * n)
            .map(|i| c64(((i % n) as f64 * 0.2).sin() * ((i / n) as f64 * 0.4).cos(), 0.0))
            .collect();
        let orig = a.clone();
        fft2(&mut a, n, Dir::Forward);
        fft2(&mut a, n, Dir::Inverse);
        for i in 0..n * n {
            assert!((a[i] - orig[i]).abs() < 1e-12, "index {i}");
        }
    }

    #[test]
    fn fftshift_centres_the_zero_frequency_and_undoes_itself() {
        let n = 8;
        let mut a: Vec<C64> = vec![C64::default(); n * n];
        a[0] = c64(1.0, 0.0); // DC in the corner
        fftshift(&mut a, n);
        assert_eq!(a[(n / 2) * n + n / 2], c64(1.0, 0.0), "DC should land at the centre");
        fftshift(&mut a, n);
        assert_eq!(a[0], c64(1.0, 0.0), "shift is its own inverse for even n");
    }

    #[test]
    fn frequencies_fold_to_negative_above_nyquist() {
        let (n, dx) = (8usize, 0.5);
        assert_eq!(freq(0, n, dx), 0.0);
        assert!((freq(1, n, dx) - 0.25).abs() < 1e-15);
        assert!((freq(7, n, dx) + 0.25).abs() < 1e-15, "index 7 is -1, not +7");
    }
}
