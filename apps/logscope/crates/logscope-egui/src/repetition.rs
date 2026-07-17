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
    strikes: usize,
    max_strikes: usize,
    keep: usize,
}

impl Default for LoopGuard {
    fn default() -> Self { Self { recent_sigs: Vec::new(), strikes: 0, max_strikes: 3, keep: 6 } }
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

    /// Observe a completed turn. `action_sig` distinguishes what the turn DID (e.g. the tool calls it made,
    /// or "" for a no-op turn). Returns the escalating action.
    pub fn observe(&mut self, model_text: &str, action_sig: &str) -> LoopAction {
        // An internally-looping turn (the reply itself is a runaway) counts as a strike on its own.
        let internal_loop = tail_cycle(model_text, 3).is_some();
        let sig = Self::signature(model_text, action_sig);
        let repeated = self.recent_sigs.contains(&sig);

        self.recent_sigs.push(sig);
        if self.recent_sigs.len() > self.keep {
            let drop = self.recent_sigs.len() - self.keep;
            self.recent_sigs.drain(..drop);
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
}

// ── tests ───────────────────────────────────────────────────────────────────────────────────────

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
}
