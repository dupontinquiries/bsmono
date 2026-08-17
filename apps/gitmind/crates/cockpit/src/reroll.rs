//! reroll — one button, every test device.
//!
//! The workflow this exists for: wipe the phones often, then get every app
//! back onto all of them from the laptop with as little friction as the
//! security allows. That is a *batch* problem, and the pieces around it were
//! all single-device — `deploy` scans and copies to one host, `installer`
//! writes one `.desktop`, `fleet` provisions one user. Nothing planned a run
//! across the roster, so a reroll was N manual passes and the friction was
//! the point of failure.
//!
//! ## Build once per target, not once per device
//!
//! Three phones on `aarch64-unknown-linux-gnu` is ONE build and three copies.
//! Planning per-device instead would triple the slow half of a reroll, and
//! the slow half is the reason it feels like a chore. The plan therefore
//! groups build jobs by `(app, triple)` and fans the install jobs out from
//! them.
//!
//! ## A wiped phone is the normal case, not an error
//!
//! After a wipe the device has no fleet key and no fleet user; it has an
//! address and a password. The plan says so — `NeedsEnrollment` — rather than
//! discovering it when `scp` fails halfway through. Re-enrolling is a step in
//! the run, not a prerequisite the operator has to remember.
//!
//! ## One dead device does not fail the batch
//!
//! A phone that is off, or on a different network, or mid-reboot is skipped
//! with a reason and the rest proceed. A batch that aborts on the first
//! unreachable device is a batch you cannot trust to leave running.

use std::collections::BTreeMap;

/// How much trust is established with a device right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enrollment {
    /// Never paired, or wiped since. Password auth is the only way in.
    None,
    /// The fleet user exists and our key is installed.
    Keyed,
}

/// Whether the device answered the last probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Online,
    Offline,
}

/// One test device on the roster.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// Stable id the roster is keyed by. Survives a wipe, unlike a host key —
    /// which is the point: the operator's mental model is "my pinephone", and
    /// that identity has to outlive the OS on it.
    pub id: String,
    pub label: String,
    pub host: String,
    /// The rust target this device needs.
    pub triple: String,
    pub enrollment: Enrollment,
    pub reach: Reach,
}

/// What a run has to do before it can install anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Pair with a device: password auth, create the fleet user, install the
    /// key. Carries the device id, not a password — a plan is a value that
    /// gets logged and compared, and a secret in it would end up in both.
    Enroll { device: String },
    /// Build an app for a target, once, however many devices want it.
    Build { app: String, triple: String },
    /// Copy the built binary and write its `.desktop` entry.
    Install { app: String, device: String },
}

/// Why a device is not in the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    Offline,
    /// Nothing on the roster's app list targets this device's triple.
    NothingToInstall,
}

/// The whole run, decided up front.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Devices left out, and why. Reported rather than dropped, so the
    /// operator sees "3 of 4 phones" instead of silently getting three.
    pub skipped: Vec<(String, Skipped)>,
}

impl Plan {
    /// How many builds this run will do. The number the operator is really
    /// asking about when they wonder how long it takes.
    pub fn build_count(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s, Step::Build { .. })).count()
    }

    pub fn install_count(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s, Step::Install { .. })).count()
    }

    pub fn enroll_count(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s, Step::Enroll { .. })).count()
    }
}

