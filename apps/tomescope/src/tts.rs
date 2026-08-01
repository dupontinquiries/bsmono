//! Text-to-speech: a sentence queue built by walking the book content
//! from a given locus, plus an optional `espeak-ng` backend (spawned as
//! a subprocess) behind the `espeak` feature. Kept backend-agnostic via
//! a trait so a platform TTS engine could be swapped in later without
//! touching the queue-building logic.

use crate::content::{BookContent, Locus};

/// One TTS-ready utterance: plain sentence text plus the locus range it
/// covers, so the reader can highlight along as speech progresses.
#[derive(Clone, Debug)]
pub struct Utterance {
    pub text: String,
    pub block: usize,
    pub char_start: usize,
    pub char_end: usize,
}

/// Split a block's plain text into sentence-ish chunks for TTS. This is a
/// simple punctuation-based splitter (., !, ?, and paragraph end), not a
/// full sentence-boundary model — adequate for narration pacing.
pub fn split_sentences(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '.' || c == '!' || c == '?' {
            // Avoid splitting on decimal points (e.g. "3.14") or on
            // abbreviations followed immediately by a lowercase letter
            // (e.g. "e.g. next").
            let prev_is_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_is_digit = chars.get(i + 1).map(|c| c.is_ascii_digit()).unwrap_or(false);
            let is_decimal_point = c == '.' && prev_is_digit && next_is_digit;
            let next_is_lower = chars.get(i + 1).map(|c| c.is_lowercase()).unwrap_or(false);
            if !next_is_lower && !is_decimal_point {
                let end = i + 1;
                if end > start {
                    ranges.push((start, end));
                }
                start = end;
            }
        }
        i += 1;
    }
    if start < chars.len() {
        ranges.push((start, chars.len()));
    }
    ranges
        .into_iter()
        .map(|(s, e)| {
            // trim leading whitespace from the range start
            let mut s2 = s;
            while s2 < e && chars[s2].is_whitespace() {
                s2 += 1;
            }
            (s2, e)
        })
        .filter(|(s, e)| e > s)
        .collect()
}

/// Build a queue of utterances starting at `from`, covering up to
/// `max_utterances` sentences (0 = unlimited / whole book from that
/// point), skipping non-text blocks.
pub fn build_queue(content: &BookContent, from: Locus, max_utterances: usize) -> Vec<Utterance> {
    let mut queue = Vec::new();
    for (bi, block) in content.blocks.iter().enumerate().skip(from.block) {
        let text = block.plain_text();
        if text.trim().is_empty() {
            continue;
        }
        let sentence_ranges = split_sentences(&text);
        for (s, e) in sentence_ranges {
            if bi == from.block && e <= from.ch {
                continue;
            }
            let slice: String = text.chars().skip(s).take(e - s).collect();
            queue.push(Utterance {
                text: slice,
                block: bi,
                char_start: s,
                char_end: e,
            });
            if max_utterances > 0 && queue.len() >= max_utterances {
                return queue;
            }
        }
    }
    queue
}

/// TTS playback state machine — backend-agnostic; the UI drives `tick`
/// each frame and reacts to `is_speaking`/`current_index` to highlight
/// the active sentence.
pub struct TtsQueuePlayer {
    pub queue: Vec<Utterance>,
    pub current_index: usize,
    pub is_speaking: bool,
    pub rate: f32,
    pub pitch: f32,
    pub voice: String,
}

impl Default for TtsQueuePlayer {
    fn default() -> Self {
        Self {
            queue: Vec::new(),
            current_index: 0,
            is_speaking: false,
            rate: 1.0,
            pitch: 1.0,
            voice: "en".to_string(),
        }
    }
}

impl TtsQueuePlayer {
    pub fn load(&mut self, queue: Vec<Utterance>) {
        self.queue = queue;
        self.current_index = 0;
    }

    pub fn current(&self) -> Option<&Utterance> {
        self.queue.get(self.current_index)
    }

    pub fn advance(&mut self) -> bool {
        if self.current_index + 1 < self.queue.len() {
            self.current_index += 1;
            true
        } else {
            self.is_speaking = false;
            false
        }
    }

    pub fn current_locus(&self) -> Option<Locus> {
        self.current().map(|u| Locus {
            block: u.block,
            ch: u.char_start,
        })
    }
}

/// Backend abstraction for actually producing audio from text.
pub trait TtsBackend {
    fn speak(&mut self, text: &str, rate: f32, pitch: f32, voice: &str) -> anyhow::Result<()>;
    fn stop(&mut self) -> anyhow::Result<()>;
    fn is_available(&self) -> bool;
}

/// No-op backend used when no TTS engine is compiled in — the queue and
/// highlighting still work (useful for testing / read-along without
/// audio), it just never actually produces sound.
pub struct SilentBackend;

impl TtsBackend for SilentBackend {
    fn speak(&mut self, _text: &str, _rate: f32, _pitch: f32, _voice: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        false
    }
}

#[cfg(feature = "espeak")]
pub struct EspeakBackend {
    child: Option<std::process::Child>,
}

#[cfg(feature = "espeak")]
impl Default for EspeakBackend {
    fn default() -> Self {
        Self { child: None }
    }
}

#[cfg(feature = "espeak")]
impl TtsBackend for EspeakBackend {
    fn speak(&mut self, text: &str, rate: f32, pitch: f32, voice: &str) -> anyhow::Result<()> {
        self.stop()?;
        let wpm = (175.0 * rate).clamp(80.0, 450.0) as u32;
        let pitch_arg = (50.0 * pitch).clamp(0.0, 99.0) as u32;
        let child = std::process::Command::new("espeak-ng")
            .arg("-s")
            .arg(wpm.to_string())
            .arg("-p")
            .arg(pitch_arg.to_string())
            .arg("-v")
            .arg(voice)
            .arg(text)
            .spawn()?;
        self.child = Some(child);
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
        Ok(())
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("espeak-ng")
            .arg("--version")
            .output()
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_split_basic() {
        let ranges = split_sentences("Hello world. This is a test! Is it working?");
        assert_eq!(ranges.len(), 3);
    }

    #[test]
    fn sentence_split_avoids_decimal_points() {
        let ranges = split_sentences("Pi is 3.14 approximately. Next sentence.");
        // "3.14" shouldn't be split at the decimal point.
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn queue_player_advances_and_stops() {
        let mut player = TtsQueuePlayer::default();
        player.load(vec![
            Utterance { text: "a".into(), block: 0, char_start: 0, char_end: 1 },
            Utterance { text: "b".into(), block: 0, char_start: 1, char_end: 2 },
        ]);
        assert!(player.advance());
        assert_eq!(player.current_index, 1);
        assert!(!player.advance());
    }
}
