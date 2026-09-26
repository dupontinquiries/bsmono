//! studio-assemble — the first 90% of a cut, from what was said.
//!
//! An editor's first pass over dialogue is mechanical: find each line of the
//! script in the takes, pick the take that says it, trim it with a little air,
//! lay them end to end. It is most of the hours and little of the judgement.
//! This crate does that pass and hands back a PROPOSAL -- never an edit --
//! with every decision explained and every doubtful one on a review list, so
//! the person's time goes to the last 10%: performance, rhythm, the cut that
//! makes the scene.
//!
//! ## The pipeline, each step visible
//!
//! 1. **Clean** ([`clean`]): drop what is not performance. Whisper's inventions
//!    over silence (the caller's noise filter -- `studio_video::spoken_slate::
//!    hallucination` in the app), and everything outside the PERFORMANCE
//!    WINDOW: after the last "action" and before the "cut" that follows it,
//!    so a take is never joined whole, slate and all (the film's rule 3).
//!    Slate chatter inside the window ("scene 13, take 2") is dropped too.
//! 2. **Match** ([`assemble`]): with an outline -- a script, lyrics, a list of
//!    questions, any ordered text -- each outline line is found across every
//!    take by word-order-aware similarity over runs of 1..=3 consecutive
//!    segments (whisper both merges and splits lines). Without one, the takes
//!    are read in name order and a line said again in a later take REPLACES
//!    the earlier one (the last take is the one the director kept going for).
//! 3. **Choose**: the best score wins, with the caller's per-source preference
//!    (from `take_report`, say) as a small bonus. The runners-up are kept as
//!    alternatives with their scores, so swapping a take is a lookup, not a
//!    search.
//! 4. **Trim**: a handle of air each side, never reaching into the previous or
//!    next line of the same take, never outside the performance window.
//! 5. **Lay out**: end to end from a start time.
//!
//! ## The review list is the product
//!
//! A proposal that is right 90% of the time is only useful if it says WHERE
//! the other 10% is. [`Review`]s name: an outline line not found; a close
//! call between two different performances; a doubtful transcript (whisper's
//! own confidence); a number in the words (never trusted -- "Scenna 3" was
//! Escena 13); a line trimmed shorter than anyone speaks. The app lays a
//! timeline marker on each, so the review is a walk along the markers.
//!
//! ## What is NOT here, and where it would go
//!
//! No language model. The matching is deterministic text alignment, which
//! runs offline on a phone and says exactly why it chose what it chose. The
//! two places a model would add judgement are INPUTS, not internals: the
//! outline (story order -- a model can write one from the script and the
//! transcripts) and the noise filter. Angle choice within a camera group is
//! `multicam_cut`'s job and recorder pairing is `dualsync`'s: angles of one
//! performance are passed in as [`Take::angle_group`] so a tie between them
//! is not reported as a choice of performance.

use std::collections::BTreeMap;

use studio_core::{Clip, ClipId, Op, SourceId, Ticks, TrackId};
use studio_voice::Segment;

/// One source's transcript, as the assembler sees it.
#[derive(Debug, Clone)]
pub struct Take {
    pub source: SourceId,
    /// For ordering without an outline (file names sort as shot order on
    /// every camera the film used) and for notes.
    pub name: String,
    pub segments: Vec<Segment>,
    /// The source's length; placements never run past it.
    pub len: Ticks,
    /// Takes with the same group are angles of ONE performance (a camera
    /// group); a close call between them is not a choice of take.
    pub angle_group: Option<u64>,
}

/// Knobs, with the defaults an editor would pick for dialogue.
#[derive(Debug, Clone)]
pub struct Options {
    /// Air kept before a line's first word / after its last.
    pub handle_in: Ticks,
    pub handle_out: Ticks,
    /// Below this similarity an outline line is "not found".
    pub min_match: f32,
    /// Two different performances this close in score: a person decides.
    pub close_call: f32,
    /// Whisper's confidence below this: flag the line.
    pub doubtful: f32,
    /// Silence left between placed lines.
    pub gap: Ticks,
    /// Added to a candidate's score, per source (0..~0.1): the caller's own
    /// ranking of takes -- clean audio, the director's circled takes.
    pub prefer: BTreeMap<SourceId, f32>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            handle_in: Ticks::from_seconds(0.25),
            handle_out: Ticks::from_seconds(0.35),
            min_match: 0.5,
            close_call: 0.06,
            doubtful: 0.6,
            gap: Ticks::ZERO,
            prefer: BTreeMap::new(),
        }
    }
}

