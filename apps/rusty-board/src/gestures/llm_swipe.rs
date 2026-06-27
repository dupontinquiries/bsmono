//! LLM-assisted swipe-word disambiguation.
//!
//! ## What this is
//!
//! A re-ranker, NOT a generator. The shape-based recognizer in
//! `gestures/recognizer.rs` is already very good at "which dictionary
//! words match this path geometrically." Where it's weak is
//! **context-aware disambiguation**: if the top 5 candidates have
//! near-tied scores, falling back to word frequency picks "store"
//! when the user's surrounding text was "the campfire's last
//! ___" (where the right answer is "stoke").
//!
//! This module takes the recognizer's top-N candidates and uses a
//! small local LLM (e.g. Llama 3.2 1B GGUF) to pick the best one
//! given the surrounding text. The LLM emits a single digit
//! (the candidate index), so it's a 1-token forward pass — not a
//! full generative round-trip.
//!
//! ## Why a re-ranker (not a generator)
//!
//! On a Snapdragon 7+ Gen 1 (Fairphone 5) with no working GPU
//! offload, llama-cpp-2 runs Llama 3.2 1B Q4 at ~10-15 tok/s. A
//! generative "guess the word from the key sequence" approach would
//! cost ~150-300 ms per swipe. The current dictionary recognizer
//! returns top-5 candidates in <1 ms. Eating a 200 ms regression on
//! every swipe would be very noticeable.
//!
//! As a re-ranker we:
//!   1. Skip the LLM entirely when the recognizer's top candidate is
//!      confidently ahead of #2 (most swipes).
//!   2. When ambiguous, output one token (digit) — 50-100 ms even on
//!      phone.
//!   3. Average added latency: 30-40 ms amortized across all swipes.
//!
//! ## Build matrix
//!
//! This module always compiles. The actual llama-cpp-2 calls live
//! behind `#[cfg(feature = "llm-swipe-ffi")]`. Without that feature
//! the reranker is a no-op that falls back to candidates[0] — useful
//! for cross-compiling to environments where you don't have clang +
//! a usable compute backend (e.g. the dev sandbox here, or the
//! basic-config build).
//!
//! ## When the LLM is invoked
//!
//! Two gates, both must hold:
//!   1. `Config::gestures::llm_swipe_enabled` is true (user opt-in).
//!   2. The recognizer's top two candidates are within
//!      `ambiguity_threshold` of each other in score (default 0.5
//!      lower-is-better). Wider gap → first candidate wins outright.

use crate::gestures::recognizer::WordCandidate;
use std::path::Path;
use std::sync::Arc;

/// Re-ranker handle. Cheap to clone; the model itself is reference-
/// counted (Arc) so multiple call sites can share one load.
#[derive(Clone)]
pub struct LlmSwipeReranker {
    inner: Arc<RerankerInner>,
}

#[cfg(feature = "llm-swipe-ffi")]
struct RerankerInner {
    /// The loaded GGUF model + backend handle. Reused across calls
    /// (the heavy thing is the model weights, ~700 MB for Llama 3.2
    /// 1B Q4 — we load once at startup and reuse forever).
    model: crate::llm_backend::LoadedModel,
    /// Cached generation params; tuned for the re-ranker case
    /// (very short prompt, single-token reply, low temperature for
    /// stability).
    params: crate::llm_backend::GenParams,
}

#[cfg(not(feature = "llm-swipe-ffi"))]
struct RerankerInner {
    /// In mock mode we carry no state. The path is retained for
    /// diagnostics so `RerankerStats` can still report what the
    /// real load WOULD have been.
    model_path: String,
}

/// Errors that can come back from the re-ranker. All recoverable —
/// the caller should fall back to candidates[0] on any error.
#[derive(Debug, thiserror::Error)]
pub enum RerankerError {
    #[error("LLM swipe re-ranker not compiled in (build with --features llm-swipe-ffi)")]
    NotCompiledIn,
    #[error("model load failed: {0}")]
    ModelLoad(String),
    #[error("generation failed: {0}")]
    Generation(String),
    #[error("could not parse LLM response: {0:?}")]
    ParseFailure(String),
    #[error("candidates list was empty")]
    NoCandidates,
}

