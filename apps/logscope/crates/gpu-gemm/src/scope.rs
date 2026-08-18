//! scope.rs — where a name lives, and which way its usage is moving.
//!
//! `corpus` answers one question: does this name exist anywhere. That is the right question for a
//! kill check and the wrong one for a preference. Between two names that both exist, the useful
//! signal is locality — `kv_dim` is unremarkable in the repository at large and is the native
//! vocabulary of one directory — and direction: whether a name is being written into that directory
//! or migrated out of it.
//!
//! Both are LESSER scores. They rank branches that have already passed the compile gate and they
//! must never gate anything themselves, for the reason `corpus::familiarity` carries and one more
//! besides. A globally-scored conformity metric is gameable by copying the repository; a
//! module-scoped one is gameable by copying ONE DIRECTORY, which is small enough to parrot
//! wholesale. Narrowing the scope sharpens the signal and sharpens the exploit by the same amount.
//!
//! ## Lift, not frequency
//!
//! Raw scoped counts are dominated by whatever is common everywhere — `self`, `len`, `new`. The
//! quantity that carries information is how much MORE common a name is here than in the repository
//! at large: a smoothed pointwise mutual information, in bits.
//!
//! ```text
//! lift(name, scope) = log2[ p(name | scope) / p(name | repo) ]
//! ```
//!
//! Zero means the scope uses it exactly as often as everywhere else, which is the same as knowing
//! nothing. Positive means local vocabulary.
//!
//! ## Scope is a hierarchy, not a directory
//!
//! An edit in `crates/gpu-gemm/src/kv/pages.rs` sits inside the file, the `kv` directory, the crate,
//! and the workspace. Each is a legitimate scope and the nearest is the most informative, so lift is
//! mixed over the path's prefixes with a geometric decay. A single hard-coded level would either
//! miss crate-wide idiom or drown in it.
//!
//! ## Trend needs history and history is nearly all positives
//!
//! Git shows what was committed, never what was tried and dropped, so a corpus mined from it cannot
//! teach what fails. REMOVALS are the exception and the reason `TrendIndex` reads diffs rather than
//! the tree: a name deleted from a module across many commits is a name that module is migrating
//! away from, and that is a negative example with no synthetic substitute.
//!
//! The trap is that recency-weighted usage rewards the current fashion, and a repository mid-
//! migration has a snapshot dominated by the API being retired. So the derivative matters more than
//! the level, which is exactly why `momentum` is reported separately from `lift` rather than folded
//! into one number.
//!
//! Pure: no wgpu, no model, no git. `TrendIndex` parses a unified diff it is handed.

use std::collections::{BTreeMap, HashMap};

/// Additive smoothing constant. Small, because the alternative to smoothing is a division by zero
/// every time a scope has never seen a name — which is most of the time.
const ALPHA: f64 = 0.5;

/// Per-scope identifier counts, where a scope is any prefix of a file's path.
#[derive(Clone, Debug, Default)]
pub struct ScopedIndex {
    scopes: BTreeMap<String, HashMap<String, u32>>,
    scope_totals: BTreeMap<String, u64>,
    global: HashMap<String, u32>,
    global_total: u64,
    vocab: usize,
}

/// Every path prefix of `path`, nearest first: the file, then each enclosing directory.
pub fn scopes_of(path: &str) -> Vec<String> {
    let norm = path.replace('\\', "/");
    let parts: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    (1..=parts.len()).rev().map(|k| parts[..k].join("/")).collect()
}

impl ScopedIndex {
    pub fn new() -> Self { Self::default() }

    /// Fold one file in under every scope that contains it.
    pub fn add_file(&mut self, path: &str, src: &str) {
        let ids = crate::corpus::identifiers(&crate::corpus::strip_noncode(src));
        let scopes = scopes_of(path);
        for id in &ids {
            let e = self.global.entry(id.clone()).or_insert(0);
            if *e == 0 { self.vocab += 1; }
            *e += 1;
            self.global_total += 1;
            for s in &scopes {
                *self.scopes.entry(s.clone()).or_default().entry(id.clone()).or_insert(0) += 1;
                *self.scope_totals.entry(s.clone()).or_insert(0) += 1;
            }
        }
    }

    pub fn global_count(&self, name: &str) -> u32 { self.global.get(name).copied().unwrap_or(0) }
    pub fn scope_count(&self, scope: &str, name: &str) -> u32 {
        self.scopes.get(scope).and_then(|m| m.get(name)).copied().unwrap_or(0)
    }
    pub fn vocab(&self) -> usize { self.vocab }
    pub fn scopes(&self) -> usize { self.scopes.len() }

