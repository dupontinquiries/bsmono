//! Deciding which minutes of a video are worth watching.
//!
//! Given a transcript and a prompt like "the bit about the battery", this
//! scores each minute and produces a plan of sections to keep and sections to
//! skip.
//!
//! # Two tiers, ported from aperture
//!
//! `aperture-complete/src/route_ai.rs` is the model here, and its own doc says
//! why: tier one is "a structural digest — pure, instant, deterministic", tier
//! two is "a model summary — generated in the background... cached by content
//! hash". The servicing seam is `take_pending`/`fulfill`, drained by whatever
//! backend is configured, and **no worker thread lives in the crate** because
//! "standing up and verifying a live model call needs a running provider this
//! crate can't supply or test itself."
//!
//! All of that applies here unchanged. So:
//!
//! - [`lexical_score`] is tier one: word overlap against the prompt. Instant,
//!   deterministic, always available, and good enough that a video with an
//!   obvious answer needs no model at all.
//! - [`Analyzer`] is tier two: it queues a request per window keyed by a hash
//!   of prompt + text, and a shell drains [`Analyzer::take_pending`] and calls
//!   [`Analyzer::fulfill`]. Nothing here spawns anything or talks to a model.
//!
//! # The safety property
//!
//! **A window nobody has scored is KEPT.** Absence of evidence is not evidence
//! of irrelevance, and the failure modes are not symmetric: wrongly keeping a
//! minute costs a minute, wrongly skipping one loses content the viewer wanted
//! and — worse — they cannot tell it happened. Everything below is arranged so
//! that a model that never answers degrades to playing the whole video.

pub mod ranges;
pub mod sections;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Duration;

/// One timed chunk of transcript, as a transcriber emits them.
#[derive(Clone, PartialEq, Debug)]
pub struct Cue {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// A minute of transcript, which is the unit analysis works on.
#[derive(Clone, PartialEq, Debug)]
pub struct Window {
    pub index: usize,
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

impl Window {
    /// Stable key for caching a model answer: the prompt and the text, so the
    /// same minute asked the same question is asked once.
    pub fn key(&self, prompt: &str) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        prompt.hash(&mut h);
        self.text.hash(&mut h);
        h.finish()
    }
}

/// Chop cues into fixed windows.
///
/// A cue straddling a boundary lands in the window its START falls in rather
/// than being split: splitting mid-sentence gives both windows half a thought
/// and makes both score badly, which is the opposite of what windowing is for.
pub fn windows(cues: &[Cue], window: Duration) -> Vec<Window> {
    if cues.is_empty() || window.is_zero() {
        return Vec::new();
    }
    let span = window.as_secs_f64();
    let mut out: Vec<Window> = Vec::new();
    for cue in cues {
        let idx = (cue.start.as_secs_f64() / span).floor() as usize;
        while out.len() <= idx {
            let i = out.len();
            out.push(Window {
                index: i,
                start: Duration::from_secs_f64(i as f64 * span),
                end: Duration::from_secs_f64((i + 1) as f64 * span),
                text: String::new(),
            });
        }
        let w = &mut out[idx];
        if !w.text.is_empty() {
            w.text.push(' ');
        }
        w.text.push_str(cue.text.trim());
    }
    out
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2 && !is_stopword(w))
        .map(str::to_string)
        .collect()
}

/// Words carrying no topic signal. Without this, "the" and "about" in a prompt
/// match every window equally and the score says nothing.
fn is_stopword(w: &str) -> bool {
    matches!(
        w,
        "the" | "and" | "for" | "that" | "this" | "with" | "you" | "are" | "was" | "have"
            | "but" | "not" | "從" | "about" | "what" | "when" | "how" | "why" | "does"
            | "did" | "its" | "his" | "her" | "they" | "them" | "there" | "here" | "then"
            | "than" | "into" | "out" | "our" | "your" | "all" | "any" | "can" | "will"
    )
}

/// Tier one: how much of the prompt's vocabulary appears in this text, 0..1.
///
/// Deliberately crude. It is not trying to be a model — it is the instant
/// answer that is always available, and for a video whose transcript literally
/// contains the words asked about it is enough on its own.
pub fn lexical_score(prompt: &str, text: &str) -> f32 {
    let want = tokenize(prompt);
    if want.is_empty() {
        // A prompt of nothing but stopwords cannot rank anything. Returning 0
        // would skip the entire video; 1 keeps it, which is the safe direction.
        return 1.0;
    }
    let have: std::collections::HashSet<String> = tokenize(text).into_iter().collect();
    let hits = want.iter().filter(|w| have.contains(*w)).count();
    hits as f32 / want.len() as f32
}

