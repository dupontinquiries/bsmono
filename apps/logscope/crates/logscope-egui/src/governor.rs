//! governor.rs — the single place that ties the isolation modules together.
//!
//! `isolate`, `taint`, `audit_log` and `review_gate` are each independently useful and each
//! independently ignorable. That is the failure mode: five correct modules that no call site
//! consults. This is the seam callers actually touch — hand it a repo, get back a configured agent
//! and a verifier that already knows the branch's posture.
//!
//! DEFAULTS CHOSEN HERE (and why)
//!
//! * The audit log lives at `$XDG_STATE_HOME/logscope/audit-<repo>.log`, OUTSIDE the workspace.
//!   A log inside the tree the agent writes to is a log the agent can rewrite, and this module
//!   refuses such a path rather than quietly accepting it. The alternative — putting it next to the
//!   root-owned git mirror on the codexbox vault — is better on a machine that has one, and is a
//!   one-line override via `with_log`.
//! * The branch is whatever git reports as current. Overridable; there is no useful guess if you
//!   are running several branches at once, in which case make one Governor each.
//! * Thresholds are `review_gate::Thresholds::default()`.
//!
//! WHAT IT DOES NOT DO
//!
//! It does not decide policy per tool call, does not hold capabilities, and does not run anything
//! privileged. It reads state, reports a posture, and narrows a `VerifySpec` before handing it on.

use std::path::{Path, PathBuf};

use crate::review_gate::{Assessment, Posture, Thresholds};

#[derive(Clone, Debug)]
pub struct Governor {
    repo: PathBuf,
    branch: String,
    log: PathBuf,
    thresholds: Thresholds,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GovernorError {
    /// The audit log would live inside the tree the agent can write. Refused: see module docs.
    LogInsideWorkspace { log: PathBuf, repo: PathBuf },
    /// git could not be read.
    Git(String),
}

impl std::fmt::Display for GovernorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GovernorError::LogInsideWorkspace { log, repo } => write!(
                f,
                "refusing to keep the audit log at {} — it is inside the workspace {}, so the agent \
                 could rewrite its own record. Put it on a different filesystem, ideally beside the \
                 root-owned mirror.",
                log.display(),
                repo.display()
            ),
            GovernorError::Git(e) => write!(f, "git: {e}"),
        }
    }
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
                .join(".local/state")
        })
        .join("logscope")
}

/// Short stable tag for a repo path, so two checkouts do not share a log.
fn repo_tag(repo: &Path) -> String {
    let h = crate::audit_log::sha256_hex(repo.to_string_lossy().as_bytes());
    h[..12].to_string()
}

impl Governor {
    /// Default wiring for a repo. Does not touch git yet, so this is cheap and infallible.
    pub fn for_repo(repo: impl Into<PathBuf>) -> Self {
        let repo = repo.into();
        let log = state_dir().join(format!("audit-{}.log", repo_tag(&repo)));
        Self { repo, branch: String::new(), log, thresholds: Thresholds::default() }
    }

    pub fn with_log(mut self, log: impl Into<PathBuf>) -> Self {
        self.log = log.into();
        self
    }
    pub fn with_branch(mut self, b: impl Into<String>) -> Self {
        self.branch = b.into();
        self
    }
    pub fn with_thresholds(mut self, t: Thresholds) -> Self {
        self.thresholds = t;
        self
    }