/// Why a segment was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// The caller's noise filter: whisper talking over silence.
    Noise,
    /// Slate or crew calls ("scene 13, take 2", "rolling", "action", "cut").
    Slate,
    /// Before "action" or after "cut".
    OutsideWindow,
}

/// A segment after cleaning.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub source: SourceId,
    /// Index into the take's segments.
    pub index: usize,
    pub start: Ticks,
    pub end: Ticks,
    pub text: String,
    pub confidence: f32,
    pub dropped: Option<Dropped>,
}

/// One piece of the proposed cut.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub source: SourceId,
    pub src_in: Ticks,
    pub src_out: Ticks,
    /// Where it goes on the timeline.
    pub at: Ticks,
    /// What it says, and why this take: shown to the person.
    pub text: String,
    pub reason: String,
    /// 0..1: how sure the match is.
    pub confidence: f32,
    /// The outline line it answers, when there was an outline.
    pub outline_line: Option<usize>,
    /// Other takes that say it, best first: (source, src_in, score).
    pub alternatives: Vec<(SourceId, Ticks, f32)>,
}

impl Placement {
    pub fn len(&self) -> Ticks { self.src_out.saturating_sub(self.src_in) }
    pub fn end(&self) -> Ticks { self.at.saturating_add(self.len()) }
}

/// Something for the person to look at.
#[derive(Debug, Clone, PartialEq)]
pub enum Review {
    /// An outline line no take says (best score shown, if any).
    Missing { outline_line: usize, text: String, best: f32 },
    /// Two DIFFERENT performances within `close_call` of each other.
    CloseCall { placement: usize, runner_up: SourceId, margin: f32 },
    /// Whisper was unsure of these words.
    Doubtful { placement: usize, confidence: f32 },
    /// A number in the words: transcripts get numbers wrong.
    Number { placement: usize },
    /// Shorter than a spoken line, after trimming.
    Short { placement: usize, len: Ticks },
}

impl Review {
    /// The placement it is about, if any (a missing line has none).
    pub fn placement(&self) -> Option<usize> {
        match *self {
            Review::Missing { .. } => None,
            Review::CloseCall { placement, .. } | Review::Doubtful { placement, .. }
            | Review::Number { placement } | Review::Short { placement, .. } => Some(placement),
        }
    }
    /// One line for a marker or a list.
    pub fn label(&self, names: &BTreeMap<SourceId, String>) -> String {
        let n = |s: &SourceId| names.get(s).cloned().unwrap_or_else(|| format!("source {}", s.0));
        match self {
            Review::Missing { text, best, .. } => format!("missing: \"{}\" (best match {:.0}%)", short(text, 40), best * 100.0),
            Review::CloseCall { runner_up, margin, .. } => format!("close call: {} is within {:.0}%", n(runner_up), margin * 100.0),
            Review::Doubtful { confidence, .. } => format!("doubtful transcript ({:.0}%)", confidence * 100.0),
            Review::Number { .. } => "a number in the words: check it against the picture".into(),
            Review::Short { len, .. } => format!("only {:.2} s after trimming", len.to_seconds()),
        }
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// The whole proposal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Assembly {
    pub placements: Vec<Placement>,
    pub reviews: Vec<Review>,
    /// Everything cleaning dropped, per source, for the "why is my line
    /// missing" question.
    pub dropped: Vec<Line>,
}

impl Assembly {
    pub fn end(&self) -> Ticks { self.placements.last().map(|p| p.end()).unwrap_or(Ticks::ZERO) }

    /// "14 lines from 5 takes, 3 to review" -- the status line.
    pub fn summary(&self) -> String {
        let mut takes: Vec<SourceId> = self.placements.iter().map(|p| p.source).collect();
        takes.sort();
        takes.dedup();
        let missing = self.reviews.iter().filter(|r| matches!(r, Review::Missing { .. })).count();
        let others = self.reviews.len() - missing;
        let mut s = format!("{} line{} from {} take{}, {:.1} s", self.placements.len(),
            if self.placements.len() == 1 { "" } else { "s" }, takes.len(), if takes.len() == 1 { "" } else { "s" },
            self.end().to_seconds());
        if others > 0 { s.push_str(&format!(" · {others} to review")); }
        if missing > 0 { s.push_str(&format!(" · {missing} outline line{} not found", if missing == 1 { "" } else { "s" })); }
        s
    }

