//! confinement.rs — what the KERNEL will enforce, as opposed to what we intend.
//!
//! `blastradius` decides what an agent should be allowed to do. It runs inside the process, which
//! means it is advisory: an agent that constructs a shell command, or a library that calls `unlink`
//! directly, never passes through it.
//!
//! **The kernel enforces what the process cannot lie about.** So the two are layers rather than
//! alternatives, and the useful thing this module does is report HONESTLY which one is actually in
//! force — because "we check before deleting" and "deletion is impossible" are very different
//! claims, and a system that conflates them is more dangerous than one with no checks, since it is
//! trusted more.
//!
//! ## The Linux options, and which fits
//!
//! * **Landlock** (5.13+) — unprivileged, per-process, filesystem paths. **The right one for a
//!   desktop app**: it needs no root, no policy file, and no administrator, and a process can
//!   restrict ITSELF at startup. Irreversible once applied, which is the property that makes it
//!   trustworthy.
//! * **seccomp** — syscall filtering, also unprivileged. Complements Landlock: Landlock says which
//!   paths, seccomp says which operations.
//! * **bubblewrap** — namespace isolation, needs a helper binary but no root. Strong, and it changes
//!   how the app is launched rather than what it does.
//! * **SELinux / AppArmor** — mandatory access control, but the policy is the ADMINISTRATOR's, not
//!   ours. We can report whether it is enforcing; we cannot set it, and pretending we can would be
//!   the worst kind of false assurance.
//! * **systemd** `ProtectSystem`, `ReadWritePaths` — for the daemon case, set by a unit file.
//!
//! The distinction that matters: Landlock and seccomp are things this process can do TO ITSELF.
//! Everything else is something done to it, which we may observe and must not claim credit for.

/// A confinement mechanism and whether it is actually doing anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mechanism {
    /// Applied by this process to itself. The strongest claim available, because nothing outside had
    /// to cooperate.
    Landlock { paths_restricted: bool },
    Seccomp { syscalls_filtered: bool },
    /// Applied to us. Observable, not ours.
    Selinux { enforcing: bool },
    AppArmor { confined: bool },
    Bubblewrap,
    SystemdSandbox { protect_system: bool },
}

impl Mechanism {
    /// Did WE apply it? The difference between a guarantee and a lucky environment.
    pub fn self_applied(self) -> bool {
        matches!(self, Mechanism::Landlock { .. } | Mechanism::Seccomp { .. })
    }

    /// Is it actually restricting anything right now?
    ///
    /// **Present is not enforcing.** SELinux in permissive mode logs and allows; reporting it as
    /// protection is exactly the false assurance this module exists to prevent.
    pub fn effective(self) -> bool {
        match self {
            Mechanism::Landlock { paths_restricted } => paths_restricted,
            Mechanism::Seccomp { syscalls_filtered } => syscalls_filtered,
            Mechanism::Selinux { enforcing } => enforcing,
            Mechanism::AppArmor { confined } => confined,
            Mechanism::Bubblewrap => true,
            Mechanism::SystemdSandbox { protect_system } => protect_system,
        }
    }
}

/// What is actually in force.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Confinement { pub mechanisms: Vec<Mechanism> }

impl Confinement {
    /// **The honest headline.** Advisory means blastradius is all there is.
    pub fn enforcement_level(&self) -> Level {
        let eff: Vec<_> = self.mechanisms.iter().copied().filter(|m| m.effective()).collect();
        if eff.is_empty() { return Level::AdvisoryOnly }
        if eff.iter().any(|m| m.self_applied()) { return Level::SelfConfined }
        Level::ExternallyConfined
    }

    /// One line for the operator, and it must not overstate.
    pub fn report(&self) -> String {
        let eff: Vec<_> = self.mechanisms.iter().copied().filter(|m| m.effective()).collect();
        match self.enforcement_level() {
            Level::AdvisoryOnly => format!(
                "NO KERNEL CONFINEMENT. {} mechanism(s) detected and none is enforcing, so the only \
limits are the ones this process chooses to respect — an agent that shells out is not covered. \
Landlock needs no root and would change that",
                self.mechanisms.len()),
            Level::SelfConfined => format!(
                "self-confined: {:?}. This process restricted ITSELF and cannot undo it, which is the \
strongest claim available here", eff),
            Level::ExternallyConfined => format!(
                "confined by the system: {:?}. Real, and not ours — it depends on this machine's \
configuration and will not follow the app elsewhere", eff),
        }
    }

