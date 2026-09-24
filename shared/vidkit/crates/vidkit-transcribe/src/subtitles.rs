//! Cues from a subtitle file: WebVTT or SRT.
//!
//! When a transcript already exists, whisper is waste. yt-dlp fetches
//! YouTube's captions (`--write-auto-subs --sub-format vtt`) for free, and a
//! downloaded lecture often sits next to a `.vtt`. This is also the source the
//! UI is exercised with when no model is installed.
//!
//! YouTube's AUTO captions are not ordinary WebVTT: each cue repeats the
//! previous line and adds the new one with per-word `<00:00:01.234><c>`
//! timing tags, so a naive parser yields every sentence twice. The fix is to
//! strip tags, then drop any line identical to the line before it.

use std::time::Duration;

use crate::{Cue, CueSource};

/// Parse `HH:MM:SS.mmm`, `MM:SS.mmm`, or SRT's `HH:MM:SS,mmm`.
fn timestamp(s: &str) -> Option<Duration> {
    let s = s.trim().replace(',', ".");
    let (hms, frac) = s.split_once('.').unwrap_or((&s, "0"));
    let parts: Vec<&str> = hms.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?, s.parse::<u64>().ok()?),
        [m, s] => (0, m.parse::<u64>().ok()?, s.parse::<u64>().ok()?),
        _ => return None,
    };
    // Millisecond field, normalised to three digits: ".5" is 500 ms, not 5.
    let frac: String = frac.chars().take_while(|c| c.is_ascii_digit()).take(3).collect();
    let ms = format!("{frac:0<3}").parse::<u64>().ok()?;
    Some(Duration::from_millis(((h * 60 + m) * 60 + sec) * 1000 + ms))
}

/// Remove `<...>` markup and decode the few entities captions use.
fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut depth = 0;
    for ch in line.chars() {
        match ch {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ")
}

/// Parse WebVTT or SRT text. Unknown blocks (NOTE, STYLE, headers) are skipped.
pub fn parse(text: &str) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    let mut last_line = String::new();
    let text = text.replace("\r\n", "\n");
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|l| !l.contains("-->"));
        let Some(timing) = lines.next() else { continue };
        let Some((a, b)) = timing.split_once("-->") else { continue };
        // Cue settings follow the end time: "00:01.000 align:start position:0%".
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (timestamp(a), timestamp(b)) else { continue };

        let mut kept: Vec<String> = Vec::new();
        for raw in lines {
            let line = strip_tags(raw);
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if line.is_empty() || line == last_line {
                continue;
            }
            last_line = line.clone();
            kept.push(line);
        }
        if kept.is_empty() {
            continue;
        }
        cues.push(Cue { start, end: end.max(start), text: kept.join(" ") });
    }
    cues.sort_by_key(|c| c.start);
    cues
}

/// A [`CueSource`] that ignores the audio and answers from a file.
pub struct SubtitleSource {
    name: String,
    cues: Vec<Cue>,
}

impl SubtitleSource {
    pub fn from_text(name: impl Into<String>, text: &str) -> Self {
        Self { name: name.into(), cues: parse(text) }
    }

    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let s = Self::from_text(
            format!("subtitles:{}", path.file_name().unwrap_or_default().to_string_lossy()),
            &text,
        );
        if s.cues.is_empty() {
            return Err(format!("{}: no cues", path.display()));
        }
        Ok(s)
    }

    pub fn all(&self) -> &[Cue] {
        &self.cues
    }
}

impl CueSource for SubtitleSource {
    /// Every cue starting in the span `pcm` would cover. With no audio the
    /// span is unknown, so the shell uses [`SubtitleSource::all`] instead.
    fn transcribe(&mut self, pcm: &[f32], offset: Duration) -> Result<Vec<Cue>, String> {
        let end = offset + Duration::from_secs_f64(pcm.len() as f64 / crate::SAMPLE_RATE as f64);
        Ok(self.cues.iter().filter(|c| c.start >= offset && c.start < end).cloned().collect())
    }
    fn name(&self) -> String {
        self.name.clone()
    }
    fn complete(&mut self) -> Option<Vec<Cue>> {
        Some(self.cues.clone())
    }
}

