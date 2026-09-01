//! stats — what a repository looks like over its whole history.
//!
//! A 4,678-commit monorepo has a shape nobody can see from `git log`: which
//! languages actually carry the work, whether tests grew with the code or
//! lagged it, when a subsystem appeared, how much of the tree is prose. Those
//! are answerable from git alone, and answering them is cheap if the walk is
//! done once and cached.
//!
//! ## Why the parsing is separated from the git calls
//!
//! Everything below the `collect` fence is PURE: it takes the text git already
//! printed and turns it into numbers. That split is the reason this file has
//! tests at all — a stats page whose only test is "run it against this repo"
//! passes on one machine and proves nothing about the arithmetic. Every rule
//! that decides what counts as source, what counts as a test, and how a
//! sample is spaced is exercised on strings here.
//!
//! ## The blob cache is the whole performance story
//!
//! Counting lines per commit means reading file contents, and 16,000 files
//! across 60 sampled commits is 960,000 reads done naively. Almost all of them
//! are the SAME blob: a file that did not change between two samples has one
//! content hash. Caching line counts by blob hash turns the walk from minutes
//! into about a fifth of a second per commit after the first.
//!
//! ## What this deliberately does not do
//!
//! No churn-per-author, no blame, no "who wrote what". Not from squeamishness
//! — this repo's history is largely one operator working with assistants, so
//! per-author numbers would measure the tooling rather than the work, and a
//! chart that invites a wrong reading is worse than no chart.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// What a file extension counts as.
///
/// A short table on purpose. Every entry here is a language something in this
/// tree is actually written in; adding speculative ones would put empty series
/// on every chart.
pub fn language_of(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rfind('.').map(|i| &name[i..])?.to_ascii_lowercase();
    Some(match ext.as_str() {
        ".rs" => "Rust",
        ".c" | ".h" | ".cpp" | ".hpp" | ".cc" => "C/C++",
        ".py" => "Python",
        ".sh" | ".bash" => "Shell",
        ".wgsl" | ".glsl" | ".vert" | ".frag" => "Shader",
        ".qml" => "QML",
        ".js" | ".ts" | ".jsx" | ".tsx" => "JS/TS",
        ".md" => "Docs",
        _ => return None,
    })
}

/// Is this path something the repository AUTHORS, rather than something it
/// carries?
///
/// Vendored copies and build output would otherwise dominate every total and
/// move it for reasons nobody chose — a dependency bump reading as a week of
/// work. `target/` matters most: it is gitignored here, but this same walk is
/// run against arbitrary repositories.
pub fn is_authored(path: &str) -> bool {
    const CARRIED: [&str; 6] =
        ["vendor/", "third_party/", "node_modules/", "target/", ".git/", "dist/"];
    !CARRIED.iter().any(|c| path.starts_with(c) || path.contains(&format!("/{c}")))
}

/// One file's contribution.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub files: u32,
    pub lines: u64,
    /// `#[test]` attributes — Rust only, and the number is a floor rather than
    /// a census: it misses integration tests declared by file placement and
    /// counts a commented-out one. It tracks the right direction, which is
    /// what a trend line needs.
    pub tests: u32,
}

impl Counts {
    pub fn add(&mut self, o: Counts) {
        self.files += o.files;
        self.lines += o.lines;
        self.tests += o.tests;
    }
}

/// The repository at one commit.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub oid: String,
    /// Unix seconds of the commit date.
    pub when: i64,
    pub by_language: Vec<(String, Counts)>,
    pub total: Counts,
    /// Authored source only — excludes Docs, so the code/prose split is
    /// answerable rather than blended.
    pub code: Counts,
    pub docs: Counts,
}

impl Snapshot {
    /// Tests per thousand lines of code. The ratio rather than the count,
    /// because a growing codebase adds tests just by existing and the
    /// interesting question is whether it adds them FASTER than code.
    pub fn tests_per_kloc(&self) -> f32 {
        if self.code.lines == 0 {
            return 0.0;
        }
        self.code.tests as f32 / (self.code.lines as f32 / 1000.0)
    }
    /// Share of authored lines that are prose, 0..1.
    pub fn doc_share(&self) -> f32 {
        let t = self.code.lines + self.docs.lines;
        if t == 0 {
            return 0.0;
        }
        self.docs.lines as f32 / t as f32
    }
    pub fn lang(&self, name: &str) -> Counts {
        self.by_language
            .iter()
            .find(|(l, _)| l == name)
            .map(|(_, c)| *c)
            .unwrap_or_default()
    }
}

