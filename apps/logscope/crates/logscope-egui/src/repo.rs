//! repo — logscope's build-aware view of the monorepo (shared with gitmind via `repo_map`). Lets
//! logscope's sandbox + autonomous loop resolve *any* crate to the correct build/test argv, so it
//! can work on this repository — pick a crate, build it, run its tests, read the verdict — instead
//! of only the workspace it happens to sit in.
use repo_map::{scan_repo, CrateEntry};
use std::path::Path;

/// Every crate in the repo, build-aware.
pub fn crates(repo_root: &str) -> Vec<CrateEntry> {
    scan_repo(Path::new(repo_root), &["apps", "shared", "games"])
}

/// Resolve a crate by name to the argv that tests it (what the sandbox runs).
pub fn test_argv_for(repo_root: &str, name: &str) -> Option<Vec<String>> {
    crates(repo_root).into_iter().find(|c| c.name == name).map(|c| c.test_argv())
}

/// Build+test one crate for real and return the report — logscope acting on the repo, using the
/// same shared runner gitmind's cockpit uses (no second implementation).
pub fn verify_crate(repo_root: &str, name: &str) -> repo_map::matrix::MatrixReport {
    let opts = repo_map::matrix::RunOpts { test: true, include_cmake: false, only: Some(vec![name.to_string()]) };
    repo_map::matrix::run_repo_matrix(repo_root, &["apps", "shared", "games"], &opts)
}

/// Build+test the whole repo.
pub fn verify_repo(repo_root: &str, opts: &repo_map::matrix::RunOpts) -> repo_map::matrix::MatrixReport {
    repo_map::matrix::run_repo_matrix(repo_root, &["apps", "shared", "games"], opts)
}

/// The autonomy step gitmind's cockpit doesn't have: feed a matrix verdict into logscope's ship
/// gate. An autonomous run that touched `files_touched` files verifies the affected crates, then
/// this decides ship / hold-for-review / block. `all_passed` = the whole matrix is green;
/// `complete` = something actually ran.
pub fn ship_from_matrix(
    report: &repo_map::matrix::MatrixReport,
    guardrails: &logscope::guardrails::Guardrails,
    agreement: f32,
    files_touched: usize,
) -> logscope::guardrails::ShipDecision {
    guardrails.ship_decision(report.all_green(), !report.results.is_empty(), agreement, files_touched)
}

#[cfg(test)]
mod ship_tests {
    use super::ship_from_matrix;
    use logscope::guardrails::{Guardrails, ShipDecision};
    use repo_map::matrix::{CrateResult, MatrixReport, Outcome};

    fn report(outcomes: &[(&str, Outcome)]) -> MatrixReport {
        MatrixReport {
            results: outcomes.iter().map(|(n, o)| CrateResult { name: (*n).into(), outcome: o.clone(), detail: String::new() }).collect(),
        }
    }

    #[test]
    fn green_matrix_ships_under_autonomous_but_holds_when_supervised() {
        let green = report(&[("a", Outcome::Ok), ("b", Outcome::Ok)]);
        // autonomous + all green + agreement above floor + within file cap → Ship
        assert_eq!(ship_from_matrix(&green, &Guardrails::autonomous(), 0.9, 5), ShipDecision::Ship);
        // same green result, but training wheels (supervised) → held for a human
        assert!(matches!(ship_from_matrix(&green, &Guardrails::training_wheels(), 0.9, 5), ShipDecision::HoldForReview(_)));
    }

    #[test]
    fn a_failing_crate_holds_even_when_autonomous() {
        let failing = report(&[("a", Outcome::Ok), ("b", Outcome::TestFail)]);
        assert!(matches!(ship_from_matrix(&failing, &Guardrails::autonomous(), 0.9, 5), ShipDecision::HoldForReview(_)),
            "the repo isn't green → the loop must not auto-ship");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors().nth(4).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn logscope_resolves_any_crate_to_a_test_command() {
        // its own crate (a workspace member) and a standalone game
        let mine = test_argv_for(&root(), "logscope-egui").expect("finds logscope-egui");
        assert!(mine.contains(&"-p".to_string()) && mine.contains(&"logscope-egui".to_string()));
        let game = test_argv_for(&root(), "bsgames").expect("finds bsgames");
        assert_eq!(game, vec!["cargo", "test", "--manifest-path", "games/bsgames/Cargo.toml"]);
    }
}
