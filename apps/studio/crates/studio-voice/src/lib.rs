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

/// Parse a whisper `.srt` file into segments (source time).
///
/// Tolerant the way a sidecar has to be: blank lines, a missing index line,
/// `\r\n`, and a stray block with no timing are all skipped rather than
/// failing the whole file — one damaged cue must not hide the other four
/// hundred. Confidence is 1.0 because `.srt` does not carry one; speaker is
/// taken from a leading `[SPEAKER]` or `NAME:` only when whisper wrote one.
pub fn parse_srt(text: &str) -> Vec<Segment> {
    fn stamp(s: &str) -> Option<Ticks> {
        let s = s.trim().replace(',', ".");
        let mut parts = s.split(':');
        let (h, m, sec) = (parts.next()?, parts.next()?, parts.next()?);
        let secs = h.trim().parse::<f64>().ok()? * 3600.0 + m.trim().parse::<f64>().ok()? * 60.0
            + sec.trim().parse::<f64>().ok()?;
        Some(Ticks::from_seconds(secs))
    }
    let mut out = Vec::new();
    let text = text.replace("\r\n", "\n");
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let Some((a, b)) = lines[ti].split_once("-->") else { continue };
        let (Some(start), Some(end)) = (stamp(a), stamp(b)) else { continue };
        let body = lines[ti + 1..].join(" ");
        if body.trim().is_empty() { continue; }
        out.push(Segment { start, end: end.max(start), text: body.trim().to_string(),
                           speaker: None, confidence: 1.0 });
    }
    out
}

/// Words to show under a clip, laid out so they never collide.
///
/// A clip's gutter is a thin strip; a transcript is dense. Drawing every
/// segment would be an unreadable smear at any zoom where a clip is shorter
/// than its dialogue, so this places a label at each segment's start (in
/// pixels from the clip's left edge), clipped to the words that fit before
/// the NEXT label, and drops a segment entirely when it would start within
/// `min_gap_px` of the previous one. Zoomed out you get an occasional word
/// per clip; zoomed in, the whole line — the same data, answering "what is
/// being said here" at whatever scale you are looking.
///
/// `char_px` is the width of one character in the gutter font; the caller
/// knows it, this does not.
pub fn gutter_words(segments: &[Segment], src_in: Ticks, len: Ticks, px_per_sec: f32,
                    char_px: f32, min_gap_px: f32) -> Vec<(f32, String)> {
    let end = src_in.saturating_add(len);
    let x_of = |t: Ticks| ((t.saturating_sub(src_in)).to_seconds() as f32 * px_per_sec).max(0.0);
    let width = len.to_seconds() as f32 * px_per_sec;
    let live: Vec<&Segment> = segments.iter().filter(|s| s.end > src_in && s.start < end).collect();
    let mut placed: Vec<(f32, &Segment)> = Vec::new();
    for s in live {
        let x = x_of(s.start);
        if placed.last().map(|(px, _)| x - *px < min_gap_px).unwrap_or(false) { continue; }
        placed.push((x, s));
    }
    let mut out = Vec::new();
    for (i, (x, s)) in placed.iter().enumerate() {
        let room = placed.get(i + 1).map(|(nx, _)| *nx).unwrap_or(width) - x - char_px;
        let max_chars = (room / char_px.max(1.0)).floor() as usize;
        if max_chars < 3 { continue; }
        // whole words only: a word cut in half reads as a different word
        let mut shown = String::new();
        for w in s.text.split_whitespace() {
            let next = if shown.is_empty() { w.len() } else { shown.len() + 1 + w.len() };
            if next > max_chars { break; }
            if !shown.is_empty() { shown.push(' '); }
            shown.push_str(w);
        }
        if shown.is_empty() { continue; }
        if shown.len() < s.text.len() { shown.push('…'); }
        out.push((*x, shown));
    }
    out
}

