//! Context under a budget: what the model sees this turn, and how much.
//!
//! TWO PROBLEMS, ONE ROOT
//!
//! A small model was re-prefilling the whole review prompt every
//! round, when only the verdict request was new. A big model on a
//! forty-op village was rereading forty ops per round. Both are the
//! same mistake: sending the whole thing every time.
//!
//! THE CHAIN
//!
//! A build's conversation is a chain: the system prompt, then each
//! exchange (what the model was shown, what it replied), then the
//! current turn. The engine keeps KV checkpoints along the chain and
//! prefills only what is new, so an exchange that is still in the
//! chain costs nothing to keep. That is the small-model win: keep the
//! transcript, pay for the delta.
//!
//! THE BUDGET
//!
//! A chain grows. When it would not fit the context less the reply,
//! the OLDEST exchanges are folded into one SUMMARY segment: not the
//! transcript but what happened -- "round 1: 2 ops, 241 blocks;
//! review dropped op 2, added a roof" -- written structurally, so it
//! is short, deterministic and never wrong about a number. Folding
//! changes the chain's prefix, which costs a re-prefill of the folded
//! part once; it then stays stable until the next fold. That is the
//! big-model win: complex builds without seeing the whole thing.
//!
//! GROUPED PLANS
//!
//! Beyond a dozen ops, a plan is shown GROUPED -- "6 x house
//! (cottage), 3 x path, 1 x well" -- with numbered detail only for the
//! ops the round is about. A model judging a village does not need
//! the sixth cottage's line; it needs to know there are six.

use crate::plan::Op;

/// One exchange with the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    /// What the model was shown (the user turn).
    pub user: String,
    /// What it replied.
    pub reply: String,
    /// One line of what happened as a result, for the summary: set by
    /// the host after the reply was applied.
    pub outcome: String,
}

/// The conversation for one build.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chain {
    pub exchanges: Vec<Exchange>,
    /// Exchanges already folded into `summary`, oldest first.
    pub folded: Vec<Exchange>,
    pub summary: String,
}

/// Rough tokens for a text: four characters each, which is what the
/// prompt budget elsewhere assumes.
pub fn tokens(s: &str) -> usize { s.len() / 4 + 1 }

impl Chain {
    pub fn push(&mut self, user: String, reply: String, outcome: String) {
        self.exchanges.push(Exchange { user, reply, outcome });
    }

    pub fn clear(&mut self) { *self = Chain::default(); }

    /// The prior turns to send, folded to fit: (summary or empty,
    /// exchanges to send verbatim). `budget` is tokens available for
    /// system + prior + this user turn; the caller has subtracted the
    /// reply reserve. Folds from the oldest until it fits.
    pub fn pack(&mut self, system_tokens: usize, user_tokens: usize, budget: usize) -> (Option<String>, Vec<(String, String)>) {
        let fits = |c: &Chain| -> bool {
            let prior: usize = c.exchanges.iter().map(|e| tokens(&e.user) + tokens(&e.reply)).sum();
            system_tokens + tokens(&c.summary) + prior + user_tokens <= budget
        };
        while !fits(self) && !self.exchanges.is_empty() {
            let oldest = self.exchanges.remove(0);
            self.folded.push(oldest);
            self.summary = summarize(&self.folded);
        }
        let prior = self.exchanges.iter().map(|e| (e.user.clone(), e.reply.clone())).collect();
        (if self.summary.is_empty() { None } else { Some(self.summary.clone()) }, prior)
    }
}

/// The folded exchanges as one paragraph of what happened.
pub fn summarize(folded: &[Exchange]) -> String {
    if folded.is_empty() { return String::new(); }
    let mut s = String::from("EARLIER IN THIS BUILD (summarised):");
    for (i, e) in folded.iter().enumerate() {
        let what = if !e.outcome.is_empty() { e.outcome.clone() } else {
            // no outcome recorded: the reply's shape, in one line
            let lines = e.reply.lines().filter(|l| !l.trim().is_empty()).count();
            format!("replied with {lines} line(s)")
        };
        s.push_str(&format!("\n  turn {}: {what}", i + 1));
    }
    s
}

