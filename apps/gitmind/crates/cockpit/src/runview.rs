//! runview — what a reroll looks like while it is happening.
//!
//! The executor streams [`Event`](crate::reroll::Event)s; a GUI needs rows and
//! a progress bar. Folding one into the other is pure, so it lives here rather
//! than in a front-end: gitmind ships a Qt GUI and an egui GUI, and a
//! progress display implemented twice is a progress display that disagrees
//! with itself about whether the run finished.
//!
//! ## The fold is where "one button" actually happens
//!
//! The operator pressed one thing and now wants to know: how far along, what
//! is it doing right now, which of my phones are done, and what broke. All
//! four are derivable from the event stream, and none of them are derivable
//! from the final [`Report`](crate::reroll::Report) — which is why a run that
//! only reports at the end feels like a hang no matter how fast it is.
//!
//! ## A step in flight is not a step done
//!
//! `steps_done` counts only completed, failed and skipped steps. Counting the
//! in-flight one would put the bar at 100% while the last install is still
//! copying, and a bar that sits full for thirty seconds is worse than one that
//! sits at 90% — the first looks broken, the second looks busy.

use crate::reroll::{Event, Skipped, Step};
use std::collections::BTreeMap;

/// What a device's row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// On the roster, nothing has happened to it yet this run.
    Waiting,
    Enrolling,
    Renaming,
    Refreshing,
    /// Installing an app, named so the row says which.
    Installing(String),
    /// Everything the plan had for it succeeded.
    Done { apps: usize },
    Failed(String),
    /// Not attempted, with the reason.
    Skipped(Skipped),
}

impl RowState {
    /// Is this device still going to change?
    pub fn active(&self) -> bool {
        matches!(self, RowState::Waiting | RowState::Enrolling | RowState::Renaming
                     | RowState::Refreshing | RowState::Installing(_))
    }

    pub fn line(&self) -> String {
        match self {
            RowState::Waiting => "waiting".into(),
            RowState::Enrolling => "pairing…".into(),
            RowState::Renaming => "renaming…".into(),
            RowState::Refreshing => "updating package index…".into(),
            RowState::Installing(app) => format!("installing {app}…"),
            RowState::Done { apps } => {
                format!("{apps} app{} installed", if *apps == 1 { "" } else { "s" })
            }
            RowState::Failed(why) => why.clone(),
            RowState::Skipped(Skipped::Offline) => "offline".into(),
            RowState::Skipped(Skipped::NothingToInstall) => "nothing to install".into(),
        }
    }
}

/// The whole run, as a UI sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunView {
    /// Total steps the plan declared, from `Started`.
    pub steps_total: usize,
    /// Steps that have reached a terminal state.
    pub steps_done: usize,
    /// What is happening right now, for the line under the bar. `None`
    /// between steps and after the run.
    pub current: Option<String>,
    /// Per device, keyed by id so the rows keep a stable order.
    pub rows: BTreeMap<String, RowState>,
    pub finished: bool,
    /// Builds are not per-device, so they get their own line — a build is the
    /// slow part and attributing it to whichever phone happens to be first in
    /// the list would be a lie about what is taking the time.
    pub building: Option<String>,
    /// How many apps have landed per device.
    ///
    /// Kept apart from `rows` because a row is DISPLAY state and this is a
    /// tally. Reading the count back out of the row looked like it worked and
    /// did not: the second install's `StepStarted` overwrites the row with
    /// `Installing` before its `StepDone` can read the previous `Done`, so
    /// every device reported exactly one app however many it got. A test
    /// caught it; the shape was wrong, not the arithmetic.
    installed: BTreeMap<String, usize>,
}

impl RunView {
    /// Seed the rows from the roster so every device has a line before the
    /// run touches it. A table that grows as devices are reached makes the
    /// operator count phones to notice one is missing.
    pub fn with_devices(ids: &[String]) -> RunView {
        RunView {
            rows: ids.iter().map(|id| (id.clone(), RowState::Waiting)).collect(),
            ..RunView::default()
        }
    }

    /// Fraction complete, 0..=1. Zero total is zero rather than a divide.
    pub fn fraction(&self) -> f32 {
        if self.steps_total == 0 {
            return 0.0;
        }
        (self.steps_done as f32 / self.steps_total as f32).clamp(0.0, 1.0)
    }

    /// One line for the button: what it is doing, or how it went.
    pub fn headline(&self) -> String {
        if self.finished {
            let done = self.rows.values().filter(|r| matches!(r, RowState::Done { .. })).count();
            let total = self.rows.len();
            let bad = self.rows.values().filter(|r| matches!(r, RowState::Failed(_))).count();
            if bad == 0 {
                return format!("{done} of {total} devices up to date");
            }
            return format!("{done} of {total} — {bad} failed");
        }
        if let Some(app) = &self.building {
            return format!("building {app}");
        }
        match &self.current {
            Some(c) => c.clone(),
            None => "starting…".into(),
        }
    }