/// Sidecar files to try for a video, best first: same stem, then yt-dlp's
/// `<stem>.<lang>.vtt` naming.
pub fn sidecar_candidates(video: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Some(stem) = video.file_stem() else { return Vec::new() };
    let dir = video.parent().unwrap_or(std::path::Path::new("."));
    let stem = stem.to_string_lossy();
    let mut out = Vec::new();
    for suffix in ["vtt", "srt", "en.vtt", "en.srt", "en-orig.vtt", "en-US.vtt"] {
        out.push(dir.join(format!("{stem}.{suffix}")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn timestamps_in_every_form() {
        assert_eq!(timestamp("01:02:03.456"), Some(ms(3_723_456)));
        assert_eq!(timestamp("02:03.4"), Some(ms(123_400)), ".4 is 400 ms, not 4");
        assert_eq!(timestamp("00:00:01,250"), Some(ms(1_250)), "SRT comma");
        assert_eq!(timestamp("12"), None);
        assert_eq!(timestamp("aa:bb.000"), None);
    }

    #[test]
    fn plain_webvtt_with_settings_and_notes() {
        let v = "WEBVTT\nKind: captions\n\nNOTE a comment\n\n1\n00:00:01.000 --> 00:00:03.500 align:start position:0%\nHello <b>world</b>\n\n00:04.000 --> 00:05.000\nTom &amp; Jerry\nsecond line\n";
        let c = parse(v);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], Cue { start: ms(1_000), end: ms(3_500), text: "Hello world".into() });
        assert_eq!(c[1].text, "Tom & Jerry second line");
    }

    #[test]
    fn srt_with_crlf() {
        let s = "1\r\n00:00:00,500 --> 00:00:02,000\r\nfirst\r\n\r\n2\r\n00:00:02,000 --> 00:00:04,000\r\nsecond\r\n";
        let c = parse(s);
        assert_eq!(c.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["first", "second"]);
        assert_eq!(c[0].start, ms(500));
    }

    /// YouTube auto-captions: each cue repeats the previous line. Without
    /// deduplication every sentence is in the transcript twice, which doubles
    /// every term count and wrecks the chapter keywords.
    #[test]
    fn youtube_rolling_captions_are_not_doubled() {
        let v = "WEBVTT\n\n00:00:00.000 --> 00:00:02.000 align:start position:0%\n \nthe<00:00:00.500><c> fourier</c><00:00:01.000><c> transform</c>\n\n00:00:02.000 --> 00:00:02.010 align:start position:0%\nthe fourier transform\n \n\n00:00:02.010 --> 00:00:04.000 align:start position:0%\nthe fourier transform\nmaps<00:00:02.500><c> time</c><00:00:03.000><c> to</c><00:00:03.500><c> frequency</c>\n";
        let c = parse(v);
        let all: String = c.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" | ");
        assert_eq!(all, "the fourier transform | maps time to frequency");
    }

    #[test]
    fn the_source_answers_the_span_and_wants_no_audio() {
        let mut s = SubtitleSource::from_text("t", "WEBVTT\n\n00:01.000 --> 00:02.000\na\n\n00:31.000 --> 00:32.000\nb\n");
        assert_eq!(s.complete().map(|c| c.len()), Some(2), "all of it, up front");
        let pcm = vec![0.0f32; 30 * crate::SAMPLE_RATE as usize];
        let got = s.transcribe(&pcm, Duration::ZERO).unwrap();
        assert_eq!(got.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["a"]);
    }

    #[test]
    fn sidecars_include_yt_dlp_naming() {
        let c = sidecar_candidates(std::path::Path::new("/v/Lecture 3.mp4"));
        assert!(c.contains(&std::path::PathBuf::from("/v/Lecture 3.vtt")));
        assert!(c.contains(&std::path::PathBuf::from("/v/Lecture 3.en.vtt")));
    }
}
