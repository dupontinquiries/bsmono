//! insert_by_content — put a piece of a source at the playhead, found by what
//! is said in it or by a time range, trimmed on the way in.
//!
//! Cutting a film from transcripts is mostly "I want the line where she says
//! X, from there to where he answers". This is that, as palette steps:
//!
//! 1. **Find.** Type words and the palette lists matching transcript lines
//!    across every source ("“Good morning, I'm Gloria” — C0057 1.0 s"). Or
//!    type a time range — `C0057 12.5-18` or `C0057 0:12-0:18` — and the
//!    palette offers exactly that range.
//! 2. **Trim.** Just the line; the line with half a second either side (a
//!    cut needs air); a fixed length from the line's start; or from this line
//!    to a later one in the same source.
//! 3. **Place.** At the playhead, on the first track of the right kind that
//!    is free there, or a new track if none is — never on top of a clip.
//!
//! Line times come from whisper's `.srt`, which times lines, not words; a
//! range cut on them is right to the line and may carry a breath at either
//! end. The half-second option exists because of that, not despite it.

use std::collections::BTreeMap;

use studio_core::{Clip, ClipId, Op, Project, SourceId, Ticks, Track, TrackId, TrackKind};
use studio_voice::Segment;

/// A candidate line: where it is and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub source: SourceId,
    pub start: Ticks,
    pub end: Ticks,
    pub text: String,
}

/// The name a person types for a source: its file stem, case-folded.
pub fn source_name(project: &Project, id: SourceId) -> String {
    project.sources.get(&id).map(|s| {
        std::path::Path::new(&s.path).file_stem().map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.path.clone())
    }).unwrap_or_default()
}

/// Every transcript line, in source then time order.
pub fn lines(words: &BTreeMap<SourceId, Vec<Segment>>) -> Vec<Line> {
    let mut out: Vec<Line> = words.iter().flat_map(|(id, segs)| segs.iter().map(move |s| Line {
        source: *id, start: s.start, end: s.end, text: s.text.clone() })).collect();
    out.sort_by_key(|l| (l.source, l.start));
    out
}