    /// Smoothed PMI of `name` in one exact scope, in bits. Zero when the scope uses it at the
    /// repository's own rate, and zero — no opinion — for a name the repository has never seen.
    ///
    /// That last guard is not tidying. Additive smoothing puts a floor of ALPHA on every count, and
    /// the floor is relatively LARGER in a small scope than in the whole repository, so an
    /// identifier appearing nowhere at all scored positive lift: the smoother reported invented
    /// vocabulary as locally distinctive. Lift is a preference between names that exist; whether a
    /// name exists is `corpus`'s question and it answers it properly.
    pub fn lift_in(&self, scope: &str, name: &str) -> f32 {
        if self.global_count(name) == 0 { return 0.0; }
        let v = self.vocab.max(1) as f64;
        let n_s = *self.scope_totals.get(scope).unwrap_or(&0) as f64;
        if n_s <= 0.0 || self.global_total == 0 { return 0.0; }
        let p_s = (self.scope_count(scope, name) as f64 + ALPHA) / (n_s + ALPHA * v);
        let p_g = (self.global_count(name) as f64 + ALPHA) / (self.global_total as f64 + ALPHA * v);
        if p_g <= 0.0 { return 0.0; }
        (p_s / p_g).log2() as f32
    }

    /// Lift mixed over the path hierarchy, nearest scope weighted highest.
    ///
    /// `decay` in (0, 1): 0.5 halves the weight at each step up. A decay of 1.0 weights the
    /// workspace as heavily as the file, which throws away the locality this exists to capture.
    pub fn lift(&self, path: &str, name: &str, decay: f32) -> f32 {
        let (mut acc, mut wsum, mut w) = (0f64, 0f64, 1f64);
        for s in scopes_of(path) {
            if !self.scope_totals.contains_key(&s) { w *= decay as f64; continue; }
            acc += w * self.lift_in(&s, name) as f64;
            wsum += w;
            w *= decay as f64;
        }
        if wsum <= 0.0 { return 0.0; }
        (acc / wsum) as f32
    }

    /// Which of two names this location prefers, and by how many bits. Positive favours `a`.
    ///
    /// The whole intended use: both names passed the corpus check, both exist, and the question is
    /// which one belongs here.
    pub fn prefer(&self, path: &str, a: &str, b: &str, decay: f32) -> f32 {
        self.lift(path, a, decay) - self.lift(path, b, decay)
    }
}

/// Additions and removals of each name within each scope, from history.
#[derive(Clone, Debug, Default)]
pub struct TrendIndex {
    added: HashMap<(String, String), u32>,
    removed: HashMap<(String, String), u32>,
}

impl TrendIndex {
    pub fn new() -> Self { Self::default() }

    /// Accumulate one unified diff. Only `+`/`-` body lines count; `+++`/`---` headers set the
    /// current path and are not content.
    ///
    /// Comments and strings are stripped from each line before identifiers are taken, for the same
    /// reason `corpus` strips them: a reworded doc comment is not a vocabulary change. Per-line
    /// stripping cannot track a block comment across lines, so a multi-line `/* */` inside a diff
    /// still leaks — accepted, because the alternative is reconstructing both file versions.
    pub fn add_diff(&mut self, diff: &str) {
        let mut path = String::new();
        for line in diff.lines() {
            if let Some(rest) = line.strip_prefix("+++ ") {
                path = rest.trim_start_matches("b/").trim().to_string();
                continue;
            }
            if line.starts_with("--- ") || line.starts_with("@@") || line.starts_with("diff ") { continue; }
            let (body, add) = match line.as_bytes().first() {
                Some(b'+') => (&line[1..], true),
                Some(b'-') => (&line[1..], false),
                _ => continue,
            };
            if path.is_empty() { continue; }
            let clean = crate::corpus::strip_noncode(body);
            for id in crate::corpus::identifiers(&clean) {
                for s in scopes_of(&path) {
                    let key = (s, id.clone());
                    let m = if add { &mut self.added } else { &mut self.removed };
                    *m.entry(key).or_insert(0) += 1;
                }
            }
        }
    }

    pub fn added(&self, scope: &str, name: &str) -> u32 {
        self.added.get(&(scope.to_string(), name.to_string())).copied().unwrap_or(0)
    }
    pub fn removed(&self, scope: &str, name: &str) -> u32 {
        self.removed.get(&(scope.to_string(), name.to_string())).copied().unwrap_or(0)
    }

    /// Net direction in [-1, 1] for one exact scope. `None` when the window contains too few
    /// mentions to mean anything — an identifier touched twice has no trend, and reporting one
    /// would be reporting noise with a decimal point on it.
    pub fn momentum_in(&self, scope: &str, name: &str, min_mentions: u32) -> Option<f32> {
        let (a, r) = (self.added(scope, name), self.removed(scope, name));
        if a + r < min_mentions.max(1) { return None; }
        Some((a as f32 - r as f32) / (a + r) as f32)
    }

