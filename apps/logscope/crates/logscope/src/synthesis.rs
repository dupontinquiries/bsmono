//! synthesis.rs — the convergence primitive: does the fan AGREE?
//!
//! After K persona-steered branches propose independently, convergence on the same answer is evidence the
//! answer is robust — convergent evolution, not an artifact of one prompt or one persona's bias. This
//! clusters the candidates by normalized equivalence and reports the dominant cluster. Two consumers:
//!   * `toolreg::should_promote(independent_convergences, threshold)` — promote a fill to the reusable
//!     registry only when several branches INDEPENDENTLY reached it (not one lucky pass).
//!   * the devloop's confidence signal — "3 of 4 branches agree" is a stronger accept than "1 passed".
//!
//! Equivalence here is structural (whitespace/comment-insensitive), a pure + testable proxy for "the same
//! solution". Semantic equivalence (alpha-renaming, reordering) layers on top later via the same interface.

/// One cluster of candidates judged equivalent, by their (0-based) indices into the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    pub key: String,        // the normalized form the members share
    pub members: Vec<usize>,
}

/// The convergence report over a fan of candidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Convergence {
    pub clusters: Vec<Cluster>, // sorted largest-first, then by first-seen order
    pub total: usize,           // how many candidates went in
}

impl Convergence {
    /// The size of the largest cluster — how many branches independently produced the same solution.
    pub fn independent_convergences(&self) -> usize {
        self.clusters.first().map(|c| c.members.len()).unwrap_or(0)
    }
    /// Fraction of the fan that agrees on the dominant solution (0..=1).
    pub fn agreement(&self) -> f32 {
        if self.total == 0 { return 0.0; }
        self.independent_convergences() as f32 / self.total as f32
    }
    /// The representative index of the dominant cluster (the winner to keep), if any.
    pub fn dominant(&self) -> Option<usize> {
        self.clusters.first().and_then(|c| c.members.first().copied())
    }
    /// True when the dominant cluster covers at least `threshold` fraction of the fan.
    pub fn is_consensus(&self, threshold: f32) -> bool {
        self.total > 0 && self.agreement() >= threshold
    }
}

/// Normalize a candidate for equivalence: strip line comments and ALL whitespace. Two candidates that
/// differ only in formatting, spacing around operators, or comments collapse to the same key. (Cheap and
/// pure; it can over-merge across a removed space, but for clustering a fan of the same task that's rare
/// and harmless — the verifier is still the source of truth for correctness.)
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        // drop a trailing line comment (// ...) — cheap, ignores string-literal edge cases by design
        let code = match line.find("//") { Some(i) => &line[..i], None => line };
        for ch in code.chars() {
            if !ch.is_whitespace() { out.push(ch); }
        }
    }
    out
}

/// Cluster `candidates` by structural equivalence. Clusters are returned largest-first; ties keep
/// first-seen order (so `dominant()` is deterministic and favors the earliest branch on a tie).
pub fn converge(candidates: &[String]) -> Convergence {
    let mut clusters: Vec<Cluster> = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        let key = normalize(c);
        if let Some(cl) = clusters.iter_mut().find(|cl| cl.key == key) {
            cl.members.push(i);
        } else {
            clusters.push(Cluster { key, members: vec![i] });
        }
    }
    // stable sort by descending size; equal sizes keep insertion order (first-seen wins ties)
    clusters.sort_by(|a, b| b.members.len().cmp(&a.members.len()));
    Convergence { clusters, total: candidates.len() }
}

/// Convenience: does the fan converge strongly enough to promote? Combines the convergence count with the
/// toolreg threshold in one call for the devloop.
pub fn converged_enough(candidates: &[String], threshold: usize) -> bool {
    crate::toolreg::should_promote(converge(candidates).independent_convergences(), threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ignores_formatting_and_comments() {
        assert_eq!(normalize("let  x =  1; // set x"), normalize("let x = 1;"));
        assert_eq!(normalize("fn a() {\n  1\n}"), normalize("fn a() { 1 }"));
        assert_ne!(normalize("let x = 1;"), normalize("let x = 2;"));
    }

    #[test]
    fn converge_finds_the_dominant_cluster() {
        // three branches agree (mod formatting), one dissents
        let cands = vec![
            "return a + b;".to_string(),
            "return a+b;   // sum".to_string(),
            "return   a +  b ;".to_string(),
            "return a - b;".to_string(),
        ];
        let c = converge(&cands);
        assert_eq!(c.total, 4);
        assert_eq!(c.independent_convergences(), 3, "three equivalent branches");
        assert_eq!(c.clusters.len(), 2, "two distinct solutions");
        assert_eq!(c.dominant(), Some(0), "earliest member of the dominant cluster");
        assert!((c.agreement() - 0.75).abs() < 1e-6);
        assert!(c.is_consensus(0.5) && !c.is_consensus(0.8));
    }

    #[test]
    fn all_distinct_means_no_convergence() {
        let cands = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let c = converge(&cands);
        assert_eq!(c.independent_convergences(), 1, "no two agree");
        assert_eq!(c.clusters.len(), 3);
        assert!(!c.is_consensus(0.5));
    }

    #[test]
    fn converged_enough_gates_on_threshold() {
        let agree = vec!["x=1;".to_string(), "x = 1;".to_string(), "x=1 ;".to_string()];
        assert!(converged_enough(&agree, 2), "3 agree ≥ threshold 2");
        assert!(!converged_enough(&agree, 4), "3 agree < threshold 4");
        let split = vec!["x=1;".to_string(), "x=2;".to_string()];
        assert!(!converged_enough(&split, 2), "no convergence");
    }

    #[test]
    fn empty_fan_is_safe() {
        let c = converge(&[]);
        assert_eq!(c.total, 0);
        assert_eq!(c.independent_convergences(), 0);
        assert_eq!(c.agreement(), 0.0);
        assert_eq!(c.dominant(), None);
        assert!(!c.is_consensus(0.0), "no candidates → never consensus");
    }
}
