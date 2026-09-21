//! dualsync — which recorder file goes with which camera take, and by how much.
//!
//! [`crate::slate::align`] answers "by how much" for ONE pair someone already
//! matched. A shoot day does not arrive matched: it is a card of camera files
//! (`C0000…C0075`) and a recorder folder of sound files (`STE-000…STE-117`)
//! with different counts, different names, and no shared clock. On the film
//! this module was written against, 77 camera files met 114 recorder files,
//! and the recorder also held room tone, false starts and chatter between
//! takes. Matching them by the numbers spoken on the slate failed on both
//! ends: whisper heard "Escena 13" as "Scenna 3", and many takes have no
//! slate in the recorder file at all.
//!
//! So this matches by LISTENING: every camera take is correlated against
//! every recorder file, and the recorder file whose sound lines up best is
//! the pair, with the offset falling out of the same correlation.
//!
//! ## The feature: onsets, not energy
//!
//! Correlating RMS envelopes ([`crate::slate::Envelope`]) directly is dominated
//! by level: a loud stretch of room tone agrees with a loud stretch of speech.
//! What two microphones in the same room agree on is WHEN things start — a
//! syllable, a door, a clap. [`Onsets`] is spectral flux in the speech band
//! (how much each frequency bin's log magnitude rose since 10 ms ago), with
//! its slow level removed and its peaks clipped, so one click cannot outvote
//! a sentence.
//!
//! ## Ranking: z, not raw correlation
//!
//! The first version ranked candidates by normalised correlation and was
//! wrong on a third of the real footage. Normalised correlation over the
//! OVERLAP rewards a short recorder file that happens to line up with one
//! burst of a long take — a 7-second file of someone saying "I'm going to
//! grab it" beat the true 2-minute take for twenty camera files. What
//! separates a true match is that its peak stands far above every other lag
//! of the same pair. That is [`Match::z`]: the peak's distance from the mean
//! of the whole correlation curve in standard deviations. Candidates are
//! ranked by z, and the overlap must cover most of the shorter file.
//!
//! ## The verdict is a rule with numbers from real footage
//!
//! Measured on that film (77 camera files, 114 recorder files, 16 kHz mono;
//! `examples/dualsync_pool.rs`, release build, 21 s): 47 takes came out
//! [`Verdict::Sure`], with z from 14.3 to 64.4 and a lead over the runner-up
//! of at least 4.6. Every one of the 47 agrees with the take's spoken slate,
//! and 43 agree exactly (same file, offset within 10 ms) with an independent
//! Python/scipy implementation of the same idea. Every other take's best
//! candidate sat at z 5.5–10.3 — wide shots with no dialogue, inserts, and
//! camera files whose sound was never recorded. The rule, z ≥ [`SURE_Z`] and a
//! lead of [`SURE_MARGIN`], sits in that gap. Everything below it is
//! [`Verdict::Unsure`] and must be listened to, which is the correct outcome
//! for a shot with nothing to hear.
//!
//! Pure: samples in, pairs out. No files, no ffmpeg.

use studio_audio::SourceAudio;
use studio_core::{Ticks, TICKS_PER_SECOND};

/// Feature rate: one value per 10 ms, the same window [`crate::slate`] uses.
pub const HOP_SECS: f64 = 0.01;

/// A take's onset strength, one value per [`HOP_SECS`].
#[derive(Debug, Clone, PartialEq)]
pub struct Onsets {
    pub values: Vec<f32>,
}

impl Onsets {
    pub fn len(&self) -> usize { self.values.len() }
    pub fn is_empty(&self) -> bool { self.values.is_empty() }
    pub fn seconds(&self) -> f64 { self.values.len() as f64 * HOP_SECS }

