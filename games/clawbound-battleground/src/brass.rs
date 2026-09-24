//! brass — the intro's fanfare, synthesized.
//!
//! The game ships no samples; its ambience is synthesized (`audio`), so the
//! fanfare is too. Brass, as physics gives it: a buzzy sawtooth (the lips),
//! low-passed by a filter whose cutoff rises with loudness -- a horn gets
//! brighter the harder it is blown, which is the "blat" of a brass attack.
//! Each note is a small section (three detuned voices), with a slow vibrato
//! arriving late in long notes, over timpani.
//!
//! Headless: [`Fanfare::render`] fills a buffer from a start time, so the
//! audio thread and a WAV writer (`examples`/tests) share one code path,
//! and the tests can check the music is the music (pitch, loudness, length)
//! without a speaker.

/// One note of the score: when, how long, which pitches (MIDI), how loud.
#[derive(Debug, Clone, Copy)]
pub struct Note {
    pub at: f32,
    pub dur: f32,
    pub midi: [u8; 3],
    pub vel: f32,
}

/// A timpani stroke.
#[derive(Debug, Clone, Copy)]
pub struct Drum {
    pub at: f32,
    pub midi: u8,
    pub vel: f32,
}

/// The call: a pickup triplet, a rising fourth, a held major chord, an
/// answer that steps up to the dominant and resolves on a long tonic chord.
/// B-flat major -- the key brass is most at home in. ~6.2 s.
pub const SCORE: &[Note] = &[
    Note { at: 0.00, dur: 0.14, midi: [58, 53, 46], vel: 0.55 }, // Bb3 triplet pickup
    Note { at: 0.16, dur: 0.14, midi: [58, 53, 46], vel: 0.60 },
    Note { at: 0.32, dur: 0.14, midi: [58, 53, 46], vel: 0.65 },
    Note { at: 0.48, dur: 0.70, midi: [63, 58, 51], vel: 0.90 }, // Eb4: the fourth
    Note { at: 1.22, dur: 0.30, midi: [62, 58, 53], vel: 0.75 }, // D4
    Note { at: 1.56, dur: 1.30, midi: [70, 65, 58], vel: 1.00 }, // Bb4 held: the call
    Note { at: 3.00, dur: 0.14, midi: [65, 60, 53], vel: 0.55 }, // answer pickup F4
    Note { at: 3.16, dur: 0.14, midi: [65, 60, 53], vel: 0.60 },
    Note { at: 3.32, dur: 0.14, midi: [65, 60, 53], vel: 0.65 },
    Note { at: 3.48, dur: 0.55, midi: [72, 67, 60], vel: 0.95 }, // C5
    Note { at: 4.06, dur: 2.10, midi: [70, 65, 62], vel: 1.00 }, // Bb4 major: home
];

pub const DRUMS: &[Drum] = &[
    Drum { at: 0.48, midi: 39, vel: 0.9 },
    Drum { at: 1.56, midi: 34, vel: 1.0 },
    Drum { at: 2.30, midi: 34, vel: 0.5 },
    Drum { at: 3.48, midi: 41, vel: 0.8 },
    Drum { at: 4.06, midi: 34, vel: 1.0 },
    Drum { at: 4.40, midi: 34, vel: 0.45 },
    Drum { at: 4.62, midi: 34, vel: 0.35 },
];

pub fn length() -> f32 {
    SCORE.iter().map(|n| n.at + n.dur).fold(0.0, f32::max) + 0.9 // release tail
}

pub fn midi_hz(m: u8) -> f32 {
    440.0 * 2f32.powf((m as f32 - 69.0) / 12.0)
}

/// PolyBLEP-corrected sawtooth: a saw without the aliasing a naive one
/// sprays across a phone speaker's range.
fn saw(phase: f32, dphase: f32) -> f32 {
    let mut v = 2.0 * phase - 1.0;
    let t = phase;
    if t < dphase {
        let x = t / dphase;
        v -= x + x - x * x - 1.0;
    } else if t > 1.0 - dphase {
        let x = (t - 1.0) / dphase;
        v -= x * x + x + x + 1.0;
    }
    v
}

