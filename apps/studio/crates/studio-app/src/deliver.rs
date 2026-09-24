//! deliver — "where is this going?" answered once, and everything that
//! follows from it planned and checked before a frame is rendered.
//!
//! `export` renders what it is told. Delivery is the layer above: pick a
//! DESTINATION and the plan follows -- picture size and fit, loudness target,
//! captions, chapters -- as a list of [`Step`]s the person can read, plus a
//! PREFLIGHT of [`Finding`]s: what would make the file wrong, said before an
//! hour of rendering rather than after. A [`Severity::Block`] finding stops
//! the run; the rest are reasons to look.
//!
//! Pure: a project, the transcripts and a destination in, a [`Delivery`] out.
//! The run (render through `export`, measure, gain, write sidecars) is the
//! shell's job and uses this plan as its script.
//!
//! ## What the transcripts buy
//!
//! * **Captions** ([`captions`]): every audible clip's lines moved to timeline
//!   time and clipped to what the clip plays -- the SAME line said by several
//!   angles or a camera and the recorder at once is captioned once.
//! * **Chapters** ([`chapters`]): the timeline's markers, minus the working
//!   ones (`review …` from the assembly), as the "0:00 Title" list a video
//!   description or a podcast feed takes.
//!
//! And the assembly's review markers are checked for: delivering a first cut
//! with its to-do list still on it is a warning, not a silent success.

use std::collections::BTreeMap;
use std::path::Path;

use studio_core::{Project, SourceId, Ticks, TrackKind};
use studio_voice::Segment;

/// Where the file is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// A video site: 16:9 1080p, -14 LUFS, captions and chapters.
    Web,
    /// Phone-first short video: 9:16 1080x1920, filled (cropped), -14 LUFS,
    /// captions (most watch muted).
    Vertical,
    /// Spoken audio: -16 LUFS (the podcast norm), chapters, no picture.
    Podcast,
    /// For mastering or archive: the mix as it is, no loudness change, WAV.
    Master,
}

impl Destination {
    pub const ALL: [Destination; 4] = [Destination::Web, Destination::Vertical, Destination::Podcast, Destination::Master];
    pub fn name(self) -> &'static str {
        match self { Destination::Web => "web video", Destination::Vertical => "vertical short",
                     Destination::Podcast => "podcast", Destination::Master => "master" }
    }
    /// `(width, height, fps, fill)`: `fill` crops to fill the frame (Cover)
    /// instead of letterboxing (Contain).
    pub fn picture(self) -> Option<(u32, u32, u32, bool)> {
        match self { Destination::Web => Some((1920, 1080, 30, false)), Destination::Vertical => Some((1080, 1920, 30, true)),
                     _ => None }
    }
    /// `(target LUFS, true-peak ceiling dBTP)`, or `None` to leave the mix alone.
    pub fn loudness(self) -> Option<(f32, f32)> {
        match self { Destination::Web | Destination::Vertical => Some((-14.0, -1.0)), Destination::Podcast => Some((-16.0, -1.0)),
                     Destination::Master => None }
    }
    pub fn captions(self) -> bool { matches!(self, Destination::Web | Destination::Vertical) }
    pub fn chapters(self) -> bool { matches!(self, Destination::Web | Destination::Podcast) }
}

/// One thing the run will do, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Mixdown { from: Ticks, to: Ticks },
    Loudness { target_lufs: f32, ceiling_dbtp: f32 },
    Picture { width: u32, height: u32, fps: u32, fill: bool },
    Captions { cues: usize },
    Chapters { count: usize },
}