    /// One byte per value (0..=4 in steps of 4/255). This is the form
    /// real-footage regression fixtures are stored in: a take's onset curve
    /// is 100 bytes a second, cannot be played back as speech, and pairs the
    /// same as the float curve (checked on the film: identical 47 pairs).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.values.iter().map(|v| (v / 4.0 * 255.0).round().clamp(0.0, 255.0) as u8).collect()
    }

    pub fn from_bytes(b: &[u8]) -> Onsets {
        Onsets { values: b.iter().map(|&x| x as f32 * 4.0 / 255.0).collect() }
    }

    /// The onset feature of a take. `None` when it is too short (under a
    /// second) or silent, because neither can be matched and pretending
    /// otherwise produces a confident pairing to nothing.
    pub fn of(audio: &SourceAudio) -> Option<Onsets> {
        let sr = audio.sample_rate as f64;
        let hop = (sr * HOP_SECS).round().max(1.0) as usize;
        let frames = audio.frames();
        if frames < (sr as usize).max(hop * 100) {
            return None;
        }
        // Spectral flux in the speech band: for each 10 ms hop, a ~32 ms
        // Hann-windowed spectrum; sum over 250 Hz..6 kHz of how much each
        // bin's log magnitude ROSE since the previous hop. A first attempt
        // used the rise of a band-passed RMS envelope instead; on the film's
        // footage it paired the same files but with the true peaks at
        // z ≈ 7–12 instead of 13–40, too close to wrong candidates to call.
        // Per-bin flux sees a new syllable even when the overall level does
        // not change, which is most of dialogue.
        let win = ((0.032 * sr) as usize).next_power_of_two().max(64);
        let hann: Vec<f64> = (0..win)
            .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / win as f64).cos())
            .collect();
        let lo_bin = ((250.0 * win as f64 / sr).floor() as usize).max(1);
        let hi_bin = ((6000.0 * win as f64 / sr).ceil() as usize).min(win / 2);
        let mono: Vec<f64> = (0..frames).map(|f| audio.mono(f) as f64).collect();
        let hops = (frames - win) / hop + 1;
        let mut prev = vec![0.0f64; hi_bin - lo_bin];
        let mut flux = Vec::with_capacity(hops);
        let (mut re, mut im) = (vec![0.0f64; win], vec![0.0f64; win]);
        for h in 0..hops {
            let at = h * hop;
            for i in 0..win { re[i] = mono[at + i] * hann[i]; im[i] = 0.0; }
            fft(&mut re, &mut im, false);
            let mut rise = 0.0f64;
            for (k, b) in (lo_bin..hi_bin).enumerate() {
                let m = (1.0 + 100.0 * (re[b] * re[b] + im[b] * im[b]).sqrt()).ln();
                if h > 0 { rise += (m - prev[k]).max(0.0); }
                prev[k] = m;
            }
            flux.push(rise);
        }
        // Remove the slow level (half-second moving mean), then normalise.
        let k = 50usize;
        let mut prefix = vec![0.0f64; flux.len() + 1];
        for (i, v) in flux.iter().enumerate() { prefix[i + 1] = prefix[i] + v; }
        for i in 0..flux.len() {
            let lo = i.saturating_sub(k / 2);
            let hi = (i + k / 2 + 1).min(flux.len());
            flux[i] -= (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
        }
        let mean = flux.iter().sum::<f64>() / flux.len() as f64;
        let sd = (flux.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / flux.len() as f64).sqrt();
        if sd <= 1e-12 {
            return None;
        }
        // Clip to [0, 4] sd: a click or a slammed door must not outvote the
        // hundred syllables around it.
        let values = flux.iter().map(|v| ((v / sd).clamp(0.0, 4.0)) as f32).collect();
        Some(Onsets { values })
    }
}

/// How much a pairing deserves to be believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Peak far above the curve AND clearly ahead of the next candidate.
    Sure,
    /// Something lined up, but not decisively. Listen before cutting on it.
    Unsure,
}

/// z at or above which a peak is a real one (measured, see the module doc).
///
/// z grows with the length of the correlation curve, so this bar is a
/// property of take LENGTH as well as of the match: it was measured on takes
/// of 30 s to 4 min. A correct 12 s pairing measured z ≈ 11 (testkit's
/// `a_short_take_pairs_right_but_is_not_sure`), so short inserts come out
/// Unsure even when right. That errs the safe way — listen, don't cut — and
/// a length-aware bar is the fix to make against real short takes, not this.
pub const SURE_Z: f32 = 13.0;
/// How far the best candidate must lead the runner-up.
pub const SURE_MARGIN: f32 = 3.0;
/// The share of `SURE_Z` a match needs when another recording corroborates it.
pub const CORROBORATED_Z: f32 = 0.75;
/// The overlap must cover at least this share of the shorter take.
pub const MIN_OVERLAP_SHARE: f64 = 0.6;

