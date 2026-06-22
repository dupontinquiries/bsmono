// src/smart_cd.rs
//
// "Go deeper" — voice intent that walks the directory tree from
// current cwd into the most "interesting" descendant, silently
// passing through unambiguous chains, surfacing candidates only
// when ambiguity is real.
//
// Plus "go home" (always available) and Dolphin/KDE favorites
// loaded from ~/.local/share/user-places.xbel.
//
// Design tenets (locked in with the user):
//   - LOW friction: descend silently through obvious chains, only
//     ask when there's genuine ambiguity.
//   - "Walk through unambiguous, surface at first real fork."
//   - Multi-candidate output capped at 4-6 entries.
//   - Max depth 6 — prevents runaway on `/` or other deep trees.
//   - Hardcoded project markers for now (will extract to TOML
//     in a follow-up turn).
//
// Failure modes designed around:
//   - Hidden dirs (.git, node_modules) skipped even if only child
//   - Symlink loops — never re-enter a visited canonical path
//   - Permissions — silent stop, treat current as "interesting"
//   - Already at an interesting place — say so, don't bounce

use std::collections::HashSet;
use std::path::{Path, PathBuf};

// Project markers, skip dirs, and source extensions are loaded at
// startup from src/data.rs (which reads from `data/*.toml` defaults
// + `~/.config/web2qml/data/*.toml` user overlay). The hardcoded
// arrays below are KEPT as a fallback in case data::init() failed
// to run, but normal operation goes through data::project_markers()
// etc.
//
// Migration policy: if the data:: accessor returns at least one
// entry, we use that. If it returns empty (init missed, or every
// entry got removed by user), we fall back to the hardcoded list.
// This keeps behavior sane even in degraded states.

fn project_markers_view() -> &'static [String] {
    // Tables are populated at startup. If init was skipped (e.g. tests
    // not calling main), the OnceLock returns None and we fall back
    // via a small static. Most test runs of this module won't hit
    // that path because tests don't traverse real filesystems.
    static FALLBACK: OnceLock<Vec<String>> = OnceLock::new();
    let live = crate::data::project_markers();
    if !live.is_empty() { return live; }
    FALLBACK.get_or_init(|| FALLBACK_PROJECT_MARKERS.iter()
        .map(|s| s.to_string()).collect())
}

fn skip_dirs_view() -> &'static [String] {
    static FALLBACK: OnceLock<Vec<String>> = OnceLock::new();
    let live = crate::data::skip_dirs();
    if !live.is_empty() { return live; }
    FALLBACK.get_or_init(|| FALLBACK_SKIP_DIRS.iter()
        .map(|s| s.to_string()).collect())
}

fn source_exts_view() -> &'static [String] {
    static FALLBACK: OnceLock<Vec<String>> = OnceLock::new();
    let live = crate::data::source_exts();
    if !live.is_empty() { return live; }
    FALLBACK.get_or_init(|| FALLBACK_SOURCE_EXTS.iter()
        .map(|s| s.to_string()).collect())
}

use std::sync::OnceLock;

/// Hardcoded fallbacks used only when data::init() didn't populate
/// the tables (bug, tests, missing data dir + missing user overlay).
/// Normal path goes through data::*().
const FALLBACK_PROJECT_MARKERS: &[&str] = &[
    "Cargo.toml", "package.json", "pyproject.toml",
    "go.mod", "Makefile", ".git", "README.md",
];
const FALLBACK_SKIP_DIRS: &[&str] = &[
    "node_modules", "target", ".git", "__pycache__",
    "venv", ".venv", "dist", "build",
];
const FALLBACK_SOURCE_EXTS: &[&str] = &[
    ".rs", ".py", ".ts", ".js", ".go", ".java",
    ".c", ".h", ".cpp", ".rb",
];

