//! The voice fast path: an utterance's acoustic fingerprint against the
//! commands the user has already said, so the common cases never wait on
//! a recogniser.
//!
//! This is voiceterm-qt's `acoustic_index` idea with switcheroo's needs:
//! the fingerprint and the distance are `voiceprint-kit` (lifted verbatim
//! from voiceterm, same coefficients, same numbers), the exemplar store is
//! small enough to be a `Vec`, and a hit resolves to an [`Intent`] through
//! [`crate::intent::parse`] rather than to a shell command.
//!
//! # How it is used
//!
//! 1. The shell captures an utterance, downmixes to 16 kHz mono
//!    (`voiceprint_kit::to_16k_mono`), and fingerprints it.
//! 2. [`FastPath::lookup`] returns the nearest enrolled exemplar if it is
//!    within [`FastPath::threshold`] — the intent fires immediately.
//! 3. Otherwise the shell runs whatever recogniser it has (whisper via
//!    `voice-kit`, or a typed fallback), gets a transcript, and
//!    [`FastPath::enrol`]s the fingerprint under it if it parsed to a
//!    known intent. Next time, step 2 hits.
//!
//! Thresholds, from voiceterm's measurements on the same 39-dim
//! fingerprint: same utterance, same speaker ≈ 0.05; same words, same
//! speaker ≈ 0.10; different speaker ≈ 0.30+. The default 0.18 sits under
//! the cross-speaker floor so a housemate's "lights on" does not fire from
//! the operator's enrolment — that is a feature, not a limitation, and the
//! threshold is a field because it is exactly the knob to turn.

use crate::intent::{self, Intent};

#[derive(Debug, Clone, PartialEq)]
pub struct Exemplar {
    pub fingerprint: Vec<f32>,
    pub transcript: String,
    /// Times this exemplar has been the hit. Ties break toward the more
    /// used one.
    pub hits: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub transcript: String,
    pub distance: f32,
    pub intent: Intent,
}

#[derive(Debug, Clone)]
pub struct FastPath {
    pub exemplars: Vec<Exemplar>,
    /// Cosine distance at or below which an exemplar counts as a match.
    pub threshold: f32,
    /// Keep at most this many; the least-hit go first.
    pub cap: usize,
}

impl Default for FastPath {
    fn default() -> Self { FastPath { exemplars: Vec::new(), threshold: 0.18, cap: 200 } }
}

impl FastPath {
    pub fn new() -> Self { Self::default() }

    /// Fingerprint audio the way every hit and enrolment does. Empty for
    /// audio too short to MFCC.
    pub fn fingerprint(audio_16k_mono: &[f32]) -> Vec<f32> {
        voiceprint_kit::fingerprint(audio_16k_mono)
    }

    /// Nearest exemplar within the threshold, with its intent re-parsed
    /// from the transcript (so a grammar improvement applies to old
    /// enrolments too). Bumps the hit count.
    pub fn lookup(&mut self, fingerprint: &[f32]) -> Option<Hit> {
        if fingerprint.is_empty() { return None; }
        let mut best: Option<(usize, f32)> = None;
        for (i, e) in self.exemplars.iter().enumerate() {
            let d = voiceprint_kit::distance(fingerprint, &e.fingerprint);
            if !d.is_finite() || d > self.threshold { continue; }
            match best {
                Some((bi, bd)) if d > bd || (d == bd && self.exemplars[bi].hits >= e.hits) => {}
                _ => best = Some((i, d)),
            }
        }
        let (i, distance) = best?;
        self.exemplars[i].hits += 1;
        let transcript = self.exemplars[i].transcript.clone();
        let intent = intent::parse(&transcript);
        Some(Hit { transcript, distance, intent })
    }

    /// Remember a fingerprint under a transcript. Only transcripts that
    /// parse to a real intent are worth remembering: an `Unknown` would
    /// just make the same failure faster.
    pub fn enrol(&mut self, fingerprint: Vec<f32>, transcript: &str) -> bool {
        if fingerprint.is_empty() || matches!(intent::parse(transcript), Intent::Unknown(_)) {
            return false;
        }
        if self.exemplars.len() >= self.cap {
            if let Some((i, _)) = self.exemplars.iter().enumerate().min_by_key(|(_, e)| e.hits) {
                self.exemplars.remove(i);
            }
        }
        self.exemplars.push(Exemplar { fingerprint, transcript: transcript.to_string(), hits: 1 });
        true
    }

    /// Line format: `hits<TAB>transcript<TAB>f0,f1,...`. One per line.
    pub fn to_lines(&self) -> String {
        self.exemplars.iter().map(|e| {
            let fp: Vec<String> = e.fingerprint.iter().map(|f| format!("{f}")).collect();
            format!("{}\t{}\t{}", e.hits, e.transcript.replace('\t', " "), fp.join(","))
        }).collect::<Vec<_>>().join("\n")
    }

