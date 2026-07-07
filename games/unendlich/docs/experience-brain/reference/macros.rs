//! Macro discovery — the smallest honest rung of "reasoning about abstraction
//! effectiveness": the proposer's op-vocabulary GROWS from harvested wins.
//!
//! The loop:
//!   1. scan experience.jsonl (the wins the arena already harvests),
//!   2. bucket fixes by a SIGNATURE (the failing-test set = the abstraction key),
//!   3. within a bucket, find a fix that RECURS across >= K distinct targets
//!      (recurrence is the generality filter — a one-off is not an abstraction),
//!   4. promote it to a named macro-op,
//!   5. apply macros as the cheapest proposer tier (no model call),
//!   6. ABLATE on a held-out split to measure whether the grown vocabulary helps.
//!
//! HONEST CEILING. These macros are *memoization-grade* abstractions: exact /
//! near-exact recurring diffs, matched by signature. That is the weakest possible
//! abstraction — there is no parameterized templating, so a macro only fires when
//! the same class of failure recurs. It is a real instance of vocabulary growing
//! from downstream success, measured honestly on held-out tasks. It is NOT proof
//! that the phenomenon scales; the next rung is templated/parameterized macros.

use std::collections::{HashMap, HashSet};
use crate::engine::diff_similarity; // revived from the (otherwise dead) diff-scoring code

/// One harvested win, parsed from experience.jsonl.
#[derive(Clone, Debug)]
pub struct Experience {
    pub target: String,
    pub failing: Vec<String>,
    pub diff: String,
    pub score: f64,
}

/// A promoted recurring fix: a new named move in the proposer's vocabulary.
#[derive(Clone, Debug)]
pub struct Macro {
    pub name: String,      // macro_1, macro_2, … (ranked by support)
    pub signature: String, // the abstraction key it answers (sorted failing-test set)
    pub diff: String,      // canonical diff for this recurring fix
    pub support: usize,    // # of DISTINCT targets it recurred across (generality)
}

/// The abstraction KEY: the normalized, sorted set of failing tests. Two wins share
/// a signature when they repair the same class of failure. (Cheapest sensible key.)
fn signature(failing: &[String]) -> String {
    let mut v: Vec<String> = failing.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    v.sort();
    v.dedup();
    v.join("|")
}

// ---- experience.jsonl parsing (engine writes JSON by hand; parse by hand) ------

/// Unescape a JSON string body (the chars between the quotes).
fn json_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('u') => { // \uXXXX — take 4 hex, best-effort
                    let hex: String = it.by_ref().take(4).collect();
                    if let Ok(n) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(n) { out.push(ch); }
                    }
                }
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Slice just after `"key" :` (tolerating whitespace around the colon).
fn after_key_colon<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let kpat = format!("\"{key}\"");
    let kpos = line.find(&kpat)? + kpat.len();
    let rest = line[kpos..].trim_start();
    let rest = rest.strip_prefix(':')?;
    Some(rest.trim_start())
}

/// Extract a top-level string field value (returns the unescaped body).
fn field_str(line: &str, key: &str) -> Option<String> {
    let rest = after_key_colon(line, key)?.strip_prefix('"')?;
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut bs = 0;
            let mut j = i;
            while j > 0 && bytes[j - 1] == b'\\' { bs += 1; j -= 1; }
            if bs % 2 == 0 { break; }
        }
        i += 1;
    }
    Some(json_unescape(&rest[..i]))
}

/// Extract a numeric field.
fn field_f64(line: &str, key: &str) -> Option<f64> {
    let rest = after_key_colon(line, key)?;
    let end = rest.find([',', '}', ']', ' ']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

/// Extract a string-array field (e.g. failing_before).
fn field_str_array(line: &str, key: &str) -> Vec<String> {
    let Some(rest) = after_key_colon(line, key).and_then(|r| r.strip_prefix('[')) else { return Vec::new() };
    let Some(end) = rest.find(']') else { return Vec::new() };
    let inner = &rest[..end];
    let bytes = inner.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() {
                if bytes[j] == b'"' && bytes[j - 1] != b'\\' { break; }
                j += 1;
            }
            out.push(json_unescape(&inner[start..j]));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Parse the whole experience.jsonl text into wins (silently skips junk lines).
pub fn parse_experience(text: &str) -> Vec<Experience> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let diff = field_str(l, "diff")?;
            Some(Experience {
                target: field_str(l, "target").unwrap_or_default(),
                failing: field_str_array(l, "failing_before"),
                diff,
                score: field_f64(l, "score").unwrap_or(0.0),
            })
        })
        .collect()
}

