//! matrix — the first real self-management loop: build + test every crate in the repo and report
//! which fail. Orchestration is pure (injected command runner, so it's tested without spawning
//! 150 cargo processes); [`spawn_run`] is the real executor.
use repo_map::{CrateEntry, CrateKind};

/// The result of running one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutcome {
    pub success: bool,
    /// Last lines of combined output — enough to see why it failed.
    pub tail: String,
}

/// What happened to one crate in the matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    BuildFail,
    TestFail,
    Skipped(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateResult {
    pub name: String,
    pub outcome: Outcome,
    pub detail: String,
}

/// Options for a matrix run.
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// Build+test (true) or build-only (false).
    pub test: bool,
    /// Run cmake crates too (needs a C++ toolchain + a configured build dir).
    pub include_cmake: bool,
    /// Restrict to these crate names (None = all).
    pub only: Option<Vec<String>>,
}
impl Default for RunOpts {
    fn default() -> Self {
        RunOpts { test: true, include_cmake: false, only: None }
    }
}

/// The aggregated report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixReport {
    pub results: Vec<CrateResult>,
}

impl MatrixReport {
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.outcome == Outcome::Ok).count()
    }
    pub fn skipped(&self) -> usize {
        self.results.iter().filter(|r| matches!(r.outcome, Outcome::Skipped(_))).count()
    }
    /// The crates that need attention (build or test failures), in order.
    pub fn failures(&self) -> Vec<&CrateResult> {
        self.results.iter().filter(|r| matches!(r.outcome, Outcome::BuildFail | Outcome::TestFail)).collect()
    }
    pub fn failed(&self) -> usize {
        self.failures().len()
    }
    pub fn all_green(&self) -> bool {
        self.failed() == 0
    }
    /// One-line status.
    pub fn summary(&self) -> String {
        let (mut bf, mut tf) = (0, 0);
        for r in &self.results {
            match r.outcome {
                Outcome::BuildFail => bf += 1,
                Outcome::TestFail => tf += 1,
                _ => {}
            }
        }
        format!(
            "{} crates: {} ok, {} build-fail, {} test-fail, {} skipped",
            self.results.len(), self.passed(), bf, tf, self.skipped()
        )
    }
}

/// Run the matrix with an injected command runner. Build first; only test if the build passed.
pub fn run_matrix<R: FnMut(&[String]) -> CmdOutcome>(
    crates: Vec<CrateEntry>,
    mut run: R,
    opts: &RunOpts,
) -> MatrixReport {
    let mut results = Vec::new();
    for c in crates {
        if let Some(only) = &opts.only {
            if !only.contains(&c.name) {
                continue;
            }
        }
        if matches!(c.kind, CrateKind::Cmake) && !opts.include_cmake {
            results.push(CrateResult {
                name: c.name,
                outcome: Outcome::Skipped("cmake (needs C++ toolchain)".into()),
                detail: String::new(),
            });
            continue;
        }
        let build = run(&c.build_argv());
        if !build.success {
            results.push(CrateResult { name: c.name, outcome: Outcome::BuildFail, detail: build.tail });
            continue;
        }
        if !opts.test {
            results.push(CrateResult { name: c.name, outcome: Outcome::Ok, detail: String::new() });
            continue;
        }
        let test = run(&c.test_argv());
        if test.success {
            results.push(CrateResult { name: c.name, outcome: Outcome::Ok, detail: String::new() });
        } else {
            results.push(CrateResult { name: c.name, outcome: Outcome::TestFail, detail: test.tail });
        }
    }
    MatrixReport { results }
}

/// The real executor: spawn a command from `repo_root`, capture success + a tail of output. The
/// crate argvs use `--manifest-path`, so running from the repo root is correct for every kind.
pub fn spawn_run(argv: &[String], repo_root: &str) -> CmdOutcome {
    if argv.is_empty() {
        return CmdOutcome { success: false, tail: "empty argv".into() };
    }
    match std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(repo_root).output() {
        Ok(o) => {
            let mut combined = String::from_utf8_lossy(&o.stdout).into_owned();
            combined.push_str(&String::from_utf8_lossy(&o.stderr));
            CmdOutcome { success: o.status.success(), tail: tail_lines(&combined, 20) }
        }
        Err(e) => CmdOutcome { success: false, tail: format!("spawn failed: {e}") },
    }
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Convenience: index the repo and run the whole matrix for real.
pub fn run_repo_matrix(repo_root: &str, opts: &RunOpts) -> MatrixReport {
    let crates = crate::repomap::indexed_crates(repo_root);
    run_matrix(crates, |argv| spawn_run(argv, repo_root), opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_map::CrateKind;

    fn member(name: &str) -> CrateEntry {
        CrateEntry { name: name.into(), path: format!("shared/{name}"), kind: CrateKind::WorkspaceMember { workspace_root: "".into() } }
    }
    fn cmake(name: &str) -> CrateEntry {
        CrateEntry { name: name.into(), path: format!("games/{name}"), kind: CrateKind::Cmake }
    }

    #[test]
    fn aggregates_ok_buildfail_testfail_and_skips_cmake() {
        let crates = vec![member("good"), member("wontbuild"), member("failstests"), cmake("blackstar")];
        // fake runner: 'wontbuild' fails its build; 'failstests' builds but fails its test; cmake skipped.
        let report = run_matrix(crates, |argv| {
            let is_test = argv.contains(&"test".to_string());
            let names_wontbuild = argv.iter().any(|a| a == "wontbuild");
            let names_failstests = argv.iter().any(|a| a == "failstests");
            if names_wontbuild {
                CmdOutcome { success: false, tail: "error[E0432]: unresolved import".into() }
            } else if names_failstests && is_test {
                CmdOutcome { success: false, tail: "test result: FAILED. 1 failed".into() }
            } else {
                CmdOutcome { success: true, tail: String::new() }
            }
        }, &RunOpts::default());

        assert_eq!(report.passed(), 1, "only 'good' is fully green");
        assert_eq!(report.failed(), 2);
        assert_eq!(report.skipped(), 1);
        assert!(!report.all_green());
        let fails: Vec<&str> = report.failures().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(fails, vec!["wontbuild", "failstests"]);
        // build failure short-circuits before the test runs; test failure carries the reason
        assert_eq!(report.failures()[0].outcome, Outcome::BuildFail);
        assert!(report.failures()[0].detail.contains("E0432"));
        assert_eq!(report.failures()[1].outcome, Outcome::TestFail);
        assert_eq!(report.summary(), "4 crates: 1 ok, 1 build-fail, 1 test-fail, 1 skipped");
    }

    #[test]
    fn only_filter_and_build_only_mode() {
        let crates = vec![member("a"), member("b"), member("c")];
        let calls = std::cell::RefCell::new(Vec::new());
        let opts = RunOpts { test: false, include_cmake: false, only: Some(vec!["b".into()]) };
        let report = run_matrix(crates, |argv| {
            calls.borrow_mut().push(argv.to_vec());
            CmdOutcome { success: true, tail: String::new() }
        }, &opts);
        assert_eq!(report.results.len(), 1, "only 'b' ran");
        assert_eq!(report.passed(), 1);
        // build-only → no 'test' command was issued
        assert!(calls.borrow().iter().all(|argv| !argv.contains(&"test".to_string())));
    }
}
