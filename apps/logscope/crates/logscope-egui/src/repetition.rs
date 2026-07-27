//! repetition.rs — detect degenerate repetition in a model's output and manage it PROPORTIONALLY.
//!
//! Small models fall into loops: a verbatim phrase repeating ("the value the value the value"), a
//! wrapped n-gram cycle, or — in an agentic loop — the SAME turn/tool-call emitted over and over. Left
//! alone this turns the model's speed into a runaway train that burns the whole token budget (or, for an
//! agent, the whole step budget) producing nothing.
//!
//! `genguard::runaway_stop` already catches EOS-framing loops and cuts the decode loop, but it's binary
//! (stop / don't) and backend-gated. This module is the graded, testable core used everywhere:
//!   * `tail_cycle` — the strongest, cheapest signal: is the TAIL a short phrase repeated k+ times?
//!   * `RepeatMonitor` — fed the growing text, tracks WORSENING and returns a `Severity` + `Recovery`, so
//!     a caller can nudge early and only cut when it's truly stuck (speed stays a tool).
//!   * `LoopGuard` — turn-level: an agent repeating the same action gets escalating STRIKES → a nudge, then
//!     (a persistent bad actor) an abort, instead of running to the step limit.
//!
//! All pure + unit-tested; no model, no GPU.

use std::collections::HashSet;

/// How badly the output is repeating right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Clean,   // healthy, varied output
    Warming, // early signs (some n-gram repeat) — worth a light touch
    Looping, // locked in a cycle — recover now
}

/// What to do about it — proportional, so a fast model gets re-steered before it's cut.
#[derive(Clone, Debug, PartialEq)]
pub enum Recovery {
    None,
    /// Raise the repetition penalty by this delta and keep going (early, cheap correction).
    BumpPenalty(f32),
    /// Stop generation now — it's a runaway; the caller may re-issue with a nudge.
    Cut,
}

// ── the strongest signal: a repeating tail ──────────────────────────────────────────────────────

/// If the END of `text` is a short WORD phrase repeated `>= min_reps` times, return that period (as words)
/// and the repetition count. This is the classic "runaway train" — the model latched onto a phrase and is
/// emitting it forever. Word-level so it catches both "x x x" and "struct S struct S struct S". Returns the
/// SHORTEST such period (so "a b a b a b" reports period ["a","b"], not the whole tail).
pub fn tail_cycle(text: &str, min_reps: usize) -> Option<(Vec<String>, usize)> {
    let min_reps = min_reps.max(2);
    let words: Vec<&str> = text.split_whitespace().collect();
    let n = words.len();
    if n < min_reps { return None; }
    // period length p from 1 up: the last p words, repeated backward as far as they match.
    for p in 1..=(n / min_reps) {
        let period = &words[n - p..];
        let mut reps = 1usize;
        let mut i = n - p;
        while i >= p && words[i - p..i] == *period {
            reps += 1;
            i -= p;
        }
        if reps >= min_reps {
            return Some((period.iter().map(|s| s.to_string()).collect(), reps));
        }
    }
    None
}

/// 4-gram repeat ratio over the words (0 = all distinct .. ~1 = locked). Mirrors genguard's metric so the
/// two guards agree; kept here so this module stands alone.
pub fn ngram_repeat_ratio(text: &str, n: usize) -> f64 {
    let n = n.max(1);
    let w: Vec<&str> = text.split_whitespace().collect();
    if w.len() < n { return 0.0; }
    let (mut seen, mut rep, mut tot) = (HashSet::new(), 0usize, 0usize);
    for g in w.windows(n) {
        tot += 1;
        if !seen.insert(g.to_vec()) { rep += 1; }
    }
    if tot == 0 { 0.0 } else { rep as f64 / tot as f64 }
}

/// A one-shot severity read over a recent window of text. `tail_cycle` (a hard loop) dominates; otherwise
/// the n-gram ratio grades it. Thresholds tuned for the small-model failure modes, not prose that happens
/// to reuse words.
pub fn severity_of(recent: &str) -> Severity {
    if tail_cycle(recent, 3).is_some() { return Severity::Looping; }
    let r = ngram_repeat_ratio(recent, 4);
    if r >= 0.5 { Severity::Looping } else if r >= 0.28 { Severity::Warming } else { Severity::Clean }
}

// ── streaming monitor: track worsening across a growing reply ─────────────────────────────────────

