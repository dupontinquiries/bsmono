//! note_stacks — every kind of note on a timeline, exportable into the file:
//! one as CHAPTERS, the rest as named subtitle tracks.
//!
//! A cut carries several layers of notes, and people want different ones in
//! the file they receive: the director wants their notes as chapters, the
//! editor the shot list, the assistant the transcript. A container holds ONE
//! chapter list that players reliably show, but any number of subtitle tracks,
//! each with a title players list by name. So: pick a [`Stack`] for the
//! chapters, and write any others as tracks alongside -- ffmpeg does the
//! muxing ([`mux_args`]), from an FFMETADATA file ([`ffmetadata`]) and one
//! SubRip file per track ([`srt`]).
//!
//! The stacks, and where each comes from:
//!
//! | stack | source |
//! |---|---|
//! | director's notes | the timeline's own markers (not the tools' working ones) |
//! | transcript | whisper's lines under the audible clips, at timeline time |
//! | shots | the picture's cuts, labelled by TRACK (name tracks for the angles: "side", "top", "A", "B") and file |
//! | reviews / vfx / constraints | those tools' markers (`review N:`, `vfx:`, `constraint:`) |

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use studio_core::{Project, SourceId, Ticks, TrackKind};
use studio_voice::Segment;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stack { Director, Transcript, Shots, Reviews, Vfx, Constraints }

impl Stack {
    pub const ALL: [Stack; 6] = [Stack::Director, Stack::Transcript, Stack::Shots, Stack::Reviews, Stack::Vfx, Stack::Constraints];
    /// The track title players show.
    pub fn title(self) -> &'static str {
        match self {
            Stack::Director => "Director's notes", Stack::Transcript => "Transcript", Stack::Shots => "Shots",
            Stack::Reviews => "Assembly reviews", Stack::Vfx => "VFX notes", Stack::Constraints => "Constraint checks",
        }
    }
}

/// A note: from `at`, until `end` when it has a span (a line, a shot), and
/// its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Note { pub at: Ticks, pub end: Option<Ticks>, pub text: String }

/// The notes of one stack, in time order.
pub fn notes(project: &Project, words: &BTreeMap<SourceId, Vec<Segment>>, base: &Path, stack: Stack) -> Vec<Note> {
    let marks = |keep: &dyn Fn(&str) -> bool, strip: &str| -> Vec<Note> {
        let mut v: Vec<Note> = project.markers.iter().filter(|m| keep(&m.name))
            .map(|m| Note { at: m.at, end: None, text: m.name.strip_prefix(strip).unwrap_or(&m.name).trim().to_string() }).collect();
        v.sort_by_key(|n| n.at);
        v
    };
    match stack {
        Stack::Director => marks(&|n| !crate::deliver::is_working_marker(n) && !n.starts_with("vfx: ") && !n.starts_with("constraint: "), ""),
        Stack::Reviews => marks(&|n| n.starts_with("review "), ""),
        Stack::Vfx => marks(&|n| n.starts_with("vfx: "), "vfx: "),
        Stack::Constraints => marks(&|n| n.starts_with("constraint: "), "constraint: "),
        Stack::Transcript => crate::deliver::captions(project, words, project.duration()).into_iter()
            .map(|c| Note { at: c.from, end: Some(c.to), text: c.text }).collect(),
        Stack::Shots => {
            let paths: BTreeMap<SourceId, PathBuf> = project.sources.values().filter(|s| s.has_video)
                .map(|s| (s.id, base.join(&s.path))).collect();
            let mut out: Vec<Note> = Vec::new();
            for seg in studio_video::edit::edit_list(project, &paths, None) {
                let studio_video::edit::Segment::Cut(c) = seg else { continue };
                // the clip this cut is: same file, covering the cut's start
                let clip = project.clips.values().find(|k| paths.get(&k.source) == Some(&c.path) && k.pos <= c.at && c.at < k.end()
                    && project.tracks.iter().any(|t| t.id == k.track && t.kind == TrackKind::Video));
                let track = clip.and_then(|k| project.tracks.iter().find(|t| t.id == k.track)).map(|t| t.name.clone()).unwrap_or_default();
                let file = c.path.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                let text = if track.is_empty() || track == file { file } else { format!("{track} — {file}") };
                let end = c.at.saturating_add(c.len);
                // one shot across a split cut (a higher track covered part of it) stays one note
                match out.last_mut() {
                    Some(prev) if prev.text == text && prev.end == Some(c.at) => prev.end = Some(end),
                    _ => out.push(Note { at: c.at, end: Some(end), text }),
                }
            }
            out
        }
    }
}

