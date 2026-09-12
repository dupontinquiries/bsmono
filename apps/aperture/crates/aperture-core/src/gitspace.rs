//! gitspace — the file's history, as data the corridor can wear.
//!
//! Integration with gitmind happens at the PROTOCOL level, not the crate
//! level: gitmind-core's `GitCli` philosophy (shell out to `git`; full
//! ecosystem compat, ~5–20 ms/query, no libgit2 build burden) is adopted
//! wholesale, and — the part that makes the two apps one system — this
//! module reads gitmind's checkpoint namespace `refs/checkpoints/*`, the
//! shadow-commit timeline its autocommit engine writes without ever
//! touching HEAD. gitmind writes the timeline; aperture-view walks it in
//! space. (Path-depending on gitmind-core was rejected: its chunk store —
//! redb, zstd, fastcdc, notify — is not feature-gated, a heavy payload for
//! a viewer that needs four subcommands. If that gating ever lands, the
//! `Runner` trait below is exactly the seam where `GitCli` slots in.)
//!
//! Everything that PARSES is pure and fixture-tested; everything that RUNS
//! goes through `Runner`, so tests inject canned output and the one
//! hermetic integration test builds a real throwaway repo.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The command seam. `GitRunner` shells out; tests hand back strings.
pub trait Runner {
    fn run(&self, args: &[&str]) -> Result<String, String>;

    /// The same, with something on stdin. Exactly one caller needs it —
    /// `git hash-object -w --stdin`, which is how a bucket's text becomes
    /// an object without a temp file on disk.
    ///
    /// Defaulted to a refusal rather than added as a required method:
    /// every canned test runner in this crate answers by matching argv,
    /// which is a shape stdin has nothing to do with, and making them all
    /// implement a method they will never be asked for would be noise in
    /// exchange for nothing. A runner that cannot do it says so and the
    /// caller reports it; it is not a silent no-op.
    fn run_stdin(&self, args: &[&str], input: &str) -> Result<String, String> {
        let _ = (args, input);
        Err("this runner cannot pass stdin".to_string())
    }
}

pub struct GitRunner {
    pub root: PathBuf,
}