/// Where a window's score came from.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Score {
    /// Tier one only.
    Lexical(f32),
    /// A model answered.
    Model(f32),
    /// Queued, no answer yet. Treated as keep.
    Pending,
}

impl Score {
    pub fn value(self) -> f32 {
        match self {
            Score::Lexical(v) | Score::Model(v) => v,
            // Not 0: an unanswered window must never be skipped.
            Score::Pending => 1.0,
        }
    }

    pub fn is_model(self) -> bool {
        matches!(self, Score::Model(_))
    }
}

/// A request a backend should service.
#[derive(Clone, PartialEq, Debug)]
pub struct Pending {
    pub key: u64,
    pub window: usize,
    /// The full text to send. Assembling the provider's prompt is the shell's
    /// job — this crate does not know whether it is talking to ollama, a local
    /// gguf, or logscope.
    pub text: String,
    pub prompt: String,
}

/// Queues model requests and caches answers. Spawns nothing.
#[derive(Default)]
pub struct Analyzer {
    cache: HashMap<u64, f32>,
    queue: Vec<Pending>,
    queued: std::collections::HashSet<u64>,
    /// Below this, tier one is confident enough that asking a model is a waste
    /// of a round trip. Above it too — the interesting band is the middle.
    pub ask_band: (f32, f32),
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            ask_band: (0.15, 0.85),
            ..Default::default()
        }
    }

    /// Score a window, queueing a model request if tier one is undecided.
    pub fn score(&mut self, prompt: &str, w: &Window) -> Score {
        let key = w.key(prompt);
        if let Some(v) = self.cache.get(&key) {
            return Score::Model(*v);
        }
        let lex = lexical_score(prompt, &w.text);
        let (lo, hi) = self.ask_band;
        if lex <= lo || lex >= hi {
            // Confident either way: no round trip. This is what makes the
            // feature usable with no model configured at all.
            return Score::Lexical(lex);
        }
        if self.queued.insert(key) {
            self.queue.push(Pending {
                key,
                window: w.index,
                text: w.text.clone(),
                prompt: prompt.to_string(),
            });
        }
        Score::Pending
    }

    /// Drain the queue. The caller services these however it likes.
    pub fn take_pending(&mut self) -> Vec<Pending> {
        std::mem::take(&mut self.queue)
    }

    /// Deliver an answer.
    pub fn fulfill(&mut self, key: u64, score: f32) {
        self.cache.insert(key, score.clamp(0.0, 1.0));
        self.queued.remove(&key);
    }

    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    pub fn in_flight(&self) -> usize {
        self.queued.len()
    }
}

/// How aggressively to skip.
#[derive(Copy, Clone, Debug)]
pub struct SkipPolicy {
    /// At or above this, a window is worth watching.
    pub keep_at: f32,
    /// A skip shorter than this is not worth making: a two-second jump is more
    /// disorienting than the two seconds it saves, and it makes playback feel
    /// broken.
    pub min_skip: Duration,
    /// Never skip more than this fraction of the video. A plan that removes
    /// almost everything means the prompt did not match, and the useful answer
    /// is "I could not find it", not four seconds of video.
    pub max_skipped_fraction: f32,
}

impl Default for SkipPolicy {
    fn default() -> Self {
        Self {
            keep_at: 0.34,
            min_skip: Duration::from_secs(20),
            max_skipped_fraction: 0.9,
        }
    }
}

/// A run of windows with the same verdict.
#[derive(Clone, PartialEq, Debug)]
pub struct Section {
    pub start: Duration,
    pub end: Duration,
    pub keep: bool,
}

impl Section {
    pub fn duration(&self) -> Duration {
        self.end.saturating_sub(self.start)
    }
}