/// Fed the GROWING reply text each step; tracks how long it's been degenerate so recovery escalates
/// (nudge first, cut only once it's clearly stuck). Bounded work: it only inspects a recent window.
#[derive(Clone, Debug)]
pub struct RepeatMonitor {
    window_words: usize,   // how many trailing words to inspect
    warming_streak: usize, // consecutive Warming/Looping observations
    bumped: bool,          // already applied a penalty bump this reply
    cut_after: usize,      // Looping observations before we recommend Cut
}

impl Default for RepeatMonitor {
    fn default() -> Self { Self { window_words: 400, warming_streak: 0, bumped: false, cut_after: 2 } }
}

impl RepeatMonitor {
    pub fn new(window_words: usize, cut_after: usize) -> Self {
        Self { window_words: window_words.max(16), cut_after: cut_after.max(1), ..Default::default() }
    }

    fn recent<'a>(&self, text: &'a str) -> String {
        let w: Vec<&str> = text.split_whitespace().collect();
        let start = w.len().saturating_sub(self.window_words);
        w[start..].join(" ")
    }

    /// Observe the current full reply; return the severity and the recommended recovery. A hard tail-loop
    /// (or sustained looping) escalates to `Cut`; early warming gets ONE penalty bump, then rides.
    pub fn observe(&mut self, full_text: &str) -> (Severity, Recovery) {
        let recent = self.recent(full_text);
        let sev = severity_of(&recent);
        match sev {
            Severity::Clean => { self.warming_streak = 0; (sev, Recovery::None) }
            Severity::Warming => {
                self.warming_streak += 1;
                if !self.bumped { self.bumped = true; (sev, Recovery::BumpPenalty(0.15)) }
                else { (sev, Recovery::None) }
            }
            Severity::Looping => {
                self.warming_streak += 1;
                // A verbatim tail-cycle is an immediate cut; a high n-gram ratio gets one grace step.
                if tail_cycle(&recent, 3).is_some() || self.warming_streak >= self.cut_after {
                    (sev, Recovery::Cut)
                } else if !self.bumped {
                    self.bumped = true; (sev, Recovery::BumpPenalty(0.2))
                } else {
                    (sev, Recovery::None)
                }
            }
        }
    }

    /// Reset for a fresh reply.
    pub fn reset(&mut self) { self.warming_streak = 0; self.bumped = false; }
}

// ── turn-level guard for the agent: escalate strikes against a repeating bad actor ───────────────

/// What to do with an agent turn that looks like a loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopAction {
    /// Productive/novel — proceed.
    Proceed,
    /// Repeating — inject this nudge into the prompt and give it another shot.
    Nudge(&'static str),
    /// Persistent bad actor — abort the run so it doesn't burn the whole budget.
    Abort,
}

/// Tracks whether an agent is going in circles: the same turn signature (or an internally-looping turn)
/// recurring. Escalates strikes → a nudge, then an abort. Reset when a genuinely new, productive turn
/// lands — so an occasional repeat doesn't trip it, but a stuck model is caught fast.
#[derive(Clone, Debug)]
pub struct LoopGuard {
    recent_sigs: Vec<u64>,
    /// Recent turns' word-shingle sets (for NEAR-duplicate detection): a model looping on the same
    /// task re-emits semantically identical turns with minor wording drift, which dodges the exact-hash
    /// `recent_sigs` check. Jaccard overlap against these catches "same block, reworded" repeats.
    recent_shingles: Vec<std::collections::HashSet<u64>>,
    /// path-hash → (last content-hash, write count) for the file-write churn guard.
    writes: std::collections::HashMap<u64, (u64, u32)>,
    strikes: usize,
    max_strikes: usize,
    keep: usize,
}

impl Default for LoopGuard {
    fn default() -> Self { Self { recent_sigs: Vec::new(), recent_shingles: Vec::new(), writes: std::collections::HashMap::new(), strikes: 0, max_strikes: 3, keep: 6 } }
}

impl LoopGuard {
    pub fn new(max_strikes: usize) -> Self { Self { max_strikes: max_strikes.max(1), ..Default::default() } }
    pub fn strikes(&self) -> usize { self.strikes }

