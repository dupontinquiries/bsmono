//! v0.1.682: path completion — CLEAN-ROOM reimplementation.
//!
//! Studied the MIT `autocomplete-paths` (Atom) package for its IDEAS, then
//! reimplemented the algorithm independently in Rust. NOTHING was copied; the
//! concepts borrowed are: (1) trigger on a scope+prefix (we trigger inside a
//! `[[` ZBracket or a markdown link/image `](`), (2) directory-aware listing
//! when the typed prefix contains `./`, `../`, or a trailing slash, (3)
//! per-context extension filtering, (4) fuzzy match with DISTANCE-TO-FILE
//! weighting (closer files rank higher), and (5) relative-path insertion with
//! an optional leading `./` and extension stripping.
//!
//! STATUS: pure core + tests, NOT wired live (awaiting greenlight). The host
//! supplies candidate paths (from path_fuzzy::PathIndex) + the current file's
//! path; this decides what to offer and how to render/insert it.

/// What the user typed after the trigger (the "path prefix"), parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPrefix {
    /// directory part typed so far ("" if none), e.g. "../assets"
    pub dir: String,
    /// the partial filename being typed (the fuzzy needle), e.g. "out"
    pub base: String,
    /// true if the prefix points into a directory (has ./ ../ or trailing /)
    pub directory_given: bool,
}

/// Parse the raw text typed after the trigger into (dir, base, directory_given).
/// Mirrors the idea that `path.parse` ignores trailing slashes (handled here).
pub fn parse_path_prefix(raw: &str) -> PathPrefix {
    let trailing_slash = raw.ends_with('/') || raw.ends_with('\\');
    let directory_given = raw.starts_with("./") || raw.starts_with("../") || trailing_slash;
    if trailing_slash {
        return PathPrefix {
            dir: raw.trim_end_matches(['/', '\\']).to_string(),
            base: String::new(),
            directory_given,
        };
    }
    match raw.rsplit_once('/') {
        Some((dir, base)) => PathPrefix {
            dir: dir.to_string(), base: base.to_string(), directory_given,
        },
        None => PathPrefix {
            dir: String::new(), base: raw.to_string(), directory_given,
        },
    }
}

/// A ranked path completion.
#[derive(Debug, Clone, PartialEq)]
pub struct PathCompletion {
    /// the text to INSERT (relative path, possibly `./`-prefixed, ext-stripped)
    pub insert: String,
    /// what to SHOW (the relative-to-current display form)
    pub display: String,
    pub score: f32,
}

/// Options controlling insertion form (per-context, like the JS "scope").
#[derive(Debug, Clone)]
pub struct CompleteOpts {
    /// only offer files with these extensions (empty = any)
    pub extensions: Vec<String>,
    /// insert a path relative to the current file (vs project-relative)
    pub relative: bool,
    /// prefix a leading `./` when the relative path doesn't start with `.`
    pub include_current_dir: bool,
    /// (regex-free) extension suffixes to strip on insert, e.g. [".md"]
    pub strip_suffixes: Vec<String>,
    pub max_results: usize,
}

impl Default for CompleteOpts {
    fn default() -> Self {
        CompleteOpts {
            extensions: Vec::new(), relative: true, include_current_dir: true,
            strip_suffixes: Vec::new(), max_results: 10,
        }
    }
}

/// Number of path segments between `from_dir` and `to_path` — the
/// "distance to file". Closer files (fewer segments) rank higher.
fn distance(from_dir: &str, to_path: &str) -> usize {
    let rel = relative_path(from_dir, to_path);
    rel.split('/').filter(|s| !s.is_empty()).count()
}

/// Compute a path relative to `from_dir` (both absolute, '/'-separated).
/// Clean-room: standard up-dir/down-dir diffing.
pub fn relative_path(from_dir: &str, to_path: &str) -> String {
    let from: Vec<&str> = from_dir.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = to_path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let common = from.iter().zip(to.iter()).take_while(|(a, b)| a == b).count();
    let ups = from.len() - common;
    let mut parts: Vec<String> = std::iter::repeat("..".to_string()).take(ups).collect();
    parts.extend(to[common..].iter().map(|s| s.to_string()));
    if parts.is_empty() { ".".to_string() } else { parts.join("/") }
}

/// Case-insensitive subsequence fuzzy match → Some(score) or None.
/// Higher score = better (contiguous + early matches favored). Clean-room.
pub fn fuzzy_score(needle: &str, hay: &str) -> Option<f32> {
    if needle.is_empty() { return Some(0.5); }
    let n: Vec<char> = needle.to_lowercase().chars().collect();
    let h: Vec<char> = hay.to_lowercase().chars().collect();
    let mut ni = 0;
    let mut score = 0.0f32;
    let mut last_match: isize = -2;
    let mut streak = 0.0f32;
    for (hi, &hc) in h.iter().enumerate() {
        if ni < n.len() && hc == n[ni] {
            // contiguous matches build a streak; gaps reset it
            if hi as isize == last_match + 1 { streak += 1.0; } else { streak = 1.0; }
            score += streak;
            // early position bonus
            if hi < 4 { score += 0.5; }
            last_match = hi as isize;
            ni += 1;
        }
    }
    if ni == n.len() { Some(score) } else { None }
}

