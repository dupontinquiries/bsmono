//! slate — syncing dual-system sound: aligning a separately-recorded audio take to the
//! picture, by the clap.
//!
//! ## The problem this is
//!
//! On any set past a phone, sound is recorded on its own machine: the camera's onboard mic
//! is a scratch track, and the take you actually cut with came off a recorder that was never
//! electrically connected to the camera. The two files start whenever each operator pressed
//! record, and their clocks differ. Something has to say by how much.
//!
//! Timecode jam-sync answers it when both machines have timecode — and most cameras a small
//! production owns do not. The other answer is the one the clapperboard was invented for: a
//! sharp transient recorded by BOTH machines at the same instant. Line the claps up and the
//! offset falls out.
//!
//! This does that, over `studio_video::extract_audio`'s output on the picture side and any
//! `SourceAudio` on the sound side.
//!
//! ## Cross-correlation, and the two ways it goes wrong
//!
//! The alignment itself is a normalised cross-correlation of the two envelopes: slide one
//! past the other, and the lag with the strongest agreement is the offset. Two failure modes
//! worth naming, because both produce a confident, wrong answer:
//!
//! * **A periodic signal correlates with itself at many lags.** Music, a hum, a rhythm — the
//!   peak is barely higher than its neighbours, and which one wins is noise. So the result
//!   carries [`Alignment::confidence`]: the peak divided by the next-best peak that is not
//!   adjacent to it. Near 1 means "this could be any of several offsets", and the caller is
//!   expected to refuse rather than cut on it.
//! * **Correlating raw samples fails across different microphones.** A camera mic and a
//!   boom hear different frequency content of the same clap, so the waveforms do not match
//!   even when the event does. The ENVELOPE — energy over short windows — is what both
//!   machines agree on, so that is what is correlated.
//!
//! ## The clap detector is separate, and it earns its place
//!
//! Correlating whole takes is `O(n·m)` and a ten-minute take against a ten-minute take is
//! hours. [`find_clap`] locates the sharp transient in each file first — a big, fast jump in
//! the envelope after quiet — and the correlation then runs over a short window around each,
//! which is seconds. When no clap is found on one side, [`align`] falls back to correlating
//! the heads of both files, and says which method it used, because the two deserve different
//! trust.
//!
//! Pure: samples in, an offset out. No files, no ffmpeg, no clock.

use studio_audio::SourceAudio;
use studio_core::Ticks;

/// The envelope window, in milliseconds. 10 ms is short enough to place a clap to within a
/// frame at any rate here (a 24 fps frame is 41 ms) and long enough that two different
/// microphones agree on the shape.
pub const WINDOW_MS: f64 = 10.0;

/// An envelope: RMS energy per window, and how long a window is in samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub values: Vec<f32>,
    pub window: usize,
    pub sample_rate: u32,
}

impl Envelope {
    /// RMS per window over the mono sum.
    pub fn of(audio: &SourceAudio) -> Envelope {
        let window = ((audio.sample_rate as f64 * WINDOW_MS / 1000.0).round() as usize).max(1);
        let frames = audio.frames();
        let mut values = Vec::with_capacity(frames / window + 1);
        let mut i = 0;
        while i < frames {
            let end = (i + window).min(frames);
            let mut sum = 0.0f64;
            for f in i..end {
                let s = audio.mono(f) as f64;
                sum += s * s;
            }
            values.push((sum / (end - i) as f64).sqrt() as f32);
            i = end;
        }
        Envelope { values, window, sample_rate: audio.sample_rate }
    }

    pub fn seconds_per_window(&self) -> f64 {
        self.window as f64 / self.sample_rate as f64
    }
}

/// A clap: which envelope window it is in, and how sharp it was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clap {
    pub window: usize,
    /// How many times louder the transient is than the second before it. A clap is tens;
    /// a door closing mid-sentence is two or three.
    pub sharpness: f32,
}

/// The smallest jump that counts as a clap: eight times the preceding second's level.
///
/// A clapperboard is enormously louder than room tone — 30× and up is ordinary. Eight is
/// low enough to catch a hand clap in a small room and high enough to ignore speech, which
/// rises by two or three.
pub const CLAP_RATIO: f32 = 8.0;