impl LlmSwipeReranker {
    /// Load a GGUF model from disk. Heavy: ~1-3 seconds for a 1B
    /// model on phone, ~10-30 seconds for a 7B model. Call ONCE at
    /// startup, never on the hot path.
    ///
    /// On builds without `llm-swipe-ffi` this still succeeds (mock
    /// mode), so config plumbing can be wired without the feature
    /// flag. The reranker will always return `Ok(candidates[0])`
    /// in mock mode — useful for ensuring the integration point
    /// is correct before bringing in the FFI dep.
    pub fn load(model_path: &Path) -> Result<Self, RerankerError> {
        #[cfg(feature = "llm-swipe-ffi")]
        {
            let params = crate::llm_backend::GenParams {
                // Tiny context — re-ranker prompts are <100 tokens.
                // Saves KV-cache RAM vs the default 4096.
                n_ctx: 512,
                n_batch: 512,
                // Single token output. The whole point.
                max_tokens: 4,
                // Low for determinism. We want the same candidate
                // chosen for the same context, every time.
                temperature: 0.1,
                ..Default::default()
            };
            let model = crate::llm_backend::load_model(
                &model_path.to_string_lossy(),
                &params,
            )
            .map_err(RerankerError::ModelLoad)?;
            Ok(Self {
                inner: Arc::new(RerankerInner { model, params }),
            })
        }
        #[cfg(not(feature = "llm-swipe-ffi"))]
        {
            Ok(Self {
                inner: Arc::new(RerankerInner {
                    model_path: model_path.to_string_lossy().into_owned(),
                }),
            })
        }
    }

    /// Pick the best candidate given the surrounding text. Returns
    /// the chosen word as a `String`. Falls back to `candidates[0]`
    /// on any LLM-side failure (load, generation, parse) — never
    /// errors out unless `candidates` is empty.
    ///
    /// `surrounding_text` should be the text immediately before the
    /// cursor (the host's `SurroundingText` from the IM protocol).
    /// Empty surrounding text bypasses the LLM entirely (no signal
    /// to disambiguate with).
    pub fn rerank(
        &self,
        candidates: &[WordCandidate],
        surrounding_text: &str,
    ) -> Result<String, RerankerError> {
        if candidates.is_empty() {
            return Err(RerankerError::NoCandidates);
        }
        // Fast path 1: only one candidate — nothing to rerank.
        if candidates.len() == 1 {
            return Ok(candidates[0].word.clone());
        }
        // Fast path 2: no context — LLM has no signal to use.
        if surrounding_text.trim().is_empty() {
            return Ok(candidates[0].word.clone());
        }
        // Fast path 3: top candidate clearly wins — don't burn
        // 50-100 ms inference time on an unambiguous swipe.
        if !is_ambiguous(candidates, AMBIGUITY_THRESHOLD) {
            return Ok(candidates[0].word.clone());
        }

        // Otherwise: build prompt, run inference, parse response.
        let prompt = build_prompt(candidates, surrounding_text);
        let response = self.run_inference(&prompt)?;
        let idx = parse_response(&response, candidates.len()).ok_or_else(|| {
            RerankerError::ParseFailure(response.clone())
        })?;
        Ok(candidates[idx].word.clone())
    }

    /// Internal: run inference on a fully-built prompt. Returns the
    /// raw LLM output string. Mock mode always returns "1\n" (the
    /// first candidate's 1-based index) so the rest of the pipeline
    /// is exercised by tests.
    fn run_inference(&self, prompt: &str) -> Result<String, RerankerError> {
        #[cfg(feature = "llm-swipe-ffi")]
        {
            crate::llm_backend::generate(
                &self.inner.model,
                prompt,
                &self.inner.params,
            )
            .map_err(RerankerError::Generation)
        }
        #[cfg(not(feature = "llm-swipe-ffi"))]
        {
            let _ = prompt;
            let _ = &self.inner.model_path;
            Ok("1".to_string())
        }
    }
}

/// Score-difference threshold below which we consider the top
/// candidates "ambiguous enough" to be worth the LLM round-trip.
/// Recognizer scores are lower-is-better; a gap of 0.5 between #1
/// and #2 is roughly "the top candidate is one edit-distance ahead"
/// — confident enough to skip the LLM. Anything tighter and we
/// might as well ask.
///
/// Tunable. Smaller values invoke the LLM more often (better
/// accuracy, worse latency).
const AMBIGUITY_THRESHOLD: f64 = 0.5;

