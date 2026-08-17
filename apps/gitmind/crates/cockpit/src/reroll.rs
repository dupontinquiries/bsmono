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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

// ── running a plan ───────────────────────────────────────────────────────────

/// What the run is doing, as it happens.
///
/// A reroll takes minutes and the operator is looking at a button, not a
/// terminal — that was the whole complaint. So the executor streams rather
/// than returning at the end: every step announces itself before it blocks,
/// and the GUI has something to draw the entire time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Started { steps: usize },
    /// A step is about to run. Emitted BEFORE the work, so a step that hangs
    /// is visible as the thing that hung.
    StepStarted(Step),
    StepDone(Step),
    StepFailed { step: Step, error: String },
    /// A step the plan contained but the run did not attempt, because
    /// something it depended on failed.
    StepSkipped { step: Step, because: String },
    Finished,
}

/// The effectful half. Implemented over ssh/cargo/scp in the app; faked in
/// tests, which is the only reason the ordering and failure rules below can
/// be pinned at all.
pub trait Runner {
    /// Pair with a device: password auth, fleet user, key install.
    fn enroll(&mut self, device: &str) -> Result<(), String>;
    /// Build one app for one target.
    fn build(&mut self, app: &str, triple: &str) -> Result<(), String>;
    /// Copy the binary and write the `.desktop` entry.
    fn install(&mut self, app: &str, device: &str) -> Result<(), String>;
    /// Which triple a device needs — the executor has the plan, not the
    /// roster, and a failed build has to know which installs it invalidated.
    fn triple_of(&self, device: &str) -> Option<String>;
}