/// Turn per-window scores into a play plan.
pub fn plan(windows: &[Window], scores: &[Score], policy: &SkipPolicy) -> Vec<Section> {
    if windows.is_empty() || windows.len() != scores.len() {
        return Vec::new();
    }

    // Merge runs first, then apply the length rule: deciding per window and
    // then merging would let three consecutive 20-second skips survive as one
    // 60-second skip while a single one is dropped, which is inconsistent.
    let mut sections: Vec<Section> = Vec::new();
    for (w, s) in windows.iter().zip(scores) {
        let keep = s.value() >= policy.keep_at;
        match sections.last_mut() {
            Some(last) if last.keep == keep => last.end = w.end,
            _ => sections.push(Section {
                start: w.start,
                end: w.end,
                keep,
            }),
        }
    }

    // A skip too short to be worth the jump becomes a keep, and then merges
    // with its neighbours.
    for s in sections.iter_mut() {
        if !s.keep && s.duration() < policy.min_skip {
            s.keep = true;
        }
    }
    let merged = merge_adjacent(sections);

    let total: f64 = merged.iter().map(|s| s.duration().as_secs_f64()).sum();
    let skipped: f64 = merged
        .iter()
        .filter(|s| !s.keep)
        .map(|s| s.duration().as_secs_f64())
        .sum();
    // >= not >: "skip no more than 90%" means 90% is already too much, and the
    // exact boundary is reachable -- nine skipped minutes out of ten lands on
    // it precisely.
    if total > 0.0 && (skipped / total) as f32 >= policy.max_skipped_fraction {
        // Nothing matched. Play the whole thing rather than a highlight reel of
        // noise; "I could not find it" is the honest answer and the UI can say
        // so from `skipped_fraction`.
        return vec![Section {
            start: merged.first().map(|s| s.start).unwrap_or_default(),
            end: merged.last().map(|s| s.end).unwrap_or_default(),
            keep: true,
        }];
    }
    merged
}

fn merge_adjacent(sections: Vec<Section>) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for s in sections {
        match out.last_mut() {
            Some(last) if last.keep == s.keep => last.end = s.end,
            _ => out.push(s),
        }
    }
    out
}

/// Fraction of the plan that is skipped, for a UI to report.
pub fn skipped_fraction(plan: &[Section]) -> f32 {
    let total: f64 = plan.iter().map(|s| s.duration().as_secs_f64()).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let skipped: f64 = plan
        .iter()
        .filter(|s| !s.keep)
        .map(|s| s.duration().as_secs_f64())
        .sum();
    (skipped / total) as f32
}

/// The next position to play from, given where playback is now.
///
/// Returns `None` when the current position is inside a kept section — nothing
/// to do. This is what the player calls each tick.
pub fn next_position(plan: &[Section], at: Duration) -> Option<Duration> {
    let here = plan.iter().find(|s| at >= s.start && at < s.end)?;
    if here.keep {
        return None;
    }
    // Jump to the end of the skipped run, which is the start of the next kept
    // one. Jumping to the next section's start would be the same value, but
    // stating it this way keeps it correct when the last section is a skip.
    Some(here.end)
}

