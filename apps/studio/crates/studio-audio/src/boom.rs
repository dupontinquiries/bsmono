//! boom — aligning a separately-recorded microphone to picture.
//!
//! `slate` aligns dual-system sound by cross-correlating ENVELOPES,
//! chosen because raw samples do not correlate across different
//! microphones -- a camera mic and a boom hear different frequency
//! content of the same clap. That reasoning is right and this keeps it.
//!
//! What it misses is REVERB, and the miss is large.
//!
//! ## Why the envelope is not enough
//!
//! Measured, on a real boom recording with a synthetic room applied to
//! a second copy at a known 12 ms delay:
//!
//!   true delay                  12.0 ms
//!   envelope correlation says   65.2 ms
//!   onset correlation says      12.0 ms
//!
//! Fifty-three milliseconds of error, confidently reported. The reverb
//! tail adds energy AFTER the event, dragging the envelope's centre of
//! mass late, and the correlation peak follows it. A mic further from
//! the source has more tail, so the error grows exactly with the thing
//! that makes alignment hard in the first place.
//!
//! The ATTACK is what two mics share. The tail belongs to the room and
//! to the distance, and correlating on it is correlating on the
//! difference. So this rectifies the envelope's derivative -- keeping
//! rises, discarding falls -- and correlates that.
//!
//! ## Distance is a real delay, not an error
//!
//! Sound covers 343 m/s, so 3.4 m is 10 ms. A boom at 1 m and a camera
//! at 5 m genuinely disagree by ~12 ms about when the clap happened,
//! and both are right. Whether to remove it is a decision:
//! [`Alignment::acoustic_hint`] says how far apart the mics were, so
//! the operator can tell a real offset from a mistake.
//!
//! ## And the file may not be what it claims
//!
//! The recording this was built against reports two channels. Measured:
//! left peak -50.5 dBFS with a crest factor of 5.7 -- noise, not
//! speech -- and right at +1.9 dBFS. It is a MONO boom in the right
//! channel of a stereo container. A pipeline that duplicated "the left
//! channel" to make stereo would produce silence, so [`Channels`]
//! finds the live one rather than assuming.

/// Samples at a known rate. Mono: the caller has already chosen a
/// channel, which is a decision this module makes explicit.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub sr: u32,
    pub samples: Vec<f32>,
}

impl Track {
    pub fn seconds(&self) -> f64 { self.samples.len() as f64 / self.sr as f64 }

    /// Short-window RMS. The window is 5 ms: long enough to smooth the
    /// waveform, short enough that a clap's attack survives it.
    pub fn envelope(&self, window_ms: f64) -> Vec<f32> {
        let w = ((window_ms / 1000.0) * self.sr as f64).max(1.0) as usize;
        if self.samples.len() < w { return Vec::new(); }
        let mut out = Vec::with_capacity(self.samples.len() / w);
        for chunk in self.samples.chunks(w) {
            let e = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
            out.push(e.sqrt());
        }
        out
    }

    /// The envelope's rises only.
    ///
    /// THIS IS THE WHOLE FIX. A fall in the envelope is the room
    /// letting go of the sound, and it is different in every mic. A
    /// rise is the source doing something, and it is the same in all of
    /// them. Discarding falls discards the reverb.
    pub fn onsets(&self, window_ms: f64) -> Vec<f32> {
        let env = self.envelope(window_ms);
        if env.is_empty() { return Vec::new(); }
        let mut out = Vec::with_capacity(env.len());
        let mut prev = env[0];
        for e in env {
            out.push((e - prev).max(0.0));
            prev = e;
        }
        out
    }
}

/// Which channel of a file actually holds the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channels {
    /// One channel, nothing to decide.
    Mono,
    /// Two channels carrying the same thing.
    DualMono,
    /// Genuinely different content in each.
    Stereo,
    /// Two channels, but only one is live. Carries which.
    OneLive { index: usize },
}