/// Brass envelope: a quick swell (the tongue), a small overshoot, sustain,
/// then a short release. Returns loudness 0..~1.1.
fn env(t: f32, dur: f32) -> f32 {
    if t < 0.0 {
        return 0.0;
    }
    let attack = 0.045;
    let a = if t < attack { t / attack } else { 1.0 + 0.12 * (-(t - attack) / 0.08).exp() };
    let release = 0.18;
    let r = if t > dur { (1.0 - (t - dur) / release).max(0.0) } else { 1.0 };
    a * r
}

/// The fanfare's state: oscillator phases and filter memory per voice.
pub struct Fanfare {
    sr: f32,
    phase: Vec<f32>,
    lp: Vec<[f32; 2]>,
    rng: u32,
    /// Seconds rendered so far.
    pub t: f32,
}

const DETUNE: [f32; 3] = [0.0, 0.0035, -0.0042];

impl Fanfare {
    pub fn new(sample_rate: f32) -> Fanfare {
        let v = SCORE.len() * 3 * 3;
        Fanfare { sr: sample_rate.max(8000.0), phase: vec![0.0; v], lp: vec![[0.0; 2]; v], rng: 0x9E37_79B9, t: 0.0 }
    }

    pub fn finished(&self) -> bool {
        self.t >= length()
    }

    fn noise(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Render mono samples into `out` (adds nothing past the end; zeros).
    pub fn render(&mut self, out: &mut [f32]) {
        let dt = 1.0 / self.sr;
        for o in out.iter_mut() {
            let t = self.t;
            let mut s = 0.0f32;
            for (ni, n) in SCORE.iter().enumerate() {
                let nt = t - n.at;
                if nt < 0.0 || nt > n.dur + 0.2 {
                    continue;
                }
                let e = env(nt, n.dur) * n.vel;
                // vibrato arrives in long notes, never in short ones
                let vib = 1.0 + 0.004 * (nt * 5.5 * std::f32::consts::TAU).sin() * ((nt - 0.35) / 0.4).clamp(0.0, 1.0);
                for (pi, &m) in n.midi.iter().enumerate() {
                    for (di, d) in DETUNE.iter().enumerate() {
                        let k = (ni * 3 + pi) * 3 + di;
                        let f = midi_hz(m) * (1.0 + d) * vib;
                        let dp = (f * dt).min(0.45);
                        self.phase[k] = (self.phase[k] + dp).fract();
                        let raw = saw(self.phase[k], dp);
                        // brightness follows loudness: cutoff 2.5x .. 9x the pitch
                        let fc = (f * (2.5 + 6.5 * e.min(1.0))).min(self.sr * 0.42);
                        let g = 1.0 - (-std::f32::consts::TAU * fc * dt).exp();
                        let lp = &mut self.lp[k];
                        lp[0] += (raw - lp[0]) * g;
                        lp[1] += (lp[0] - lp[1]) * g;
                        // the lead voice louder than the harmony under it
                        let part = [1.0, 0.62, 0.5][pi];
                        s += lp[1] * e * part * 0.11;
                    }
                }
            }
            for d in DRUMS {
                let dt0 = t - d.at;
                if (0.0..1.2).contains(&dt0) {
                    // a timpani: a pitched thud falling slightly, and a little skin noise
                    let f = midi_hz(d.midi) * (1.0 + 0.3 * (-dt0 * 30.0).exp());
                    let body = (dt0 * f * std::f32::consts::TAU).sin() * (-dt0 * 3.2).exp();
                    let skin = self.noise() * (-dt0 * 40.0).exp() * 0.3;
                    s += (body + skin) * d.vel * 0.35;
                }
            }
            // gentle saturation: loud chords compress instead of clipping
            *o = (s * 1.2).tanh() * 0.8;
            self.t += dt;
        }
    }

    /// The whole fanfare as mono f32 samples.
    pub fn render_all(sample_rate: f32) -> Vec<f32> {
        let mut f = Fanfare::new(sample_rate);
        let mut v = vec![0.0; (length() * f.sr).ceil() as usize];
        f.render(&mut v);
        v
    }
}

/// 16-bit mono PCM WAV bytes, for listening to it without the game.
pub fn wav_bytes(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&sample_rate.to_le_bytes());
    b.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 22050.0;