impl Runner for GitRunner {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn run_stdin(&self, args: &[&str], input: &str) -> Result<String, String> {
        use std::io::Write;
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        // Take the pipe and drop it before waiting. A child reading to EOF
        // (which hash-object does) never exits while we still hold the
        // write end, and `wait_with_output` would then block forever.
        {
            let mut si = child.stdin.take().ok_or("no stdin pipe")?;
            si.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Walk up from `file` to the repo root (a dir containing `.git`), and give
/// back the runner plus the file's repo-relative path.
pub fn discover(file: &Path) -> Option<(GitRunner, String)> {
    let abs = file.canonicalize().ok()?;
    let mut dir = abs.parent()?;
    loop {
        if dir.join(".git").exists() {
            let rel = abs.strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/");
            return Some((GitRunner { root: dir.to_path_buf() }, rel));
        }
        dir = dir.parent()?;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevKind {
    Commit,
    /// A gitmind checkpoint (shadow autocommit); the session label rides along.
    Checkpoint(String),
}

#[derive(Clone, Debug)]
pub struct Rev {
    pub oid: String,
    pub time: i64,
    pub subject: String,
    pub kind: RevKind,
    /// Parent commit oids — empty for a root commit, 2+ for a merge.
    /// Checkpoints (from `refs/checkpoints/*`, no ancestry query run for
    /// them) carry an empty list too: they're a NAME pointing at a commit,
    /// not a distinct node in the ancestry graph — the commit they point at
    /// is the real graph node, already present via the ordinary log walk.
    pub parents: Vec<String>,
}

/// Parse `git log --format=%H%x1f%ct%x1f%P%x1f%s` output (unit separator
/// fields; `%P` is parent oids space-separated, empty for a root commit —
/// note %P sits BEFORE %s: a subject can itself contain spaces, so it must
/// be the last field or splitting would misparse it).
pub fn parse_log(s: &str) -> Vec<Rev> {
    s.lines()
        .filter_map(|l| {
            let mut f = l.split('\u{1f}');
            let oid = f.next()?.to_string();
            let time = f.next()?.parse().ok()?;
            let parents = f.next().unwrap_or("").split_whitespace().map(str::to_string).collect();
            let subject = f.next().unwrap_or("").to_string();
            Some(Rev { oid, time, subject, kind: RevKind::Commit, parents })
        })
        .collect()
}

/// Parse `git for-each-ref refs/checkpoints
/// --format=%(objectname)%x1f%(creatordate:unix)%x1f%(refname)` — gitmind's
/// namespace is `refs/checkpoints/<session>/…` or `refs/checkpoints/<session>`.
pub fn parse_checkpoints(s: &str) -> Vec<Rev> {
    s.lines()
        .filter_map(|l| {
            let mut f = l.split('\u{1f}');
            let oid = f.next()?.to_string();
            let time = f.next()?.parse().ok()?;
            let refname = f.next().unwrap_or("");
            let session = refname.strip_prefix("refs/checkpoints/").unwrap_or(refname).to_string();
            Some(Rev { oid, time, subject: format!("checkpoint ◈ {session}"), kind: RevKind::Checkpoint(session), parents: Vec::new() })
        })
        .collect()
}

/// The file's timeline: commits touching `rel` merged with gitmind
/// checkpoints, newest first. Checkpoints snapshot the whole tree, so they
/// appear regardless of which file changed — they're moments, not patches.
pub fn history(r: &dyn Runner, rel: &str, limit: usize) -> Vec<Rev> {
    let mut revs = r
        .run(&["log", &format!("--max-count={limit}"), "--format=%H\u{1f}%ct\u{1f}%P\u{1f}%s", "--", rel])
        .map(|s| parse_log(&s))
        .unwrap_or_default();
    if let Ok(s) = r.run(&[
        "for-each-ref",
        "refs/checkpoints",
        "--format=%(objectname)\u{1f}%(creatordate:unix)\u{1f}%(refname)",
    ]) {
        revs.extend(parse_checkpoints(&s));
    }
    // STABLE sort on time only: commits arrive in log order (newest first,
    // the authoritative parent order), and same-second commits — rapid
    // autocommits, test fixtures — must keep it. An oid tiebreak here once
    // scrambled a two-commits-in-one-second history.
    revs.sort_by(|a, b| b.time.cmp(&a.time));
    revs.truncate(limit);
    revs
}

/// The file's content at a revision (`git show oid:rel`). None when the file
/// didn't exist there (checkpoints predating it, renames…).
pub fn file_at(r: &dyn Runner, oid: &str, rel: &str) -> Option<String> {
    r.run(&["show", &format!("{oid}:{rel}")]).ok()
}

/// Parse `-U0` hunk headers: `@@ -a,b +c,d @@` → new-side changed line
/// ranges (0-based, half-open). Pure additions/edits; deletions have d=0
/// and mark the single line the removal sits above (a tombstone to tint).
pub fn parse_hunks(s: &str) -> Vec<(usize, usize)> {
    s.lines()
        .filter(|l| l.starts_with("@@"))
        .filter_map(|l| {
            let plus = l.split('+').nth(1)?;
            let seg = plus.split(' ').next()?;
            let mut nums = seg.split(',');
            let start: usize = nums.next()?.parse().ok()?;
            let len: usize = nums.next().map(|n| n.parse().unwrap_or(1)).unwrap_or(1);
            let s0 = start.saturating_sub(1); // git is 1-based
            Some(if len == 0 { (s0, s0 + 1) } else { (s0, s0 + len) })
        })
        .collect()
}

/// Lines this revision changed relative to its parent (`oid^`), on the
/// revision's own line numbering — what to tint when SHOWING that revision.
pub fn changed_ranges(r: &dyn Runner, oid: &str, rel: &str) -> Vec<(usize, usize)> {
    r.run(&["diff", "-U0", &format!("{oid}^"), oid, "--", rel])
        .map(|s| parse_hunks(&s))
        .unwrap_or_default()
}

/// Worktree lines changed since a base revision — what to tint on the live
/// file.
pub fn worktree_changed(r: &dyn Runner, base: &str, rel: &str) -> Vec<(usize, usize)> {
    r.run(&["diff", "-U0", base, "--", rel]).map(|s| parse_hunks(&s)).unwrap_or_default()
}

/// Parse `git blame --line-porcelain` into per-line committer times. The
/// porcelain repeats a header block per line; `committer-time <unix>` is the
/// field we keep.
pub fn parse_blame_times(s: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut cur: Option<i64> = None;
    for l in s.lines() {
        if let Some(t) = l.strip_prefix("committer-time ") {
            cur = t.trim().parse().ok();
        } else if l.starts_with('\t') {
            // The content line closes each per-line block.
            out.push(cur.unwrap_or(0));
        }
    }
    out
}

/// Per-line last-touched times for the worktree file.
pub fn line_ages(r: &dyn Runner, rel: &str) -> Vec<i64> {
    r.run(&["blame", "--line-porcelain", "--", rel]).map(|s| parse_blame_times(&s)).unwrap_or_default()
}

/// Age → heat in [0, 1]: touched now = 1, half-life ~a week, ancient → 0.
/// Log-scaled so "today vs yesterday" reads hotter than "month vs quarter" —
/// recency is what wayfinding wants.
pub fn heat(now: i64, touched: i64) -> f32 {
    if touched <= 0 {
        return 0.0;
    }
    let days = ((now - touched).max(0) as f32) / 86_400.0;
    (1.0 / (1.0 + days / 7.0)).clamp(0.0, 1.0)
}

/// Promote any revision — a commit OR a gitmind checkpoint — to a real
/// branch, one `git branch`. This is gitmind's own design (ARCHITECTURE.md:
/// "aggressive branching with checkpoints — promoting any checkpoint to a
/// real branch is one `branch <n> <ckpt-oid>`"); this function is that
/// operation from aperture's side of the fence, reachable from whichever
/// revision the person is currently LOOKING AT — the corridor or the flat
/// preview both know an oid, and turning "I want to keep exploring from
/// here" into a real branch shouldn't require leaving either shell.
///
/// Refuses to overwrite an existing branch (git already refuses this; the
/// message is passed through rather than re-worded, since it's already
/// exactly right: "fatal: A branch named '<name>' already exists.").
pub fn branch_from(r: &dyn Runner, oid: &str, name: &str) -> Result<(), String> {
    r.run(&["branch", name, oid]).map(|_| ())
}

/// A branch name that reads like what it is: `from/<subject-slug>-<oid8>`.
/// Deterministic (same revision → same suggested name, so re-running after
/// an earlier attempt failed on a collision shows you the SAME name, not a
/// fresh one every time) and short enough to type over if you want to.
pub fn suggest_branch_name(rev: &Rev) -> String {
    let slug: String = rev
        .subject
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-");
    let short = &rev.oid[..rev.oid.len().min(8)];
    if slug.is_empty() {
        format!("from/{short}")
    } else {
        format!("from/{slug}-{short}")
    }
}

/// Check a revision out into a FRESH worktree next to the repo, detached —
/// `git worktree add --detach`, gitmind's own operation
/// (checkpoint.rs `materialize`). Where `branch_from` says "I want to keep
/// developing from here," `materialize` says "I want to stand in this
/// version, in a real directory, without disturbing the one I'm in" — open
/// it in another editor window, run its tests, diff two whole trees on
/// disk. `into` is repo-root-relative or absolute, same as git accepts.
pub fn materialize(r: &dyn Runner, oid: &str, into: &str) -> Result<(), String> {
    r.run(&["worktree", "add", "--detach", into, oid]).map(|_| ())
}

/// A destination next to the repo root, not inside it — `../<repo-name>@<slug>-<oid8>`
/// — so the new worktree doesn't need a gitignore entry and doesn't nest
/// inside the tree it's a snapshot OF. Deterministic, same reasoning as
/// `suggest_branch_name`.
pub fn suggest_worktree_path(repo_root: &Path, rev: &Rev) -> PathBuf {
    let name = repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let slug = suggest_branch_name(rev);
    let slug = slug.strip_prefix("from/").unwrap_or(&slug);
    repo_root
        .parent()
        .unwrap_or(repo_root)
        .join(format!("{name}@{slug}"))
}

/// The current HEAD, if there is one (a fresh repo before the first commit
/// has none — callers treat that as "nowhere to mark yet," not an error).
pub fn head_oid(r: &dyn Runner) -> Option<String> {
    r.run(&["rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string())
}

/// Aperture's own personal-bookmark namespace, `refs/aperture/visited/*` —
/// deliberately parallel to gitmind's `refs/checkpoints/*`: a ref is the
/// right primitive for "a name pointing at a commit that isn't a branch,"
/// and reusing the mechanism means no new file format, nothing to gitignore,
/// nothing that needs its own backup story. One ref per file, git-native.
///
/// This is PERSONAL wayfinding, not project history: it says "where YOU
/// were," not "what happened." A ref name can contain `/` as path
/// separators (that's how `refs/heads/feature/x` already works), so a
/// repo-relative path becomes the ref's tail directly — sanitized only for
/// the rare character git itself would reject in a ref component.
fn visited_ref(rel: &str) -> String {
    let safe: String = rel
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '/' | '_' | '.' | '-') { c } else { '_' })
        .collect();
    format!("refs/aperture/visited/{safe}")
}

/// Mark "I was here" — write `refs/aperture/visited/<file>` at `oid`
/// (typically the current HEAD). Overwrites any previous mark for this
/// file; there is only ever one "last visited" point, not a history of
/// them — that's what commit history is already for.
pub fn mark_visited(r: &dyn Runner, rel: &str, oid: &str) -> Result<(), String> {
    r.run(&["update-ref", &visited_ref(rel), oid]).map(|_| ())
}

/// Where you left off, if you ever marked it.
pub fn last_visited(r: &dyn Runner, rel: &str) -> Option<String> {
    r.run(&["rev-parse", "--verify", &visited_ref(rel)]).ok().map(|s| s.trim().to_string())
}

/// `refs/aperture/bucket/<slug>` — a bucket's text, stored as a git blob
/// with a ref holding it alive.
///
/// The same reasoning as `visited` above, one step further: `visited`
/// needs a ref pointing at a COMMIT that already exists, a bucket needs a
/// ref pointing at content of its own. A ref may point at any object, so
/// the content becomes a blob (`hash-object -w`) and the ref names it.
/// No new file format, no gitignore entry, nothing to back up separately,
/// and `git gc` keeps it because a ref is a reachability root.
///
/// **Measured, not assumed** (a scratch repo, git 2.x):
/// * `git gc --prune=now` keeps the blob. `git fsck` is silent about it.
/// * `git rev-list --all` skips the ref rather than failing on a
///   non-commit, so nothing that walks history trips over it.
/// * `git bundle create --all` DOES include it —
///   `git bundle list-heads` lists the ref — but a plain `git clone` of
///   that bundle does not create it locally, because clone only takes
///   heads and tags. It is fetched the same way notes are:
///   `git fetch <remote> 'refs/aperture/*:refs/aperture/*'`. That is
///   exactly the rule `CLAUDE.md` §1 already states for
///   `refs/notes/toolcalls`, and it is worth stating again here because
///   the failure mode is identical: buckets look silently absent in a
///   fresh clone rather than looking un-fetched.
fn bucket_ref(slug: &str) -> String {
    format!("refs/aperture/bucket/{slug}")
}

/// Buckets kept as git refs. See [`bucket_ref`].
///
/// Holds a `&dyn Runner` rather than a `GitRunner` so the hermetic test
/// and any future `GitCli` from gitmind-core both fit — the same seam
/// reasoning as the rest of this module.
pub struct GitStore<'a> {
    pub runner: &'a dyn Runner,
}

impl<'a> GitStore<'a> {
    pub fn new(runner: &'a dyn Runner) -> GitStore<'a> {
        GitStore { runner }
    }
}

impl crate::bucket::Store for GitStore<'_> {
    fn list(&self) -> Vec<String> {
        let out = match self.runner.run(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/aperture/bucket/",
        ]) {
            Ok(s) => s,
            // No refs, or no repo. Both mean "no buckets", which is a
            // normal state and not worth surfacing as an error.
            Err(_) => return Vec::new(),
        };
        out.lines()
            .filter_map(|l| l.trim().strip_prefix("refs/aperture/bucket/"))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn load(&self, slug: &str) -> Option<String> {
        self.runner.run(&["cat-file", "blob", &bucket_ref(slug)]).ok()
    }

    fn save(&self, slug: &str, text: &str) -> Result<(), String> {
        let oid = self.runner.run_stdin(&["hash-object", "-w", "--stdin"], text)?;
        let oid = oid.trim();
        if oid.is_empty() {
            return Err("hash-object wrote nothing".to_string());
        }
        self.runner.run(&["update-ref", &bucket_ref(slug), oid]).map(|_| ())
    }

    fn delete(&self, slug: &str) -> Result<(), String> {
        // `-d` on a ref that is already gone is an error from git and a
        // success from the caller's point of view. The blob itself is
        // left for gc, which is what gc is for.
        match self.runner.run(&["update-ref", "-d", &bucket_ref(slug)]) {
            Ok(_) => Ok(()),
            Err(_) if self.load(slug).is_none() => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Group time-sorted (newest-first, `history()`'s own order) revisions into
/// consecutive same-UTC-day runs — a session's worth of checkpoints and
/// commits collapses into one batch instead of forty individual stops. No
/// timezone handling (day = `unix_seconds.div_euclid(86400)`): deterministic
/// and good enough for "roughly a day's work," which is the actual question
/// wind-forward batching is answering, not a calendar-accurate boundary.
pub fn batch_by_day(revs: &[Rev]) -> Vec<Vec<Rev>> {
    let mut out: Vec<Vec<Rev>> = Vec::new();
    for r in revs {
        let day = r.time.div_euclid(86_400);
        match out.last_mut() {
            Some(batch) if batch.last().map(|b: &Rev| b.time.div_euclid(86_400)) == Some(day) => {
                batch.push(r.clone());
            }
            _ => out.push(vec![r.clone()]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Canned(Vec<(Vec<String>, String)>);
    impl Runner for Canned {
        fn run(&self, args: &[&str]) -> Result<String, String> {
            for (pat, out) in &self.0 {
                if pat.iter().zip(args).all(|(p, a)| p == a) && pat.len() <= args.len() {
                    return Ok(out.clone());
                }
            }
            Err("no canned output".into())
        }
    }

    #[test]
    fn parses_log_checkpoints_and_merges_history() {
        let log = "aaa\u{1f}200\u{1f}fix: the thing\nbbb\u{1f}100\u{1f}feat: begin\n";
        let ckpt = "ccc\u{1f}150\u{1f}refs/checkpoints/sess-1\n";
        let r = Canned(vec![
            (vec!["log".into()], log.into()),
            (vec!["for-each-ref".into()], ckpt.into()),
        ]);
        let h = history(&r, "src/lib.rs", 10);
        let oids: Vec<&str> = h.iter().map(|r| r.oid.as_str()).collect();
        assert_eq!(oids, ["aaa", "ccc", "bbb"], "merged, newest first");
        assert_eq!(h[1].kind, RevKind::Checkpoint("sess-1".into()));
        assert!(h[1].subject.contains("◈"), "checkpoints wear their mark");
    }

    #[test]
    fn parses_hunk_headers_including_deletion_tombstones() {
        let d = "@@ -1,3 +1,4 @@\nctx\n@@ -10,2 +12 @@\n@@ -20,5 +19,0 @@\n";
        assert_eq!(parse_hunks(d), vec![(0, 4), (11, 12), (18, 19)]);
    }

    #[test]
    fn parses_blame_times_per_content_line() {
        let b = "aaa 1 1 2\nauthor A\ncommitter-time 100\n\tfn one() {}\naaa 2 2\n\tfn two() {}\nbbb 3 3 1\ncommitter-time 300\n\tfn three() {}\n";
        assert_eq!(parse_blame_times(b), vec![100, 100, 300], "header carries over within a group");
    }

    #[test]
    fn heat_is_hot_now_and_cold_later() {
        let now = 1_000_000;
        assert!(heat(now, now) > 0.99);
        let week = heat(now, now - 7 * 86_400);
        assert!((week - 0.5).abs() < 0.02, "half-life at a week: {week}");
        assert!(heat(now, now - 365 * 86_400) < 0.02);
        assert_eq!(heat(now, 0), 0.0, "unblamed lines stay cold");
    }

    /// The hermetic one: a real throwaway repo, two commits and a synthetic
    /// gitmind checkpoint ref, exercised end to end through the real runner.
    #[test]
    fn end_to_end_against_a_real_repo() {
        let dir = std::env::temp_dir().join(format!("aperture-gitspace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.rs"), "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "begin"]);
        std::fs::write(dir.join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "grow"]);
        // A gitmind-style checkpoint: any commit oid under refs/checkpoints.
        let head = git(&["rev-parse", "HEAD"]).trim().to_string();
        git(&["update-ref", "refs/checkpoints/sess-test", &head]);

        let (r, rel) = discover(&dir.join("a.rs")).expect("repo discovered");
        assert_eq!(rel, "a.rs");
        let h = history(&r, &rel, 10);
        assert!(h.len() >= 3, "2 commits + 1 checkpoint: {}", h.len());
        assert!(h.iter().any(|v| matches!(v.kind, RevKind::Checkpoint(_))));
        // Same-second commits are likely here; identify by subject, and
        // check that log order survived the merge (begin after grow).
        let begin = h.iter().find(|v| v.subject == "begin").unwrap();
        let grow = h.iter().find(|v| v.subject == "grow").unwrap();
        let pos = |oid: &str| h.iter().position(|v| v.oid == oid).unwrap();
        assert!(pos(&grow.oid) < pos(&begin.oid), "stable merge keeps parent order");
        assert_eq!(file_at(&r, &begin.oid, &rel).unwrap(), "fn one() {}\n");
        assert_eq!(changed_ranges(&r, &grow.oid, &rel), vec![(1, 2)], "the grown line");
        let ages = line_ages(&r, &rel);
        assert_eq!(ages.len(), 2);
        assert!(ages[1] >= ages[0], "line 2 is younger or same-instant");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn suggests_deterministic_readable_names() {
        let rev = Rev { oid: "abc123deadbeef".into(), time: 0, subject: "Fix the Auth Bug!! (final)".into(), kind: RevKind::Commit, parents: vec![] };
        let n1 = suggest_branch_name(&rev);
        let n2 = suggest_branch_name(&rev);
        assert_eq!(n1, n2, "same revision, same suggestion, every time");
        assert_eq!(n1, "from/fix-the-auth-bug-abc123de");
        let empty = Rev { oid: "ffffffffffff".into(), time: 0, subject: "".into(), kind: RevKind::Commit, parents: vec![] };
        assert_eq!(suggest_branch_name(&empty), "from/ffffffff", "empty subject still yields a usable name");
    }

    #[test]
    fn branch_from_creates_a_real_branch_and_refuses_a_collision() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-gitspace-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.rs"), "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        let r = GitRunner { root: dir.clone() };
        let head = r.run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();

        branch_from(&r, &head, "keep-exploring").expect("branch created");
        let branches = r.run(&["branch", "--list"]).unwrap();
        assert!(branches.contains("keep-exploring"));

        let err = branch_from(&r, &head, "keep-exploring").unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggests_a_sibling_path_never_nested_inside_the_repo() {
        let root = Path::new("/home/dev/myproj");
        let rev = Rev { oid: "cafef00dbeef".into(), time: 0, subject: "Add the thing".into(), kind: RevKind::Commit, parents: vec![] };
        let p = suggest_worktree_path(root, &rev);
        assert_eq!(p, Path::new("/home/dev/myproj@add-the-thing-cafef00d"));
        assert!(!p.starts_with(root), "never nested inside the tree it snapshots");
    }

    #[test]
    fn materialize_checks_out_a_real_detached_worktree() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-gitspace-mat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.rs"), "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        let r = GitRunner { root: dir.clone() };
        let head = r.run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();

        let dest = std::env::temp_dir().join(format!("aperture-gitspace-mat-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        materialize(&r, &head, dest.to_str().unwrap()).expect("worktree created");
        assert_eq!(std::fs::read_to_string(dest.join("a.rs")).unwrap(), "fn one() {}\n", "the real file, checked out for real");

        let list = r.run(&["worktree", "list"]).unwrap();
        assert!(list.contains(dest.to_str().unwrap()));

        let _ = Command::new("git").arg("-C").arg(&dir).args(["worktree", "remove", "--force", dest.to_str().unwrap()]).output();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn batch_by_day_groups_consecutive_same_day_revs() {
        let day = 86_400i64;
        let mk = |t: i64, s: &str| Rev { oid: s.into(), time: t, subject: s.into(), kind: RevKind::Commit, parents: vec![] };
        let revs = vec![
            mk(day * 3 + 100, "c"), // day 3
            mk(day * 3 + 10, "b"),  // day 3
            mk(day * 1 + 50, "a"),  // day 1 (a gap at day 2 — no revs that day)
        ];
        let batches = batch_by_day(&revs);
        assert_eq!(batches.len(), 2, "{batches:#?}");
        assert_eq!(batches[0].len(), 2, "c and b share a day");
        assert_eq!(batches[1].len(), 1, "a is alone, a day apart");
    }

    #[test]
    fn batch_by_day_handles_empty_and_singleton() {
        assert!(batch_by_day(&[]).is_empty());
        let one = vec![Rev { oid: "x".into(), time: 0, subject: "x".into(), kind: RevKind::Commit, parents: vec![] }];
        let batches = batch_by_day(&one);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn homecoming_marks_and_recalls_a_real_repo() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-gitspace-visited-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.rs"), "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        let r = GitRunner { root: dir.clone() };

        assert!(last_visited(&r, "a.rs").is_none(), "never marked yet");
        let head1 = head_oid(&r).unwrap();
        mark_visited(&r, "a.rs", &head1).expect("marks cleanly");
        assert_eq!(last_visited(&r, "a.rs"), Some(head1.clone()));

        // A second commit lands; the mark stays at the OLD point until
        // explicitly re-marked — "where I left off," not "what's current."
        std::fs::write(dir.join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "two"]);
        assert_eq!(last_visited(&r, "a.rs"), Some(head1.clone()), "unmoved by new commits");

        // Re-marking overwrites, doesn't accumulate a history of its own.
        let head2 = head_oid(&r).unwrap();
        assert_ne!(head1, head2);
        mark_visited(&r, "a.rs", &head2).unwrap();
        assert_eq!(last_visited(&r, "a.rs"), Some(head2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn visited_ref_sanitizes_but_stays_readable() {
        assert_eq!(visited_ref("src/lib.rs"), "refs/aperture/visited/src/lib.rs");
        assert_eq!(visited_ref("weird name!.rs"), "refs/aperture/visited/weird_name_.rs");
    }

    #[test]
    fn parse_log_reads_parents_root_linear_and_merge() {
        let s = "ccc\u{1f}300\u{1f}aaa bbb\u{1f}merge\nbbb\u{1f}200\u{1f}aaa\u{1f}second\naaa\u{1f}100\u{1f}\u{1f}root\n";
        let revs = parse_log(s);
        assert_eq!(revs.len(), 3);
        assert_eq!(revs[0].parents, vec!["aaa".to_string(), "bbb".to_string()], "merge: two parents");
        assert_eq!(revs[1].parents, vec!["aaa".to_string()], "linear: one parent");
        assert!(revs[2].parents.is_empty(), "root commit: no parents");
        // Subjects survived even though %P sits right before them.
        assert_eq!(revs[0].subject, "merge");
        assert_eq!(revs[2].subject, "root");
    }

    #[test]
    fn history_reads_real_parent_oids_along_a_linear_chain() {
        // Path-scoped log (`history()`'s whole point) applies git's default
        // history simplification, which drops merge commits that don't
        // UNIQUELY touch the given path — so the realistic, common case a
        // per-file revision list actually sees is a linear chain, and
        // that's what this proves end to end. Merge-oid PARSING itself is
        // covered directly above (`parse_log_reads_parents_...`), against
        // real git merge output, without the path filter getting in the way.
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-gitspace-parents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "two"]);
        let r = GitRunner { root: dir.clone() };

        let revs = history(&r, "f.txt", 10);
        let two = revs.iter().find(|v| v.subject == "two").unwrap();
        let one = revs.iter().find(|v| v.subject == "one").unwrap();
        assert_eq!(two.parents, vec![one.oid.clone()], "two's parent IS one's real oid");
        assert!(one.parents.is_empty(), "the root commit has none");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Buckets through a REAL repo: save, list, load, round-trip the
    /// text, survive `gc --prune=now`, delete. The `gc` step is the one
    /// worth having — a ref pointing at a blob keeping that blob alive is
    /// the assumption the whole storage choice rests on, and "refs are
    /// reachability roots" is a claim about git, not about this code.
    #[test]
    fn buckets_live_in_refs_and_survive_gc() {
        use crate::bucket::{Bucket, Entry, Store};
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-gitspace-buckets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);

        let r = GitRunner { root: dir.clone() };
        let store = GitStore::new(&r);
        assert!(store.list().is_empty(), "a fresh repo has no buckets");

        let mut b = Bucket::new("Collision investigation");
        b.add(Entry::symbol("src/physics.rs", "resolve_collision"));
        b.add(Entry { note: Some("the caller".into()), ..Entry::symbol("src/player.rs", "update") });
        store.save(&b.slug(), &b.to_text()).unwrap();
        store.save("rendering", &Bucket::new("Rendering").to_text()).unwrap();

        let mut slugs = store.list();
        slugs.sort();
        assert_eq!(slugs, ["collision-investigation", "rendering"]);

        // The ref really is a ref, and it really points at a blob.
        let listed = git(&["for-each-ref", "--format=%(refname) %(objecttype)", "refs/aperture/bucket/"]);
        assert!(
            listed.contains("refs/aperture/bucket/collision-investigation blob"),
            "expected a blob ref, got: {listed}"
        );

        // gc is the real question.
        git(&["gc", "--prune=now", "-q"]);
        let back = crate::bucket::parse(&store.load(&b.slug()).expect("still there after gc")).unwrap();
        assert_eq!(back, b, "the bucket round-trips through a git object");

        store.delete(&b.slug()).unwrap();
        assert_eq!(store.list(), ["rendering"]);
        assert!(store.load(&b.slug()).is_none(), "a deleted bucket does not load");
        store.delete(&b.slug()).unwrap(); // deleting it twice is not an error
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default `run_stdin` refuses rather than pretending. A canned
    /// runner reaching the save path is a bug in the caller, and it has
    /// to be visible as one.
    #[test]
    fn a_runner_without_stdin_refuses_to_save_a_bucket() {
        use crate::bucket::{Bucket, Store};
        let r = Canned(vec![]);
        let err = GitStore::new(&r).save("x", &Bucket::new("x").to_text()).unwrap_err();
        assert!(err.contains("stdin"), "the refusal names the reason: {err}");
    }
}