/// Pick `n` commits spread evenly through `total`, oldest first, always
/// including both ends.
///
/// Even spacing by INDEX rather than by date: this history is bursty — whole
/// days with nothing and days with two hundred commits — and date-spacing
/// would sample the quiet weeks as heavily as the busy ones, which flattens
/// exactly the shape worth seeing.
pub fn sample_indices(total: usize, n: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    if total <= n || n <= 1 {
        return (0..total).collect();
    }
    let mut out: Vec<usize> = (0..n)
        .map(|i| (i as f64 * (total - 1) as f64 / (n - 1) as f64).round() as usize)
        .collect();
    out.dedup();
    out
}

/// Parse one `git ls-tree -r <sha>` line into (blob oid, path).
///
/// Tolerant of what git actually emits — mode, type, oid, TAB, path — and
/// silently skips anything that is not a blob, because a tree contains
/// submodule commits too and counting one as a file would add a phantom.
pub fn parse_ls_tree_line(line: &str) -> Option<(&str, &str)> {
    let (meta, path) = line.split_once('\t')?;
    let mut it = meta.split_whitespace();
    let _mode = it.next()?;
    if it.next()? != "blob" {
        return None;
    }
    Some((it.next()?, path))
}

/// Line and `#[test]` counts for one blob's bytes.
///
/// Counts newlines rather than splitting: a 200 KB file split into a Vec to be
/// counted is 200 KB of allocation per blob, and the walk touches tens of
/// thousands. A file with no trailing newline still counts its last line.
pub fn count_blob(body: &[u8], rust: bool) -> Counts {
    let mut lines = body.iter().filter(|b| **b == b'\n').count() as u64;
    if !body.is_empty() && !body.ends_with(b"\n") {
        lines += 1;
    }
    let tests = if rust { count_occurrences(body, b"#[test]") as u32 } else { 0 };
    Counts { files: 1, lines, tests }
}

fn count_occurrences(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || hay.len() < needle.len() {
        return 0;
    }
    let mut n = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            n += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    n
}

/// Fold per-file counts into a snapshot.
pub fn assemble(oid: String, when: i64, files: &[(&str, Counts)]) -> Snapshot {
    let mut by: HashMap<&'static str, Counts> = HashMap::new();
    let mut code = Counts::default();
    let mut docs = Counts::default();
    for (path, c) in files {
        if !is_authored(path) {
            continue;
        }
        let Some(lang) = language_of(path) else { continue };
        by.entry(lang).or_default().add(*c);
        if lang == "Docs" {
            docs.add(*c);
        } else {
            code.add(*c);
        }
    }
    let mut by_language: Vec<(String, Counts)> =
        by.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    // biggest first, then by name so equal sizes do not reorder between runs
    by_language.sort_by(|a, b| b.1.lines.cmp(&a.1.lines).then(a.0.cmp(&b.0)));
    let mut total = code;
    total.add(docs);
    Snapshot { oid, when, by_language, total, code, docs }
}