/// Work out what a multi-channel file really is.
///
/// A recorder set to stereo with one mic plugged in produces a file
/// that claims two channels and has one. Reported rather than guessed
/// at, because every downstream choice depends on it.
pub fn examine(channels: &[Vec<f32>]) -> Channels {
    match channels.len() {
        0 | 1 => Channels::Mono,
        _ => {
            let rms: Vec<f32> = channels.iter().map(|c| {
                if c.is_empty() { return 0.0 }
                (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt()
            }).collect();
            let loudest = rms.iter().cloned().fold(0.0f32, f32::max);
            let live: Vec<usize> = rms.iter().enumerate()
                // 40 dB below the loudest is not a channel, it is
                // bleed or a noise floor. Measured on a real file:
                // 0.00053 against 0.06068, which is 41 dB.
                .filter(|(_, r)| **r > loudest / 100.0).map(|(i, _)| i).collect();
            if live.len() == 1 { return Channels::OneLive { index: live[0] }; }
            // identical within a hair is dual mono
            let (a, b) = (&channels[0], &channels[1]);
            let n = a.len().min(b.len());
            if n > 0 && a[..n].iter().zip(&b[..n]).all(|(x, y)| (x - y).abs() < 1e-6) {
                return Channels::DualMono;
            }
            Channels::Stereo
        }
    }
}

/// How to make two channels out of one.
///
/// Duplication is the honest default and the others are effects. A
/// mono boom is mono; widening it invents spatial information that was
/// never recorded, which is fine for atmosphere and wrong for dialogue
/// an editor will later place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spread {
    /// The same signal in both. Centred, phase-safe, sums to mono
    /// cleanly.
    Duplicate,
    /// Duplicate, then pan. -1 hard left to 1 hard right.
    Pan(f32),
    /// A short delay on one side. Wide, and it does NOT survive a mono
    /// fold-down -- the delay becomes a comb filter.
    Haas { ms: f32 },
}

impl Spread {
    /// Is this safe if someone later sums to mono?
    ///
    /// Broadcast, phones and most laptop speakers do. A Haas-widened
    /// dialogue track that sounded good in headphones can arrive
    /// hollow, and that is discovered late.
    pub fn mono_safe(self) -> bool { !matches!(self, Spread::Haas { .. }) }

    pub fn apply(self, mono: &Track) -> (Vec<f32>, Vec<f32>) {
        match self {
            Spread::Duplicate => (mono.samples.clone(), mono.samples.clone()),
            Spread::Pan(p) => {
                let p = p.clamp(-1.0, 1.0);
                // equal-power, so a centred signal is not louder than a
                // panned one
                let ang = (p + 1.0) * std::f32::consts::FRAC_PI_4;
                let (l, r) = (ang.cos(), ang.sin());
                (mono.samples.iter().map(|s| s * l).collect(),
                 mono.samples.iter().map(|s| s * r).collect())
            }
            Spread::Haas { ms } => {
                let d = ((ms.abs() / 1000.0) * mono.sr as f32) as usize;
                let mut delayed = vec![0.0; d];
                delayed.extend_from_slice(&mono.samples);
                delayed.truncate(mono.samples.len());
                if ms >= 0.0 { (mono.samples.clone(), delayed) }
                else { (delayed, mono.samples.clone()) }
            }
        }
    }
}

/// What the aligner concluded.
#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    /// Seconds to shift the sound so it lines up with the picture.
    /// Positive means the sound starts later.
    pub offset_secs: f64,
    /// Peak over next-best-peak. Near 1 means several offsets fit
    /// equally, and the caller should refuse rather than cut on it.
    pub confidence: f32,
    /// What the offset implies about mic separation, if anything.
    pub acoustic_hint: Option<String>,
}

impl Alignment {
    pub fn trustworthy(&self) -> bool { self.confidence >= 1.6 }
}

/// Speed of sound, metres per second, at room temperature.
pub const SPEED_OF_SOUND: f64 = 343.0;