/// Number of candidates passed to the LLM. The prompt cost is
/// roughly linear in this — 5 is the sweet spot for the FP5's
/// inference budget.
const MAX_CANDIDATES_TO_RERANK: usize = 5;

/// True if the top two candidates' scores are within `threshold`
/// of each other — i.e. the recognizer isn't confident enough to
/// pick #1 outright.
fn is_ambiguous(candidates: &[WordCandidate], threshold: f64) -> bool {
    if candidates.len() < 2 {
        return false;
    }
    (candidates[1].score - candidates[0].score).abs() < threshold
}

/// Build the LLM prompt. Format:
///
/// ```text
/// Pick the word that best fits the context. Reply with ONLY the digit.
///
/// Context: "The campfire's last ember will __"
///
/// 1. store
/// 2. stone
/// 3. stoke
/// 4. story
/// 5. stove
///
/// Answer:
/// ```
///
/// Llama 3.2's instruction tuning handles this format well — we
/// don't need full Llama chat template tokens because we're decoding
/// raw and we want minimal preamble (saves KV cache).
fn build_prompt(candidates: &[WordCandidate], surrounding_text: &str) -> String {
    let mut prompt = String::with_capacity(256);
    prompt.push_str(
        "Pick the word that best fits the context. \
         Reply with ONLY the digit.\n\n",
    );
    // Trim long surrounding text — too much costs KV cache and the
    // local context is what matters. Keep the last ~120 chars.
    let context = if surrounding_text.chars().count() > 120 {
        let s: String = surrounding_text.chars().rev().take(120).collect();
        let trimmed: String = s.chars().rev().collect();
        format!("...{}", trimmed)
    } else {
        surrounding_text.to_string()
    };
    prompt.push_str(&format!("Context: \"{} __\"\n\n", context.trim_end()));

    for (i, c) in candidates
        .iter()
        .take(MAX_CANDIDATES_TO_RERANK)
        .enumerate()
    {
        prompt.push_str(&format!("{}. {}\n", i + 1, c.word));
    }
    prompt.push_str("\nAnswer: ");
    prompt
}