// ------------------------------------------------------------ git fence ----
// Everything above is pure. Everything below shells out, and is the part that
// cannot be unit-tested without a repository.

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Walk `n` evenly-spaced commits and measure each.
///
/// `cache` is the blob line-count memo and is the reason this is fast enough
/// to run from a GUI. Pass the same map across calls to keep it warm.
pub fn collect(
    dir: &Path,
    n: usize,
    cache: &mut HashMap<String, Counts>,
) -> Result<Vec<Snapshot>, String> {
    let list = git(dir, &["rev-list", "--reverse", "HEAD"])
        .ok_or_else(|| "not a git repository, or no commits".to_string())?;
    let all: Vec<&str> = list.lines().collect();
    if all.is_empty() {
        return Err("no commits".into());
    }
    let mut out = Vec::new();
    for i in sample_indices(all.len(), n) {
        let sha = all[i];
        let when = git(dir, &["show", "-s", "--format=%ct", sha])
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let tree = git(dir, &["ls-tree", "-r", sha]).unwrap_or_default();

        let mut paths: Vec<(&str, &str)> = Vec::new();
        for line in tree.lines() {
            if let Some((blob, path)) = parse_ls_tree_line(line) {
                if is_authored(path) && language_of(path).is_some() {
                    paths.push((blob, path));
                }
            }
        }
        // (blob, is_rust) — the Rust flag has to travel WITH the blob, because
        // `#[test]` only counts in Rust and the batch reader cannot tell from
        // the oid alone. The first version passed `false` for everything, so
        // every test count in a real walk would have been zero while the unit
        // tests stayed green: they exercise `count_blob` directly and never
        // reach this.
        let missing: Vec<(&str, bool)> = paths
            .iter()
            .filter(|(b, _)| !cache.contains_key(*b))
            .map(|(b, p)| (*b, p.ends_with(".rs")))
            .collect();
        if !missing.is_empty() {
            measure_blobs(dir, &missing, cache);
        }
        let files: Vec<(&str, Counts)> = paths
            .iter()
            .map(|(b, p)| (*p, cache.get(*b).copied().unwrap_or_default()))
            .collect();
        out.push(assemble(sha.to_string(), when, &files));
    }
    Ok(out)
}

