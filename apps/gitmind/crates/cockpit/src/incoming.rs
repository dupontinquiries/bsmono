//! incoming — taking someone else's work into this repo.
//!
//! A phone is a poor place to resolve a failed merge, so the interesting part
//! of this module is deciding what to do BEFORE anything is applied. That
//! decision is the one this project keeps getting wrong in practice, and the
//! evidence is a day of it:
//!
//! * A `logscope-agent-isolation.bundle` that could not be fetched, because it
//!   was incremental against a commit a history rewrite had replaced.
//! * A `clawbound-changes.bundle` where the same thing happened and the base
//!   was unreconstructable — its tree contained blobs that had been
//!   deliberately deleted. Ten commit messages were recoverable by stripping
//!   the bundle header and unpacking the raw packfile; not one file was.
//! * The same clawbound work re-sent as `format-patch` output, which applied
//!   with `git am -3` onto a HEAD it was never authored against.
//!
//! The pattern: **a bundle references history, a patch series references
//! content.** After a rewrite only the second still applies, and a rewrite is
//! not rare here — three happened in one day.
//!
//! ## What this models and what it does not
//!
//! It classifies an artifact, reports what it needs, and drives an apply as a
//! state machine with resume points. It runs no git: the host does that and
//! feeds results back, so the whole decision path is testable without a repo.

/// What arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Artifact {
    /// A git bundle. `refs` are what it carries, `requires` its prerequisites.
    Bundle { refs: Vec<String>, requires: Vec<String> },
    /// A `format-patch` series, in order.
    Patches { count: usize },
    /// Something else — a zip of files, a tarball.
    Files { count: usize },
}

/// Can this be applied here?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// Every prerequisite is present. A bundle can simply be fetched.
    Ready,
    /// Prerequisites are missing, and they are listed.
    ///
    /// This is the case that matters. A bundle in this state is not "broken" —
    /// it is complete and unusable, and the remedy is to ask for patches
    /// rather than to try harder.
    MissingBase(Vec<String>),
    /// Nothing to apply.
    Empty,
}

impl Readiness {
    pub fn can_apply(&self) -> bool {
        matches!(self, Readiness::Ready)
    }
}

/// How to bring the work in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `git fetch <bundle> <ref>:<ref>` — keeps history exactly.
    Fetch,
    /// `git am -3` — applies by CONTENT, so a rewritten base does not matter.
    /// Keeps author, date and message; the commit hashes are new.
    ThreeWayAm,
    /// Copy files in and commit as one. Loses the incoming history.
    CopyTree,
    /// Nothing can be done with this artifact here.
    Refuse,
}

impl Route {
    pub fn label(self) -> &'static str {
        match self {
            Route::Fetch => "fetch the bundle",
            Route::ThreeWayAm => "apply the patch series (git am -3)",
            Route::CopyTree => "copy the files in as one commit",
            Route::Refuse => "cannot be applied here",
        }
    }

    /// Does the incoming history survive?
    pub fn keeps_history(self) -> bool {
        matches!(self, Route::Fetch | Route::ThreeWayAm)
    }
}

/// What the operator is told and offered.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub route: Route,
    pub readiness: Readiness,
    /// One line explaining the choice.
    pub because: String,
    /// What to ask the sender for, when nothing here can work.
    pub ask_for: Option<String>,
}