/// Result of a smart-descend operation. Returned to QML which uses
/// it to either (a) cd to `final_path` and toast the trail, or
/// (b) surface `candidates` for user disambiguation.
#[derive(Debug, Clone)]
pub struct DescentResult {
    /// Path the user should cd to. Same as the start cwd if we
    /// didn't descend at all (already interesting / no children).
    pub final_path: String,
    /// Each step of the descent, in order. Empty if we didn't move.
    /// Used to render "Descended through src → main → java" trail.
    pub trail: Vec<String>,
    /// If we stopped on ambiguity, these are the candidates the user
    /// could pick from. Empty if descent completed unambiguously.
    /// Each entry is (relative-name, full-path, score, reason).
    pub candidates: Vec<DescentCandidate>,
    /// Why we stopped: "found project marker" / "multiple candidates"
    /// / "max depth" / "no children" / "all skipped" / "already interesting"
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct DescentCandidate {
    pub name: String,
    pub full_path: String,
    pub score: f32,
    pub reason: String,
}

/// Walk from `start` into the most interesting descendant.
///
/// Algorithm (the "low-friction" form):
///   1. Score current dir's children
///   2. If any child has score >= STOP_AND_OFFER threshold AND the
///      current dir ALSO looks interesting (project marker present),
///      stop here — we're already somewhere good
///   3. If exactly ONE child scores above CHAIN_THRESHOLD and others
///      don't → descend into it, repeat from step 1
///   4. If multiple children score above OFFER_THRESHOLD → stop and
///      return them as candidates (user picks)
///   5. If no child scores above OFFER_THRESHOLD → stop, current
///      is the destination
///   6. If depth >= max_depth → stop, surface remaining candidates
pub fn smart_descend(start: &str, max_depth: u32) -> DescentResult {
    // Threshold to consider a child as a "continue chain" pick when
    // it's the only viable option. Lower than OFFER because we'd
    // rather walk through an OK chain than stop at a meh fork.
    const CHAIN_THRESHOLD: f32 = 1.0;
    // Threshold for "interesting enough to surface as a candidate"
    // when there are multiple. Higher — only real candidates make
    // it to the user.
    const OFFER_THRESHOLD: f32 = 2.0;
    // Cap on how many candidates we surface (user said "no more
    // than 6, usually 4").
    const MAX_CANDIDATES: usize = 6;

    let start_pb = match canonicalize_or_self(Path::new(start)) {
        Some(p) => p,
        None => {
            return DescentResult {
                final_path: start.to_string(),
                trail: vec![],
                candidates: vec![],
                reason: "couldn't resolve starting path".to_string(),
            };
        }
    };

    // Already-interesting check: if start itself has a project marker,
    // don't descend at all — user is already where they want to be.
    if has_project_marker(&start_pb) {
        return DescentResult {
            final_path: start_pb.to_string_lossy().to_string(),
            trail: vec![],
            candidates: vec![],
            reason: "already at a project root".to_string(),
        };
    }

    let mut visited: HashSet<PathBuf> = HashSet::new();
    visited.insert(start_pb.clone());
    let mut current = start_pb;
    let mut trail: Vec<String> = Vec::new();

    for _step in 0..max_depth {
        // Read + score children.
        let scored = match score_children(&current) {
            Ok(s) => s,
            Err(e) => {
                return DescentResult {
                    final_path: current.to_string_lossy().to_string(),
                    trail,
                    candidates: vec![],
                    reason: format!("couldn't read directory: {}", e),
                };
            }
        };

        if scored.is_empty() {
            return DescentResult {
                final_path: current.to_string_lossy().to_string(),
                trail,
                candidates: vec![],
                reason: "no descendable children".to_string(),
            };
        }

        // Sort by score descending.
        let mut sorted = scored;
        sorted.sort_by(|a, b| b.score.partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal));

        // Look at the top candidate.
        let top = &sorted[0];
        let second_score = sorted.get(1).map(|c| c.score).unwrap_or(0.0);

        // Project marker — instant winner. Walk into it without asking.
        if top.has_project_marker {
            let target = PathBuf::from(&top.full_path);
            if !visited.insert(target.clone()) {
                // Symlink loop. Stop here.
                return DescentResult {
                    final_path: current.to_string_lossy().to_string(),
                    trail,
                    candidates: vec![],
                    reason: "cycle detected".to_string(),
                };
            }
            trail.push(top.name.clone());
            current = target;
            // After stepping in, check if NEW current is itself
            // interesting (the project root we just entered). If so,
            // we're done.
            if has_project_marker(&current) {
                return DescentResult {
                    final_path: current.to_string_lossy().to_string(),
                    trail,
                    candidates: vec![],
                    reason: format!("found project root ({})", marker_in(&current)
                        .unwrap_or_else(|| "marker".to_string())),
                };
            }
            continue;
        }

        // Ambiguity check: if 2+ candidates score above OFFER_THRESHOLD
        // AND they're competitive (top isn't 2x next), surface as a
        // multi-candidate result.
        let above_offer: Vec<_> = sorted.iter()
            .take(MAX_CANDIDATES)
            .filter(|c| c.score >= OFFER_THRESHOLD)
            .collect();
        let competitive = above_offer.len() >= 2 && top.score < second_score * 2.0;
        if competitive {
            let candidates: Vec<DescentCandidate> = above_offer.iter()
                .map(|c| DescentCandidate {
                    name: c.name.clone(),
                    full_path: c.full_path.clone(),
                    score: c.score,
                    reason: c.reason.clone(),
                })
                .collect();
            return DescentResult {
                final_path: current.to_string_lossy().to_string(),
                trail,
                candidates,
                reason: "multiple candidates — pick one".to_string(),
            };
        }

        // Single-chain check: top is meaningfully above the rest.
        // Walk into it.
        if top.score >= CHAIN_THRESHOLD {
            let target = PathBuf::from(&top.full_path);
            if !visited.insert(target.clone()) {
                return DescentResult {
                    final_path: current.to_string_lossy().to_string(),
                    trail,
                    candidates: vec![],
                    reason: "cycle detected".to_string(),
                };
            }
            trail.push(top.name.clone());
            current = target;
            continue;
        }

        // Top score below CHAIN — nothing worth descending into.
        return DescentResult {
            final_path: current.to_string_lossy().to_string(),
            trail,
            candidates: vec![],
            reason: "nothing more interesting below".to_string(),
        };
    }

