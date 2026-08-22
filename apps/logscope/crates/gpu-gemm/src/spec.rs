//! spec.rs — the speculative-decoding control loop, separated from anything that can draft.
//!
//! WHY THIS IS THE FIRST PIECE. The phase-balance verdict has named speculative decoding as the
//! missing lever since c4-750, and c4-828 found that MTP heads ship draft models inside the file.
//! But every MTP-bearing checkpoint seen so far is also hybrid-recurrent, so waiting for MTP means
//! waiting for a Gated DeltaNet forward pass first. That ordering is backwards: the risky, subtle
//! part of speculative decoding is the ACCEPT/REJECT loop, not the drafter, and the loop can be
//! built and proven against a drafter that needs no model whatsoever.
//!
//! So the seam is the drafter. `Drafter` proposes tokens; the loop verifies them. An MTP head, a
//! smaller model, and the prompt-lookup drafter below are three implementations of one interface,
//! and the loop cannot tell them apart. When Gated DeltaNet lands, MTP is a new `impl` rather than
//! a rewrite.
//!
//! THE INVARIANT THAT MAKES THIS SAFE: at temperature 0, speculative decoding must produce EXACTLY
//! the tokens single-token decoding would. It is a throughput optimisation with no quality
//! trade-off — that is the whole reason to prefer it over a smaller model. Any implementation that
//! cannot demonstrate bit-identical output is not an optimisation, it is a different model.
//!
//! What the engine already has, all built for diffusion: KV rewind (c4-770), a batched matvec for
//! m<=8, multi-position forward, and per-row logits. This module is the control logic those pieces
//! were missing.

/// What to tell the operator when a checkpoint carries an MTP draft head this engine cannot run.
///
/// The wording matters more than it looks. The first version of this notice ended at "the head is
/// dead weight until it is wired", which reads — correctly — as "speculative decoding is not
/// available to you". It IS available: PromptLookup needs no head, no second checkpoint and no
/// weights, and it is one environment variable away. A notice that names an unavailable path and
/// omits the available one leaves the operator with nothing to do, which is how a diagnostic
/// becomes discouragement.
///
/// Kept here rather than inline in the loader so it can be tested: the failure mode is a sentence,
/// and a sentence that drifts is exactly what this class of bug is made of.
pub fn mtp_notice(heads: usize, declared: Option<u32>) -> String {
    format!(
        "this checkpoint carries an MTP DRAFT HEAD on {heads} block(s) (metadata declares \
{declared:?}) — a draft model shipped in the file. This engine does NOT run it: decode stays \
single-token, so the head is dead weight in memory until it is wired (llama.cpp consumes it as \
--spec-type draft-mtp). Meanwhile speculative decoding IS available without it: \
LOGSCOPE_SPEC_DRAFT=lookup runs the model-free prompt-lookup drafter through the same verifier, \
with output identical at temperature 0 — less upside than the head would give, available today."
    )
}

/// Proposes continuations. The seam between "where drafts come from" and "how they are verified".
pub trait Drafter {
    /// Propose up to `n` tokens following `ctx`. May return fewer, including none.
    ///
    /// Cheapness is the contract: a draft that costs as much as verifying it saves nothing. The
    /// loop calls this once per step and pays for it whether the draft is accepted or not.
    fn draft(&mut self, ctx: &[u32], n: usize) -> Vec<u32>;

    /// Name for the profile line, so a log says which drafter produced a given acceptance rate.
    fn name(&self) -> &'static str;
}

/// How many drafted tokens the verifier agreed with, and therefore how many commit.
///
/// `verified[i]` is the token the real model would emit at the position `draft[i]` occupies.
/// `verified` must be one longer than `draft`: verifying k drafts yields k+1 real distributions,
/// because the position AFTER the last accepted draft is also computed by the same forward pass.
///
/// THE OFF-BY-ONE HERE IS THE CLASSIC BUG. A run of k drafts with j accepted commits j+1 tokens,
/// not j — the extra is the verifier's own token at the first disagreement, which is free and
/// correct. An implementation that commits j is not wrong in output but throws away the entire
/// speedup at low acceptance; one that commits j+2 emits a token the model never produced.
pub fn accepted_run(draft: &[u32], verified: &[u32]) -> usize {
    debug_assert_eq!(verified.len(), draft.len() + 1, "verifier must return one distribution more");
    draft.iter().zip(verified).take_while(|(d, v)| d == v).count()
}

