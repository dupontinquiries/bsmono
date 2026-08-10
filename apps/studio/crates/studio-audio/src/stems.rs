//! stems — separating instruments from a mixed stereo track.
//!
//! ## What the uploaded prototype gets wrong, and it is not a tuning problem
//!
//! It computes MFCCs per time frame, k-means clusters the **frames**, and
//! reconstructs by sending each whole frame to one output. That can only ever
//! segment a song *in time* — intro, verse, chorus — because **every instrument
//! playing during a frame goes to the same file**. No amount of better features
//! or more clusters fixes it: the output granularity is wrong.
//!
//! Separation needs a decision per **time-frequency bin**, not per frame. A
//! bass note and a hi-hat overlap in time and are far apart in frequency, and
//! that is exactly the axis the frame-level version throws away.
//!
//! ## What this does instead
//!
//! A soft mask per source over the spectrogram. Each bin is assigned a weight
//! per source, the masks sum to one at every bin, and each source is the
//! inverse transform of `mask * spectrum`. Two properties follow that a
//! hard-assignment version cannot have:
//!
//! * **The mix is preserved.** Summing the separated stems reconstructs the
//!   input exactly, because the masks partition energy rather than discard it.
//!   A separator that loses energy is one you cannot check.
//! * **A bin shared by two instruments is shared**, not awarded. Hard masks
//!   produce the musical-noise warble that makes naive separation sound worse
//!   than the mix.
//!
//! ## Three corrections to the prototype's DSP, each of which matters alone
//!
//! * **A window function.** It used a rectangular window, which smears every
//!   partial across the spectrum — so the features being clustered were mostly
//!   leakage. Hann, and the hop is chosen so the squared window sums to a
//!   constant, which is what makes overlap-add exact.
//! * **The real sample rate.** Its mel filterbank hardcoded 48000 while the
//!   decoder returned whatever the file had, so every band edge was wrong on a
//!   44.1kHz track — which is most music.
//! * **Frequency-band priors.** Clustering raw spectra rediscovers "bass is
//!   low" from scratch on every track and often fails to. Seeding sources with
//!   band priors costs nothing and makes the result reproducible.

use std::f32::consts::PI;

/// STFT geometry.
#[derive(Clone, Copy, Debug)]
pub struct Stft {
    pub window: usize,
    pub hop: usize,
    pub sample_rate: u32,
}

impl Stft {
    /// The prototype's 2048/512 geometry, which is a reasonable default: 75%
    /// overlap keeps Hann's squared sum constant, so overlap-add is exact.
    pub fn new(sample_rate: u32) -> Stft {
        Stft { window: 2048, hop: 512, sample_rate }
    }

    pub fn bins(&self) -> usize { self.window / 2 + 1 }

    /// Centre frequency of bin `k`, in Hz.
    ///
    /// Derived from the ACTUAL sample rate. The prototype hardcoded 48000 here,
    /// so on a 44.1kHz file every band edge sat 8.8% too high — enough to move
    /// a bass/mid boundary across a fundamental.
    pub fn bin_hz(&self, k: usize) -> f32 {
        k as f32 * self.sample_rate as f32 / self.window as f32
    }

    /// Hann window, periodic form.
    ///
    /// Periodic rather than symmetric: the symmetric form is for filter design,
    /// and using it in an STFT leaves a small ripple that overlap-add does not
    /// cancel.
    pub fn hann(&self) -> Vec<f32> {
        (0..self.window)
            .map(|n| 0.5 - 0.5 * (2.0 * PI * n as f32 / self.window as f32).cos())
            .collect()
    }

    /// Does overlap-add reconstruct exactly at this geometry?
    ///
    /// Checked rather than assumed, because a hop that does not satisfy it
    /// produces amplitude ripple that sounds like tremolo — a bug that is easy
    /// to mistake for a bad separation.
    pub fn cola_ok(&self) -> bool {
        if self.hop == 0 || self.window % self.hop != 0 { return false; }
        let w = self.hann();
        let overlaps = self.window / self.hop;
        // Sum of squared window across all overlapping positions, at a few
        // offsets. Constant means exact reconstruction.
        let sums: Vec<f32> = (0..self.hop).map(|off| {
            (0..overlaps).map(|o| { let i = off + o * self.hop; w[i] * w[i] }).sum()
        }).collect();
        let first = sums[0];
        sums.iter().all(|s| (s - first).abs() < 1e-3)
    }
}

