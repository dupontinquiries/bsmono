//! Fuzzy path resolution against a filesystem index.
//!
//! The problem: voice transcribes paths like a forgetful person giving
//! directions. They might omit a segment ("Code/kwin" when it's actually
//! "Code/kde/kwin"), mishear a middle segment ("Code/kde/cable" when it's
//! "Code/kde/kwin"), or scramble word boundaries ("kege win" when it's
//! "kde kwin"). The literal `cd` command will fail; we want to surface the
//! likely intended path as a high-confidence alternative.
//!
//! ## Design
//!
//! A background thread maintains an index of directory paths under common
//! roots (~/Code, ~/Documents, ~/Downloads, plus the current cwd). The
//! index is rebuilt on demand and cached with a TTL.
//!
//! When voice produces a path-shaped utterance, we match the voiced
//! segments against indexed paths. Scoring:
//!
//!   - Each matching segment scores 1.0
//!   - A misspelled segment that's a separator-collapsed substring scores 0.6
//!   - A missing segment in the voiced path costs 0.3 (forgotten intersection)
//!   - An extra segment in the voiced path that doesn't match anything costs 0.5
//!   - First and last segments are weighted 1.5x (anchors are remembered better)
//!
//! Top-scored paths above a threshold are returned. If the literal voiced
//! path doesn't exist on disk but a high-confidence fuzzy match does, the
//! fuzzy match wins. If both exist, the literal wins.
//!
//! ## Why background-thread
//!
//! Walking ~/Code on a project-heavy disk can take 50-200ms. We can't
//! block voice processing for that. The index lives in an Arc<Mutex<...>>;
//! a background thread refreshes it periodically. Voice queries read the
//! current snapshot non-blockingly. If the snapshot is stale, the answer
//! might be slightly out of date — but stale dirname info is fine for
//! suggestion purposes, and the next refresh will catch up.
//!
//! ## Bounded
//!
//! Hard caps: 10000 paths total, depth 5 from each root, ignore the same
//! skip-dirs as completion.rs. On a 100k-file repo with deep nesting, we'd
//! still finish in <500ms because we're only counting directories, not files.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_PATHS: usize = 10000;
const MAX_DEPTH: usize = 5;
const STALE_AFTER: Duration = Duration::from_secs(60);

/// Skip these directory names. Same list as completion.rs — anything that's
/// a build artifact, dependency cache, or VCS internals.
const SKIP_DIRS: &[&str] = &[
    "target", "node_modules", ".git", ".cargo", "build",
    "dist", ".next", ".cache", "vendor", "__pycache__",
    ".venv", "venv", ".tox", ".mypy_cache", ".pytest_cache",
    ".idea", ".vscode", "Pods", ".gradle",
];

/// Common roots to index. Plus the current cwd at query time.
fn default_roots() -> Vec<PathBuf> {
    let home = match std::env::var("HOME") { Ok(h) => h, Err(_) => return Vec::new() };
    let h = PathBuf::from(&home);
    vec![
        h.join("Code"),
        h.join("Documents"),
        h.join("Downloads"),
        h.join("Desktop"),
        h.join("src"),       // some users keep code under ~/src
        h.join("workspace"), // others under ~/workspace
        h.clone(),           // shallow scan of $HOME itself for top-level dirs
    ].into_iter().filter(|p| p.is_dir()).collect()
}

/// One indexed directory path, kept as a string for cheap matching.
#[derive(Clone, Debug)]
struct IndexedPath {
    /// Display form, with ~ for home (matches what the user voiced).
    display: String,
    /// Lowercase segments split on /. Each is also collapsed of non-alphanum.
    /// ["home", "me", "code", "kde", "kwin"]
    segments: Vec<String>,
}

#[derive(Default)]
pub struct PathIndex {
    paths: Vec<IndexedPath>,
    built_at: Option<Instant>,
}

impl PathIndex {
    pub fn new() -> Self { Self::default() }

