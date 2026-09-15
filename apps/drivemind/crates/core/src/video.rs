// video — find footage worth re-encoding, and hand the batch to av1tuner.
//
// drivemind does NOT encode. av1tuner (apps/av1tuner) already owns the hard
// parts — scene detection, chunked parallel encode, quality targeting,
// merge — so the correct feature here is a good FEEDER: walk a tree, probe
// codecs, estimate savings, and emit a batch file av1tuner's discovery
// layer can consume (plain path list, one per line — the least clever
// format that both sides agree on).
//
// Candidacy, via ffprobe when available:
//   * legacy codec (mpeg2/mpeg4/wmv/mjpeg/vp8…)      → big AV1 win
//   * h264 at high bitrate (≥ 8 Mbps)                → solid AV1 win
//   * already av1/hevc/vp9                           → skip
// Without ffprobe we fall back to extension-only listing with a Note that
// codecs are unknown — visible degradation, not silent wrong answers.

use crate::{human_bytes, Event, Sink, SysRunner};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Mirrors av1tuner_core::discovery::VIDEO_EXTENSIONS (independent copy;
/// see deltahostile.rs for the rationale).
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "m4v", "mpg", "mpeg", "ts", "mts", "m2ts", "wmv", "flv",
    "ogv", "3gp", "vob", "y4m", "ivf",
];

const LEGACY_CODECS: &[&str] =
    &["mpeg2video", "mpeg4", "msmpeg4v3", "wmv1", "wmv2", "wmv3", "vc1", "mjpeg", "vp8", "h263"];
const MODERN_CODECS: &[&str] = &["av1", "hevc", "vp9"];
const H264_HIGH_BITRATE: i64 = 8_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoCandidate {
    pub path: String,
    pub size: u64,
    /// None = ffprobe unavailable or failed; candidacy is by extension only.
    pub codec: Option<String>,
    pub bit_rate: Option<i64>,
    pub why: String,
    /// Rough fraction of size an AV1 re-encode typically recovers.
    pub est_savings_ratio: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VideoScan {
    pub candidates: Vec<VideoCandidate>,
    pub skipped_modern: u64,
    pub total_candidate_bytes: u64,
}

impl VideoScan {
    pub fn est_savings_bytes(&self) -> u64 {
        self.candidates
            .iter()
            .map(|c| (c.size as f64 * c.est_savings_ratio as f64) as u64)
            .sum()
    }

    /// Write the batch file av1tuner ingests (one absolute path per line)
    /// and return the suggested handoff description.
    pub fn write_batch(&self, out: &Path) -> Result<String, String> {
        let body: String =
            self.candidates.iter().map(|c| format!("{}\n", c.path)).collect();
        fs::write(out, body).map_err(|e| format!("write batch: {e}"))?;
        Ok(format!(
            "av1tuner batch: {} file(s), {} — load '{}' in av1tuner (Add → From list), est. savings {}",
            self.candidates.len(),
            human_bytes(self.total_candidate_bytes),
            out.display(),
            human_bytes(self.est_savings_bytes())
        ))
    }
}

fn is_video(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn walk_videos(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&dir) {
            for ent in rd.flatten() {
                let p = ent.path();
                match ent.file_type() {
                    Ok(t) if t.is_dir() => stack.push(p),
                    Ok(t) if t.is_file() && is_video(&p) => out.push(p),
                    _ => {}
                }
            }
        }
    }
    out.sort();
    out
}

const FFPROBE_ARGS: &[&str] = &[
    "-v",
    "error",
    "-select_streams",
    "v:0",
    "-show_entries",
    "stream=codec_name,bit_rate:format=bit_rate",
    "-of",
    "json",
];

fn probe_codec(sys: &dyn SysRunner, path: &str) -> Option<(String, Option<i64>)> {
    let mut args: Vec<&str> = FFPROBE_ARGS.to_vec();
    args.push(path);
    let out = sys.run("ffprobe", &args).ok()?;
    let v: serde_json::Value = serde_json::from_str(&out.stdout).ok()?;
    let codec = v.pointer("/streams/0/codec_name")?.as_str()?.to_string();
    // Stream bit_rate is often absent (mkv); fall back to format bit_rate.
    let br = v
        .pointer("/streams/0/bit_rate")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| {
            v.pointer("/format/bit_rate")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<i64>().ok())
        });
    Some((codec, br))
}