    /// Apply one event.
    pub fn apply(&mut self, e: &Event) {
        match e {
            Event::Started { steps } => {
                self.steps_total = *steps;
                self.steps_done = 0;
                self.finished = false;
            }
            Event::StepStarted(step) => {
                self.current = Some(describe(step));
                match step {
                    Step::Enroll { device } => {
                        self.rows.insert(device.clone(), RowState::Enrolling);
                    }
                    Step::Install { app, device } => {
                        self.rows.insert(device.clone(), RowState::Installing(app.clone()));
                    }
                    Step::Build { app, .. } => self.building = Some(app.clone()),
                    Step::Rename { device, .. } => {
                        self.rows.insert(device.clone(), RowState::Renaming);
                    }
                    Step::Refresh { device } => {
                        self.rows.insert(device.clone(), RowState::Refreshing);
                    }
                }
            }
            Event::StepDone(step) => {
                self.steps_done += 1;
                self.current = None;
                match step {
                    Step::Build { .. } => self.building = None,
                    Step::Install { device, .. } => {
                        let n = self.installed.entry(device.clone()).or_insert(0);
                        *n += 1;
                        self.rows.insert(device.clone(), RowState::Done { apps: *n });
                    }
                    // Enrolment, rename and refresh are all stages, not
                    // finish lines: the device goes back to waiting for the
                    // installs that are the actual point.
                    Step::Enroll { device }
                    | Step::Rename { device, .. }
                    | Step::Refresh { device } => {
                        self.rows.insert(device.clone(), RowState::Waiting);
                    }
                }
            }
            Event::StepFailed { step, error } => {
                self.steps_done += 1;
                self.current = None;
                if let Step::Build { .. } = step {
                    self.building = None;
                }
                if let Some(device) = device_of(step) {
                    self.rows.insert(device, RowState::Failed(error.clone()));
                }
            }
            Event::StepSkipped { step, because } => {
                self.steps_done += 1;
                if let Some(device) = device_of(step) {
                    // Do NOT overwrite an existing failure: the first reason is
                    // the true one, and the skips that follow are its
                    // consequences. Showing the last would tell the operator
                    // "build failed" on a device whose real problem was that
                    // it could not be paired.
                    if !matches!(self.rows.get(&device), Some(RowState::Failed(_))) {
                        self.rows.insert(device, RowState::Failed(because.clone()));
                    }
                }
            }
            Event::Finished => {
                self.finished = true;
                self.current = None;
                self.building = None;
            }
        }
    }
}

fn device_of(step: &Step) -> Option<String> {
    match step {
        Step::Enroll { device } => Some(device.clone()),
        Step::Rename { device, .. } => Some(device.clone()),
        Step::Refresh { device } => Some(device.clone()),
        Step::Install { device, .. } => Some(device.clone()),
        Step::Build { .. } => None,
    }
}

