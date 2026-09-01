// src/key_detection.rs
//
// Whether `tempo_from_analysis`, `key_from_analysis`, and `camelot_to_keytext`
// are reachable depends on the analysis feature flag, so dead-code warnings
// here are noise. The whole module is intentionally pre-staged.
#![allow(dead_code)]

// Derives human-readable BPM and Camelot key from bliss's Analysis.
//
// ── BPM (Tempo) ─────────────────────────────────────────────────────────────
//
// bliss's `AnalysisIndex::Tempo` is normalized to roughly [-1, 1]. The
// blissify reference implementation uses a denormalization step. The exact
// scaling factor depends on the bliss version; the canonical formula from
// blissify is:
//
//     bpm = (normalized + 1.0) * 100.0
//
// which maps [-1, 1] -> [0, 200]. This is a reasonable approximation
// for most pop/rock/electronic music (~60–180 BPM range). It's not
// frame-accurate beat detection; it's the song's dominant tempo
// summary feature.
//
// ── Key ─────────────────────────────────────────────────────────────────────
//
// HONEST CAVEAT: bliss exposes Chroma1..Chroma10 as *summarization* features
// over time (means/medians/etc), NOT a raw 12-bin chromagram. Computing
// Krumhansl-Schmuckler key correlation against these would be wrong.
//
// For v1 we ship a STUB that returns key_text="—" and camelot="?". The
// UI is built to handle missing keys gracefully (the chip just reads "—"
// when no key is known).
//
// When you're ready to ship real key detection, the integration point is
// `key_from_analysis()`. Two paths forward:
//
//   1. Call into aubio's `notes` / `pitch` detector via aubio-rs (bliss
//      already pulls aubio in). Extract chroma frame-by-frame, sum across
//      the song, correlate with major/minor profiles.
//
//   2. Use a separate crate like `bbloop` or roll a small chroma+KS
//      detector against a Symphonia decode stream.
//
// Until then: BPM is real and useful, key is a placeholder users can
// override manually via a "tag track" UI affordance.

#[cfg(feature = "analysis")]
use bliss_audio::{Analysis, AnalysisIndex};

#[cfg(feature = "analysis")]
pub fn tempo_from_analysis(analysis: &Analysis) -> f32 {
    let normalized = analysis[AnalysisIndex::Tempo];
    // Standard blissify denormalization.
    let bpm = (normalized + 1.0) * 100.0;
    bpm.clamp(40.0, 240.0)
}

#[cfg(feature = "analysis")]
pub fn key_from_analysis(_analysis: &Analysis) -> (String, String) {
    // (camelot, key_text). See module docs.
    ("?".to_string(), "—".to_string())
}

// ── No-analysis stubs (for cargo check without bliss-audio) ─────────────────

#[cfg(not(feature = "analysis"))]
pub fn tempo_from_analysis(_analysis: &()) -> f32 { 0.0 }

#[cfg(not(feature = "analysis"))]
pub fn key_from_analysis(_analysis: &()) -> (String, String) {
    ("?".to_string(), "—".to_string())
}

// ─── Camelot wheel utilities (work regardless of how key is derived) ────────
//
// Camelot notation is a DJ shorthand for the circle of fifths:
//   1A=Ab minor   1B=B major
//   2A=Eb minor   2B=F# major
//   ...
//   12A=Dm minor  12B=A major
//
// Compatible mixing rules (the "energy boost" rules):
//   - Same key       : same Camelot number+letter (e.g. 8A -> 8A)
//   - Energy ±1      : ±1 number, same letter   (8A -> 7A, 9A)
//   - Relative       : same number, flip letter (8A -> 8B)
//
// These four targets per key are what the "Compatible now" surface uses.

pub fn camelot_neighbors(camelot: &str) -> Vec<String> {
    let parsed = parse_camelot(camelot);
    let Some((num, letter)) = parsed else { return vec![] };

    let wrap = |n: i32| -> i32 { ((n - 1).rem_euclid(12)) + 1 };
    let other_letter = if letter == 'A' { 'B' } else { 'A' };

    vec![
        format!("{}{}", num, letter),
        format!("{}{}", wrap(num - 1), letter),
        format!("{}{}", wrap(num + 1), letter),
        format!("{}{}", num, other_letter),
    ]
}

fn parse_camelot(s: &str) -> Option<(i32, char)> {
    let s = s.trim();
    if s.len() < 2 || s.len() > 3 { return None }
    let (num_part, letter_part) = s.split_at(s.len() - 1);
    let num = num_part.parse::<i32>().ok()?;
    let letter = letter_part.chars().next()?;
    if !(1..=12).contains(&num) { return None }
    if letter != 'A' && letter != 'B' { return None }
    Some((num, letter.to_ascii_uppercase()))
}

/// Pretty-print a Camelot code as "Key major/minor" if recognized.
pub fn camelot_to_keytext(camelot: &str) -> String {
    let table = [
        // (camelot, key_text)
        ("1A", "Ab minor"), ("1B", "B major"),
        ("2A", "Eb minor"), ("2B", "F# major"),
        ("3A", "Bb minor"), ("3B", "Db major"),
        ("4A", "F minor"),  ("4B", "Ab major"),
        ("5A", "C minor"),  ("5B", "Eb major"),
        ("6A", "G minor"),  ("6B", "Bb major"),
        ("7A", "D minor"),  ("7B", "F major"),
        ("8A", "A minor"),  ("8B", "C major"),
        ("9A", "E minor"),  ("9B", "G major"),
        ("10A", "B minor"), ("10B", "D major"),
        ("11A", "F# minor"),("11B", "A major"),
        ("12A", "C# minor"),("12B", "E major"),
    ];
    table.iter().find(|(c, _)| *c == camelot)
        .map(|(_, k)| k.to_string())
        .unwrap_or_else(|| "—".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbors_basic() {
        let n = camelot_neighbors("8A");
        assert!(n.contains(&"8A".to_string()));
        assert!(n.contains(&"7A".to_string()));
        assert!(n.contains(&"9A".to_string()));
        assert!(n.contains(&"8B".to_string()));
    }

    #[test]
    fn neighbors_wrap() {
        let n = camelot_neighbors("12A");
        assert!(n.contains(&"1A".to_string()));
        assert!(n.contains(&"11A".to_string()));
    }

    #[test]
    fn neighbors_wrap_low() {
        let n = camelot_neighbors("1B");
        assert!(n.contains(&"12B".to_string()));
        assert!(n.contains(&"2B".to_string()));
    }
}