    /// The ops that lay the proposal down: one clip per placement on `track`,
    /// and a marker at every review (named `review N: …`, unique, so the
    /// markers are the editor's to-do list along the timeline). Clip ids are
    /// minted from `next_clip`. The caller applies them as ONE undo group.
    pub fn ops(&self, track: TrackId, next_clip: u64, names: &BTreeMap<SourceId, String>) -> Vec<Op> {
        let mut ops: Vec<Op> = self.placements.iter().enumerate().map(|(i, p)| Op::AddClip { clip: Clip {
            id: ClipId(next_clip + i as u64), source: p.source, track, pos: p.at, src_in: p.src_in,
            len: p.len(), gain: 1.0, effects: vec![] } }).collect();
        let mut missing_at = Ticks::ZERO;
        for (k, r) in self.reviews.iter().enumerate() {
            // a missing line is marked where it would have gone: after the
            // placement that precedes it in the outline
            let at = match r.placement() {
                Some(i) => self.placements[i].at,
                None => {
                    if let Review::Missing { outline_line, .. } = r {
                        missing_at = self.placements.iter().filter(|p| p.outline_line.is_some_and(|o| o < *outline_line))
                            .map(|p| p.end()).max().unwrap_or(Ticks::ZERO);
                    }
                    missing_at
                }
            };
            ops.push(Op::SetMarker { at, name: format!("review {}: {}", k + 1, r.label(names)) });
        }
        ops
    }
}

// ── cleaning ────────────────────────────────────────────────────────────

/// Words a take is started and stopped with, in the languages the film was
/// shot in. Lower case, accents kept (whisper keeps them).
const ACTION: &[&str] = &["action", "acción", "accion"];
const CUT: &[&str] = &["cut", "corte", "corten"];
const SLATE: &[&str] = &["scene", "escena", "take", "toma", "slate", "mark", "marker", "rolling", "roll",
    "camera", "cámara", "camara", "grabando", "speed", "sound", "sonido", "action", "acción", "accion",
    "cut", "corte", "corten", "and", "y", "plano", "shot"];

/// Lower-case alphanumeric words.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

fn is_number_word(w: &str) -> bool { w.chars().all(|c| c.is_ascii_digit()) }

/// Mostly slate: at least 60% of its words are crew calls or numbers.
fn is_slate(text: &str) -> bool {
    let w = words(text);
    if w.is_empty() { return false; }
    let n = w.iter().filter(|x| SLATE.contains(&x.as_str()) || is_number_word(x)).count();
    n * 5 >= w.len() * 3
}

fn has_any(text: &str, set: &[&str]) -> bool { words(text).iter().any(|w| set.contains(&w.as_str())) }

/// Step 1: every segment of `take`, with the ones that are not performance
/// marked [`Dropped`]. `noise` is the caller's hallucination test.
///
/// The window: after the LAST segment that calls "action" (a take is often
/// restarted: "action... sorry, again... action"), up to the first "cut"
/// after it. No "action" at all: the whole take (an interview has none).
pub fn clean(take: &Take, noise: &dyn Fn(&str) -> bool) -> Vec<Line> {
    let segs = &take.segments;
    let start = segs.iter().rposition(|s| has_any(&s.text, ACTION)).map(|i| i + 1).unwrap_or(0);
    let stop = segs.iter().enumerate().skip(start).find(|(_, s)| has_any(&s.text, CUT)).map(|(i, _)| i).unwrap_or(segs.len());
    segs.iter().enumerate().map(|(i, s)| {
        let dropped = if i < start || i >= stop { Some(Dropped::OutsideWindow) }
            else if noise(&s.text) { Some(Dropped::Noise) }
            else if is_slate(&s.text) { Some(Dropped::Slate) }
            else { None };
        Line { source: take.source, index: i, start: s.start, end: s.end, text: s.text.clone(),
               confidence: s.confidence, dropped }
    }).collect()
}

// ── matching ────────────────────────────────────────────────────────────

/// How alike two texts are, 0..1: the longest common SUBSEQUENCE of their
/// words, as a share of both (2·lcs / (|a|+|b|)). Order-aware, so "you like
/// acting, come on" against "come on, you like acting" scores lower than a
/// bag of words would say -- a line is its order.
pub fn similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (words(a), words(b));
    if a.is_empty() || b.is_empty() { return 0.0; }
    let mut dp = vec![vec![0u16; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            dp[i][j] = if a[i - 1] == b[j - 1] { dp[i - 1][j - 1] + 1 } else { dp[i - 1][j].max(dp[i][j - 1]) };
        }
    }
    2.0 * dp[a.len()][b.len()] as f32 / (a.len() + b.len()) as f32
}