/// Scan `root` for re-encode candidates.
pub fn scan(sys: &dyn SysRunner, root: &Path, sink: Sink) -> VideoScan {
    sink(Event::Phase { name: "video-scan", subject: root.display().to_string() });
    let mut scan = VideoScan::default();
    let files = walk_videos(root);
    let total = files.len() as u64;
    let mut ffprobe_ok = true;

    for (i, p) in files.iter().enumerate() {
        let path = p.display().to_string();
        let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let probed = if ffprobe_ok {
            let r = probe_codec(sys, &path);
            if r.is_none() && sys.run("ffprobe", &["-version"]).is_err() {
                ffprobe_ok = false;
                sink(Event::Note(
                    "ffprobe not available — listing by extension only, codecs unknown".into(),
                ));
            }
            r
        } else {
            None
        };

        let cand = match &probed {
            Some((codec, _)) if MODERN_CODECS.contains(&codec.as_str()) => {
                scan.skipped_modern += 1;
                None
            }
            Some((codec, br)) if LEGACY_CODECS.contains(&codec.as_str()) => Some(VideoCandidate {
                path,
                size,
                codec: Some(codec.clone()),
                bit_rate: *br,
                why: format!("legacy codec {codec}"),
                est_savings_ratio: 0.6,
            }),
            Some((codec, Some(br))) if codec == "h264" && *br >= H264_HIGH_BITRATE => {
                Some(VideoCandidate {
                    path,
                    size,
                    codec: Some(codec.clone()),
                    bit_rate: Some(*br),
                    why: format!("h264 at {:.1} Mbps", *br as f64 / 1e6),
                    est_savings_ratio: 0.45,
                })
            }
            Some(_) => None, // h264 at sane bitrate, or something exotic: leave it
            None => Some(VideoCandidate {
                path,
                size,
                codec: None,
                bit_rate: None,
                why: "video file (codec unprobed)".into(),
                est_savings_ratio: 0.3,
            }),
        };
        if let Some(c) = cand {
            scan.total_candidate_bytes += c.size;
            scan.candidates.push(c);
        }
        sink(Event::Progress { done: (i + 1) as u64, total: Some(total) });
    }
    scan.candidates.sort_by(|a, b| b.size.cmp(&a.size));
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{null_sink, FixtureRunner};

    fn probe_fixture(path: &str, json: &str) -> (String, String) {
        let mut args: Vec<&str> = FFPROBE_ARGS.to_vec();
        args.push(path);
        (format!("ffprobe {}", args.join(" ")), json.to_string())
    }

    #[test]
    fn classifies_legacy_modern_and_hot_h264() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        for (name, sz) in
            [("old.avi", 100usize), ("new.mkv", 100), ("cam.mp4", 100), ("note.txt", 10)]
        {
            fs::write(r.join(name), vec![0u8; sz]).unwrap();
        }
        let old = r.join("old.avi").display().to_string();
        let new = r.join("new.mkv").display().to_string();
        let cam = r.join("cam.mp4").display().to_string();

        let (k1, v1) =
            probe_fixture(&old, r#"{"streams":[{"codec_name":"mpeg4","bit_rate":"900000"}]}"#);
        let (k2, v2) = probe_fixture(&new, r#"{"streams":[{"codec_name":"av1"}],"format":{"bit_rate":"2000000"}}"#);
        let (k3, v3) =
            probe_fixture(&cam, r#"{"streams":[{"codec_name":"h264","bit_rate":"45000000"}]}"#);
        let sys = FixtureRunner::new().on(&k1, 0, &v1).on(&k2, 0, &v2).on(&k3, 0, &v3);

        let mut sink = null_sink();
        let s = scan(&sys, r, &mut sink);
        assert_eq!(s.skipped_modern, 1);
        let whys: Vec<&str> = s.candidates.iter().map(|c| c.why.as_str()).collect();
        assert_eq!(s.candidates.len(), 2, "{whys:?}");
        assert!(whys.iter().any(|w| w.contains("legacy codec mpeg4")));
        assert!(whys.iter().any(|w| w.contains("45.0 Mbps")));

        let batch = dir.path().join("batch.txt");
        let msg = s.write_batch(&batch).unwrap();
        assert!(msg.contains("av1tuner batch: 2"));
        let body = fs::read_to_string(&batch).unwrap();
        assert_eq!(body.lines().count(), 2);
        assert!(body.contains("old.avi"));
    }

    #[test]
    fn h264_at_a_sane_bitrate_is_left_alone_not_flagged() {
        // The `Some(_) => None` arm -- "h264 at sane bitrate... leave it"
        // per its own comment -- had zero test coverage. Every existing
        // h264 fixture was deliberately above the 8 Mbps threshold; a
        // regression that flagged EVERY h264 file regardless of bitrate
        // would not have been caught by any test before this one.
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::write(r.join("normal.mp4"), vec![0u8; 100]).unwrap();
        let path = r.join("normal.mp4").display().to_string();
        let (k, v) = probe_fixture(&path, r#"{"streams":[{"codec_name":"h264","bit_rate":"3000000"}]}"#);
        let sys = FixtureRunner::new().on(&k, 0, &v);
        let mut sink = null_sink();
        let s = scan(&sys, r, &mut sink);
        assert!(s.candidates.is_empty(), "ordinary h264 must not be suggested for re-encode: {:?}", s.candidates);
        assert_eq!(s.skipped_modern, 0, "h264 is not a MODERN codec either -- it's neither skip-as-modern nor flag-as-legacy");
    }

    #[test]
    fn h264_bit_rate_exactly_at_the_threshold_counts_high() {
        // `*br >= H264_HIGH_BITRATE` -- the boundary itself. The existing
        // fixture used 45 Mbps, comfortably clear of 8 Mbps either way;
        // a `>` vs `>=` mistake would not be caught without a value
        // planted exactly on the line.
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::write(r.join("edge.mp4"), vec![0u8; 100]).unwrap();
        let path = r.join("edge.mp4").display().to_string();
        let (k, v) = probe_fixture(&path, r#"{"streams":[{"codec_name":"h264","bit_rate":"8000000"}]}"#);
        let sys = FixtureRunner::new().on(&k, 0, &v);
        let mut sink = null_sink();
        let s = scan(&sys, r, &mut sink);
        assert_eq!(s.candidates.len(), 1, "exactly 8,000,000 bps must count as high bitrate");
    }

    #[test]
    fn missing_ffprobe_falls_back_to_extension_only_listing_with_a_visible_note() {
        // The module's own header states the design goal precisely:
        // "Without ffprobe we fall back to extension-only listing with a
        // Note that codecs are unknown -- visible degradation, not
        // silent wrong answers." This had NO test at all -- every
        // existing fixture answered every ffprobe call, so the "ffprobe
        // is not installed on this machine" path (a real, common case on
        // a minimal postmarketOS image) was never exercised.
        struct NoFfprobe;
        impl SysRunner for NoFfprobe {
            fn run(&self, program: &str, _: &[&str]) -> Result<crate::SysOutput, String> {
                if program == "ffprobe" {
                    Err("ffprobe: command not found".into())
                } else {
                    Err("unexpected command in this fixture".into())
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::write(r.join("mystery.mp4"), vec![0u8; 1234]).unwrap();

        let mut notes: Vec<String> = Vec::new();
        {
            let mut sink = |e: Event| {
                if let Event::Note(n) = e { notes.push(n); }
            };
            let s = scan(&NoFfprobe, r, &mut sink);
            assert_eq!(s.candidates.len(), 1, "the file must still be listed by extension alone");
            assert_eq!(s.candidates[0].codec, None, "codec must be honestly unknown, not guessed");
            assert_eq!(s.candidates[0].why, "video file (codec unprobed)");
        }
        assert!(notes.iter().any(|n| n.contains("ffprobe not available")),
            "the degradation must be VISIBLE, per this module's own stated design goal: {notes:?}");
    }
}
