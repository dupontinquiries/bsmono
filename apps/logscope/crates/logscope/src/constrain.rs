//! constrain.rs — deciding WHEN a constraint is worth consulting, which is what makes LSP-tight
//! decoding affordable.
//!
//! ## The two constraints have costs three orders of magnitude apart
//!
//! ```text
//!   grammar DFA transition          ~0.001 ms   a table lookup
//!   vocab mask, 202k tokens         ~0.2   ms   precomputed per state
//!   rust-analyzer completion       50-2000 ms   a round trip; cold, seconds
//! ```
//!
//! Against a per-token budget of 40.7 ms (Scout at its measured best) or 12.5 ms (a 1B on llama.cpp),
//! an LSP call per token costs between five and a hundred times the entire token. Applied naively it
//! does not slow generation down, it stops it.
//!
//! ## But identifier positions are a small fraction of tokens
//!
//! The grammar already knows where an identifier can appear. So the grammar runs on every token and
//! the LSP runs only where the grammar says a NAME is expected — which turns "an LSP call per token"
//! into "an LSP call per identifier", and identifiers are a few percent of a token stream.
//!
//! ## Why this is worth more than a syntax grammar alone
//!
//! From a real 30-turn coordinator transcript, the faults that survived every iteration were
//! `ratatouille` (a crate that does not exist), and `start`, `e` and `InvalidInput` used as values that
//! were never bound. **Not one is a syntax error.** A context-free grammar accepts all of them happily;
//! they are name-resolution failures, and scope information makes them literally unsamplable rather
//! than merely detectable afterwards.
//!
//! That is the difference from `prechecks`, which finds these AFTER generation and spends a round trip
//! asking for a fix. A constraint prevents the token.

/// What the grammar says can come next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expect {
    /// Structural: punctuation, a keyword, a delimiter. The grammar alone decides.
    Syntax,
    /// A name must appear here — variable, function, type, field.
    Identifier(NameKind),
    /// Free text: a string body or comment. Nothing to constrain.
    Free,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NameKind { Value, Type, Field, CratePath }

/// Should the LSP be consulted at this position?
///
/// Only at identifier positions, and only when a scope query would actually narrow things. Asking at a
/// `Syntax` position spends 50 ms to learn what a table lookup already knew.
pub fn should_query_lsp(e: Expect) -> bool { matches!(e, Expect::Identifier(_)) }

/// Rough fraction of tokens at which the LSP would be consulted, given a token stream's shape.
///
/// The number that decides feasibility, so it is computed from counts rather than asserted.
pub fn query_fraction(expects: &[Expect]) -> f32 {
    if expects.is_empty() { return 0.0; }
    expects.iter().filter(|e| should_query_lsp(**e)).count() as f32 / expects.len() as f32
}

/// Milliseconds per token added by the constraint, given how often the LSP is consulted.
pub fn overhead_ms(fraction: f32, grammar_ms: f32, lsp_ms: f32) -> f32 {
    grammar_ms + fraction * lsp_ms
}

/// Is constrained decoding affordable at this token rate?
///
/// Returns the overhead as a fraction of the token budget. Above ~0.25 the constraint is costing more
/// than a quarter of generation speed, which is the point at which a person notices and turns it off —
/// and a correctness feature that gets turned off is worth nothing.
pub fn affordability(tok_per_s: f32, fraction: f32, grammar_ms: f32, lsp_ms: f32) -> f32 {
    if tok_per_s <= 0.0 { return f32::INFINITY; }
    let budget_ms = 1000.0 / tok_per_s;
    overhead_ms(fraction, grammar_ms, lsp_ms) / budget_ms
}

/// A cache of scope queries, keyed by position context.
///
/// The same scope is asked about repeatedly — a function body queries the same locals at every
/// identifier. Without this, `query_fraction` understates the cost by however many times the model
/// writes a name in one block.
#[derive(Clone, Debug, Default)]
pub struct ScopeCache { entries: Vec<(String, Vec<String>)>, hits: u32, misses: u32 }

impl ScopeCache {
    pub fn new() -> Self { Self::default() }

    /// Look up names for a scope key, or None to signal the caller must query.
    pub fn get(&mut self, key: &str) -> Option<&[String]> {
        if let Some(i) = self.entries.iter().position(|(k, _)| k == key) {
            self.hits += 1;
            return Some(&self.entries[i].1);
        }
        self.misses += 1;
        None
    }

    pub fn put(&mut self, key: &str, names: Vec<String>) {
        self.entries.push((key.to_string(), names));
    }

    /// Fraction of identifier positions that avoided an LSP round trip.
    pub fn hit_rate(&self) -> f32 {
        let t = self.hits + self.misses;
        if t == 0 { return 0.0; }
        self.hits as f32 / t as f32
    }

    /// Invalidate everything. Any edit can change scope, and a stale scope is worse than none — it
    /// would constrain generation to names that no longer exist while excluding ones that now do.
    pub fn clear(&mut self) { self.entries.clear(); }
}