    /// Returns true if the index has never been built or is older than STALE_AFTER.
    pub fn is_stale(&self) -> bool {
        match self.built_at {
            None => true,
            Some(t) => t.elapsed() > STALE_AFTER,
        }
    }

    /// Rebuild from defaults plus any extra roots (typically the cwd).
    pub fn rebuild(&mut self, extra_roots: &[PathBuf]) {
        let mut all_roots = default_roots();
        for r in extra_roots {
            if r.is_dir() && !all_roots.contains(r) {
                all_roots.push(r.clone());
            }
        }
        let mut paths: Vec<IndexedPath> = Vec::with_capacity(1024);
        for root in &all_roots {
            walk_dir(root, root, 0, &mut paths);
            if paths.len() >= MAX_PATHS { break; }
        }
        self.paths = paths;
        self.built_at = Some(Instant::now());
    }

    /// v0.1.675: list indexed paths whose extension is a searchable text
    /// format (md/txt/markdown/org/rst), capped, for content (full-text)
    /// search. The display form is returned (~ for home).
    pub fn text_file_paths(&self, cap: usize) -> Vec<String> {
        const EXTS: &[&str] = &["md", "markdown", "txt", "org", "rst", "text"];
        self.paths.iter()
            .filter(|p| {
                let d = p.display.to_lowercase();
                EXTS.iter().any(|e| d.ends_with(&format!(".{}", e)))
            })
            .take(cap)
            .map(|p| p.display.clone())
            .collect()
    }