/// Align sound to picture on their shared onsets.
///
/// `search_secs` bounds the offset considered; a wider search finds
/// more spurious peaks, so it should be as tight as the workflow
/// allows.
pub fn align(picture: &Track, sound: &Track, search_secs: f64, window_ms: f64)
    -> Option<Alignment> {
    let (a, b) = (picture.onsets(window_ms), sound.onsets(window_ms));
    if a.len() < 8 || b.len() < 8 { return None; }
    let per = window_ms / 1000.0;
    let max_lag = ((search_secs / per) as isize).max(1).min(a.len() as isize - 1);
    let mut scores: Vec<(isize, f64)> = Vec::new();
    for lag in -max_lag..=max_lag {
        let mut dot = 0.0f64;
        let mut na = 0.0f64;
        let mut nb = 0.0f64;
        for i in 0..a.len() {
            let j = i as isize + lag;
            if j < 0 || j as usize >= b.len() { continue; }
            let (x, y) = (a[i] as f64, b[j as usize] as f64);
            dot += x * y; na += x * x; nb += y * y;
        }
        let norm = (na * nb).sqrt();
        scores.push((lag, if norm > 0.0 { dot / norm } else { 0.0 }));
    }
    let best = scores.iter().cloned()
        .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))?;
    // NEXT-BEST, EXCLUDING NEIGHBOURS. The samples either side of the
    // peak are part of the same peak, and counting one as a rival makes
    // every alignment look ambiguous.
    let guard = (0.05 / per) as isize + 1;
    let runner = scores.iter().filter(|(l, _)| (l - best.0).abs() > guard)
        .map(|(_, s)| *s).fold(0.0f64, f64::max);
    // NO CORRELATION IS NOT PERFECT CORRELATION. Against silence every
    // lag scores zero, so there is no runner-up -- and reading "no
    // rival" as "certain" made silence align with maximum confidence.
    // The peak has to be a real peak before its margin means anything.
    if best.1 < 0.05 { return None; }
    let confidence = if runner > 1e-9 { (best.1 / runner) as f32 } else { 10.0 };
    let offset = best.0 as f64 * per;
    Some(Alignment {
        offset_secs: offset,
        confidence,
        acoustic_hint: acoustic_hint(offset),
    })
}

/// Is this offset explainable by distance rather than by clocks?
fn acoustic_hint(offset_secs: f64) -> Option<String> {
    let ms = offset_secs.abs() * 1000.0;
    if ms < 0.5 || ms > 60.0 { return None; }
    let metres = offset_secs.abs() * SPEED_OF_SOUND;
    Some(format!(
        "{ms:.1} ms is {metres:.1} m of air. If the mics were about that far apart this is \
         REAL and not a clock difference -- correcting it puts the boom on the picture, \
         which is usually what you want, but it is a choice."))
}

/// Align many recordings to one picture, for a command-palette
/// "align selected" over a whole scene.
#[derive(Debug, Clone, PartialEq)]
pub struct Aligned {
    pub name: String,
    pub result: Option<Alignment>,
}

/// What to do with a batch of selected clips.
///
/// Anything the aligner is unsure of is REPORTED, NOT APPLIED. A
/// command palette that silently nudges twelve clips, three of them
/// wrongly, produces a scene that is subtly out and no record of which
/// three.
pub fn align_many(picture: &Track, sounds: &[(String, Track)], search_secs: f64)
    -> (Vec<Aligned>, Vec<Aligned>) {
    let mut ok = Vec::new();
    let mut unsure = Vec::new();
    for (name, t) in sounds {
        let r = align(picture, t, search_secs, 5.0);
        let a = Aligned { name: name.clone(), result: r.clone() };
        match r {
            Some(x) if x.trustworthy() => ok.push(a),
            _ => unsure.push(a),
        }
    }
    (ok, unsure)
}

