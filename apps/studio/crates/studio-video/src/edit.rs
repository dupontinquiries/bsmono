//! edit — the timeline's picture, rendered through ffmpeg's edit list.
//!
//! The arrangement already renders to audio; this is the other half of
//! a music video: every clip on a video track, in timeline order, cut
//! from its source file and concatenated into one picture, with the
//! mixdown underneath. No frame is decoded here — the cut list is pure
//! timeline arithmetic ([`edit_list`]), the command is a string
//! ([`render_args`]), and ffmpeg does the rest, which is the crate's
//! whole stance (see its module docs).
//!
//! ## The rule for two clips at once
//!
//! Video tracks stack: the FIRST video track in the project is on top,
//! and where two clips overlap in time the top one is the picture. The
//! same as every editor's track order, and the same as this app's
//! arrangement paints them. A gap where no video track has a clip is
//! black for that long — the timing of the audio must hold, so a gap is
//! a segment, not a skip.
//!
//! ## What ffmpeg is asked
//!
//! One `-i` per distinct source file, one `-i` for the mixdown WAV, and
//! a `filter_complex` that trims each cut out of its input (`trim` +
//! `setpts`), scales and pads every cut to one frame size (sources of
//! different sizes concatenate only if they agree), generates the black
//! gaps (`color`), and concatenates them all in order. `-shortest` so
//! the picture ends with the audio and never a frame past it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use studio_core::{Project, SourceId, Ticks, TrackKind};

/// One piece of the picture: `len` of `path` from `src_in`, placed at
/// `at` on the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    pub path: PathBuf,
    pub src_in: Ticks,
    pub len: Ticks,
    pub at: Ticks,
}

/// A segment of the output: a cut, or black.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Cut(Cut),
    Black { at: Ticks, len: Ticks },
}

impl Segment {
    pub fn at(&self) -> Ticks { match self { Segment::Cut(c) => c.at, Segment::Black { at, .. } => *at } }
    pub fn len(&self) -> Ticks { match self { Segment::Cut(c) => c.len, Segment::Black { len, .. } => *len } }
    pub fn end(&self) -> Ticks { self.at().saturating_add(self.len()) }
}

/// The picture as segments covering `[0, end)` — `end` is the project's
/// duration unless given — with no gaps: black where no video clip is.
/// Only clips whose source has a path in `paths` and sits on a video
/// track count; a video source whose file has not arrived is a gap.
pub fn edit_list(project: &Project, paths: &BTreeMap<SourceId, PathBuf>, end: Option<Ticks>) -> Vec<Segment> {
    let end = end.unwrap_or_else(|| project.duration());
    // Video clips in priority order: track index ascending (first is top).
    let mut clips: Vec<(usize, &studio_core::Clip)> = project.clips.values()
        .filter_map(|c| {
            let ti = project.tracks.iter().position(|t| t.id == c.track)?;
            if project.tracks[ti].kind != TrackKind::Video { return None; }
            if !paths.contains_key(&c.source) { return None; }
            Some((ti, c))
        })
        .collect();
    clips.sort_by_key(|(ti, c)| (*ti, c.pos));
    // Sweep: each clip claims whatever of its span is not yet covered.
    let mut covered: Vec<(Ticks, Ticks)> = Vec::new();
    let mut cuts: Vec<Cut> = Vec::new();
    for (_, c) in clips {
        let (mut from, to) = (c.pos, c.end().min(end));
        if from >= to { continue; }
        // Walk the uncovered pieces of [from, to).
        let mut pieces = Vec::new();
        let mut occupied: Vec<(Ticks, Ticks)> = covered.iter().copied()
            .filter(|(s, e)| *s < to && *e > from).collect();
        occupied.sort();
        for (s, e) in occupied {
            if s > from { pieces.push((from, s)); }
            from = from.max(e);
        }
        if from < to { pieces.push((from, to)); }
        for (s, e) in pieces {
            cuts.push(Cut {
                path: paths[&c.source].clone(),
                src_in: c.src_in.saturating_add(s.saturating_sub(c.pos)),
                len: e.saturating_sub(s),
                at: s,
            });
            covered.push((s, e));
        }
    }
    cuts.sort_by_key(|c| c.at);
    // Fill the gaps with black.
    let mut out = Vec::new();
    let mut t = Ticks::ZERO;
    for c in cuts {
        if c.at > t { out.push(Segment::Black { at: t, len: c.at.saturating_sub(t) }); }
        t = c.at.saturating_add(c.len);
        out.push(Segment::Cut(c));
    }
    if end > t { out.push(Segment::Black { at: t, len: end.saturating_sub(t) }); }
    out
}

