//! guardrails.rs — the training wheels for the autonomous loop.
//!
//! The swarm can write files and produce patches. These gates decide what it may do WITHOUT a human in the
//! loop, and they default to conservative. Pure + testable; `run_mounted` consults `ship_decision` at the
//! ship boundary and `may_write` before any file write. The wheels come OFF deliberately (set Autonomy),
//! never by accident.
//!
//! Note the isolation already in place is the strongest wheel: `run_mounted` fills a /tmp scratch and emits
//! a reviewable patch — it never touches the user's tree. These gates layer policy on top of that safety.

/// How much the loop may do unattended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Autonomy {
    DryRun,     // generate + verify only — never write a patch. The safest wheel.
    Supervised, // may produce a patch, but always HELD for human review (never presented as auto-applied).
    Autonomous, // may ship automatically once the confidence floor is met.
}
impl Autonomy {
    pub fn label(self) -> &'static str { match self { Autonomy::DryRun => "dry-run", Autonomy::Supervised => "supervised", Autonomy::Autonomous => "autonomous" } }
}

/// The policy consulted at every autonomy decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Guardrails {
    pub autonomy: Autonomy,
    pub scope: Vec<String>,       // path prefixes the swarm may modify (empty ⇒ deny all writes)
    pub require_all_passed: bool, // ship only when every subtask verified
    pub min_agreement: f32,       // consensus floor (0..=1) for auto-ship; 0 disables
    pub max_files: usize,         // cap on files a single run may touch
}

impl Default for Guardrails {
    fn default() -> Self { Self::training_wheels() }
}

impl Guardrails {
    /// Wheels ON: supervised, writes confined to `src/`, ship only a fully-verified artifact, ≤ 20 files.
    pub fn training_wheels() -> Self {
        Self { autonomy: Autonomy::Supervised, scope: vec!["src/".into()], require_all_passed: true, min_agreement: 0.0, max_files: 20 }
    }
    /// Wheels OFF: autonomous, but STILL requires all-passed + a 2/3 consensus floor. "Off" is not "reckless".
    pub fn autonomous() -> Self {
        Self { autonomy: Autonomy::Autonomous, scope: vec!["src/".into()], require_all_passed: true, min_agreement: 0.66, max_files: 50 }
    }

    /// A path is writable iff it sits under an allowed scope prefix and isn't an escape (absolute or `..`).
    pub fn may_write(&self, path: &str) -> bool {
        if path.starts_with('/') || path.split(['/', '\\']).any(|seg| seg == "..") { return false; }
        !self.scope.is_empty() && self.scope.iter().any(|p| path.starts_with(p.as_str()))
    }

    /// Filter a set of target paths to the writable subset (used to pre-flight a run's file list).
    pub fn writable<'a>(&self, paths: &'a [String]) -> Vec<&'a String> {
        paths.iter().filter(|p| self.may_write(p)).collect()
    }

    /// Decide what to do with a completed run: ship, hold for review, or block — with a reason.
    pub fn ship_decision(&self, all_passed: bool, complete: bool, agreement: f32, files_touched: usize) -> ShipDecision {
        if !complete { return ShipDecision::Blocked("no complete artifact to ship".into()); }
        if files_touched > self.max_files { return ShipDecision::Blocked(format!("touched {files_touched} files > cap {}", self.max_files)); }
        if self.require_all_passed && !all_passed { return ShipDecision::HoldForReview("not all subtasks verified".into()); }
        if self.min_agreement > 0.0 && agreement < self.min_agreement {
            return ShipDecision::HoldForReview(format!("agreement {:.0}% below floor {:.0}%", agreement * 100.0, self.min_agreement * 100.0));
        }
        match self.autonomy {
            Autonomy::DryRun => ShipDecision::Blocked("dry-run: nothing written".into()),
            Autonomy::Supervised => ShipDecision::HoldForReview("supervised: patch ready for your review".into()),
            Autonomy::Autonomous => ShipDecision::Ship,
        }
    }
}

/// The outcome of the ship gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShipDecision {
    Ship,                  // produce the patch and present it as ready to apply
    HoldForReview(String), // produce the patch but flag it for human review
    Blocked(String),       // produce nothing
}
impl ShipDecision {
    /// Both Ship and HoldForReview still WRITE the reviewable patch (run_mounted never auto-applies to the
    /// user's tree anyway); only Blocked writes nothing.
    pub fn writes_patch(&self) -> bool { !matches!(self, ShipDecision::Blocked(_)) }
    pub fn label(&self) -> String {
        match self {
            ShipDecision::Ship => "ship".into(),
            ShipDecision::HoldForReview(r) => format!("hold for review — {r}"),
            ShipDecision::Blocked(r) => format!("blocked — {r}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_confines_writes_and_blocks_escapes() {
        let g = Guardrails::training_wheels();
        assert!(g.may_write("src/lib.rs"));
        assert!(g.may_write("src/a/b.rs"));
        assert!(!g.may_write("Cargo.toml"), "outside src/ scope");
        assert!(!g.may_write("/etc/passwd"), "absolute path");
        assert!(!g.may_write("src/../../../etc/passwd"), "path escape");
        assert!(!g.may_write("../secrets"), "parent escape");
        // empty scope denies everything
        let deny = Guardrails { scope: vec![], ..Guardrails::training_wheels() };
        assert!(!deny.may_write("src/lib.rs"));
    }

    #[test]
    fn dry_run_never_ships() {
        let g = Guardrails { autonomy: Autonomy::DryRun, ..Guardrails::training_wheels() };
        let d = g.ship_decision(true, true, 1.0, 1);
        assert!(matches!(d, ShipDecision::Blocked(_)));
        assert!(!d.writes_patch(), "dry-run writes nothing");
    }

    #[test]
    fn supervised_holds_for_review_even_when_perfect() {
        let g = Guardrails::training_wheels(); // supervised
        let d = g.ship_decision(true, true, 1.0, 3);
        assert!(matches!(d, ShipDecision::HoldForReview(_)), "supervised never auto-ships");
        assert!(d.writes_patch(), "but the reviewable patch is still produced");
    }

    #[test]
    fn autonomous_ships_only_above_the_floors() {
        let g = Guardrails::autonomous(); // all_passed + 0.66 agreement
        assert_eq!(g.ship_decision(true, true, 1.0, 5), ShipDecision::Ship);
        // fails the agreement floor → held
        assert!(matches!(g.ship_decision(true, true, 0.5, 5), ShipDecision::HoldForReview(_)));
        // not all passed → held
        assert!(matches!(g.ship_decision(false, true, 1.0, 5), ShipDecision::HoldForReview(_)));
        // over the file cap → blocked
        assert!(matches!(g.ship_decision(true, true, 1.0, 999), ShipDecision::Blocked(_)));
        // incomplete → blocked
        assert!(matches!(g.ship_decision(true, false, 1.0, 5), ShipDecision::Blocked(_)));
    }

    #[test]
    fn writable_filters_a_file_list() {
        let g = Guardrails::training_wheels();
        let paths = vec!["src/a.rs".to_string(), "Cargo.toml".to_string(), "src/b.rs".to_string(), "/tmp/x".to_string()];
        let ok = g.writable(&paths);
        assert_eq!(ok, vec![&"src/a.rs".to_string(), &"src/b.rs".to_string()]);
    }
}