/// Which words the transcript gutter shows.
///
/// The gutter answers a different question depending on the job:
///
/// * **All** — what is being said here (the default).
/// * **Filler** — where the junk is: "um", "uh", "you know", "o sea". For a
///   pass that cuts hesitations out of an interview or a narration take,
///   the gutter becomes a list of places to cut.
/// * **Unique** — the words that carry the content: names, places, the nouns
///   that occur once or twice in the whole project. Skimming a long take for
///   the line about the audition is faster when "the", "and" and every
///   repeated word are gone.
/// * **Dictionary** — only words from a list you supply: character names,
///   product names, a scene's key terms, a list of banned words for a
///   broadcast deliverable.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum WordFilter {
    #[default]
    All,
    Filler,
    Unique,
    Dictionary(BTreeSet<String>),
}

impl WordFilter {
    pub fn label(&self) -> &'static str {
        match self {
            WordFilter::All => "all words",
            WordFilter::Filler => "filler words",
            WordFilter::Unique => "unique words",
            WordFilter::Dictionary(_) => "words from a list",
        }
    }
}

/// Filler words and phrases, English and Spanish (the film's crew speaks
/// both). Multi-word fillers are matched as phrases, so "you know" is a
/// filler while "know" alone is not. Deliberately short: a filler list
/// that also catches "so" and "like" in their real meanings turns the
/// gutter into noise, so only the words that are almost never content are
/// here; "like" and "so" are left to a dictionary for people who want them.
pub const FILLERS: &[&str] = &[
    "um", "umm", "uh", "uhh", "er", "erm", "ah", "hmm", "mm", "mhm",
    "you know", "i mean", "kind of", "sort of", "basically", "literally",
    "este", "o sea", "eh", "em", "pues nada", "digamos",
];

/// Words too common to ever be "unique", whatever their count.
const STOPWORDS: &[&str] = &[
    "the", "and", "that", "this", "with", "have", "what", "your", "you", "are", "for", "was",
    "but", "not", "just", "they", "there", "then", "them", "from", "will", "would", "could",
    "about", "been", "were", "when", "which", "their", "i'm", "it's", "don't", "okay",
    "que", "para", "pero", "como", "esta", "este", "eso", "esto", "una", "los", "las", "del",
];

/// One word of a segment with an ESTIMATED time span.
///
/// Whisper's `.srt` times whole lines, not words. A word's position is
/// estimated by where it falls in the line's characters, which is right to
/// within a syllable or two for steady speech and wrong for a line with a
/// long pause in it. Good enough to put a cut near an "um" and then let
/// the waveform say exactly where; not good enough to cut on blind. The
/// confidence is halved to say so.
fn words_of(seg: &Segment) -> Vec<Segment> {
    let total = seg.text.chars().count().max(1) as f64;
    let span = (seg.end.0 - seg.start.0) as f64;
    let mut out = Vec::new();
    let mut pos = 0usize;
    for w in seg.text.split_whitespace() {
        let at = seg.text[pos..].find(w).map(|i| pos + i).unwrap_or(pos);
        let c0 = seg.text[..at].chars().count() as f64;
        let c1 = c0 + w.chars().count() as f64;
        pos = at + w.len();
        out.push(Segment {
            start: Ticks(seg.start.0 + (span * c0 / total) as i64),
            end: Ticks(seg.start.0 + (span * c1 / total) as i64),
            text: w.to_string(),
            speaker: seg.speaker.clone(),
            confidence: seg.confidence * 0.5,
        });
    }
    out
}

/// How often each normalized word occurs across every transcript given —
/// the corpus [`WordFilter::Unique`] measures rarity against.
pub fn corpus_counts<'a>(all: impl IntoIterator<Item = &'a Vec<Segment>>) -> BTreeMap<String, usize> {
    let mut c = BTreeMap::new();
    for segs in all {
        for s in segs {
            for w in s.text.split_whitespace() {
                let n = normalize(w);
                if !n.is_empty() { *c.entry(n).or_insert(0) += 1; }
            }
        }
    }
    c
}

