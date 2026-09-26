//! resample — band-limited sample-rate conversion, once, at load.
//!
//! The mixdown reads a source at another rate by LINEAR INTERPOLATION
//! (`mixdown_into`), which is fine for 44.1 -> 48 kHz and wrong for a
//! render at 8 or 16 kHz: going down, everything above the new Nyquist
//! folds back into the band. A render at another rate therefore converts
//! every source here first, and the mixdown then reads it sample for sample.
//!
//! Kaiser-windowed sinc (beta 8.6, about -90 dB stopband), `HALF` zero
//! crossings each side, read from a table sampled `TABLE` times per zero
//! crossing with linear interpolation between entries -- the usual
//! polyphase-by-table construction, for any ratio. The cutoff is 0.94 of
//! the LOWER Nyquist, so a downsample removes what cannot be represented
//! and an upsample adds no images.

const HALF: usize = 24;
const TABLE: usize = 512;
const BETA: f64 = 8.6;

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0f64, 1.0f64, 1.0f64);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}

/// The windowed sinc at 0, 1/TABLE, 2/TABLE, ... HALF zero crossings.
fn kernel() -> Vec<f32> {
    let n = HALF * TABLE + 2;
    let i0b = bessel_i0(BETA);
    (0..n).map(|k| {
        let x = k as f64 / TABLE as f64;
        if x >= HALF as f64 { return 0.0; }
        let sinc = if x == 0.0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
        let r = x / HALF as f64;
        let w = bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / i0b;
        (sinc * w) as f32
    }).collect()
}

/// `x` (at `from` Hz) at `to` Hz. Equal rates return the input unchanged.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() { return x.to_vec(); }
    let k = kernel();
    let ratio = to as f64 / from as f64;
    // cutoff relative to the INPUT rate's Nyquist: 1 upsampling, `ratio` down
    let cut = 0.94 * ratio.min(1.0);
    let n_out = ((x.len() as f64) * ratio).round() as usize;
    let reach = (HALF as f64 / cut).ceil() as isize;
    let tab = |d: f64| -> f32 {
        // the kernel at distance d (in input samples), scaled to the cutoff
        let p = (d.abs() * cut) * TABLE as f64;
        let i = p as usize;
        if i + 1 >= k.len() { return 0.0; }
        let f = (p - i as f64) as f32;
        k[i] * (1.0 - f) + k[i + 1] * f
    };
    (0..n_out).map(|j| {
        let t = j as f64 / ratio;
        let c = t.floor() as isize;
        let mut acc = 0.0f32;
        for i in (c - reach + 1)..=(c + reach) {
            if i < 0 || i as usize >= x.len() { continue; }
            acc += x[i as usize] * tab(t - i as f64);
        }
        acc * cut as f32
    }).collect()
}

/// Every channel of `a` at `to` Hz.
pub fn resample_audio(a: &crate::SourceAudio, to: u32) -> crate::SourceAudio {
    crate::SourceAudio { sample_rate: to, channels: a.channels.iter().map(|c| resample(c, a.sample_rate, to)).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, secs: f64) -> Vec<f32> {
        (0..(rate as f64 * secs) as usize).map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.5).collect()
    }
    fn rms(v: &[f32]) -> f64 { (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt() }
    /// the tone's amplitude at `hz` by projection (a one-bin DFT)
    fn amp(v: &[f32], hz: f64, rate: u32) -> f64 {
        let (mut c, mut s) = (0.0, 0.0);
        for (i, x) in v.iter().enumerate() { let w = std::f64::consts::TAU * hz * i as f64 / rate as f64; c += *x as f64 * w.cos(); s += *x as f64 * w.sin(); }
        2.0 * (c * c + s * s).sqrt() / v.len() as f64
    }

    #[test]
    fn a_tone_in_band_keeps_its_level_and_pitch_at_every_rate() {
        for (from, to, hz) in [(48_000u32, 8_000u32, 1_000.0f64), (48_000, 16_000, 3_000.0), (44_100, 48_000, 5_000.0),
                               (48_000, 96_000, 9_000.0), (48_000, 192_000, 15_000.0), (96_000, 44_100, 12_000.0)] {
            let y = resample(&tone(hz, from, 1.0), from, to);
            assert_eq!(y.len(), to as usize, "{from}->{to}: length");
            let mid = &y[y.len() / 4..y.len() * 3 / 4]; // away from the edges
            let a = amp(mid, hz, to);
            assert!((20.0 * (a / 0.5).log10()).abs() < 0.1, "{from}->{to} {hz} Hz: {:.3} dB", 20.0 * (a / 0.5).log10());
        }
    }

    #[test]
    fn a_tone_above_the_new_nyquist_is_removed_not_folded() {
        // 7 kHz into an 8 kHz render (Nyquist 4 kHz): linear interpolation
        // folds it to 1 kHz at nearly full level; this must remove it
        let y = resample(&tone(7_000.0, 48_000, 1.0), 48_000, 8_000);
        let mid = &y[2_000..6_000];
        let folded = amp(mid, 1_000.0, 8_000);
        assert!(20.0 * (folded / 0.5).log10() < -60.0, "folded image at {:.1} dB", 20.0 * (folded / 0.5).log10());
        assert!(rms(mid) < 0.5 * 1e-3, "and nothing else either: rms {}", rms(mid));
    }

    #[test]
    fn equal_rates_are_untouched() {
        let x = tone(440.0, 48_000, 0.1);
        assert_eq!(resample(&x, 48_000, 48_000), x);
    }
}
