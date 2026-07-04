//! context — the learned "fastpath" context model that lets recognition beat cold whisper.
//!
//! voiceterm's fastpath (decision_cache / token_stats / command_index) works because it *learns*
//! your vocabulary: frequent, confirmed utterances get recognised instantly and accurately,
//! whisper only runs for novel input. This module is the portable heart of that idea, so every
//! voice surface (keyboard dictation, voiceterm, logscope voice) shares one learning DB:
//!
//! * `record` a confirmed utterance → it's learned (frequency + recency).
//! * `fastpath` returns a confident match for a known/near utterance — **skip whisper**.
//! * `hints` gives ranked completions for a partial prefix — drives the live viewer / inline
//!   hinting in any input.
//! * `rank` re-scores a recogniser's candidates using the learned prior.
//!
//! Pure and testable; the acoustic side (whisper, speaker embeddings) stays behind the
//! [`Transcriber`](crate::pipeline::Transcriber) trait and only runs when the fastpath misses.

use std::collections::HashMap;

/// A learned utterance context: how often + how recently each confirmed phrase was used.
#[derive(Debug, Default)]
pub struct ContextModel {
    counts: HashMap<String, u32>,
    /// Monotonic tick bumped each record, stored per phrase for recency.
    last_seen: HashMap<String, u64>,
    tick: u64,
}

impl ContextModel {
    pub fn new() -> ContextModel {
        ContextModel::default()
    }

    fn norm(s: &str) -> String {
        s.trim().to_lowercase()
    }

    /// Learn a confirmed utterance (called when the user accepts a transcription/command).
    pub fn record(&mut self, utterance: &str) {
        let key = Self::norm(utterance);
        if key.is_empty() {
            return;
        }
        self.tick += 1;
        *self.counts.entry(key.clone()).or_insert(0) += 1;
        self.last_seen.insert(key, self.tick);
    }

    /// How many times this exact phrase has been confirmed.
    pub fn frequency(&self, utterance: &str) -> u32 {
        self.counts.get(&Self::norm(utterance)).copied().unwrap_or(0)
    }

    pub fn learned_phrases(&self) -> usize {
        self.counts.len()
    }

    /// A learned prior in [0,1) for a phrase: rises with frequency and recency. Used to bias a
    /// recogniser's candidates toward what you actually say.
    pub fn prior(&self, utterance: &str) -> f32 {
        let key = Self::norm(utterance);
        let f = self.counts.get(&key).copied().unwrap_or(0) as f32;
        if f == 0.0 {
            return 0.0;
        }
        let recency = self
            .last_seen
            .get(&key)
            .map(|&t| (t as f32) / (self.tick.max(1) as f32))
            .unwrap_or(0.0);
        // frequency saturates; recency nudges. Result in (0,1).
        let freq_term = f / (f + 3.0);
        0.7 * freq_term + 0.3 * recency
    }

    /// Confident fastpath match: if a known phrase equals or closely extends `utterance` and it's
    /// been confirmed at least `min_freq` times, return it so whisper can be skipped. Prefers the
    /// most-used matching phrase.
    pub fn fastpath(&self, utterance: &str, min_freq: u32) -> Option<String> {
        let key = Self::norm(utterance);
        if key.is_empty() {
            return None;
        }
        // exact learned phrase → immediate
        if self.counts.get(&key).copied().unwrap_or(0) >= min_freq {
            return Some(key);
        }
        // otherwise the best learned phrase that starts with this (partial) utterance
        self.counts
            .iter()
            .filter(|(p, c)| **c >= min_freq && p.starts_with(&key))
            .max_by_key(|(p, c)| (**c, std::cmp::Reverse(p.len())))
            .map(|(p, _)| p.clone())
    }

    /// Ranked completion hints for a partial `prefix` (for the live viewer / inline hinting),
    /// ordered by learned prior, up to `max`.
    pub fn hints(&self, prefix: &str, max: usize) -> Vec<String> {
        let key = Self::norm(prefix);
        let mut cands: Vec<(&String, f32)> = self
            .counts
            .keys()
            .filter(|p| key.is_empty() || (p.starts_with(&key) && **p != key))
            .map(|p| (p, self.prior(p)))
            .collect();
        cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        cands.into_iter().take(max).map(|(p, _)| p.clone()).collect()
    }

    /// Re-rank recogniser candidates `(text, acoustic_score)` by blending in the learned prior,
    /// so a frequently-used phrase wins ties over an acoustically-similar but unused one.
    pub fn rank(&self, candidates: Vec<(String, f32)>) -> Vec<(String, f32)> {
        let mut scored: Vec<(String, f32)> = candidates
            .into_iter()
            .map(|(text, acoustic)| {
                let blended = 0.6 * acoustic + 0.4 * self.prior(&text);
                (text, blended)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_counts_frequency() {
        let mut m = ContextModel::new();
        m.record("open the terminal");
        m.record("Open the Terminal"); // case/space normalised
        assert_eq!(m.frequency("open the terminal"), 2);
        assert_eq!(m.learned_phrases(), 1);
    }

    #[test]
    fn fastpath_returns_confident_known_phrase() {
        let mut m = ContextModel::new();
        for _ in 0..3 {
            m.record("git status");
        }
        // exact, confirmed ≥ min_freq → fastpath hit (skip whisper)
        assert_eq!(m.fastpath("git status", 3).as_deref(), Some("git status"));
        // partial extends to the learned phrase
        assert_eq!(m.fastpath("git stat", 3).as_deref(), Some("git status"));
        // below min_freq → miss (fall back to whisper)
        assert!(m.fastpath("git status", 5).is_none());
        assert!(m.fastpath("kubectl apply", 1).is_none());
    }

    #[test]
    fn hints_are_ranked_by_learned_prior() {
        let mut m = ContextModel::new();
        for _ in 0..5 {
            m.record("deploy staging");
        }
        m.record("deploy production");
        let hints = m.hints("deploy", 5);
        assert_eq!(hints.first().map(|s| s.as_str()), Some("deploy staging"), "more-used ranks first");
        assert!(hints.contains(&"deploy production".to_string()));
    }

    #[test]
    fn rank_lets_learned_phrase_win_a_tie() {
        let mut m = ContextModel::new();
        for _ in 0..4 {
            m.record("restart the server");
        }
        // two candidates with equal acoustic score; learned one should win
        let ranked = m.rank(vec![
            ("restart the service".into(), 0.8),
            ("restart the server".into(), 0.8),
        ]);
        assert_eq!(ranked[0].0, "restart the server");
    }

    #[test]
    fn unknown_phrases_have_zero_prior() {
        let m = ContextModel::new();
        assert_eq!(m.prior("never said"), 0.0);
        assert!(m.hints("any", 3).is_empty());
    }
}