// ---- discovery + application -------------------------------------------------

/// Promote recurring wins into macros. `min_support` = how many DISTINCT targets a
/// fix must recur across to count as general; `sim` = the diff-similarity threshold
/// for "the same fix" (1.0 = exact, lower = near-exact).
pub fn discover(exps: &[Experience], min_support: usize, sim: f64) -> Vec<Macro> {
    let mut buckets: HashMap<String, Vec<&Experience>> = HashMap::new();
    for e in exps {
        let sig = signature(&e.failing);
        if sig.is_empty() || e.diff.trim().is_empty() { continue; }
        buckets.entry(sig).or_default().push(e);
    }
    let mut out: Vec<Macro> = Vec::new();
    for (sig, group) in buckets {
        // For each candidate diff, count distinct targets whose diff is >= sim similar.
        // The candidate with the widest distinct-target support, if it clears
        // min_support, becomes the macro for this signature.
        let mut best: Option<(&Experience, usize)> = None;
        for cand in &group {
            let mut targets: HashSet<&str> = HashSet::new();
            for other in &group {
                if diff_similarity(&cand.diff, &other.diff) >= sim {
                    targets.insert(other.target.as_str());
                }
            }
            let support = targets.len();
            if support >= min_support && best.map_or(true, |(_, b)| support > b) {
                best = Some((cand, support));
            }
        }
        if let Some((cand, support)) = best {
            out.push(Macro { name: String::new(), signature: sig, diff: cand.diff.clone(), support });
        }
    }
    out.sort_by(|a, b| b.support.cmp(&a.support).then(a.signature.cmp(&b.signature)));
    for (i, m) in out.iter_mut().enumerate() { m.name = format!("macro_{}", i + 1); }
    out
}

/// The macro proposer tier: given a failing context, return the macro whose
/// signature matches (no model call). Memoization-grade — fires only on a known
/// failure class.
pub fn apply<'a>(macros: &'a [Macro], failing: &[String]) -> Option<&'a Macro> {
    let sig = signature(failing);
    macros.iter().find(|m| m.signature == sig)
}

// ---- ablation: does the grown vocabulary actually help? ----------------------

/// A task in the held-out family: a failing context plus its known-good fix.
#[derive(Clone, Debug)]
pub struct Task {
    pub target: String,
    pub failing: Vec<String>,
    pub gold_diff: String,
}

/// Ablation result: the tier split and the cost with/without macros.
#[derive(Clone, Debug, PartialEq)]
pub struct Ablation {
    pub tasks: usize,
    pub solved_by_macro: usize, // free (no model call), and the macro matched the gold fix
    pub fell_through: usize,    // no macro fired → would hit the model tier
    pub cost_with: f64,         // total proposer cost WITH the macro tier
    pub cost_without: f64,      // total proposer cost WITHOUT it (everything hits the model)
}

impl Ablation {
    pub fn saved_fraction(&self) -> f64 {
        if self.cost_without == 0.0 { 0.0 } else { 1.0 - self.cost_with / self.cost_without }
    }
}

