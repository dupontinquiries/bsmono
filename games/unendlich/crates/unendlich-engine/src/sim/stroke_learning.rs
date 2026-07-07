//! `sim::stroke_learning` — the bending system learning what a player *means*.
//!
//! Every bending shape the player attempts writes a stroke record: the spoken
//! phrase, what the corpus resolved it to, the match confidence, and whether the
//! player corrected, rejected, or undid it. Over time these records let the
//! recognizer adapt to one player's accent/mic without any remote training: it
//! infers **aliases** — a player's habitual mispronunciation mapped to the term
//! they actually meant.
//!
//! The inference (spec §9.2): a spoken phrase that keeps failing to match but is
//! followed by the player succeeding with some canonical term is a near-miss for
//! that term. When the same near-miss recurs [`MIN_NEAR_MISS_GROUP`]+ times and
//! the player's own subsequent corrections agree ≥[`ALIAS_CONF_THRESHOLD`], a
//! **session alias** is proposed. After [`PROMOTE_USES`] uses without a rejection
//! it's offered for permanent registration. The player's correction behaviour is
//! the only training signal.
//!
//! Pure/std-only: `String`/`HashMap`/`Vec`. The caller supplies the ordered
//! stroke history (already filtered to one player); no clock or IO.

use std::collections::HashMap;

/// Confidence at/above which a match counts as a confident success.
pub const HIGH_CONFIDENCE: f32 = 0.75;
/// Below this a record is treated as a near-miss (failed/low-confidence).
pub const NEAR_MISS_CONFIDENCE: f32 = 0.75;
/// How many times the same near-miss must recur before an alias is considered.
pub const MIN_NEAR_MISS_GROUP: usize = 6;
/// Fraction of a near-miss group's corrections that must agree on one target.
pub const ALIAS_CONF_THRESHOLD: f32 = 0.70;
/// Uses without rejection before a session alias is offered for permanent status.
pub const PROMOTE_USES: u32 = 10;

/// One bending-shape attempt (success or failure).
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeRecord {
    /// Raw phoneme transcription of what the player said.
    pub spoken: String,
    /// What the corpus resolved it to, if anything.
    pub matched: Option<String>,
    /// Match confidence 0..1.
    pub confidence: f32,
    /// Player said "no" to a low-confidence confirm prompt.
    pub was_rejected: bool,
    /// Player undid the produced shape within a few seconds.
    pub was_undone: bool,
}

impl StrokeRecord {
    /// A confident, kept success — the signal that the player got what they meant.
    pub fn is_success(&self) -> bool {
        self.matched.is_some()
            && self.confidence >= HIGH_CONFIDENCE
            && !self.was_rejected
            && !self.was_undone
    }

    /// A failed or low-confidence attempt — a candidate near-miss.
    pub fn is_near_miss(&self) -> bool {
        self.confidence < NEAR_MISS_CONFIDENCE
    }
}

/// A proposed mapping from a player's pronunciation to a canonical term.
#[derive(Debug, Clone, PartialEq)]
pub struct InferredAlias {
    pub spoken: String,
    pub target: String,
    /// Agreement among the player's own corrections, 0..1.
    pub confidence: f32,
}

/// The canonical term the player succeeded with *after* the near-miss at `idx`
/// — evidence they corrected themselves and that near-miss was the intended word.
fn next_success_term(history: &[StrokeRecord], idx: usize) -> Option<String> {
    history[idx + 1..]
        .iter()
        .find(|r| r.is_success())
        .and_then(|r| r.matched.clone())
}

fn most_frequent(items: &[String]) -> Option<(String, f32)> {
    if items.is_empty() {
        return None;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for it in items {
        *counts.entry(it.as_str()).or_insert(0) += 1;
    }
    // deterministic: highest count, ties broken by lexicographically smaller term
    let (term, n) = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
        .unwrap();
    Some((term.to_string(), n as f32 / items.len() as f32))
}

/// Infer session aliases from a player's ordered stroke history (§9.2). Groups
/// near-misses by spoken phrase; for each group with at least
/// [`MIN_NEAR_MISS_GROUP`] entries, takes the term the player subsequently
/// succeeded with after each near-miss, and — if a single target accounts for
/// ≥[`ALIAS_CONF_THRESHOLD`] of those corrections — proposes it as an alias.
/// Deterministic; aliases are returned sorted by `spoken`.
pub fn infer_aliases(history: &[StrokeRecord]) -> Vec<InferredAlias> {
    // group near-miss record indices by spoken phrase
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, rec) in history.iter().enumerate() {
        if rec.is_near_miss() {
            groups.entry(rec.spoken.as_str()).or_default().push(i);
        }
    }

    let mut out = Vec::new();
    for (spoken, idxs) in groups {
        if idxs.len() < MIN_NEAR_MISS_GROUP {
            continue;
        }
        // the term the player corrected to after each near-miss
        let corrections: Vec<String> =
            idxs.iter().filter_map(|&i| next_success_term(history, i)).collect();
        if corrections.is_empty() {
            continue;
        }
        if let Some((target, confidence)) = most_frequent(&corrections) {
            // don't alias a word to itself
            if confidence >= ALIAS_CONF_THRESHOLD && target != spoken {
                out.push(InferredAlias { spoken: spoken.to_string(), target, confidence });
            }
        }
    }
    out.sort_by(|a, b| a.spoken.cmp(&b.spoken));
    out
}

#[derive(Debug, Clone)]
struct AliasEntry {
    target: String,
    uses: u32,
    rejected: bool,
    permanent: bool,
}