/// The ffmpeg arguments that render `segments` at `w`×`h`, `fps`, with
/// `audio` (a WAV of the mixdown) underneath, to `out`. Pure, so the
/// command is testable without running it.
pub fn render_args(segments: &[Segment], audio: Option<&Path>, out: &Path, w: u32, h: u32, fps: u32) -> Vec<String> {
    let mut args: Vec<String> = vec!["-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into()];
    // One input per distinct file, in first-use order.
    let mut inputs: Vec<PathBuf> = Vec::new();
    for s in segments {
        if let Segment::Cut(c) = s {
            if !inputs.contains(&c.path) { inputs.push(c.path.clone()); }
        }
    }
    for p in &inputs { args.push("-i".into()); args.push(p.to_string_lossy().into_owned()); }
    let audio_index = inputs.len();
    if let Some(a) = audio { args.push("-i".into()); args.push(a.to_string_lossy().into_owned()); }
    let fit = format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format=yuv420p");
    let mut filter = String::new();
    let mut labels = Vec::new();
    for (i, s) in segments.iter().enumerate() {
        let label = format!("[s{i}]");
        match s {
            Segment::Cut(c) => {
                let input = inputs.iter().position(|p| *p == c.path).expect("collected above");
                filter.push_str(&format!(
                    "[{input}:v]trim=start={:.6}:duration={:.6},setpts=PTS-STARTPTS,{fit}{label};",
                    c.src_in.to_seconds(), c.len.to_seconds()));
            }
            Segment::Black { len, .. } => {
                filter.push_str(&format!(
                    "color=c=black:s={w}x{h}:r={fps}:d={:.6},format=yuv420p{label};", len.to_seconds()));
            }
        }
        labels.push(label);
    }
    filter.push_str(&format!("{}concat=n={}:v=1:a=0[vout]", labels.concat(), segments.len()));
    args.push("-filter_complex".into());
    args.push(filter);
    args.push("-map".into()); args.push("[vout]".into());
    if audio.is_some() {
        args.push("-map".into()); args.push(format!("{audio_index}:a"));
        args.push("-c:a".into()); args.push("aac".into());
        args.push("-shortest".into());
    }
    args.extend(["-c:v".into(), "libx264".into(), "-pix_fmt".into(), "yuv420p".into(),
                 "-movflags".into(), "+faststart".into()]);
    args.push(out.to_string_lossy().into_owned());
    args
}

/// Render `segments` with `audio` under it to `out`. `segments` must be
/// non-empty (a project with no duration has no picture to render).
pub fn render(segments: &[Segment], audio: Option<&Path>, out: &Path, w: u32, h: u32, fps: u32)
    -> Result<(), super::VideoError>
{
    if segments.is_empty() {
        return Err(super::VideoError::UnexpectedOutput("edit list is empty: nothing on the timeline".into()));
    }
    let args = render_args(segments, audio, out, w, h, fps);
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    super::run("ffmpeg", &refs).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, Track, TrackId};

    fn project_with(paths: &BTreeMap<SourceId, PathBuf>) -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video, name: "cam A".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(2), kind: TrackKind::Video, name: "cam B".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 1 }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(3), kind: TrackKind::Audio, name: "vox".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 2 }).unwrap();
        for id in paths.keys() {
            p.apply(&Op::AddSource { source: Source { id: *id, path: paths[id].to_string_lossy().into_owned(),
                len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 1, has_video: true } }).unwrap();
        }
        p
    }
    fn clip(p: &mut Project, id: u64, source: u64, track: u64, pos: f64, len: f64, src_in: f64) {
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(source), track: TrackId(track),
            pos: Ticks::from_seconds(pos), src_in: Ticks::from_seconds(src_in), len: Ticks::from_seconds(len),
            gain: 1.0, effects: vec![] } }).unwrap();
    }
    fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }

    #[test]
    fn the_top_track_wins_an_overlap_and_gaps_are_black() {
        let paths: BTreeMap<SourceId, PathBuf> =
            [(SourceId(1), "a.mp4".into()), (SourceId(2), "b.mp4".into()), (SourceId(3), "c.wav".into())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 2.0, 0.0);   // cam A: 0–2 from a.mp4
        clip(&mut p, 2, 2, 2, 1.0, 3.0, 0.5);   // cam B: 1–4 from b.mp4 (from 0.5 in)
        clip(&mut p, 3, 2, 2, 5.0, 1.0, 0.0);   // cam B: 5–6
        clip(&mut p, 4, 3, 3, 0.0, 7.0, 0.0);   // audio: not picture
        let list = edit_list(&p, &paths, None);
        let shape: Vec<(f64, f64, Option<(&str, f64)>)> = list.iter().map(|seg| match seg {
            Segment::Cut(c) => (c.at.to_seconds(), c.len.to_seconds(),
                                Some((c.path.to_str().unwrap(), c.src_in.to_seconds()))),
            Segment::Black { at, len } => (at.to_seconds(), len.to_seconds(), None),
        }).collect();
        // A wins 0–2 outright (top track), B shows 2–4 from 1.5 s in
        // (its own 0.5 plus the second it lost), black 4–5, B 5–6, black
        // to the project's end at 7 (the audio clip's).
        let want: Vec<(f64, f64, Option<(&str, f64)>)> = vec![
            (0.0, 2.0, Some(("a.mp4", 0.0))),
            (2.0, 2.0, Some(("b.mp4", 1.5))),
            (4.0, 1.0, None),
            (5.0, 1.0, Some(("b.mp4", 0.0))),
            (6.0, 1.0, None),
        ];
        for (g, w) in shape.iter().zip(&want) {
            assert!((g.0 - w.0).abs() < 1e-6 && (g.1 - w.1).abs() < 1e-6, "{shape:?}");
            match (&g.2, &w.2) {
                (Some((gp, gi)), Some((wp, wi))) => { assert_eq!(gp, wp); assert!((gi - wi).abs() < 1e-6, "{shape:?}"); }
                (None, None) => {}
                _ => panic!("{shape:?}"),
            }
        }
        assert_eq!(shape.len(), want.len(), "{shape:?}");
        // Contiguous: every segment starts where the last ended.
        for w in list.windows(2) { assert_eq!(w[0].end(), w[1].at()); }
        // A source with no file yet is a gap, not a crash.
        let fewer: BTreeMap<SourceId, PathBuf> = [(SourceId(2), "b.mp4".into())].into();
        let list = edit_list(&p, &fewer, Some(s(4.0)));
        assert!(matches!(list[0], Segment::Black { .. }), "cam A's file is not here: black where it was");
        assert!(matches!(&list[1], Segment::Cut(c) if c.at == s(1.0) && c.len == s(3.0)), "{list:?}");
        assert!(edit_list(&Project::default(), &paths, None).is_empty());
    }

    #[test]
    fn the_command_names_each_file_once_and_concatenates_in_order() {
        let a: PathBuf = "a.mp4".into();
        let segs = vec![
            Segment::Cut(Cut { path: a.clone(), src_in: s(0.0), len: s(1.0), at: s(0.0) }),
            Segment::Black { at: s(1.0), len: s(0.5) },
            Segment::Cut(Cut { path: a.clone(), src_in: s(3.0), len: s(1.0), at: s(1.5) }),
        ];
        let args = render_args(&segs, Some(Path::new("mix.wav")), Path::new("out.mp4"), 640, 360, 25);
        let inputs: Vec<&String> = args.windows(2).filter(|w| w[0] == "-i").map(|w| &w[1]).collect();
        assert_eq!(inputs, vec!["a.mp4", "mix.wav"], "a.mp4 once, though it is cut twice; the audio last");
        let fc = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(fc.contains("[0:v]trim=start=0.000000:duration=1.000000"), "{fc}");
        assert!(fc.contains("[0:v]trim=start=3.000000:duration=1.000000"), "{fc}");
        assert!(fc.contains("color=c=black:s=640x360:r=25:d=0.500000"), "{fc}");
        assert!(fc.ends_with("[s0][s1][s2]concat=n=3:v=1:a=0[vout]"), "{fc}");
        assert!(args.windows(2).any(|w| w[0] == "-map" && w[1] == "1:a"), "the audio is input 1");
        assert!(args.contains(&"-shortest".to_string()));
        assert_eq!(args.last().unwrap(), "out.mp4");
        let silent = render_args(&segs, None, Path::new("out.mp4"), 640, 360, 25);
        assert!(!silent.contains(&"-shortest".to_string()) && !silent.iter().any(|a| a == "mix.wav"));
    }

    #[test]
    fn a_wav_transcodes_to_mp3_and_m4a_that_probe_as_audio_of_the_same_length() {
        if !super::super::tests::has_ffmpeg() { return; }
        let enc = super::super::run("ffmpeg", &["-hide_banner", "-encoders"]).unwrap();
        let enc = String::from_utf8_lossy(&enc);
        let tmp = std::env::temp_dir();
        let wav = tmp.join(format!("studio-tc-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..96_000).map(|i| (i as f32 * 0.03).sin() * 0.3).collect();
        std::fs::write(&wav, studio_audio::export::export_wav_dithered_bytes(&tone, 2, 48_000, 1).unwrap()).unwrap();
        for (codec, ext) in [("libmp3lame", "mp3"), ("aac", "m4a")] {
            if !enc.contains(&format!(" {codec} ")) { continue; }
            let out = tmp.join(format!("studio-tc-{}.{ext}", std::process::id()));
            super::super::transcode_audio(&wav, &out, codec, 160_000).expect(codec);
            // `probe` is a video probe (an audio-only file is NoVideoStream
            // to it, correctly); the app's decoder is the reader here.
            let back = studio_media::decode_file(&out).expect("studio-media reads it back");
            assert_eq!(back.channels.len(), 2, "{ext}: stereo");
            let secs = back.frames() as f64 / back.sample_rate as f64;
            assert!((secs - 1.0).abs() < 0.15, "{ext}: a second, got {secs}");
            let size = std::fs::metadata(&out).unwrap().len();
            assert!(size > 5_000 && size < 60_000, "{ext}: 160 kbps of a second is ~20 KB, got {size}");
            let _ = std::fs::remove_file(&out);
        }
        assert!(super::super::transcode_audio(&wav, &tmp.join("x.bin"), "no-such-codec", 1000).is_err(),
                "a codec ffmpeg lacks fails, with its stderr");
        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn a_two_camera_cut_renders_and_the_frames_are_the_right_cameras() {
        if !super::super::tests::has_ffmpeg() { return; }
        // Two one-second fixtures of solid colour, no audio.
        let fixture = |colour: &str| -> PathBuf {
            let path = std::env::temp_dir().join(format!("studio-edit-{colour}-{}-{}.mp4", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
            let src = format!("color=c={colour}:s=64x48:r=10:d=1");
            super::super::run("ffmpeg", &["-y", "-f", "lavfi", "-i", &src, "-c:v", "libx264", "-pix_fmt", "yuv420p",
                "-an", path.to_str().unwrap()]).expect("fixture");
            path
        };
        let red = fixture("red");
        let blue = fixture("blue");
        let paths: BTreeMap<SourceId, PathBuf> = [(SourceId(1), red.clone()), (SourceId(2), blue.clone())].into();
        let mut p = project_with(&paths);
        clip(&mut p, 1, 1, 1, 0.0, 0.5, 0.0);   // red 0–0.5
        clip(&mut p, 2, 2, 2, 0.5, 0.5, 0.2);   // blue 0.5–1.0
        clip(&mut p, 3, 1, 1, 1.5, 0.5, 0.3);   // red 1.5–2.0, black 1.0–1.5 between
        let list = edit_list(&p, &paths, None);
        assert_eq!(list.len(), 4);
        let out = std::env::temp_dir().join(format!("studio-edit-out-{}.mp4", std::process::id()));
        // A mixdown to go under it: 2 s of a quiet tone, as WAV.
        let wav = std::env::temp_dir().join(format!("studio-edit-mix-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..96_000).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        std::fs::write(&wav, studio_audio::export::export_wav_dithered_bytes(&tone, 1, 48_000, 1).unwrap()).unwrap();
        render(&list, Some(&wav), &out, 64, 48, 10).expect("render");
        let probe = super::super::probe(&out).expect("probe the render");
        assert!((probe.duration.to_seconds() - 2.0).abs() < 0.15, "two seconds of picture: {:?}", probe.duration);
        assert!(probe.has_audio, "the mixdown is under it");
        let colour_at = |t: f64| -> (u8, u8, u8) {
            let px = super::super::frame_rgb(&out, s(t), 8, 6).unwrap();
            // The centre pixel of an 8×6 frame.
            let i = (3 * 8 + 4) * 3;
            (px[i], px[i + 1], px[i + 2])
        };
        let is = |c: (u8, u8, u8), r: bool, g: bool, b: bool| {
            (c.0 > 128) == r && (c.1 > 128) == g && (c.2 > 128) == b
        };
        assert!(is(colour_at(0.25), true, false, false), "0.25 s: red, got {:?}", colour_at(0.25));
        assert!(is(colour_at(0.75), false, false, true), "0.75 s: blue, got {:?}", colour_at(0.75));
        assert!(is(colour_at(1.25), false, false, false), "1.25 s: black, got {:?}", colour_at(1.25));
        assert!(is(colour_at(1.75), true, false, false), "1.75 s: red again, got {:?}", colour_at(1.75));
        for f in [red, blue, out, wav] { let _ = std::fs::remove_file(f); }
    }
}