/// Walk a plan, emitting events, and report per device.
///
/// The failure rules are the substance here, and each exists because the
/// naive version does the wrong thing:
///
/// * A device whose **enrolment** fails gets no installs. Attempting them
///   would produce a second, more confusing error per app against a device we
///   already know we cannot log into.
/// * A failed **build** skips that app's installs on every device sharing the
///   triple — they were all waiting on the one binary — but leaves other apps
///   and other architectures alone. Aborting the run instead would mean one
///   broken app costs you the whole fleet.
/// * A failed **install** is confined to that device and that app. It is the
///   only failure that says nothing about anything else.
pub fn run<R: Runner>(plan: &Plan, r: &mut R, on: &mut dyn FnMut(Event)) -> Report {
    let mut report = Report::default();
    for (device, why) in &plan.skipped {
        report.record(device, Outcome::Skipped(*why));
    }
    on(Event::Started { steps: plan.steps.len() });

    // Devices we can no longer reach, and (app, triple) pairs with no binary.
    let mut dead_devices: BTreeMap<String, String> = BTreeMap::new();
    let mut dead_builds: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut installed: BTreeMap<String, usize> = BTreeMap::new();

    for step in &plan.steps {
        match step {
            Step::Enroll { device } => {
                on(Event::StepStarted(step.clone()));
                match r.enroll(device) {
                    Ok(()) => on(Event::StepDone(step.clone())),
                    Err(e) => {
                        dead_devices.insert(device.clone(), e.clone());
                        report.record(device, Outcome::EnrollFailed(e.clone()));
                        on(Event::StepFailed { step: step.clone(), error: e });
                    }
                }
            }
            Step::Build { app, triple } => {
                on(Event::StepStarted(step.clone()));
                match r.build(app, triple) {
                    Ok(()) => on(Event::StepDone(step.clone())),
                    Err(e) => {
                        dead_builds.insert((app.clone(), triple.clone()), e.clone());
                        on(Event::StepFailed { step: step.clone(), error: e });
                    }
                }
            }
            Step::Install { app, device } => {
                if let Some(why) = dead_devices.get(device) {
                    on(Event::StepSkipped {
                        step: step.clone(),
                        because: format!("enrolment failed: {why}"),
                    });
                    continue;
                }
                let triple = r.triple_of(device).unwrap_or_default();
                if let Some(why) = dead_builds.get(&(app.clone(), triple.clone())) {
                    // The device is fine; the binary does not exist. Recorded
                    // as a build failure so nobody goes to reflash a phone
                    // over a missing linker.
                    report.record(device, Outcome::BuildFailed {
                        app: app.clone(),
                        error: why.clone(),
                    });
                    on(Event::StepSkipped {
                        step: step.clone(),
                        because: format!("build failed: {why}"),
                    });
                    continue;
                }
                on(Event::StepStarted(step.clone()));
                match r.install(app, device) {
                    Ok(()) => {
                        *installed.entry(device.clone()).or_insert(0) += 1;
                        on(Event::StepDone(step.clone()));
                    }
                    Err(e) => {
                        report.record(device, Outcome::InstallFailed {
                            app: app.clone(),
                            error: e.clone(),
                        });
                        on(Event::StepFailed { step: step.clone(), error: e });
                    }
                }
            }
        }
    }

    // A device only counts as Installed if nothing already claimed it as a
    // failure — one broken app out of four is not a green device.
    for (device, n) in installed {
        report.per_device.entry(device).or_insert(Outcome::Installed { apps: n });
    }
    on(Event::Finished);
    report
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

    // ── the executor ──

    /// A Runner that does nothing but remember, and fail where told to.
    struct Fake {
        triples: BTreeMap<String, String>,
        fail_enroll: Vec<String>,
        fail_build: Vec<(String, String)>,
        fail_install: Vec<(String, String)>,
        did: Vec<String>,
    }

    impl Fake {
        fn new(devices: &[Device]) -> Fake {
            Fake {
                triples: devices.iter().map(|d| (d.id.clone(), d.triple.clone())).collect(),
                fail_enroll: Vec::new(),
                fail_build: Vec::new(),
                fail_install: Vec::new(),
                did: Vec::new(),
            }
        }
    }

    impl Runner for Fake {
        fn enroll(&mut self, device: &str) -> Result<(), String> {
            self.did.push(format!("enroll {device}"));
            if self.fail_enroll.iter().any(|d| d == device) {
                return Err("bad password".into());
            }
            Ok(())
        }
        fn build(&mut self, app: &str, triple: &str) -> Result<(), String> {
            self.did.push(format!("build {app} {triple}"));
            if self.fail_build.iter().any(|(a, t)| a == app && t == triple) {
                return Err("linker not found".into());
            }
            Ok(())
        }
        fn install(&mut self, app: &str, device: &str) -> Result<(), String> {
            self.did.push(format!("install {app} {device}"));
            if self.fail_install.iter().any(|(a, d)| a == app && d == device) {
                return Err("no space left".into());
            }
            Ok(())
        }
        fn triple_of(&self, device: &str) -> Option<String> {
            self.triples.get(device).cloned()
        }
    }

    fn drain(plan: &Plan, f: &mut Fake) -> (Report, Vec<Event>) {
        let mut events = Vec::new();
        let report = run(plan, f, &mut |e| events.push(e));
        (report, events)
    }

    #[test]
    fn a_clean_run_touches_every_device_and_brackets_itself() {
        let devices = vec![
            dev("fp5", ARM, Enrollment::Keyed, Reach::Online),
            dev("pine", ARM, Enrollment::Keyed, Reach::Online),
        ];
        let p = plan(&devices, &["voiceterm".to_string()]);
        let mut f = Fake::new(&devices);
        let (report, events) = drain(&p, &mut f);
        assert_eq!(report.summary(), "2 of 2 devices up to date");
        assert_eq!(events.first(), Some(&Event::Started { steps: p.steps.len() }));
        assert_eq!(events.last(), Some(&Event::Finished));
        // One build, two installs — the dedupe survives into the run.
        assert_eq!(f.did.iter().filter(|s| s.starts_with("build")).count(), 1);
        assert_eq!(f.did.iter().filter(|s| s.starts_with("install")).count(), 2);
    }

    #[test]
    fn a_step_announces_itself_before_it_blocks() {
        // The operator is watching a button, not a terminal. A step that
        // hangs has to be visible as the thing that hung, which means the
        // event goes out BEFORE the work.
        let devices = vec![dev("pine", ARM, Enrollment::Keyed, Reach::Online)];
        let p = plan(&devices, &["voiceterm".to_string()]);
        let (_, events) = drain(&p, &mut Fake::new(&devices));
        let started = events.iter().position(|e| matches!(e, Event::StepStarted(_))).unwrap();
        let done = events.iter().position(|e| matches!(e, Event::StepDone(_))).unwrap();
        assert!(started < done);
    }

    #[test]
    fn a_device_we_cannot_enrol_gets_no_installs_attempted() {
        let devices = vec![dev("pine", ARM, Enrollment::None, Reach::Online)];
        let p = plan(&devices, &apps());
        let mut f = Fake::new(&devices);
        f.fail_enroll.push("pine".into());
        let (report, events) = drain(&p, &mut f);
        assert!(matches!(report.per_device["pine"], Outcome::EnrollFailed(_)));
        // Two apps, and NEITHER was attempted: a second and third error
        // against a device we already know we cannot log into is noise.
        assert!(!f.did.iter().any(|s| s.starts_with("install")));
        assert_eq!(events.iter().filter(|e| matches!(e, Event::StepSkipped { .. })).count(), 2);
    }

    #[test]
    fn a_failed_build_spares_other_apps_and_other_architectures() {
        let devices = vec![
            dev("pine", ARM, Enrollment::Keyed, Reach::Online),
            dev("lap", "x86_64-unknown-linux-gnu", Enrollment::Keyed, Reach::Online),
        ];
        let p = plan(&devices, &apps());
        let mut f = Fake::new(&devices);
        // voiceterm fails on ARM only.
        f.fail_build.push(("voiceterm".into(), ARM.to_string()));
        let (report, _) = drain(&p, &mut f);
        // The arm phone is reported as a BUILD failure, not its own fault —
        // nobody should reflash a phone over a missing linker.
        assert!(matches!(report.per_device["pine"], Outcome::BuildFailed { .. }));
        // The x86 box got voiceterm anyway: a different triple, a different
        // binary, no reason to punish it.
        assert!(f.did.contains(&"install voiceterm lap".to_string()));
        // And snapclone reached BOTH — one broken app must not cost the fleet.
        assert!(f.did.contains(&"install snapclone pine".to_string()));
        assert!(f.did.contains(&"install snapclone lap".to_string()));
    }

    #[test]
    fn one_bad_install_does_not_make_the_device_green() {
        let devices = vec![dev("pine", ARM, Enrollment::Keyed, Reach::Online)];
        let p = plan(&devices, &apps());
        let mut f = Fake::new(&devices);
        f.fail_install.push(("snapclone".into(), "pine".into()));
        let (report, _) = drain(&p, &mut f);
        // One of two apps landed. That is not a device that is up to date,
        // and reporting Installed here would hide the failure behind a tick.
        assert!(matches!(report.per_device["pine"], Outcome::InstallFailed { .. }));
        assert!(report.succeeded().is_empty());
    }

    #[test]
    fn an_offline_device_is_in_the_report_before_the_run_starts() {
        let devices = vec![
            dev("pine", ARM, Enrollment::Keyed, Reach::Online),
            dev("fp5", ARM, Enrollment::Keyed, Reach::Offline),
        ];
        let p = plan(&devices, &["voiceterm".to_string()]);
        let (report, _) = drain(&p, &mut Fake::new(&devices));
        assert_eq!(report.per_device["fp5"], Outcome::Skipped(Skipped::Offline));
        assert_eq!(report.summary(), "1 of 2 devices — 1 need attention");
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