    /// Momentum at the nearest scope that has enough mentions to report one.
    pub fn momentum(&self, path: &str, name: &str, min_mentions: u32) -> Option<f32> {
        scopes_of(path).into_iter().find_map(|s| self.momentum_in(&s, name, min_mentions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_run_from_the_file_outward() {
        assert_eq!(scopes_of("a/b/c.rs"), vec!["a/b/c.rs", "a/b", "a"]);
        assert_eq!(scopes_of("solo.rs"), vec!["solo.rs"]);
        assert!(scopes_of("").is_empty());
    }

    fn idx() -> ScopedIndex {
        let mut i = ScopedIndex::new();
        // `kv_dim` is native to kv/, `helper` is everywhere, `widget` is native to ui/.
        i.add_file("crate/kv/pages.rs", "fn a() { kv_dim(); kv_dim(); kv_dim(); helper(); }");
        i.add_file("crate/kv/pool.rs", "fn b() { kv_dim(); kv_dim(); helper(); }");
        i.add_file("crate/ui/view.rs", "fn c() { widget(); widget(); widget(); helper(); }");
        i.add_file("crate/ui/list.rs", "fn d() { widget(); widget(); helper(); }");
        i
    }

    #[test]
    fn a_name_native_to_a_directory_lifts_there_and_not_elsewhere() {
        let i = idx();
        assert!(i.lift("crate/kv/pages.rs", "kv_dim", 0.5) > 0.5);
        assert!(i.lift("crate/ui/view.rs", "kv_dim", 0.5) < -0.5);
    }

    #[test]
    fn a_name_common_everywhere_lifts_nowhere() {
        // The reason lift beats raw frequency: `helper` appears in every file and says nothing.
        let i = idx();
        for p in ["crate/kv/pages.rs", "crate/ui/view.rs"] {
            assert!(i.lift(p, "helper", 0.5).abs() < 0.6, "helper lifted at {p}");
        }
    }

    #[test]
    fn prefer_picks_the_local_vocabulary_between_two_real_names() {
        let i = idx();
        assert!(i.prefer("crate/kv/pool.rs", "kv_dim", "widget", 0.5) > 0.0);
        assert!(i.prefer("crate/ui/list.rs", "kv_dim", "widget", 0.5) < 0.0);
    }

    #[test]
    fn a_decay_of_one_flattens_the_hierarchy_and_loses_the_signal() {
        let i = idx();
        let sharp = i.lift("crate/kv/pages.rs", "kv_dim", 0.3);
        let flat = i.lift("crate/kv/pages.rs", "kv_dim", 1.0);
        assert!(sharp > flat, "sharp {sharp} flat {flat}");
    }

    #[test]
    fn an_unseen_name_lifts_negative_rather_than_panicking() {
        let i = idx();
        assert!(i.lift("crate/kv/pages.rs", "never_written", 0.5) <= 0.0);
        assert_eq!(i.lift("nowhere/at/all.rs", "kv_dim", 0.5), 0.0, "unknown scope is not an opinion");
    }

    #[test]
    fn a_diff_records_direction_per_scope() {
        let mut t = TrendIndex::new();
        t.add_diff("+++ b/crate/kv/pool.rs\n@@\n+    new_api();\n+    new_api();\n-    old_api();\n-    old_api();\n-    old_api();\n");
        assert_eq!(t.added("crate/kv/pool.rs", "new_api"), 2);
        assert_eq!(t.removed("crate/kv/pool.rs", "old_api"), 3);
        assert_eq!(t.added("crate/kv", "new_api"), 2, "counted at every enclosing scope");
    }

    #[test]
    fn momentum_separates_a_rising_name_from_a_retiring_one() {
        let mut t = TrendIndex::new();
        t.add_diff("+++ b/m/f.rs\n+a(); a(); a(); a();\n-b(); b(); b(); b();\n");
        assert_eq!(t.momentum("m/f.rs", "a", 2), Some(1.0));
        assert_eq!(t.momentum("m/f.rs", "b", 2), Some(-1.0));
    }

    #[test]
    fn too_few_mentions_reports_no_trend_rather_than_a_confident_one() {
        let mut t = TrendIndex::new();
        t.add_diff("+++ b/m/f.rs\n+once();\n");
        assert_eq!(t.momentum("m/f.rs", "once", 5), None);
        assert_eq!(t.momentum("m/f.rs", "once", 1), Some(1.0));
    }

    #[test]
    fn diff_headers_are_not_content() {
        // `+++ b/path` starts with '+' and would otherwise be read as an added line, putting the
        // path's own words into the vocabulary of every file it names.
        let mut t = TrendIndex::new();
        t.add_diff("diff --git a/m/f.rs b/m/f.rs\n--- a/m/f.rs\n+++ b/m/f.rs\n@@ -1 +1 @@\n+real();\n");
        assert_eq!(t.added("m/f.rs", "real"), 1);
        assert_eq!(t.added("m/f.rs", "f"), 0, "the path leaked into the counts");
    }

    #[test]
    fn comments_in_a_diff_are_not_vocabulary_changes() {
        let mut t = TrendIndex::new();
        t.add_diff("+++ b/m/f.rs\n+// reworded note about isosceles triangles\n+real();\n");
        assert_eq!(t.added("m/f.rs", "isosceles"), 0);
        assert_eq!(t.added("m/f.rs", "real"), 1);
    }
}