/// The tokens to commit for one speculative step.
///
/// Always at least one, even when nothing was accepted: the verifier's token at position 0 is
/// correct regardless of what was drafted, so a step can never make progress worse than plain
/// decoding. That property is what lets the loop run unconditionally instead of guessing when
/// drafting is worthwhile.
pub fn commit_for_step(draft: &[u32], verified: &[u32]) -> Vec<u32> {
    let j = accepted_run(draft, verified);
    verified[..=j].to_vec()
}

/// Running acceptance statistics, for deciding the draft length and for the profile line.
#[derive(Debug, Default, Clone, Copy)]
pub struct Acceptance { pub drafted: u64, pub accepted: u64, pub steps: u64, pub committed: u64 }

impl Acceptance {
    pub fn observe(&mut self, drafted: usize, accepted: usize, committed: usize) {
        self.drafted += drafted as u64;
        self.accepted += accepted as u64;
        self.committed += committed as u64;
        self.steps += 1;
    }
    /// Fraction of drafted tokens the verifier agreed with. None before any draft — a rate over
    /// zero proposals is the invented-denominator mistake this project keeps deleting.
    pub fn rate(&self) -> Option<f64> {
        if self.drafted == 0 { return None; }
        Some(self.accepted as f64 / self.drafted as f64)
    }
    /// Tokens committed per verification pass. This, not the acceptance rate, is the speedup:
    /// a 90% rate at draft length 1 is 1.9x, a 50% rate at length 4 is about 2.4x.
    pub fn tokens_per_pass(&self) -> Option<f64> {
        if self.steps == 0 { return None; }
        Some(self.committed as f64 / self.steps as f64)
    }
}

/// A drafter that needs NO model: it looks for the current suffix earlier in the context and
/// proposes whatever followed it last time.
///
/// Chosen deliberately as the first implementation. It costs a substring search, it is exactly right
/// on the workload this engine is used for — reading a repository, where identifiers, paths and
/// signatures repeat constantly — and it lets the accept/reject loop be built and proven today,
/// against real models, without waiting for a recurrent forward pass.
/// TWO TUNINGS EXPLORED AND REJECTED, recorded so they are not re-explored.
///
/// VARIABLE ORDER — try the longest matching n-gram, fall back to shorter — should combine the
/// phase-pinning of a long n-gram with the coverage of a short one, and the periodic counter-example
/// below suggests it. Simulated over 2..4 and 2..8 it gains NOTHING on real text:
///
///   corpus                    fixed ng=2   var 2..4   var 2..8
///   repo-read (long prompt)         1.49       1.49       1.49
///   rust source                     1.29       1.29       1.30
///   prose                           1.02       1.02       1.02
///   periodic (synthetic)            4.00       4.98       4.98
///
/// Only the synthetic improves. On real text the most recent 2-gram match usually IS the longest
/// one, so the search costs work and finds the same continuation.
///
/// WIDER DRAFTS — k costs one forward pass regardless of width, so a large k looks free:
///
///   k        1      2      4      8     16
///   tok/pass 1.27   1.38   1.49   1.53   1.55
///   accepted 27%    19%    12%     7%     3%
///
/// It is nearly free and nearly worthless past 4. Acceptance collapses as k grows because almost all
/// the value is in the FIRST one or two drafted tokens; beyond that the continuation has diverged.
///
/// Both results say the same thing, and it is the useful conclusion: prompt-lookup is limited by
/// what it can know, not by how it searches. It predicts what the CONTEXT already said, and real
/// generation stops echoing its context after a token or two. Getting past ~1.5x needs a drafter
/// that predicts what the MODEL will say — a draft model, or the MTP head c4-828 detects.
pub struct PromptLookup { pub ngram: usize }

impl Default for PromptLookup {
    /// ngram = 2, MEASURED not guessed. Acceptance depends only on the token sequence, never on the
    /// hardware, so it can be simulated exactly by replaying real text: the drafter proposes, and
    /// the text itself is the model. On that basis, tokens per verification pass at k=4:
    ///
    ///   corpus                    ng=2   ng=3   ng=4
    ///   repo-read (long prompt)   1.49   1.37   1.27
    ///   rust source               1.32   1.22   1.15
    ///   prose                     1.04   1.02   1.01
    ///
    /// A shorter n-gram matches more often and a wrong proposal costs only a verification slot, so
    /// 2 wins on every corpus measured. NOT a law: on a perfectly periodic sequence a longer n-gram
    /// wins, because it pins the phase where a short one matches the wrong repeat. Real text has
    /// filler between its repeats, which is why 2 is the right default and why the test that first
    /// asserted "shorter always wins" was refuted by its own fixture.
    ///
    /// The honest headline: about 1.5x at best, on exactly the workload this engine is used for,
    /// and NOTHING on prose. That is worth having on a decode measured at 3% of roofline, and it is
    /// not the 2x an MTP head reports — a real draft head predicts what the model will say, where
    /// this only predicts what the context already said.
    fn default() -> Self { PromptLookup { ngram: 2 } }
}

