//! audio — dropped sound files, and the shapes their transcripts take.
//!
//! ## Format is detected from the bytes, not the extension
//!
//! A `.wav` that is actually FLAC is common — converters rename, browsers
//! guess, and phones save with whatever suffix the app chose. Sniffing the
//! magic bytes costs twelve bytes of read and removes a whole class of "it
//! worked on my machine".
//!
//! ## Why JSON Lines rather than YAML for the structured export
//!
//! Both were on the table and JSONL wins on a property specific to *speech*:
//! transcribed text is full of colons, dashes and leading quotes — "so, the
//! thing is: it didn't work" — and every one of those is a YAML quoting hazard.
//! A YAML export of speech either needs careful escaping on every line or
//! silently reparses as a map. JSON has one escaping rule and it is boring.
//!
//! Lines rather than one array because a transcript is **append-shaped**: a
//! long recording can be written and read segment by segment, `tail -f` works,
//! and a truncated file loses the last line instead of the whole document.

/// Audio container, as sniffed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// Ogg container — Opus or Vorbis inside.
    Ogg,
    Flac,
    Wav,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self { Format::Ogg => "Ogg/Opus", Format::Flac => "FLAC", Format::Wav => "WAV" }
    }
}

/// Sniff a container from its leading bytes.
///
/// Returns `None` for anything unrecognised rather than guessing: a wrong guess
/// hands a decoder bytes it will misread, and "unsupported file" is a better
/// message than a transcript of noise.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.len() < 12 { return None; }
    match &bytes[..4] {
        b"OggS" => Some(Format::Ogg),
        b"fLaC" => Some(Format::Flac),
        // RIFF is a container family; only the WAVE form is audio we want, and
        // RIFF/AVI shares the first four bytes exactly.
        b"RIFF" if &bytes[8..12] == b"WAVE" => Some(Format::Wav),
        _ => None,
    }
}

/// One piece of recognised speech.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// 0..=1, same meaning as an OCR region's score.
    pub score: f32,
}

impl Segment {
    /// `HH:MM:SS.mmm`, fixed width so a column of them aligns.
    pub fn timestamp(&self) -> String {
        let ms = self.start_ms;
        format!("{:02}:{:02}:{:02}.{:03}",
                ms / 3_600_000, (ms / 60_000) % 60, (ms / 1_000) % 60, ms % 1_000)
    }
    pub fn is_confident(&self) -> bool { self.score >= crate::CONFIDENT }
}

/// What a transcription backend must do. Mirrors [`crate::Ocr`] deliberately:
/// both turn a blob into scored, positioned text, and an app that treats them
/// as one shape gets one set of export and overview code instead of two.
pub trait Transcriber {
    fn transcribe(&self, bytes: &[u8], format: Format) -> Result<Vec<Segment>, crate::OcrError>;
}

/// How to export a transcript.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Export {
    /// Just the words.
    Raw,
    /// `[00:00:12.400] words`
    Timestamped,
    /// One JSON object per line.
    Jsonl,
}

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            // Control characters are legal in speech-to-text output only by
            // accident, and an unescaped one makes the whole line unparseable.
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