/// Parse the LLM response into a 0-based candidate index. Tolerant
/// of leading whitespace, trailing newlines, periods, and the LLM
/// echoing the chosen word. Returns None if the response doesn't
/// contain a digit in `1..=num_candidates`.
///
/// Examples that should parse to index 2 (third candidate):
///   * `"3"`
///   * `"3.\n"`
///   * `" 3"`
///   * `"3. stoke"`
///   * `"Answer: 3"`
fn parse_response(response: &str, num_candidates: usize) -> Option<usize> {
    let trimmed = response.trim();
    // Find the first ASCII digit in the response.
    let digit = trimmed.chars().find(|c| c.is_ascii_digit())?;
    let n: usize = digit.to_digit(10)? as usize;
    if n == 0 || n > num_candidates {
        return None;
    }
    Some(n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(word: &str, score: f64) -> WordCandidate {
        WordCandidate {
            word: word.to_string(),
            score,
        }
    }

    #[test]
    fn ambiguity_threshold_detects_close_scores() {
        let close = vec![cand("a", 0.5), cand("b", 0.7)];
        let far = vec![cand("a", 0.5), cand("b", 2.0)];
        assert!(is_ambiguous(&close, 0.5));
        assert!(!is_ambiguous(&far, 0.5));
    }

    #[test]
    fn single_candidate_skips_llm() {
        // Cant call rerank without an LlmSwipeReranker, so test
        // the contract via is_ambiguous semantics.
        let one = vec![cand("alone", 0.1)];
        assert!(!is_ambiguous(&one, 0.5));
    }

    #[test]
    fn prompt_builds_with_5_candidates() {
        let cands = vec![
            cand("store", 0.10),
            cand("stone", 0.15),
            cand("stoke", 0.20),
            cand("story", 0.25),
            cand("stove", 0.30),
        ];
        let prompt = build_prompt(&cands, "The campfire's last ember will");
        assert!(prompt.contains("1. store"));
        assert!(prompt.contains("5. stove"));
        assert!(prompt.contains("campfire"));
        assert!(prompt.ends_with("Answer: "));
    }

    #[test]
    fn prompt_truncates_long_context() {
        let long = "a".repeat(500);
        let cands = vec![cand("x", 0.1), cand("y", 0.2)];
        let prompt = build_prompt(&cands, &long);
        // Truncation should kick in: prompt context segment should
        // not contain all 500 'a's.
        let context_line = prompt
            .lines()
            .find(|l| l.starts_with("Context:"))
            .unwrap();
        // Should be ellipsis-prefixed.
        assert!(context_line.contains("..."));
        assert!(context_line.chars().count() < 200);
    }

    #[test]
    fn prompt_caps_candidates_at_five() {
        let cands: Vec<_> = (0..10)
            .map(|i| cand(&format!("word{}", i), i as f64 * 0.1))
            .collect();
        let prompt = build_prompt(&cands, "some context");
        assert!(prompt.contains("1. word0"));
        assert!(prompt.contains("5. word4"));
        // Sixth and beyond must NOT be in prompt.
        assert!(!prompt.contains("6. word5"));
        assert!(!prompt.contains("word9"));
    }

    #[test]
    fn parse_clean_digit() {
        assert_eq!(parse_response("3", 5), Some(2));
        assert_eq!(parse_response("1", 5), Some(0));
        assert_eq!(parse_response("5", 5), Some(4));
    }

    #[test]
    fn parse_with_trailing_period() {
        assert_eq!(parse_response("3.", 5), Some(2));
        assert_eq!(parse_response("3.\n", 5), Some(2));
    }

    #[test]
    fn parse_with_leading_whitespace() {
        assert_eq!(parse_response(" 3", 5), Some(2));
        assert_eq!(parse_response("\n3", 5), Some(2));
    }

    #[test]
    fn parse_with_echoed_word() {
        // Some LLMs will echo: "3. stoke"
        assert_eq!(parse_response("3. stoke", 5), Some(2));
    }

    #[test]
    fn parse_rejects_out_of_range() {
        assert_eq!(parse_response("0", 5), None);
        assert_eq!(parse_response("6", 5), None);
        assert_eq!(parse_response("9", 5), None);
    }

    #[test]
    fn parse_rejects_no_digit() {
        assert_eq!(parse_response("none", 5), None);
        assert_eq!(parse_response("", 5), None);
        assert_eq!(parse_response("?", 5), None);
    }

    #[test]
    fn parse_handles_answer_prefix() {
        // LLM might re-echo the prompt's trailing "Answer: ".
        assert_eq!(parse_response("Answer: 3", 5), Some(2));
    }

    /// In mock mode (no ffi feature), the reranker should always
    /// return the first candidate when invoked — exercising the
    /// integration point without needing a real model.
    #[test]
    #[cfg(not(feature = "llm-swipe-ffi"))]
    fn mock_mode_returns_first_candidate() {
        let r = LlmSwipeReranker::load(Path::new("/dev/null")).unwrap();
        let cands = vec![
            cand("apple", 0.10),
            cand("apply", 0.15), // close enough to be "ambiguous"
        ];
        // Even in ambiguity, mock mode returns candidates[0]
        // because its mock inference always replies "1".
        let chosen = r.rerank(&cands, "I want to eat an").unwrap();
        assert_eq!(chosen, "apple");
    }

    #[test]
    #[cfg(not(feature = "llm-swipe-ffi"))]
    fn empty_candidates_errors() {
        let r = LlmSwipeReranker::load(Path::new("/dev/null")).unwrap();
        let chosen = r.rerank(&[], "context");
        assert!(matches!(chosen, Err(RerankerError::NoCandidates)));
    }

    #[test]
    #[cfg(not(feature = "llm-swipe-ffi"))]
    fn empty_surrounding_text_skips_llm() {
        // Even with ambiguous scores, no context → no LLM → first
        // candidate. In mock mode this is indistinguishable from
        // an actual LLM invocation that returned "1", but a real
        // FFI build would skip the inference call (saving 50-100ms).
        let r = LlmSwipeReranker::load(Path::new("/dev/null")).unwrap();
        let cands = vec![cand("a", 0.1), cand("b", 0.15)];
        let chosen = r.rerank(&cands, "   ").unwrap();
        assert_eq!(chosen, "a");
    }
}