/// Decide how to take an artifact.
///
/// `have` is the set of commits this repo already has — the caller resolves
/// that, because only it can ask git.
pub fn plan(a: &Artifact, have: &dyn Fn(&str) -> bool) -> Plan {
    match a {
        Artifact::Bundle { refs, requires } if refs.is_empty() => Plan {
            route: Route::Refuse,
            readiness: Readiness::Empty,
            because: "the bundle carries no refs".into(),
            ask_for: None,
            }.tap_requires(requires),
        Artifact::Bundle { requires, .. } => {
            let missing: Vec<String> =
                requires.iter().filter(|r| !have(r)).cloned().collect();
            if missing.is_empty() {
                Plan {
                    route: Route::Fetch,
                    readiness: Readiness::Ready,
                    because: "every prerequisite is present".into(),
                    ask_for: None,
                }
            } else {
                // NOT a fallback to CopyTree. The bundle's own objects are
                // unreadable without the base — that was tried, and even the
                // raw packfile left every tree an unresolved delta. The only
                // real remedy is a different artifact.
                Plan {
                    route: Route::Refuse,
                    readiness: Readiness::MissingBase(missing.clone()),
                    because: format!(
                        "incremental against {} commit(s) this repo does not have — \
                         a bundle references history, and a rewrite replaced it",
                        missing.len()
                    ),
                    ask_for: Some(
                        "git format-patch <base>..HEAD -o patches/ — patches apply by \
                         content, so a rewritten base does not matter"
                            .into(),
                    ),
                }
            }
        }
        Artifact::Patches { count } if *count == 0 => Plan {
            route: Route::Refuse,
            readiness: Readiness::Empty,
            because: "no patches in the series".into(),
            ask_for: None,
        },
        Artifact::Patches { count } => Plan {
            route: Route::ThreeWayAm,
            readiness: Readiness::Ready,
            because: format!(
                "{count} patches — applied by content, so the base this was \
                 written against does not have to still exist"
            ),
            ask_for: None,
        },
        Artifact::Files { count } if *count == 0 => Plan {
            route: Route::Refuse,
            readiness: Readiness::Empty,
            because: "the archive is empty".into(),
            ask_for: None,
        },
        Artifact::Files { count } => Plan {
            route: Route::CopyTree,
            readiness: Readiness::Ready,
            because: format!("{count} files — no history to keep, so it lands as one commit"),
            ask_for: Some(
                "if the sender has the commits, patches keep their messages and dates".into(),
            ),
        },
    }
}

impl Plan {
    fn tap_requires(self, _r: &[String]) -> Plan {
        self
    }
}

/// Where an apply has got to.
///
/// `git am` stops on the first patch it cannot apply and waits. That pause is
/// the whole reason this is a state machine rather than a call: on a phone the
/// operator needs to see WHICH patch stopped, what conflicted, and the three
/// choices, without a terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum Apply {
    Idle,
    /// Applying patch `at` of `total`.
    Running { at: usize, total: usize },
    /// Stopped on a conflict.
    Conflicted {
        at: usize,
        total: usize,
        /// Paths with conflict markers.
        paths: Vec<String>,
    },
    /// Every patch applied.
    Done { applied: usize },
    /// Given up; the tree is back where it started.
    Aborted { at: usize, reason: String },
}

/// What the operator can do from a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Conflicts fixed in the working tree; continue.
    Continue,
    /// This patch is already applied or unwanted; move past it.
    Skip,
    /// Undo everything and restore the original HEAD.
    Abort,
}

/// An apply in progress.
#[derive(Debug, Clone)]
pub struct Session {
    pub state: Apply,
    pub total: usize,
    /// Patch subjects, for naming which one stopped.
    pub subjects: Vec<String>,
}

impl Session {
    pub fn new(subjects: Vec<String>) -> Session {
        Session { state: Apply::Idle, total: subjects.len(), subjects }
    }

    pub fn start(&mut self) {
        self.state = if self.total == 0 {
            Apply::Done { applied: 0 }
        } else {
            Apply::Running { at: 0, total: self.total }
        };
    }

    /// The current patch applied cleanly.
    pub fn advance(&mut self) {
        if let Apply::Running { at, total } = self.state {
            let next = at + 1;
            self.state = if next >= total {
                Apply::Done { applied: total }
            } else {
                Apply::Running { at: next, total }
            };
        }
    }

    /// The current patch stopped.
    pub fn conflict(&mut self, paths: Vec<String>) {
        if let Apply::Running { at, total } = self.state {
            self.state = Apply::Conflicted { at, total, paths };
        }
    }