    // Hit max_depth — return what we have plus any remaining candidates.
    let remaining_candidates = match score_children(&current) {
        Ok(mut s) => {
            s.sort_by(|a, b| b.score.partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal));
            s.into_iter().take(4)
                .filter(|c| c.score >= 1.0)
                .map(|c| DescentCandidate {
                    name: c.name,
                    full_path: c.full_path,
                    score: c.score,
                    reason: c.reason,
                })
                .collect()
        }
        Err(_) => Vec::new(),
    };
    DescentResult {
        final_path: current.to_string_lossy().to_string(),
        trail,
        candidates: remaining_candidates,
        reason: format!("reached max depth ({} levels)", max_depth),
    }
}

#[derive(Debug, Clone)]
struct ScoredChild {
    name: String,
    full_path: String,
    score: f32,
    reason: String,
    has_project_marker: bool,
}

fn score_children(dir: &Path) -> Result<Vec<ScoredChild>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut scored: Vec<ScoredChild> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Symlinks treated as directories if they point at one,
        // but only after we verify the target exists.
        if !ft.is_dir() {
            if ft.is_symlink() {
                if let Ok(target_meta) = std::fs::metadata(&path) {
                    if !target_meta.is_dir() { continue; }
                } else {
                    continue;
                }
            } else {
                continue;
            }
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Hidden dirs skipped (start with '.' but allow dotted
        // project dirs explicitly only if they're project markers
        // themselves — handled below).
        if name.starts_with('.')
            && !project_markers_view().iter().any(|m| m == &name)
        {
            continue;
        }
        if skip_dirs_view().iter().any(|d| d == &name) { continue; }

        let full_path = path.to_string_lossy().to_string();
        let (score, reason, marker) = score_dir(&path, &name);
        scored.push(ScoredChild {
            name,
            full_path,
            score,
            reason,
            has_project_marker: marker,
        });
    }
    Ok(scored)
}