    /// Score a voiced path against the index and return top matches above threshold.
    /// Returns up to `limit` results in descending score order.
    pub fn fuzzy_match(&self, voiced: &str, limit: usize) -> Vec<(String, f32)> {
        let v_segments = split_segments(voiced);
        if v_segments.is_empty() { return Vec::new(); }

        let mut scored: Vec<(usize, f32)> = self.paths.iter().enumerate()
            .filter_map(|(i, p)| {
                let score = score_path(&v_segments, &p.segments);
                if score > 0.5 { Some((i, score)) } else { None }
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(limit)
            .map(|(i, s)| (self.paths[i].display.clone(), s))
            .collect()
    }
}

/// Walk a directory recursively, adding paths up to MAX_DEPTH and MAX_PATHS.
/// Skips SKIP_DIRS and hidden directories.
fn walk_dir(root: &Path, dir: &Path, depth: usize, out: &mut Vec<IndexedPath>) {
    if out.len() >= MAX_PATHS { return; }
    if depth > MAX_DEPTH { return; }
    let entries = match std::fs::read_dir(dir) { Ok(e) => e, Err(_) => return };
    for entry in entries.flatten() {
        if out.len() >= MAX_PATHS { return; }
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name.starts_with('.') { continue; }
        let ft = match entry.file_type() { Ok(f) => f, Err(_) => continue };
        if !ft.is_dir() { continue; }
        if SKIP_DIRS.contains(&name) { continue; }
        // Add this directory itself (we index directories, not files —
        // the user is `cd`-ing into something, not opening a file)
        let display = path_display(&path);
        let segments = split_segments(&display);
        out.push(IndexedPath { display, segments });
        walk_dir(root, &path, depth + 1, out);
    }
}

/// Format a path for display, replacing $HOME with ~.
fn path_display(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if let Ok(home) = std::env::var("HOME") {
        if s.starts_with(&home) {
            return format!("~{}", &s[home.len()..]);
        }
    }
    s
}

/// Split a path into lowercase, alphanumeric-only segments.
/// "~/Code/kde/kwin" → ["code", "kde", "kwin"]  (~ and intermediate slashes
/// collapse out, $HOME collapses to nothing useful so we just take what
/// looks like a word per slash-segment).
fn split_segments(s: &str) -> Vec<String> {
    s.split(|c: char| c == '/' || c == '\\')
        .map(|seg| {
            seg.to_lowercase().chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|seg| !seg.is_empty() && seg != "home")
        .collect()
}

/// Score how well `voiced` segments match `indexed` segments.
///
/// Strategy: walk both lists, allowing skips on either side with cost. A
/// segment match is exact (1.0) or substring (0.6 if a 3+ char overlap).
/// Skips on the indexed side cost 0.3 (user forgot a segment); skips on
/// the voiced side cost 0.5 (user added a wrong segment).
///
/// First and last matched segments get a 1.5x weight bonus — anchors are
/// remembered more reliably.
///
/// Final score is normalized to [0, 1] by dividing by the max possible
/// (sum of weights for an exact full match).
fn score_path(voiced: &[String], indexed: &[String]) -> f32 {
    if voiced.is_empty() || indexed.is_empty() { return 0.0; }
    // Greedy alignment: walk both lists, prefer matches; on mismatch, try
    // skipping one segment on each side and pick the better result. This
    // is O(n*m) but n and m are small (typically 2-5 segments each).
    let n = voiced.len();
    let m = indexed.len();
    // dp[i][j] = best score matching voiced[..i] against indexed[..j]
    let mut dp = vec![vec![0.0f32; m + 1]; n + 1];
    for i in 0..=n {
        for j in 0..=m {
            if i == 0 && j == 0 { continue; }
            let mut best = -1.0f32;
            // Match (or skip-misspelled-match) at this position
            if i > 0 && j > 0 {
                let segment_score = match_segments(&voiced[i-1], &indexed[j-1]);
                let weight = if i == 1 || i == n || j == 1 || j == m { 1.5 } else { 1.0 };
                let cand = dp[i-1][j-1] + segment_score * weight;
                if cand > best { best = cand; }
            }
            // Skip this voiced segment (user added something extra)
            if i > 0 {
                let cand = dp[i-1][j] - 0.5;
                if cand > best { best = cand; }
            }
            // Skip this indexed segment (user forgot something)
            if j > 0 {
                let cand = dp[i][j-1] - 0.3;
                if cand > best { best = cand; }
            }
            if best > 0.0 { dp[i][j] = best; }
        }
    }
    let raw = dp[n][m];
    // Normalize: max possible is roughly 1.5 * 2 + 1.0 * (max(n,m) - 2)
    // for the anchor weighting. Cap at 1.0.
    let max_possible = if n.max(m) >= 2 {
        3.0 + (n.max(m) as f32 - 2.0)
    } else {
        1.5 * n.max(m) as f32
    };
    if max_possible <= 0.0 { return 0.0; }
    (raw / max_possible).clamp(0.0, 1.0)
}

/// Score one voiced segment against one indexed segment.
/// Returns 1.0 for exact match, 0.6 for substring containment of length 3+,
/// 0.0 otherwise.
fn match_segments(voiced: &str, indexed: &str) -> f32 {
    if voiced == indexed { return 1.0; }
    if voiced.len() < 2 || indexed.len() < 2 { return 0.0; }
    // Substring containment in either direction
    if voiced.len() >= 3 && indexed.contains(voiced) { return 0.6; }
    if indexed.len() >= 3 && voiced.contains(indexed) { return 0.6; }
    // Single-char-difference Levenshtein? Cheap version: equal length and
    // 1-char diff means probably misheard. "kwin" vs "kwen" etc.
    if voiced.len() == indexed.len() && voiced.len() >= 4 {
        let diffs: usize = voiced.chars().zip(indexed.chars())
            .filter(|(a, b)| a != b).count();
        if diffs == 1 { return 0.5; }
    }
    0.0
}

/// Shared, thread-safe path index used by Backend.
pub type SharedIndex = Arc<Mutex<PathIndex>>;

pub fn new_shared() -> SharedIndex {
    Arc::new(Mutex::new(PathIndex::new()))
}

/// Spawn a background refresh of the index if it's stale. Non-blocking.
/// Caller should pass the current cwd as an extra root so it gets indexed
/// even if it's outside the default roots.
pub fn refresh_in_background(idx: SharedIndex, extra_roots: Vec<PathBuf>) {
    // Quick non-blocking check — if not stale, return immediately
    {
        let g = match idx.lock() { Ok(g) => g, Err(_) => return };
        if !g.is_stale() { return; }
    }
    std::thread::spawn(move || {
        // Build outside the lock to avoid blocking voice queries during walk
        let mut new_idx = PathIndex::new();
        new_idx.rebuild(&extra_roots);
        if let Ok(mut g) = idx.lock() {
            *g = new_idx;
        }
        crate::clog!("path_fuzzy", "index rebuilt");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_segments_basic() {
        let s = split_segments("~/Code/kde/kwin");
        assert_eq!(s, vec!["code", "kde", "kwin"]);
    }

    #[test]
    fn match_misheard_segment() {
        // User voiced "Code/kde/cable" but real path is "Code/kde/kwin"
        let voiced = vec!["code".into(), "kde".into(), "cable".into()];
        let indexed = vec!["code".into(), "kde".into(), "kwin".into()];
        let s = score_path(&voiced, &indexed);
        // Should still be > 0.5 because two of three segments match exactly
        // and they're the anchors (start and middle, with end mismatched)
        assert!(s > 0.4, "score {} too low", s);
    }

    #[test]
    fn match_missing_segment() {
        // User voiced "Code/kwin" but real path is "Code/kde/kwin"
        let voiced = vec!["code".into(), "kwin".into()];
        let indexed = vec!["code".into(), "kde".into(), "kwin".into()];
        let s = score_path(&voiced, &indexed);
        assert!(s > 0.5, "score {} too low for missing-segment case", s);
    }

    #[test]
    fn full_match_scores_high() {
        let voiced = vec!["code".into(), "kde".into(), "kwin".into()];
        let indexed = vec!["code".into(), "kde".into(), "kwin".into()];
        let s = score_path(&voiced, &indexed);
        assert!(s > 0.9, "exact match should score very high, got {}", s);
    }

    #[test]
    fn unrelated_paths_score_low() {
        let voiced = vec!["foo".into(), "bar".into(), "baz".into()];
        let indexed = vec!["code".into(), "kde".into(), "kwin".into()];
        let s = score_path(&voiced, &indexed);
        assert!(s < 0.3, "unrelated paths should score low, got {}", s);
    }

    #[test]
    fn empty_inputs_score_zero() {
        assert_eq!(score_path(&[], &["code".into()]), 0.0);
        assert_eq!(score_path(&["code".into()], &[]), 0.0);
        assert_eq!(score_path(&[], &[]), 0.0);
    }

    #[test]
    fn partial_tail_match_beats_unrelated() {
        // User says just the last segment of a deeper path. Should still
        // score better than a totally unrelated path of the same shape.
        let voiced = vec!["kwin".into()];
        let on_path = score_path(&voiced, &["code".into(), "kde".into(), "kwin".into()]);
        let off_path = score_path(&voiced, &["code".into(), "kde".into(), "dolphin".into()]);
        assert!(on_path > off_path,
            "matching the named tail segment should win: on={on_path} off={off_path}");
    }

    #[test]
    fn anchor_segments_weighted() {
        // The scorer weights first/last segments 1.5x. Matching the
        // final segment (the thing the user usually names) should score
        // higher than matching only a middle segment.
        let last = score_path(&vec!["kwin".into()],
            &["a".into(), "b".into(), "kwin".into()]);
        let middle = score_path(&vec!["b".into()],
            &["a".into(), "b".into(), "kwin".into()]);
        assert!(last >= middle,
            "final-segment match should be >= middle-segment: last={last} middle={middle}");
    }

    #[test]
    fn single_exact_segment_scores_high() {
        let s = score_path(&vec!["kwin".into()], &vec!["kwin".into()]);
        assert!(s > 0.9, "single exact segment should score high, got {s}");
    }
}