fn describe(step: &Step) -> String {
    match step {
        Step::Enroll { device } => format!("pairing with {device}"),
    Step::Rename { device, hostname } => format!("renaming {device} to {hostname}"),
    Step::Refresh { device } => format!("updating package index on {device}"),
        Step::Build { app, triple } => format!("building {app} for {triple}"),
        Step::Install { app, device } => format!("installing {app} on {device}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reroll::{plan, Device, Enrollment, Reach};

    fn dev(id: &str, enr: Enrollment, reach: Reach) -> Device {
        Device {
            id: id.into(),
            label: id.into(),
            host: format!("{id}.local"),
            triple: "aarch64-unknown-linux-gnu".into(),
            enrollment: enr,
            reach,
            desired_hostname: None,
            needs_refresh: false,
        }
    }

    #[test]
    fn the_bar_does_not_fill_until_the_work_is_actually_done() {
        let mut v = RunView::with_devices(&["pine".to_string()]);
        v.apply(&Event::Started { steps: 2 });
        assert_eq!(v.fraction(), 0.0);
        let build = Step::Build { app: "voiceterm".into(), triple: "arm".into() };
        v.apply(&Event::StepStarted(build.clone()));
        // In flight is NOT done. A bar that sits at 100% while the last copy
        // runs looks broken; one at 50% looks busy.
        assert_eq!(v.fraction(), 0.0);
        v.apply(&Event::StepDone(build));
        assert_eq!(v.fraction(), 0.5);
    }

    #[test]
    fn every_device_has_a_row_before_the_run_reaches_it() {
        // A table that grows as devices are touched makes the operator count
        // phones to notice one is missing.
        let v = RunView::with_devices(&["fp5".to_string(), "pine".to_string()]);
        assert_eq!(v.rows.len(), 2);
        assert!(v.rows.values().all(|r| *r == RowState::Waiting));
        assert_eq!(v.headline(), "starting…");
    }

    #[test]
    fn a_build_is_its_own_line_and_not_blamed_on_a_phone() {
        let mut v = RunView::with_devices(&["pine".to_string()]);
        v.apply(&Event::Started { steps: 2 });
        v.apply(&Event::StepStarted(Step::Build {
            app: "voiceterm".into(), triple: "arm".into() }));
        assert_eq!(v.headline(), "building voiceterm");
        // The phone is still waiting — the slow part is not its doing.
        assert_eq!(v.rows["pine"], RowState::Waiting);
    }

    #[test]
    fn a_device_counts_its_apps_up_rather_than_flicking_back_to_one() {
        let mut v = RunView::with_devices(&["pine".to_string()]);
        v.apply(&Event::Started { steps: 2 });
        for app in ["voiceterm", "snapclone"] {
            let s = Step::Install { app: app.into(), device: "pine".into() };
            v.apply(&Event::StepStarted(s.clone()));
            v.apply(&Event::StepDone(s));
        }
        assert_eq!(v.rows["pine"], RowState::Done { apps: 2 });
        assert_eq!(v.rows["pine"].line(), "2 apps installed");
    }

    #[test]
    fn the_first_reason_wins_over_the_skips_it_caused() {
        // Enrolment fails, and the two installs that depended on it are then
        // skipped. The row must keep saying "bad password" — telling the
        // operator "enrolment failed" on the row and then overwriting it with
        // the consequence would send them looking in the wrong place.
        let mut v = RunView::with_devices(&["fp5".to_string()]);
        v.apply(&Event::Started { steps: 3 });
        let enroll = Step::Enroll { device: "fp5".into() };
        v.apply(&Event::StepStarted(enroll.clone()));
        v.apply(&Event::StepFailed { step: enroll, error: "bad password".into() });
        for app in ["voiceterm", "snapclone"] {
            v.apply(&Event::StepSkipped {
                step: Step::Install { app: app.into(), device: "fp5".into() },
                because: "enrolment failed: bad password".into(),
            });
        }
        assert_eq!(v.rows["fp5"], RowState::Failed("bad password".into()));
        assert_eq!(v.fraction(), 1.0, "skipped steps still count as resolved");
    }

    #[test]
    fn enrolling_is_a_stage_not_a_finish_line() {
        let mut v = RunView::with_devices(&["fp5".to_string()]);
        let enroll = Step::Enroll { device: "fp5".into() };
        v.apply(&Event::StepStarted(enroll.clone()));
        assert_eq!(v.rows["fp5"], RowState::Enrolling);
        assert!(v.rows["fp5"].active());
        v.apply(&Event::StepDone(enroll));
        // Paired, but nothing installed yet — Done here would be a green tick
        // on a device with no apps on it.
        assert_eq!(v.rows["fp5"], RowState::Waiting);
    }

    #[test]
    fn a_finished_run_reads_as_a_count_not_a_percentage() {
        let mut v = RunView::with_devices(&["pine".to_string(), "fp5".to_string()]);
        let s = Step::Install { app: "voiceterm".into(), device: "pine".into() };
        v.apply(&Event::StepStarted(s.clone()));
        v.apply(&Event::StepDone(s));
        v.rows.insert("fp5".into(), RowState::Failed("no space left".into()));
        v.apply(&Event::Finished);
        assert_eq!(v.headline(), "1 of 2 — 1 failed");
        assert!(!v.rows["pine"].active(), "nothing is still moving");
    }

    #[test]
    fn it_folds_a_real_plans_events_end_to_end() {
        // The view and the executor agree because they are driven by the same
        // stream — this pins that, rather than the view's idea of one.
        let devices = vec![
            dev("fp5", Enrollment::None, Reach::Online),
            dev("pine", Enrollment::Keyed, Reach::Online),
            dev("old", Enrollment::Keyed, Reach::Offline),
        ];
        let p = plan(&devices, &["voiceterm".to_string()]);
        let ids: Vec<String> = devices.iter().map(|d| d.id.clone()).collect();
        let mut v = RunView::with_devices(&ids);
        for (id, why) in &p.skipped {
            v.rows.insert(id.clone(), RowState::Skipped(*why));
        }
        // Replay a clean run.
        v.apply(&Event::Started { steps: p.steps.len() });
        for step in &p.steps {
            v.apply(&Event::StepStarted(step.clone()));
            v.apply(&Event::StepDone(step.clone()));
        }
        v.apply(&Event::Finished);
        assert_eq!(v.fraction(), 1.0);
        assert_eq!(v.rows["pine"], RowState::Done { apps: 1 });
        assert_eq!(v.rows["fp5"], RowState::Done { apps: 1 });
        assert_eq!(v.rows["old"], RowState::Skipped(Skipped::Offline));
        assert_eq!(v.headline(), "2 of 3 devices up to date");
    }
}
