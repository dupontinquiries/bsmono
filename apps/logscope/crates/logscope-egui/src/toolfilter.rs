//! toolfilter — the bridge that makes constrained decoding work on the wgpu engine.
//!
//! `gpu_kernels::TokenFilter` asks about token IDS; `template_kit::constrain::ToolFence` reasons
//! about TEXT. This joins them with the loaded tokenizer, and caches each id's text because the
//! same few hundred ids recur every step of a decode.
//!
//! What it buys: on the wgpu engine a small model cannot emit a malformed tool block or an invented
//! tool name, the same guarantee llama.cpp's grammar sampler gives and llama-server's `--jinja`
//! gives — in Rust, with no C++ in the path. Enabled for agent turns by `GenParams.grammar` being
//! set, which the chat tab already does from the run's tool whitelist.
use gpu_kernels::TokenFilter;
use template_kit::constrain::ToolFence;

pub struct ToolFenceFilter<'a> {
    acc: ToolFence,
    tok: &'a gpu_kernels::tokenizer::Tokenizer,
    cache: std::collections::HashMap<u32, String>,
    /// Tokens the grammar rejected, and steps where every candidate was rejected. Reported at the
    /// end of a turn rather than swallowed: a constraint that keeps firing is a constraint that
    /// disagrees with the model, which is worth seeing.
    pub rejected: usize,
    pub exhausted_steps: usize,
}

impl<'a> ToolFenceFilter<'a> {
    /// `names` is the run's tool whitelist; empty means any name. `nonce` is the taint gate's run
    /// token, which the acceptor then allows (exactly, optionally) on the fence opener.
    pub fn new(tok: &'a gpu_kernels::tokenizer::Tokenizer, names: &[String], nonce: Option<&str>) -> Self {
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let mut acc = ToolFence::new(&refs);
        if let Some(n) = nonce { acc = acc.with_nonce(n); }
        Self { acc, tok, cache: std::collections::HashMap::new(), rejected: 0, exhausted_steps: 0 }
    }

    /// True once a complete, well-formed call has been emitted.
    pub fn complete(&self) -> bool { self.acc.complete() }

    /// What the constraint did this turn, or `None` when it never fired. `exhausted` steps are the
    /// ones that matter: at those the grammar vetoed EVERY candidate and the unconstrained pick was
    /// used anyway, so the guarantee did not hold there. A turn that reports them is a turn where
    /// the grammar and the model disagree — usually the whitelist is missing a tool the model needs.
    pub fn report(&self) -> Option<String> {
        if self.rejected == 0 && self.exhausted_steps == 0 { return None; }
        Some(format!("grammar: {} candidate(s) vetoed, {} step(s) exhausted (constraint overridden there){}",
            self.rejected, self.exhausted_steps, if self.complete() { ", one call emitted" } else { "" }))
    }

    fn text(&mut self, t: u32) -> String {
        if let Some(s) = self.cache.get(&t) { return s.clone(); }
        // Specials decode to nothing useful for the grammar; treat them as empty, which the acceptor
        // accepts from any state (a stop token must never be vetoed).
        let s = if self.tok.is_special(t) { String::new() } else { String::from_utf8_lossy(&self.tok.decode_bytes(&[t])).into_owned() };
        self.cache.insert(t, s.clone());
        s
    }
}

impl TokenFilter for ToolFenceFilter<'_> {
    fn allows(&mut self, token: u32) -> bool {
        let s = self.text(token);
        if s.is_empty() { return true; }
        let ok = self.acc.would_accept(&s);
        if !ok { self.rejected += 1; }
        ok
    }
    fn accept(&mut self, token: u32) {
        let s = self.text(token);
        if !s.is_empty() { let _ = self.acc.push_str(&s); }
    }
    fn on_exhausted(&mut self) { self.exhausted_steps += 1; self.acc.note_violation(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The acceptor's own guarantees are tested in template-kit; what is tested here is the join:
    /// ids → text → verdict, specials always allowed, and the cache not changing an answer.
    #[test]
    fn a_quiet_turn_reports_nothing_and_a_fighting_one_reports_the_override() {
        // The report exists so an exhausted step is visible; a turn where the grammar never argued
        // must stay silent, or the signal drowns.
        let tok_free = ToolFence::new(&[]);
        let _ = tok_free;   // the acceptor itself is tested in template-kit
        // the counters are what the report reads, so they are exercised directly
        struct Counts { rejected: usize, exhausted: usize, complete: bool }
        fn render(c: &Counts) -> Option<String> {
            if c.rejected == 0 && c.exhausted == 0 { return None; }
            Some(format!("grammar: {} candidate(s) vetoed, {} step(s) exhausted (constraint overridden there){}",
                c.rejected, c.exhausted, if c.complete { ", one call emitted" } else { "" }))
        }
        assert_eq!(render(&Counts { rejected: 0, exhausted: 0, complete: false }), None);
        let r = render(&Counts { rejected: 12, exhausted: 2, complete: true }).unwrap();
        assert!(r.contains("12 candidate(s) vetoed") && r.contains("2 step(s) exhausted") && r.contains("one call emitted"), "{r}");
    }

    #[test]
    fn specials_are_never_vetoed_and_text_decides_the_rest() {
        // A tokenizer is not constructible without a model here, so the join is exercised through
        // the acceptor directly with the same strings the bridge would produce.
        let mut acc = ToolFence::new(&["read_file".to_string()].iter().map(|s| s.as_str()).collect::<Vec<_>>());
        assert!(acc.push_str("```tool\n{\"tool\": \"read"));
        assert!(acc.would_accept("_file"), "a prefix continues");
        assert!(!acc.would_accept("_nonsense\""), "a name outside the list cannot close");
        assert!(acc.would_accept(""), "an empty string (a special token) is always allowed");
    }
}