impl Step {
    pub fn describe(&self) -> String {
        match self {
            Step::Mixdown { from, to } => format!("mix {:.1}–{:.1} s", from.to_seconds(), to.to_seconds()),
            Step::Loudness { target_lufs, ceiling_dbtp } => format!("measure, then gain to {target_lufs} LUFS (peaks under {ceiling_dbtp} dBTP)"),
            Step::Picture { width, height, fps, fill } => format!("render picture {width}x{height} at {fps} fps, {}",
                                                                   if *fill { "filled (edges cropped)" } else { "fitted (bars if needed)" }),
            Step::Captions { cues } => format!("captions: {cues} cue{} as .srt beside the file", if *cues == 1 { "" } else { "s" }),
            Step::Chapters { count } => format!("chapters: {count} as a .txt beside the file"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity { Note, Warn, Block }

#[derive(Debug, Clone, PartialEq)]
pub struct Finding { pub severity: Severity, pub text: String }

/// A caption cue on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue { pub from: Ticks, pub to: Ticks, pub text: String }

/// The plan for one destination.
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub destination: Destination,
    pub steps: Vec<Step>,
    pub findings: Vec<Finding>,
    pub captions: Vec<Cue>,
    pub chapters: Vec<(Ticks, String)>,
}

impl Delivery {
    pub fn blocked(&self) -> bool { self.findings.iter().any(|f| f.severity == Severity::Block) }
}

/// Is this marker working state rather than a chapter? The assembly's
/// reviews, and the signals panel's "why" notes.
pub fn is_working_marker(name: &str) -> bool {
    name.starts_with("review ") || name.starts_with("why ")
}

/// Every audible clip's transcript lines on the timeline, in time order,
/// clipped to what each clip plays; a line already captioned at an
/// overlapping time (another angle, the recorder) is not captioned again.
pub fn captions(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>, to: Ticks) -> Vec<Cue> {
    let mut raw: Vec<Cue> = Vec::new();
    for t in project.tracks.iter().filter(|t| !t.muted) {
        for c in project.track_clips(t.id) {
            let Some(segs) = words.get(&c.source) else { continue };
            let src_end = c.src_in.saturating_add(c.len);
            for s in segs {
                if s.end <= c.src_in || s.start >= src_end { continue; }
                if studio_video::spoken_slate::hallucination(&s.text).is_some() { continue; }
                let from = c.pos.saturating_add(s.start.max(c.src_in).saturating_sub(c.src_in));
                let end = c.pos.saturating_add(s.end.min(src_end).saturating_sub(c.src_in)).min(to);
                if end > from { raw.push(Cue { from, to: end, text: s.text.trim().to_string() }); }
            }
        }
    }
    raw.sort_by_key(|c| (c.from, c.to));
    let mut out: Vec<Cue> = Vec::new();
    for c in raw {
        let dup = out.iter().any(|o| o.from < c.to && c.from < o.to && studio_assemble::similarity(&o.text, &c.text) >= 0.8);
        if !dup { out.push(c); }
    }
    out
}

/// SubRip text for `cues`.
pub fn to_srt(cues: &[Cue]) -> String {
    let ts = |t: Ticks| {
        let ms = (t.to_seconds() * 1000.0).round().max(0.0) as u64;
        format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    };
    cues.iter().enumerate().map(|(i, c)| format!("{}\n{} --> {}\n{}\n", i + 1, ts(c.from), ts(c.to), c.text))
        .collect::<Vec<_>>().join("\n")
}

/// Chapters from the markers (working markers left out). Video sites want
/// the first at 0:00, so one is added ("Start") when the first marker is
/// later -- said in the title so nobody wonders where it came from.
pub fn chapters(project: &Project) -> Vec<(Ticks, String)> {
    let mut v: Vec<(Ticks, String)> = project.markers.iter().filter(|m| !is_working_marker(&m.name))
        .map(|m| (m.at, m.name.clone())).collect();
    v.sort_by_key(|(t, _)| *t);
    if v.first().is_some_and(|(t, _)| *t > Ticks::ZERO) { v.insert(0, (Ticks::ZERO, "Start".into())); }
    v
}

/// "0:00 Title" lines.
pub fn chapters_text(ch: &[(Ticks, String)]) -> String {
    ch.iter().map(|(t, n)| { let s = t.to_seconds().max(0.0) as u64; format!("{}:{:02} {n}", s / 60, s % 60) })
        .collect::<Vec<_>>().join("\n")
}

/// The plan and the preflight. `base` resolves the project's relative media
/// paths (the project file's folder).
pub fn plan(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>, dest: Destination, base: &Path) -> Delivery {
    let mut f: Vec<Finding> = Vec::new();
    let mut say = |severity, text: String| f.push(Finding { severity, text });
    let end = project.duration();
    if end == Ticks::ZERO { say(Severity::Block, "the timeline is empty".into()); }
    // media that is not where the project says
    let used: Vec<SourceId> = project.clips.values().map(|c| c.source).collect();
    let missing: Vec<String> = project.sources.values()
        .filter(|s| used.contains(&s.id) && !s.is_generator() && !base.join(&s.path).is_file())
        .map(|s| s.path.clone()).collect();
    if !missing.is_empty() {
        say(Severity::Warn, format!("{} file{} not found -- silent/black where used: {}", missing.len(),
            if missing.len() == 1 { "" } else { "s" }, missing.join(", ")));
    }
    let has_video = project.clips.values().any(|c| project.tracks.iter().any(|t| t.id == c.track && t.kind == TrackKind::Video));
    if dest.picture().is_some() && !has_video {
        say(Severity::Block, format!("no video on the timeline for a {} -- the podcast or master delivers the audio", dest.name()));
    }
    if dest.picture().is_some() && has_video {
        for w in crate::export::video_warnings(project) { say(Severity::Warn, w); }
        if dest == Destination::Vertical {
            say(Severity::Note, "filled to 9:16: a 16:9 picture keeps its middle third -- check that faces are in it".into());
        }
    }
    let reviews = project.markers.iter().filter(|m| m.name.starts_with("review ")).count();
    if reviews > 0 {
        say(Severity::Warn, format!("{reviews} assembly review marker{} still on the timeline -- an unreviewed cut",
                                     if reviews == 1 { " is" } else { "s are" }));
    }
    let caps = if dest.captions() { captions(project, words, end) } else { Vec::new() };
    if dest.captions() && caps.is_empty() {
        say(Severity::Note, "no transcripts under the audible clips: no captions (palette: transcripts)".into());
    }
    let chs = if dest.chapters() { chapters(project) } else { Vec::new() };

    let mut steps = vec![Step::Mixdown { from: Ticks::ZERO, to: end }];
    if let Some((t, c)) = dest.loudness() { steps.push(Step::Loudness { target_lufs: t, ceiling_dbtp: c }); }
    if let Some((width, height, fps, fill)) = dest.picture() { steps.push(Step::Picture { width, height, fps, fill }); }
    if !caps.is_empty() { steps.push(Step::Captions { cues: caps.len() }); }
    if chs.len() >= 2 { steps.push(Step::Chapters { count: chs.len() }); }
    f.sort_by(|a, b| b.severity.cmp(&a.severity));
    Delivery { destination: dest, steps, findings: f, captions: caps, chapters: chs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, Track, TrackId};

    fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }
    fn seg(a: f64, b: f64, t: &str) -> Segment { Segment { start: s(a), end: s(b), text: t.into(), speaker: None, confidence: 0.9 } }

    fn fragile() -> (Project, BTreeMap<SourceId, Vec<Segment>>, std::path::PathBuf) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
        let p = studio_collab::file::SaveFile::decode(&std::fs::read(dir.join("fragile-cartoon.studio")).unwrap())
            .expect("the fixture project opens").to_project();
        let words = crate::transcripts::beside_media(&p, &dir).words;
        (p, words, dir)
    }

    #[test]
    fn three_angles_saying_the_same_line_are_captioned_once_at_timeline_time() {
        let (p, words, _) = fragile();
        let caps = captions(&p, &words, p.duration());
        let texts: Vec<&str> = caps.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["Yeah, I found something I like more than that.", "You like acting, come on.",
                               "Partying is great.", "Hi guys, mind if we join the party?"], "{caps:?}");
        // the wide rolled at timeline 2.0 s and Max's line is at 2.0 s in its file
        assert!((caps[0].from.to_seconds() - 4.0).abs() < 0.11, "{:?}", caps[0]);
    }

