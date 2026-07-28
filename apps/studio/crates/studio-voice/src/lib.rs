//! studio-voice — finding things by what was said, and saying what to do.
//!
//! Three features that look separate and are not: **transcript search**,
//! **voice commands**, and the **command palette** are all "type or say a
//! phrase, get the thing you meant". Building them as one avoids three
//! half-good search boxes.
//!
//! ## The insight that makes transcript search correct
//!
//! whisper.cpp transcribes a **source file**. An editor searches a **timeline**.
//! Those are not the same coordinate space, and the difference is not cosmetic:
//! a clip has an in-point and a length, so most of a take is usually *not on the
//! timeline at all*. A search that returns raw transcript offsets will happily
//! send you to a line that was trimmed out — and the same line can appear
//! **twice** if the source was used in two clips.
//!
//! [`TranscriptIndex::find`] therefore maps every hit through the clips that
//! reference the source, keeping only the parts actually visible and emitting
//! one result per clip that uses them. That is the whole difference between a
//! search that helps and one you stop trusting after the third wrong jump.
//!
//! ## Speakers
//!
//! Whisper's diarization gives a speaker label per segment. Keeping it lets the
//! obvious things work — "find where the second singer comes in", "show only the
//! interviewer" — and costs one field. Filtering by speaker is a first-class
//! argument rather than a post-filter, because on a two-hour multi-mic session
//! it is the difference between a useful list and 400 hits.
//!
//! ## Fast path
//!
//! The index is an inverted map from a normalised token to the segments
//! containing it, built once per transcript. A query is a set intersection over
//! postings, not a scan — which matters because the palette searches **on every
//! keystroke**, and a palette that stutters is a palette nobody opens.

use std::collections::{BTreeMap, BTreeSet};
use studio_core::{ClipId, Project, SourceId, Ticks};

/// One transcribed span of a source, as whisper.cpp emits it.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Offset into the SOURCE, not the timeline.
    pub start: Ticks,
    pub end: Ticks,
    pub text: String,
    /// Diarization label, when the model produced one.
    pub speaker: Option<String>,
    /// 0..1. Kept so the UI can show a hedge on a doubtful line rather than
    /// presenting a guess as fact.
    pub confidence: f32,
}

/// A source's transcript plus its search index.
#[derive(Debug, Clone)]
pub struct Transcript {
    pub source: SourceId,
    pub segments: Vec<Segment>,
    /// token → segment indices.
    index: BTreeMap<String, Vec<usize>>,
}

/// Fold a word to its match form: lowercase, strip punctuation. Deliberately
/// not stemming — "sing" must not match "singer" in a tool where you are
/// looking for an exact lyric.
pub fn normalize(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric() || *c == '\'').flat_map(|c| c.to_lowercase()).collect()
}

impl Transcript {
    pub fn new(source: SourceId, segments: Vec<Segment>) -> Transcript {
        let mut index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, seg) in segments.iter().enumerate() {
            for w in seg.text.split_whitespace() {
                let t = normalize(w);
                if t.is_empty() { continue; }
                let e = index.entry(t).or_default();
                if e.last() != Some(&i) { e.push(i); }
            }
        }
        Transcript { source, segments, index }
    }

    /// Segments containing every query token — a posting intersection, not a
    /// scan.
    pub fn segments_matching(&self, query: &str) -> Vec<usize> {
        let tokens: Vec<String> = query.split_whitespace().map(normalize).filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() { return Vec::new(); }
        let mut acc: Option<BTreeSet<usize>> = None;
        for t in &tokens {
            let hits: BTreeSet<usize> = self.index.get(t).cloned().unwrap_or_default().into_iter().collect();
            acc = Some(match acc {
                None => hits,
                Some(a) => a.intersection(&hits).copied().collect(),
            });
            if acc.as_ref().map(|a| a.is_empty()).unwrap_or(false) { break; }
        }
        let mut v: Vec<usize> = acc.unwrap_or_default().into_iter().collect();
        v.sort();
        v
    }

    pub fn speakers(&self) -> Vec<String> {
        let mut s: Vec<String> = self.segments.iter().filter_map(|x| x.speaker.clone()).collect();
        s.sort();
        s.dedup();
        s
    }
}

/// A search result, already in TIMELINE coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub clip: ClipId,
    /// Where on the timeline to send the playhead.
    pub at: Ticks,
    pub text: String,
    pub speaker: Option<String>,
    pub confidence: f32,
}

/// All transcripts in a project.
#[derive(Debug, Clone, Default)]
pub struct TranscriptIndex {
    pub by_source: BTreeMap<SourceId, Transcript>,
}

