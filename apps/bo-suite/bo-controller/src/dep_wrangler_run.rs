// src/dep_wrangler_run.rs
#![allow(dead_code)]
//
// Runtime orchestrator for dep_wrangler.
//
// Where dep_wrangler.rs is pure logic (build a plan, render it as
// text, predict packages per distro), THIS module is the part that
// actually shells out:
//
//   * Refresh the package index (apt update, apk update, etc).
//   * Run each prerequisite command in order, bailing on the first
//     failure.
//   * Run the install command.
//   * Verify the binary appeared.
//   * If verification failed, move to the next step in the plan.
//   * Record every command, its exit code, and its stderr in a
//     WrangleResult for audit.
//
// The runtime is callable from two places:
//   1. The QML "Audit + Install" button on SettingsPage. The user
//      sees the plan, confirms, watches each step run.
//   2. The bash install-deps.sh script, which calls this via the
//      `bo-controller --wrangle <tool>` CLI subcommand.
//
// Safety guard: this module never runs commands as root by *itself*.
// It assumes sudo is configured (the user has already authorized
// the install action via the script or UI). When sudo isn't available,
// the commands fail loudly rather than silently doing nothing.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::dep_wrangler::{
    InstallStep, PackageManager,
    StepLog, StepOutcome, WrangleResult,
    plan_for_tool, verify_installed,
};

/// Cap on how long we let any single command run before declaring it
/// stuck. apk/apt/dnf rarely take more than 60-90 s; an unbounded hang
/// usually means a stalled mirror or a pkexec prompt waiting forever.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(180);

/// Refresh the PM's index before any install. Idempotent and quick.
/// Logged into the WrangleResult so a "stale mirror" failure shows
/// up as a distinct line rather than blamed on the actual install.
pub fn refresh_index(pm: PackageManager) -> StepOutcome {
    let base = pm.base_cmd();
    let refresh = pm.refresh_args();
    if base.is_empty() || refresh.is_empty() { return StepOutcome::Skipped; }
    let mut argv: Vec<String> = base.into_iter().map(String::from).collect();
    argv.extend(refresh.into_iter().map(String::from));
    run_command(&argv)
}

/// Execute one step. Runs prerequisites first; bails on the first
/// prereq failure; if all prereqs pass, runs the main command;
/// finally re-verifies the binary.
pub fn run_step(step: &InstallStep, target_binary: &str) -> StepOutcome {
    // Prereqs (e.g. "enable RPMFusion repo", "install software-properties-common")
    for prereq in &step.prerequisites {
        let outcome = run_command(prereq);
        if !matches!(outcome, StepOutcome::Success) {
            // If a prereq is just refreshing state (and the desired binary
            // is now present anyway), treat it as a no-op success.
            if verify_installed(target_binary) {
                return StepOutcome::Success;
            }
            return outcome;
        }
    }
    let outcome = run_command(&step.command);
    if matches!(outcome, StepOutcome::Success) {
        if verify_installed(target_binary) {
            return StepOutcome::Success;
        }
        return StepOutcome::InstalledButNotVisible;
    }
    outcome
}

/// Top-level: run the whole plan for a tool, return an audit result.
/// Short-circuits on the first successful step (with positive
/// verification). Refreshes the index once at the start.
pub fn wrangle(binary: &str, friendly_name: &str, pm: PackageManager) -> WrangleResult {
    let initial = verify_installed(binary);
    if initial {
        return WrangleResult {
            binary: binary.into(),
            initial_state: true,
            final_state: true,
            steps: vec![],
        };
    }
    // Refresh once before the plan runs. Failures here aren't fatal —
    // the install commands may still succeed against a slightly stale
    // index. We log the refresh attempt anyway so the user can see it.
    let mut steps: Vec<StepLog> = Vec::new();
    let refresh_outcome = refresh_index(pm);
    let refresh_step = InstallStep {
        repo: super::dep_wrangler::Repository::Default,
        package: "(refresh index)".into(),
        description: "refresh package list".into(),
        command: {
            let mut v: Vec<String> = pm.base_cmd().into_iter().map(String::from).collect();
            v.extend(pm.refresh_args().into_iter().map(String::from));
            v
        },
        prerequisites: vec![],
    };
    steps.push(StepLog { step: refresh_step, outcome: refresh_outcome });

    let plan = plan_for_tool(binary, friendly_name, pm);
    let mut final_state = false;
    for step in &plan.steps {
        let outcome = run_step(step, binary);
        let is_success = matches!(outcome, StepOutcome::Success);
        steps.push(StepLog { step: step.clone(), outcome });
        if is_success { final_state = true; break; }
    }
    WrangleResult {
        binary: binary.into(),
        initial_state: false,
        final_state,
        steps,
    }
}