    pub fn from_lines(text: &str) -> Self {
        let mut fp = FastPath::new();
        for line in text.lines() {
            let mut it = line.splitn(3, '\t');
            let (Some(h), Some(t), Some(f)) = (it.next(), it.next(), it.next()) else { continue };
            let Ok(hits) = h.parse() else { continue };
            let fingerprint: Vec<f32> = f.split(',').filter_map(|x| x.parse().ok()).collect();
            if fingerprint.is_empty() { continue; }
            fp.exemplars.push(Exemplar { fingerprint, transcript: t.to_string(), hits });
        }
        fp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{MediaCommand, Target};

    /// A synthetic "utterance": a tone burst with a per-utterance timbre,
    /// long enough to MFCC. Real speech is not this, but the pipeline is
    /// the real one and the distances behave the same way: identical audio
    /// is ~0, a different burst is far.
    fn burst(seed: u32, secs: f32) -> Vec<f32> {
        let n = (voiceprint_kit::RATE as f32 * secs) as usize;
        let f0 = 120.0 + (seed % 7) as f32 * 40.0;
        let f1 = 900.0 + (seed % 5) as f32 * 300.0;
        (0..n).map(|i| {
            let t = i as f32 / voiceprint_kit::RATE as f32;
            let env = (t * 3.0).min(1.0) * (1.0 - t / secs).max(0.0);
            env * (0.6 * (2.0 * std::f32::consts::PI * f0 * t).sin() + 0.4 * (2.0 * std::f32::consts::PI * f1 * t * (1.0 + 0.3 * (seed as f32 * 0.7).sin())).sin())
        }).collect()
    }

    fn noise(secs: f32) -> Vec<f32> {
        let n = (voiceprint_kit::RATE as f32 * secs) as usize;
        let mut x: u32 = 0x9E37_79B9;
        (0..n).map(|_| {
            x ^= x << 13; x ^= x >> 17; x ^= x << 5;
            (x as f32 / u32::MAX as f32) * 0.8 - 0.4
        }).collect()
    }

    #[test]
    fn an_enrolled_utterance_hits_and_a_different_one_misses() {
        let mut fp = FastPath::new();
        let lights_on = FastPath::fingerprint(&burst(1, 0.8));
        assert!(!lights_on.is_empty());
        assert!(fp.enrol(lights_on.clone(), "lights on"));
        let hit = fp.lookup(&lights_on).expect("identical audio hits");
        assert!(hit.distance < 0.01, "{}", hit.distance);
        assert_eq!(hit.intent, Intent::Power { target: Target::Lights, on: true });
        assert_eq!(fp.exemplars[0].hits, 2);
        // A spectrally different utterance: noise-like rather than tonal.
        // (Two tone bursts with different pitches sit ~0.08 apart on this
        // fingerprint — closer than two different words from one speaker —
        // so they are not a fair "different utterance".)
        let other = FastPath::fingerprint(&noise(0.8));
        let d = voiceprint_kit::distance(&other, &lights_on);
        assert!(d > fp.threshold, "a different utterance is {d}, above the threshold");
        assert!(fp.lookup(&other).is_none());
    }

    #[test]
    fn unknown_transcripts_are_not_enrolled_and_short_audio_is_ignored() {
        let mut fp = FastPath::new();
        assert!(!fp.enrol(vec![0.1; 39], "what is the weather"));
        assert!(!fp.enrol(Vec::new(), "lights on"));
        assert!(fp.lookup(&[]).is_none());
        assert!(fp.exemplars.is_empty());
        assert!(FastPath::fingerprint(&[0.0; 100]).is_empty(), "100 samples cannot MFCC");
    }

    #[test]
    fn hits_re_parse_so_the_grammar_can_improve_later() {
        let mut fp = FastPath::new();
        let f = FastPath::fingerprint(&burst(2, 0.8));
        fp.enrol(f.clone(), "next song");
        assert_eq!(fp.lookup(&f).unwrap().intent, Intent::Media(MediaCommand::Next));
    }

    #[test]
    fn the_cap_evicts_the_least_used_and_the_store_round_trips() {
        let mut fp = FastPath { cap: 2, ..Default::default() };
        let a = FastPath::fingerprint(&burst(1, 0.6));
        let b = FastPath::fingerprint(&burst(2, 0.6));
        let c = FastPath::fingerprint(&burst(3, 0.6));
        fp.enrol(a.clone(), "lights on");
        fp.enrol(b.clone(), "lights off");
        fp.lookup(&a); // a: 2 hits, b: 1
        fp.enrol(c.clone(), "play");
        assert_eq!(fp.exemplars.len(), 2);
        assert!(fp.exemplars.iter().all(|e| e.transcript != "lights off"), "b was least used");
        let text = fp.to_lines();
        let back = FastPath::from_lines(&text);
        assert_eq!(back.exemplars.len(), 2);
        assert_eq!(back.exemplars[0].transcript, "lights on");
        assert_eq!(back.exemplars[0].hits, 2);
        assert_eq!(back.exemplars[0].fingerprint.len(), 39);
    }
}