// ── Pairing constraints with extreme quantization ───────────────────────────────────────────────
//
// The pairing is well motivated: a constraint collapses the decision from ranking 202,048 tokens to
// ranking the handful in scope, and a model that has lost precision has degraded ORDERING ability
// rather than lost all information.
//
//     unconstrained (Scout vocab)   202048 options = 17.62 bits
//     syntax-constrained                30 options =  4.91 bits
//     identifiers in scope              12 options =  3.58 bits
//
// So a scope constraint removes ~14 bits of the burden. That is why a sub-1-bit model behind a
// constraint can still emit a name that exists, when unconstrained it emits `ratatouille`.
//
// ## The failure it creates, which is worse than the one it fixes
//
// A constraint guarantees the token is VALID. It says nothing about whether it is RIGHT. Given twelve
// names in scope, an unconstrained degraded model writes something that does not compile — visibly,
// immediately, with a compiler error pointing at it. The same model behind a constraint writes
// `start_index` where `start_offset` was meant: it compiles, it runs, and it is wrong.
//
// So the pairing converts loud failures into quiet ones. That is a bad trade for a coding assistant
// unless something downstream catches semantic errors, and `prechecks` explicitly cannot — it checks
// that names RESOLVE, which is exactly the property the constraint just guaranteed.
//
// The honest position: constraints make extreme quantization SAFE TO SAMPLE FROM, not competent. They
// belong together only where a test or a type checker judges the result, and the more aggressive the
// quantization, the more that downstream judge is doing the actual work.

/// How much of the decision the constraint removes, in bits.
///
/// The argument for pairing, computed rather than asserted.
pub fn bits_removed(vocab: usize, allowed: usize) -> f32 {
    if allowed == 0 || vocab == 0 { return 0.0; }
    (vocab as f32).log2() - (allowed.max(1) as f32).log2()
}

/// Does a constrained position still require judgement the model may no longer have?
///
/// True whenever more than one option is semantically distinct. This is the check that stops
/// "constrained" being read as "safe": two valid identifiers are a choice, and a degraded model makes
/// it badly and silently.
pub fn still_requires_judgement(allowed: usize) -> bool { allowed > 1 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lsp_is_consulted_only_at_identifier_positions() {
        // Asking at a Syntax position spends 50 ms to learn what a table lookup already knew.
        assert!(should_query_lsp(Expect::Identifier(NameKind::Value)));
        assert!(should_query_lsp(Expect::Identifier(NameKind::CratePath)));
        assert!(!should_query_lsp(Expect::Syntax));
        assert!(!should_query_lsp(Expect::Free), "a string body has nothing to resolve");
    }

    #[test]
    fn a_realistic_token_mix_makes_the_lsp_affordable() {
        // The whole feasibility argument. An LSP call per token is 5x-100x the token budget; an LSP
        // call per IDENTIFIER is a small multiple of a small fraction.
        let mut mix = vec![Expect::Syntax; 85];
        mix.extend(vec![Expect::Identifier(NameKind::Value); 10]);
        mix.extend(vec![Expect::Free; 5]);
        let f = query_fraction(&mix);
        assert!((f - 0.10).abs() < 1e-5, "got {f}");
        // at 24.6 tok/s (Scout measured), a 50 ms LSP at 10% of positions
        let a = affordability(24.6, f, 0.2, 50.0);
        assert!(a < 0.25, "overhead is {:.0}% of the token budget", a * 100.0);
    }

    #[test]
    fn querying_at_EVERY_token_is_not_affordable_and_the_number_says_so() {
        // The naive design, priced. This is why the grammar has to gate the LSP rather than run
        // beside it.
        let a = affordability(24.6, 1.0, 0.2, 50.0);
        assert!(a > 1.0, "an LSP call per token exceeds the entire budget: {a:.1}x");
    }

    #[test]
    fn a_slow_language_server_is_unaffordable_even_at_ten_percent() {
        // rust-analyzer on a cold project takes seconds. The feature has to degrade rather than stall,
        // and this is the number that tells a caller to stop asking.
        let a = affordability(24.6, 0.10, 0.2, 2000.0);
        assert!(a > 1.0, "a 2 s query at 10% of positions is {a:.1}x the budget");
    }

    #[test]
    fn a_scope_constraint_removes_most_of_the_decision() {
        // Why pairing with extreme quantization is well motivated: 17.6 bits down to 3.6.
        let b = bits_removed(202048, 12);
        assert!(b > 13.0 && b < 15.0, "got {b:.1} bits");
        // and a binary choice removes nearly all of it
        assert!(bits_removed(202048, 2) > 16.0);
    }

    #[test]
    fn a_constraint_does_NOT_remove_the_need_for_judgement() {
        // The failure the pairing creates, and the reason it is not a free win. Twelve valid names is
        // still a choice: unconstrained, a degraded model writes something that does not compile —
        // loudly, with the compiler pointing at it. Constrained, it writes `start_index` where
        // `start_offset` was meant, and that compiles.
        assert!(still_requires_judgement(12));
        assert!(still_requires_judgement(2), "even a boolean is a real decision");
        // only a forced single option is judgement-free
        assert!(!still_requires_judgement(1));
    }

    #[test]
    fn the_scope_cache_counts_what_it_saved() {
        // The same scope is asked about repeatedly — a function body queries the same locals at every
        // identifier. Without the cache, the per-identifier cost is multiplied by how many names a
        // block contains.
        let mut c = ScopeCache::new();
        assert!(c.get("fn main:body").is_none());
        c.put("fn main:body", vec!["args".into(), "cfg".into()]);
        for _ in 0..9 { assert!(c.get("fn main:body").is_some()); }
        assert!((c.hit_rate() - 0.9).abs() < 1e-5, "got {}", c.hit_rate());
    }

    #[test]
    fn any_edit_clears_the_cache_because_a_stale_scope_is_worse_than_none() {
        // A stale scope would constrain generation to names that no longer exist while excluding ones
        // that now do — actively steering the model wrong, with the authority of a constraint.
        let mut c = ScopeCache::new();
        c.put("k", vec!["x".into()]);
        assert!(c.get("k").is_some());
        c.clear();
        assert!(c.get("k").is_none());
    }
}