impl TranscriptIndex {
    pub fn add(&mut self, t: Transcript) { self.by_source.insert(t.source, t); }

    /// Find spoken text on the TIMELINE.
    ///
    /// Every raw hit is mapped through the clips using its source: a segment
    /// trimmed out of every clip does not appear, and a segment inside two
    /// clips appears twice — once per place you could actually go.
    pub fn find(&self, project: &Project, query: &str, speaker: Option<&str>) -> Vec<Hit> {
        let mut out = Vec::new();
        for (source, tr) in &self.by_source {
            let idx = tr.segments_matching(query);
            if idx.is_empty() { continue; }
            let clips: Vec<_> = project.clips.values().filter(|c| c.source == *source).collect();
            for i in idx {
                let seg = &tr.segments[i];
                if let Some(want) = speaker {
                    if seg.speaker.as_deref() != Some(want) { continue; }
                }
                for c in &clips {
                    // the clip shows [src_in, src_in+len) of the source
                    let vis_start = c.src_in;
                    let vis_end = c.src_in.saturating_add(c.len);
                    if seg.end <= vis_start || seg.start >= vis_end { continue; }
                    let into = seg.start.max(vis_start).saturating_sub(c.src_in);
                    out.push(Hit {
                        clip: c.id,
                        at: c.pos.saturating_add(into),
                        text: seg.text.clone(),
                        speaker: seg.speaker.clone(),
                        confidence: seg.confidence,
                    });
                }
            }
        }
        out.sort_by_key(|h| h.at);
        out
    }
}

// ── the repertoire ──────────────────────────────────────────────────────────

/// A saved workflow, bound to a phrase you can say or type.
///
/// This is voiceterm's repertoire idea applied to editing: the useful unit is
/// not a single command but the **sequence you keep repeating** — "split here,
/// delete the left, snap the rest up". Recording that once and naming it turns
/// a nine-key ritual into three words.
///
/// The steps are ops, so a repertoire entry is undoable, shareable and
/// replayable by exactly the machinery everything else uses. Nothing about
/// voice is special-cased downstream.
#[derive(Debug, Clone, PartialEq)]
pub struct Routine {
    pub name: String,
    /// Phrases that invoke it. Several, because people do not say the same
    /// words twice — "tighten this", "tighten it up", "tighten".
    pub phrases: Vec<String>,
    /// What it does, as a template. `Placeholder` marks what the caller fills
    /// in from context (the selection, the playhead).
    pub steps: Vec<Step>,
    /// How often it has been used — the database buildup that makes the
    /// palette's ranking get better the more you edit.
    pub uses: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A concrete op, ready to apply.
    Op(studio_core::Op),
    /// Filled in at invocation from what is selected / where the playhead is.
    Placeholder(String),
}

#[derive(Debug, Clone, Default)]
pub struct Repertoire {
    pub routines: Vec<Routine>,
}

impl Repertoire {
    /// Match a spoken or typed phrase to a routine.
    ///
    /// Speech recognition drops and mangles words, so an exact match would fail
    /// constantly. Scoring is by token overlap with a bonus for order, and a
    /// floor beneath which nothing matches at all — because running the WRONG
    /// workflow on someone's timeline is far worse than running none.
    pub fn resolve(&self, spoken: &str) -> Option<&Routine> {
        let said: Vec<String> = spoken.split_whitespace().map(normalize).filter(|t| !t.is_empty()).collect();
        if said.is_empty() { return None; }
        let mut best: Option<(f32, &Routine)> = None;
        for r in &self.routines {
            for p in &r.phrases {
                let want: Vec<String> = p.split_whitespace().map(normalize).filter(|t| !t.is_empty()).collect();
                if want.is_empty() { continue; }
                let matched = want.iter().filter(|w| said.contains(w)).count();
                if matched == 0 { continue; }
                // coverage of the PHRASE, so a long sentence containing the
                // phrase still triggers it, but a single shared word does not
                let mut score = matched as f32 / want.len() as f32;
                if said.len() >= want.len()
                    && said.windows(want.len()).any(|w| w == want.as_slice()) {
                    score += 0.5; // said in order, verbatim
                }
                score += (r.uses as f32).min(50.0) * 0.002; // the more you use it, the more it wins ties
                if best.map(|(b, _)| score > b).unwrap_or(true) {
                    best = Some((score, r));
                }
            }
        }
        best.filter(|(s, _)| *s >= 0.6).map(|(_, r)| r)
    }