/// A transcript reduced to what `filter` keeps, ready for [`gutter_words`].
///
/// `All` returns the lines as they are. The other modes return one segment
/// per kept word (or filler phrase) at its estimated time, so the gutter
/// shows it where it is said rather than at the start of its line.
pub fn filter_words(segments: &[Segment], filter: &WordFilter,
                    corpus: &BTreeMap<String, usize>) -> Vec<Segment> {
    match filter {
        WordFilter::All => segments.to_vec(),
        WordFilter::Filler => {
            let mut out = Vec::new();
            for seg in segments {
                let words = words_of(seg);
                let norm: Vec<String> = words.iter().map(|w| normalize(&w.text)).collect();
                let mut i = 0;
                while i < words.len() {
                    // longest phrase first, so "you know" wins over nothing
                    let hit = [2usize, 1].into_iter().find(|&n| {
                        i + n <= words.len()
                            && FILLERS.contains(&norm[i..i + n].join(" ").as_str())
                    });
                    match hit {
                        Some(n) => {
                            out.push(Segment {
                                start: words[i].start, end: words[i + n - 1].end,
                                text: words[i..i + n].iter().map(|w| w.text.as_str())
                                    .collect::<Vec<_>>().join(" "),
                                speaker: seg.speaker.clone(),
                                confidence: words[i].confidence,
                            });
                            i += n;
                        }
                        None => i += 1,
                    }
                }
            }
            out
        }
        WordFilter::Unique => segments.iter().flat_map(words_of).filter(|w| {
            let n = normalize(&w.text);
            n.chars().count() >= 4
                && !STOPWORDS.contains(&n.as_str())
                && corpus.get(&n).copied().unwrap_or(0) <= 2
        }).collect(),
        WordFilter::Dictionary(set) => segments.iter().flat_map(words_of)
            .filter(|w| set.contains(&normalize(&w.text))).collect(),
    }
}

/// A dictionary file: one word per line, `#` comments, case-folded the way
/// transcript words are, so "Gloria" in the list matches "gloria," in a line.
pub fn parse_dictionary(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect()
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
    /// How often it has been used — the ties `Repertoire::resolve` breaks on
    /// (see `mark_used`, which is what actually increments this). NOT
    /// weighted into this module's own `rank`/`Entry` palette scoring below:
    /// `Entry::Routine` here (a simpler, earlier design) has no field to
    /// carry a use count at all. The usage-weighted, UI-facing palette is
    /// `studio_egui::palette::Entry` — a deliberately more careful
    /// reimplementation whose own module docs explain the "ties only, never
    /// outranks a better textual match" nuance this crate's `rank` doesn't
    /// attempt. Read this field's effect through `resolve`'s voice-matching
    /// tie-break, not through anything in this module's palette code.
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

    /// Record that `name` was just successfully invoked, so it ranks higher
    /// on future ties. `resolve`'s scoring already reads `Routine.uses` (see
    /// its tie-break term), but until this method existed nothing in the
    /// crate ever wrote to it — the "the palette gets better the more you
    /// edit" promise had no mechanism behind it, since `resolve` takes `&self`
    /// and can't update its own counter from inside a read. A caller invokes
    /// this after a routine's steps actually ran, not on every resolve — a
    /// routine that merely matched but whose steps then failed shouldn't
    /// rank higher for it.
    pub fn mark_used(&mut self, name: &str) {
        if let Some(r) = self.routines.iter_mut().find(|r| r.name == name) {
            r.uses = r.uses.saturating_add(1);
        }
    }
}

// ── the command palette ─────────────────────────────────────────────────────

