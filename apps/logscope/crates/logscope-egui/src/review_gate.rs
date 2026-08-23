//! review_gate.rs — a budget on how much unreviewed work is allowed to accumulate.
//!
//! WHAT THIS IS FOR
//!
//! Months of autonomous work do not produce an unmaintainable mess because isolation failed. They
//! produce one because nobody read the diff, and by the time anyone looks, the context needed to
//! judge it is gone. Every other module here is a security boundary; this one is not. It is a
//! scheduler and a counter, and it is the part that actually prevents the failure people mean when
//! they worry about a long-running agent.
//!
//! THE METRIC: UNREVIEWED LINE-DAYS
//!
//! Not lines, and not age — the product, summed over unreviewed commits.
//!
//! ```text
//! debt = Σ (lines_changed × days_since_written)
//! ```
//!
//! Five hundred lines reviewed the same day is fine. Fifty lines unreviewed for two months is a
//! hazard, because reviewability decays: the author's reasoning is gone, the surrounding code has
//! moved, and whatever assumption the change encoded is now load-bearing somewhere else. Age is the
//! multiplier because it is what turns a diff you could have skimmed into an archaeology problem.
//!
//! This is the same shape as key expiry. A credential is not revoked because it was misused; it is
//! revoked because unbounded validity is the thing that eventually hurts you.
//!
//! HOW IT BITES
//!
//! Past the freeze threshold the branch is not stopped — it is restricted to hermetic, offline
//! verification. The agent can still compile and test what exists; it cannot fetch, and it cannot
//! be granted anything new. That is deliberately survivable: a gate that halts everything gets
//! switched off, and a gate that is switched off is worse than no gate, because you thought you
//! had one.
//!
//! WHAT MAKES IT NON-CIRCUMVENTABLE
//!
//! Reviews are read from the audit log, not from anything in the workspace. The log must live
//! outside the workspace or the agent can mark its own work reviewed — see `audit_log`. This module
//! never writes; it only reads reviews and reports a posture.

use std::path::Path;