/// A plan that keeps exactly `keep` and skips the rest of `0..total`.
///
/// For "watch these chapters": the viewer chose, so none of `plan`'s
/// protections apply -- no minimum skip, no "too much skipped" fallback. A
/// viewer who picks one five-minute chapter of an hour wants five minutes.
/// Overlapping or unsorted spans are normalised; spans past `total` are cut.
pub fn plan_keeping(keep: &[(Duration, Duration)], total: Duration) -> Vec<Section> {
    let mut spans: Vec<(Duration, Duration)> = keep
        .iter()
        .map(|&(a, b)| (a.min(total), b.min(total)))
        .filter(|(a, b)| a < b)
        .collect();
    spans.sort();
    let mut merged: Vec<(Duration, Duration)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    if merged.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = Duration::ZERO;
    for (a, b) in merged {
        if a > at {
            out.push(Section { start: at, end: a, keep: false });
        }
        out.push(Section { start: a, end: b, keep: true });
        at = b;
    }
    if at < total {
        out.push(Section { start: at, end: total, keep: false });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(from: u64, to: u64, text: &str) -> Cue {
        Cue {
            start: Duration::from_secs(from),
            end: Duration::from_secs(to),
            text: text.into(),
        }
    }

    fn min(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    // ---- windowing -------------------------------------------------------

    #[test]
    fn cues_land_in_the_minute_they_start_in() {
        let cues = vec![
            cue(0, 5, "hello"),
            cue(30, 35, "world"),
            cue(61, 70, "second minute"),
        ];
        let w = windows(&cues, min(1));
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].text, "hello world");
        assert_eq!(w[1].text, "second minute");
        assert_eq!(w[1].start, min(1));
    }

    /// A gap in the transcript must still produce a window, or the indices stop
    /// matching the timeline and every later section is misplaced.
    #[test]
    fn a_silent_minute_still_gets_a_window() {
        let cues = vec![cue(0, 5, "start"), cue(130, 140, "much later")];
        let w = windows(&cues, min(1));
        assert_eq!(w.len(), 3, "the silent middle minute was dropped");
        assert!(w[1].text.is_empty());
        assert_eq!(w[2].start, min(2));
    }

    #[test]
    fn no_cues_is_no_windows_rather_than_a_panic() {
        assert!(windows(&[], min(1)).is_empty());
        assert!(windows(&[cue(0, 1, "x")], Duration::ZERO).is_empty());
    }

    // ---- tier one --------------------------------------------------------

    #[test]
    fn lexical_scoring_finds_the_prompts_words() {
        let hit = lexical_score("battery life", "the battery life is excellent here");
        let miss = lexical_score("battery life", "today we talk about camera lenses");
        assert!(hit > 0.9, "hit scored {hit}");
        assert!(miss < 0.1, "miss scored {miss}");
    }

    /// Without stopword filtering, "the" and "about" match every window equally
    /// and the score carries no signal at all.
    #[test]
    fn stopwords_do_not_carry_the_score() {
        let s = lexical_score("what about the thing", "the quick brown fox and the dog");
        assert!(s < 0.5, "stopwords dominated the score: {s}");
    }

    /// A prompt of nothing but stopwords cannot rank anything, and scoring it 0
    /// would skip the whole video.
    #[test]
    fn an_empty_prompt_keeps_everything() {
        assert_eq!(lexical_score("the and but", "anything at all"), 1.0);
        assert_eq!(lexical_score("", "anything at all"), 1.0);
    }

    // ---- tier two --------------------------------------------------------

    fn win(i: usize, text: &str) -> Window {
        Window {
            index: i,
            start: min(i as u64),
            end: min(i as u64 + 1),
            text: text.into(),
        }
    }

    /// Confident either way needs no round trip. This is what makes the feature
    /// work with no model configured at all.
    #[test]
    fn confident_windows_do_not_queue_a_model_call() {
        let mut a = Analyzer::new();
        let clear_hit = a.score("battery life", &win(0, "battery life battery life"));
        let clear_miss = a.score("battery life", &win(1, "unrelated cooking recipe"));
        assert!(matches!(clear_hit, Score::Lexical(_)));
        assert!(matches!(clear_miss, Score::Lexical(_)));
        assert!(a.take_pending().is_empty(), "queued an unnecessary call");
    }

    #[test]
    fn an_undecided_window_queues_once_and_caches_the_answer() {
        let mut a = Analyzer::new();
        let w = win(0, "battery something else entirely different topic");
        assert_eq!(a.score("battery life span", &w), Score::Pending);
        // Asking again must not queue a second time.
        assert_eq!(a.score("battery life span", &w), Score::Pending);
        let pending = a.take_pending();
        assert_eq!(pending.len(), 1, "queued {} times", pending.len());

        a.fulfill(pending[0].key, 0.9);
        assert_eq!(a.score("battery life span", &w), Score::Model(0.9));
        assert_eq!(a.in_flight(), 0);
    }

    /// The same minute asked the same question is asked once; asked a different
    /// question it is a different request.
    #[test]
    fn the_cache_key_covers_the_prompt_as_well_as_the_text() {
        let w = win(0, "some text");
        assert_ne!(w.key("prompt a"), w.key("prompt b"));
        assert_eq!(w.key("prompt a"), w.key("prompt a"));
    }

    #[test]
    fn a_model_answer_is_clamped() {
        let mut a = Analyzer::new();
        a.fulfill(1, 5.0);
        a.fulfill(2, -1.0);
        assert_eq!(a.cached(), 2);
    }

    // ---- the safety property ---------------------------------------------

    /// The property everything else is arranged around: a window nobody scored
    /// is kept. A model that never answers degrades to playing the whole video.
    #[test]
    fn unscored_windows_are_never_skipped() {
        assert_eq!(Score::Pending.value(), 1.0);
        let ws: Vec<Window> = (0..5).map(|i| win(i, "")).collect();
        let scores = vec![Score::Pending; 5];
        let p = plan(&ws, &scores, &SkipPolicy::default());
        assert!(p.iter().all(|s| s.keep), "skipped an unanalysed window");
    }

    // ---- planning --------------------------------------------------------

    fn scores(vals: &[f32]) -> Vec<Score> {
        vals.iter().map(|v| Score::Model(*v)).collect()
    }

    #[test]
    fn runs_of_the_same_verdict_merge_into_one_section() {
        let ws: Vec<Window> = (0..6).map(|i| win(i, "")).collect();
        let p = plan(&ws, &scores(&[0.9, 0.9, 0.0, 0.0, 0.9, 0.9]), &SkipPolicy::default());
        assert_eq!(p.len(), 3);
        assert!(p[0].keep && !p[1].keep && p[2].keep);
        assert_eq!(p[1].start, min(2));
        assert_eq!(p[1].end, min(4));
    }

    /// A jump shorter than the policy's floor is more disorienting than the
    /// time it saves.
    #[test]
    fn a_skip_too_short_to_be_worth_it_becomes_a_keep() {
        let ws: Vec<Window> = (0..3)
            .map(|i| Window {
                index: i,
                start: Duration::from_secs(i as u64 * 10),
                end: Duration::from_secs((i as u64 + 1) * 10),
                text: String::new(),
            })
            .collect();
        // The middle 10 seconds scores badly, but 10 < min_skip of 20.
        let p = plan(&ws, &scores(&[0.9, 0.0, 0.9]), &SkipPolicy::default());
        assert_eq!(p.len(), 1, "a 10-second skip survived: {p:?}");
        assert!(p[0].keep);
    }

    /// If almost everything scores badly the prompt did not match, and a
    /// highlight reel of noise is a worse answer than the whole video.
    #[test]
    fn a_plan_that_skips_nearly_everything_is_abandoned() {
        let ws: Vec<Window> = (0..10).map(|i| win(i, "")).collect();
        let mut vals = vec![0.0; 10];
        vals[4] = 0.9;
        let p = plan(&ws, &scores(&vals), &SkipPolicy::default());
        assert_eq!(p.len(), 1, "expected the whole video, got {p:?}");
        assert!(p[0].keep);
        assert_eq!(p[0].end, min(10));
    }

    #[test]
    fn mismatched_inputs_produce_no_plan_rather_than_a_wrong_one() {
        let ws: Vec<Window> = (0..3).map(|i| win(i, "")).collect();
        assert!(plan(&ws, &scores(&[0.9]), &SkipPolicy::default()).is_empty());
        assert!(plan(&[], &[], &SkipPolicy::default()).is_empty());
    }

    #[test]
    fn skipped_fraction_reports_what_was_cut() {
        let ws: Vec<Window> = (0..4).map(|i| win(i, "")).collect();
        let p = plan(&ws, &scores(&[0.9, 0.9, 0.0, 0.0]), &SkipPolicy::default());
        assert!((skipped_fraction(&p) - 0.5).abs() < 0.01);
    }

    // ---- playback --------------------------------------------------------

    #[test]
    fn playing_inside_a_kept_section_does_not_jump() {
        let ws: Vec<Window> = (0..4).map(|i| win(i, "")).collect();
        let p = plan(&ws, &scores(&[0.9, 0.9, 0.0, 0.0]), &SkipPolicy::default());
        assert_eq!(next_position(&p, min(0) + Duration::from_secs(30)), None);
    }

    #[test]
    fn entering_a_skipped_section_jumps_to_its_end() {
        let ws: Vec<Window> = (0..4).map(|i| win(i, "")).collect();
        let p = plan(&ws, &scores(&[0.9, 0.0, 0.0, 0.9]), &SkipPolicy::default());
        assert_eq!(next_position(&p, min(1)), Some(min(3)));
        assert_eq!(next_position(&p, min(2)), Some(min(3)), "mid-skip too");
    }

    #[test]
    fn a_position_past_the_end_is_not_a_jump() {
        let ws: Vec<Window> = (0..2).map(|i| win(i, "")).collect();
        let p = plan(&ws, &scores(&[0.9, 0.9]), &SkipPolicy::default());
        assert_eq!(next_position(&p, min(99)), None);
    }

    #[test]
    fn plan_keeping_tiles_the_timeline_and_merges_overlaps() {
        let s = |n: u64| Duration::from_secs(n);
        let p = plan_keeping(&[(s(300), s(400)), (s(100), s(200)), (s(150), s(250))], s(600));
        let got: Vec<(u64, u64, bool)> = p.iter().map(|x| (x.start.as_secs(), x.end.as_secs(), x.keep)).collect();
        assert_eq!(got, vec![(0, 100, false), (100, 250, true), (250, 300, false), (300, 400, true), (400, 600, false)]);
        // Chosen, so no protections: 90% skipped is what was asked for.
        assert!(skipped_fraction(&p) > 0.5);
        assert_eq!(next_position(&p, s(50)), Some(s(100)));
        assert_eq!(next_position(&p, s(120)), None);
        assert_eq!(next_position(&p, s(500)), Some(s(600)), "past the last kept span: to the end");
    }

    #[test]
    fn plan_keeping_of_nothing_is_no_plan() {
        let s = |n: u64| Duration::from_secs(n);
        assert!(plan_keeping(&[], s(10)).is_empty());
        assert!(plan_keeping(&[(s(20), s(30))], s(10)).is_empty(), "entirely past the end");
        assert_eq!(plan_keeping(&[(s(0), s(10))], s(10)).len(), 1, "keep everything: one section");
    }
}
