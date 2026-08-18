//! horizon.rs — when a long conversation should be summarised instead of trimmed.
//!
//! A window that fills has two ways out. TRIM drops or reshuffles older turns to make the prompt
//! fit; COMPACT replaces the dropped span with a summary and continues. They cost about the same
//! tokens and they are not equivalent, because of the KV prefix tree.
//!
//! ## What the operator's own log shows
//!
//! Seventeen turns, Llama-3.2-3B, a 4096 window. The prompt length by turn:
//!
//! ```text
//! 1626 2046 2069 2264 2909 3222 3282 3265 2946 2819 2740 2883 2739 2906 3134 2246 2670
//! ```
//!
//! It stops growing and starts oscillating, which is trimming: the conversation got longer and the
//! prompt got shorter, so something in the middle was dropped. And the moment the middle moves, the
//! new prompt is no longer an EXTENSION of the previous one — the longest shared prefix falls back
//! to whatever survived, which here is the system block. Prefix reuse tracks it exactly:
//!
//! ```text
//! turn      1..4          5..7        8..16      one turn
//! reuse     79 78 72 78   50 49 50    55..59     93%
//! prefill   5.5 5.9 8.2   22.8 23.5   15..19s    4.3s
//! reused    1614 1619 1626 2264   1616 1622 1622  ...  2906
//! ```
//!
//! The `reused` column is the tell. It sits at ~1620 turn after turn while the prompt swings by
//! 500 tokens — the tree is not finding the previous turn at all, it is finding the SYSTEM PROMPT,
//! every time. The single turn that reused 2906 of 3134 is the one where the previous prompt really
//! was a prefix, and it prefilled in 4.3 s instead of 17.
//!
//! So trimming costs roughly 13 seconds of re-prefill per turn, indefinitely, and the fix is not a
//! bigger window: a bigger window makes the re-prefilled span larger. The fix is to stop moving the
//! prefix. A summary is written ONCE, becomes part of a new stable prefix, and every later turn
//! extends it — reuse returns to the 93% case rather than the 50% one.
//!
//! ## Growing the window is the other lever and it is usually the wrong one
//!
//! Growing buys headroom and costs KV bytes and a longer prefill, and on this device it does not
//! touch the actual limit: the roofline says 5% of spec bandwidth, so decode is waiting on the
//! driver rather than on memory. What growing DOES fix is the output ceiling — `up to 874` on turn
//! 6 against `up to 2470` on turn 1 — which is a real constraint and a different one from speed.
//! Both are represented below because choosing between them needs both.
//!
//! Pure: no engine, no tokenizer. The caller supplies measurements and a summariser.

/// One completed turn's measurements, as the engine already reports them.
#[derive(Clone, Copy, Debug)]
pub struct Turn {
    pub prompt_len: usize,
    /// Tokens the prefix tree reused. The engine prints this directly.
    pub reused: usize,
    pub prefill_s: f32,
    pub emitted: usize,
}

impl Turn {
    pub fn reuse_rate(&self) -> f32 {
        if self.prompt_len == 0 { 0.0 } else { self.reused as f32 / self.prompt_len as f32 }
    }
}

/// What to do before the next turn.
#[derive(Clone, PartialEq, Debug)]
pub enum Move {
    /// Nothing is wrong yet.
    Continue,
    /// Summarise the span after `keep_prefix` and restart the context from the summary.
    ///
    /// `keep_prefix` is the span that is already stable — typically the system block, which is what
    /// the tree keeps falling back to anyway. Summarising below it would throw away the one prefix
    /// that IS being reused.
    ///
    /// `also_grow` is set when `plan` reports that even the compacted context will not fit. The two
    /// moves are not exclusive and treating them as exclusive costs a second failed turn to find out.
    Compact { keep_prefix: usize, also_grow: Option<usize>, why: String },
    /// Raise the context window. Buys output headroom, costs KV and a longer prefill, and does
    /// nothing for a prefix that keeps moving.
    Grow { to: usize, why: String },
}