/// FFMETADATA text escaping: `=`, `;`, `#`, `\` and newlines.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '=' | ';' | '#' | '\\' | '\n') { o.push('\\'); }
        o.push(ch);
    }
    o
}

/// Chapters as an FFMETADATA1 file, millisecond timebase. A chapter runs to
/// the next one's start (chapters tile the programme; players expect that),
/// the last to `end`. A first chapter later than 0 gets a "Start" before it.
pub fn ffmetadata(title: &str, chapters: &[Note], end: Ticks) -> String {
    let ms = |t: Ticks| (t.to_seconds() * 1000.0).round().max(0.0) as u64;
    let mut ch: Vec<(Ticks, String)> = chapters.iter().map(|n| (n.at, n.text.clone())).collect();
    ch.sort_by_key(|c| c.0);
    ch.dedup_by_key(|c| c.0);
    if ch.first().is_some_and(|c| c.0 > Ticks::ZERO) { ch.insert(0, (Ticks::ZERO, "Start".into())); }
    let mut t = format!(";FFMETADATA1\ntitle={}\n", esc(title));
    for (i, (at, name)) in ch.iter().enumerate() {
        let stop = ch.get(i + 1).map(|n| n.0).unwrap_or(end).max(*at);
        t.push_str(&format!("\n[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n", ms(*at), ms(stop), esc(name)));
    }
    t
}

/// A stack as SubRip. A note with no span shows for `hold`, or until the next
/// note if that is sooner, so two notes never overlap on screen.
pub fn srt(notes: &[Note], hold: Ticks) -> String {
    let cues: Vec<crate::deliver::Cue> = notes.iter().enumerate().map(|(i, n)| {
        let natural = n.end.unwrap_or_else(|| n.at.saturating_add(hold));
        let to = notes.get(i + 1).filter(|_| n.end.is_none()).map(|x| x.at.min(natural)).unwrap_or(natural).max(n.at);
        crate::deliver::Cue { from: n.at, to, text: n.text.clone() }
    }).collect();
    crate::deliver::to_srt(&cues)
}