/// Plan a reroll of `apps` onto `devices`.
///
/// Ordering is fixed and deterministic — enrol, then build, then install —
/// because a re-run after a failure has to do the same thing in the same
/// order, and because a build that finishes while a device is still
/// unenrolled has nowhere to go.
///
/// Within each phase the order is by id/name so two runs of the same roster
/// produce identical plans. A plan that reshuffles is a plan you cannot diff
/// against the last one to see what changed.
pub fn plan(devices: &[Device], apps: &[String]) -> Plan {
    let mut steps = Vec::new();
    let mut skipped = Vec::new();

    let mut live: Vec<&Device> = Vec::new();
    for d in devices {
        match d.reach {
            Reach::Offline => skipped.push((d.id.clone(), Skipped::Offline)),
            Reach::Online if apps.is_empty() => {
                skipped.push((d.id.clone(), Skipped::NothingToInstall))
            }
            Reach::Online => live.push(d),
        }
    }
    live.sort_by(|a, b| a.id.cmp(&b.id));

    // Enrolment first: a build finishing against a device we cannot log into
    // is a build with nowhere to go.
    for d in live.iter().filter(|d| d.enrollment == Enrollment::None) {
        steps.push(Step::Enroll { device: d.id.clone() });
    }

    // One build per (app, triple). Three phones on the same architecture is
    // one compile — this dedupe is the whole reason a reroll is quick.
    let mut needed: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for d in &live {
        for app in apps {
            needed
                .entry((app.clone(), d.triple.clone()))
                .or_default()
                .push(d.id.clone());
        }
    }
    for (app, triple) in needed.keys() {
        steps.push(Step::Build { app: app.clone(), triple: triple.clone() });
    }

    // Installs fan out from the builds, grouped the same way so a run reads
    // as "built X, pushed it to these three".
    for ((app, _triple), targets) in &needed {
        for device in targets {
            steps.push(Step::Install { app: app.clone(), device: device.clone() });
        }
    }

    Plan { steps, skipped }
}

/// How one device's part of a run turned out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Installed { apps: usize },
    /// Enrolment failed — usually a wrong password or sshd not up yet.
    EnrollFailed(String),
    /// A build failed. Named separately because it is NOT that device's
    /// fault, and it will have failed for every device sharing the triple.
    BuildFailed { app: String, error: String },
    InstallFailed { app: String, error: String },
    Skipped(Skipped),
}

impl Outcome {
    pub fn ok(&self) -> bool {
        matches!(self, Outcome::Installed { .. })
    }
}

/// The result of a run, per device.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub per_device: BTreeMap<String, Outcome>,
}

impl Report {
    pub fn record(&mut self, device: &str, outcome: Outcome) {
        self.per_device.insert(device.to_string(), outcome);
    }

    /// Devices that got everything.
    pub fn succeeded(&self) -> Vec<&str> {
        self.per_device
            .iter()
            .filter(|(_, o)| o.ok())
            .map(|(d, _)| d.as_str())
            .collect()
    }

    /// Devices that did not, with the reason. What the operator reads first.
    pub fn failures(&self) -> Vec<(&str, &Outcome)> {
        self.per_device
            .iter()
            .filter(|(_, o)| !o.ok())
            .map(|(d, o)| (d.as_str(), o))
            .collect()
    }