    /// A stable signature for a turn: the normalized text + the action taken. Two turns that do the same
    /// thing hash the same even if whitespace/casing differ.
    fn signature(model_text: &str, action_sig: &str) -> u64 {
        let norm: String = model_text.to_ascii_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
        // FNV-1a over normalized text + action
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in norm.bytes().chain(b"|".iter().copied()).chain(action_sig.bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    /// Record a strike from an external detector (e.g. the file-write churn guard) so it escalates on the
    /// same counter as text/action repeats.
    pub fn note_strike(&mut self) -> LoopAction {
        self.strikes += 1;
        if self.strikes >= self.max_strikes { LoopAction::Abort } else { LoopAction::Proceed }
    }

    /// File-write churn guard — catches the loop the text/action signature CANNOT: a model rewriting the
    /// same path every turn with slightly different prose around it (text differs → signature differs →
    /// no repeat detected → "writes files multiple times forever, even if coherent"). Two triggers:
    /// an IDENTICAL rewrite (semantic no-op — the model re-emitted a file it already wrote), or the same
    /// path written `4+` times in one run regardless of content (churn). Returns a nudge message when hit.
    pub fn observe_write(&mut self, path: &str, contents: &str) -> Option<&'static str> {
        let ph = Self::signature(path, "");
        let ch = Self::signature(contents, "");
        let e = self.writes.entry(ph).or_insert((0u64, 0u32));
        e.1 += 1;
        let identical = e.0 == ch && e.1 > 1;
        e.0 = ch;
        if identical {
            Some("You re-wrote a file with IDENTICAL contents to what it already has. That is a no-op loop. Do not write this file again — verify it (list/read/build), move to the next step, or reply TASK_COMPLETE.")
        } else if e.1 >= 4 {
            Some("You have rewritten the same file 4+ times this run. Stop churning it: verify the current contents (read/build/test), then take a DIFFERENT step or reply TASK_COMPLETE.")
        } else { None }
    }

    /// Observe a completed turn. `action_sig` distinguishes what the turn DID (e.g. the tool calls it made,
    /// or "" for a no-op turn). Returns the escalating action.
    pub fn observe(&mut self, model_text: &str, action_sig: &str) -> LoopAction {
        // An internally-looping turn (the reply itself is a runaway) counts as a strike on its own.
        let internal_loop = tail_cycle(model_text, 3).is_some();
        let sig = Self::signature(model_text, action_sig);
        let exact_repeat = self.recent_sigs.contains(&sig);
        // NEAR-duplicate: a semantically-identical turn with minor wording drift (the "add more blocks"
        // loop — same block regenerated each turn at temp>0, dodging the exact hash). If this turn's
        // word-shingles overlap ≥0.8 with any recent turn, it's a repeat even without an exact match.
        let shingles = Self::shingles(model_text);
        let near_repeat = !shingles.is_empty()
            && self.recent_shingles.iter().any(|prev| jaccard(&shingles, prev) >= 0.6);
        let repeated = exact_repeat || near_repeat;

        self.recent_sigs.push(sig);
        if self.recent_sigs.len() > self.keep {
            let drop = self.recent_sigs.len() - self.keep;
            self.recent_sigs.drain(..drop);
        }
        self.recent_shingles.push(shingles);
        if self.recent_shingles.len() > self.keep {
            let drop = self.recent_shingles.len() - self.keep;
            self.recent_shingles.drain(..drop);
        }

        if repeated || internal_loop {
            self.strikes += 1;
            if self.strikes >= self.max_strikes {
                LoopAction::Abort
            } else if internal_loop {
                LoopAction::Nudge("You are repeating the same text. STOP and do something different: emit a concrete tool call that makes progress, or reply TASK_COMPLETE if done.")
            } else {
                LoopAction::Nudge("You just repeated a previous step. Do NOT repeat it — take a different, concrete action toward the goal, or reply TASK_COMPLETE if finished.")
            }
        } else {
            // a novel, productive turn forgives one accumulated strike (don't punish a one-off wobble)
            self.strikes = self.strikes.saturating_sub(1);
            LoopAction::Proceed
        }
    }

    /// Word-level BIGRAM shingle set (FNV-hashed) for a turn — the fingerprint the near-duplicate check
    /// compares. Bigrams (not 4-grams): a reworded-but-identical turn keeps most of its adjacent word
    /// pairs, so bigram Jaccard stays high (~0.7) while genuinely different turns score ~0; 4-grams are
    /// too brittle (a single inserted word breaks four of them). Lowercased; empty for trivially-short
    /// text (nothing to compare).
    fn shingles(text: &str) -> std::collections::HashSet<u64> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut set = std::collections::HashSet::new();
        if words.len() < 2 { return set; }
        for w in words.windows(2) {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for tok in w {
                for b in tok.to_ascii_lowercase().bytes().chain(b" ".iter().copied()) {
                    h ^= b as u64; h = h.wrapping_mul(0x100_0000_01b3);
                }
            }
            set.insert(h);
        }
        set
    }
}