/// ffmpeg arguments that put `metadata` (an FFMETADATA file: the chapters)
/// and each `(srt file, title)` as a subtitle track into a COPY of `video`
/// -- no re-encode. MP4 takes subtitles as `mov_text`; Matroska as SubRip.
pub fn mux_args(video: &Path, metadata: &Path, tracks: &[(PathBuf, String)], out: &Path) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mkv = out.extension().is_some_and(|e| e.eq_ignore_ascii_case("mkv"));
    let mut a: Vec<String> = vec!["-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into(), "-i".into(), s(video),
                                  "-f".into(), "ffmetadata".into(), "-i".into(), s(metadata)];
    for (p, _) in tracks { a.push("-i".into()); a.push(s(p)); }
    a.extend(["-map".into(), "0:v?".into(), "-map".into(), "0:a?".into()]);
    for i in 0..tracks.len() { a.push("-map".into()); a.push(format!("{}:0", i + 2)); }
    a.extend(["-map_metadata".into(), "1".into(), "-map_chapters".into(), "1".into(),
              "-c:v".into(), "copy".into(), "-c:a".into(), "copy".into(),
              "-c:s".into(), (if mkv { "srt" } else { "mov_text" }).into()]);
    for (i, (_, title)) in tracks.iter().enumerate() {
        // MP4 players name a track by its HANDLER; `title` alone was dropped
        // from the mp4 (seen with ffprobe). Both, so MKV and MP4 each keep one.
        a.push(format!("-metadata:s:s:{i}")); a.push(format!("title={title}"));
        a.push(format!("-metadata:s:s:{i}")); a.push(format!("handler_name={title}"));
        a.push(format!("-metadata:s:s:{i}")); a.push("language=eng".into());
    }
    a.push(s(out));
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::Op;

    fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }

    #[test]
    fn chapters_tile_the_programme_escape_what_ffmetadata_reserves_and_start_at_zero() {
        let n = |at: f64, t: &str| Note { at: s(at), end: None, text: t.into() };
        let m = ffmetadata("Blue Moon", &[n(12.5, "Verse = Allie; take #2"), n(40.0, "Hook")], s(60.0));
        assert!(m.starts_with(";FFMETADATA1\ntitle=Blue Moon\n"), "{m}");
        let chapters: Vec<&str> = m.split("[CHAPTER]").skip(1).collect();
        assert_eq!(chapters.len(), 3, "a Start chapter before the first note: {m}");
        assert!(chapters[0].contains("START=0\nEND=12500\ntitle=Start"));
        assert!(chapters[1].contains("START=12500\nEND=40000\ntitle=Verse \\= Allie\\; take \\#2"), "{m}");
        assert!(chapters[2].contains("START=40000\nEND=60000\ntitle=Hook"));
    }

    #[test]
    fn a_point_note_shows_for_a_while_but_never_under_the_next() {
        let t = srt(&[Note { at: s(1.0), end: None, text: "a".into() }, Note { at: s(2.0), end: None, text: "b".into() },
                      Note { at: s(10.0), end: Some(s(11.5)), text: "c".into() }], s(4.0));
        assert!(t.contains("00:00:01,000 --> 00:00:02,000\na"), "{t}");
        assert!(t.contains("00:00:02,000 --> 00:00:06,000\nb"), "{t}");
        assert!(t.contains("00:00:10,000 --> 00:00:11,500\nc"), "{t}");
    }

    #[test]
    fn stacks_sort_the_markers_by_the_tool_that_wrote_them() {
        let mut p = Project::default();
        for (at, name) in [(5.0, "the kiss -- hold longer"), (3.0, "review 1: close call"), (7.0, "vfx: 1 A001: shake -> stabilize"),
                           (9.0, "constraint: 1 violated by 2.0 s: x"), (1.0, "why @1.00s: cut")] {
            p.apply(&Op::SetMarker { at: s(at), name: name.into() }).unwrap();
        }
        let w = BTreeMap::new();
        let get = |st| notes(&p, &w, Path::new("."), st).into_iter().map(|n| n.text).collect::<Vec<_>>();
        assert_eq!(get(Stack::Director), vec!["the kiss -- hold longer"]);
        assert_eq!(get(Stack::Reviews), vec!["review 1: close call"]);
        assert_eq!(get(Stack::Vfx), vec!["1 A001: shake -> stabilize"]);
        assert_eq!(get(Stack::Constraints), vec!["1 violated by 2.0 s: x"]);
    }

    /// The fragile fixture: its three angles give a shot list labelled by
    /// track, and a REAL mux puts chapters and two named tracks in the file.
    #[test]
    fn shots_are_labelled_by_track_and_a_real_mux_carries_chapters_and_named_tracks() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() { eprintln!("skip: no ffmpeg"); return; }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
        let p = studio_collab::file::SaveFile::decode(&std::fs::read(dir.join("fragile-cartoon.studio")).unwrap()).unwrap().to_project();
        let words = crate::transcripts::beside_media(&p, &dir).words;
        let shots = notes(&p, &words, &dir, Stack::Shots);
        assert_eq!(shots.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), vec!["A001_wide"],
                   "the wide is the top track and covers the scene; a track named as its file is named once: {shots:?}");
        let tr = notes(&p, &words, &dir, Stack::Transcript);
        assert_eq!(tr.len(), 4, "{tr:?}");
        let tmp = std::env::temp_dir().join(format!("studio-notes-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let video = tmp.join("in.mp4");
        crate::export::tests_support_run(&["-y", "-v", "error", "-f", "lavfi", "-i", "color=c=gray:s=64x36:r=10:d=16", "-c:v", "mpeg4",
            video.to_str().unwrap()]);
        let meta = tmp.join("chapters.txt");
        std::fs::write(&meta, ffmetadata("Fragile", &shots, p.duration())).unwrap();
        let (a, b) = (tmp.join("t.srt"), tmp.join("s.srt"));
        std::fs::write(&a, srt(&tr, s(3.0))).unwrap();
        std::fs::write(&b, srt(&shots, s(3.0))).unwrap();
        let out = tmp.join("out.mp4");
        let args = mux_args(&video, &meta, &[(a, "Transcript".into()), (b, "Shots".into())], &out);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::export::tests_support_run(&refs);
        let probe = std::process::Command::new("ffprobe").args(["-v", "error", "-show_chapters", "-show_entries", "stream=codec_type:stream_tags=title,handler_name",
            "-of", "compact", out.to_str().unwrap()]).output().unwrap();
        let text = String::from_utf8_lossy(&probe.stdout);
        let chapters = text.lines().filter(|l| l.starts_with("chapter|")).count();
        assert!(chapters >= 2, "{text}");
        assert!(text.contains("=Transcript") && text.contains("=Shots"), "{text}");
        assert_eq!(text.lines().filter(|l| l.contains("codec_type=subtitle")).count(), 2, "{text}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