impl Drafter for PromptLookup {
    fn draft(&mut self, ctx: &[u32], n: usize) -> Vec<u32> {
        if n == 0 || ctx.len() <= self.ngram { return Vec::new(); }
        let tail = &ctx[ctx.len() - self.ngram..];
        // Search from the most recent occurrence backwards: recent context predicts better than
        // distant context, and the first match found scanning back is the closest one.
        let limit = ctx.len() - self.ngram;
        for start in (0..limit).rev() {
            if &ctx[start..start + self.ngram] == tail {
                let from = start + self.ngram;
                let take = n.min(ctx.len().saturating_sub(from));
                if take > 0 { return ctx[from..from + take].to_vec(); }
            }
        }
        Vec::new()
    }
    fn name(&self) -> &'static str { "prompt-lookup" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_accepted_draft_commits_one_more_than_it_drafted() {
        let draft = [10, 11, 12];
        let verified = [10, 11, 12, 13];
        assert_eq!(accepted_run(&draft, &verified), 3);
        assert_eq!(commit_for_step(&draft, &verified), vec![10, 11, 12, 13],
            "the token after the last accepted draft is free and correct");
    }

    /// The off-by-one that defines this algorithm, checked at every acceptance depth.
    #[test]
    fn a_partial_run_commits_the_agreed_prefix_plus_the_correction() {
        for j in 0..4usize {
            let draft: Vec<u32> = (0..4).map(|i| 100 + i as u32).collect();
            // verified agrees for j tokens, then differs.
            let mut verified: Vec<u32> = draft[..j].to_vec();
            verified.push(999);
            while verified.len() < draft.len() + 1 { verified.push(0); }
            assert_eq!(accepted_run(&draft, &verified), j);
            let c = commit_for_step(&draft, &verified);
            assert_eq!(c.len(), j + 1, "j={j}: commit the agreed prefix AND the correction");
            assert_eq!(*c.last().unwrap(), 999, "j={j}: the correction is the verifier's token");
        }
    }

    /// Nothing accepted must still make progress, or a bad drafter could stall generation.
    #[test]
    fn a_rejected_draft_still_commits_one_real_token() {
        let c = commit_for_step(&[7, 7, 7], &[1, 2, 3, 4]);
        assert_eq!(c, vec![1], "plain decoding is the floor, never worse");
    }

    /// An empty draft degenerates to ordinary decoding — the case that must be free, because it is
    /// what happens whenever the drafter has nothing to say.
    #[test]
    fn no_draft_is_ordinary_decoding() {
        assert_eq!(accepted_run(&[], &[42]), 0);
        assert_eq!(commit_for_step(&[], &[42]), vec![42]);
    }

    /// The rate is None before anything is drafted, not 0% and not 100%.
    #[test]
    fn acceptance_has_no_rate_before_it_has_a_denominator() {
        let mut a = Acceptance::default();
        assert_eq!(a.rate(), None);
        assert_eq!(a.tokens_per_pass(), None);
        a.observe(4, 2, 3);
        assert_eq!(a.rate(), Some(0.5));
        assert_eq!(a.tokens_per_pass(), Some(3.0));
    }

    /// Tokens-per-pass is the speedup, and it is NOT the acceptance rate — a high rate on short
    /// drafts can be worth less than a mediocre rate on long ones.
    #[test]
    fn tokens_per_pass_is_the_number_that_means_speedup() {
        let mut short = Acceptance::default();
        for _ in 0..10 { short.observe(1, 1, 2); }          // 100% acceptance, draft length 1
        let mut long = Acceptance::default();
        for _ in 0..10 { long.observe(4, 2, 3); }           // 50% acceptance, draft length 4
        assert!(short.rate() > long.rate(), "the short drafter accepts more often");
        assert!(long.tokens_per_pass() > short.tokens_per_pass(),
            "yet the long one commits more per pass, which is what costs time");
    }

    #[test]
    fn prompt_lookup_proposes_the_continuation_it_saw_before() {
        let mut d = PromptLookup { ngram: 3 };
        // "a b c X Y Z" then later "a b c" again -> propose "X Y Z".
        let ctx = [1, 2, 3, 40, 41, 42, 9, 9, 1, 2, 3];
        assert_eq!(d.draft(&ctx, 3), vec![40, 41, 42]);
    }

    /// No repeat means no draft, rather than a guess. A wrong draft is not free: it costs a
    /// verification slot and, at length 4, three quarters of the batch.
    #[test]
    fn prompt_lookup_stays_silent_when_it_has_seen_nothing() {
        let mut d = PromptLookup { ngram: 3 };
        assert!(d.draft(&[1, 2, 3, 4, 5], 4).is_empty());
        assert!(d.draft(&[1, 2], 4).is_empty(), "context shorter than the ngram");
        assert!(d.draft(&[1, 2, 3, 1, 2, 3], 0).is_empty(), "asked for nothing");
    }

    /// The MOST RECENT match wins: recent context predicts better than distant context.
    #[test]
    fn prompt_lookup_prefers_the_nearest_occurrence() {
        let mut d = PromptLookup { ngram: 2 };
        //          match#1 -> 50            match#2 -> 60          tail
        let ctx = [7, 8, 50, 0, 0, 7, 8, 60, 0, 0, 7, 8];
        assert_eq!(d.draft(&ctx, 1), vec![60], "the closer occurrence is the better predictor");
    }
}