    /// Apply a decision. Returns whether the run continues.
    pub fn resolve(&mut self, r: Resolution) -> bool {
        match (&self.state, r) {
            (Apply::Conflicted { at, total, .. }, Resolution::Continue)
            | (Apply::Conflicted { at, total, .. }, Resolution::Skip) => {
                let (at, total) = (*at, *total);
                self.state = Apply::Running { at, total };
                self.advance();
                !matches!(self.state, Apply::Done { .. })
            }
            (Apply::Conflicted { at, .. }, Resolution::Abort) => {
                self.state = Apply::Aborted {
                    at: *at,
                    reason: "aborted at a conflict; the tree is unchanged".into(),
                };
                false
            }
            _ => false,
        }
    }

    /// Which patch is in hand, by subject.
    pub fn current_subject(&self) -> Option<&str> {
        let i = match self.state {
            Apply::Running { at, .. } | Apply::Conflicted { at, .. } => at,
            _ => return None,
        };
        self.subjects.get(i).map(String::as_str)
    }

    /// The line under a progress bar.
    pub fn status_line(&self) -> String {
        match &self.state {
            Apply::Idle => "ready".into(),
            Apply::Running { at, total } => format!(
                "applying {} of {}: {}",
                at + 1, total,
                self.current_subject().unwrap_or("")
            ),
            Apply::Conflicted { at, total, paths } => format!(
                "stopped at {} of {} — {} conflicted file(s)",
                at + 1, total, paths.len()
            ),
            Apply::Done { applied } => format!("{applied} applied"),
            Apply::Aborted { at, reason } => format!("stopped at {}: {reason}", at + 1),
        }
    }
}

/// A step in the mobile wizard.
///
/// The same model drives both layouts: a phone walks these one at a time
/// because a wizard is what fits, and a desktop shows all of them at once
/// because the operator can hold the whole plan in view and skip ahead. The
/// STEPS do not differ between the two — only how many are on screen — so a
/// decision made on a phone and a decision made on a desktop cannot diverge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// What arrived, and what it needs.
    Inspect,
    /// The route, with alternatives.
    Choose,
    /// The apply itself.
    Apply,
    /// Keep or roll back.
    Confirm,
}

impl Step {
    pub const ALL: [Step; 4] = [Step::Inspect, Step::Choose, Step::Apply, Step::Confirm];