/// Reuse below this, sustained, means the tree is finding the system block rather than the previous
/// turn. Chosen from the measured split: the trimming turns sit at 49–59% and the one clean
/// extension hit 93%, so anything under two thirds is on the wrong side of a wide gap.
pub const REUSE_FLOOR: f32 = 0.66;
/// Turns of sustained low reuse before compacting. One bad turn is a topic change; three is a shape.
pub const PATIENCE: usize = 3;

pub struct Horizon {
    window: usize,
    /// Output tokens a turn should be able to produce before the window is judged too tight.
    min_headroom: usize,
    /// Length of the span known to be stable — the system block plus any committed summary.
    stable_prefix: usize,
    hist: Vec<Turn>,
}

impl Horizon {
    pub fn new(window: usize, min_headroom: usize, stable_prefix: usize) -> Self {
        Horizon { window, min_headroom, stable_prefix, hist: Vec::new() }
    }

    pub fn observe(&mut self, t: Turn) { self.hist.push(t); }
    pub fn turns(&self) -> usize { self.hist.len() }
    pub fn window(&self) -> usize { self.window }

    /// Note that a compaction happened: the summary is now part of the stable prefix.
    pub fn compacted(&mut self, new_prefix_len: usize) {
        self.stable_prefix = new_prefix_len;
        self.hist.clear();
    }

    /// Is the history being trimmed rather than appended to?
    ///
    /// A conversation that only grows produces monotonically growing prompts. A prompt that SHRANK
    /// while the conversation continued means something in the middle was dropped, and a dropped
    /// middle is what moves the prefix.
    pub fn trimming(&self) -> bool {
        self.hist.windows(2).any(|w| w[1].prompt_len < w[0].prompt_len)
    }

    /// Is the tree finding the same short anchor every turn regardless of prompt length?
    ///
    /// The strongest signal in the measured log and the one a reuse RATE alone misses: `reused`
    /// barely moves while `prompt_len` swings by hundreds of tokens. A rate can drift for benign
    /// reasons; an anchor frozen to the byte while the prompt moves means the tree is matching the
    /// system block and nothing else.
    pub fn anchor_pinned(&self) -> bool {
        let n = self.hist.len();
        if n < PATIENCE { return false; }
        let w = &self.hist[n - PATIENCE..];
        let (mn, mx) = w.iter().fold((usize::MAX, 0usize), |(a, b), t| (a.min(t.reused), b.max(t.reused)));
        let reused_spread = mx - mn;
        let (pmn, pmx) = w.iter().fold((usize::MAX, 0usize), |(a, b), t| (a.min(t.prompt_len), b.max(t.prompt_len)));
        let prompt_spread = pmx - pmn;
        // Anchor steady to within a few percent while the prompt moved several times as much.
        reused_spread * 4 < prompt_spread && reused_spread < mn / 10
    }

    /// Output tokens the next turn could produce at `prompt_len`.
    pub fn headroom(&self, prompt_len: usize) -> usize { self.window.saturating_sub(prompt_len) }

    /// Seconds of prefill a compaction would save per turn, from what has actually been measured.
    ///
    /// Prefill time over the tokens that had to be prefilled gives a per-token cost; a compacted
    /// prefix would leave only the new turn's own tokens to prefill.
    pub fn compaction_saving_s(&self, next_turn_tokens: usize) -> f32 {
        let (mut toks, mut secs) = (0usize, 0f32);
        for t in &self.hist {
            toks += t.prompt_len.saturating_sub(t.reused);
            secs += t.prefill_s;
        }
        if toks == 0 { return 0.0; }
        let per_tok = secs / toks as f32;
        let now = secs / self.hist.len().max(1) as f32;
        (now - per_tok * next_turn_tokens as f32).max(0.0)
    }

    /// As `advise`, but with the turn sizes, so a `Compact` can say whether it will be enough.
    ///
    /// `advise` alone cannot: it sees prompt lengths, not the per-turn breakdown a compaction plan
    /// needs. Callers that have the graph should use this one.
    pub fn advise_with(&self, next_prompt_len: usize, turns: &[Span]) -> Move {
        match self.advise(next_prompt_len) {
            Move::Compact { keep_prefix, why, .. } => {
                let also_grow = match plan(self.stable_prefix, turns, self.window) {
                    Some(c) if !c.fits => Some(self.window * 2),
                    Some(_) => None,
                    // Nothing worth summarising and still out of room: growth is the only move left.
                    None if self.headroom(next_prompt_len) < self.min_headroom => Some(self.window * 2),
                    None => None,
                };
                Move::Compact { keep_prefix, also_grow, why }
            }
            other => other,
        }
    }