/// One camera take's best recorder file.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// Index into the `sounds` slice given to [`match_pool`].
    pub sound: usize,
    /// Add this to a position in the SOUND file to get the position in the
    /// PICTURE file — the same convention as [`crate::slate::Alignment`].
    /// Negative is the normal case: the recorder was rolling first.
    pub offset: Ticks,
    /// Normalised correlation at the peak, over the overlap.
    pub ncc: f32,
    /// Peak height in standard deviations of this pair's correlation curve.
    pub z: f32,
    /// The next-best recorder file that DISAGREES with the chosen one, and
    /// its z. Other mics of the same moment are not competitors -- see
    /// `corroborated_by`.
    pub runner_up: Option<(usize, f32)>,
    /// Other recordings that line up with the chosen one at exactly the offset
    /// the picture implies: a boom and two lavs of the same take. They
    /// confirm the match rather than contest it.
    pub corroborated_by: Vec<usize>,
    pub verdict: Verdict,
}

impl Match {
    pub fn offset_seconds(&self) -> f64 { self.offset.0 as f64 / TICKS_PER_SECOND as f64 }
}

/// Correlate one picture take against one sound take.
///
/// Returns `(lag, ncc, z)` where `lag` is in feature windows and positive
/// means the sound's content happens LATER in the sound file than in the
/// picture file (`sound_pos = picture_pos + lag`). `None` when the two
/// cannot overlap enough to be compared at all.
pub fn correlate(picture: &Onsets, sound: &Onsets) -> Option<(isize, f32, f32)> {
    let prep = Prepared::new(picture.values.len() + sound.values.len());
    let p = prep.spectrum(&picture.values, true);
    let s = prep.spectrum(&sound.values, false);
    prep.correlate(&p, picture, &s, sound)
}

/// Pair every picture take with its best sound take.
///
/// One entry per picture, in order. `None` for a picture no sound could be
/// compared with (too short, or the pool was empty).
pub fn match_pool(pictures: &[Onsets], sounds: &[Onsets]) -> Vec<Option<Match>> {
    let longest_p = pictures.iter().map(|p| p.len()).max().unwrap_or(0);
    let longest_s = sounds.iter().map(|s| s.len()).max().unwrap_or(0);
    if longest_p == 0 || longest_s == 0 {
        return vec![None; pictures.len()];
    }
    // One transform size for the whole pool, so every sound is transformed
    // once instead of once per picture: 77 × 114 pairs is then 77 + 114
    // forward transforms and 8,778 inverse ones.
    let prep = Prepared::new(longest_p + longest_s);
    let sound_specs: Vec<Spectrum> = sounds.iter().map(|s| prep.spectrum(&s.values, false)).collect();
    pictures.iter().map(|pic| {
        if pic.is_empty() { return None; }
        let ps = prep.spectrum(&pic.values, true);
        let mut scored: Vec<(usize, isize, f32, f32)> = sounds.iter().enumerate()
            .filter_map(|(i, s)| prep.correlate(&ps, pic, &sound_specs[i], s)
                .map(|(lag, ncc, z)| (i, lag, ncc, z)))
            .collect();
        scored.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        let &(sound, lag, ncc, z) = scored.first()?;
        // The runner-up is the best recording that DISAGREES with this one.
        // With several mics on one take (boom plus lavs), the second-best
        // score is another mic of the SAME moment: ranked plainly, it made
        // every such take "unsure" -- 0 of 6 in the crowded-shoot test, with
        // every pairing right. Two recordings of one moment line up with
        // EACH OTHER at the offset their picture lags imply; those are
        // corroboration and are skipped. Only the next few are checked:
        // agreement is decided among the strongest, and it keeps the cost
        // flat however large the pool.
        let mut corroborated_by = Vec::new();
        let mut runner_up = None;
        for &(i, lag_i, _, z_i) in scored.iter().skip(1).take(6) {
            // correlate() sizes and time-reverses for THIS pair: the pool's
            // shared transform is sized for picture + sound and expects a
            // reversed picture spectrum, and reusing it here computed a
            // convolution that silently agreed with nothing
            let agrees = z_i >= SURE_Z * 0.6 && correlate(&sounds[sound], &sounds[i])
                .map(|(l, _, zz)| zz >= SURE_Z && (l - (lag_i - lag)).abs() <= 2)
                .unwrap_or(false);
            if agrees { corroborated_by.push(i); } else { runner_up = Some((i, z_i)); break; }
        }
        if runner_up.is_none() {
            runner_up = scored.iter().skip(1).find(|r| !corroborated_by.contains(&r.0)).map(|r| (r.0, r.3));
        }
        let lead = runner_up.map(|r| z - r.1).unwrap_or(f32::INFINITY);
        // A second, independent recording confirming the SAME alignment is
        // strong evidence: a false match would need two recordings to line up
        // wrongly in the same way. So a corroborated match is trusted at a
        // lower peak, still requiring a clear lead over anything that
        // disagrees. In the crowded-shoot test every take was right, its two
        // other mics agreed, the nearest decoy sat ~6 z behind -- and four of
        // six were still "unsure", only for peaks of 11-13 under a fixed 13.
        let bar = if corroborated_by.is_empty() { SURE_Z } else { SURE_Z * CORROBORATED_Z };
        let verdict = if z >= bar && lead >= SURE_MARGIN { Verdict::Sure } else { Verdict::Unsure };
        // sound_pos = picture_pos + lag  =>  picture_pos = sound_pos - lag.
        let secs = -(lag as f64) * HOP_SECS;
        Some(Match {
            sound,
            offset: Ticks((secs * TICKS_PER_SECOND as f64).round() as i64),
            ncc, z, runner_up, corroborated_by, verdict,
        })
    }).collect()
}