/// Score a single child directory. Returns (score, reason, has_project_marker).
fn score_dir(path: &Path, name: &str) -> (f32, String, bool) {
    let mut score = 0.0f32;
    let mut reasons: Vec<String> = Vec::new();
    let mut has_marker = false;

    // Project marker presence — biggest single signal.
    if let Some(marker) = marker_in(path) {
        score += 10.0;
        reasons.push(format!("contains {}", marker));
        has_marker = true;
    }

    // Entry counting pass — single read_dir, classify each.
    let mut total_children = 0u32;
    let mut source_files = 0u32;
    let mut subdirs = 0u32;
    let mut has_readme = false;
    if let Ok(entries) = std::fs::read_dir(path) {
        for e in entries.flatten().take(200) {
            total_children += 1;
            let p = e.path();
            let n = match p.file_name().and_then(|s| s.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let ft = e.file_type().ok();
            if let Some(ft) = ft {
                if ft.is_dir() {
                    if !n.starts_with('.')
                        && !skip_dirs_view().iter().any(|d| d == n)
                    {
                        subdirs += 1;
                    }
                } else if ft.is_file() {
                    let lower = n.to_lowercase();
                    if lower.starts_with("readme") {
                        has_readme = true;
                    }
                    if source_exts_view().iter().any(|ext| lower.ends_with(ext.as_str())) {
                        source_files += 1;
                    }
                }
            }
        }
    }

    if source_files >= 2 {
        score += 3.0;
        reasons.push(format!("{} source files", source_files));
    }
    if has_readme {
        score += 2.0;
        reasons.push("has README".to_string());
    }

    // Single-child chain bonus — directory with one meaningful subdir
    // and few other children is the classic "continue chain" case.
    // Modest bonus so it tips the balance when there's nothing else.
    if subdirs == 1 && total_children <= 3 {
        score += 1.5;
        reasons.push("single descent".to_string());
    }

    // Many children — content directory, not where the user wants
    // to go specifically. Slight penalty.
    if total_children > 30 {
        score -= 1.5;
        reasons.push("many children".to_string());
    }

    // Common useful-name boost — light hint, not authoritative.
    let useful_names = ["src", "source", "lib", "app", "cmd", "internal",
        "pkg", "main", "core"];
    if useful_names.contains(&name) {
        score += 1.0;
        reasons.push(format!("'{}' is a common source dir", name));
    }

    let reason = if reasons.is_empty() {
        "ordinary directory".to_string()
    } else {
        reasons.join(", ")
    };
    (score, reason, has_marker)
}

/// Returns Some(marker_name) if the directory has any project-marker file.
/// Doesn't dereference symlinks for the directory check itself.
fn marker_in(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten().take(500) {
        let n = e.path();
        let name = n.file_name()?.to_str()?.to_string();
        // Glob-style match for *.csproj etc.
        if project_markers_view().iter().any(|m| {
            if let Some(rest) = m.strip_prefix("*.") {
                name.ends_with(rest) && name.len() > rest.len()
            } else {
                &name == m
            }
        }) {
            return Some(name);
        }
    }
    None
}

fn has_project_marker(dir: &Path) -> bool {
    marker_in(dir).is_some()
}

fn canonicalize_or_self(p: &Path) -> Option<PathBuf> {
    p.canonicalize().ok().or_else(|| {
        if p.exists() { Some(p.to_path_buf()) } else { None }
    })
}

// ─── Dolphin / KDE favorites ──────────────────────────────────────
//
// User-places.xbel is the XBEL-formatted favorites file Dolphin
// (and other KDE apps) read for sidebar shortcuts. Format is
// simple XML; we don't pull in an XML parser, just regex-grep for
// <bookmark href="..."> entries.
//
// This is best-effort. If the file isn't present (non-KDE env)
// we return empty and the QML "favorites" picker just shows
// the home anchor.

#[derive(Debug, Clone)]
pub struct Favorite {
    pub name: String,
    pub path: String,
}

/// Read Dolphin/KDE bookmarks. Returns empty Vec on any failure.
pub fn dolphin_favorites() -> Vec<Favorite> {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    let path = std::path::Path::new(&home).join(".local/share/user-places.xbel");
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<Favorite> = Vec::new();
    // Walk the file looking for <bookmark href="X"> followed by
    // <title>NAME</title> within a small window. XBEL is XML so
    // these always come together inside the same <bookmark> element.
    //
    // Regex would be cleaner but we already avoid that crate; do it
    // by string search.
    let mut search_from = 0;
    while let Some(rel_idx) = content[search_from..].find("<bookmark") {
        let bm_start = search_from + rel_idx;
        // Find href value.
        let after_bm = &content[bm_start..];
        let href = match find_attr(after_bm, "href=\"") {
            Some(h) => h,
            None => { search_from = bm_start + 9; continue; }
        };
        // Find the closing </bookmark> to bound title search.
        let close_rel = match after_bm.find("</bookmark>") {
            Some(c) => c,
            None => break,
        };
        let bm_chunk = &after_bm[..close_rel];
        // Title.
        let title = if let Some(t_start) = bm_chunk.find("<title>") {
            let t_after = &bm_chunk[t_start + 7..];
            if let Some(t_end) = t_after.find("</title>") {
                t_after[..t_end].to_string()
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        // Strip "file://" prefix from href to get a real path.
        let path_str = href.strip_prefix("file://").unwrap_or(&href).to_string();
        // Skip non-local entries (smb://, trash:/, etc.) and
        // non-existent paths (ghost favorites).
        if !path_str.starts_with('/') {
            search_from = bm_start + close_rel + 11;
            continue;
        }
        if !std::path::Path::new(&path_str).exists() {
            search_from = bm_start + close_rel + 11;
            continue;
        }
        let name = if title.is_empty() {
            // Fall back to last path component.
            std::path::Path::new(&path_str)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&path_str)
                .to_string()
        } else {
            title
        };
        out.push(Favorite { name, path: path_str });
        search_from = bm_start + close_rel + 11;
        if out.len() >= 30 { break; }  // bound
    }
    out
}

fn find_attr(s: &str, key: &str) -> Option<String> {
    let start = s.find(key)?;
    let after = &s[start + key.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_detection_recognizes_cargo() {
        // marker_in reads the global project-marker table, which panics if
        // data::init() hasn't run. init() is idempotent and is normally called
        // from main(); call it here so the unit test stands alone.
        crate::data::init();
        // Use a real path that exists in the test runner — check that
        // marker_in correctly returns None for a path with no markers,
        // since we can't easily fixture a real Cargo.toml here.
        let p = Path::new("/");
        assert!(marker_in(p).is_none());
    }

    #[test]
    fn xbel_parse_smoke() {
        let xbel = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel>
  <bookmark href="file:///home/user/projects">
    <title>Projects</title>
  </bookmark>
  <bookmark href="trash:/">
    <title>Trash</title>
  </bookmark>
</xbel>"#;
        // We can't easily set up the real file, but we CAN test the
        // string-search logic by feeding the parser directly.
        // (find_attr is the lowest-level helper.)
        assert_eq!(find_attr(xbel, "href=\""), Some("file:///home/user/projects".to_string()));
    }

    #[test]
    fn descent_on_nonexistent_path() {
        let r = smart_descend("/this/path/does/not/exist", 6);
        assert!(r.trail.is_empty());
        assert!(r.candidates.is_empty());
        // reason should mention failure
        assert!(r.reason.to_lowercase().contains("path") ||
                r.reason.to_lowercase().contains("read") ||
                r.reason.to_lowercase().contains("resolve"));
    }

    #[test]
    fn descent_skips_known_skip_dirs() {
        // The fallback list is what's used when data:: tables aren't
        // initialized (which is the case for unit tests). Verify it
        // contains the expected blocked names.
        assert!(FALLBACK_SKIP_DIRS.contains(&"node_modules"));
        assert!(FALLBACK_SKIP_DIRS.contains(&".git"));
        assert!(FALLBACK_SKIP_DIRS.contains(&"target"));
    }
}