    /// What to do before the next turn, given the prompt it would use.
    pub fn advise(&self, next_prompt_len: usize) -> Move {
        let head = self.headroom(next_prompt_len);

        // A pinned anchor is the expensive failure and the one a bigger window makes worse.
        if self.hist.len() >= PATIENCE && self.anchor_pinned() && self.trimming() {
            let rate = self.hist[self.hist.len() - 1].reuse_rate();
            return Move::Compact {
                keep_prefix: self.stable_prefix,
                also_grow: None,
                why: format!(
                    "the prefix tree has matched the same ~{} tokens for {PATIENCE} turns while the \
prompt moved by {} — it is finding the stable prefix, not the previous turn, because trimming keeps \
moving the middle. Reuse is {:.0}% and every turn re-prefills what the last one already had. A \
summary is written once and then stays a prefix; a bigger window would only enlarge the span being \
re-prefilled. Saving is about {:.1}s of prefill per turn",
                    self.hist[self.hist.len() - 1].reused,
                    self.hist.iter().map(|t| t.prompt_len).max().unwrap_or(0)
                        - self.hist.iter().map(|t| t.prompt_len).min().unwrap_or(0),
                    rate * 100.0,
                    self.compaction_saving_s(256)),
            };
        }

        // Sustained low reuse without a pinned anchor is a different problem — the conversation is
        // genuinely diverging, and compaction helps less because there is less repetition to keep.
        let low = self.hist.len() >= PATIENCE
            && self.hist[self.hist.len() - PATIENCE..].iter().all(|t| t.reuse_rate() < REUSE_FLOOR);

        if head < self.min_headroom {
            // Out of room to answer in. Compaction and growth both fix it; compaction also fixes
            // reuse, so it wins unless reuse is already healthy.
            return if low || self.trimming() {
                Move::Compact { keep_prefix: self.stable_prefix, also_grow: None, why: format!(
                    "only {head} output tokens fit under a {} window and reuse is already poor — \
compacting recovers headroom AND stops the re-prefill", self.window) }
            } else {
                Move::Grow { to: self.window * 2, why: format!(
                    "only {head} output tokens fit under a {} window, but reuse is healthy so the \
history is worth keeping intact. Growing costs KV bytes and a longer first prefill",
                    self.window) }
            };
        }

        if low {
            return Move::Compact { keep_prefix: self.stable_prefix, also_grow: None, why: format!(
                "reuse has been under {:.0}% for {PATIENCE} turns", REUSE_FLOOR * 100.0) };
        }
        Move::Continue
    }
}

/// One turn's identity and size, as the caller's conversation graph already knows them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span { pub id: usize, pub tokens: usize }

/// A concrete compaction: which turns collapse into a summary and which stay verbatim.
#[derive(Clone, PartialEq, Debug)]
pub struct Compaction {
    /// Turns to replace, oldest first. Contiguous, and never inside the stable prefix.
    pub summarize: Vec<usize>,
    /// Turns after them, kept word for word.
    pub keep_tail: Vec<usize>,
    /// Tokens the summary may use.
    pub summary_budget: usize,
    pub before_tokens: usize,
    pub after_tokens: usize,
    /// Does the compacted context actually fit the window?
    ///
    /// False means compaction alone is not enough and the window has to grow too. Saying so here is
    /// the difference between one fix and a second failed turn: an earlier draft of this function
    /// took `budget` and ignored it, under a comment claiming the caller would be told.
    pub fits: bool,
}

impl Compaction {
    pub fn saved(&self) -> usize { self.before_tokens.saturating_sub(self.after_tokens) }
}

/// Turns of recent history kept verbatim. Summarising the turn currently being worked on is what
/// makes compaction feel like amnesia, and it saves the least, because recent turns are the ones the
/// model is actually using.
pub const KEEP_TAIL: usize = 4;
/// Summary length as a fraction of what it replaces.
pub const SUMMARY_RATIO: f32 = 0.15;
/// Below this saving a compaction is not worth its cost: it moves the prefix, which forfeits the
/// prefix tree for one turn, to reclaim tokens that were not the problem.
pub const MIN_SAVING: usize = 400;