    /// One line for the button that started it.
    pub fn summary(&self) -> String {
        let total = self.per_device.len();
        let ok = self.succeeded().len();
        if total == 0 {
            return "nothing to do".into();
        }
        if ok == total {
            return format!("{ok} of {total} devices up to date");
        }
        format!("{ok} of {total} devices — {} need attention", total - ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, triple: &str, enr: Enrollment, reach: Reach) -> Device {
        Device {
            id: id.into(),
            label: id.into(),
            host: format!("{id}.local"),
            triple: triple.into(),
            enrollment: enr,
            reach,
        }
    }

    const ARM: &str = "aarch64-unknown-linux-gnu";

    fn apps() -> Vec<String> {
        vec!["voiceterm".to_string(), "snapclone".to_string()]
    }

    #[test]
    fn three_phones_on_one_architecture_is_one_build_each() {
        let devices = vec![
            dev("pine", ARM, Enrollment::Keyed, Reach::Online),
            dev("fp5", ARM, Enrollment::Keyed, Reach::Online),
            dev("pixel", ARM, Enrollment::Keyed, Reach::Online),
        ];
        let p = plan(&devices, &apps());
        // Two apps, one architecture: two builds, not six. This dedupe is the
        // difference between a reroll being a button and being a chore.
        assert_eq!(p.build_count(), 2);
        assert_eq!(p.install_count(), 6);
        assert!(p.skipped.is_empty());
    }

    #[test]
    fn a_second_architecture_earns_its_own_build() {
        let devices = vec![
            dev("pine", ARM, Enrollment::Keyed, Reach::Online),
            dev("lap", "x86_64-unknown-linux-gnu", Enrollment::Keyed, Reach::Online),
        ];
        let p = plan(&devices, &["voiceterm".to_string()]);
        assert_eq!(p.build_count(), 2, "different triples cannot share a binary");
        assert_eq!(p.install_count(), 2);
    }

    #[test]
    fn a_wiped_phone_is_enrolled_as_part_of_the_run() {
        // After a wipe there is no fleet user and no key — an address and a
        // password. That is the NORMAL case here, so the plan schedules it
        // rather than discovering it when scp fails halfway through.
        let devices = vec![
            dev("pine", ARM, Enrollment::None, Reach::Online),
            dev("fp5", ARM, Enrollment::Keyed, Reach::Online),
        ];
        let p = plan(&devices, &["voiceterm".to_string()]);
        assert_eq!(p.enroll_count(), 1);
        // And it happens BEFORE the build finishes, or the binary has nowhere
        // to go.
        let enroll_at = p.steps.iter().position(|s| matches!(s, Step::Enroll { .. })).unwrap();
        let build_at = p.steps.iter().position(|s| matches!(s, Step::Build { .. })).unwrap();
        assert!(enroll_at < build_at);
        // No secret rides in the plan: it gets logged and diffed.
        assert_eq!(p.steps[enroll_at], Step::Enroll { device: "pine".into() });
    }

    #[test]
    fn one_dead_phone_does_not_take_the_batch_down() {
        let devices = vec![
            dev("pine", ARM, Enrollment::Keyed, Reach::Online),
            dev("fp5", ARM, Enrollment::Keyed, Reach::Offline),
        ];
        let p = plan(&devices, &["voiceterm".to_string()]);
        assert_eq!(p.install_count(), 1, "the live one still gets it");
        assert_eq!(p.skipped, vec![("fp5".to_string(), Skipped::Offline)]);
        // Reported, not dropped: the operator should see "1 of 2", not
        // silently get one.
        assert_eq!(p.skipped.len(), 1);
    }

    #[test]
    fn the_same_roster_always_produces_the_same_plan() {
        // A plan that reshuffles cannot be diffed against the last run to see
        // what changed.
        let a = vec![
            dev("fp5", ARM, Enrollment::Keyed, Reach::Online),
            dev("pine", ARM, Enrollment::Keyed, Reach::Online),
        ];
        let b = vec![a[1].clone(), a[0].clone()];
        assert_eq!(plan(&a, &apps()), plan(&b, &apps()), "order in, same plan out");
    }

    #[test]
    fn an_empty_app_list_is_a_skip_rather_than_an_empty_success() {
        let devices = vec![dev("pine", ARM, Enrollment::Keyed, Reach::Online)];
        let p = plan(&devices, &[]);
        assert!(p.steps.is_empty());
        assert_eq!(p.skipped, vec![("pine".to_string(), Skipped::NothingToInstall)]);
    }

    #[test]
    fn a_build_failure_is_not_blamed_on_the_device() {
        let mut r = Report::default();
        r.record("pine", Outcome::Installed { apps: 2 });
        r.record("fp5", Outcome::BuildFailed {
            app: "voiceterm".into(),
            error: "linker not found".into(),
        });
        assert_eq!(r.succeeded(), vec!["pine"]);
        assert_eq!(r.failures().len(), 1);
        // The distinction matters: a build failure hit every device sharing
        // that triple, and telling the operator "fp5 failed" would send them
        // to reflash a phone that is fine.
        assert!(matches!(r.failures()[0].1, Outcome::BuildFailed { .. }));
        assert_eq!(r.summary(), "1 of 2 devices — 1 need attention");
    }

    #[test]
    fn a_clean_run_says_so_in_one_line() {
        let mut r = Report::default();
        r.record("pine", Outcome::Installed { apps: 2 });
        r.record("fp5", Outcome::Installed { apps: 2 });
        assert_eq!(r.summary(), "2 of 2 devices up to date");
        assert_eq!(Report::default().summary(), "nothing to do");
    }
}
