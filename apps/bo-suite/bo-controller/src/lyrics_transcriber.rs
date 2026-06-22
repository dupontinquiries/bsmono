// bo-controller/src/lyrics_transcriber.rs
//
// Whisper-backed lyrics transcription with smart-templated output.
//
// CHOICE OF BACKEND — we shell out to `whisper-cli` (the ggml/whisper.cpp
// CLI) rather than linking via a Rust binding for three reasons:
//
//   1. The binding crates (whisper-rs etc.) require building
//      whisper.cpp from source with the right BLAS/CUDA flags
//      for the host. The CLI sidesteps that entirely: if the user
//      has `whisper-cli` in PATH, transcription works; if not, we
//      surface a clear error and the rest of the controller is
//      unaffected.
//
//   2. The whisper.cpp project's CLI surface is far more stable
//      than its C API. Bindings rot on every minor version; the
//      CLI flags we use (-m, -f, --output-srt, --print-progress)
//      have been stable since 2023.
//
//   3. Subprocess isolation: if whisper-cli crashes on a
//      malformed input or runs out of memory on a 90-min DJ
//      mix, the controller process doesn't die. We get a non-
//      zero exit code and a clear error.
//
// This module mirrors the pattern in voiceterm/src/voice.rs:4366
// (the user's reference upload) which uses the same approach.
//
// SMART TEMPLATING — raw whisper output is one line of plain text
// (when invoked without --output-* flags) or an SRT/VTT file (with
// --output-srt). We use SRT mode because it's:
//   - Standard format any editor can show
//   - Timestamped per phrase, which is what makes "lyrics" useful
//     vs an undifferentiated wall of text
//
// We then re-render the SRT into one of three templates:
//   - "lrc": industry-standard time-stamped lyrics format,
//     compatible with music players. Each line gets a [mm:ss.cc]
//     prefix.
//   - "markdown": prose-style document with section headers for
//     verse/chorus/bridge if we can detect them via gap analysis,
//     and a metadata header with title/artist/date.
//   - "plain": just the text lines, one per phrase, no timestamps.
//
// SECTION DETECTION (markdown only) — a "section break" is a gap
// > 4 seconds between consecutive whisper-detected phrases. Most
// pop songs have ~1-2s between phrases within a verse and ~5-10s
// during instrumental bridges. We don't try to classify
// verse/chorus/bridge by content (that's a research problem);
// we just insert a visual `---` between groups so the user gets
// readable chunks instead of a wall.
//
// STORAGE — generated lyrics get cached under
// ~/.local/share/bo-controller/lyrics/<sha256-of-path>.lrc
// so subsequent reads are instant. The cache is content-addressed
// by file PATH (not by content hash) which means re-tagging a
// song invalidates the title metadata in the cache but not the
// lyrics themselves — that's the right tradeoff because the
// lyrics are tied to the audio, not the tags.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the user's whisper model lives. We probe a few common
/// install locations; failing that, the env var WHISPER_MODEL_PATH
/// is the override.
fn whisper_model_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WHISPER_MODEL_PATH") {
        let pb = PathBuf::from(&p);
        if pb.exists() { return Some(pb); }
    }
    // Common locations whisper.cpp's setup script puts models.
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [
        home.join(".local/share/whisper/ggml-base.en.bin"),
        home.join(".local/share/whisper.cpp/models/ggml-base.en.bin"),
        home.join(".cache/whisper/ggml-base.en.bin"),
        PathBuf::from("/usr/share/whisper/ggml-base.en.bin"),
        PathBuf::from("/usr/local/share/whisper/ggml-base.en.bin"),
        // Fall back to the bigger model if base isn't there.
        home.join(".local/share/whisper/ggml-small.en.bin"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Where to cache generated lyrics. One file per source track.
fn cache_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bo-controller/lyrics")
}

/// SHA-256 of the path string — used as cache key. We use the
/// path (not file content) so a small edit to the file's tags
/// doesn't invalidate hours of transcription. If the user
/// re-encodes the file to a different bitrate they should
/// invalidate manually.
fn cache_key(path: &Path) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    h.update(path.to_string_lossy().as_bytes());
    format!("{:x}", h.finalize())
}

/// One transcribed phrase with its time window.
#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    /// Start offset from the beginning of the track, in milliseconds.
    pub start_ms: u64,
    /// End offset from the beginning of the track, in milliseconds.
    pub end_ms: u64,
    /// The transcribed text. Whitespace normalized; no trailing
    /// newline.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// Time-stamped LRC format. Compatible with most music players.
    Lrc,
    /// Markdown with section breaks at >4s gaps.
    Markdown,
    /// Plain text, one phrase per line.
    Plain,
}

/// Result of a transcription.
pub struct TranscriptionResult {
    pub phrases: Vec<Phrase>,
    pub raw_srt: String,
}