/// Plan a compaction, or `None` when one is not worth doing.
///
/// `stable` is the current stable prefix — the system block plus any summaries already committed.
/// It is never summarised: it is the one span the prefix tree IS matching, and rewriting it is how
/// a compaction makes things worse than the trimming it replaces.
pub fn plan(stable_tokens: usize, turns: &[Span], budget: usize) -> Option<Compaction> {
    if turns.len() <= KEEP_TAIL { return None; }
    let cut = turns.len() - KEEP_TAIL;
    let (head, tail) = turns.split_at(cut);
    let head_tokens: usize = head.iter().map(|t| t.tokens).sum();
    let tail_tokens: usize = tail.iter().map(|t| t.tokens).sum();
    let summary_budget = ((head_tokens as f32) * SUMMARY_RATIO).ceil() as usize;
    let before = stable_tokens + head_tokens + tail_tokens;
    let after = stable_tokens + summary_budget + tail_tokens;
    if before.saturating_sub(after) < MIN_SAVING { return None; }
    Some(Compaction {
        summarize: head.iter().map(|t| t.id).collect(),
        keep_tail: tail.iter().map(|t| t.id).collect(),
        summary_budget, before_tokens: before, after_tokens: after,
        fits: after <= budget,
    })
}

/// Does the new stable prefix EXTEND the old one?
///
/// The invariant the whole design rests on. Compaction is append-only: each pass adds a summary and
/// leaves earlier ones word for word, so the prefix only ever grows and every later turn extends it.
/// Re-rolling several summaries into one tighter summary is the obvious optimisation and it is a
/// trap — it rewrites the prefix, so the KV tree loses everything past the system block, which is
/// precisely the failure compaction was introduced to fix. Re-rolling is affordable rarely and
/// deliberately, never as a routine tidy-up.
pub fn is_append_only(prev_prefix: &[usize], next_prefix: &[usize]) -> bool {
    next_prefix.len() >= prev_prefix.len() && next_prefix[..prev_prefix.len()] == *prev_prefix
}