/// Build ranked path completions.
/// - `candidates`: project file paths (absolute, '/'-separated).
/// - `current_file`: absolute path of the file being edited.
/// - `raw_prefix`: the text typed after the trigger.
pub fn complete(candidates: &[String], current_file: &str, raw_prefix: &str,
                opts: &CompleteOpts) -> Vec<PathCompletion> {
    let pp = parse_path_prefix(raw_prefix);
    let cur_dir = parent_dir(current_file);

    // when a directory is given, restrict to files under the requested dir,
    // and fuzzy-match only the base; otherwise match the whole prefix.
    let req_dir = if pp.directory_given {
        Some(resolve(&cur_dir, &pp.dir))
    } else { None };
    let needle = if pp.directory_given { pp.base.clone() } else { raw_prefix.to_string() };

    let mut out: Vec<PathCompletion> = Vec::new();
    for cand in candidates {
        // extension filter
        if !opts.extensions.is_empty() {
            let ok = opts.extensions.iter().any(|e| {
                cand.to_lowercase().ends_with(&format!(".{}", e.trim_start_matches('.').to_lowercase()))
            });
            if !ok { continue; }
        }
        // directory restriction
        if let Some(rd) = &req_dir {
            if parent_dir(cand) != *rd { continue; }
        }
        let rel = relative_path(&cur_dir, cand);
        // what we fuzzy-match against
        let match_target = if pp.directory_given { base_name(cand) } else { rel.clone() };
        let fz = match fuzzy_score(&needle, &match_target) { Some(s) => s, None => continue };

        // distance weighting: closer files get a bump
        let dist = distance(&cur_dir, cand) as f32;
        let score = fz + 2.0 / (1.0 + dist);

        // insertion form
        let mut insert = if opts.relative {
            let mut r = rel.clone();
            if opts.include_current_dir && !r.starts_with('.') { r = format!("./{}", r); }
            r
        } else {
            cand.clone()
        };
        for suf in &opts.strip_suffixes {
            if insert.ends_with(suf) {
                insert.truncate(insert.len() - suf.len());
                break;
            }
        }
        out.push(PathCompletion { insert, display: rel, score });
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.display.cmp(&b.display)));
    out.truncate(opts.max_results);
    out
}

fn parent_dir(p: &str) -> String {
    match p.rsplit_once('/') { Some((d, _)) => d.to_string(), None => String::new() }
}
fn base_name(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_string()
}
/// Resolve `rel` against `base` (handles `.` and `..`). '/'-separated.
fn resolve(base: &str, rel: &str) -> String {
    let mut segs: Vec<String> = base.trim_matches('/').split('/')
        .filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => { segs.pop(); }
            other => segs.push(other.to_string()),
        }
    }
    format!("/{}", segs.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prefix_forms() {
        assert_eq!(parse_path_prefix("out"),
            PathPrefix { dir: "".into(), base: "out".into(), directory_given: false });
        let p = parse_path_prefix("../assets/out");
        assert_eq!(p.dir, "../assets"); assert_eq!(p.base, "out"); assert!(p.directory_given);
        let t = parse_path_prefix("./img/");
        assert_eq!(t.base, ""); assert!(t.directory_given); assert_eq!(t.dir, "./img");
    }

    #[test]
    fn relative_path_basics() {
        assert_eq!(relative_path("/a/b", "/a/b/c.md"), "c.md");
        assert_eq!(relative_path("/a/b", "/a/c.md"), "../c.md");
        assert_eq!(relative_path("/a/b/c", "/a/x/y.md"), "../../x/y.md");
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        assert!(fuzzy_score("oc", "outer_cloak").is_some());
        assert!(fuzzy_score("xyz", "outer_cloak").is_none());
        // contiguous beats scattered
        let a = fuzzy_score("out", "outer").unwrap();
        let b = fuzzy_score("out", "o_u_t").unwrap();
        assert!(a > b);
    }

    #[test]
    fn complete_ranks_closer_files_higher() {
        let cands = vec![
            "/proj/notes/a/deep/cloak.md".to_string(),
            "/proj/notes/cloak.md".to_string(),
        ];
        let opts = CompleteOpts::default();
        let r = complete(&cands, "/proj/notes/cur.md", "cloak", &opts);
        // the sibling cloak.md (distance 1) ranks above the deep one
        assert_eq!(r[0].display, "cloak.md");
    }

    #[test]
    fn complete_extension_filter() {
        let cands = vec![
            "/p/img.png".to_string(), "/p/doc.md".to_string(), "/p/style.css".to_string(),
        ];
        let opts = CompleteOpts { extensions: vec!["png".into(), "svg".into()], ..Default::default() };
        let r = complete(&cands, "/p/cur.md", "", &opts);
        assert!(r.iter().all(|c| c.display.ends_with(".png")));
    }

    #[test]
    fn complete_relative_insert_with_dot_slash() {
        let cands = vec!["/p/sub/x.md".to_string()];
        let opts = CompleteOpts::default();
        let r = complete(&cands, "/p/cur.md", "x", &opts);
        assert_eq!(r[0].insert, "./sub/x.md");
        assert_eq!(r[0].display, "sub/x.md");
    }

    #[test]
    fn complete_strips_suffix_on_insert() {
        let cands = vec!["/p/note.md".to_string()];
        let opts = CompleteOpts { strip_suffixes: vec![".md".into()], ..Default::default() };
        let r = complete(&cands, "/p/cur.md", "note", &opts);
        assert_eq!(r[0].insert, "./note");
    }

    #[test]
    fn complete_directory_given_lists_that_dir() {
        let cands = vec![
            "/p/assets/a.png".to_string(),
            "/p/assets/b.png".to_string(),
            "/p/other/c.png".to_string(),
        ];
        let opts = CompleteOpts::default();
        // typing "assets/" should list only files in assets
        let r = complete(&cands, "/p/cur.md", "assets/", &opts);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|c| c.display.starts_with("assets/")));
    }

    #[test]
    fn empty_candidates_no_panic() {
        let _ = complete(&[], "/p/cur.md", "x", &CompleteOpts::default());
    }
}