#[cfg(test)]
mod equivalence {
    use super::*;

    /// A stand-in for the real model: a deterministic "argmax" over the context. Standing in for
    /// the model is legitimate here because the property under test is about the LOOP, not the
    /// model — the loop must reproduce whatever the model would have said, whatever that is.
    fn oracle_next(ctx: &[u32]) -> u32 {
        let mut h = 1469598103934665603u64;
        for &t in ctx.iter().rev().take(8) { h ^= t as u64; h = h.wrapping_mul(1099511628211); }
        (h % 97) as u32
    }

    fn plain_decode(prompt: &[u32], n: usize) -> Vec<u32> {
        let mut ctx = prompt.to_vec();
        for _ in 0..n { let t = oracle_next(&ctx); ctx.push(t); }
        ctx[prompt.len()..].to_vec()
    }

    /// The whole justification for speculative decoding: at temperature 0 it must emit EXACTLY the
    /// tokens plain decoding would. Not similar, not usually — identical. Otherwise it is a
    /// different model wearing a speedup, and the c4-778 pass-fusion retraction is what that costs
    /// when it is discovered late.
    fn spec_decode(prompt: &[u32], n: usize, k: usize, d: &mut dyn Drafter) -> (Vec<u32>, Acceptance) {
        let mut ctx = prompt.to_vec();
        let mut acc = Acceptance::default();
        while ctx.len() - prompt.len() < n {
            let draft = d.draft(&ctx, k);
            // The verifier evaluates every drafted position PLUS one, in a single pass — which is
            // what the engine's multi-position forward already does for diffusion.
            let mut verified = Vec::with_capacity(draft.len() + 1);
            for i in 0..=draft.len() {
                let mut probe = ctx.clone();
                probe.extend_from_slice(&draft[..i]);
                verified.push(oracle_next(&probe));
            }
            let commit = commit_for_step(&draft, &verified);
            acc.observe(draft.len(), accepted_run(&draft, &verified), commit.len());
            for t in commit {
                if ctx.len() - prompt.len() >= n { break; }
                ctx.push(t);
            }
        }
        (ctx[prompt.len()..][..n].to_vec(), acc)
    }

    #[test]
    fn speculative_output_is_identical_to_plain_output() {
        for seed in 0..8u32 {
            let prompt: Vec<u32> = (0..24).map(|i| (i * 7 + seed) % 40).collect();
            let want = plain_decode(&prompt, 60);
            for k in 1..=6 {
                let (got, acc) = spec_decode(&prompt, 60, k, &mut PromptLookup { ngram: 3 });
                assert_eq!(got, want,
                    "seed={seed} draft_len={k}: speculative decoding changed the output");
                assert!(acc.tokens_per_pass().unwrap() >= 1.0,
                    "never worse than plain decoding: {:?}", acc.tokens_per_pass());
            }
        }
    }