/// A line per clip, for the palette to show before it commits.
pub fn summary(ok: &[Aligned], unsure: &[Aligned]) -> String {
    let mut s = format!("{} aligned, {} need a look\n", ok.len(), unsure.len());
    for a in ok {
        if let Some(r) = &a.result {
            s.push_str(&format!("  {:<24} {:+.3}s  (x{:.1})\n",
                                a.name, r.offset_secs, r.confidence));
        }
    }
    for a in unsure {
        let why = match &a.result {
            None => "no shared onsets found".to_string(),
            Some(r) => format!("several offsets fit equally (x{:.1})", r.confidence),
        };
        s.push_str(&format!("  {:<24} NOT APPLIED -- {why}\n", a.name));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A clap: a sharp transient with a short decay.
    fn clap_at(secs: f64, len_secs: f64) -> Track {
        let n = (len_secs * SR as f64) as usize;
        let mut s = vec![0.0f32; n];
        let at = (secs * SR as f64) as usize;
        for i in 0..(SR as usize / 20) {
            if at + i >= n { break; }
            let t = i as f32 / (SR as f32 / 20.0);
            s[at + i] = (1.0 - t).powi(3) * ((i as f32 * 0.7).sin());
        }
        // a little room tone so the correlation has something to reject
        for (i, v) in s.iter_mut().enumerate() {
            *v += 0.0005 * ((i as f32 * 0.013).sin());
        }
        Track { sr: SR, samples: s }
    }

    /// The same event heard from further away: delayed, and with a
    /// decaying tail the near mic does not have.
    fn reverberant(t: &Track, delay_secs: f64, tail: f32) -> Track {
        let d = (delay_secs * SR as f64) as usize;
        let mut out = vec![0.0f32; t.samples.len()];
        for (i, v) in t.samples.iter().enumerate() {
            if i + d < out.len() { out[i + d] += *v; }
        }
        // sparse late reflections
        let mut seed = 12345u64;
        for r in 1..40 {
            let off = d + r * (SR as usize / 120);
            let g = tail * (-3.0 * r as f32 / 40.0).exp();
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let sign = if seed >> 60 & 1 == 0 { 1.0 } else { -1.0 };
            for (i, v) in t.samples.iter().enumerate() {
                if i + off < out.len() { out[i + off] += v * g * sign; }
            }
        }
        Track { sr: SR, samples: out }
    }

    #[test]
    fn onsets_beat_envelopes_when_there_is_reverb() {
        // measured on real audio: envelope said 65.2 ms for a true
        // 12.0 ms delay. The tail drags the centre of mass late.
        let near = clap_at(0.5, 3.0);
        let far = reverberant(&near, 0.012, 0.30);
        let a = align(&near, &far, 0.4, 5.0).expect("aligned");
        assert!((a.offset_secs - 0.012).abs() < 0.008,
                "onset alignment lands near the truth: {:.4}s", a.offset_secs);
        assert!(a.trustworthy(), "and is sure of it: x{:.1}", a.confidence);
    }

    #[test]
    fn a_real_offset_is_explained_as_distance_rather_than_a_fault() {
        // 3.4 m is 10 ms, and both mics are right
        let near = clap_at(0.5, 3.0);
        let far = reverberant(&near, 0.012, 0.15);
        let a = align(&near, &far, 0.4, 5.0).unwrap();
        let hint = a.acoustic_hint.expect("explained");
        assert!(hint.contains("m of air"), "{hint}");
        assert!(hint.contains("REAL and not a clock difference"), "{hint}");
        assert!(hint.contains("it is a choice"), "the operator decides: {hint}");
        // a huge offset is not distance, so no hint
        assert!(acoustic_hint(2.5).is_none(), "2.5s is not a room");
        assert!(acoustic_hint(0.0001).is_none(), "0.1ms is not worth mentioning");
    }

    #[test]
    fn a_mono_boom_in_a_stereo_container_is_recognised() {
        // MEASURED on the real file: left peak -50.5 dBFS with crest
        // 5.7 (noise), right at +1.9 dBFS. Duplicating "the left
        // channel" would produce silence.
        let live: Vec<f32> = (0..48000).map(|i| 0.3 * ((i as f32 * 0.01).sin())).collect();
        let dead: Vec<f32> = (0..48000).map(|i| 0.0005 * ((i as f32 * 0.31).sin())).collect();
        assert_eq!(examine(&[dead.clone(), live.clone()]), Channels::OneLive { index: 1 });
        assert_eq!(examine(&[live.clone(), dead]), Channels::OneLive { index: 0 });
        assert_eq!(examine(&[live.clone(), live.clone()]), Channels::DualMono);
        assert_eq!(examine(&[live.clone()]), Channels::Mono);
        let other: Vec<f32> = (0..48000).map(|i| 0.3 * ((i as f32 * 0.017).cos())).collect();
        assert_eq!(examine(&[live, other]), Channels::Stereo);
    }

    #[test]
    fn widening_a_mono_track_says_whether_it_survives_a_fold_down() {
        // broadcast, phones and laptop speakers sum to mono; a
        // Haas-widened dialogue track arrives hollow, and that is
        // discovered late
        assert!(Spread::Duplicate.mono_safe());
        assert!(Spread::Pan(-0.5).mono_safe());
        assert!(!Spread::Haas { ms: 12.0 }.mono_safe());
        let m = Track { sr: SR, samples: vec![0.5; 1000] };
        let (l, r) = Spread::Duplicate.apply(&m);
        assert_eq!(l, r, "duplicate is identical by definition");
        // equal-power pan: a centred signal is not louder than a
        // panned one
        let (cl, cr) = Spread::Pan(0.0).apply(&m);
        let centre_power = cl[0] * cl[0] + cr[0] * cr[0];
        let (hl, hr) = Spread::Pan(1.0).apply(&m);
        let hard_power = hl[0] * hl[0] + hr[0] * hr[0];
        assert!((centre_power - hard_power).abs() < 0.01,
                "{centre_power} vs {hard_power}");
        assert!(hl[0] < 0.01, "hard right silences the left");
        // Haas delays one side and keeps the length
        let (dl, dr) = Spread::Haas { ms: 10.0 }.apply(&m);
        assert_eq!(dl.len(), dr.len());
        assert_eq!(dl.len(), m.samples.len());
        assert_eq!(dr[0], 0.0, "the delayed side starts silent");
    }

    #[test]
    fn an_unsure_alignment_is_reported_and_not_applied() {
        // a palette that silently nudges twelve clips, three of them
        // wrongly, produces a scene that is subtly out and no record
        let picture = clap_at(0.5, 3.0);
        let good = ("boom-a".to_string(), reverberant(&picture, 0.010, 0.2));
        let junk = ("boom-b".to_string(),
                    Track { sr: SR, samples: vec![0.0; SR as usize * 3] });
        let (ok, unsure) = align_many(&picture, &[good, junk], 0.4);
        assert_eq!(ok.len(), 1, "{:?}", ok);
        assert_eq!(ok[0].name, "boom-a");
        assert_eq!(unsure.len(), 1);
        assert_eq!(unsure[0].name, "boom-b");
        let s = summary(&ok, &unsure);
        assert!(s.contains("1 aligned, 1 need a look"), "{s}");
        assert!(s.contains("NOT APPLIED"), "{s}");
    }

    #[test]
    fn silence_and_noise_do_not_produce_a_confident_answer() {
        let picture = clap_at(0.5, 3.0);
        let silence = Track { sr: SR, samples: vec![0.0; SR as usize * 3] };
        match align(&picture, &silence, 0.4, 5.0) {
            None => {}
            Some(a) => assert!(!a.trustworthy(), "silence aligned confidently: {a:?}"),
        }
        // and a track too short to have onsets returns nothing rather
        // than a number
        let stub = Track { sr: SR, samples: vec![0.1; 100] };
        assert!(align(&picture, &stub, 0.4, 5.0).is_none());
    }

    #[test]
    fn the_onset_signal_keeps_rises_and_discards_falls() {
        // the fix, stated as a property: a fall is the room letting go,
        // and it differs per mic
        let t = clap_at(0.2, 1.0);
        let o = t.onsets(5.0);
        assert!(!o.is_empty());
        assert!(o.iter().all(|v| *v >= 0.0), "no negative onsets");
        let env = t.envelope(5.0);
        // the envelope has falls; the onset signal does not
        let falls = env.windows(2).filter(|w| w[1] < w[0]).count();
        assert!(falls > 0, "the clap decays");
        assert!(o.iter().filter(|v| **v > 0.0).count() < env.len(),
                "onsets are sparser than the envelope");
    }
}