/// Jaccard overlap between two shingle sets: |A∩B| / |A∪B|, in 0..1. 1.0 = identical shingles.
fn jaccard(a: &std::collections::HashSet<u64>, b: &std::collections::HashSet<u64>) -> f64 {
    if a.is_empty() || b.is_empty() { return 0.0; }
    let inter = a.iter().filter(|x| b.contains(x)).count();
    let union = a.len() + b.len() - inter;
    if union == 0 { 0.0 } else { inter as f64 / union as f64 }
}

// ── tests ───────────────────────────────────────────────────────────────────────────────────────


// ── improved recognition: line-, char-, and counter-normalized cycles ────────────────────────────
//
// tail_cycle is word-level, which misses three real failure modes seen in the wild:
//   * STANZA loops — a multi-line code/JSON block repeating (long word period, but a SHORT line
//     period), e.g. the same 4-line struct emitted forever;
//   * GLYPH runs — whitespace-free character loops ("``````````", "————", "abcabc...") that
//     word-splitting collapses into one token;
//   * COUNTER loops — the same sentence with only a number ticking ("Step 3: retry the fetch."
//     over and over with 3→4→5), which verbatim matching never sees.
// Each has its own conservative thresholds; severity_of_v2 composes them.

/// Trailing LINE cycle: the last `p` (trimmed) lines repeat `>= min_reps` times.
/// Returns (period_in_lines, reps). Blank-only periods are ignored.
pub fn line_tail_cycle(text: &str, min_reps: usize) -> Option<(usize, usize)> {
    let min_reps = min_reps.max(2);
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
    let n = lines.len();
    if n < min_reps { return None; }
    for p in 1..=(n / min_reps).min(12) {
        let period = &lines[n - p..];
        if period.iter().all(|l| l.trim().is_empty()) { continue; }
        let mut reps = 1usize;
        let mut i = n - p;
        while i >= p && lines[i - p..i] == *period { reps += 1; i -= p; }
        if reps >= min_reps { return Some((p, reps)); }
    }
    None
}

/// Trailing CHARACTER cycle over the raw tail (last 512 chars): a period of 1..=`max_period`
/// chars repeated `>= min_reps` times. Catches glyph runs and separator spam that have no
/// whitespace for the word detector to split on.
pub fn char_tail_cycle(text: &str, min_reps: usize, max_period: usize) -> Option<(String, usize)> {
    let min_reps = min_reps.max(2);
    let tail: Vec<char> = text.chars().rev().take(512).collect::<Vec<_>>().into_iter().rev().collect();
    let n = tail.len();
    for p in 1..=max_period.min(n / min_reps.max(1)) {
        let period = &tail[n - p..];
        if period.iter().all(|c| c.is_whitespace()) { continue; }
        let mut reps = 1usize;
        let mut i = n - p;
        while i >= p && tail[i - p..i] == *period { reps += 1; i -= p; }
        if reps >= min_reps { return Some((period.iter().collect(), reps)); }
    }
    None
}

/// Counter-normalized word cycle: digits fold to '#' before the word-level scan, so a sentence
/// repeating with only a number ticking is caught. Deliberately STRICT — period >= 4 words and
/// reps >= 4 — so legitimate numbered lists (different words per item) never match.
pub fn counter_tail_cycle(text: &str, min_reps: usize) -> Option<(Vec<String>, usize)> {
    let norm: String = text.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    match tail_cycle(&norm, min_reps.max(4)) {
        Some((period, reps)) if period.len() >= 4 => Some((period, reps)),
        _ => None,
    }
}

/// severity_of, composed over ALL detectors — the full read for realtime callers.
pub fn severity_of_v2(recent: &str) -> Severity {
    if tail_cycle(recent, 3).is_some() { return Severity::Looping; }
    if line_tail_cycle(recent, 3).is_some() { return Severity::Looping; }
    if char_tail_cycle(recent, 12, 8).is_some() { return Severity::Looping; }
    if counter_tail_cycle(recent, 4).is_some() { return Severity::Looping; }
    let r = ngram_repeat_ratio(recent, 4);
    if r >= 0.5 { Severity::Looping } else if r >= 0.28 { Severity::Warming } else { Severity::Clean }
}