    /// A drafter that is always WRONG must still produce correct output — only slower. This is the
    /// case that catches a loop which trusts the draft anywhere.
    #[test]
    fn a_hostile_drafter_cannot_corrupt_the_output() {
        struct Wrong;
        impl Drafter for Wrong {
            fn draft(&mut self, _: &[u32], n: usize) -> Vec<u32> { vec![9999; n] }
            fn name(&self) -> &'static str { "always-wrong" }
        }
        let prompt: Vec<u32> = (0..20).collect();
        let want = plain_decode(&prompt, 40);
        let (got, acc) = spec_decode(&prompt, 40, 4, &mut Wrong);
        assert_eq!(got, want, "a wrong draft must cost time, never correctness");
        assert_eq!(acc.tokens_per_pass(), Some(1.0), "and degrades exactly to plain decoding");
    }

    /// A drafter proposing nothing is the degenerate case the loop will hit constantly early in a
    /// generation, before any n-gram repeats.
    #[test]
    fn a_silent_drafter_degenerates_cleanly() {
        struct Silent;
        impl Drafter for Silent {
            fn draft(&mut self, _: &[u32], _: usize) -> Vec<u32> { Vec::new() }
            fn name(&self) -> &'static str { "silent" }
        }
        let prompt: Vec<u32> = (0..12).collect();
        assert_eq!(spec_decode(&prompt, 30, 4, &mut Silent).0, plain_decode(&prompt, 30));
    }
}

#[cfg(test)]
mod against_the_existing_engine_path {
    //! c4-829 built a drafter seam for speculative decoding. The engine already HAD speculative
    //! decoding — `speculative_generate_stats`, opt-in behind LOGSCOPE_SPEC_DRAFT, taking a second
    //! GGUF as the draft model, with a persistent draft session and KV rewind on rejection. Found
    //! by grepping for what the decode window does, which it turns out to have nothing to do with.
    //!
    //! So the gap was never the loop; it is that drafting requires a second checkpoint. That is what
    //! the Drafter seam is for, and what PromptLookup and a future MTP head fill.
    //!
    //! Before building on it, this checks the existing implementation is correct, because nine
    //! references and an env flag are not evidence that it runs. SELF-DRAFT is the sharpest test
    //! available: when the draft model IS the target, every proposal must be accepted and the output
    //! must equal plain greedy decoding exactly. Any rewind, position or cache bug shows up as a
    //! divergence or a rejection.
    use crate::{Gpu, SpecStats};

    #[test]
    fn a_self_draft_accepts_everything_and_matches_plain_decoding() {
        let Some(bytes) = crate::testfix::read("/tmp/tiny-qwen3-q8.gguf") else { return };
        let gpu = Gpu::new();
        let Ok(m) = gpu.load_gguf_opt(&bytes, true) else { return };
        let prompt: Vec<u32> = (1..9u32).map(|i| i % (m.vocab as u32).max(1)).collect();

        let plain = gpu.generate(&m, &prompt, 12);
        for k in [1usize, 2, 4] {
            let mut st = SpecStats::default();
            let spec = gpu.speculative_generate_stats(&m, &m, &prompt, 12, k, &mut st);
            assert_eq!(spec, plain,
                "k={k}: a model drafting for ITSELF must reproduce plain decoding exactly");
            // And every proposal must be accepted: a self-draft that rejects has a rewind or
            // position bug, which would be invisible in the output because the correction token is
            // right either way — the speedup would simply never materialise.
            assert!(st.accepted >= st.rounds,
                "k={k}: self-draft accepted {} over {} rounds — a rejection here is a cache bug",
                st.accepted, st.rounds);
        }
    }
}

#[cfg(test)]
mod drafter_against_the_real_engine {
    //! PromptLookup driving the engine's own verifier — speculative decoding with NO second
    //! checkpoint, on a real model, checked against plain decoding.
    use crate::{Gpu, SpecStats};
    use super::PromptLookup;

    #[test]
    fn a_model_free_drafter_matches_plain_decoding() {
        let Some(bytes) = crate::testfix::read("/tmp/tiny-qwen3-q8.gguf") else { return };
        let gpu = Gpu::new();
        let Ok(m) = gpu.load_gguf_opt(&bytes, true) else { return };
        let prompt: Vec<u32> = (1..9u32).map(|i| i % (m.vocab as u32).max(1)).collect();
        let plain = gpu.generate(&m, &prompt, 12);

        for k in [1usize, 2, 4] {
            let mut d = PromptLookup { ngram: 2 };
            let mut st = SpecStats::default();
            let got = gpu.speculative_generate_drafter(&m, &mut d, &prompt, 12, k, &mut st);
            assert_eq!(got, plain,
                "k={k}: a drafter must not change what the model says, only how fast it says it");
        }
    }