/// The instruction the summariser is given.
///
/// A default, not a considered product decision: it asks for decisions and open threads rather than
/// a narrative, because what the next turns need from dropped history is what was AGREED and what is
/// still outstanding, not a retelling. Worth revisiting against real transcripts.
pub fn summary_instruction(budget_tokens: usize) -> String {
    format!("Summarise the conversation so far in at most {budget_tokens} tokens. Record decisions that were made, constraints that were agreed, names of files and functions that were introduced, and questions still open. Omit pleasantries, restatements and anything already visible in the messages that follow. Write it as notes, not prose.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(prompt_len: usize, reused: usize, prefill_s: f32, emitted: usize) -> Turn {
        Turn { prompt_len, reused, prefill_s, emitted }
    }

    /// The seventeen turns from the operator's log, verbatim.
    fn measured() -> Vec<Turn> {
        vec![
            t(1626, 0, 13.39, 55),    t(2046, 1614, 5.51, 158),  t(2069, 1619, 5.88, 304),
            t(2264, 1626, 8.24, 540), t(2909, 2264, 9.99, 480),  t(3222, 1616, 22.78, 360),
            t(3282, 1622, 23.54, 546), t(3265, 1622, 23.30, 615), t(2946, 1616, 19.17, 462),
            t(2819, 1619, 16.17, 511), t(2740, 1630, 14.95, 587), t(2883, 1646, 16.97, 407),
            t(2739, 1627, 15.00, 1357), t(2906, 1623, 17.79, 352), t(3134, 2906, 4.29, 389),
            t(2246, 1619, 8.00, 300),  t(2670, 1630, 12.00, 400),
        ]
    }

    #[test]
    fn an_appending_conversation_is_not_trimming() {
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[..5] { h.observe(*x); }
        assert!(!h.trimming(), "prompts 1626..2909 only grow");
    }

    #[test]
    fn a_prompt_that_shrinks_while_the_chat_continues_is_trimming() {
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[..8] { h.observe(*x); }
        assert!(h.trimming(), "3282 -> 3265 means the middle was dropped");
    }

    #[test]
    fn a_frozen_anchor_is_detected_even_though_the_reuse_RATE_drifts() {
        // Turns 6..8: reused 1616, 1622, 1622 while prompt_len swings 3222..3282. The rate moves
        // between 49% and 50%, which alone looks like ordinary drift; the anchor being steady to
        // six tokens while the prompt moves sixty is what says the tree found the system block.
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[5..8] { h.observe(*x); }
        assert!(h.anchor_pinned());
    }

    #[test]
    fn a_genuine_extension_does_not_look_pinned() {
        let mut h = Horizon::new(4096, 512, 1626);
        for x in [t(2000, 1900, 2.0, 100), t(2200, 2000, 2.0, 100), t(2400, 2200, 2.0, 100)] {
            h.observe(x);
        }
        assert!(!h.anchor_pinned(), "the anchor tracks the prompt, which is what reuse working looks like");
    }

    #[test]
    fn the_measured_conversation_is_told_to_COMPACT_not_to_grow() {
        // The whole point. A bigger window enlarges the span being re-prefilled every turn; only a
        // stable prefix stops it.
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[..8] { h.observe(*x); }
        match h.advise(3300) {
            Move::Compact { keep_prefix, why, .. } => {
                assert_eq!(keep_prefix, 1626, "keep the span that IS being reused");
                assert!(why.contains("not the previous turn"), "{why}");
                assert!(why.contains("bigger window"), "{why}");
            }
            other => panic!("expected Compact, got {other:?}"),
        }
    }

    #[test]
    fn a_healthy_conversation_that_runs_out_of_room_is_told_to_GROW() {
        // Reuse working, history worth keeping, only headroom missing — the one case where the
        // window is the right knob.
        let mut h = Horizon::new(4096, 512, 200);
        for x in [t(3000, 2900, 2.0, 100), t(3300, 3200, 2.0, 100), t(3700, 3600, 2.0, 100)] {
            h.observe(x);
        }
        match h.advise(3800) {
            Move::Grow { to, why } => { assert_eq!(to, 8192); assert!(why.contains("reuse is healthy")); }
            other => panic!("expected Grow, got {other:?}"),
        }
    }

    #[test]
    fn a_short_healthy_conversation_is_left_alone() {
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[1..4] { h.observe(*x); }
        assert_eq!(h.advise(2300), Move::Continue);
    }

    #[test]
    fn one_bad_turn_is_a_topic_change_and_not_a_shape() {
        let mut h = Horizon::new(8192, 512, 500);
        h.observe(t(2000, 1900, 2.0, 100));
        h.observe(t(2200, 400, 6.0, 100)); // user pasted something new
        assert_eq!(h.advise(2300), Move::Continue, "PATIENCE exists so one turn cannot trigger a rewrite");
    }

    #[test]
    fn a_compact_that_will_not_be_enough_asks_to_grow_as_well() {
        // The operator's own shape: reuse collapsed AND the output ceiling fell to 874. Compaction
        // fixes the first; if the recent turns alone still overflow, only growth fixes the second,
        // and finding that out on the next failed turn is a wasted turn.
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[..8] { h.observe(*x); }
        let big = vec![Span { id: 0, tokens: 900 }, Span { id: 1, tokens: 900 },
                       Span { id: 2, tokens: 900 }, Span { id: 3, tokens: 900 },
                       Span { id: 4, tokens: 900 }, Span { id: 5, tokens: 900 }];
        match h.advise_with(3300, &big) {
            Move::Compact { also_grow: Some(to), .. } => assert_eq!(to, 8192),
            other => panic!("expected Compact wanting growth, got {other:?}"),
        }
        // Small turns compact down comfortably and need no growth.
        let small = spans(20, 100);
        match h.advise_with(3300, &small) {
            Move::Compact { also_grow: None, .. } => {}
            other => panic!("expected Compact alone, got {other:?}"),
        }
    }

    #[test]
    fn compacting_clears_the_history_and_moves_the_stable_prefix() {
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[..8] { h.observe(*x); }
        assert!(matches!(h.advise(3300), Move::Compact { .. }));
        h.compacted(1900); // system block plus a 274-token summary
        assert_eq!(h.turns(), 0);
        assert_eq!(h.advise(2000), Move::Continue, "a fresh prefix has no history to condemn it");
    }

    #[test]
    fn the_predicted_saving_comes_from_the_measured_prefill_cost() {
        let mut h = Horizon::new(4096, 512, 1626);
        for x in &measured()[5..14] { h.observe(*x); }
        // These turns re-prefilled ~1300 tokens each at ~17s. A compacted prefix leaves only the new
        // turn's own tokens, so the saving should be most of that.
        let s = h.compaction_saving_s(256);
        assert!(s > 10.0 && s < 20.0, "saving {s}s");
    }

    fn spans(n: usize, tokens: usize) -> Vec<Span> {
        (0..n).map(|i| Span { id: i, tokens }).collect()
    }

    #[test]
    fn compaction_keeps_the_recent_turns_word_for_word() {
        let c = plan(1626, &spans(20, 200), 4096).unwrap();
        assert_eq!(c.keep_tail.len(), KEEP_TAIL);
        assert_eq!(c.keep_tail, vec![16, 17, 18, 19], "the newest turns, not the oldest");
        assert_eq!(c.summarize.len(), 16);
    }

    #[test]
    fn the_stable_prefix_is_never_summarised() {
        // It is the one span the tree IS matching. Rewriting it makes compaction worse than the
        // trimming it replaces.
        let c = plan(1626, &spans(20, 200), 4096).unwrap();
        assert!(!c.summarize.is_empty());
        assert_eq!(c.before_tokens - c.after_tokens, c.saved());
        assert!(c.after_tokens >= 1626, "the stable prefix survives whole");
    }

    #[test]
    fn a_compaction_that_still_does_not_fit_says_so() {
        // Four recent turns of 900 tokens each cannot be summarised away, so a small window is still
        // exceeded afterwards and the caller has to grow as well.
        let mut t = spans(12, 900);
        for (i, s) in t.iter_mut().enumerate() { s.id = i; }
        let c = plan(1600, &t, 4096).unwrap();
        assert!(!c.fits, "after {} against a 4096 window", c.after_tokens);
        assert!(plan(1600, &t, 32768).unwrap().fits);
    }

    #[test]
    fn a_short_conversation_is_not_worth_compacting() {
        assert!(plan(1626, &spans(4, 200), 4096).is_none(), "nothing outside the tail");
        assert!(plan(1626, &spans(6, 50), 4096).is_none(), "saving under the floor");
    }

    #[test]
    fn the_saving_is_what_the_summary_does_not_spend() {
        let c = plan(1000, &spans(14, 300), 4096).unwrap();
        let head: usize = 10 * 300;
        assert_eq!(c.summary_budget, (head as f32 * SUMMARY_RATIO).ceil() as usize);
        assert_eq!(c.saved(), head - c.summary_budget);
    }

    #[test]
    fn appending_a_summary_extends_the_prefix_and_re_rolling_does_not() {
        // The trap, stated as a test. Two summaries rewritten into one is tidier and forfeits the
        // prefix tree for a turn, which is the cost compaction exists to avoid paying every turn.
        let before = vec![0usize, 1];          // system, summary #1
        assert!(is_append_only(&before, &[0, 1, 2]), "appending summary #2 extends");
        assert!(!is_append_only(&before, &[0, 9]), "re-rolling 1 and 2 into 9 rewrites");
        assert!(is_append_only(&before, &before), "no change is trivially append-only");
        assert!(!is_append_only(&before, &[0]), "a prefix cannot shrink");
    }

    #[test]
    fn the_summary_instruction_carries_its_own_budget() {
        let s = summary_instruction(240);
        assert!(s.contains("240 tokens"));
        assert!(s.contains("still open"), "outstanding threads are what the next turns need");
    }

    #[test]
    fn a_history_with_no_prefill_measurements_predicts_nothing_rather_than_dividing_by_zero() {
        let h = Horizon::new(4096, 512, 100);
        assert_eq!(h.compaction_saving_s(256), 0.0);
    }
}