/// One unreviewed unit of work. `lines` is added + removed: a deletion still has to be understood.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Commit id, or any stable identifier ordered consistently with `unix_time`.
    pub id: String,
    pub unix_time: u64,
    pub lines: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    /// Line-days at which the UI should start saying something.
    pub warn: u64,
    /// Line-days at which the branch drops to offline-only verification.
    pub freeze: u64,
    /// A hard cap on age regardless of size: anything older than this is overdue on its own, because
    /// a one-line change nobody has looked at in a quarter is still a one-line change nobody
    /// understands. 0 disables.
    pub max_age_days: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        // 1,000 lines untouched for 10 days freezes. 100 lines for 20 days warns. Tune to taste —
        // the units are lines × days, so the numbers mean something you can reason about.
        Self { warn: 2_000, freeze: 10_000, max_age_days: 90 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posture {
    /// Normal operation.
    Open,
    /// Over budget but still working. Surface it; do not restrict.
    Warn,
    /// Offline-only verification, no new capability grants, until someone reads the diff.
    Frozen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub posture: Posture,
    pub debt_line_days: u64,
    pub unreviewed_commits: usize,
    pub unreviewed_lines: u64,
    /// Age of the oldest unreviewed change, in days.
    pub oldest_days: u64,
    /// Line-days that may still accrue before freezing. `None` once frozen.
    pub headroom: Option<u64>,
}

impl Assessment {
    pub fn explain(&self) -> String {
        match self.posture {
            Posture::Open => format!(
                "{} unreviewed commits, {} lines, {} line-days of debt. {} line-days of headroom.",
                self.unreviewed_commits,
                self.unreviewed_lines,
                self.debt_line_days,
                self.headroom.unwrap_or(0)
            ),
            Posture::Warn => format!(
                "review is falling behind: {} commits / {} lines / {} line-days, oldest {} days. \
                 {} line-days before this branch drops to offline-only.",
                self.unreviewed_commits,
                self.unreviewed_lines,
                self.debt_line_days,
                self.oldest_days,
                self.headroom.unwrap_or(0)
            ),
            Posture::Frozen => format!(
                "FROZEN: {} unreviewed commits / {} lines / {} line-days, oldest {} days. \
                 Verification is offline-only and no new capabilities will be granted until someone \
                 reads the diff and records a review.",
                self.unreviewed_commits, self.unreviewed_lines, self.debt_line_days, self.oldest_days
            ),
        }
    }
}

/// The commit id of the most recent recorded review of `branch`, from the audit log.
///
/// Reviews are recorded as `Kind::Review` with `subject = branch` and `detail` beginning with the
/// reviewed commit id. Nothing in the workspace is consulted.
pub fn last_reviewed(log_path: &Path, branch: &str) -> Option<(String, u64)> {
    let log = crate::audit_log::AuditLog::at(log_path);
    log.entries()
        .into_iter()
        .filter(|e| e.kind == crate::audit_log::Kind::Review && e.subject == branch)
        .last()
        .map(|e| {
            let id = e.detail.split_whitespace().next().unwrap_or("").to_string();
            (id, e.unix_time)
        })
}

/// Record that a human read up to `commit`. The caller is responsible for that being true; this is
/// a ledger, not a proof.
///
/// Also the way to adopt the gate on an existing repo: record the current HEAD once as a baseline,
/// which starts the clock rather than billing you for history you were never going to read.
pub fn record_review(
    log_path: &Path, branch: &str, commit: &str, note: &str,
) -> std::io::Result<()> {
    crate::audit_log::AuditLog::at(log_path)
        .append(crate::audit_log::Kind::Review, branch, &format!("{commit} {note}"))
        .map(|_| ())
}

/// Assess a branch. `changes` should be every commit on it; anything at or before the last recorded
/// review is discounted.
///
/// IMPORTANT: with no recorded review, this measures the whole history, which for an existing repo
/// is an enormous number that means nothing — run against this monorepo's `main` it reported 3,313
/// commits and 367 million line-days, which is true and useless. The gate measures work done SINCE
/// a baseline. Call `record_review` once at adoption time with the current HEAD ("baseline, not
/// read") and the number becomes what you actually want: what the agent has written since.
pub fn assess(
    changes: &[Change], reviewed_up_to: Option<&str>, now: u64, th: Thresholds,
) -> Assessment {
    // Everything up to and including the reviewed commit is settled. Position, not timestamp:
    // commit times are attacker- and rebase-controlled, ordering in the branch is not.
    let start = match reviewed_up_to {
        Some(id) => changes.iter().position(|c| c.id == id).map(|i| i + 1).unwrap_or(0),
        None => 0,
    };
    let pending = &changes[start.min(changes.len())..];

    let mut debt: u64 = 0;
    let mut lines: u64 = 0;
    let mut oldest_days: u64 = 0;
    for c in pending {
        // saturating: a commit dated in the future (clock skew, a rebase, a lying timestamp)
        // contributes zero age rather than wrapping into an enormous debt.
        let age_days = now.saturating_sub(c.unix_time) / 86_400;
        debt = debt.saturating_add(c.lines.saturating_mul(age_days));
        lines = lines.saturating_add(c.lines);
        oldest_days = oldest_days.max(age_days);
    }

    let over_age = th.max_age_days > 0 && oldest_days >= th.max_age_days && !pending.is_empty();
    let posture = if debt >= th.freeze || over_age {
        Posture::Frozen
    } else if debt >= th.warn {
        Posture::Warn
    } else {
        Posture::Open
    };

    Assessment {
        posture,
        debt_line_days: debt,
        unreviewed_commits: pending.len(),
        unreviewed_lines: lines,
        oldest_days,
        headroom: if posture == Posture::Frozen { None } else { Some(th.freeze - debt) },
    }
}

// ---------------------------------------------------------------- feeding it real history

/// Where the reviewed-up-to marker lives, relative to the repo root.
pub const BASELINE_PATH: &str = ".logscope/reviewed";

/// Parse `git log --pretty=format:%H %ct --numstat` into changes, OLDEST FIRST.
///
/// Order matters and is not cosmetic: `assess` finds the baseline by POSITION and treats everything
/// up to it as settled, so a newest-first list would mark the wrong side of the branch reviewed —
/// silently, and in the direction that hides debt. git prints newest first, so this reverses.
///
/// Kept pure and separate from the process call because everything interesting here is parsing: a
/// merge commit with no numstat lines, a binary file printed as `-`, a rename line with braces. A
/// test can hand it those; a test that has to build a git repo first mostly tests git.
pub fn parse_git_log(text: &str) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() { continue; }
        let mut parts = line.split_whitespace();
        let (a, b) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let is_header = a.len() >= 7 && a.chars().all(|c| c.is_ascii_hexdigit())
            && b.chars().all(|c| c.is_ascii_digit()) && !b.is_empty();
        if is_header {
            out.push(Change { id: a.to_string(), unix_time: b.parse().unwrap_or(0), lines: 0 });
            continue;
        }
        // numstat: added \t deleted \t path. "-" means binary, which contributes no LINES to
        // review — a reviewer does not read a PNG, and counting its bytes as debt would freeze a
        // branch for adding an icon.
        let (add, del) = (a, b);
        if add == "-" && del == "-" { continue; }
        let n: u64 = add.parse::<u64>().unwrap_or(0) + del.parse::<u64>().unwrap_or(0);
        if let Some(c) = out.last_mut() { c.lines = c.lines.saturating_add(n); }
    }
    out.reverse();
    out
}