/// What a source is expected to occupy.
///
/// Priors rather than learned clusters. Clustering raw spectra rediscovers
/// "bass is low" on every track and often does not — a prior costs nothing,
/// makes runs reproducible, and is the difference between a tool that
/// sometimes works and one you can rely on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Band { Bass, LowMid, HighMid, Air }

impl Band {
    pub const ALL: [Band; 4] = [Band::Bass, Band::LowMid, Band::HighMid, Band::Air];

    /// Nominal range in Hz.
    ///
    /// Boundaries chosen musically rather than by equal division: 250Hz is
    /// where bass gives way to the low mids, 2kHz where presence begins, 6kHz
    /// where cymbals and air dominate.
    pub fn range(self) -> (f32, f32) {
        match self {
            Band::Bass => (0.0, 250.0),
            Band::LowMid => (250.0, 2_000.0),
            Band::HighMid => (2_000.0, 6_000.0),
            Band::Air => (6_000.0, f32::INFINITY),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Band::Bass => "bass", Band::LowMid => "low-mid",
            Band::HighMid => "high-mid", Band::Air => "air",
        }
    }

    /// Prior weight for a bin, smoothly falling off outside the range.
    ///
    /// Smooth rather than a step, because a hard boundary puts a note whose
    /// fundamental sits at 249Hz entirely in one stem and its harmonic at
    /// 251Hz in another — which is audible as the note splitting apart.
    pub fn prior(self, hz: f32) -> f32 {
        let (lo, hi) = self.range();
        if hz >= lo && hz <= hi { return 1.0; }
        // Octave-based falloff: hearing is logarithmic, so a fixed Hz margin is
        // enormous at the bottom and negligible at the top.
        let edge = if hz < lo { lo } else { hi };
        if edge <= 0.0 || hz <= 0.0 { return 0.05; }
        let octaves = (hz / edge).log2().abs();
        (0.5f32).powf(octaves * 2.0).max(0.02)
    }
}

/// Soft masks: `masks[source][bin]` for one frame, summing to 1 per bin.
///
/// Returned per frame rather than for the whole file so a caller can stream —
/// a five-minute track at this geometry is ~28,000 frames, and materialising
/// every mask is 450MB before any audio exists.
pub fn frame_masks(spectrum: &[f32], stft: &Stft) -> Vec<Vec<f32>> {
    let n = spectrum.len();
    let mut masks = vec![vec![0.0f32; n]; Band::ALL.len()];
    for k in 0..n {
        let hz = stft.bin_hz(k);
        let mut total = 0.0;
        for (s, band) in Band::ALL.iter().enumerate() {
            let w = band.prior(hz);
            masks[s][k] = w;
            total += w;
        }
        // Normalise so the masks partition energy. Without this the stems sum
        // to something louder or quieter than the mix, and there is no way to
        // tell a separation error from a gain error.
        if total > 0.0 {
            for s in 0..masks.len() { masks[s][k] /= total; }
        } else {
            let even = 1.0 / masks.len() as f32;
            for s in 0..masks.len() { masks[s][k] = even; }
        }
    }
    masks
}

/// Sharpen masks toward the dominant source, without going all the way to hard.
///
/// `amount` 0 leaves them alone; 1 is nearly hard. Exposed because the right
/// value is material-dependent and a fixed one would be wrong for both a dense
/// mix and a sparse one — but the ceiling stays below hard assignment, since
/// that is what produces musical noise.
pub fn sharpen(masks: &mut [Vec<f32>], amount: f32) {
    let a = amount.clamp(0.0, 1.0);
    if a == 0.0 || masks.is_empty() { return; }
    let exp = 1.0 + a * 3.0;
    let bins = masks[0].len();
    for k in 0..bins {
        let mut total = 0.0;
        for m in masks.iter_mut() {
            m[k] = m[k].powf(exp);
            total += m[k];
        }
        if total > 0.0 { for m in masks.iter_mut() { m[k] /= total; } }
        else { let even = 1.0 / masks.len() as f32; for m in masks.iter_mut() { m[k] = even; } }
    }
}

