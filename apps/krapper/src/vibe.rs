//! The contract between the audio side (Path A) and the writing side (Path B).
//!
//! [`VibeSummary`] is what Vibe-capture *will* produce and what Scratchpad
//! consumes today. For Path B (ship-first), the Scratchpad fills it in by
//! hand via the Vibe inputs panel. For Path A, it'll come from analysing the
//! user's hum-over recording.
//!
//! Keep this module dependency-light so the struct can be serialised for the
//! arranger handoff without dragging audio code along.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VibeSummary {
    pub bpm: f32,
    pub syllables_per_bar: u8,
    /// Stress pattern across one bar. `1` = stressed syllable, `0` = unstressed.
    /// When fully specified, `len() == syllables_per_bar`. Shorter is fine —
    /// the suggester repeats the pattern to cover the bar.
    pub stress_pattern: Vec<u8>,
    /// Anchor rhyme words the writer wants line endings to land on. Empty =
    /// no rhyme constraint.
    pub rhyme_seeds: Vec<String>,
    pub vocab_name: String,
    pub source: VibeSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VibeSource {
    /// Filled in by hand in the Vibe inputs panel. (Path B)
    Manual,
    /// Computed from a recorded hum-over take. (Path A — `audio_path` is the
    /// saved take on disk so the writer can replay alongside the lyrics.)
    Recording { audio_path: String },
}

impl Default for VibeSummary {
    /// A reasonable starting point: mid-tempo 4/4 with alternating stress.
    fn default() -> Self {
        Self {
            bpm: 80.0,
            syllables_per_bar: 8,
            stress_pattern: vec![1, 0, 1, 0, 1, 0, 1, 0],
            rhyme_seeds: vec![],
            vocab_name: "Juice WRLD".to_string(),
            source: VibeSource::Manual,
        }
    }
}

impl VibeSummary {
    /// Beats per second.
    pub fn bps(&self) -> f32 {
        self.bpm / 60.0
    }

    /// Milliseconds per bar (assuming 4 beats per bar).
    pub fn ms_per_bar(&self) -> f32 {
        4.0 * 1000.0 / self.bps()
    }

    /// The stress pattern stretched/truncated to exactly `syllables_per_bar`
    /// values. If the stored pattern is shorter, it tiles. Empty patterns
    /// yield all-unstressed (`0`s) — degrades gracefully rather than panicking.
    pub fn full_stress(&self) -> Vec<u8> {
        let n = self.syllables_per_bar as usize;
        if self.stress_pattern.is_empty() {
            return vec![0; n];
        }
        (0..n).map(|i| self.stress_pattern[i % self.stress_pattern.len()]).collect()
    }
}

/// Parse a stress pattern string. Spaces are ignored. Recognises:
/// `'1' | 'x' | 'X' | '!'` = stressed; `'0' | '.' | '-' | '_'` = unstressed.
///
/// Examples:
/// ```ignore
/// parse_stress_pattern("x.x.x.x.") == vec![1,0,1,0,1,0,1,0]
/// parse_stress_pattern("11001100") == vec![1,1,0,0,1,1,0,0]
/// parse_stress_pattern("x . x .")  == vec![1,0,1,0]
/// ```
pub fn parse_stress_pattern(s: &str) -> Vec<u8> {
    s.chars()
        .filter_map(|c| match c {
            '1' | 'x' | 'X' | '!' => Some(1u8),
            '0' | '.' | '-' | '_' => Some(0u8),
            _ => None,
        })
        .collect()
}

/// Render `[1,0,1,0]` as `"x.x."` — the conventional notation in this app.
pub fn render_stress_pattern(p: &[u8]) -> String {
    p.iter().map(|b| if *b == 1 { 'x' } else { '.' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_renders_round_trip() {
        let cases = ["x.x.x.x.", "11001100", "x", ".", ""];
        for c in &cases {
            let parsed = parse_stress_pattern(c);
            let rendered = render_stress_pattern(&parsed);
            // Re-parsing the rendered form must match.
            assert_eq!(parse_stress_pattern(&rendered), parsed, "round-trip on {c:?}");
        }
    }

    #[test]
    fn full_stress_tiles_short_patterns() {
        let v = VibeSummary {
            stress_pattern: vec![1, 0],
            syllables_per_bar: 8,
            ..Default::default()
        };
        assert_eq!(v.full_stress(), vec![1, 0, 1, 0, 1, 0, 1, 0]);
    }

    #[test]
    fn full_stress_truncates_long_patterns() {
        let v = VibeSummary {
            stress_pattern: vec![1, 0, 1, 0, 1, 0, 1, 0, 1, 0],
            syllables_per_bar: 4,
            ..Default::default()
        };
        assert_eq!(v.full_stress(), vec![1, 0, 1, 0]);
    }
}