/// A candidate: a run of consecutive kept lines in one take.
#[derive(Debug, Clone)]
struct Cand { take: usize, first: usize, last: usize, score: f32, text: String, confidence: f32 }

/// Every run of 1..=3 consecutive KEPT lines of each take, with its text.
fn runs(kept: &[Vec<&Line>]) -> Vec<(usize, usize, usize, String, f32)> {
    let mut out = Vec::new();
    for (t, lines) in kept.iter().enumerate() {
        for i in 0..lines.len() {
            let mut text = String::new();
            let mut conf = 1.0f32;
            for j in i..(i + 3).min(lines.len()) {
                if !text.is_empty() { text.push(' '); }
                text.push_str(&lines[j].text);
                conf = conf.min(lines[j].confidence);
                out.push((t, i, j, text.clone(), conf));
            }
        }
    }
    out
}

/// Split outline text into lines: blank lines and `#` comments skipped, a
/// leading speaker tag removed (scripts have them, transcripts don't).
///
/// A tag is what scripts actually write: ALL CAPS (`MAX:`, `DR. SMITH:`) or
/// ONE capitalised name (`Jude:`). "A line: with a colon" is words, and
/// keeps its first half.
pub fn parse_outline(text: &str) -> Vec<String> {
    let is_tag = |who: &str| {
        let letters: Vec<char> = who.chars().filter(|c| c.is_alphabetic()).collect();
        !letters.is_empty() && who.len() <= 24 && who.chars().all(|c| c.is_alphabetic() || c == ' ' || c == '.')
            && (letters.iter().all(|c| c.is_uppercase())
                || (!who.contains(' ') && letters[0].is_uppercase()))
    };
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| {
        match l.split_once(':') {
            Some((who, rest)) if is_tag(who.trim()) && !rest.trim().is_empty() => rest.trim().to_string(),
            _ => l.to_string(),
        }
    }).collect()
}