/// A player's live alias table: session aliases accrue use and, once trusted,
/// are offered for permanent registration.
#[derive(Debug, Clone, Default)]
pub struct AliasTable {
    entries: HashMap<String, AliasEntry>,
}

impl AliasTable {
    pub fn new() -> Self {
        AliasTable::default()
    }

    /// Register a session alias (e.g. from [`infer_aliases`]). Re-registering the
    /// same spoken phrase updates the target and clears any prior rejection.
    pub fn add_session_alias(&mut self, spoken: impl Into<String>, target: impl Into<String>) {
        let spoken = spoken.into();
        self.entries.insert(
            spoken,
            AliasEntry { target: target.into(), uses: 0, rejected: false, permanent: false },
        );
    }

    /// Resolve a spoken phrase through the aliases, if one applies.
    pub fn resolve(&self, spoken: &str) -> Option<&str> {
        self.entries.get(spoken).map(|e| e.target.as_str())
    }

    /// Record a use of an alias. `rejected` marks that the player rejected the
    /// resolved shape (which permanently disqualifies the alias from promotion).
    pub fn record_use(&mut self, spoken: &str, rejected: bool) {
        if let Some(e) = self.entries.get_mut(spoken) {
            e.uses += 1;
            if rejected {
                e.rejected = true;
            }
        }
    }

    /// Whether a session alias has earned permanent registration:
    /// [`PROMOTE_USES`] uses with no rejection, and not already permanent.
    pub fn should_promote(&self, spoken: &str) -> bool {
        self.entries
            .get(spoken)
            .map_or(false, |e| !e.permanent && !e.rejected && e.uses >= PROMOTE_USES)
    }

    /// Mark an alias permanently registered (after the player accepts the offer).
    pub fn promote(&mut self, spoken: &str) {
        if let Some(e) = self.entries.get_mut(spoken) {
            e.permanent = true;
        }
    }

    pub fn is_permanent(&self, spoken: &str) -> bool {
        self.entries.get(spoken).map_or(false, |e| e.permanent)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn miss(spoken: &str) -> StrokeRecord {
        StrokeRecord { spoken: spoken.into(), matched: None, confidence: 0.4, was_rejected: false, was_undone: false }
    }
    fn hit(spoken: &str, term: &str) -> StrokeRecord {
        StrokeRecord {
            spoken: spoken.into(),
            matched: Some(term.into()),
            confidence: 0.95,
            was_rejected: false,
            was_undone: false,
        }
    }

    #[test]
    fn success_and_near_miss_classification() {
        assert!(hit("rise", "Rise").is_success());
        assert!(miss("rys").is_near_miss());
        // undone / rejected / low-confidence are not successes
        let mut undone = hit("rise", "Rise");
        undone.was_undone = true;
        assert!(!undone.is_success());
    }

    #[test]
    fn six_near_misses_with_consistent_correction_yield_an_alias() {
        // player keeps saying "rys", fails, then says it right ("Rise")
        let mut h = Vec::new();
        for _ in 0..6 {
            h.push(miss("rys"));
            h.push(hit("rise", "Rise"));
        }
        let aliases = infer_aliases(&h);
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].spoken, "rys");
        assert_eq!(aliases[0].target, "Rise");
        assert!((aliases[0].confidence - 1.0).abs() < 1e-6, "all corrections agree");
    }

    #[test]
    fn fewer_than_six_near_misses_is_not_enough() {
        let mut h = Vec::new();
        for _ in 0..5 {
            h.push(miss("rys"));
            h.push(hit("rise", "Rise"));
        }
        assert!(infer_aliases(&h).is_empty(), "under the recurrence threshold");
    }

    #[test]
    fn inconsistent_corrections_below_threshold_are_rejected() {
        // 6 near-misses, but corrections split 4 Rise / 2 Surge → 67% < 70%
        let mut h = Vec::new();
        for _ in 0..4 {
            h.push(miss("srr"));
            h.push(hit("rise", "Rise"));
        }
        for _ in 0..2 {
            h.push(miss("srr"));
            h.push(hit("surge", "Surge"));
        }
        assert!(infer_aliases(&h).is_empty(), "ambiguous intent → no alias");
    }

    #[test]
    fn alias_promotion_after_ten_clean_uses() {
        let mut table = AliasTable::new();
        table.add_session_alias("rys", "Rise");
        assert_eq!(table.resolve("rys"), Some("Rise"));
        for _ in 0..(PROMOTE_USES - 1) {
            table.record_use("rys", false);
        }
        assert!(!table.should_promote("rys"), "not yet at the use threshold");
        table.record_use("rys", false);
        assert!(table.should_promote("rys"), "ten clean uses → offer permanent");
        table.promote("rys");
        assert!(table.is_permanent("rys"));
        assert!(!table.should_promote("rys"), "already permanent");
    }

    #[test]
    fn a_rejection_blocks_promotion() {
        let mut table = AliasTable::new();
        table.add_session_alias("rys", "Rise");
        for _ in 0..20 {
            table.record_use("rys", false);
        }
        table.record_use("rys", true); // player rejected the shape once
        assert!(!table.should_promote("rys"), "a rejection disqualifies the alias");
    }

    #[test]
    fn deterministic_and_self_alias_guarded() {
        // an exact-word near-miss that corrects to itself must not alias
        let mut h = Vec::new();
        for _ in 0..6 {
            let mut m = miss("rise");
            m.confidence = 0.4;
            h.push(m);
            h.push(hit("rise", "rise"));
        }
        assert!(infer_aliases(&h).is_empty(), "never alias a word to itself");
        // determinism
        let a = infer_aliases(&h);
        let b = infer_aliases(&h);
        assert_eq!(a, b);
    }
}