    pub fn record(&mut self, name: &str, phrases: &[&str], steps: Vec<Step>) {
        self.routines.push(Routine {
            name: name.into(),
            phrases: phrases.iter().map(|s| s.to_string()).collect(),
            steps, uses: 0,
        });
    }
}

// ── the command palette ─────────────────────────────────────────────────────

/// Anything the palette can offer.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A built-in command.
    Command { id: String, label: String },
    /// Jump to a clip.
    Clip { id: ClipId, label: String, at: Ticks },
    /// Jump to a line of dialogue or a lyric.
    Spoken(Hit),
    /// Run a saved workflow.
    Routine { name: String },
}

impl Entry {
    pub fn label(&self) -> &str {
        match self {
            Entry::Command { label, .. } => label,
            Entry::Clip { label, .. } => label,
            Entry::Spoken(h) => &h.text,
            Entry::Routine { name } => name,
        }
    }
}

/// Subsequence match with a score, the pattern the fleet's other palettes use:
/// letters must appear in order but not adjacently, so "spl" finds "split".
/// Consecutive matches and a match at a word start score higher, which is what
/// makes short queries feel like they read your mind.
pub fn fuzzy(query: &str, target: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() { return Some(0); }
    let t: Vec<char> = target.to_lowercase().chars().collect();
    let (mut qi, mut score, mut prev) = (0usize, 0i32, None::<usize>);
    for (ti, ch) in t.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            score += 1;
            if prev == Some(ti.wrapping_sub(1)) { score += 3; }
            if ti == 0 || t.get(ti.wrapping_sub(1)).map(|c| !c.is_alphanumeric()).unwrap_or(false) {
                score += 5;
            }
            prev = Some(ti);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score)
}