/// A plan in words, grouped when it is long. Ops named in `detail`
/// (1-based) are listed individually; the rest are counted by kind.
pub fn plan_summary(ops: &[Op], detail: &[usize], group_above: usize) -> String {
    let named: Vec<(usize, &Op)> = ops.iter().enumerate().map(|(i, o)| (i + 1, o))
        .filter(|(_, o)| !matches!(o, Op::Spec(_))).collect();
    if named.len() <= group_above {
        return named.iter().map(|(n, o)| format!("  {n}. {}", crate::math::describe_op(o))).collect::<Vec<_>>().join("\n");
    }
    let kind = |o: &Op| -> String {
        match o {
            Op::Form { name, style, .. } => if style.is_empty() { name.clone() } else { format!("{name} ({style})") },
            Op::Tree { species, .. } => format!("tree ({species})"),
            Op::Plant { species, .. } => format!("living {species}"),
            other => {
                let d = format!("{other:?}");
                d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("op").to_lowercase()
            }
        }
    };
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (_, o) in &named {
        let k = kind(o);
        match counts.iter_mut().find(|(kk, _)| *kk == k) { Some((_, c)) => *c += 1, None => counts.push((k, 1)) }
    }
    let mut s = format!("  {} ops: {}", named.len(),
                        counts.iter().map(|(k, c)| format!("{c} x {k}")).collect::<Vec<_>>().join(", "));
    let shown: Vec<String> = named.iter().filter(|(n, _)| detail.contains(n))
        .map(|(n, o)| format!("  {n}. {}", crate::math::describe_op(o))).collect();
    if !shown.is_empty() { s.push_str("\n  in detail:\n"); s.push_str(&shown.join("\n")); }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan_at;

    #[test]
    fn a_chain_folds_the_oldest_turns_into_a_summary_to_fit() {
        let mut c = Chain::default();
        for i in 0..6 {
            c.push(format!("REVIEW round {i} {}", "x".repeat(400)), "drop 2\nadd roof @0,0 @4,4\n".into(),
                   format!("dropped op 2, added a roof ({} blocks)", 200 + i));
        }
        // plenty of room: nothing folds
        let (sum, prior) = c.pack(300, 100, 100_000);
        assert!(sum.is_none() && prior.len() == 6);
        // tight: the oldest fold until it fits
        let (sum, prior) = c.pack(300, 100, 700);
        assert!(prior.len() < 6, "some folded: {} left", prior.len());
        let sum = sum.expect("a summary of the folded");
        assert!(sum.starts_with("EARLIER IN THIS BUILD"));
        assert!(sum.contains("turn 1: dropped op 2, added a roof (200 blocks)"), "{sum}");
        assert!(!sum.contains("xxxx"), "the summary is what happened, not the transcript");
        // the survivors are the most recent
        assert!(prior.last().unwrap().0.contains("round 5"));
        // folding is stable: packing again with the same budget folds nothing more
        let n = c.exchanges.len();
        c.pack(300, 100, 700);
        assert_eq!(c.exchanges.len(), n);
    }

    #[test]
    fn a_long_plan_is_grouped_with_detail_only_where_asked() {
        let ir: String = (0..8).map(|i| format!("form house @{},0 @{},8 style=cottage\n", i * 12, i * 12 + 9)).collect::<String>()
            + "path @0,10 @96,10 w=2\npath @0,-2 @96,-2 w=2\ntree @50,20 oak h=7\ncylinder @50,-10 r=2 h=2 hollow\n";
        let (plan, _) = parse_plan_at(&ir, Some((0, 65, 0)));
        let s = plan_summary(&plan.ops, &[9, 12], 10);
        assert!(s.starts_with("  12 ops:"), "{s}");
        assert!(s.contains("8 x house (cottage)") && s.contains("2 x path"), "{s}");
        assert!(s.contains("in detail:") && s.contains("  9. ") && s.contains("  12. "), "{s}");
        assert!(!s.contains("  3. "), "the third cottage is counted, not listed: {s}");
        // a short plan is simply listed
        let (short, _) = parse_plan_at("enclose @-4,-4 @4,4 h=5\nroof @-4,-4 @4,4 gable\n", Some((0, 65, 0)));
        let t = plan_summary(&short.ops, &[], 12);
        assert!(t.contains("  1. ") && t.contains("  2. ") && !t.contains("ops:"));
    }
}
