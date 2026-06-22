// gitmind-core/src/review.rs
//
// Review sessions — the thing a "no staging area" workflow actually wants
// in place of manual staging: instead of curating an index by hand, you
// work freely, then mark a REVIEW. Marking summarizes everything done
// since the last review (files, authors, languages, line counts), tags
// the point (refs/reviews/<name>), and can fire hook scripts — so a
// review can kick off CI-like tasks, notifications, or auto-formatting,
// the same way git hooks do, but triggered by review boundaries.
//
// State:
//   refs/reviews/<name>  a named review point at the HEAD when marked
//   refs/reviews/_last   the most recent review point (the "since" anchor)
//
// The user's index and HEAD are never touched; reviews are pure refs.

use crate::backend::{GitCli, RepoBackend};
use std::path::Path;

// git's well-known empty-tree object: the "before anything" baseline used
// when there is no prior review to diff against.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReviewSummary {
    pub since: Option<String>,
    pub head: String,
    pub commits: u32,
    pub files_changed: Vec<String>,
    pub authors: Vec<String>,
    /// (language, files-touched) sorted most-touched first.
    pub languages: Vec<(String, u32)>,
    pub insertions: u64,
    pub deletions: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HookResult {
    pub name: String,
    pub ok: bool,
}

pub struct ReviewEngine<'a> {
    repo: &'a GitCli,
}

impl<'a> ReviewEngine<'a> {
    pub fn new(repo: &'a GitCli) -> Self {
        ReviewEngine { repo }
    }

    /// The most recent review point, if any.
    pub fn last_review(&self) -> Option<String> {
        self.repo
            .git(&["rev-parse", "--verify", "--quiet", "refs/reviews/_last"])
            .ok()
            .filter(|s| !s.is_empty())
    }

    /// Summarize work between `since` (default: last review, else the
    /// empty tree) and HEAD.
    pub fn summarize_since(&self, since: Option<&str>) -> Result<ReviewSummary, String> {
        let head = self.repo.git(&["rev-parse", "HEAD"])?;
        let owned_last;
        let base: &str = match since {
            Some(s) => s,
            None => match self.last_review() {
                Some(l) => {
                    owned_last = l;
                    &owned_last
                }
                None => EMPTY_TREE,
            },
        };

        let mut s = ReviewSummary {
            since: if base == EMPTY_TREE { None } else { Some(base.chars().take(12).collect()) },
            head: head.chars().take(12).collect(),
            ..Default::default()
        };

        // Net file changes (numstat: insertions \t deletions \t path).
        let numstat = self.repo.git(&["diff", "--numstat", base, "HEAD"])?;
        let mut lang_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for line in numstat.lines() {
            let mut p = line.split('\t');
            let ins = p.next().unwrap_or("0");
            let del = p.next().unwrap_or("0");
            let path = p.next().unwrap_or("");
            if path.is_empty() {
                continue;
            }
            s.insertions += ins.parse().unwrap_or(0);
            s.deletions += del.parse().unwrap_or(0);
            s.files_changed.push(path.to_string());
            let ext = path.rsplit('.').next().unwrap_or("");
            if ext != path && !ext.is_empty() {
                *lang_counts.entry(ext.to_string()).or_insert(0) += 1;
            }
        }
        let mut langs: Vec<(String, u32)> = lang_counts.into_iter().collect();
        langs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        s.languages = langs;

        // Commit count + distinct authors in the range (skip if no prior
        // commits, e.g. base == EMPTY_TREE on a fresh repo handled by git).
        let range = if base == EMPTY_TREE { "HEAD".to_string() } else { format!("{base}..HEAD") };
        if let Ok(c) = self.repo.git(&["rev-list", "--count", &range]) {
            s.commits = c.parse().unwrap_or(0);
        }
        if let Ok(a) = self.repo.git(&["log", "--format=%an", &range]) {
            let mut seen = std::collections::BTreeSet::new();
            for name in a.lines() {
                if !name.is_empty() {
                    seen.insert(name.to_string());
                }
            }
            s.authors = seen.into_iter().collect();
        }
        Ok(s)
    }

    /// Mark a review at HEAD: summarize since the previous review, advance
    /// the `_last` anchor, tag the named ref, and optionally run hooks.
    pub fn mark(
        &self,
        name: &str,
        hooks_dir: Option<&Path>,
    ) -> Result<(ReviewSummary, Vec<HookResult>), String> {
        let summary = self.summarize_since(None)?;
        let head = self.repo.git(&["rev-parse", "HEAD"])?;
        self.repo.git(&["update-ref", &format!("refs/reviews/{name}"), &head])?;
        self.repo.git(&["update-ref", "refs/reviews/_last", &head])?;
        let hooks = match hooks_dir {
            Some(d) => self.run_hooks(d, "review", &summary),
            None => Vec::new(),
        };
        Ok((summary, hooks))
    }

    /// Run every executable file in `dir` as a hook, passing the summary
    /// through environment variables (the git-hook-style escape hatch for
    /// CI-like tasks). Returns each hook's success.
    pub fn run_hooks(&self, dir: &Path, event: &str, s: &ReviewSummary) -> Vec<HookResult> {
        use std::os::unix::fs::PermissionsExt;
        let mut results = Vec::new();
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return results,
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let meta = match std::fs::metadata(&p) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
                continue;
            }
            let langs = s
                .languages
                .iter()
                .map(|(l, n)| format!("{l}:{n}"))
                .collect::<Vec<_>>()
                .join(",");
            let status = std::process::Command::new(&p)
                .current_dir(self.repo.root())
                .env("GITMIND_EVENT", event)
                .env("GITMIND_COMMITS", s.commits.to_string())
                .env("GITMIND_FILES", s.files_changed.len().to_string())
                .env("GITMIND_INSERTIONS", s.insertions.to_string())
                .env("GITMIND_DELETIONS", s.deletions.to_string())
                .env("GITMIND_LANGUAGES", langs)
                .status();
            results.push(HookResult {
                name: e.file_name().to_string_lossy().to_string(),
                ok: status.map(|st| st.success()).unwrap_or(false),
            });
        }
        results
    }
}