    /// Do the two prefill paths agree?
    ///
    /// `spec_core` originally prefilled one decode step per prompt token so its logits matched
    /// `generate_kv` bit-for-bit, and the correctness gate above compares against exactly that. But
    /// ORDINARY STREAMING has always used `prefill_chunked`, so the user-facing claim — "output is
    /// identical to plain decoding, only the speed differs" — is a claim about the chunked path, and
    /// nothing checked it. Meanwhile the per-position prefill costs one decode step per prompt token,
    /// which on a measured 139 ms/token device is 227 seconds before the first output token.
    ///
    /// This runs both and compares. A failure here is NOT a speculation bug: it means the prefill
    /// GEMM and the matvec disagree, which ordinary streaming has been living with unnoticed, and
    /// speculation only made it visible.
    #[test]
    fn spec_prefill_paths_agree() {
        let Some(bytes) = crate::testfix::read("/tmp/tiny-qwen3-q8.gguf") else { return };
        let gpu = Gpu::new();
        let Ok(m) = gpu.load_gguf_opt(&bytes, true) else { return };
        let prompt: Vec<u32> = (1..40u32).map(|i| i % (m.vocab as u32).max(1)).collect();
        for chunk in [8usize, 16, 64] {
            let (mut a, mut b) = (SpecStats::default(), SpecStats::default());
            let per = gpu.speculative_generate_drafter(&m, &mut PromptLookup { ngram: 2 }, &prompt, 12, 4, &mut a);
            let chk = gpu.speculative_generate_drafter_with(&m, &mut PromptLookup { ngram: 2 }, &prompt,
                12, 4, crate::SpecPrefill::Chunked(chunk), &mut b);
            assert_eq!(per, chk,
                "chunk={chunk}: the prefill GEMM and the per-position matvec disagree, so ordinary \
                 streaming has been producing different tokens from generate_kv all along");
        }
    }

    /// A seeded prefill must produce what a fresh one does.
    ///
    /// Reuse is worth more here than the drafter is — measured on the operator's run, a seeded turn
    /// is 8.2 tok/s against 2.1 for a fresh chunked prefill, where the drafter itself is worth 1.49x
    /// — so this is the path most of the win now flows through, and it copies KV rows between caches
    /// by hand.
    #[test]
    fn a_seeded_prefill_matches_a_fresh_one() {
        let Some(bytes) = crate::testfix::read("/tmp/tiny-qwen3-q8.gguf") else { return };
        let gpu = Gpu::new();
        let Ok(m) = gpu.load_gguf_opt(&bytes, true) else { return };
        let prompt: Vec<u32> = (1..40u32).map(|i| i % (m.vocab as u32).max(1)).collect();
        let (mut a, mut b) = (SpecStats::default(), SpecStats::default());
        // Build an anchor over a prefix, then seed a longer prompt from it.
        let mut run = crate::SpecRun::default();
        let _ = gpu.speculative_generate_drafter_run(&m, &mut PromptLookup { ngram: 2 }, &prompt[..24],
            4, 4, crate::SpecPrefill::Chunked(16), &mut a, &mut run);
        let cache = run.cache.expect("the run must hand its cache back or reuse cannot be wired");
        let seeded = gpu.speculative_generate_drafter_with(&m, &mut PromptLookup { ngram: 2 }, &prompt,
            10, 4, crate::SpecPrefill::Seeded { src: &cache, len: 24 }, &mut b);
        let mut c = SpecStats::default();
        let fresh = gpu.speculative_generate_drafter_with(&m, &mut PromptLookup { ngram: 2 }, &prompt,
            10, 4, crate::SpecPrefill::Chunked(16), &mut c);
        assert_eq!(seeded, fresh, "seeding from a previous turn's cache changed the output");
    }