    pub fn log_path(&self) -> &Path {
        &self.log
    }
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// Reject a log the agent could rewrite. Called by everything that reads or writes it, so this
    /// cannot be skipped by using a different entry point.
    fn check_log_location(&self) -> Result<(), GovernorError> {
        let repo = self.repo.canonicalize().unwrap_or_else(|_| self.repo.clone());
        // The log may not exist yet, so compare against its parent.
        let logdir = self
            .log
            .parent()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("/"));
        if logdir == repo || logdir.starts_with(&repo) {
            return Err(GovernorError::LogInsideWorkspace {
                log: self.log.clone(),
                repo,
            });
        }
        Ok(())
    }

    fn branch_name(&self) -> Result<String, GovernorError> {
        if !self.branch.is_empty() {
            return Ok(self.branch.clone());
        }
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .map_err(|e| GovernorError::Git(e.to_string()))?;
        if !out.status.success() {
            return Err(GovernorError::Git(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Where the branch stands right now.
    pub fn assess(&self) -> Result<Assessment, GovernorError> {
        self.check_log_location()?;
        let branch = self.branch_name()?;
        let changes = crate::review_gate::changes_from_git(&self.repo, &branch)
            .map_err(|e| GovernorError::Git(e.to_string()))?;
        let reviewed = crate::review_gate::last_reviewed(&self.log, &branch).map(|(id, _)| id);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(crate::review_gate::assess(
            &changes,
            reviewed.as_deref(),
            now,
            self.thresholds,
        ))
    }

    /// Adopt the gate on an existing repo: baseline at HEAD so the counter measures work from here
    /// rather than billing you for history you were never going to read.
    pub fn record_baseline(&self) -> Result<String, GovernorError> {
        self.check_log_location()?;
        let branch = self.branch_name()?;
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| GovernorError::Git(e.to_string()))?;
        let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
        crate::review_gate::record_review(&self.log, &branch, &head, "baseline at adoption (not read)")
            .map_err(|e| GovernorError::Git(e.to_string()))?;
        Ok(head)
    }

    pub fn record_review(&self, commit: &str, note: &str) -> Result<(), GovernorError> {
        self.check_log_location()?;
        let branch = self.branch_name()?;
        crate::review_gate::record_review(&self.log, &branch, commit, note)
            .map_err(|e| GovernorError::Git(e.to_string()))
    }

    /// An `AgentConfig` with the tool-channel gate on and anomalies pointed at the audit log.
    pub fn agent_config(&self) -> Result<crate::workspace_exec::AgentConfig, GovernorError> {
        self.check_log_location()?;
        let mut cfg = crate::workspace_exec::AgentConfig::default();
        cfg.audit_log = Some(self.log.clone());
        Ok(cfg)
    }

    /// Narrow a verification request to what the branch's posture permits. Returns the assessment
    /// alongside so the caller can say WHY the run was restricted rather than just failing oddly.
    ///
    /// Split from `verify` so the decision is testable without spawning a compiler.
    pub fn prepare(
        &self, mut spec: crate::sandbox::VerifySpec,
    ) -> Result<(crate::sandbox::VerifySpec, Assessment), GovernorError> {
        let a = self.assess()?;
        crate::review_gate::apply(a.posture, &mut spec);
        Ok((spec, a))
    }

    /// Assess, narrow, run, and record a refusal if policy rejected it.
    pub fn verify(
        &self, spec: crate::sandbox::VerifySpec,
    ) -> Result<(crate::sandbox::VerifyResult, Assessment), GovernorError> {
        let (spec, a) = self.prepare(spec)?;
        let r = crate::sandbox::verify(&spec)
            .map_err(|e| GovernorError::Git(e.to_string()))?;
        if r.output.starts_with("REFUSED:") {
            let _ = crate::audit_log::AuditLog::at(&self.log).append(
                crate::audit_log::Kind::Refusal,
                &spec.workspace.display().to_string(),
                &r.output,
            );
        }
        Ok((r, a))
    }

    /// Whether a new capability may be handed out right now.
    pub fn may_grant(&self) -> bool {
        self.assess().map(|a| crate::review_gate::grants_allowed(a.posture)).unwrap_or(false)
    }

    /// The value to publish out of band so log truncation is detectable. Commit it somewhere the
    /// agent cannot reach — a root-owned mirror is the obvious home.
    pub fn anchor(&self) -> String {
        crate::audit_log::AuditLog::at(&self.log).head()
    }

    /// One line for the UI.
    pub fn status_line(&self) -> String {
        match self.assess() {
            Ok(a) => format!("[{:?}] {}", a.posture, a.explain()),
            Err(e) => format!("[unknown] {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo {
        dir: PathBuf,
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .output()
            .unwrap();
        assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
    }

    fn repo(name: &str, commits: usize, lines_each: usize) -> Repo {
        let d = std::env::temp_dir().join(format!("ls-gov-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q", "-b", "main"]);
        for i in 0..commits {
            let body: String = (0..lines_each).map(|j| format!("line {i}-{j}\n")).collect();
            std::fs::write(d.join(format!("f{i}.txt")), body).unwrap();
            git(&d, &["add", "."]);
            git(&d, &["commit", "-qm", &format!("c{i}")]);
        }
        Repo { dir: d }
    }

    fn logfile(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ls-gov-log-{name}-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn the_default_log_lives_outside_the_workspace() {
        let g = Governor::for_repo("/home/someone/code/proj");
        assert!(
            !g.log_path().starts_with("/home/someone/code/proj"),
            "default log must not be inside the repo: {}",
            g.log_path().display()
        );
    }

    #[test]
    fn two_checkouts_do_not_share_a_log() {
        let a = Governor::for_repo("/home/x/proj-a");
        let b = Governor::for_repo("/home/x/proj-b");
        assert_ne!(a.log_path(), b.log_path());
    }

    #[test]
    fn a_log_inside_the_workspace_is_refused() {
        let r = repo("inside", 1, 3);
        let g = Governor::for_repo(&r.dir).with_log(r.dir.join(".logscope/audit.log"));
        match g.assess() {
            Err(GovernorError::LogInsideWorkspace { .. }) => {}
            other => panic!("expected refusal, got {other:?}"),
        }
        // and the message says what to do about it
        let msg = GovernorError::LogInsideWorkspace { log: "a".into(), repo: "b".into() }.to_string();
        assert!(msg.contains("rewrite its own record"));
    }

    #[test]
    fn a_fresh_repo_with_no_baseline_reports_its_whole_history() {
        let r = repo("nobaseline", 3, 10);
        let g = Governor::for_repo(&r.dir).with_log(logfile("nobaseline"));
        let a = g.assess().unwrap();
        assert_eq!(a.unreviewed_commits, 3);
        let _ = std::fs::remove_file(g.log_path());
    }

    #[test]
    fn a_baseline_clears_the_backlog_and_new_work_reappears() {
        let r = repo("baseline", 3, 10);
        let log = logfile("baseline");
        let g = Governor::for_repo(&r.dir).with_log(&log);

        let head = g.record_baseline().unwrap();
        assert!(!head.is_empty());
        assert_eq!(g.assess().unwrap().unreviewed_commits, 0, "baseline settles history");

        std::fs::write(r.dir.join("new.txt"), "a\nb\n").unwrap();
        git(&r.dir, &["add", "."]);
        git(&r.dir, &["commit", "-qm", "agent work"]);
        let a = g.assess().unwrap();
        assert_eq!(a.unreviewed_commits, 1);
        assert_eq!(a.unreviewed_lines, 2);
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn a_frozen_branch_gets_a_narrowed_verify_spec() {
        let r = repo("frozen", 2, 200);
        let log = logfile("frozen");
        // freeze immediately: any unreviewed work at all is over budget
        let th = Thresholds { warn: 0, freeze: 0, max_age_days: 0 };
        let g = Governor::for_repo(&r.dir).with_log(&log).with_thresholds(th);

        let spec = crate::sandbox::VerifySpec::cargo_build(&r.dir)
            .with_net(true)
            .with_trust(crate::isolate::Trust::Own)
            .allowing(&["serde".to_string()]);
        let (narrowed, a) = g.prepare(spec).unwrap();
        assert_eq!(a.posture, Posture::Frozen);
        assert!(!narrowed.net);
        assert!(narrowed.allowed_deps.is_empty());
        assert_eq!(narrowed.trust, crate::isolate::Trust::Generated);
        assert!(!g.may_grant());
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn an_open_branch_is_left_alone() {
        let r = repo("open", 1, 2);
        let log = logfile("open");
        let g = Governor::for_repo(&r.dir).with_log(&log);
        g.record_baseline().unwrap();
        let spec = crate::sandbox::VerifySpec::cargo_build(&r.dir).with_net(true);
        let (narrowed, a) = g.prepare(spec).unwrap();
        assert_eq!(a.posture, Posture::Open);
        assert!(narrowed.net, "an open branch must not be restricted");
        assert!(g.may_grant());
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn the_agent_config_is_wired_to_the_gate_and_the_log() {
        let r = repo("cfg", 1, 2);
        let log = logfile("cfg");
        let g = Governor::for_repo(&r.dir).with_log(&log);
        let cfg = g.agent_config().unwrap();
        assert!(matches!(cfg.gate, crate::taint::ToolGate::Required(_)));
        assert_eq!(cfg.audit_log.as_deref(), Some(log.as_path()));
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn the_anchor_advances_as_the_log_grows() {
        let r = repo("anchor", 1, 2);
        let log = logfile("anchor");
        let g = Governor::for_repo(&r.dir).with_log(&log);
        let a0 = g.anchor();
        g.record_baseline().unwrap();
        let a1 = g.anchor();
        assert_ne!(a0, a1);
        assert_eq!(crate::audit_log::AuditLog::at(&log).verify(), Ok(1));
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn the_status_line_never_panics_on_a_broken_repo() {
        let g = Governor::for_repo("/definitely/not/a/repo").with_log(logfile("broken"));
        let s = g.status_line();
        assert!(s.starts_with("[unknown]"), "{s}");
    }
}

/// Ignored by default. The whole chain against a real repo and a real sandbox: adopt a baseline,
/// let an "agent" write something, watch the posture move, verify under isolation, and confirm the
/// record is intact.
///
///   cargo test -p logscope-egui --lib --no-default-features -- --ignored full_chain --nocapture
#[cfg(test)]
mod full_chain {
    use super::*;
    use super::tests_support::*;

    #[test]
    #[ignore]
    fn adopt_then_work_then_freeze_then_review() {
        assert!(which_git(), "needs git");
        let r = scratch_repo("chain");
        let log = std::env::temp_dir().join(format!("ls-chain-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&log);

        // Tight budget so a modest amount of work trips it.
        let th = Thresholds { warn: 10, freeze: 40, max_age_days: 0 };
        let g = Governor::for_repo(&r).with_log(&log).with_thresholds(th);

        g.record_baseline().unwrap();
        let a0 = g.assess().unwrap();
        println!("after baseline : {}", a0.explain());
        assert_eq!(a0.posture, Posture::Open);
        assert!(g.may_grant());

        // The agent writes, and the commit is backdated so debt accrues without waiting a week.
        commit_lines(&r, "gen.txt", 30, 3);
        let a = g.assess().unwrap();
        println!("after agent    : {}", a.explain());
        assert_eq!(a.posture, Posture::Frozen, "{}", a.explain());
        assert!(!g.may_grant(), "no new capabilities while frozen");

        // A verify request is narrowed rather than refused outright.
        let spec = crate::sandbox::VerifySpec::cargo_build(&r).with_net(true);
        let (narrowed, _) = g.prepare(spec).unwrap();
        assert!(!narrowed.net);

        // A human reads it and says so; the branch reopens.
        let head = head_of(&r);
        g.record_review(&head, "read the generated module").unwrap();
        let a2 = g.assess().unwrap();
        println!("after review   : {}", a2.explain());
        assert_eq!(a2.posture, Posture::Open);
        assert!(g.may_grant());

        // And the record of all of it verifies.
        let l = crate::audit_log::AuditLog::at(&log);
        assert_eq!(l.verify(), Ok(2), "baseline + review");
        let anchor = g.anchor();
        assert!(l.verify_against_anchor(&anchor).is_ok());

        let _ = std::fs::remove_dir_all(&r);
        let _ = std::fs::remove_file(&log);
    }
}

#[cfg(test)]
mod tests_support {
    use std::path::{Path, PathBuf};

    pub fn which_git() -> bool {
        std::process::Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn run(dir: &Path, args: &[&str], when: Option<&str>) {
        let mut c = std::process::Command::new("git");
        c.arg("-C").arg(dir).args(args)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@e");
        if let Some(d) = when {
            c.env("GIT_AUTHOR_DATE", d).env("GIT_COMMITTER_DATE", d);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
    }

    pub fn scratch_repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ls-chain-repo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        run(&d, &["init", "-q", "-b", "main"], None);
        std::fs::write(d.join("seed.txt"), "seed\n").unwrap();
        run(&d, &["add", "."], None);
        run(&d, &["commit", "-qm", "seed"], None);
        d
    }

    /// Commit `lines` lines, dated `days_ago`, so line-days accrue without waiting.
    pub fn commit_lines(repo: &Path, file: &str, lines: usize, days_ago: u64) {
        let body: String = (0..lines).map(|i| format!("l{i}\n")).collect();
        std::fs::write(repo.join(file), body).unwrap();
        let when = format!("{} +0000", now_secs() - days_ago * 86_400);
        run(repo, &["add", "."], Some(&when));
        run(repo, &["commit", "-qm", "generated"], Some(&when));
    }

    pub fn head_of(repo: &Path) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }
}