    fn window(v: &[f32], from: f32, to: f32) -> &[f32] {
        &v[(from * SR) as usize..((to * SR) as usize).min(v.len())]
    }
    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }
    /// Dominant pitch in a window by autocorrelation, Hz.
    fn pitch(v: &[f32], lo: f32, hi: f32) -> f32 {
        let (min_lag, max_lag) = ((SR / hi) as usize, (SR / lo) as usize);
        let best = (min_lag..=max_lag).max_by(|&a, &b| {
            let ca: f32 = v.iter().zip(&v[a..]).map(|(x, y)| x * y).sum();
            let cb: f32 = v.iter().zip(&v[b..]).map(|(x, y)| x * y).sum();
            ca.partial_cmp(&cb).unwrap()
        });
        SR / best.unwrap() as f32
    }

    #[test]
    fn it_is_music_not_noise_and_never_clips() {
        let v = Fanfare::render_all(SR);
        assert!((v.len() as f32 / SR - length()).abs() < 0.01);
        assert!(v.iter().all(|x| x.is_finite() && x.abs() <= 0.8 + 1e-4), "clipped");
        // loud where the score is, quiet in the rest before the answer
        assert!(rms(window(&v, 1.7, 2.6)) > 0.08, "the held call is loud: {}", rms(window(&v, 1.7, 2.6)));
        assert!(rms(window(&v, 2.95, 2.99)) < rms(window(&v, 1.7, 2.6)) * 0.5, "the breath before the answer");
        assert!(rms(window(&v, length() - 0.1, length())) < 0.01, "it ends");
    }

    /// The held call sounds its written pitch: the chord's top is Bb4
    /// (466 Hz); the fundamental period of the chord is the root's.
    #[test]
    fn the_notes_are_the_notes() {
        let v = Fanfare::render_all(SR);
        // the pickup: a single pitch class, Bb3 on top
        let p = pitch(window(&v, 0.02, 0.12), 180.0, 300.0);
        assert!((p - midi_hz(58)).abs() / midi_hz(58) < 0.04, "pickup at {p} Hz, want {}", midi_hz(58));
        let q = pitch(window(&v, 2.0, 2.3), 300.0, 520.0);
        assert!((q - midi_hz(70)).abs() / midi_hz(70) < 0.04, "the call at {q} Hz, want {}", midi_hz(70));
    }

    /// Louder is brighter: the energy above ~2 kHz rises with loudness, which
    /// is what makes it sound blown rather than like an organ.
    #[test]
    fn louder_notes_are_brighter() {
        let v = Fanfare::render_all(SR);
        let hf = |w: &[f32]| {
            // first difference ~ a high-pass
            let d: Vec<f32> = w.windows(2).map(|p| p[1] - p[0]).collect();
            rms(&d) / rms(w).max(1e-6)
        };
        let soft = hf(window(&v, 0.02, 0.12)); // vel 0.55
        let loud = hf(window(&v, 1.7, 2.0)); // vel 1.0
        assert!(loud > soft * 1.2, "loud {loud} vs soft {soft}");
    }

    #[test]
    fn the_wav_is_a_wav() {
        let b = wav_bytes(&[0.0, 0.5, -0.5], 22050);
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..16], b"WAVEfmt ");
        assert_eq!(b.len(), 44 + 6);
        assert_eq!(i16::from_le_bytes([b[46], b[47]]), 16383);
    }
}