/// Read the branch's commit history. `limit` bounds the walk — a repo with 40k commits does not
/// need them all to answer "how much is unreviewed", and an unbounded log on a large tree is a
/// visible stall in the UI thread.
pub fn read_history(repo: &std::path::Path, limit: usize) -> std::io::Result<Vec<Change>> {
    let out = std::process::Command::new("git")
        .arg("-C").arg(repo)
        .args(["log", &format!("-{limit}"), "--pretty=format:%H %ct", "--numstat", "--no-color"])
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(parse_git_log(&String::from_utf8_lossy(&out.stdout)))
}

/// The commit the human has read up to, if they have said so.
pub fn read_baseline(repo: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(repo.join(BASELINE_PATH)).ok()
        .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Record that review has caught up to `commit`.
///
/// A FILE rather than a git ref or a config key, for a reason that shows up on the second machine:
/// the marker is about what a PERSON has read, so it belongs to the checkout rather than to the
/// branch. Pushing it would tell a colleague their unread commits are reviewed.
pub fn write_baseline(repo: &std::path::Path, commit: &str) -> std::io::Result<()> {
    let p = repo.join(BASELINE_PATH);
    if let Some(d) = p.parent() { std::fs::create_dir_all(d)?; }
    std::fs::write(p, format!("{}\n", commit.trim()))
}

/// The whole question in one call: how much unreviewed work is on this branch, and what posture
/// does that imply.
///
/// A repo that is not a git checkout, or a git that fails, yields Posture::Open rather than an
/// error the caller must handle — the review budget is a safety rail on top of a workflow, and a
/// rail that stops the workflow when it cannot measure is worse than no rail. The Err case is still
/// returned so a caller that wants to say something can.
pub fn assess_repo(repo: &std::path::Path, now: u64, th: Thresholds) -> std::io::Result<Assessment> {
    // ADOPTION. With no baseline, `assess` correctly reports that nothing has ever been reviewed —
    // and on any real repository that means FROZEN on the first run. Measured on this monorepo:
    // 501 commits, 870k lines, 8 million line-days, instantly frozen. A rail that fires the moment
    // it is installed gets switched off, and then it protects nothing.
    //
    // So the first run ADOPTS: it records HEAD as the baseline and starts counting from there. The
    // claim is honest — the marker means "review has caught up to here", and installing the gate is
    // exactly the moment you decide the existing tree is what you are building on. What it must not
    // do is silently re-adopt later, which is why it only writes when the file is absent.
    if read_baseline(repo).is_none() {
        if let Some(head) = head_commit(repo) {
            write_baseline(repo, &head)?;
            eprintln!("review-gate: adopted this checkout at {} — commits from here on count \
toward the review budget", &head[..head.len().min(12)]);
        }
    }
    let changes = read_history(repo, HISTORY_LIMIT)?;
    // A walk that hits its cap makes every number a FLOOR rather than a count. Saying so matters
    // because the posture derived from a truncated history can only be too lenient, never too
    // strict, and a lenient safety rail that looks precise is the worse failure.
    if changes.len() >= HISTORY_LIMIT {
        eprintln!("review-gate: history walk hit {HISTORY_LIMIT} commits — debt figures are a floor");
    }
    Ok(assess(&changes, read_baseline(repo).as_deref(), now, th))
}

/// How far back the history walk goes. Bounded because an unbounded `git log --numstat` on a large
/// tree is a visible stall, and the question ("how much is unreviewed") is answered by the recent
/// end of the branch once a baseline exists.
pub const HISTORY_LIMIT: usize = 500;

/// The current HEAD commit, or None when this is not a git checkout.
pub fn head_commit(repo: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Make the posture bite. A frozen branch may still build and test what it has; it may not reach the
/// network, and it may not be promoted to a more trusted tier.
pub fn apply(posture: Posture, spec: &mut crate::sandbox::VerifySpec) {
    if posture == Posture::Frozen {
        spec.net = false;
        spec.allowed_deps.clear();
        if spec.trust == crate::isolate::Trust::Own {
            spec.trust = crate::isolate::Trust::Generated;
        }
    }
}

/// Whether a new capability (GPU lease, network grant, writable path) may be handed out.
pub fn grants_allowed(posture: Posture) -> bool {
    posture != Posture::Frozen
}

/// Read a branch's commits with their line counts straight from git. Shells out to `git log
/// --numstat` rather than parsing a repo by hand: the numbers must match what a human would see in
/// the diff they are being asked to read.
///
/// Returns oldest-first, which is the order `assess` expects.
pub fn changes_from_git(repo: &Path, branch: &str) -> std::io::Result<Vec<Change>> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["log", "--reverse", "--numstat", "--format=%x01%H %ct", branch])
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut changes: Vec<Change> = Vec::new();
    for line in text.lines() {
        if let Some(hdr) = line.strip_prefix('\u{1}') {
            let mut it = hdr.split_whitespace();
            let id = it.next().unwrap_or("").to_string();
            let t: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            changes.push(Change { id, unix_time: t, lines: 0 });
            continue;
        }
        // "<added>\t<removed>\t<path>"; binary files report "-" and count as zero lines,
        // which is right — there is no diff for a human to read.
        let mut f = line.split('\t');
        let add: u64 = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let del: u64 = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if let Some(c) = changes.last_mut() {
            c.lines = c.lines.saturating_add(add).saturating_add(del);
        }
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;
    fn now() -> u64 {
        1_000 * DAY
    }
    fn ch(id: &str, days_ago: u64, lines: u64) -> Change {
        Change { id: id.into(), unix_time: now() - days_ago * DAY, lines }
    }

    #[test]
    fn a_big_change_reviewed_today_is_no_debt() {
        let a = assess(&[ch("a", 0, 5_000)], None, now(), Thresholds::default());
        assert_eq!(a.debt_line_days, 0);
        assert_eq!(a.posture, Posture::Open);
    }

    #[test]
    fn a_small_change_left_for_months_still_freezes() {
        // 20 lines × 95 days = 1,900 line-days: under the freeze budget, but past max_age.
        let a = assess(&[ch("a", 95, 20)], None, now(), Thresholds::default());
        assert!(a.debt_line_days < Thresholds::default().freeze);
        assert_eq!(a.posture, Posture::Frozen, "age alone must be able to freeze");
        assert_eq!(a.oldest_days, 95);
    }

    #[test]
    fn debt_is_the_product_not_the_sum() {
        let th = Thresholds { warn: 2_000, freeze: 10_000, max_age_days: 0 };
        let many_lines_fresh = assess(&[ch("a", 1, 5_000)], None, now(), th);
        let few_lines_old = assess(&[ch("b", 100, 50)], None, now(), th);
        assert_eq!(many_lines_fresh.debt_line_days, 5_000);
        assert_eq!(few_lines_old.debt_line_days, 5_000);
        assert_eq!(many_lines_fresh.posture, Posture::Warn);
        assert_eq!(few_lines_old.posture, Posture::Warn);
    }

    #[test]
    fn crossing_the_thresholds() {
        let th = Thresholds { warn: 2_000, freeze: 10_000, max_age_days: 0 };
        assert_eq!(assess(&[ch("a", 1, 100)], None, now(), th).posture, Posture::Open);
        assert_eq!(assess(&[ch("a", 20, 100)], None, now(), th).posture, Posture::Warn);
        assert_eq!(assess(&[ch("a", 10, 1_000)], None, now(), th).posture, Posture::Frozen);
    }

    #[test]
    fn reviewing_clears_only_what_was_reviewed() {
        let th = Thresholds { warn: 2_000, freeze: 10_000, max_age_days: 0 };
        let changes = vec![ch("a", 30, 400), ch("b", 20, 400), ch("c", 1, 400)];
        // Nothing reviewed: 12,000 + 8,000 + 400.
        assert_eq!(assess(&changes, None, now(), th).posture, Posture::Frozen);
        // Reviewed through "b": only "c" remains.
        let after = assess(&changes, Some("b"), now(), th);
        assert_eq!(after.unreviewed_commits, 1);
        assert_eq!(after.debt_line_days, 400);
        assert_eq!(after.posture, Posture::Open);
    }

    #[test]
    fn an_unknown_review_marker_discounts_nothing() {
        // A review naming a commit that is not on this branch must not silently clear the debt.
        let th = Thresholds::default();
        let changes = vec![ch("a", 30, 1_000)];
        let a = assess(&changes, Some("does-not-exist"), now(), th);
        assert_eq!(a.unreviewed_commits, 1);
        assert_eq!(a.posture, Posture::Frozen);
    }

    #[test]
    fn a_future_dated_commit_cannot_inflate_or_wrap_the_debt() {
        let th = Thresholds::default();
        let future = Change { id: "f".into(), unix_time: now() + 500 * DAY, lines: 1_000 };
        let a = assess(&[future], None, now(), th);
        assert_eq!(a.debt_line_days, 0, "clock skew must not create debt");
        assert_eq!(a.posture, Posture::Open);
    }

    #[test]
    fn freezing_forces_offline_and_drops_vetted_deps() {
        let mut spec = crate::sandbox::VerifySpec::cargo_build("/tmp/x")
            .with_net(true)
            .with_trust(crate::isolate::Trust::Own)
            .allowing(&["serde".to_string()]);
        apply(Posture::Frozen, &mut spec);
        assert!(!spec.net, "a frozen branch must not reach the network");
        assert!(spec.allowed_deps.is_empty());
        assert_eq!(spec.trust, crate::isolate::Trust::Generated, "no trust promotion while frozen");
        assert!(!grants_allowed(Posture::Frozen));
    }

    #[test]
    fn a_warned_branch_is_not_restricted() {
        let mut spec = crate::sandbox::VerifySpec::cargo_build("/tmp/x").with_net(true);
        apply(Posture::Warn, &mut spec);
        assert!(spec.net, "warning is a message, not a restriction");
        assert!(grants_allowed(Posture::Warn));
    }

    #[test]
    fn reviews_come_from_the_log_and_round_trip() {
        let p = std::env::temp_dir().join(format!("ls-review-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert_eq!(last_reviewed(&p, "main"), None);

        record_review(&p, "main", "abc123", "read the parser rewrite").unwrap();
        record_review(&p, "other", "zzz999", "unrelated branch").unwrap();
        record_review(&p, "main", "def456", "read the rest").unwrap();

        let (id, _t) = last_reviewed(&p, "main").unwrap();
        assert_eq!(id, "def456", "the latest review for THIS branch wins");
        assert_eq!(crate::audit_log::AuditLog::at(&p).verify(), Ok(3));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn the_explanation_says_what_to_do() {
        let a = assess(&[ch("a", 30, 1_000)], None, now(), Thresholds::default());
        let s = a.explain();
        assert!(s.contains("FROZEN"));
        assert!(s.contains("offline-only"));
        assert!(s.contains("records a review"));
    }

    #[test]
    fn headroom_is_reported_while_there_is_any() {
        let th = Thresholds { warn: 2_000, freeze: 10_000, max_age_days: 0 };
        let a = assess(&[ch("a", 1, 1_000)], None, now(), th);
        assert_eq!(a.headroom, Some(9_000));
        let b = assess(&[ch("a", 20, 1_000)], None, now(), th);
        assert_eq!(b.headroom, None, "no headroom once frozen");
    }

    #[test]
    fn git_reader_counts_the_lines_a_human_would_have_to_read() {
        let d = std::env::temp_dir().join(format!("ls-rg-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-C"]).arg(&d).args(args)
                .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@e")
                .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@e")
                .output().unwrap();
            assert!(ok.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&ok.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "first"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "second"]);

        let changes = changes_from_git(&d, "main").unwrap();
        assert_eq!(changes.len(), 2, "oldest first: {changes:?}");
        assert_eq!(changes[0].lines, 3, "three added lines");
        assert_eq!(changes[1].lines, 1, "one added line");
        assert!(changes[0].unix_time <= changes[1].unix_time);

        // and it composes with assess(): reviewing the first commit leaves only the second
        let a = assess(&changes, Some(&changes[0].id), now(), Thresholds::default());
        assert_eq!(a.unreviewed_commits, 1);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// Ignored by default: runs against whatever repo CBX_REPO points at, to sanity-check the
    /// reader on a real history rather than a two-commit fixture.
    ///   CBX_REPO=/path/to/repo cargo test -p logscope-egui --lib --no-default-features -- --ignored real_repo --nocapture
    #[test]
    #[ignore]
    fn real_repo_smoke() {
        let repo = std::env::var("CBX_REPO").expect("set CBX_REPO");
        let branch = std::env::var("CBX_BRANCH").unwrap_or_else(|_| "main".into());
        let changes = changes_from_git(Path::new(&repo), &branch).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let a = assess(&changes, None, now, Thresholds::default());
        println!("{} commits on {branch}", changes.len());
        println!("{}", a.explain());
        assert!(!changes.is_empty(), "no commits parsed — the reader is broken");
    }

    /// Adoption on an existing repo: baseline at HEAD, then only new work counts.
    #[test]
    fn a_baseline_review_makes_an_existing_repo_measurable() {
        let th = Thresholds::default();
        // A long history nobody is going to read retroactively.
        let mut history: Vec<Change> = (0..50)
            .map(|i| ch(&format!("old{i}"), 300 - i as u64, 2_000))
            .collect();
        assert_eq!(assess(&history, None, now(), th).posture, Posture::Frozen);

        let baseline = history.last().unwrap().id.clone();
        // Agent then writes something modest.
        history.push(ch("new1", 2, 300));
        let a = assess(&history, Some(&baseline), now(), th);
        assert_eq!(a.unreviewed_commits, 1);
        assert_eq!(a.debt_line_days, 600);
        assert_eq!(a.posture, Posture::Open, "the clock starts at adoption, not at genesis");
    }

    #[test]
    fn an_empty_branch_is_open() {
        let a = assess(&[], None, now(), Thresholds::default());
        assert_eq!(a.posture, Posture::Open);
        assert_eq!(a.unreviewed_commits, 0);
        assert_eq!(a.oldest_days, 0);
    }
    #[test]
    fn git_log_parses_OLDEST_first_and_sums_both_sides_of_a_diff() {
        // Newest-first would make assess treat the wrong end of the branch as reviewed — silently,
        // and in the direction that hides debt.
        let log = "aaaaaaa 2000\n10\t2\tsrc/a.rs\n5\t0\tsrc/b.rs\nbbbbbbb 1000\n1\t1\tsrc/c.rs\n";
        let cs = parse_git_log(log);
        assert_eq!(cs.len(), 2);
        assert_eq!((cs[0].id.as_str(), cs[0].unix_time, cs[0].lines), ("bbbbbbb", 1000, 2), "oldest first");
        assert_eq!(cs[1].lines, 17, "additions AND deletions are both review work");
    }

    #[test]
    fn a_merge_with_no_numstat_and_a_BINARY_file_do_not_distort_the_debt() {
        // A merge commit prints no numstat; a binary prints "-\t-". Counting either as lines would
        // freeze a branch for adding an icon or for merging.
        let log = "ccccccc 3000\nddddddd 2500\n-\t-\tlogo.png\n3\t0\tsrc/x.rs\n";
        let cs = parse_git_log(log);
        assert_eq!(cs.iter().map(|c| c.lines).sum::<u64>(), 3);
    }

    #[test]
    fn the_baseline_round_trips_through_the_checkout() {
        let dir = std::env::temp_dir().join("logscope-baseline-test");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_baseline(&dir).is_none(), "no marker = nothing reviewed");
        write_baseline(&dir, "  deadbeef\n").expect("write");
        assert_eq!(read_baseline(&dir).as_deref(), Some("deadbeef"), "trimmed on both ends");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "needs git and a real checkout; run with --ignored"]
    fn real_repo_assessment_runs() {
        let repo = std::env::var("CBX_REPO").unwrap_or_else(|_| ".".into());
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        let a = assess_repo(std::path::Path::new(&repo), now, Thresholds::default()).expect("git log");
        eprintln!("{}", a.explain());
        assert!(a.unreviewed_commits > 0, "a repo with history has unreviewed commits by default");
    }

    #[test]
    #[ignore = "needs git; run with --ignored"]
    fn adopting_a_repo_does_not_freeze_it_on_the_first_run() {
        // The flaw this fixes, pinned: measured on the monorepo, a first run without adoption
        // reported 8 MILLION line-days of debt and froze immediately.
        let repo = std::env::var("CBX_REPO").unwrap_or_else(|_| ".".into());
        let repo = std::path::Path::new(&repo);
        let marker = repo.join(BASELINE_PATH);
        let had = std::fs::read_to_string(&marker).ok();
        let _ = std::fs::remove_file(&marker);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        let a = assess_repo(repo, now, Thresholds::default()).expect("git");
        // restore whatever was there before touching the operator's checkout
        match had { Some(v) => { let _ = std::fs::write(&marker, v); } None => { let _ = std::fs::remove_file(&marker); } }
        assert_eq!(a.posture, Posture::Open, "adoption must not freeze: {}", a.explain());
        assert_eq!(a.unreviewed_commits, 0);
    }

}