// ── the correlation itself ──────────────────────────────────────────────

type Spectrum = (Vec<f64>, Vec<f64>, f64);  // re, im, mean removed

struct Prepared { n: usize }

impl Prepared {
    fn new(total: usize) -> Prepared { Prepared { n: total.next_power_of_two().max(2) } }

    /// Mean-removed, zero-padded transform; the picture is time-reversed so
    /// that multiplying spectra correlates rather than convolves.
    fn spectrum(&self, v: &[f32], reverse: bool) -> Spectrum {
        let mean = v.iter().map(|&x| x as f64).sum::<f64>() / v.len().max(1) as f64;
        let mut re = vec![0.0f64; self.n];
        let mut im = vec![0.0f64; self.n];
        for (i, &x) in v.iter().enumerate() {
            let j = if reverse { v.len() - 1 - i } else { i };
            re[j] = x as f64 - mean;
        }
        fft(&mut re, &mut im, false);
        (re, im, mean)
    }

    fn correlate(&self, ps: &Spectrum, pic: &Onsets, ss: &Spectrum, snd: &Onsets) -> Option<(isize, f32, f32)> {
        let (lp, ls) = (pic.len(), snd.len());
        let mut re = vec![0.0f64; self.n];
        let mut im = vec![0.0f64; self.n];
        for k in 0..self.n {
            re[k] = ps.0[k] * ss.0[k] - ps.1[k] * ss.1[k];
            im[k] = ps.0[k] * ss.1[k] + ps.1[k] * ss.0[k];
        }
        fft(&mut re, &mut im, true);
        // Full correlation index c in [0, lp + ls - 1): sound[j] aligns with
        // picture[i] where j - i = c - (lp - 1).
        let pp = prefix_sq(&pic.values, ps.2);
        let sp = prefix_sq(&snd.values, ss.2);
        let need = ((lp.min(ls) as f64 * MIN_OVERLAP_SHARE) as usize).max(100);
        let mut curve = Vec::with_capacity(lp + ls);
        let mut best = (0isize, f32::MIN);
        for c in 0..(lp + ls - 1) {
            let lag = c as isize - (lp as isize - 1);
            // overlapping picture indices i with 0 <= i + lag < ls
            let i0 = (-lag).max(0) as usize;
            let i1 = ((ls as isize - lag).min(lp as isize)).max(0) as usize;
            if i1 <= i0 || i1 - i0 < need { continue; }
            let (j0, j1) = ((i0 as isize + lag) as usize, (i1 as isize + lag) as usize);
            let e = ((pp[i1] - pp[i0]) * (sp[j1] - sp[j0])).sqrt();
            if e <= 1e-12 { continue; }
            let v = (re[c] / e) as f32;
            curve.push(v);
            if v > best.1 { best = (lag, v); }
        }
        if curve.len() < 3 { return None; }
        let mean = curve.iter().map(|&v| v as f64).sum::<f64>() / curve.len() as f64;
        let sd = (curve.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / curve.len() as f64).sqrt();
        let z = if sd > 1e-12 { ((best.1 as f64 - mean) / sd) as f32 } else { 0.0 };
        Some((best.0, best.1, z))
    }
}