/// Read a batch of blobs in one `git cat-file --batch` and memo their counts.
fn measure_blobs(dir: &Path, blobs: &[(&str, bool)], cache: &mut HashMap<String, Counts>) {
    use std::io::Write;
    let mut child = match Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["cat-file", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    if let Some(mut si) = child.stdin.take() {
        let req: String = blobs.iter().map(|(b, _)| format!("{b}\n")).collect();
        let _ = si.write_all(req.as_bytes());
    }
    let Ok(out) = child.wait_with_output() else { return };
    let buf = out.stdout;
    let mut i = 0usize;
    for (b, is_rust) in blobs {
        let Some(nl) = buf[i..].iter().position(|c| *c == b'\n') else { break };
        let header = String::from_utf8_lossy(&buf[i..i + nl]).into_owned();
        let size: usize = match header.split_whitespace().nth(2).and_then(|s| s.parse().ok()) {
            Some(s) => s,
            None => break, // "missing" — leave it uncached rather than guess
        };
        let start = i + nl + 1;
        let end = (start + size).min(buf.len());
        cache.insert(b.to_string(), count_blob(&buf[start..end], *is_rust));
        i = end + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_languages_count_and_docs_are_separate() {
        assert_eq!(language_of("src/main.rs"), Some("Rust"));
        assert_eq!(language_of("engine_src/game.c"), Some("C/C++"));
        assert_eq!(language_of("docs/PLAN.md"), Some("Docs"));
        assert_eq!(language_of("src/shader.wgsl"), Some("Shader"));
        // no extension, and an extension we do not track
        assert_eq!(language_of("Makefile"), None);
        assert_eq!(language_of("assets/logo.png"), None);
        // a dot in a directory must not be read as the file's extension
        assert_eq!(language_of("a.b/Cargo.lock"), None);
        assert_eq!(language_of(".gitignore"), None);
    }

    #[test]
    fn carried_code_is_not_counted_as_authored() {
        // The failure this prevents: a dependency bump reading as a week of
        // work because 40,000 vendored lines arrived in one commit.
        assert!(is_authored("games/blackstar-rs/src/main.rs"));
        assert!(!is_authored("vendor/snapcore/src/lib.rs"));
        assert!(!is_authored("games/blackstar-rs/vendor/netcode/src/lobby.rs"));
        assert!(!is_authored("node_modules/x/index.js"));
        assert!(!is_authored("target/debug/build.rs"));
        // a path that merely CONTAINS the word is still ours
        assert!(is_authored("src/vendoring_notes.rs"));
    }

    #[test]
    fn a_tree_line_yields_its_blob_and_skips_submodules() {
        let l = "100644 blob a1b2c3d4\tsrc/main.rs";
        assert_eq!(parse_ls_tree_line(l), Some(("a1b2c3d4", "src/main.rs")));
        // a submodule is a commit inside a tree; counting it would add a
        // phantom file with no content
        assert_eq!(parse_ls_tree_line("160000 commit deadbeef\tvendor/thing"), None);
        assert_eq!(parse_ls_tree_line("040000 tree abc\tsrc"), None);
        assert_eq!(parse_ls_tree_line("garbage"), None);
        // paths with spaces survive, because the split is on TAB
        assert_eq!(
            parse_ls_tree_line("100644 blob ff\tdocs/a file.md"),
            Some(("ff", "docs/a file.md"))
        );
    }

    #[test]
    fn a_file_without_a_trailing_newline_still_has_a_last_line() {
        assert_eq!(count_blob(b"a\nb\nc\n", false).lines, 3);
        assert_eq!(count_blob(b"a\nb\nc", false).lines, 3, "the last line was dropped");
        assert_eq!(count_blob(b"", false).lines, 0);
        assert_eq!(count_blob(b"\n", false).lines, 1);
    }

    #[test]
    fn tests_are_counted_only_where_they_mean_something() {
        let src = b"#[test]\nfn a() {}\n#[test]\nfn b() {}\n";
        assert_eq!(count_blob(src, true).tests, 2);
        // the same bytes in a non-Rust file are not Rust tests
        assert_eq!(count_blob(src, false).tests, 0);
        // overlapping matches must not double-count
        assert_eq!(count_blob(b"#[test]#[test]", true).tests, 2);
    }

    #[test]
    fn the_sample_covers_both_ends_and_does_not_repeat() {
        let s = sample_indices(4678, 60);
        assert_eq!(s.first(), Some(&0), "the oldest commit is missing");
        assert_eq!(s.last(), Some(&4677), "HEAD is missing");
        assert!(s.len() <= 60);
        assert!(s.windows(2).all(|w| w[0] < w[1]), "the sample is not ordered or repeats");
        // a history smaller than the sample is taken whole
        assert_eq!(sample_indices(5, 60), vec![0, 1, 2, 3, 4]);
        assert!(sample_indices(0, 60).is_empty());
        // and the spacing is even, not front- or back-loaded
        let gaps: Vec<usize> = s.windows(2).map(|w| w[1] - w[0]).collect();
        let (lo, hi) = (*gaps.iter().min().unwrap(), *gaps.iter().max().unwrap());
        assert!(hi - lo <= 1, "uneven sample spacing: {lo}..{hi}");
    }

    #[test]
    fn a_snapshot_splits_code_from_prose_and_ranks_by_size() {
        let files = [
            ("src/a.rs", Counts { files: 1, lines: 300, tests: 4 }),
            ("src/b.rs", Counts { files: 1, lines: 200, tests: 2 }),
            ("engine/c.c", Counts { files: 1, lines: 100, tests: 0 }),
            ("docs/d.md", Counts { files: 1, lines: 400, tests: 0 }),
            ("vendor/e.rs", Counts { files: 1, lines: 9999, tests: 99 }),
        ];
        let s = assemble("abc".into(), 100, &files);
        assert_eq!(s.code.lines, 600, "vendored lines leaked into the code total");
        assert_eq!(s.docs.lines, 400);
        assert_eq!(s.total.lines, 1000);
        assert_eq!(s.code.tests, 6, "vendored tests were counted as ours");
        assert_eq!(s.lang("Rust").lines, 500);
        assert_eq!(s.lang("C/C++").lines, 100);
        // largest first
        assert_eq!(s.by_language[0].0, "Rust");
        // ratios
        assert!((s.tests_per_kloc() - 10.0).abs() < 1e-3, "{}", s.tests_per_kloc());
        assert!((s.doc_share() - 0.4).abs() < 1e-3);
    }

    #[test]
    fn an_empty_snapshot_does_not_divide_by_zero() {
        let s = assemble("x".into(), 0, &[]);
        assert_eq!(s.tests_per_kloc(), 0.0);
        assert_eq!(s.doc_share(), 0.0);
        assert_eq!(s.lang("Rust"), Counts::default());
    }
}