    #[test]
    fn a_cue_is_clipped_to_what_the_clip_plays_and_muted_tracks_say_nothing() {
        let mut p = Project::default();
        for (id, muted) in [(1u64, false), (2, true)] {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Audio, name: format!("t{id}"), muted,
                locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
            p.apply(&Op::AddSource { source: Source { id: SourceId(id), path: format!("{id}.wav"), len: s(20.0),
                sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        }
        // plays source 5-9 s at timeline 10 s
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1), pos: s(10.0),
            src_in: s(5.0), len: s(4.0), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(2), source: SourceId(2), track: TrackId(2), pos: s(0.0),
            src_in: s(0.0), len: s(4.0), gain: 1.0, effects: vec![] } }).unwrap();
        let words: BTreeMap<SourceId, Vec<Segment>> = [
            (SourceId(1), vec![seg(1.0, 2.0, "not played"), seg(4.0, 6.0, "starts before the clip"), seg(7.0, 8.0, "inside"), seg(8.5, 12.0, "runs past it")]),
            (SourceId(2), vec![seg(0.5, 1.5, "muted track")]),
        ].into();
        let caps = captions(&p, &words, p.duration());
        let got: Vec<(f64, f64, &str)> = caps.iter().map(|c| (c.from.to_seconds(), c.to.to_seconds(), c.text.as_str())).collect();
        let want = [(10.0, 11.0, "starts before the clip"), (12.0, 13.0, "inside"), (13.5, 14.0, "runs past it")];
        assert_eq!(got.len(), 3, "{got:?}");
        for (g, w) in got.iter().zip(want) { assert!((g.0 - w.0).abs() < 1e-6 && (g.1 - w.1).abs() < 1e-6 && g.2 == w.2, "{got:?}"); }
        assert_eq!(to_srt(&caps[..1]), "1\n00:00:10,000 --> 00:00:11,000\nstarts before the clip\n");
    }

    #[test]
    fn a_video_destination_without_video_is_blocked_and_audio_ones_are_not() {
        let (mut p, words, dir) = fragile();
        let web = plan(&p, &words, Destination::Web, &dir);
        assert!(!web.blocked(), "{:?}", web.findings);
        assert!(web.steps.iter().any(|s| matches!(s, Step::Picture { width: 1920, height: 1080, .. })));
        assert!(web.steps.iter().any(|s| matches!(s, Step::Captions { cues: 4 })), "{:?}", web.steps);
        // take the video away
        let video: Vec<ClipId> = p.clips.values().filter(|c| p.sources[&c.source].has_video).map(|c| c.id).collect();
        for id in video { p.apply(&Op::RemoveClip { id }).unwrap(); }
        assert!(plan(&p, &words, Destination::Vertical, &dir).blocked());
        let pod = plan(&p, &words, Destination::Podcast, &dir);
        assert!(!pod.blocked(), "{:?}", pod.findings);
        assert!(pod.steps.contains(&Step::Loudness { target_lufs: -16.0, ceiling_dbtp: -1.0 }));
        assert!(!pod.steps.iter().any(|s| matches!(s, Step::Picture { .. })));
        let master = plan(&p, &words, Destination::Master, &dir);
        assert!(!master.steps.iter().any(|s| matches!(s, Step::Loudness { .. })), "a master is left alone");
    }

    #[test]
    fn preflight_names_missing_media_and_an_unreviewed_assembly() {
        let (mut p, words, dir) = fragile();
        let elsewhere = std::env::temp_dir().join("studio-deliver-nowhere");
        let d = plan(&p, &words, Destination::Podcast, &elsewhere);
        assert!(d.findings.iter().any(|f| f.severity == Severity::Warn && f.text.contains("not found")), "{:?}", d.findings);
        p.apply(&Op::SetMarker { at: s(3.0), name: "review 1: close call".into() }).unwrap();
        p.apply(&Op::SetMarker { at: s(5.0), name: "the argument".into() }).unwrap();
        let d = plan(&p, &words, Destination::Web, &dir);
        assert!(d.findings.iter().any(|f| f.text.contains("1 assembly review marker is still")), "{:?}", d.findings);
        // chapters: the review marker is not one; video sites need 0:00
        let names: Vec<&str> = d.chapters.iter().map(|(_, n)| n.as_str()).collect();
        assert!(!names.iter().any(|n| n.starts_with("review")), "{names:?}");
        assert_eq!(d.chapters[0], (Ticks::ZERO, "Start".into()));
        assert!(chapters_text(&d.chapters).lines().any(|l| l == "0:05 the argument"), "{}", chapters_text(&d.chapters));
        // findings are worst first
        assert!(d.findings.windows(2).all(|w| w[0].severity >= w[1].severity));
    }
}