fn prefix_sq(v: &[f32], mean: f64) -> Vec<f64> {
    let mut p = vec![0.0f64; v.len() + 1];
    for (i, &x) in v.iter().enumerate() { p[i + 1] = p[i] + (x as f64 - mean).powi(2); }
    p
}

/// In-place iterative radix-2 FFT in f64 (the inverse is scaled by 1/n).
/// f64 rather than `studio_fx::analysis::fft`'s f32 because a correlation of
/// two five-minute takes sums ~30,000 products per lag, and single precision
/// loses the small peak differences the ranking depends on.
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j |= bit;
        if i < j { re.swap(i, j); im.swap(i, j); }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * std::f64::consts::TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (a, b) = (i + k, i + k + len / 2);
                let vr = re[b] * cr - im[b] * ci;
                let vi = re[b] * ci + im[b] * cr;
                re[b] = re[a] - vr; im[b] = im[a] - vi;
                re[a] += vr; im[a] += vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
    if inverse {
        let s = 1.0 / n as f64;
        for k in 0..n { re[k] *= s; im[k] *= s; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    /// Deterministic pseudo-random stream (xorshift), so tests need no crate.
    /// The seed goes through splitmix64 first: raw xorshift streams from
    /// seeds 1, 2, 3 are bit-shifted copies of each other, and spectral flux
    /// hears two "independent" noise floors built that way as the same room
    /// — the first version of these fixtures matched takes by their noise.
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Rng {
            let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            Rng((z ^ (z >> 31)) | 1)
        }
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// A "performance": syllable-like bursts at irregular times, each a
    /// voiced sound (a 100–250 Hz fundamental with harmonics up to ~6 kHz).
    /// Returned as (start_seconds, duration, f0) events.
    ///
    /// Harmonic-rich on purpose. An earlier fixture used pure tones, and a
    /// pure tone lives in one or two spectral bins while room noise lives
    /// in all of them — flux then hears the noise, not the syllables, and
    /// the test measured the fixture. Speech is broadband; so is this.
    fn performance(seed: u64, secs: f64) -> Vec<(f64, f64, f64)> {
        let mut r = Rng::new(seed);
        let mut t = 0.3;
        let mut ev = Vec::new();
        while t < secs {
            let d = 0.08 + 0.25 * r.next();
            ev.push((t, d, 100.0 + 150.0 * r.next()));
            t += d + 0.05 + 0.6 * r.next();
        }
        ev
    }

    /// Record `events` (in scene time) on a machine that started rolling at
    /// scene time `start` and ran for `len` seconds, with its own noise and gain.
    fn record(events: &[(f64, f64, f64)], start: f64, len: f64, gain: f32, noise: f32, seed: u64) -> SourceAudio {
        let n = (len * SR as f64) as usize;
        let mut r = Rng::new(seed);
        let mut s: Vec<f32> = (0..n).map(|_| noise * (r.next() as f32 * 2.0 - 1.0)).collect();
        for &(t0, d, f) in events {
            let a = ((t0 - start) * SR as f64) as isize;
            let b = a + (d * SR as f64) as isize;
            for i in a.max(0)..b.min(n as isize) {
                let tt = (i - a) as f64 / SR as f64;
                let env = (std::f64::consts::PI * tt / d).sin();
                let mut v = 0.0f64;
                let mut h = 1.0f64;
                while h * f < 6000.0 {
                    v += (std::f64::consts::TAU * h * f * tt).sin() / h;
                    h += 1.0;
                }
                s[i as usize] += gain * (0.3 * env * v) as f32;
            }
        }
        SourceAudio { sample_rate: SR, channels: vec![s] }
    }

    #[test]
    fn a_take_pairs_with_its_own_recording_and_the_offset_is_right() {
        let scene = performance(7, 40.0);
        // recorder rolled at t=0 for 40 s, camera rolled 3.5 s later for 30 s
        let sound = record(&scene, 0.0, 40.0, 0.5, 0.01, 1);
        let picture = record(&scene, 3.5, 30.0, 0.2, 0.03, 2);
        let decoy = record(&performance(99, 40.0), 0.0, 40.0, 0.5, 0.01, 3);
        let m = match_pool(&[Onsets::of(&picture).unwrap()],
                           &[Onsets::of(&decoy).unwrap(), Onsets::of(&sound).unwrap()]);
        let m = m[0].as_ref().expect("a match");
        assert_eq!(m.sound, 1, "{m:?}");
        // picture_pos = sound_pos + offset; picture t=0 is sound t=3.5
        assert!((m.offset_seconds() - -3.5).abs() <= 0.02, "offset {} {m:?}", m.offset_seconds());
        assert_eq!(m.verdict, Verdict::Sure, "{m:?}");
    }

    #[test]
    fn a_short_file_that_lines_up_with_one_burst_does_not_steal_the_take() {
        // The failure that made this module rank by z: a 3-second recorder
        // file cut from the SAME performance correlates almost perfectly
        // over its tiny overlap. It must not beat the full recording.
        let scene = performance(11, 60.0);
        let full = record(&scene, 0.0, 60.0, 0.5, 0.01, 1);
        let picture = record(&scene, 2.0, 50.0, 0.2, 0.03, 2);
        // the snippet is 3 s cut out of the picture's OWN microphone — the
        // most favourable decoy there is for raw correlation (a perfect 1.0).
        let a = (18.0 * SR as f64) as usize;
        let snippet = SourceAudio { sample_rate: SR,
            channels: vec![picture.channels[0][a..a + 3 * SR as usize].to_vec()] };
        let m = match_pool(&[Onsets::of(&picture).unwrap()],
                           &[Onsets::of(&snippet).unwrap(), Onsets::of(&full).unwrap()]);
        // Ranked by raw ncc the snippet wins (0.99 vs ~0.7) — checked by
        // swapping the sort key. Ranked by z the full take wins, but only
        // just, so the verdict must not claim more than that.
        let m = m[0].as_ref().unwrap();
        assert_eq!(m.sound, 1, "{m:?}");
        assert_eq!(m.verdict, Verdict::Unsure, "{m:?}");
    }

    #[test]
    fn a_take_with_no_recording_is_not_sure_of_anything() {
        let picture = record(&performance(5, 30.0), 0.0, 30.0, 0.2, 0.03, 2);
        let pool: Vec<Onsets> = (0..6).map(|k| {
            Onsets::of(&record(&performance(100 + k, 40.0), 0.0, 40.0, 0.5, 0.01, 300 + k)).unwrap()
        }).collect();
        let m = match_pool(&[Onsets::of(&picture).unwrap()], &pool);
        let m = m[0].as_ref().unwrap();
        assert_eq!(m.verdict, Verdict::Unsure, "{m:?}");
        // and the curve really was flat: nothing near the sure bar
        assert!(m.z < SURE_Z - 3.0, "z {}", m.z);
    }

    /// Abundance: a crowded shoot. Six camera takes of six performances; for
    /// each scene THREE mics that all genuinely match (boom plus two lavs,
    /// each rolled at its own time, so each has its own right offset); and
    /// thirty decoy recordings of performances nobody filmed. Every take must
    /// pair with one of its OWN scene's mics, at that mic's offset.
    #[test]
    fn abundance_six_takes_eighteen_true_mics_thirty_decoys() {
        let t0 = std::time::Instant::now();
        let scenes: Vec<Vec<(f64, f64, f64)>> = (0..6).map(|k| performance(500 + k, 36.0)).collect();
        let mut pool = Vec::new();
        let mut owner = Vec::new();       // pool index -> (scene, that mic's start)
        for (k, sc) in scenes.iter().enumerate() {
            for (m, (start, gain, noise)) in [(0.0, 0.5, 0.01), (1.2, 0.35, 0.02), (-0.8, 0.3, 0.025)].iter().enumerate() {
                pool.push(Onsets::of(&record(sc, *start, 36.0, *gain, *noise, 1000 + (k * 10 + m) as u64)).unwrap());
                owner.push((Some(k), *start));
            }
        }
        for d in 0..30u64 {
            pool.push(Onsets::of(&record(&performance(900 + d, 36.0), 0.0, 36.0, 0.5, 0.01, 2000 + d)).unwrap());
            owner.push((None, 0.0));
        }
        // shuffle the pool order deterministically so position is no clue
        let mut order: Vec<usize> = (0..pool.len()).collect();
        let mut r = Rng::new(42);
        for i in (1..order.len()).rev() { let j = (r.next() * (i + 1) as f64) as usize; order.swap(i, j.min(i)); }
        let pool_s: Vec<Onsets> = order.iter().map(|&i| pool[i].clone()).collect();
        let owner_s: Vec<(Option<usize>, f64)> = order.iter().map(|&i| owner[i]).collect();
        // each camera rolled 3 s after its scene started, for 28 s
        let pics: Vec<Onsets> = scenes.iter().enumerate()
            .map(|(k, sc)| Onsets::of(&record(sc, 3.0, 28.0, 0.2, 0.03, 3000 + k as u64)).unwrap()).collect();
        let built = t0.elapsed();
        let t1 = std::time::Instant::now();
        let m = match_pool(&pics, &pool_s);
        let matched = t1.elapsed();
        let mut sure = 0;
        for (k, mm) in m.iter().enumerate() {
            let mm = mm.as_ref().unwrap_or_else(|| panic!("take {k} found nothing"));
            let (scene, start) = owner_s[mm.sound];
            assert_eq!(scene, Some(k), "take {k} paired with a decoy or another scene: {mm:?}");
            // picture t=0 is scene t=3; this mic's t=0 is scene t=start; so offset = start - 3
            assert!((mm.offset_seconds() - (start - 3.0)).abs() <= 0.03, "take {k}: offset {} for a mic that rolled at {start}", mm.offset_seconds());
            if mm.verdict == Verdict::Sure { sure += 1; }
            // corroboration must come from the SAME scene, never a decoy
            assert!(mm.corroborated_by.iter().all(|i| owner_s[*i].0 == Some(k)),
                    "take {k} was 'corroborated' by another scene or a decoy: {:?}", mm.corroborated_by);
        }
        assert!(sure >= 5, "most pairings are confident even among 48 candidates: {sure}/6");
        eprintln!("abundance: 6 takes x {} recordings: built {built:?}, matched {matched:?}", pool_s.len());
        // cost scales as takes x recordings; guard against a quadratic surprise
        assert!(matched.as_secs_f64() < 60.0, "matching took {matched:?}");
    }

    #[test]
    fn many_takes_find_their_own_files_in_one_pool() {
        let mut pics = Vec::new();
        let mut snds = Vec::new();
        let offsets = [1.0, 4.2, 2.7, 7.5];
        for (k, off) in offsets.iter().enumerate() {
            let scene = performance(200 + k as u64, 35.0);
            snds.push(Onsets::of(&record(&scene, 0.0, 35.0, 0.4, 0.01, 10 + k as u64)).unwrap());
            pics.push(Onsets::of(&record(&scene, *off, 25.0, 0.3, 0.02, 20 + k as u64)).unwrap());
        }
        snds.reverse();  // pool order must not matter
        for (k, m) in match_pool(&pics, &snds).into_iter().enumerate() {
            let m = m.unwrap();
            assert_eq!(m.sound, offsets.len() - 1 - k);
            assert!((m.offset_seconds() + offsets[k]).abs() <= 0.02, "take {k}: {}", m.offset_seconds());
            assert_eq!(m.verdict, Verdict::Sure);
        }
    }

    #[test]
    fn silence_and_tiny_files_are_refused_not_matched() {
        let silent = SourceAudio { sample_rate: SR, channels: vec![vec![0.0; SR as usize * 5]] };
        assert!(Onsets::of(&silent).is_none());
        let tiny = SourceAudio { sample_rate: SR, channels: vec![vec![0.1; SR as usize / 4]] };
        assert!(Onsets::of(&tiny).is_none());
        assert_eq!(match_pool(&[], &[]), Vec::<Option<Match>>::new());
    }

    #[test]
    fn fft_roundtrips() {
        let mut re: Vec<f64> = (0..64).map(|i| (i as f64 * 0.37).sin()).collect();
        let orig = re.clone();
        let mut im = vec![0.0; 64];
        fft(&mut re, &mut im, false);
        fft(&mut re, &mut im, true);
        for (a, b) in re.iter().zip(&orig) { assert!((a - b).abs() < 1e-9); }
    }
}