/// Run the held-out family through the cascade, with and without the macro tier.
/// `macro_cost` ≈ 0 (a dict lookup + apply); `model_cost` is the price of one model
/// call. A macro "solves" a task only if its diff matches the gold fix (>= sim) —
/// firing on the wrong fix would cost a wasted gate, so we count only real solves.
pub fn ablate(macros: &[Macro], family: &[Task], sim: f64, macro_cost: f64, model_cost: f64) -> Ablation {
    let mut solved = 0usize;
    let mut through = 0usize;
    let mut cost_with = 0.0;
    for t in family {
        match apply(macros, &t.failing) {
            Some(m) if diff_similarity(&m.diff, &t.gold_diff) >= sim => {
                solved += 1;
                cost_with += macro_cost; // free win
            }
            _ => {
                through += 1;
                cost_with += macro_cost + model_cost; // tried a lookup, then escalated
            }
        }
    }
    Ablation {
        tasks: family.len(),
        solved_by_macro: solved,
        fell_through: through,
        cost_with,
        cost_without: family.len() as f64 * model_cost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp(target: &str, failing: &[&str], diff: &str) -> Experience {
        Experience { target: target.into(), failing: failing.iter().map(|s| s.to_string()).collect(),
                     diff: diff.into(), score: 0.95 }
    }

    #[test]
    fn signature_is_order_insensitive() {
        assert_eq!(signature(&["b".into(), "a".into()]), signature(&["a".into(), "b".into()]));
        assert!(signature(&[]).is_empty());
    }

    #[test]
    fn parses_handrolled_experience_jsonl() {
        // exactly the engine's format: target, score, failing_before[], diff (escaped)
        let line = r#"{"target":"/p/repo","score":0.9600,"failing_before":["tests::add","tests::sub"],"diff":"--- a/src/lib.rs\n+++ b/src/lib.rs\n@@\n-a - b\n+a + b\n"}"#;
        let v = parse_experience(line);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].target, "/p/repo");
        assert_eq!(v[0].failing, vec!["tests::add", "tests::sub"]);
        assert!(v[0].diff.contains("-a - b") && v[0].diff.contains("+a + b"));
        assert!((v[0].score - 0.96).abs() < 1e-6);
    }

    #[test]
    fn discover_promotes_recurring_ignores_oneoffs() {
        // "minus→plus" recurs across 3 DISTINCT targets; a one-off noise fix does not.
        let plus = "--- a/x\n+++ b/x\n@@\n-a - b\n+a + b\n";
        let exps = vec![
            exp("repo/m1", &["tests::add"], plus),
            exp("repo/m2", &["tests::add"], plus),
            exp("repo/m3", &["tests::add"], plus),
            exp("repo/z9", &["tests::weird"], "--- a/y\n+++ b/y\n@@\n-foo\n+bar\n"), // one-off
        ];
        let macros = discover(&exps, 3, 0.9);
        assert_eq!(macros.len(), 1, "only the recurring class is promoted");
        assert_eq!(macros[0].signature, "tests::add");
        assert_eq!(macros[0].support, 3);
        // the one-off class never clears min_support
        assert!(apply(&macros, &["tests::weird".into()]).is_none());
        // the recurring class matches
        assert!(apply(&macros, &["tests::add".into()]).is_some());
    }

    #[test]
    fn ablation_on_heldout_shows_macros_help_and_dont_overfire() {
        // TRAIN: a recurring class harvested from several distinct targets.
        let plus = "--- a/x\n+++ b/x\n@@\n-a - b\n+a + b\n";
        let train = vec![
            exp("t/a", &["tests::add"], plus),
            exp("t/b", &["tests::add"], plus),
            exp("t/c", &["tests::add"], plus),
        ];
        let macros = discover(&train, 3, 0.9);
        assert_eq!(macros.len(), 1);

        // HELD-OUT family: a NEW target of the recurring class (should solve free),
        // plus a genuinely novel class (must fall through, not misfire).
        let family = vec![
            Task { target: "held/new".into(), failing: vec!["tests::add".into()], gold_diff: plus.into() },
            Task { target: "held/novel".into(), failing: vec!["tests::brandnew".into()],
                   gold_diff: "--- a/q\n+++ b/q\n@@\n-1\n+2\n".into() },
        ];
        let ab = ablate(&macros, &family, 0.9, 0.0, 1.0);
        assert_eq!(ab.solved_by_macro, 1, "recurring class solved on a held-out target, for free");
        assert_eq!(ab.fell_through, 1, "novel class escalates instead of misfiring");
        // cost: 1 free + 1 model call = 1.0, vs 2 model calls = 2.0 → 50% saved here
        assert!((ab.cost_with - 1.0).abs() < 1e-9);
        assert!((ab.cost_without - 2.0).abs() < 1e-9);
        assert!((ab.saved_fraction() - 0.5).abs() < 1e-9);
    }
}