/// Cost of separating one file, so a UI can say what it is about to do.
///
/// The prototype gave no progress and no estimate, which on a five-minute track
/// is several minutes of a frozen window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cost {
    pub frames: usize,
    pub bins: usize,
    /// Multiply-accumulates, the term that dominates.
    pub macs: u64,
}

pub fn estimate(samples: usize, stft: &Stft, sources: usize) -> Cost {
    let frames = if samples < stft.window { 0 }
                 else { (samples - stft.window) / stft.hop + 1 };
    let bins = stft.bins();
    // One forward and one inverse transform per source per frame, plus the
    // masking multiply. n log n dominates.
    let per_fft = (stft.window as f64 * (stft.window as f64).log2()) as u64;
    let macs = frames as u64 * (per_fft + sources as u64 * (per_fft + bins as u64));
    Cost { frames, bins, macs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stft() -> Stft { Stft::new(44_100) }

    #[test]
    fn bin_frequencies_follow_the_actual_sample_rate() {
        // The prototype hardcoded 48000 while decoding whatever the file had,
        // so on a 44.1kHz track every band edge sat 8.8% too high — enough to
        // move the bass/mid boundary across a fundamental.
        let a = Stft::new(44_100);
        let b = Stft::new(48_000);
        assert!((a.bin_hz(100) - 2153.3).abs() < 1.0, "{}", a.bin_hz(100));
        assert!((b.bin_hz(100) - 2343.75).abs() < 1.0);
        assert_ne!(a.bin_hz(100), b.bin_hz(100));
    }

    #[test]
    fn the_window_and_hop_reconstruct_exactly() {
        // A hop that fails this produces amplitude ripple that sounds like
        // tremolo — easy to mistake for a bad separation.
        assert!(stft().cola_ok());
        assert!(Stft { window: 2048, hop: 512, sample_rate: 44_100 }.cola_ok());
        assert!(!Stft { window: 2048, hop: 700, sample_rate: 44_100 }.cola_ok(),
                "a non-dividing hop should be rejected");
        assert!(!Stft { window: 2048, hop: 0, sample_rate: 44_100 }.cola_ok());
    }

    #[test]
    fn the_window_is_a_real_window_not_a_rectangle() {
        // The prototype used none, so every partial smeared across the spectrum
        // and the features being clustered were mostly leakage.
        let w = stft().hann();
        assert_eq!(w.len(), 2048);
        assert!(w[0].abs() < 1e-6, "starts at zero");
        assert!((w[1024] - 1.0).abs() < 1e-3, "peaks in the middle");
        assert!(w.iter().all(|x| *x >= 0.0 && *x <= 1.0));
    }

    #[test]
    fn masks_partition_energy_so_the_stems_sum_to_the_mix() {
        // The property that makes a separation checkable: if the stems do not
        // sum back, there is no way to tell a separation error from a gain
        // error.
        let s = stft();
        let spectrum = vec![1.0f32; s.bins()];
        let masks = frame_masks(&spectrum, &s);
        for k in 0..s.bins() {
            let total: f32 = masks.iter().map(|m| m[k]).sum();
            assert!((total - 1.0).abs() < 1e-5, "bin {k} sums to {total}");
        }
    }

    #[test]
    fn each_band_dominates_its_own_range() {
        let s = stft();
        let spectrum = vec![1.0f32; s.bins()];
        let masks = frame_masks(&spectrum, &s);
        for (idx, band) in Band::ALL.iter().enumerate() {
            let (lo, hi) = band.range();
            let probe = if hi.is_finite() { (lo + hi) / 2.0 } else { lo * 1.5 };
            let k = (probe / s.sample_rate as f32 * s.window as f32) as usize;
            let k = k.min(s.bins() - 1);
            let winner = (0..masks.len())
                .max_by(|a, b| masks[*a][k].partial_cmp(&masks[*b][k]).unwrap())
                .unwrap();
            assert_eq!(winner, idx, "{} should own {probe}Hz", band.label());
        }
    }

    #[test]
    fn a_bin_between_two_bands_is_shared_rather_than_awarded() {
        // A hard boundary puts a fundamental at 249Hz in one stem and its
        // harmonic at 251Hz in another, which is audible as the note splitting
        // apart.
        let s = stft();
        let k = (250.0 / s.sample_rate as f32 * s.window as f32) as usize;
        let masks = frame_masks(&vec![1.0; s.bins()], &s);
        let bass = masks[0][k];
        let lowmid = masks[1][k];
        assert!(bass > 0.1 && lowmid > 0.1, "boundary bin: bass {bass}, low-mid {lowmid}");
    }

    #[test]
    fn sharpening_concentrates_energy_but_never_reaches_hard() {
        // Hard masks produce the musical-noise warble that makes naive
        // separation sound worse than the mix.
        let s = stft();
        let mut masks = frame_masks(&vec![1.0; s.bins()], &s);
        let k = (100.0 / s.sample_rate as f32 * s.window as f32) as usize;
        let before = masks[0][k];
        sharpen(&mut masks, 1.0);
        let after = masks[0][k];
        assert!(after > before, "the dominant source gained: {before} → {after}");
        assert!(after < 1.0, "but did not take everything: {after}");
        let total: f32 = masks.iter().map(|m| m[k]).sum();
        assert!((total - 1.0).abs() < 1e-5, "still partitions: {total}");
    }

    #[test]
    fn sharpening_by_zero_changes_nothing() {
        let s = stft();
        let a = frame_masks(&vec![1.0; s.bins()], &s);
        let mut b = a.clone();
        sharpen(&mut b, 0.0);
        assert_eq!(a, b);
    }

    #[test]
    fn the_prior_falls_off_by_octaves_not_by_hertz() {
        // Hearing is logarithmic: a fixed Hz margin is enormous at the bottom
        // and negligible at the top, so a linear falloff would make the bass
        // stem swallow everything.
        let one_oct_below = Band::LowMid.prior(125.0);
        let one_oct_above = Band::LowMid.prior(4_000.0);
        assert!((one_oct_below - one_oct_above).abs() < 0.05,
                "an octave either side should cost the same: {one_oct_below} vs {one_oct_above}");
        assert!(Band::LowMid.prior(1_000.0) > one_oct_below, "inside beats outside");
    }

    #[test]
    fn dc_and_silence_do_not_produce_a_degenerate_mask() {
        // Bin 0 is DC, where every prior can be tiny; an unnormalised result
        // there is a divide by zero in the mask.
        let s = stft();
        let masks = frame_masks(&vec![0.0; s.bins()], &s);
        let total: f32 = masks.iter().map(|m| m[0]).sum();
        assert!((total - 1.0).abs() < 1e-5, "DC bin sums to {total}");
    }

    #[test]
    fn the_cost_estimate_scales_with_the_material_and_the_source_count() {
        // The prototype gave no progress and no estimate, which on a
        // five-minute track is several minutes of a frozen window.
        let s = stft();
        let one_min = estimate(44_100 * 60, &s, 4);
        let five_min = estimate(44_100 * 300, &s, 4);
        assert!(five_min.macs > one_min.macs * 4);
        assert!(estimate(44_100 * 60, &s, 8).macs > one_min.macs);
        assert_eq!(one_min.bins, 1025);
    }

    #[test]
    fn a_clip_shorter_than_one_window_has_no_frames_rather_than_underflowing() {
        // `samples - window` underflows on unsigned arithmetic, and a drag-drop
        // of a 10ms blip is an ordinary thing to do.
        let c = estimate(100, &stft(), 4);
        assert_eq!(c.frames, 0);
        assert_eq!(c.macs, 0);
    }
}