/// Errors the transcriber can surface to the caller.
#[derive(Debug)]
pub enum LyricsError {
    NoModel(String),
    NoWhisperCli(String),
    InputMissing(PathBuf),
    WhisperFailed { status: Option<i32>, stderr: String },
    Io(std::io::Error),
}

impl std::fmt::Display for LyricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LyricsError::NoModel(s) => write!(f,
                "whisper model not found at any common path. \
                 Set WHISPER_MODEL_PATH=/path/to/ggml-base.en.bin or \
                 install via whisper.cpp setup script. ({})", s),
            LyricsError::NoWhisperCli(s) => write!(f,
                "whisper-cli not in PATH. Install whisper.cpp ({}).", s),
            LyricsError::InputMissing(p) => write!(f,
                "input file not found: {}", p.display()),
            LyricsError::WhisperFailed { status, stderr } => write!(f,
                "whisper-cli exited with status {:?}:\n{}", status, stderr),
            LyricsError::Io(e) => write!(f, "io: {}", e),
        }
    }
}

/// Run whisper-cli on the given audio path. Returns the parsed phrases
/// + raw SRT for diagnostics. Synchronous and potentially slow — call
/// from a worker thread.
///
/// The CLI invocation: `whisper-cli -m <model> -f <audio> --output-srt
/// --output-file <prefix> --no-prints`. We write to a temp file because
/// reading the SRT from stdout requires parsing the CLI's own progress
/// chatter; the file is a clean two-state output.
pub fn transcribe(audio_path: &Path) -> Result<TranscriptionResult, LyricsError> {
    if !audio_path.exists() {
        return Err(LyricsError::InputMissing(audio_path.to_path_buf()));
    }
    let model = whisper_model_path()
        .ok_or_else(|| LyricsError::NoModel(
            "tried ~/.local/share/whisper/, ~/.cache/whisper/, \
             /usr/share/whisper/, /usr/local/share/whisper/".to_string()))?;

    // Sanity-check whisper-cli is in PATH.
    if Command::new("whisper-cli").arg("--help")
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().is_err()
    {
        return Err(LyricsError::NoWhisperCli(
            "whisper-cli --help failed; is the binary on $PATH?".to_string()));
    }

    // Output goes to a tempdir we own + remove after parsing.
    let tmp = std::env::temp_dir().join(format!(
        "bo-lyrics-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let out_prefix = tmp.join("out");

    let result = Command::new("whisper-cli")
        .args(["-m", &model.to_string_lossy()])
        .args(["-f", &audio_path.to_string_lossy()])
        .args(["--output-srt"])
        .args(["--output-file", &out_prefix.to_string_lossy()])
        .args(["--no-prints"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(LyricsError::Io)?;

    if !result.status.success() {
        return Err(LyricsError::WhisperFailed {
            status: result.status.code(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
        });
    }

    // whisper-cli writes <prefix>.srt — read + parse.
    let srt_path = tmp.join("out.srt");
    let raw_srt = std::fs::read_to_string(&srt_path)
        .map_err(LyricsError::Io)?;
    let phrases = parse_srt(&raw_srt);

    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);

    Ok(TranscriptionResult { phrases, raw_srt })
}

/// Parse a whisper-cli-style SRT into Phrase records. Whisper's SRT
/// blocks look like:
///   1
///   00:00:01,234 --> 00:00:04,500
///   [Music] Hello world
///   (blank line)
///
/// We're tolerant of stray whitespace and accept both `,` and `.` as
/// the millisecond separator (different whisper builds vary).
pub fn parse_srt(srt: &str) -> Vec<Phrase> {
    let mut out = Vec::new();
    // Split on double-newline (block boundary).
    for block in srt.split("\n\n") {
        let block = block.trim();
        if block.is_empty() { continue; }
        let mut lines = block.lines();
        // First line is the index; skip it.
        let _idx = lines.next();
        // Second line is the time range.
        let time_line = match lines.next() { Some(s) => s, None => continue };
        let (start, end) = match parse_srt_time_range(time_line) {
            Some(v) => v,
            None => continue,
        };
        // Remaining lines are the text — join with single spaces.
        let text: String = lines.collect::<Vec<_>>().join(" ").trim().to_string();
        if !text.is_empty() {
            out.push(Phrase { start_ms: start, end_ms: end, text });
        }
    }
    out
}

fn parse_srt_time_range(line: &str) -> Option<(u64, u64)> {
    // "00:00:01,234 --> 00:00:04,500"
    let mut parts = line.split("-->");
    let l = parts.next()?.trim();
    let r = parts.next()?.trim();
    Some((parse_srt_timestamp(l)?, parse_srt_timestamp(r)?))
}

fn parse_srt_timestamp(ts: &str) -> Option<u64> {
    // hh:mm:ss,mmm or hh:mm:ss.mmm
    let ts = ts.replace(',', ".");
    let mut hms = ts.split(':');
    let h: u64 = hms.next()?.parse().ok()?;
    let m: u64 = hms.next()?.parse().ok()?;
    let sec_str = hms.next()?;
    let mut sec_parts = sec_str.split('.');
    let s: u64 = sec_parts.next()?.parse().ok()?;
    let ms: u64 = sec_parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    Some(h * 3_600_000 + m * 60_000 + s * 1_000 + ms)
}

/// Render phrases using the requested template.
///
/// `title` / `artist` are used only by Markdown for the header
/// metadata block; for LRC/Plain they're ignored.
pub fn render(
    phrases: &[Phrase],
    template: Template,
    title: &str,
    artist: &str,
) -> String {
    match template {
        Template::Lrc => render_lrc(phrases, title, artist),
        Template::Plain => render_plain(phrases),
        Template::Markdown => render_markdown(phrases, title, artist),
    }
}

fn render_lrc(phrases: &[Phrase], title: &str, artist: &str) -> String {
    let mut out = String::new();
    // LRC metadata tags. Optional but supported by most players.
    if !artist.is_empty() {
        out.push_str(&format!("[ar:{}]\n", artist));
    }
    if !title.is_empty() {
        out.push_str(&format!("[ti:{}]\n", title));
    }
    out.push_str("[tool:bo-controller lyrics_transcriber]\n");
    for p in phrases {
        // [mm:ss.cc] format — minutes:seconds.centiseconds.
        let total_sec = p.start_ms / 1000;
        let mm = total_sec / 60;
        let ss = total_sec % 60;
        let cc = (p.start_ms % 1000) / 10;
        out.push_str(&format!("[{:02}:{:02}.{:02}]{}\n", mm, ss, cc, p.text));
    }
    out
}

fn render_plain(phrases: &[Phrase]) -> String {
    phrases.iter()
        .map(|p| p.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Markdown template — wraps phrases into paragraphs separated by
/// `---` at every gap larger than SECTION_GAP_MS. Adds a metadata
/// header. Suitable for putting into Obsidian or similar.
const SECTION_GAP_MS: u64 = 4_000;

fn render_markdown(phrases: &[Phrase], title: &str, artist: &str) -> String {
    let mut out = String::new();
    // Header
    if !title.is_empty() || !artist.is_empty() {
        out.push_str("---\n");
        if !title.is_empty()  { out.push_str(&format!("title: {}\n", title)); }
        if !artist.is_empty() { out.push_str(&format!("artist: {}\n", artist)); }
        out.push_str("source: whisper-cli (bo-controller)\n");
        out.push_str("---\n\n");
    }
    if !title.is_empty() {
        out.push_str(&format!("# {}\n\n", title));
    }
    // Body with section breaks
    let mut last_end: u64 = 0;
    let mut first = true;
    for p in phrases {
        if !first && p.start_ms.saturating_sub(last_end) > SECTION_GAP_MS {
            out.push_str("\n---\n\n");
        }
        out.push_str(&p.text);
        out.push('\n');
        last_end = p.end_ms;
        first = false;
    }
    out
}

/// Cache-aware top-level entry. If a cached LRC exists for this
/// path, return it instantly. Otherwise run whisper, render to the
/// requested template, write to cache (always as LRC since that's
/// the most useful re-renderable format), and return the requested
/// rendering.
pub fn transcribe_with_cache(
    audio_path: &Path,
    template: Template,
    title: &str,
    artist: &str,
) -> Result<String, LyricsError> {
    let cache = cache_dir();
    let _ = std::fs::create_dir_all(&cache);
    let cache_file = cache.join(format!("{}.lrc", cache_key(audio_path)));

    // Read from cache if present — even if the user asked for a
    // different template, we can re-render from the cached LRC
    // because LRC carries the timing info.
    if cache_file.exists() {
        if let Ok(lrc) = std::fs::read_to_string(&cache_file) {
            // Parse the LRC back into phrases for re-rendering.
            // (Lightweight: lines starting with `[mm:ss.cc]`.)
            let phrases = parse_lrc(&lrc);
            return Ok(render(&phrases, template, title, artist));
        }
    }

    let result = transcribe(audio_path)?;
    // Always cache as LRC.
    let cached_lrc = render(&result.phrases, Template::Lrc, title, artist);
    let _ = std::fs::write(&cache_file, &cached_lrc);
    Ok(render(&result.phrases, template, title, artist))
}

/// Minimal LRC parser — just enough to round-trip our own output.
pub fn parse_lrc(lrc: &str) -> Vec<Phrase> {
    let mut out = Vec::new();
    for line in lrc.lines() {
        let line = line.trim();
        // Skip metadata tags [ar:...], [ti:...], [tool:...]
        if line.starts_with('[') && !line.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            continue;
        }
        // [mm:ss.cc]text
        if let Some(close) = line.find(']') {
            let ts = &line[1..close];
            let text = &line[close+1..];
            // Parse mm:ss.cc
            let mut parts = ts.split(':');
            let mm: u64 = match parts.next().and_then(|x| x.parse().ok()) { Some(v)=>v, None=>continue };
            let rest = match parts.next() { Some(v)=>v, None=>continue };
            let mut sp = rest.split('.');
            let ss: u64 = match sp.next().and_then(|x| x.parse().ok()) { Some(v)=>v, None=>continue };
            let cc: u64 = sp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let start_ms = mm * 60_000 + ss * 1_000 + cc * 10;
            // We don't have end times in LRC; estimate as +5s.
            out.push(Phrase {
                start_ms,
                end_ms: start_ms + 5_000,
                text: text.trim().to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_srt_with_comma_separator() {
        let srt = "1\n00:00:01,234 --> 00:00:04,500\nHello world\n\n\
                   2\n00:00:05,000 --> 00:00:07,250\nGoodbye world\n";
        let phrases = parse_srt(srt);
        assert_eq!(phrases.len(), 2);
        assert_eq!(phrases[0].start_ms, 1_234);
        assert_eq!(phrases[0].end_ms, 4_500);
        assert_eq!(phrases[0].text, "Hello world");
        assert_eq!(phrases[1].start_ms, 5_000);
    }

    #[test]
    fn parses_srt_with_dot_separator() {
        let srt = "1\n00:00:01.234 --> 00:00:04.500\nfoo\n";
        let phrases = parse_srt(srt);
        assert_eq!(phrases.len(), 1);
        assert_eq!(phrases[0].start_ms, 1_234);
    }

    #[test]
    fn lrc_render_includes_metadata_and_timing() {
        let phrases = vec![
            Phrase { start_ms: 1_234, end_ms: 4_500, text: "Hello world".into() },
            Phrase { start_ms: 65_120, end_ms: 70_000, text: "Goodbye".into() },
        ];
        let lrc = render_lrc(&phrases, "Demo Song", "Demo Artist");
        assert!(lrc.contains("[ti:Demo Song]"));
        assert!(lrc.contains("[ar:Demo Artist]"));
        assert!(lrc.contains("[00:01.23]Hello world"));
        assert!(lrc.contains("[01:05.12]Goodbye"));
    }

    #[test]
    fn markdown_inserts_section_break_on_big_gap() {
        let phrases = vec![
            Phrase { start_ms: 0,      end_ms: 2_000,  text: "Verse line 1".into() },
            Phrase { start_ms: 2_500,  end_ms: 4_500,  text: "Verse line 2".into() },
            // 6 second gap → section break
            Phrase { start_ms: 10_500, end_ms: 12_500, text: "Chorus line 1".into() },
        ];
        let md = render_markdown(&phrases, "Song", "Artist");
        // Section break renders as `---` between paragraphs
        assert!(md.contains("---\n\nChorus line 1"),
            "expected section break before chorus, got:\n{}", md);
        // No break between the two close-together verse lines
        let verse_section: String = md.lines()
            .skip_while(|l| !l.contains("Verse line 1"))
            .take(3)
            .collect::<Vec<_>>().join("\n");
        assert!(!verse_section.contains("---"),
            "no section break between adjacent verse lines, got:\n{}",
            verse_section);
    }

    #[test]
    fn lrc_round_trips_through_parse() {
        let original = vec![
            Phrase { start_ms: 1_500, end_ms: 4_500, text: "Hello".into() },
            Phrase { start_ms: 6_120, end_ms: 8_000, text: "World".into() },
        ];
        let lrc = render_lrc(&original, "", "");
        let parsed = parse_lrc(&lrc);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].start_ms, 1_500);
        assert_eq!(parsed[0].text, "Hello");
        assert_eq!(parsed[1].start_ms, 6_120);
    }

    #[test]
    fn plain_render_is_just_lines() {
        let phrases = vec![
            Phrase { start_ms: 0, end_ms: 1_000, text: "Line 1".into() },
            Phrase { start_ms: 2_000, end_ms: 3_000, text: "Line 2".into() },
        ];
        let out = render_plain(&phrases);
        assert_eq!(out, "Line 1\nLine 2");
    }

    #[test]
    fn empty_phrases_renders_empty_or_minimal() {
        let out = render_plain(&[]);
        assert!(out.is_empty());
        let out_lrc = render_lrc(&[], "", "");
        assert!(out_lrc.contains("[tool:"));   // tool header still there
    }
}