// ── post-hoc analysis + automatic context condensation ───────────────────────────────────────────

/// What a finished message's repetition looked like — for a badge, a HUD note, and the
/// context-cleanup marker.
#[derive(Clone, Debug)]
pub struct RepetitionReport {
    pub kind: &'static str,   // "phrase" | "lines" | "glyphs" | "counter" | "paragraph"
    pub sample: String,       // one instance of the repeating unit (clipped)
    pub reps: usize,
    pub saved_chars: usize,   // how much condensation removed
}

fn clip(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n { out.push('…'); }
    out
}

/// Is the END of `text` inside an unterminated ``` fence? Condensation never edits code.
fn in_open_fence(text: &str) -> bool { text.matches("```").count() % 2 == 1 }

/// Analyze a FINISHED message. Returns the strongest repetition found, if any.
pub fn analyze(text: &str) -> Option<RepetitionReport> {
    if !in_open_fence(text) {
        if let Some((p, reps)) = line_tail_cycle(text, 3) {
            if reps >= 3 {
                let lines: Vec<&str> = text.lines().collect();
                let sample = lines[lines.len() - p..].join("\n");
                return Some(RepetitionReport { kind: "lines", sample: clip(&sample, 80), reps, saved_chars: 0 });
            }
        }
    }
    if let Some((period, reps)) = tail_cycle(text, 4) {
        return Some(RepetitionReport { kind: "phrase", sample: clip(&period.join(" "), 60), reps, saved_chars: 0 });
    }
    if let Some((gl, reps)) = char_tail_cycle(text, 16, 8) {
        return Some(RepetitionReport { kind: "glyphs", sample: clip(&gl, 16), reps, saved_chars: 0 });
    }
    if let Some((period, reps)) = counter_tail_cycle(text, 4) {
        return Some(RepetitionReport { kind: "counter", sample: clip(&period.join(" "), 60), reps, saved_chars: 0 });
    }
    None
}

/// Condense a message for CONTEXT: collapse a trailing cycle to ONE instance plus a marker, and
/// collapse runs of identical consecutive paragraphs. Fence-aware — anything touching ``` fences
/// is left byte-for-byte (repeated lines are the fabric of code). The user-visible message is
/// never touched; this only shapes what the model re-reads.
pub fn condense(text: &str) -> (String, Option<RepetitionReport>) {
    let mut out = text.to_string();
    let mut report: Option<RepetitionReport> = None;
    // 1) trailing LINE cycle (outside fences)
    if !in_open_fence(&out) {
        if let Some((p, reps)) = line_tail_cycle(&out, 3) {
            let lines: Vec<String> = out.lines().map(|l| l.to_string()).collect();
            let period = lines[lines.len() - p..].join("\n");
            if !period.contains("```") {
                let cut = lines.len() - p * (reps - 1);
                let saved0 = out.len();
                out = lines[..cut].join("\n");
                out.push_str(&format!("\n⟲ the previous {} line(s) repeated ×{} — condensed for context\n", p, reps));
                let saved = saved0.saturating_sub(out.len());
                report = Some(RepetitionReport { kind: "lines", sample: clip(&period, 80), reps, saved_chars: saved });
            }
        }
    }
    // 2) trailing WORD cycle (only if nothing collapsed yet)
    if report.is_none() && !in_open_fence(&out) {
        if let Some((period, reps)) = tail_cycle(&out, 4) {
            let pstr = period.join(" ");
            if !pstr.contains("```") && pstr.len() >= 8 {
                if let Some(first) = out.find(&pstr) {
                    let saved = out.len().saturating_sub(first + pstr.len());
                    let head = out[..first].trim_end().to_string();
                    out = format!("{}\n{} ⟲ (this phrase repeated ×{} — condensed)\n", head, pstr, reps);
                    report = Some(RepetitionReport { kind: "phrase", sample: clip(&pstr, 60), reps,
                                                     saved_chars: saved });
                }
            }
        }
    }
    // 3) identical consecutive PARAGRAPHS (only in fully fence-free text)
    if !out.contains("```") {
        let paras: Vec<String> = out.split("\n\n").map(|p| p.to_string()).collect();
        if paras.len() >= 3 {
            let mut kept: Vec<String> = Vec::new();
            let (mut i, mut collapsed) = (0usize, 0usize);
            let mut first_sample = String::new();
            while i < paras.len() {
                let mut j = i + 1;
                while j < paras.len() && paras[j].trim() == paras[i].trim() && !paras[i].trim().is_empty() { j += 1; }
                kept.push(paras[i].clone());
                if j - i >= 2 {
                    kept.push(format!("⟲ (paragraph repeated ×{} — condensed)", j - i));
                    collapsed += (j - i - 1) * paras[i].len();
                    if first_sample.is_empty() { first_sample = paras[i].clone(); }
                }
                i = j;
            }
            if collapsed > 0 {
                out = kept.join("\n\n");
                if report.is_none() {
                    report = Some(RepetitionReport { kind: "paragraph", sample: clip(&first_sample, 60), reps: 0, saved_chars: collapsed });
                }
            }
        }
    }
    (out, report)
}