/// `12.5`, `0:12.5`, `1:02:03` → seconds.
fn seconds(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 { return None; }
    let mut total = 0.0;
    for p in &parts {
        let v: f64 = p.parse().ok()?;
        if v < 0.0 { return None; }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// Read a typed time range: `<source> <from>-<to>`. The source is matched by
/// the END of its name, so `C0057` finds `2026-09-10_0905_C0057`; a name that
/// matches more than one source is refused rather than guessed.
pub fn parse_range(project: &Project, query: &str) -> Option<(SourceId, Ticks, Ticks)> {
    let q = query.trim();
    let (name, range) = q.rsplit_once(char::is_whitespace)?;
    let (a, b) = range.split_once('-')?;
    let (a, b) = (seconds(a)?, seconds(b)?);
    if b <= a { return None; }
    let want = name.trim().to_lowercase();
    if want.is_empty() { return None; }
    let hits: Vec<SourceId> = project.sources.keys().copied()
        .filter(|id| source_name(project, *id).to_lowercase().ends_with(&want)).collect();
    let [id] = hits.as_slice() else { return None };
    let len = project.sources[id].len;
    let (a, b) = (Ticks::from_seconds(a), Ticks::from_seconds(b).min(len));
    (b > a).then_some((*id, a, b))
}

/// The ops that place `source[from..to]` at `at`, on the first track of the
/// right kind with room there, or on a new track. Clip and track ids are
/// allocated past the project's current maximum.
pub fn place(project: &Project, source: SourceId, from: Ticks, to: Ticks, at: Ticks) -> Vec<Op> {
    let Some(src) = project.sources.get(&source) else { return Vec::new() };
    let kind = if src.has_video { TrackKind::Video } else { TrackKind::Audio };
    let len = to.saturating_sub(from);
    if len <= Ticks::ZERO { return Vec::new(); }
    let end = at.saturating_add(len);
    let free = project.tracks.iter().filter(|t| t.kind == kind && !t.locked).find(|t| {
        !project.clips.values().any(|c| c.track == t.id && c.pos < end && c.end() > at)
    }).map(|t| t.id);
    let mut ops = Vec::new();
    let track = match free {
        Some(t) => t,
        None => {
            let id = TrackId(project.tracks.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
            ops.push(Op::AddTrack { track: Track { id, kind, name: format!("inserts {}", id.0),
                muted: false, locked: false, gain: 1.0, route: None }, at: project.tracks.len() });
            id
        }
    };
    let id = ClipId(project.clips.keys().map(|c| c.0).max().unwrap_or(0) + 1);
    ops.push(Op::AddClip { clip: Clip { id, source, track, pos: at, src_in: from, len,
                                        gain: 1.0, effects: Vec::new() } });
    ops
}

/// Milliseconds, for carrying a time through a `Copy` palette verb.
pub fn ms(t: Ticks) -> u64 { (t.to_seconds() * 1000.0).round().max(0.0) as u64 }
pub fn from_ms(ms: u64) -> Ticks { Ticks::from_seconds(ms as f64 / 1000.0) }

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::Source;

    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "/card/2026-09-10_0905_C0057.mkv",
            Ticks::from_seconds(54.0), 48_000, 2, true) }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(2), "/card/2026-09-10_0910_C0058.mkv",
            Ticks::from_seconds(60.0), 48_000, 2, true) }).unwrap();
        p.apply(&Op::AddSource { source: Source::file(SourceId(3), "/rec/STE-013.wav",
            Ticks::from_seconds(60.0), 48_000, 1, false) }).unwrap();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video, name: "V1".into(),
            muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p
    }

    #[test]
    fn a_typed_range_finds_its_source_by_the_end_of_its_name() {
        let p = project();
        assert_eq!(parse_range(&p, "C0057 12.5-18"),
                   Some((SourceId(1), Ticks::from_seconds(12.5), Ticks::from_seconds(18.0))));
        assert_eq!(parse_range(&p, "c0058 0:12-0:18.5"),
                   Some((SourceId(2), Ticks::from_seconds(12.0), Ticks::from_seconds(18.5))));
        assert_eq!(parse_range(&p, "C0057 50-90").map(|r| r.2), Some(Ticks::from_seconds(54.0)),
                   "clamped to the source's length");
        assert_eq!(parse_range(&p, "C005 1-2"), None, "matches two sources: refused, not guessed");
        assert_eq!(parse_range(&p, "C0057 18-12"), None, "backwards");
        assert_eq!(parse_range(&p, "good morning"), None, "words are a search, not a range");
    }

    #[test]
    fn placing_uses_a_free_track_of_the_right_kind_and_never_covers_a_clip() {
        let mut p = project();
        // the video track is free at 10 s: the clip goes there
        let ops = place(&p, SourceId(1), Ticks::from_seconds(12.5), Ticks::from_seconds(18.0), Ticks::from_seconds(10.0));
        assert_eq!(ops.len(), 1, "{ops:?}");
        p.apply(&Op::Group { ops }).unwrap();
        let c = p.clips.values().next().unwrap().clone();
        assert_eq!((c.track, c.pos, c.src_in, c.len),
                   (TrackId(1), Ticks::from_seconds(10.0), Ticks::from_seconds(12.5), Ticks::from_seconds(5.5)));
        // again at 12 s: V1 is now occupied there, so a new video track is made
        let ops = place(&p, SourceId(2), Ticks::from_seconds(0.0), Ticks::from_seconds(3.0), Ticks::from_seconds(12.0));
        assert!(matches!(ops[0], Op::AddTrack { track: Track { kind: TrackKind::Video, .. }, .. }), "{ops:?}");
        p.apply(&Op::Group { ops }).unwrap();
        assert_eq!(p.clips.len(), 2);
        // a sound-only source goes to a sound track (a new one: there is none)
        let ops = place(&p, SourceId(3), Ticks::ZERO, Ticks::from_seconds(2.0), Ticks::ZERO);
        assert!(matches!(ops[0], Op::AddTrack { track: Track { kind: TrackKind::Audio, .. }, .. }));
        assert!(place(&p, SourceId(9), Ticks::ZERO, Ticks::from_seconds(1.0), Ticks::ZERO).is_empty());
    }

    #[test]
    fn lines_come_out_in_source_then_time_order() {
        let seg = |a: f64, b: f64, t: &str| Segment { start: Ticks::from_seconds(a), end: Ticks::from_seconds(b),
                                                      text: t.into(), speaker: None, confidence: 1.0 };
        let mut w = BTreeMap::new();
        w.insert(SourceId(2), vec![seg(5.0, 6.0, "later"), seg(1.0, 2.0, "earlier")]);
        w.insert(SourceId(1), vec![seg(3.0, 4.0, "first source")]);
        let l = lines(&w);
        assert_eq!(l.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["first source", "earlier", "later"]);
    }

    #[test]
    fn times_survive_the_millisecond_round_trip() {
        for s in [0.0, 1.25, 12.5, 3599.999] {
            let t = Ticks::from_seconds(s);
            assert!((from_ms(ms(t)).to_seconds() - s).abs() < 0.001);
        }
    }
}