/// Find the clap: the sharpest rise over the preceding second, at the head of the take.
///
/// `search_secs` bounds the search because a clap is at the START — that is the point of
/// slating. Searching the whole file finds the loudest event instead, which on an action
/// take is a door slam in the middle and on a music take is the downbeat.
pub fn find_clap(env: &Envelope, search_secs: f64) -> Option<Clap> {
    let spw = env.seconds_per_window();
    let limit = ((search_secs / spw).round() as usize).min(env.values.len());
    let back = ((1.0 / spw).round() as usize).max(1);
    let mut best: Option<Clap> = None;
    for i in 1..limit {
        let from = i.saturating_sub(back);
        // The floor keeps a transient after true digital silence from reading as infinite;
        // -60 dBFS is quieter than any room this would be used in.
        let before = env.values[from..i].iter().copied().fold(0.0f32, f32::max).max(0.001);
        let ratio = env.values[i] / before;
        if ratio >= CLAP_RATIO && best.map(|b| ratio > b.sharpness).unwrap_or(true) {
            best = Some(Clap { window: i, sharpness: ratio });
        }
    }
    best
}

/// How the offset was arrived at, because the two do not deserve equal trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// A clap was found in both and the correlation ran around it. The ordinary case.
    Clap,
    /// No clap on one side; the heads of both files were correlated instead. Works on
    /// continuous sound (an interview with both mics running) and is worth less on a take
    /// that starts with silence.
    Correlation,
}

/// The answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alignment {
    /// Add this to a position in the SOUND file to get the position in the PICTURE file.
    /// Negative means the sound recorder was rolling first, which is the normal case —
    /// sound rolls, then camera, then the slate.
    pub offset: Ticks,
    /// Peak over next-best. 1.0 means the correlation could not tell several offsets apart;
    /// above about 1.5 the peak is a real one.
    pub confidence: f32,
    pub method: Method,
}

impl Alignment {
    /// Is this worth cutting on? A caller that ignores confidence gets a confident wrong
    /// answer on any periodic material.
    pub fn trustworthy(&self) -> bool {
        self.confidence >= 1.5
    }
}

/// Normalised cross-correlation of two envelope slices at every lag, returning
/// `(best_lag, confidence)`. `lag` is `b`'s position relative to `a`, in windows.
fn best_lag(a: &[f32], b: &[f32], max_lag: usize) -> Option<(isize, f32)> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let score = |lag: isize| -> f32 {
        let mut dot = 0.0f64;
        let mut na = 0.0f64;
        let mut nb = 0.0f64;
        for (i, av) in a.iter().enumerate() {
            let j = i as isize - lag;
            if j < 0 || j as usize >= b.len() {
                continue;
            }
            let bv = b[j as usize] as f64;
            dot += *av as f64 * bv;
            na += (*av as f64) * (*av as f64);
            nb += bv * bv;
        }
        if na <= 0.0 || nb <= 0.0 {
            return 0.0;
        }
        (dot / (na.sqrt() * nb.sqrt())) as f32
    };
    let lo = -(max_lag as isize);
    let hi = max_lag as isize;
    let mut scores: Vec<(isize, f32)> = (lo..=hi).map(|l| (l, score(l))).collect();
    let (best_l, best_s) = *scores
        .iter()
        .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))?;
    if best_s <= 0.0 {
        return None;
    }
    // The next-best peak that is not part of this one: at least three windows away, so the
    // shoulder of the same peak does not count as a rival.
    scores.retain(|(l, _)| (l - best_l).abs() > 3);
    let rival = scores
        .iter()
        .map(|(_, s)| *s)
        .fold(0.0f32, f32::max)
        .max(0.0001);
    Some((best_l, best_s / rival))
}