    pub fn title(self) -> &'static str {
        match self {
            Step::Inspect => "What arrived",
            Step::Choose => "How to take it",
            Step::Apply => "Applying",
            Step::Confirm => "Keep it?",
        }
    }

    /// Can this step be reached yet?
    ///
    /// Gated on the plan rather than on step order: a bundle whose base is
    /// missing must not let the operator walk to Apply and press a button that
    /// cannot work.
    pub fn reachable(self, plan: &Plan, session: &Session) -> bool {
        match self {
            Step::Inspect => true,
            Step::Choose => plan.route != Route::Refuse || plan.ask_for.is_some(),
            Step::Apply => plan.readiness.can_apply(),
            Step::Confirm => matches!(
                session.state,
                Apply::Done { .. } | Apply::Aborted { .. }
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing_present(_: &str) -> bool {
        false
    }
    fn all_present(_: &str) -> bool {
        true
    }

    #[test]
    fn a_bundle_with_a_missing_base_asks_for_patches_rather_than_trying_harder() {
        // The clawbound case, exactly. The bundle was complete and unusable:
        // its objects could not be read without the base, and even unpacking
        // the raw packfile left every tree an unresolved delta. The remedy is
        // a different artifact, not more effort.
        let a = Artifact::Bundle {
            refs: vec!["refs/heads/main".into()],
            requires: vec!["e2bdd70c9a9fbaee207935e7eb3c340dfa9a938a".into()],
        };
        let p = plan(&a, &nothing_present);
        assert_eq!(p.route, Route::Refuse);
        assert!(matches!(p.readiness, Readiness::MissingBase(ref m) if m.len() == 1));
        assert!(!p.readiness.can_apply());
        assert!(p.ask_for.as_ref().unwrap().contains("format-patch"));
        assert!(p.because.contains("rewrite"), "{}", p.because);
    }

    #[test]
    fn the_same_bundle_is_fetchable_where_the_base_exists() {
        let a = Artifact::Bundle {
            refs: vec!["refs/heads/main".into()],
            requires: vec!["e2bdd70c".into()],
        };
        let p = plan(&a, &all_present);
        assert_eq!(p.route, Route::Fetch);
        assert!(p.readiness.can_apply());
        assert!(p.route.keeps_history());
    }

    #[test]
    fn patches_apply_regardless_of_what_the_base_is() {
        // The property that makes them the recommended re-send: format-patch
        // applies by CONTENT, so a rewritten base is irrelevant. This is not
        // theoretical — the clawbound series applied onto a HEAD three
        // rewrites away from the one it was authored against.
        let p = plan(&Artifact::Patches { count: 11 }, &nothing_present);
        assert_eq!(p.route, Route::ThreeWayAm);
        assert!(p.readiness.can_apply(), "patches must not need a base to be present");
        assert!(p.route.keeps_history(), "am keeps author, date and message");
    }

    #[test]
    fn a_file_drop_says_plainly_that_history_is_lost() {
        // A zip of files is applicable and lossy, and the operator should be
        // told which — silently squashing eleven commits into one is the thing
        // that makes a later bisect useless.
        let p = plan(&Artifact::Files { count: 67 }, &nothing_present);
        assert_eq!(p.route, Route::CopyTree);
        assert!(!p.route.keeps_history());
        assert!(p.ask_for.as_ref().unwrap().contains("patches"),
                "the better option should still be named");
    }

    #[test]
    fn an_empty_artifact_is_refused_rather_than_started() {
        for a in [
            Artifact::Bundle { refs: vec![], requires: vec![] },
            Artifact::Patches { count: 0 },
            Artifact::Files { count: 0 },
        ] {
            let p = plan(&a, &all_present);
            assert_eq!(p.route, Route::Refuse, "{a:?}");
            assert_eq!(p.readiness, Readiness::Empty);
        }
    }

    #[test]
    fn a_conflict_names_the_patch_that_stopped_and_offers_three_ways_out() {
        // On a phone there is no terminal to read. Which patch, what
        // conflicted, and the three choices have to be on screen.
        let mut s = Session::new(vec![
            "Battleground of the Ancestors".into(),
            "battleground deepened".into(),
            "the deep roster".into(),
        ]);
        s.start();
        s.advance();
        s.conflict(vec!["games/clawbound/README.md".into()]);
        assert_eq!(s.current_subject(), Some("battleground deepened"));
        assert!(s.status_line().contains("2 of 3"));
        assert!(s.status_line().contains("1 conflicted"));

        // Continue moves on.
        assert!(s.resolve(Resolution::Continue));
        assert_eq!(s.current_subject(), Some("the deep roster"));
    }

    #[test]
    fn skip_and_continue_both_advance_but_abort_stops_everything() {
        let mut s = Session::new(vec!["one".into(), "two".into()]);
        s.start();
        s.conflict(vec!["a.rs".into()]);
        assert!(s.resolve(Resolution::Skip), "skip moves past the patch");

        let mut s = Session::new(vec!["one".into(), "two".into()]);
        s.start();
        s.conflict(vec!["a.rs".into()]);
        assert!(!s.resolve(Resolution::Abort));
        match s.state {
            Apply::Aborted { at, ref reason } => {
                assert_eq!(at, 0);
                assert!(reason.contains("unchanged"), "the operator must know the tree is safe");
            }
            ref other => panic!("expected Aborted, got {other:?}"),
        }
    }

    #[test]
    fn the_last_patch_finishes_the_run() {
        let mut s = Session::new(vec!["one".into(), "two".into()]);
        s.start();
        s.advance();
        s.advance();
        assert_eq!(s.state, Apply::Done { applied: 2 });
        assert_eq!(s.current_subject(), None, "nothing is in hand once it is done");
    }

    #[test]
    fn apply_cannot_be_reached_when_the_base_is_missing() {
        // The gate that stops a phone operator walking to a button that cannot
        // work. Choose stays reachable, because there IS something to say
        // there: what to ask the sender for.
        let a = Artifact::Bundle {
            refs: vec!["main".into()],
            requires: vec!["deadbeef".into()],
        };
        let p = plan(&a, &nothing_present);
        let s = Session::new(vec![]);
        assert!(Step::Inspect.reachable(&p, &s));
        assert!(Step::Choose.reachable(&p, &s), "the remedy is worth showing");
        assert!(!Step::Apply.reachable(&p, &s), "there is nothing to apply");
        assert!(!Step::Confirm.reachable(&p, &s));
    }

    #[test]
    fn confirm_opens_only_once_the_run_has_settled() {
        let p = plan(&Artifact::Patches { count: 2 }, &nothing_present);
        let mut s = Session::new(vec!["one".into(), "two".into()]);
        assert!(!Step::Confirm.reachable(&p, &s), "nothing has happened yet");
        s.start();
        assert!(!Step::Confirm.reachable(&p, &s), "still running");
        s.advance();
        s.advance();
        assert!(Step::Confirm.reachable(&p, &s));

        // An abort also settles it — the operator still has to be told, and
        // "aborted" is a result rather than an absence of one.
        let mut s = Session::new(vec!["one".into()]);
        s.start();
        s.conflict(vec!["x".into()]);
        s.resolve(Resolution::Abort);
        assert!(Step::Confirm.reachable(&p, &s));
    }

    #[test]
    fn the_steps_are_the_same_on_a_phone_and_a_desktop() {
        // Only how many are on screen differs. If the step SET differed, a
        // decision made on a phone and one made on a desktop could diverge,
        // and the wizard would be a second implementation of the dashboard.
        assert_eq!(Step::ALL.len(), 4);
        for s in Step::ALL {
            assert!(!s.title().is_empty());
        }
    }
}

#[cfg(test)]
mod real_bundle_tests {
    use super::*;

    /// `bundle_refs` against a bundle this repo can actually produce.
    ///
    /// The parser reads `git bundle verify`, whose output format is the kind of
    /// thing that drifts between git versions — refs on stdout in some, stderr
    /// in others — so it is worth exercising against the real command rather
    /// than a fixture string.
    #[test]
    fn a_real_bundle_reports_its_refs() {
        let dir = std::env::temp_dir().join(format!("incoming-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git").args(args).current_dir(&dir).output()
        };
        if git(&["init", "-q", "."]).is_err() {
            return; // no git here; nothing to assert
        }
        let _ = git(&["config", "user.email", "t@t"]);
        let _ = git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let _ = git(&["add", "-A"]);
        let _ = git(&["commit", "-q", "-m", "one"]);
        let bundle = dir.join("all.bundle");
        let _ = git(&["bundle", "create", bundle.to_str().unwrap(), "--all"]);
        if !bundle.exists() {
            return;
        }

        let (refs, requires) = crate::ops::bundle_refs(bundle.to_str().unwrap());
        assert!(!refs.is_empty(), "a --all bundle must report a ref");
        assert!(requires.is_empty(), "a complete bundle needs no prerequisite");

        // And the plan for it is a plain fetch, because nothing is missing.
        let a = Artifact::Bundle { refs, requires };
        let p = plan(&a, &|_| true);
        assert_eq!(p.route, Route::Fetch);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_commit_refuses_a_sha_that_is_not_a_commit() {
        // rev-parse would say yes to things git cannot read, and a prerequisite
        // check that passes for an unreadable commit defeats the whole flow.
        assert!(!crate::ops::has_commit(""));
        assert!(!crate::ops::has_commit("0000000000000000000000000000000000000000"));
        assert!(!crate::ops::has_commit("not-a-sha"));
    }
}

// ── driving a real `git am -3` ───────────────────────────────────────────────

use std::path::Path;

fn git(repo: &Path, args: &[&str]) -> (bool, String, String) {
    match std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("PATH", crate::ops::tool_path_env())
        // `am` opens an editor on some configurations; a GUI has none, and a
        // blocked editor looks exactly like a hung apply.
        .env("GIT_EDITOR", "true")
        .output()
    {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
            String::from_utf8_lossy(&o.stderr).trim().to_string(),
        ),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

/// Files with conflict markers right now.
pub fn conflicted_paths(repo: &Path) -> Vec<String> {
    let (_, out, _) = git(repo, &["diff", "--name-only", "--diff-filter=U"]);
    out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Files git still calls unmerged that STILL CONTAIN conflict markers.
///
/// The distinction from [`conflicted_paths`] is the whole guard: that reports
/// the index, which only staging clears, while this reports the bytes, which
/// is what the operator actually fixes.
pub fn unresolved_markers(repo: &Path) -> Vec<String> {
    conflicted_paths(repo)
        .into_iter()
        .filter(|p| {
            std::fs::read_to_string(repo.join(p))
                .map(|s| s.contains("<<<<<<<") || s.contains(">>>>>>>"))
                .unwrap_or(false)
        })
        .collect()
}

/// Is an `am` already in progress?
///
/// Checked before starting rather than after failing: `git am` refuses to
/// begin while a previous one is unfinished, and its message ("previous
/// rebase directory ... still exists") reads as a bug in this tool rather than
/// as a state the operator can resolve.
pub fn am_in_progress(repo: &Path) -> bool {
    repo.join(".git/rebase-apply").exists()
}

/// Start applying a series with three-way merge.
///
/// `-3` is not optional here. Without it `am` needs the exact blobs the patch
/// was written against and fails on anything that has moved; with it, the
/// patch applies by content — which is the entire reason a patch series
/// survives a history rewrite when a bundle does not.
pub fn am_start(repo: &Path, patches: &[String], s: &mut Session) -> Apply {
    if am_in_progress(repo) {
        s.state = Apply::Aborted {
            at: 0,
            reason: "an apply is already in progress — finish or abort it first".into(),
        };
        return s.state.clone();
    }
    if patches.is_empty() {
        s.state = Apply::Done { applied: 0 };
        return s.state.clone();
    }
    s.start();
    let mut args: Vec<&str> = vec!["am", "-3", "--keep-non-patch"];
    args.extend(patches.iter().map(String::as_str));
    let (ok, _, err) = git(repo, &args);
    settle(repo, s, ok, &err)
}

/// Continue, skip or abort, then report where the run ended up.
pub fn am_resolve(repo: &Path, s: &mut Session, r: Resolution) -> Apply {
    let verb = match r {
        Resolution::Continue => "--continue",
        Resolution::Skip => "--skip",
        Resolution::Abort => "--abort",
    };
    if r == Resolution::Continue {
        // Refused on MARKERS IN THE CONTENT, not on an unmerged index.
        //
        // The first version checked `diff --diff-filter=U`, which reports the
        // index — and editing a file does not clear that, staging does. So a
        // correctly resolved conflict was refused forever: the operator fixes
        // the file, presses continue, and is told the file is still
        // conflicted. Caught by the test that resolves and continues.
        //
        // What must actually be prevented is committing markers, and that is a
        // question about the bytes. `am --continue` with `<<<<<<<` in a file
        // commits it, which is the one irreversible mistake available here.
        let unresolved = unresolved_markers(repo);
        if !unresolved.is_empty() {
            if let Apply::Conflicted { at, total, .. } = s.state {
                s.state = Apply::Conflicted { at, total, paths: unresolved };
            }
            return s.state.clone();
        }
    }
    if r == Resolution::Continue {
        // Everything resolved has to be staged, or `am` reports an empty patch
        // and stops again on the same one.
        let _ = git(repo, &["add", "-A"]);
    }
    let (ok, _, err) = git(repo, &[ "am", verb ]);
    if r == Resolution::Abort {
        s.state = Apply::Aborted { at: applied_so_far(repo, s), reason:
            "aborted; the tree is back where it started".into() };
        return s.state.clone();
    }
    settle(repo, s, ok, &err)
}

fn applied_so_far(_repo: &Path, s: &Session) -> usize {
    match s.state {
        Apply::Running { at, .. } | Apply::Conflicted { at, .. } => at,
        _ => 0,
    }
}

/// Read the outcome from the repo rather than from the exit code.
///
/// `git am` exits non-zero both for a conflict the operator can fix and for a
/// refusal they cannot, and the difference is whether an apply is still in
/// progress. Trusting the exit code alone reports a resolvable conflict as a
/// failure.
fn settle(repo: &Path, s: &mut Session, ok: bool, err: &str) -> Apply {
    if ok && !am_in_progress(repo) {
        s.state = Apply::Done { applied: s.total };
        return s.state.clone();
    }
    if am_in_progress(repo) {
        let paths = conflicted_paths(repo);
        let at = current_patch_index(repo).unwrap_or_else(|| applied_so_far(repo, s));
        s.state = Apply::Conflicted { at, total: s.total, paths };
        return s.state.clone();
    }
    s.state = Apply::Aborted {
        at: applied_so_far(repo, s),
        reason: if err.is_empty() { "apply failed".into() } else { err.to_string() },
    };
    s.state.clone()
}

/// Which patch `am` is holding, from its own bookkeeping.
///
/// `.git/rebase-apply/next` is 1-based and counts the patch being attempted.
fn current_patch_index(repo: &Path) -> Option<usize> {
    let n = std::fs::read_to_string(repo.join(".git/rebase-apply/next")).ok()?;
    n.trim().parse::<usize>().ok().map(|v| v.saturating_sub(1))
}

#[cfg(test)]
mod real_am_tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a repo, a patch series from a divergent branch, and return both.
    ///
    /// The scenario is the one this module exists for: patches authored against
    /// a base the target no longer has at its tip, which is what a history
    /// rewrite leaves behind.
    fn scenario(name: &str, conflicting: bool) -> Option<(PathBuf, Vec<String>)> {
        let dir = std::env::temp_dir().join(format!("am-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args).current_dir(&dir)
                .env("GIT_EDITOR", "true")
                .output().ok()
        };
        g(&["init", "-q", "-b", "main", "."])?;
        g(&["config", "user.email", "t@t"])?;
        g(&["config", "user.name", "t"])?;
        std::fs::write(dir.join("f.txt"), "base\n").ok()?;
        g(&["add", "-A"])?;
        g(&["commit", "-q", "-m", "base"])?;

        // A feature branch with two commits, exported as patches.
        g(&["checkout", "-q", "-b", "feature"])?;
        std::fs::write(dir.join("f.txt"), "base\nfeature one\n").ok()?;
        g(&["commit", "-q", "-am", "feature one"])?;
        std::fs::write(dir.join("g.txt"), "second file\n").ok()?;
        g(&["add", "-A"])?;
        g(&["commit", "-q", "-m", "feature two"])?;
        let pdir = dir.join("patches");
        std::fs::create_dir_all(&pdir).ok()?;
        g(&["format-patch", "-o", pdir.to_str()?, "main..feature"])?;

        // main moves on. With `conflicting`, it touches the same line the first
        // patch does, so `am -3` must stop.
        g(&["checkout", "-q", "main"])?;
        if conflicting {
            std::fs::write(dir.join("f.txt"), "base\nmain went its own way\n").ok()?;
        } else {
            std::fs::write(dir.join("unrelated.txt"), "elsewhere\n").ok()?;
            g(&["add", "-A"])?;
        }
        g(&["commit", "-q", "-am", "main moves"])?;
        g(&["branch", "-q", "-D", "feature"])?;

        let mut patches: Vec<String> = std::fs::read_dir(&pdir).ok()?
            .flatten()
            .map(|e| e.path().to_string_lossy().to_string())
            .filter(|p| p.ends_with(".patch"))
            .collect();
        patches.sort();
        Some((dir, patches))
    }

    #[test]
    fn a_clean_series_applies_onto_a_moved_head() {
        // The property that makes patches the right re-send: the base they were
        // written against is no longer HEAD, and they still apply.
        let Some((dir, patches)) = scenario("clean", false) else { return };
        assert_eq!(patches.len(), 2, "two patches expected");
        let mut s = Session::new(vec!["feature one".into(), "feature two".into()]);
        let st = am_start(&dir, &patches, &mut s);
        assert_eq!(st, Apply::Done { applied: 2 }, "{}", s.status_line());
        assert!(!am_in_progress(&dir));
        // And the work really landed.
        let log = std::process::Command::new("git")
            .args(["log", "--oneline", "-3", "--format=%s"])
            .current_dir(&dir).output().unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(log.contains("feature two") && log.contains("feature one"), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conflict_stops_and_reports_the_file_and_the_patch() {
        let Some((dir, patches)) = scenario("conflict", true) else { return };
        let mut s = Session::new(vec!["feature one".into(), "feature two".into()]);
        let st = am_start(&dir, &patches, &mut s);
        match st {
            Apply::Conflicted { at, total, ref paths } => {
                assert_eq!(at, 0, "the FIRST patch is the one that conflicts");
                assert_eq!(total, 2);
                assert!(paths.iter().any(|p| p.contains("f.txt")), "{paths:?}");
                assert_eq!(s.current_subject(), Some("feature one"));
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert!(am_in_progress(&dir), "am should still be holding the series");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn continue_is_refused_while_markers_remain() {
        // `am --continue` with markers in the tree COMMITS them. That is the
        // one irreversible mistake available here, so it must be refused rather
        // than attempted.
        let Some((dir, patches)) = scenario("markers", true) else { return };
        let mut s = Session::new(vec!["feature one".into(), "feature two".into()]);
        am_start(&dir, &patches, &mut s);
        assert!(matches!(s.state, Apply::Conflicted { .. }));

        let st = am_resolve(&dir, &mut s, Resolution::Continue);
        assert!(matches!(st, Apply::Conflicted { .. }),
                "continue went ahead with conflicts unresolved: {st:?}");
        // Nothing was committed.
        let body = std::fs::read_to_string(dir.join("f.txt")).unwrap();
        assert!(body.contains("<<<<<<<"), "the markers should still be there");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolving_the_conflict_then_continuing_finishes_the_series() {
        let Some((dir, patches)) = scenario("resolve", true) else { return };
        let mut s = Session::new(vec!["feature one".into(), "feature two".into()]);
        am_start(&dir, &patches, &mut s);
        // The operator fixes the file.
        std::fs::write(dir.join("f.txt"), "base\nboth ways\n").unwrap();
        let st = am_resolve(&dir, &mut s, Resolution::Continue);
        assert_eq!(st, Apply::Done { applied: 2 }, "{}", s.status_line());
        assert!(!am_in_progress(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abort_puts_the_tree_back() {
        let Some((dir, patches)) = scenario("abort", true) else { return };
        let before = std::fs::read_to_string(dir.join("f.txt")).unwrap();
        let mut s = Session::new(vec!["feature one".into(), "feature two".into()]);
        am_start(&dir, &patches, &mut s);
        let st = am_resolve(&dir, &mut s, Resolution::Abort);
        assert!(matches!(st, Apply::Aborted { .. }));
        assert!(!am_in_progress(&dir));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), before,
                   "abort left the working tree changed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skipping_the_conflicting_patch_applies_the_rest() {
        let Some((dir, patches)) = scenario("skip", true) else { return };
        let mut s = Session::new(vec!["feature one".into(), "feature two".into()]);
        am_start(&dir, &patches, &mut s);
        let st = am_resolve(&dir, &mut s, Resolution::Skip);
        assert_eq!(st, Apply::Done { applied: 2 }, "{}", s.status_line());
        // The second patch's file is there; the first patch's change is not.
        assert!(dir.join("g.txt").exists(), "the skipped-past patch did not apply");
        let f = std::fs::read_to_string(dir.join("f.txt")).unwrap();
        assert!(!f.contains("feature one"), "the skipped patch was applied anyway");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_start_while_one_is_in_progress_is_refused_with_a_reason() {
        // git am's own message ("previous rebase directory still exists") reads
        // as a bug in this tool rather than a state the operator can fix.
        let Some((dir, patches)) = scenario("busy", true) else { return };
        let mut s = Session::new(vec!["one".into(), "two".into()]);
        am_start(&dir, &patches, &mut s);
        assert!(am_in_progress(&dir));

        let mut s2 = Session::new(vec!["one".into(), "two".into()]);
        let st = am_start(&dir, &patches, &mut s2);
        match st {
            Apply::Aborted { ref reason, .. } => {
                assert!(reason.contains("already in progress"), "{reason}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