/// Rank palette entries for a query. Spoken hits carry a small penalty so a
/// command you asked for by name is not buried under fifty transcript lines
/// that happen to contain the word.
pub fn rank(entries: &[Entry], query: &str, limit: usize) -> Vec<Entry> {
    let mut scored: Vec<(i32, usize, Entry)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let mut s = fuzzy(query, e.label())?;
            if matches!(e, Entry::Spoken(_)) { s -= 2; }
            if matches!(e, Entry::Routine { .. }) { s += 1; }
            Some((s, i, e.clone()))
        })
        .collect();
    // stable: equal scores keep source order, so the list does not shuffle as
    // you type another letter
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, Op, Source, Track, TrackId, TrackKind};

    fn seg(a: f64, b: f64, text: &str, sp: &str) -> Segment {
        Segment { start: Ticks::from_seconds(a), end: Ticks::from_seconds(b),
                  text: text.into(), speaker: Some(sp.into()), confidence: 0.9 }
    }

    /// A 60s source used by ONE clip that shows only 20s..40s of it.
    fn project() -> Project {
        let mut p = Project { name: "t".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "take.wav".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 1, has_video: false } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(100.0), src_in: Ticks::from_seconds(20.0),
            len: Ticks::from_seconds(20.0), gain: 1.0, effects: Vec::new() } }).unwrap();
        p
    }

    fn transcript() -> Transcript {
        Transcript::new(SourceId(1), vec![
            seg(5.0, 8.0, "this line was trimmed away", "ada"),
            seg(25.0, 27.0, "hold the light a little longer", "ada"),
            seg(30.0, 33.0, "and the chorus comes back in", "bo"),
            seg(50.0, 52.0, "another trimmed line about light", "bo"),
        ])
    }

    #[test]
    fn a_hit_lands_in_timeline_time_not_source_time() {
        // The clip starts at 100s on the timeline and shows from 20s of the
        // source, so a line at 25s of the source is at 105s of the timeline.
        let mut ix = TranscriptIndex::default();
        ix.add(transcript());
        let hits = ix.find(&project(), "chorus", None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].at, Ticks::from_seconds(110.0), "source 30s → timeline 110s");
        assert_eq!(hits[0].clip, ClipId(1));
    }

    #[test]
    fn trimmed_away_dialogue_does_not_appear() {
        // The whole reason find() maps through clips: a search that sends you
        // to a line that is not on the timeline is one you stop trusting.
        let mut ix = TranscriptIndex::default();
        ix.add(transcript());
        let hits = ix.find(&project(), "light", None);
        assert_eq!(hits.len(), 1, "two segments say 'light'; only one is on the timeline");
        assert!(hits[0].text.contains("hold the light"));
    }

    #[test]
    fn a_source_used_twice_yields_a_hit_per_place_you_could_go() {
        let mut p = project();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(2), source: SourceId(1), track: TrackId(1),
            pos: Ticks::from_seconds(200.0), src_in: Ticks::from_seconds(20.0),
            len: Ticks::from_seconds(20.0), gain: 1.0, effects: Vec::new() } }).unwrap();
        let mut ix = TranscriptIndex::default();
        ix.add(transcript());
        let hits = ix.find(&p, "chorus", None);
        assert_eq!(hits.len(), 2, "the same line is reachable in two clips");
        assert_eq!(hits[0].at, Ticks::from_seconds(110.0));
        assert_eq!(hits[1].at, Ticks::from_seconds(210.0));
        assert!(hits[0].at < hits[1].at, "results are in timeline order");
    }

    #[test]
    fn speakers_can_be_isolated() {
        let mut ix = TranscriptIndex::default();
        ix.add(transcript());
        let p = project();
        assert_eq!(ix.find(&p, "the", Some("ada")).len(), 1);
        assert_eq!(ix.find(&p, "the", Some("bo")).len(), 1);
        assert_eq!(ix.find(&p, "the", Some("nobody")).len(), 0);
        assert_eq!(transcript().speakers(), vec!["ada".to_string(), "bo".to_string()]);
    }

    #[test]
    fn multi_word_queries_intersect_rather_than_union() {
        let t = transcript();
        // both words, same segment
        assert_eq!(t.segments_matching("hold light").len(), 1);
        // words that never co-occur
        assert!(t.segments_matching("chorus trimmed").is_empty());
        // punctuation and case are folded, but stems are not: "sing" must not
        // match "singer" in a tool for finding an exact lyric
        assert_eq!(normalize("Light,"), "light");
        assert!(t.segments_matching("lights").is_empty());
    }

    #[test]
    fn a_routine_answers_to_how_people_actually_speak() {
        let mut r = Repertoire::default();
        r.record("tighten", &["tighten this", "tighten it up"],
                 vec![Step::Placeholder("selection".into())]);
        r.record("mute track", &["mute this track"], vec![Step::Placeholder("track".into())]);

        assert_eq!(r.resolve("tighten this").map(|x| x.name.as_str()), Some("tighten"));
        // extra words around the phrase still resolve — speech is not typing
        assert_eq!(r.resolve("okay now tighten it up please").map(|x| x.name.as_str()), Some("tighten"));
        // a partial phrase resolves when coverage is high enough
        assert_eq!(r.resolve("mute this").map(|x| x.name.as_str()), Some("mute track"));
        // but a single incidental word must NOT fire a workflow: running the
        // wrong one on someone's timeline is worse than running none
        assert!(r.resolve("this").is_none());
        assert!(r.resolve("what did you say about the weather").is_none());
        assert!(r.resolve("").is_none());
    }

    #[test]
    fn frequently_used_routines_win_ties() {
        // the database buildup: the palette gets better the more you edit
        let mut r = Repertoire::default();
        r.record("a", &["do the thing"], vec![]);
        r.record("b", &["do the thing"], vec![]);
        r.routines[1].uses = 40;
        assert_eq!(r.resolve("do the thing").map(|x| x.name.as_str()), Some("b"));
    }

    #[test]
    fn the_palette_ranks_the_way_a_palette_should() {
        let entries = vec![
            Entry::Command { id: "split".into(), label: "Split clip at playhead".into() },
            Entry::Command { id: "silence".into(), label: "Insert silence".into() },
            Entry::Routine { name: "split and tighten".into() },
            Entry::Spoken(Hit { clip: ClipId(1), at: Ticks::ZERO,
                text: "we should split up".into(), speaker: None, confidence: 0.8 }),
        ];
        let top = rank(&entries, "spl", 10);
        assert!(!top.is_empty());
        assert!(matches!(top[0], Entry::Command { .. } | Entry::Routine { .. }),
                "a command or routine outranks a transcript line that merely contains the word");
        // subsequence, not substring: letters in order but not adjacent
        assert!(fuzzy("sap", "Split at playhead").is_some());
        assert!(fuzzy("zq", "Split clip").is_none());
        // an empty query matches everything, so opening the palette shows a list
        assert_eq!(rank(&entries, "", 10).len(), 4);
        // and the limit is honoured
        assert_eq!(rank(&entries, "", 2).len(), 2);
    }

    #[test]
    fn ranking_is_stable_so_the_list_does_not_shuffle_as_you_type() {
        let entries: Vec<Entry> = (0..6)
            .map(|i| Entry::Command { id: format!("c{i}"), label: format!("Command {i}") })
            .collect();
        let a = rank(&entries, "command", 10);
        let b = rank(&entries, "command", 10);
        assert_eq!(a, b, "equal scores keep source order");
    }
}
