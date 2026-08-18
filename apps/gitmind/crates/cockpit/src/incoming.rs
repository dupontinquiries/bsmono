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
