//! Pitch ↔ note conversion and scale guessing.
//!
//! Pitch detection itself lives in `audio.rs` (YIN algorithm) — this module is
//! the music-theory layer that turns a raw frequency into a note name and a
//! key/scale guess. Designed to be lightweight: a decaying 12-bin histogram of
//! the pitch classes you've been hitting, scored against 60 candidate scales
//! (12 roots × 5 scale flavours).

/// A=440 reference frequency.
pub const A4_HZ: f32 = 440.0;

/// Note names indexed by pitch class (0..12, C=0).
pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Frequency → MIDI note number (e.g. A4 = 69.0). Fractional values express
/// how far off the nearest semitone we are.
pub fn freq_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / A4_HZ).log2()
}

/// Frequency → `(note_name, octave, cents_offset)`. Cents in [-50, +50).
pub fn freq_to_note(hz: f32) -> (String, i8, i8) {
    let midi = freq_to_midi(hz);
    let nearest = midi.round() as i32;
    let pitch_class = ((nearest % 12) + 12) % 12;
    let octave = (nearest / 12) - 1;
    let cents = ((midi - nearest as f32) * 100.0).round() as i8;
    (NOTE_NAMES[pitch_class as usize].to_string(), octave as i8, cents)
}

// --- scale patterns (rooted at C, rotate to other roots) --------------------
//
// Each pattern is a 12-bool mask: position i = semitone i above the root.

pub const MAJOR: [u8; 12] = [1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0, 1];
pub const NATURAL_MINOR: [u8; 12] = [1, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 0];
pub const PENT_MAJOR: [u8; 12] = [1, 0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0];
pub const PENT_MINOR: [u8; 12] = [1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 0];
pub const BLUES: [u8; 12] = [1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 1, 0];

pub const SCALES: &[(&str, &[u8; 12])] = &[
    ("major", &MAJOR),
    ("minor", &NATURAL_MINOR),
    ("pent maj", &PENT_MAJOR),
    ("pent min", &PENT_MINOR),
    ("blues", &BLUES),
];

// --- pitch histogram --------------------------------------------------------

/// Exponentially decaying histogram of pitch classes (0=C..11=B). Older
/// observations weigh less so the scale guess reflects what you've been
/// singing in the last few seconds, not the whole session.
#[derive(Debug, Clone, Default)]
pub struct PitchHistogram {
    pub counts: [f32; 12],
}

impl PitchHistogram {
    /// Decay factor per observation. Closer to 1 = longer memory.
    const DECAY: f32 = 0.97;

    pub fn new() -> Self {
        Self::default()
    }

    /// Record a detected pitch. Ignores out-of-range frequencies (sub-50Hz
    /// rumble, super-2kHz whistles) which are usually noise.
    pub fn observe(&mut self, hz: f32) {
        if !(50.0..=2000.0).contains(&hz) {
            return;
        }
        for c in self.counts.iter_mut() {
            *c *= Self::DECAY;
        }
        let midi = freq_to_midi(hz);
        let pitch_class = ((midi.round() as i32 % 12) + 12) % 12;
        self.counts[pitch_class as usize] += 1.0;
    }

    pub fn total(&self) -> f32 {
        self.counts.iter().sum()
    }

    pub fn clear(&mut self) {
        self.counts = [0.0; 12];
    }
}

// --- scale guessing ---------------------------------------------------------

/// The winning scale guess plus the runner-up, so the UI can show confidence
/// as the gap between #1 and #2.
#[derive(Debug, Clone)]
pub struct ScaleGuess {
    pub root_name: &'static str,
    pub scale_name: &'static str,
    pub score: f32,
    pub margin: f32, // score - second_best_score; 0..1
}

/// Pick the scale that best fits the current histogram. Returns `None` if the
/// histogram is too sparse to make a confident guess.
pub fn guess_scale(hist: &PitchHistogram) -> Option<ScaleGuess> {
    if hist.total() < 5.0 {
        return None;
    }

    let mut scored: Vec<(&'static str, &'static str, f32)> =
        Vec::with_capacity(12 * SCALES.len());
    for (scale_name, pattern) in SCALES {
        for root in 0..12u8 {
            let s = score_scale(hist, pattern, root);
            scored.push((NOTE_NAMES[root as usize], scale_name, s));
        }
    }
    scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let best = scored[0];
    let second = scored.get(1).map(|x| x.2).unwrap_or(0.0);
    Some(ScaleGuess {
        root_name: best.0,
        scale_name: best.1,
        score: best.2,
        margin: (best.2 - second).max(0.0),
    })
}

/// Score how well a histogram matches `pattern` rotated to `root`. Range:
/// roughly [-0.8, 1.0] depending on histogram concentration.
fn score_scale(hist: &PitchHistogram, pattern: &[u8; 12], root: u8) -> f32 {
    let total = hist.total();
    if total == 0.0 {
        return 0.0;
    }
    let mut score = 0.0;
    for i in 0..12usize {
        // i is the pitch class; rotate so root=0 maps to pattern[0].
        let scale_pos = (i + 12 - root as usize) % 12;
        let in_scale = pattern[scale_pos] == 1;
        if in_scale {
            score += hist.counts[i];
        } else {
            // Penalise out-of-scale notes proportionally — a single accidental
            // shouldn't destroy the guess, but a pile of them should.
            score -= hist.counts[i] * 0.8;
        }
    }
    score / total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_is_a4() {
        let (n, o, c) = freq_to_note(440.0);
        assert_eq!(n, "A");
        assert_eq!(o, 4);
        assert!(c.abs() <= 1);
    }

    #[test]
    fn middle_c_is_c4() {
        let (n, o, _) = freq_to_note(261.63);
        assert_eq!(n, "C");
        assert_eq!(o, 4);
    }

    #[test]
    fn guesses_c_major_from_c_major_notes() {
        // Hum C, E, G, B repeatedly — should land on C major (or its modes).
        let mut h = PitchHistogram::new();
        for _ in 0..20 {
            h.observe(261.63); // C4
            h.observe(329.63); // E4
            h.observe(392.00); // G4
            h.observe(493.88); // B4
        }
        let g = guess_scale(&h).expect("should guess");
        // C, E, G, B are all in C major. Should pick C-rooted major or pent.
        assert!(matches!(g.root_name, "C" | "G" | "E" | "A"));
    }
}