/// Align a sound take to a picture take.
///
/// `picture` is the camera's scratch audio (`studio_video::extract_audio`), `sound` the
/// recorder's file. `search_secs` is how far into each to look for the slate.
pub fn align(picture: &SourceAudio, sound: &SourceAudio, search_secs: f64) -> Option<Alignment> {
    let pe = Envelope::of(picture);
    let se = Envelope::of(sound);
    if pe.values.is_empty() || se.values.is_empty() {
        return None;
    }
    // The two files can be at different sample rates — a camera at 48 k and a recorder at
    // 96 k is ordinary — but the envelope is per WINDOW and the window is a fixed duration,
    // so both envelopes are already on the same time base. That is the other reason to
    // correlate envelopes rather than samples: no resampling.
    let spw = pe.seconds_per_window();
    let (pc, sc) = (find_clap(&pe, search_secs), find_clap(&se, search_secs));
    let (a, b, base, method) = match (pc, sc) {
        (Some(p), Some(s)) => {
            // A window around each clap: a quarter-second before, two seconds after. Long
            // enough for the correlation to have something to agree on, short enough that
            // this is instant on a ten-minute take.
            let back = ((0.25 / spw).round() as usize).max(1);
            let fwd = ((2.0 / spw).round() as usize).max(4);
            let pa = p.window.saturating_sub(back);
            let sa = s.window.saturating_sub(back);
            let pv = &pe.values[pa..(p.window + fwd).min(pe.values.len())];
            let sv = &se.values[sa..(s.window + fwd).min(se.values.len())];
            (pv.to_vec(), sv.to_vec(), pa as isize - sa as isize, Method::Clap)
        }
        _ => {
            // No slate: correlate the heads and let the confidence say how much to believe.
            let n = ((search_secs / spw).round() as usize).max(8);
            let pv = pe.values[..n.min(pe.values.len())].to_vec();
            let sv = se.values[..n.min(se.values.len())].to_vec();
            (pv, sv, 0, Method::Correlation)
        }
    };
    let max_lag = a.len().min(b.len()).max(2) / 2;
    let (lag, confidence) = best_lag(&a, &b, max_lag)?;
    // `lag` is sound's position relative to picture WITHIN the compared slices; `base` puts
    // the slices back where they came from.
    let windows = base + lag;
    let secs = windows as f64 * spw;
    Some(Alignment {
        offset: Ticks((secs * studio_core::TICKS_PER_SECOND as f64).round() as i64),
        confidence,
        method,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A take from one machine.
    ///
    /// `starts_at` is when this machine began recording, on the SET's clock; `clap_abs` is
    /// when the slate hit, also on the set's clock. The content is a function of ABSOLUTE
    /// time, which is the whole point: both machines hear the same room, and what differs
    /// between the files is only where each one starts. A generator keyed to in-file time
    /// instead makes the two files identical, and then the correlation's true answer is
    /// zero for any pair — which is how a passing test can hide a broken aligner.
    fn take(starts_at: f64, dur: f64, seed: u32, clap_abs: Option<f64>, clap_gain: f32) -> SourceAudio {
        let n = (SR as f64 * dur) as usize;
        let mut ch = Vec::with_capacity(n);
        let mut r = seed.max(1);
        for i in 0..n {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            // this machine's own noise floor — different mic, different noise
            let noise = (r as f32 / u32::MAX as f32 - 0.5) * 0.02;
            let t = starts_at + i as f64 / SR as f64;
            // The room, in absolute time: a slow amplitude pattern both machines hear.
            // Quiet on purpose — dialogue sits around −20 dBFS and a slate peaks near 0,
            // which is the ratio `CLAP_RATIO` is calibrated for. Content at the same level
            // as the clap would mean there was nothing to find.
            let content = (0.05 * (1.0 + (t * 3.1).sin()) * (t * 220.0).sin()) as f32;
            let clap = match clap_abs {
                Some(c) if t >= c && t < c + 0.010 => clap_gain,
                _ => 0.0,
            };
            ch.push(noise + content + clap);
        }
        SourceAudio { sample_rate: SR, channels: vec![ch] }
    }

    #[test]
    fn the_envelope_is_energy_per_window_at_a_fixed_duration() {
        let a = take(0.0, 3.0, 7, Some(1.0), 0.9);
        let e = Envelope::of(&a);
        assert_eq!(e.window, 480, "10 ms at 48 kHz");
        assert!((e.seconds_per_window() - 0.010).abs() < 1e-9);
        assert!(e.values.len() >= 299 && e.values.len() <= 301, "{}", e.values.len());
        // a 96 kHz file has twice the samples per window and the SAME time base, which is
        // what lets two machines at different rates be compared without resampling
        let mut b = a.clone();
        b.sample_rate = 96_000;
        let eb = Envelope::of(&b);
        assert_eq!(eb.window, 960);
        assert!((eb.seconds_per_window() - 0.010).abs() < 1e-9);
    }

    #[test]
    fn the_clap_is_found_at_the_head_and_speech_is_not_mistaken_for_one() {
        let a = take(0.0, 5.0, 11, Some(1.5), 0.9);
        let e = Envelope::of(&a);
        let c = find_clap(&e, 4.0).expect("a clap");
        // within two windows (20 ms) of where it was put
        assert!((c.window as isize - 150).abs() <= 2, "window {}", c.window);
        assert!(c.sharpness > CLAP_RATIO);
        // the same room with no slate: the content alone must not read as one
        let quiet = take(0.0, 5.0, 11, None, 0.0);
        assert!(find_clap(&Envelope::of(&quiet), 4.0).is_none(), "the room is not a clap");
    }

    /// The case the clapperboard exists for: two machines, started at different moments,
    /// hearing the same clap.
    #[test]
    fn two_takes_of_the_same_clap_align_to_their_true_offset() {
        // Sound rolled 1.2 s before camera, and the slate hit at 0.8 on the set's clock:
        // the clap is 2.0 s into the sound file and 0.8 s into the picture file, so a
        // position in sound is 1.2 s LATER than the same moment in picture.
        let sound = take(-1.2, 6.0, 3, Some(0.8), 0.9);
        let picture = take(0.0, 5.0, 99, Some(0.8), 0.7); // a different mic: other noise, other level
        let al = align(&picture, &sound, 4.0).expect("an alignment");
        assert_eq!(al.method, Method::Clap);
        let secs = al.offset.0 as f64 / studio_core::TICKS_PER_SECOND as f64;
        assert!((secs + 1.2).abs() < 0.03, "offset {secs}s, expected −1.2");
        assert!(al.trustworthy(), "confidence {}", al.confidence);
        // applying it lines the claps up
        let pc = find_clap(&Envelope::of(&picture), 4.0).unwrap().window as f64 * 0.010;
        let sc = find_clap(&Envelope::of(&sound), 4.0).unwrap().window as f64 * 0.010;
        assert!(((sc + secs) - pc).abs() < 0.03);
    }

    #[test]
    fn with_no_slate_it_correlates_the_heads_and_says_so() {
        // Both machines rolling on the same room from the same moment, nobody slated.
        let sound = take(0.0, 5.0, 5, None, 0.0);
        let picture = take(0.0, 5.0, 77, None, 0.0);
        let al = align(&picture, &sound, 3.0).expect("an alignment");
        assert_eq!(al.method, Method::Correlation, "no clap to find");
        let secs = al.offset.0 as f64 / studio_core::TICKS_PER_SECOND as f64;
        assert!(secs.abs() < 0.03, "identical heads align at zero, got {secs}");
    }

    /// A periodic signal correlates with itself at many lags. The peak is then noise, and a
    /// caller that cuts on it is cutting on a coin flip — so the confidence must say so.
    #[test]
    fn a_periodic_signal_reports_low_confidence_rather_than_a_confident_guess() {
        let n = (SR as f64 * 4.0) as usize;
        let tone = |phase: f64| SourceAudio {
            sample_rate: SR,
            channels: vec![
                (0..n)
                    .map(|i| {
                        let t = i as f64 / SR as f64;
                        // a 2 Hz amplitude pulse: the envelope repeats every half second
                        (0.5 * (1.0 + ((t + phase) * std::f64::consts::TAU * 2.0).sin())) as f32
                    })
                    .collect(),
            ],
        };
        let al = align(&tone(0.0), &tone(0.5), 3.0).expect("an alignment");
        assert!(!al.trustworthy(), "a repeating envelope must not read as certain: {}", al.confidence);
        // and a clap-slated pair IS trustworthy
        let al2 = align(&take(0.0, 5.0, 4, Some(0.9), 0.8), &take(-0.6, 5.0, 44, Some(0.9), 0.8), 4.0).unwrap();
        assert!(al2.trustworthy(), "{}", al2.confidence);
    }

    #[test]
    fn empty_or_silent_input_returns_nothing_rather_than_zero() {
        let empty = SourceAudio { sample_rate: SR, channels: vec![vec![]] };
        let a = take(0.0, 3.0, 8, Some(1.0), 0.9);
        assert!(align(&a, &empty, 2.0).is_none());
        assert!(align(&empty, &a, 2.0).is_none());
        let silence = SourceAudio { sample_rate: SR, channels: vec![vec![0.0; SR as usize]] };
        // digital silence correlates with nothing; an offset of 0 here would be a lie
        assert!(align(&silence, &silence, 0.5).is_none());
        assert!(find_clap(&Envelope::of(&silence), 1.0).is_none());
    }
}