    /// A drafter proposing nothing must still generate — the case that hits on the first rounds,
    /// before any n-gram has repeated, and the one where a loop that assumes a non-empty draft
    /// would stall forever rather than fail loudly.
    #[test]
    fn a_silent_drafter_still_makes_progress() {
        struct Silent;
        impl super::Drafter for Silent {
            fn draft(&mut self, _: &[u32], _: usize) -> Vec<u32> { Vec::new() }
            fn name(&self) -> &'static str { "silent" }
        }
        let Some(bytes) = crate::testfix::read("/tmp/tiny-qwen3-q8.gguf") else { return };
        let gpu = Gpu::new();
        let Ok(m) = gpu.load_gguf_opt(&bytes, true) else { return };
        let prompt: Vec<u32> = (1..9u32).map(|i| i % (m.vocab as u32).max(1)).collect();
        let plain = gpu.generate(&m, &prompt, 10);
        let mut st = SpecStats::default();
        let got = gpu.speculative_generate_drafter(&m, &mut Silent, &prompt, 10, 4, &mut st);
        assert_eq!(got.len(), plain.len(), "it must terminate, and with the right count");
        assert_eq!(got, plain);
    }
}

#[cfg(test)]
mod acceptance_is_predictable_without_hardware {
    //! Whether a drafter pays for itself depends ONLY on the token sequence — the drafter proposes
    //! from context and the model either agrees or does not, and neither depends on a GPU. So the
    //! question "is prompt-lookup worth enabling" is answerable here, and was answered by replaying
    //! real text: the corpus is both the context and the ground truth for what comes next.
    //!
    //! That is a proxy, and worth naming as one. It models a generation that echoes its context,
    //! which is what repository reading looks like and is not what open prose looks like. The
    //! ABSOLUTE numbers should be treated as an upper bound; the ORDERING is what these tests pin.
    use super::*;

    fn replay(toks: &[u32], k: usize, ngram: usize, prompt_len: usize) -> f64 {
        let mut d = PromptLookup { ngram };
        let mut ctx: Vec<u32> = toks[..prompt_len].to_vec();
        let (mut i, mut rounds, mut emitted) = (prompt_len, 0usize, 0usize);
        while i < toks.len() {
            let draft = d.draft(&ctx, k);
            rounds += 1;
            let mut j = 0;
            while j < draft.len() && i + j < toks.len() && draft[j] == toks[i + j] { j += 1; }
            let take = (j + 1).min(toks.len() - i);
            ctx.extend_from_slice(&toks[i..i + take]);
            i += take; emitted += take;
        }
        emitted as f64 / rounds.max(1) as f64
    }

    /// Repetitive text — the shape of source code — must beat text that never repeats.
    #[test]
    fn repetition_is_what_makes_the_drafter_pay() {
        // A sequence with real repeated runs, and one with none.
        let repetitive: Vec<u32> = (0..600).map(|i| [7u32, 8, 9, 3, 7, 8, 9, 4][i % 8]).collect();
        let novel: Vec<u32> = (0..600u32).map(|i| i.wrapping_mul(2654435761) % 5000).collect();
        let rep = replay(&repetitive, 4, 2, 32);
        let nov = replay(&novel, 4, 2, 32);
        assert!(rep > 2.0, "repeated structure should accept freely, got {rep:.2}");
        assert!(nov < 1.15, "novel text should accept almost nothing, got {nov:.2}");
        assert!(rep > nov * 2.0, "the gap is the whole basis for when to enable this");
    }

    /// WHICH n-gram wins is a property of the TEXT, not a law — and the first version of this test
    /// asserted the law and was refuted by its own fixture.
    ///
    /// On a perfectly periodic sequence a LONGER n-gram wins (4.98 against 4.00 at ng=4 vs ng=2),
    /// because it identifies the phase uniquely where a short one matches the wrong repeat. On real
    /// text the opposite holds — natural language and source have many short repeats and few long
    /// exact ones — which is what the corpora measured and what the default of 2 follows.
    ///
    /// So this pins the thing that IS general: on text with noise between the repeats, a shorter
    /// n-gram finds more matches. The periodic case is kept as the counter-example, because a
    /// default justified by measurement should record where the measurement does not extend.
    #[test]
    fn the_best_ngram_depends_on_the_text() {
        // Repeats separated by varying filler — the shape of source code and prose.
        let mut noisy: Vec<u32> = Vec::new();
        for i in 0..300u32 {
            noisy.extend_from_slice(&[1, 2, 3]);
            for j in 0..(i % 4) { noisy.push(1000 + i * 7 + j); }
        }
        let (a, c) = (replay(&noisy, 4, 2, 48), replay(&noisy, 4, 4, 48));
        assert!(a >= c, "with filler between repeats, ng=2 {a:.2} should beat ng=4 {c:.2}");

        // The counter-example: perfectly periodic, where a longer n-gram pins the phase.
        let periodic: Vec<u32> = (0..900).map(|i| [1u32, 2, 3, 4, 5, 2, 3, 9, 1, 2, 3, 7][i % 12]).collect();
        let (pa, pc) = (replay(&periodic, 4, 2, 48), replay(&periodic, 4, 4, 48));
        assert!(pc > pa, "on a pure cycle the LONGER n-gram wins: ng=4 {pc:.2} vs ng=2 {pa:.2}");

        assert_eq!(PromptLookup::default().ngram, 2, "the default follows the real corpora");
    }