/// Steps 2 to 5: the proposal. `outline` empty = no outline (name order,
/// last take wins). Laid out from `at`.
pub fn assemble(takes: &[Take], outline: &[String], opts: &Options, noise: &dyn Fn(&str) -> bool, at: Ticks) -> Assembly {
    let cleaned: Vec<Vec<Line>> = takes.iter().map(|t| clean(t, noise)).collect();
    let kept: Vec<Vec<&Line>> = cleaned.iter().map(|ls| ls.iter().filter(|l| l.dropped.is_none()).collect()).collect();
    let mut asm = Assembly { dropped: cleaned.iter().flatten().filter(|l| l.dropped.is_some()).cloned().collect(), ..Default::default() };
    let prefer = |t: usize| opts.prefer.get(&takes[t].source).copied().unwrap_or(0.0);
    let same_performance = |a: usize, b: usize| a == b
        || (takes[a].angle_group.is_some() && takes[a].angle_group == takes[b].angle_group);

    // (candidate chosen, outline index, alternatives, runner-up margin)
    let mut chosen: Vec<(Cand, Option<usize>, Vec<Cand>)> = Vec::new();
    if !outline.is_empty() {
        let all = runs(&kept);
        for (oi, want) in outline.iter().enumerate() {
            let mut cands: Vec<Cand> = all.iter().map(|(t, i, j, text, conf)| Cand {
                take: *t, first: *i, last: *j, score: similarity(want, text) + prefer(*t), text: text.clone(), confidence: *conf,
            }).collect();
            // best run per take (a longer run that only adds noise must not
            // beat the exact line), then best first; ties: the later take
            cands.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.take.cmp(&a.take)).then((a.last - a.first).cmp(&(b.last - b.first))));
            let mut per_take: Vec<Cand> = Vec::new();
            for c in cands {
                if !per_take.iter().any(|p| p.take == c.take) { per_take.push(c); }
            }
            match per_take.first() {
                Some(best) if best.score >= opts.min_match => {
                    let best = best.clone();
                    let alts: Vec<Cand> = per_take.into_iter().skip(1).filter(|c| c.score >= opts.min_match).collect();
                    chosen.push((best, Some(oi), alts));
                }
                other => asm.reviews.push(Review::Missing { outline_line: oi, text: want.clone(),
                                                            best: other.map(|c| c.score).unwrap_or(0.0) }),
            }
        }
    } else {
        // name order; a line said again later replaces the earlier saying
        let mut order: Vec<usize> = (0..takes.len()).collect();
        order.sort_by(|a, b| takes[*a].name.cmp(&takes[*b].name));
        for t in order {
            for (i, l) in kept[t].iter().enumerate() {
                let c = Cand { take: t, first: i, last: i, score: 1.0, text: l.text.clone(), confidence: l.confidence };
                match chosen.iter_mut().find(|(prev, _, _)| similarity(&prev.text, &l.text) >= 0.75) {
                    Some(slot) => {
                        let old = std::mem::replace(&mut slot.0, c);
                        slot.2.insert(0, old);
                    }
                    None => chosen.push((c, None, Vec::new())),
                }
            }
        }
    }

    // trim and lay out
    let mut cursor = at;
    for (c, oi, alts) in chosen {
        let take = &takes[c.take];
        let lines = &kept[c.take];
        let (first, last) = (lines[c.first], lines[c.last]);
        let segs = &take.segments;
        // never into the neighbouring words: at most halfway to them
        let prev_end = first.index.checked_sub(1).map(|i| segs[i].end).unwrap_or(Ticks::ZERO);
        let next_start = segs.get(last.index + 1).map(|s| s.start).unwrap_or(take.len);
        let lo = Ticks((first.start.0 + prev_end.0) / 2).max(Ticks::ZERO);
        let hi = Ticks((last.end.0 + next_start.0) / 2).min(take.len);
        let src_in = first.start.saturating_sub(opts.handle_in).max(lo.min(first.start));
        let src_out = last.end.saturating_add(opts.handle_out).min(hi.max(last.end)).min(take.len);
        let idx = asm.placements.len();
        let runner = alts.iter().find(|a| !same_performance(a.take, c.take));
        let reason = match (oi, runner) {
            (Some(_), Some(r)) => format!("{:.0}% match; next best {} at {:.0}%", c.score.min(1.0) * 100.0, takes[r.take].name, r.score.min(1.0) * 100.0),
            (Some(_), None) => format!("{:.0}% match; the only take that says it", c.score.min(1.0) * 100.0),
            (None, _) if !alts.is_empty() => format!("said in {} take{}; the last, {}, kept", alts.len() + 1,
                                                     if alts.is_empty() { "" } else { "s" }, take.name),
            (None, _) => "in shot order".into(),
        };
        if let Some(r) = runner {
            if oi.is_some() && c.score - r.score < opts.close_call {
                asm.reviews.push(Review::CloseCall { placement: idx, runner_up: takes[r.take].source, margin: c.score - r.score });
            }
        }
        if c.confidence < opts.doubtful { asm.reviews.push(Review::Doubtful { placement: idx, confidence: c.confidence }); }
        // any digit at all: "14th" and "3B" are as untrustworthy as "13"
        if words(&c.text).iter().any(|w| w.chars().any(|ch| ch.is_ascii_digit())) { asm.reviews.push(Review::Number { placement: idx }); }
        let len = src_out.saturating_sub(src_in);
        if len < Ticks::from_seconds(0.4) { asm.reviews.push(Review::Short { placement: idx, len }); }
        asm.placements.push(Placement {
            source: take.source, src_in, src_out, at: cursor, text: c.text.clone(), reason,
            confidence: (c.score.min(1.0) * c.confidence).clamp(0.0, 1.0), outline_line: oi,
            alternatives: alts.iter().map(|a| (takes[a.take].source, kept[a.take][a.first].start, a.score.min(1.0))).collect(),
        });
        cursor = cursor.saturating_add(len).saturating_add(opts.gap);
    }
    asm
}

pub mod story;
pub mod constraints;

#[cfg(test)]
mod tests;