pub fn export(segments: &[Segment], how: Export) -> String {
    let mut out = String::new();
    for s in segments {
        match how {
            Export::Raw => { out.push_str(s.text.trim()); out.push('\n'); }
            Export::Timestamped => {
                out.push('[');
                out.push_str(&s.timestamp());
                out.push_str("] ");
                out.push_str(s.text.trim());
                out.push('\n');
            }
            Export::Jsonl => {
                out.push_str(&format!(
                    "{{\"start_ms\":{},\"end_ms\":{},\"score\":{:.3},\"text\":\"{}\"}}\n",
                    s.start_ms, s.end_ms, s.score, json_escape(s.text.trim())));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: u64, end: u64, text: &str, score: f32) -> Segment {
        Segment { start_ms: start, end_ms: end, text: text.into(), score }
    }

    fn hdr(tag: &[u8], form: &[u8]) -> Vec<u8> {
        let mut v = tag.to_vec();
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(form);
        v
    }

    #[test]
    fn containers_are_sniffed_from_bytes_not_extensions() {
        // A .wav that is actually FLAC is common: converters rename, browsers
        // guess, phones save with whatever suffix the app chose.
        assert_eq!(sniff(&hdr(b"OggS", b"____")), Some(Format::Ogg));
        assert_eq!(sniff(&hdr(b"fLaC", b"____")), Some(Format::Flac));
        assert_eq!(sniff(&hdr(b"RIFF", b"WAVE")), Some(Format::Wav));
    }

    #[test]
    fn a_riff_that_is_not_wave_is_refused() {
        // RIFF is a container family and AVI shares the first four bytes
        // exactly. Guessing hands a decoder bytes it will misread, and
        // "unsupported" beats a transcript of noise.
        assert_eq!(sniff(&hdr(b"RIFF", b"AVI ")), None);
    }

    #[test]
    fn a_short_or_unknown_file_is_none_rather_than_a_guess() {
        assert_eq!(sniff(b"Ogg"), None, "too short to be sure");
        assert_eq!(sniff(&hdr(b"%PDF", b"____")), None);
        assert_eq!(sniff(&[]), None);
    }

    #[test]
    fn timestamps_are_fixed_width_so_a_column_aligns() {
        assert_eq!(seg(0, 1, "", 1.0).timestamp(), "00:00:00.000");
        assert_eq!(seg(12_400, 1, "", 1.0).timestamp(), "00:00:12.400");
        assert_eq!(seg(3_723_456, 1, "", 1.0).timestamp(), "01:02:03.456");
    }

    #[test]
    fn raw_export_is_only_the_words() {
        let s = vec![seg(0, 1000, " hello ", 0.9), seg(1000, 2000, "world", 0.9)];
        assert_eq!(export(&s, Export::Raw), "hello\nworld\n");
    }

    #[test]
    fn timestamped_export_leads_with_the_time() {
        let s = vec![seg(12_400, 13_000, "hello", 0.9)];
        assert_eq!(export(&s, Export::Timestamped), "[00:00:12.400] hello\n");
    }

    #[test]
    fn jsonl_survives_the_punctuation_speech_is_full_of() {
        // The reason this is JSONL rather than YAML: colons, dashes and leading
        // quotes are everywhere in speech, and each is a YAML quoting hazard
        // that either needs escaping or silently reparses as a map.
        let s = vec![seg(0, 1, "so, the thing is: it \"didn't\" work — really", 0.87)];
        let line = export(&s, Export::Jsonl);
        assert!(line.contains(r#"\"didn't\""#), "quotes escaped: {line}");
        assert!(line.contains("is:"), "a colon needs no special handling here");
        assert!(line.ends_with("\n"));
        assert_eq!(line.lines().count(), 1, "one segment, one line");
    }

    #[test]
    fn jsonl_escapes_control_characters_rather_than_emitting_them() {
        // Legal in speech-to-text output only by accident, and one unescaped
        // control character makes the whole line unparseable.
        let s = vec![seg(0, 1, "a\nb\tc", 0.9)];
        let line = export(&s, Export::Jsonl);
        assert!(line.contains("a\\nb\\tc"));
        assert_eq!(line.lines().count(), 1, "the newline did not split the record");
    }

    #[test]
    fn jsonl_is_one_line_per_segment_so_a_truncated_file_loses_one_segment() {
        // Append-shaped on purpose: a long recording can be written and read
        // segment by segment, and a cut-off file loses the last line rather
        // than the whole document.
        let s: Vec<Segment> = (0..5).map(|i| seg(i * 1000, i * 1000 + 999, "x", 0.9)).collect();
        assert_eq!(export(&s, Export::Jsonl).lines().count(), 5);
    }

    #[test]
    fn an_empty_transcript_exports_to_nothing_in_every_format() {
        for how in [Export::Raw, Export::Timestamped, Export::Jsonl] {
            assert_eq!(export(&[], how), "", "{how:?}");
        }
    }
}