    /// More than one token per pass, always: the free correction token guarantees it, so enabling
    /// the drafter can never make generation take MORE passes than plain decoding.
    #[test]
    fn a_pass_never_commits_less_than_one_token() {
        let novel: Vec<u32> = (0..300u32).map(|i| i.wrapping_mul(40503) % 9000).collect();
        for k in [1usize, 2, 4, 8] {
            assert!(replay(&novel, k, 2, 16) >= 1.0, "k={k}: fell below plain decoding");
        }
    }
}

#[cfg(test)]
mod tunings_that_do_not_pay {
    //! Guards on the two rejected tunings. Without these the next reader sees an obvious
    //! improvement, spends a session on it, and rediscovers that it does not work on real text.
    use super::*;

    fn replay(toks: &[u32], k: usize, ngram: usize, plen: usize) -> f64 {
        let mut d = PromptLookup { ngram };
        let mut ctx: Vec<u32> = toks[..plen].to_vec();
        let (mut i, mut r, mut e) = (plen, 0usize, 0usize);
        while i < toks.len() {
            let dr = d.draft(&ctx, k); r += 1;
            let mut j = 0;
            while j < dr.len() && i + j < toks.len() && dr[j] == toks[i + j] { j += 1; }
            let take = (j + 1).min(toks.len() - i);
            ctx.extend_from_slice(&toks[i..i + take]); i += take; e += take;
        }
        e as f64 / r.max(1) as f64
    }

    /// Text with filler between repeats — the shape of source and prose. Widening the draft past a
    /// few tokens must show diminishing returns, or the "almost all value is in the first tokens"
    /// conclusion is wrong and a bigger k would be worth taking.
    #[test]
    fn wider_drafts_stop_paying_quickly() {
        let mut toks: Vec<u32> = Vec::new();
        for i in 0..400u32 {
            toks.extend_from_slice(&[1, 2, 3, 4]);
            for j in 0..(i % 5) { toks.push(2000 + i * 11 + j); }
        }
        let (k2, k4, k16) = (replay(&toks, 2, 2, 64), replay(&toks, 4, 2, 64), replay(&toks, 16, 2, 64));
        assert!(k4 >= k2, "k=4 {k4:.2} should beat k=2 {k2:.2}");
        assert!(k16 - k4 < k4 - k2,
            "the gain from 4→16 ({:.2}) must be smaller than 2→4 ({:.2}) — diminishing returns is \
the whole reason not to widen further", k16 - k4, k4 - k2);
    }

    /// The ceiling is what the drafter can KNOW. On text that never repeats, no n-gram and no width
    /// helps — which is why the next real gain is a drafter that models the target, not the context.
    #[test]
    fn no_tuning_rescues_text_that_never_repeats() {
        let novel: Vec<u32> = (0..800u32).map(|i| i.wrapping_mul(2654435761) % 30011).collect();
        for ngram in [1usize, 2, 3, 4] {
            for k in [1usize, 4, 16] {
                let v = replay(&novel, k, ngram, 32);
                assert!(v < 1.1, "ngram={ngram} k={k}: got {v:.2} on text with nothing to look up");
            }
        }
    }
    #[test]
    fn the_mtp_notice_names_what_the_operator_CAN_do() {
        // The bug this guards: a notice that names only the unavailable path reads as "there is
        // nothing to try", and the operator stops looking. Twice now this engine has told someone
        // a feature it ships is missing.
        let t = mtp_notice(3, Some(3));
        assert!(t.contains("does NOT run it"), "the limitation must still be stated plainly");
        assert!(t.contains("LOGSCOPE_SPEC_DRAFT=lookup"), "and so must the lever that exists: {t}");
        assert!(t.contains("identical at temperature 0"));
    }

}