#[cfg(test)]
mod write_guard_tests {
    use super::*;
    #[test]
    fn identical_rewrite_and_churn_flagged() {
        let mut g = LoopGuard::default();
        assert!(g.observe_write("src/lib.rs", "fn a(){}").is_none(), "first write is fine");
        assert!(g.observe_write("src/lib.rs", "fn a(){}").is_some(), "identical rewrite = no-op loop");
        assert!(g.observe_write("src/lib.rs", "fn b(){}").is_none(), "changed content, 3rd write, below churn cap");
        assert!(g.observe_write("src/lib.rs", "fn c(){}").is_some(), "4th write = churn even with new content");
        assert!(g.observe_write("src/main.rs", "fn main(){}").is_none(), "other paths unaffected");
        // strikes escalate through the same counter
        let mut g2 = LoopGuard::new(2);
        assert!(matches!(g2.note_strike(), LoopAction::Proceed));
        assert!(matches!(g2.note_strike(), LoopAction::Abort));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_cycle_catches_verbatim_loops() {
        assert_eq!(tail_cycle("hello the value the value the value", 3).map(|(p, r)| (p, r)),
                   Some((vec!["the".into(), "value".into()], 3)));
        // single-word runaway
        assert!(tail_cycle("ok ok ok ok ok", 3).is_some());
        // healthy prose → no cycle
        assert!(tail_cycle("the quick brown fox jumps over the lazy dog", 3).is_none());
        // shortest period preferred
        let (p, _) = tail_cycle("a b a b a b a b", 3).unwrap();
        assert_eq!(p, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn severity_grades_correctly() {
        assert_eq!(severity_of("a well formed sentence with varied distinct words throughout it"), Severity::Clean);
        assert_eq!(severity_of(&"struct S a u8 b u16 struct S a u8 b u16 struct S a u8 b u16 ".repeat(3)), Severity::Looping);
    }

    #[test]
    fn monitor_bumps_then_cuts() {
        let mut m = RepeatMonitor::new(400, 2);
        // clean → nothing
        assert_eq!(m.observe("the program reads a file and prints each line to stdout").1, Recovery::None);
        // a hard verbatim loop → immediate cut
        let loopy = format!("done {}", "the value the value the value the value ".repeat(4));
        assert_eq!(m.observe(&loopy).1, Recovery::Cut);
    }

    #[test]
    fn monitor_early_warming_bumps_penalty() {
        let mut m = RepeatMonitor::new(400, 3);
        // moderate n-gram repetition (not a hard cycle) → a one-time penalty bump, not a cut
        let warm = "set the offset then read then set the offset then check then set the offset then read then wait";
        let (_sev, rec) = m.observe(warm);
        assert!(matches!(rec, Recovery::BumpPenalty(_) | Recovery::None));
    }

    #[test]
    fn loopguard_escalates_nudge_then_abort() {
        let mut g = LoopGuard::new(3);
        // three identical turns → nudge, nudge, abort
        let a1 = g.observe("I'll write the file", "write:src/lib.rs");
        assert_eq!(a1, LoopAction::Proceed); // first sighting
        let a2 = g.observe("I'll write the file", "write:src/lib.rs");
        assert!(matches!(a2, LoopAction::Nudge(_))); // repeat → strike 1
        let a3 = g.observe("I'll write the file", "write:src/lib.rs");
        assert!(matches!(a3, LoopAction::Nudge(_))); // strike 2
        let a4 = g.observe("I'll write the file", "write:src/lib.rs");
        assert_eq!(a4, LoopAction::Abort); // strike 3 → abort the bad actor
    }

    #[test]
    fn loopguard_forgives_novel_progress() {
        let mut g = LoopGuard::new(3);
        g.observe("step one", "write:a");
        g.observe("step one", "write:a"); // strike 1
        assert_eq!(g.strikes(), 1);
        g.observe("a genuinely different next step", "run:cargo test"); // novel → forgives
        assert_eq!(g.strikes(), 0);
    }

    #[test]
    fn loopguard_catches_internal_runaway_turn() {
        let mut g = LoopGuard::new(3);
        // a single turn whose TEXT is a runaway loop is struck even if its signature is new
        let runaway = "ok ".repeat(20);
        assert!(matches!(g.observe(&runaway, "noop"), LoopAction::Nudge(_)));
    }

    #[test]
    fn loopguard_catches_reworded_near_duplicate() {
        // The "add more blocks" loop: each turn regenerates a semantically identical block with minor
        // wording drift at temp>0, so the EXACT-hash signature differs every time and the old guard
        // proceeded forever. The shingle/Jaccard check must catch it as a repeat.
        let mut g = LoopGuard::new(3);
        let a = "Here is a new block that adds a coordinator module to manage the worker pool and dispatch tasks across lanes efficiently";
        let b = "Here is a new block which adds a coordinator module to manage the worker pool and dispatch tasks across the lanes efficiently"; // one word swapped + inserted
        // exact hashes differ (proves the old path would have proceeded)
        assert_ne!(LoopGuard::signature(a, "noop"), LoopGuard::signature(b, "noop"));
        assert!(matches!(g.observe(a, "noop"), LoopAction::Proceed));
        // the near-identical follow-up is caught despite the different hash
        assert!(matches!(g.observe(b, "noop"), LoopAction::Nudge(_)), "reworded near-duplicate must strike");
    }

    #[test]
    fn loopguard_allows_genuinely_distinct_turns() {
        // don't over-trigger: two turns about different things must NOT read as near-duplicates
        let mut g = LoopGuard::new(3);
        let a = "Add a coordinator module that manages the worker pool and dispatches tasks across lanes";
        let b = "Write the KV cache eviction policy using a value-ranked heap with demote-before-drop tiers";
        assert!(matches!(g.observe(a, "noop"), LoopAction::Proceed));
        assert!(matches!(g.observe(b, "noop"), LoopAction::Proceed), "distinct turns must not strike");
        assert_eq!(g.strikes(), 0);
    }

    #[test]
    fn line_cycle_catches_stanza_loops_words_miss() {
        let stanza = "let x = 1;\nlet y = 2;\ncall(x, y);\n";
        let text = format!("prelude\n{}{}{}{}", stanza, stanza, stanza, stanza);
        assert!(line_tail_cycle(&text, 3).is_some());
    }
    #[test]
    fn char_cycle_catches_glyph_runs() {
        assert!(char_tail_cycle("done\n``````````````````````", 12, 8).is_some());
        assert!(char_tail_cycle("normal prose without runs at all here", 12, 8).is_none());
    }
    #[test]
    fn counter_cycle_ticks_but_lists_do_not() {
        let loopy = "Step 3: retry the fetch now. Step 4: retry the fetch now. Step 5: retry the fetch now. Step 6: retry the fetch now. Step 7: retry the fetch now.";
        assert!(counter_tail_cycle(loopy, 4).is_some());
        let list = "1. bake the bread 2. cool the loaf 3. slice it thin 4. toast to taste";
        assert!(counter_tail_cycle(list, 4).is_none());
    }
    #[test]
    fn condense_collapses_and_reports_but_never_touches_code() {
        let text = "answer below\n\nsame para\n\nsame para\n\nsame para\n\ntail";
        let (out, rep) = condense(text);
        assert_eq!(out.matches("same para").count(), 1, "{out}");
        assert!(rep.is_some());
        let code = "```rust\nlet a=1;\nlet a=1;\nlet a=1;\nlet a=1;\n```";
        let (out2, _) = condense(code);
        assert_eq!(out2, code, "fenced code must be untouched");
    }
    #[test]
    fn analyze_reads_a_finished_runaway() {
        let t = "the model said the value the value the value the value the value";
        assert_eq!(analyze(t).expect("phrase loop").kind, "phrase");
    }
}
