//! Export chosen sections to one file, without re-encoding.
//!
//! One ffmpeg process, the concat demuxer, `inpoint`/`outpoint` per span, and
//! `-c copy`. Exporting three chapters of an hour-long lecture reads those
//! three chapters' packets and writes them; nothing is decoded, so it runs at
//! disk speed and costs the phone next to nothing.
//!
//! # The trade, stated
//!
//! Stream copy can only cut at keyframes. Each span STARTS at the keyframe at
//! or before its inpoint, so an export carries up to one GOP (typically 2-5 s
//! on YouTube uploads) of extra lead-in per span. For "keep the chapters I
//! care about" that is the right trade. Frame-accurate cuts need a re-encode
//! of at least the first GOP of each span -- hardware encoders
//! (`h264_vaapi`, `h264_v4l2m2m`) would make that cheap, and it is not built.
//!
//! Matroska output: it takes any codec pair a stream copy might bring (VP9 +
//! Opus from YouTube, H.264 + AAC from a phone camera) where MP4 refuses some.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Quote a path for an ffconcat `file` line: single quotes, with `'` written
/// as `'\''` -- the same rule as a POSIX shell, which is what ffmpeg follows.
fn quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

/// The ffconcat script: the same input once per span. Pure.
pub fn ffconcat(input: &Path, spans: &[(Duration, Duration)]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    for &(a, b) in spans.iter().filter(|(a, b)| a < b) {
        s.push_str(&format!(
            "file {}\ninpoint {:.3}\noutpoint {:.3}\n",
            quote(input),
            a.as_secs_f64(),
            b.as_secs_f64()
        ));
    }
    s
}

/// ffmpeg arguments for the export. Pure.
pub fn export_args(list: &Path, out: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-v", "error", "-nostdin", "-y", "-f", "concat", "-safe", "0", "-i"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.push(list.to_string_lossy().into_owned());
    // Video and audio if present. `-map 0` would also take data and timecode
    // tracks, which Matroska cannot hold, and the export would fail at the
    // end of the work rather than the start.
    for s in ["-map", "0:v?", "-map", "0:a?", "-c", "copy", "-avoid_negative_ts", "make_zero"] {
        a.push(s.into());
    }
    a.push(out.to_string_lossy().into_owned());
    a
}

/// `<stem>.sections.mkv` beside the input, numbered if that exists.
pub fn default_output(input: &Path) -> PathBuf {
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let mut p = dir.join(format!("{stem}.sections.mkv"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}.sections-{n}.mkv"));
        n += 1;
    }
    p
}

/// Run the export. Blocking; the app calls it on its own thread.
pub fn export(input: &Path, spans: &[(Duration, Duration)], out: &Path) -> Result<(), String> {
    if spans.iter().all(|(a, b)| a >= b) {
        return Err("nothing to export".into());
    }
    let list = std::env::temp_dir().join(format!("vidkit-export-{}.ffconcat", std::process::id()));
    std::fs::write(&list, ffconcat(input, spans)).map_err(|e| format!("{}: {e}", list.display()))?;
    let res = Command::new("ffmpeg")
        .args(export_args(&list, out))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let _ = std::fs::remove_file(&list);
    let res = res.map_err(|e| format!("ffmpeg: {e}"))?;
    if !res.status.success() {
        let _ = std::fs::remove_file(out);
        return Err(format!(
            "ffmpeg: {}",
            String::from_utf8_lossy(&res.stderr).lines().last().unwrap_or("export failed")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn the_script_names_the_input_once_per_span_and_skips_empty_spans() {
        let t = ffconcat(Path::new("/v/it's.mp4"), &[(s(2), s(4)), (s(9), s(9)), (s(6), s(8))]);
        assert!(t.starts_with("ffconcat version 1.0\n"));
        assert_eq!(t.matches("file ").count(), 2, "{t}");
        assert!(t.contains("file '/v/it'\\''s.mp4'"), "quote escaped: {t}");
        assert!(t.contains("inpoint 2.000\noutpoint 4.000"));
        assert!(t.contains("inpoint 6.000\noutpoint 8.000"));
    }

    #[test]
    fn it_copies_rather_than_encodes() {
        let a = export_args(Path::new("l"), Path::new("o.mkv"));
        assert!(a.windows(2).any(|w| w[0] == "-c" && w[1] == "copy"), "{a:?}");
        assert!(!a.iter().any(|x| x.starts_with("lib")), "an encoder crept in: {a:?}");
        assert_eq!(a.last().unwrap(), "o.mkv");
    }

    fn probe(p: &Path, what: &str) -> String {
        let o = Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", what, "-of", "default=noprint_wrappers=1:nokey=1"])
            .arg(p)
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// Real ffmpeg: two spans of a clip with 1 s keyframes. The result has
    /// video and audio, lasts the sum of the spans plus at most a GOP of
    /// lead-in each, and is not the whole clip.
    #[test]
    fn two_sections_become_one_file_of_about_their_length() {
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| !s.success()).unwrap_or(true) { return; }
        let dir = std::env::temp_dir().join(format!("vk_exp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("lecture.mp4");
        assert!(Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10:duration=12",
                   "-f", "lavfi", "-i", "sine=duration=12", "-c:v", "libx264", "-g", "10", "-pix_fmt", "yuv420p",
                   "-c:a", "aac", "-shortest"])
            .arg(&input).status().unwrap().success());
        let out = default_output(&input);
        assert_eq!(out.file_name().unwrap(), "lecture.sections.mkv");
        let spans = [(s(2), s(4)), (s(7), s(9))];
        let began = std::time::Instant::now();
        export(&input, &spans, &out).expect("export");
        let took = began.elapsed();
        let dur: f64 = probe(&out, "format=duration").parse().unwrap();
        eprintln!("DIAG export {dur:.2} s in {took:?}");
        assert!((3.8..=6.2).contains(&dur), "4 s of spans, at most 1 s lead-in each: got {dur}");
        let kinds = probe(&out, "stream=codec_type");
        assert!(kinds.contains("video") && kinds.contains("audio"), "{kinds}");
        assert_eq!(default_output(&input).file_name().unwrap(), "lecture.sections-2.mkv", "no overwrite");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_to_export_is_an_error_not_an_empty_file() {
        let e = export(Path::new("/nonexistent.mp4"), &[(s(3), s(3))], Path::new("/tmp/never.mkv")).unwrap_err();
        assert!(e.contains("nothing"), "{e}");
    }
}