/// Anything the palette can offer.
///
/// A simpler, earlier design than `studio_egui::palette::Entry`: this
/// `Entry::Routine` carries only a name, not a use count, so `rank` below
/// can't weight by usage the way that crate's own palette does — see
/// `Routine::uses`'s doc comment for where that distinction actually lives.
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
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "take.wav".into(),
            len: Ticks::from_seconds(60.0), sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
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
        // the database buildup: the palette gets better the more you edit —
        // now exercised through the real API (`mark_used`), not by poking
        // the `uses` field directly the way this test used to have to.
        let mut r = Repertoire::default();
        r.record("a", &["do the thing"], vec![]);
        r.record("b", &["do the thing"], vec![]);
        for _ in 0..40 { r.mark_used("b"); }
        assert_eq!(r.resolve("do the thing").map(|x| x.name.as_str()), Some("b"));
    }

    #[test]
    fn mark_used_increments_only_the_named_routine() {
        let mut r = Repertoire::default();
        r.record("a", &["split it"], vec![]);
        r.record("b", &["tighten it"], vec![]);
        r.mark_used("a");
        r.mark_used("a");
        assert_eq!(r.routines[0].uses, 2);
        assert_eq!(r.routines[1].uses, 0, "marking one routine used must not touch another's count");
    }

    #[test]
    fn mark_used_on_an_unknown_name_is_a_harmless_no_op() {
        let mut r = Repertoire::default();
        r.record("a", &["split it"], vec![]);
        r.mark_used("this routine does not exist");
        assert_eq!(r.routines[0].uses, 0, "an unknown name must not panic or affect anything else");
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
    fn this_modules_entry_routine_carries_no_use_count_and_rank_cannot_weight_by_it() {
        // Documents, rather than just asserts in a comment, the distinction
        // this module's docs draw: `Entry::Routine` here has no `uses`
        // field, so two routines with identical labels score identically
        // regardless of how often either has actually been invoked via
        // `Repertoire::mark_used`. The usage-weighted palette is
        // `studio_egui::palette::Entry` — a different type entirely, not
        // reachable or testable from this crate.
        let entries = vec![
            Entry::Routine { name: "tighten".into() },
            Entry::Routine { name: "tighten".into() },
        ];
        let scored = rank(&entries, "tighten", 10);
        assert_eq!(scored.len(), 2, "both entries score identically — there is no per-routine usage data here to differentiate them");
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

#[cfg(test)]
mod gutter_tests {
    use super::*;

    const SRT: &str = "1\r\n00:00:00,000 --> 00:00:02,000\r\n Good morning, I'm Gloria.\r\n\r\n\
2\n00:00:02,000 --> 00:00:04,500\nThis is Max.\n\n\
garbage block with no timing\n\n\
3\n00:00:05,000 --> 00:00:09,000\nYou will be with other kids and they will take care of you.\n";

    #[test]
    fn whisper_srt_parses_tolerantly() {
        let s = parse_srt(SRT);
        assert_eq!(s.len(), 3, "the untimed block is skipped, not fatal: {s:?}");
        assert_eq!(s[0].text, "Good morning, I'm Gloria.");
        assert_eq!(s[1].start, Ticks::from_seconds(2.0));
        assert_eq!(s[1].end, Ticks::from_seconds(4.5));
        assert_eq!(s[2].start, Ticks::from_seconds(5.0));
        assert!(parse_srt("").is_empty() && parse_srt("nonsense").is_empty());
    }

    #[test]
    fn zoomed_in_the_whole_line_shows_and_zoomed_out_only_occasional_words() {
        let s = parse_srt(SRT);
        // 100 px/s, 7 px per char: plenty of room
        let wide = gutter_words(&s, Ticks::ZERO, Ticks::from_seconds(10.0), 100.0, 7.0, 40.0);
        assert_eq!(wide.len(), 3);
        assert_eq!(wide[0], (0.0, "Good morning, I'm Gloria.".to_string()));
        // 8 px/s: the whole 10 s clip is 80 px — at most one or two labels,
        // cut to whole words with an ellipsis, never overlapping
        let narrow = gutter_words(&s, Ticks::ZERO, Ticks::from_seconds(10.0), 8.0, 7.0, 40.0);
        assert!(!narrow.is_empty() && narrow.len() < 3, "{narrow:?}");
        for w in narrow.windows(2) {
            assert!(w[1].0 - w[0].0 >= 40.0, "labels keep their distance: {narrow:?}");
        }
        for (x, t) in &narrow {
            assert!(x + t.chars().count() as f32 * 7.0 <= 80.0 + 7.0, "{t} overruns the clip");
            assert!(!t.ends_with(|c: char| c.is_alphabetic()) || s.iter().any(|seg| seg.text.contains(t.as_str())),
                    "no word is cut in half: {t}");
        }
    }

    #[test]
    fn a_trimmed_clip_shows_only_what_it_plays() {
        let s = parse_srt(SRT);
        // clip plays source 4.6..9.0: only the third line is inside it
        let w = gutter_words(&s, Ticks::from_seconds(4.6), Ticks::from_seconds(4.4), 100.0, 7.0, 40.0);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].1.starts_with("You will be"));
        assert!((w[0].0 - 40.0).abs() < 1.0, "placed where the line starts inside the clip: {}", w[0].0);
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    fn seg(a: f64, b: f64, text: &str) -> Segment {
        Segment { start: Ticks::from_seconds(a), end: Ticks::from_seconds(b), text: text.into(),
                  speaker: None, confidence: 1.0 }
    }

    #[test]
    fn filler_mode_finds_the_junk_and_places_it_where_it_is_said() {
        let s = vec![seg(0.0, 4.0, "So um I went to the uh audition, you know, and Gloria said")];
        let f = filter_words(&s, &WordFilter::Filler, &BTreeMap::new());
        let texts: Vec<&str> = f.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["um", "uh", "you know,"], "phrases matched as phrases; 'so' is not junk");
        // estimated positions are in order and inside the line
        assert!(f.windows(2).all(|w| w[0].start < w[1].start));
        assert!(f[0].start > Ticks::ZERO && f[2].end < Ticks::from_seconds(4.0));
        assert!(f[0].confidence < 1.0, "an estimated word time says it is an estimate");
        // Spanish fillers too
        let es = filter_words(&[seg(0.0, 2.0, "Eh, o sea, este plano")], &WordFilter::Filler, &BTreeMap::new());
        assert_eq!(es.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["Eh,", "o sea,", "este"]);
    }

    #[test]
    fn unique_mode_keeps_the_rare_content_words() {
        // "audition" three times: past the rarity bar of two occurrences
        let a = vec![seg(0.0, 3.0, "the audition was great, the audition was long, another audition")];
        let b = vec![seg(0.0, 3.0, "Gloria left Max with Sister Frances at the church")];
        let corpus = corpus_counts([&a, &b]);
        let ua: Vec<String> = filter_words(&a, &WordFilter::Unique, &corpus).iter().map(|w| w.text.clone()).collect();
        assert_eq!(ua, vec!["great,", "long,", "another"], "a word said three times is not unique");
        let ub: Vec<String> = filter_words(&b, &WordFilter::Unique, &corpus).iter().map(|w| w.text.clone()).collect();
        assert!(ub.contains(&"Gloria".to_string()) && ub.contains(&"Frances".to_string()), "{ub:?}");
        assert!(!ub.iter().any(|w| w == "the" || w == "with" || w == "at"), "stopwords and short words go: {ub:?}");
    }

    #[test]
    fn dictionary_mode_shows_only_listed_words_whatever_their_case() {
        let dict = parse_dictionary("# characters\nGloria\nMax  # the lead\n\nsister frances\n");
        assert!(dict.contains("gloria") && dict.contains("max"));
        let s = vec![seg(0.0, 3.0, "Hi MAX, I'm gloria.")];
        let got: Vec<String> = filter_words(&s, &WordFilter::Dictionary(dict), &BTreeMap::new())
            .iter().map(|w| w.text.clone()).collect();
        assert_eq!(got, vec!["MAX,", "gloria."]);
    }

    #[test]
    fn all_mode_is_the_lines_untouched() {
        let s = vec![seg(0.0, 3.0, "um hello there")];
        assert_eq!(filter_words(&s, &WordFilter::All, &BTreeMap::new()), s);
    }
}