/// Execute a shell command argv-style with a timeout. Returns Success
/// on exit 0, CommandFailed otherwise. Captures stderr (truncated to
/// 2 KB) for the audit trail.
fn run_command(argv: &[String]) -> StepOutcome {
    if argv.is_empty() {
        return StepOutcome::CommandFailed {
            exit_code: None,
            stderr: "empty command".into(),
        };
    }
    eprintln!("dep_wrangler: running: {}", argv.join(" "));
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return StepOutcome::CommandFailed {
            exit_code: None,
            stderr: format!("spawn failed: {}", e),
        },
    };

    // Timed wait loop. std::process doesn't have wait-with-timeout
    // built in, so we poll. Sleep is exponentially backed off from
    // 50 ms up to 1 s to keep the loop tight at first.
    let start = Instant::now();
    let mut sleep_ms = 50;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code();
                let mut stderr_buf = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let mut tmp = [0u8; 2048];
                    if let Ok(n) = e.read(&mut tmp) {
                        stderr_buf = String::from_utf8_lossy(&tmp[..n]).to_string();
                    }
                }
                if status.success() { return StepOutcome::Success; }
                return StepOutcome::CommandFailed {
                    exit_code: code,
                    stderr: stderr_buf,
                };
            }
            Ok(None) => {
                if start.elapsed() > COMMAND_TIMEOUT {
                    let _ = child.kill();
                    return StepOutcome::CommandFailed {
                        exit_code: None,
                        stderr: format!("timeout after {}s", COMMAND_TIMEOUT.as_secs()),
                    };
                }
                std::thread::sleep(Duration::from_millis(sleep_ms));
                sleep_ms = (sleep_ms * 2).min(1000);
            }
            Err(e) => return StepOutcome::CommandFailed {
                exit_code: None,
                stderr: format!("wait failed: {}", e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dep_wrangler::{InstallStep, Repository};

    #[test]
    fn run_command_with_true_succeeds() {
        let argv = vec!["true".to_string()];
        let outcome = run_command(&argv);
        assert!(matches!(outcome, StepOutcome::Success));
    }

    #[test]
    fn run_command_with_false_fails() {
        let argv = vec!["false".to_string()];
        let outcome = run_command(&argv);
        assert!(matches!(outcome, StepOutcome::CommandFailed { .. }));
    }

    #[test]
    fn run_command_empty_returns_error() {
        let outcome = run_command(&[]);
        match outcome {
            StepOutcome::CommandFailed { stderr, .. } => {
                assert!(stderr.contains("empty"));
            }
            _ => panic!("expected CommandFailed"),
        }
    }

    #[test]
    fn run_command_captures_stderr_on_failure() {
        // `sh -c "echo X >&2; exit 1"` — emits to stderr then fails.
        let argv = vec!["sh".to_string(), "-c".to_string(), "echo X 1>&2; exit 1".to_string()];
        let outcome = run_command(&argv);
        match outcome {
            StepOutcome::CommandFailed { exit_code, stderr } => {
                assert_eq!(exit_code, Some(1));
                assert!(stderr.contains("X"));
            }
            _ => panic!("expected CommandFailed"),
        }
    }

    #[test]
    fn run_command_with_nonexistent_binary_returns_error() {
        let argv = vec!["bogus-binary-xyzzy".to_string()];
        let outcome = run_command(&argv);
        assert!(matches!(outcome, StepOutcome::CommandFailed { .. }));
    }

    #[test]
    fn refresh_index_unknown_pm_skips() {
        let outcome = refresh_index(PackageManager::Unknown);
        assert!(matches!(outcome, StepOutcome::Skipped));
    }

    #[test]
    fn wrangle_returns_initial_state_when_present() {
        // `true` is on every Linux system.
        let result = wrangle("true", "Test", PackageManager::Apt);
        assert!(result.initial_state, "/bin/true should be present");
        assert!(result.succeeded());
        assert!(!result.changed_state());
    }

    #[test]
    fn run_step_runs_prereqs_first() {
        let step = InstallStep {
            repo: Repository::Default,
            package: "test".into(),
            description: "d".into(),
            command: vec!["true".into()],
            prerequisites: vec![
                vec!["true".into()],
                vec!["true".into()],
            ],
        };
        // Target: a binary that exists (so verification succeeds).
        let outcome = run_step(&step, "true");
        assert!(matches!(outcome, StepOutcome::Success));
    }

    #[test]
    fn run_step_bails_on_failed_prereq() {
        let step = InstallStep {
            repo: Repository::Default,
            package: "test".into(),
            description: "d".into(),
            command: vec!["true".into()],
            prerequisites: vec![vec!["false".into()]],
        };
        // Target binary doesn't exist; the failed prereq should
        // produce a CommandFailed outcome.
        let outcome = run_step(&step, "nonexistent-binary-xyz");
        assert!(matches!(outcome, StepOutcome::CommandFailed { .. }));
    }

    #[test]
    fn run_step_succeeds_if_target_appears_after_prereq() {
        // Prereq is allowed to be a no-op refresh; if the binary is
        // already present, the step succeeds even if the prereq does.
        let step = InstallStep {
            repo: Repository::Default,
            package: "test".into(),
            description: "d".into(),
            command: vec!["true".into()],
            prerequisites: vec![vec!["false".into()]],   // would fail
        };
        // But the target ("true") IS present so we treat this as success.
        let outcome = run_step(&step, "true");
        assert!(matches!(outcome, StepOutcome::Success));
    }

    #[test]
    fn step_outcome_audit_distinguishes_failure_types() {
        let log_success = StepLog {
            step: InstallStep {
                repo: Repository::Default, package: "x".into(),
                description: "d".into(), command: vec!["true".into()],
                prerequisites: vec![],
            },
            outcome: StepOutcome::Success,
        };
        let log_fail = StepLog {
            step: log_success.step.clone(),
            outcome: StepOutcome::CommandFailed { exit_code: Some(1), stderr: "bad".into() },
        };
        let log_invisible = StepLog {
            step: log_success.step.clone(),
            outcome: StepOutcome::InstalledButNotVisible,
        };
        let log_skipped = StepLog {
            step: log_success.step.clone(),
            outcome: StepOutcome::Skipped,
        };
        let r = WrangleResult {
            binary: "x".into(),
            initial_state: false,
            final_state: true,
            steps: vec![log_success, log_fail, log_invisible, log_skipped],
        };
        let lines = r.audit_lines();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("OK"));
        assert!(lines[1].contains("FAIL"));
        assert!(lines[2].contains("not visible"));
        assert!(lines[3].contains("skipped"));
    }
}
