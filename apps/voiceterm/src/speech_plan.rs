//! v0.1.701: speech-plan — the "read this section" blend of a recorded
//! utterance DB with TTS, for more natural read-aloud. Given the target text
//! and the set of phrases we have real clips for, greedily segment the text
//! into the LONGEST available recorded spans (played from the clip DB) with
//! TTS filling the gaps. So common phrases sound like the real speaker and
//! only the novel bits are synthesized.
//!
//! PURE + testable. The actual audio playback (clip files + a TTS engine) is
//! the runtime layer; this just decides WHAT to play and HOW.

/// One segment of the rendered utterance.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// play a recorded clip for this phrase (DB key = the phrase, lowercased)
    Clip(String),
    /// synthesize this text with TTS
    Tts(String),
}

/// Build a render plan for `text` given the phrases we have clips for.
/// Greedy longest-match over word spans: at each position, try the longest
/// run of words that exists in `available` (case-insensitive); if found, emit
/// a Clip and advance past it; otherwise accumulate the word into a pending
/// TTS run. Adjacent TTS words are merged into one Tts segment.
///
/// `max_span` caps how many words a single clip match may cover (clips are
/// usually short phrases; a huge match is unlikely and slow to check).
pub fn plan_utterance(text: &str, available: &[String], max_span: usize) -> Vec<Segment> {
    let words: Vec<&str> = text.split_whitespace().collect();
    // normalized clip set for O(1) lookup
    let set: std::collections::HashSet<String> =
        available.iter().map(|p| normalize(p)).collect();
    let mut out: Vec<Segment> = Vec::new();
    let mut tts_buf: Vec<&str> = Vec::new();
    let mut i = 0;
    let cap = max_span.max(1);
    while i < words.len() {
        // try the longest clip-backed span starting at i
        let mut matched: Option<usize> = None; // span length in words
        let upper = cap.min(words.len() - i);
        for len in (1..=upper).rev() {
            let span = normalize(&words[i..i + len].join(" "));
            if set.contains(&span) {
                matched = Some(len);
                break;
            }
        }
        match matched {
            Some(len) => {
                // flush any pending TTS run first
                if !tts_buf.is_empty() {
                    out.push(Segment::Tts(tts_buf.join(" ")));
                    tts_buf.clear();
                }
                out.push(Segment::Clip(normalize(&words[i..i + len].join(" "))));
                i += len;
            }
            None => {
                tts_buf.push(words[i]);
                i += 1;
            }
        }
    }
    if !tts_buf.is_empty() {
        out.push(Segment::Tts(tts_buf.join(" ")));
    }
    out
}

/// What fraction of the words are covered by real clips (0..1) — a "realism"
/// estimate the UI can surface ("82% in your voice").
pub fn clip_coverage(plan: &[Segment]) -> f32 {
    let mut clip_words = 0usize;
    let mut total = 0usize;
    for seg in plan {
        let n = match seg {
            Segment::Clip(p) => { let c = p.split_whitespace().count(); clip_words += c; c }
            Segment::Tts(t) => t.split_whitespace().count(),
        };
        total += n;
    }
    if total == 0 { 0.0 } else { clip_words as f32 / total as f32 }
}

/// Wire for QML: one segment per line, "C\t<phrase>" or "T\t<text>".
pub fn plan_to_wire(plan: &[Segment]) -> String {
    plan.iter().map(|s| match s {
        Segment::Clip(p) => format!("C\t{}", p),
        Segment::Tts(t) => format!("T\t{}", t),
    }).collect::<Vec<_>>().join("\n")
}

fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
        .chars().filter(|c| !matches!(c, '.' | ',' | '!' | '?' | ';' | ':'))
        .collect::<String>()
        .split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn av(p: &[&str]) -> Vec<String> { p.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn all_tts_when_no_clips() {
        let plan = plan_utterance("hello there world", &[], 4);
        assert_eq!(plan, vec![Segment::Tts("hello there world".into())]);
    }

    #[test]
    fn single_word_clip_splices() {
        let plan = plan_utterance("the quick brown fox", &av(&["quick"]), 4);
        assert_eq!(plan, vec![
            Segment::Tts("the".into()),
            Segment::Clip("quick".into()),
            Segment::Tts("brown fox".into()),
        ]);
    }

    #[test]
    fn prefers_longest_span() {
        // both "open the" and "open" are available → take the longer
        let plan = plan_utterance("open the editor", &av(&["open", "open the"]), 4);
        assert_eq!(plan[0], Segment::Clip("open the".into()));
        assert_eq!(plan[1], Segment::Tts("editor".into()));
    }

    #[test]
    fn punctuation_normalized() {
        // "Hello," should match a "hello" clip
        let plan = plan_utterance("Hello, friend", &av(&["hello"]), 4);
        assert_eq!(plan[0], Segment::Clip("hello".into()));
    }

    #[test]
    fn coverage_fraction() {
        let plan = plan_utterance("the quick brown fox", &av(&["quick brown"]), 4);
        // "quick brown" = 2 of 4 words → 0.5
        assert!((clip_coverage(&plan) - 0.5).abs() < 0.01);
        assert!((clip_coverage(&plan_utterance("a b c", &[], 4))).abs() < 0.01);
    }

    #[test]
    fn wire_round_trip() {
        let plan = plan_utterance("hi there", &av(&["hi"]), 4);
        let w = plan_to_wire(&plan);
        assert!(w.contains("C\thi"));
        assert!(w.contains("T\tthere"));
    }

    #[test]
    fn max_span_caps_match_length() {
        // even though the whole thing is "available", max_span 2 limits it
        let plan = plan_utterance("a b c d", &av(&["a b c d", "a b"]), 2);
        assert_eq!(plan[0], Segment::Clip("a b".into()));
    }
}