    /// Should an irreversible action be permitted at all?
    ///
    /// Not a hard block: on an unconfined machine that would refuse everything, and a safety feature
    /// that makes the tool unusable gets disabled. It raises the bar instead.
    pub fn irreversible_needs_extra_care(&self) -> bool {
        self.enforcement_level() == Level::AdvisoryOnly
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level { AdvisoryOnly, SelfConfined, ExternallyConfined }

#[cfg(test)]
mod tests {
    use super::*;

    fn c(m: Vec<Mechanism>) -> Confinement { Confinement { mechanisms: m } }

    #[test]
    fn PRESENT_is_not_ENFORCING() {
        // SELinux in permissive mode logs and allows. Reporting it as protection is the false
        // assurance this module exists to prevent.
        let permissive = c(vec![Mechanism::Selinux { enforcing: false }]);
        assert_eq!(permissive.enforcement_level(), Level::AdvisoryOnly);
        assert!(permissive.report().contains("none is enforcing"));
    }

    #[test]
    fn SELF_APPLIED_confinement_outranks_the_environment_being_lucky() {
        // Landlock is applied by this process and cannot be undone; SELinux is the administrator's
        // and will not follow the app to another machine.
        assert_eq!(c(vec![Mechanism::Landlock { paths_restricted: true }]).enforcement_level(),
                   Level::SelfConfined);
        assert_eq!(c(vec![Mechanism::Selinux { enforcing: true }]).enforcement_level(),
                   Level::ExternallyConfined);
    }

    #[test]
    fn an_UNCONFINED_system_says_so_plainly_and_names_the_fix() {
        // "An agent that shells out is not covered" is the actionable part — and Landlock needing no
        // root is why the fix is available rather than a wish.
        let r = c(vec![]).report();
        assert!(r.contains("NO KERNEL CONFINEMENT"));
        assert!(r.contains("an agent that shells out is not covered"));
        assert!(r.contains("Landlock needs no root"));
    }

    #[test]
    fn EXTERNAL_confinement_is_credited_honestly_and_not_claimed() {
        // Real protection, and not ours: it depends on the machine and does not travel with the app.
        let r = c(vec![Mechanism::Selinux { enforcing: true }]).report();
        assert!(r.contains("not ours"));
        assert!(r.contains("will not follow the app elsewhere"));
    }

    #[test]
    fn being_unconfined_RAISES_THE_BAR_rather_than_blocking_everything() {
        // A hard block on an unconfined machine refuses everything, and a safety feature that makes
        // the tool unusable gets disabled — which leaves less protection than the honest warning.
        assert!(c(vec![]).irreversible_needs_extra_care());
        assert!(!c(vec![Mechanism::Landlock { paths_restricted: true }]).irreversible_needs_extra_care());
    }

    #[test]
    fn several_mechanisms_where_only_one_enforces_reads_as_that_one() {
        // The common real case: SELinux permissive, AppArmor absent, Landlock applied.
        let mixed = c(vec![Mechanism::Selinux { enforcing: false },
                           Mechanism::AppArmor { confined: false },
                           Mechanism::Landlock { paths_restricted: true }]);
        assert_eq!(mixed.enforcement_level(), Level::SelfConfined);
    }
}

/// Detect what is in force, warning only.
///
/// **Deliberately minimal**: this observes, it does not apply Landlock. Applying it is irreversible
/// per process and would need a decision about which paths, which is a bigger question than a first
/// step should answer. Warning first is the honest order — the system says what it can and cannot
/// promise, and enforcement follows once the paths are understood.
pub fn detect() -> Confinement {
    let mut mechanisms = Vec::new();

    // SELinux: /sys/fs/selinux/enforce is "1" when enforcing, "0" when permissive. Absent means the
    // LSM is not loaded at all, which is different from loaded-and-permissive and is why absence is
    // not recorded as a disabled mechanism.
    if let Ok(v) = std::fs::read_to_string("/sys/fs/selinux/enforce") {
        mechanisms.push(Mechanism::Selinux { enforcing: v.trim() == "1" });
    }

    // AppArmor: /proc/self/attr/current is "unconfined" when not, or a profile name followed by a
    // mode. "(complain)" is AppArmor's permissive mode and must not count as confinement, for the
    // same reason SELinux permissive does not.
    if let Ok(v) = std::fs::read_to_string("/proc/self/attr/current") {
        let v = v.trim();
        if !v.is_empty() {
            let confined = v != "unconfined" && !v.contains("(complain)");
            mechanisms.push(Mechanism::AppArmor { confined });
        }
    }

    // bubblewrap and flatpak both leave /.flatpak-info or set container=, which is the only signal
    // available from inside — a namespace does not announce itself.
    if std::path::Path::new("/.flatpak-info").exists()
        || std::env::var_os("container").is_some() {
        mechanisms.push(Mechanism::Bubblewrap);
    }

    // Landlock: reported as NOT applied, because this function does not apply it. Recording the
    // kernel's SUPPORT as though it were protection would be the precise false assurance this module
    // exists to prevent — a capability nobody used protects nothing.
    mechanisms.push(Mechanism::Landlock { paths_restricted: false });

    Confinement { mechanisms }
}

#[cfg(test)]
mod detect_tests {
    use super::*;

    #[test]
    fn detection_never_claims_LANDLOCK_it_did_not_apply() {
        // The whole risk of a detector: recording the kernel's support as though it were protection.
        // A capability nobody used protects nothing.
        let c = detect();
        assert!(c.mechanisms.iter().any(|m| matches!(m, Mechanism::Landlock { paths_restricted: false })));
        assert!(!c.mechanisms.iter().any(|m| m.self_applied() && m.effective()));
    }

    #[test]
    fn an_unconfined_container_reports_ADVISORY_and_says_so() {
        // The expected result on most desktops, and it must read as a warning rather than as silence.
        let c = detect();
        if c.enforcement_level() == Level::AdvisoryOnly {
            assert!(c.report().contains("NO KERNEL CONFINEMENT"));
        }
    }

    #[test]
    fn detection_does_not_PANIC_when_the_lsm_files_are_absent() {
        // Most systems have neither, and a detector that fails on a normal machine is worse than no
        // detector.
        let _ = detect();
    }
}
