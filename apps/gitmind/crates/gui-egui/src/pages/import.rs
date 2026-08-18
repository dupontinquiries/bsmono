//! Import page — pull a repo bundle or a zipped project into the workspace (cockpit::flow::
//! import_bundle / import). Runs on the background Task.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub path: String,
    pub note: String,
    /// The git-artifact flow: a bundle or a patch series someone sent.
    pub incoming: Option<Incoming>,
    /// Which wizard step the phone is on. Ignored on a wide screen, which
    /// shows every step at once.
    pub step: usize,
}

/// A classified artifact and its apply session.
pub struct Incoming {
    pub artifact: cockpit::incoming::Artifact,
    pub plan: cockpit::incoming::Plan,
    pub session: cockpit::incoming::Session,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    // WIDTH DECIDES THE SHAPE, not the content. A phone walks the steps one at
    // a time because that is what fits; a desktop shows them together because
    // the operator can hold the whole plan in view and skip ahead. The step
    // SET is identical — cockpit::incoming::Step — so a decision made on a
    // phone and one made on a desktop cannot diverge.
    let adaptive = uikit_egui::adaptive::Adaptive::detect(ui.ctx());
    if app.import.incoming.is_some() {
        if adaptive.is_compact() {
            wizard(app, ui);
        } else {
            dashboard(app, ui);
        }
        ui.separator();
    }

    ui.heading("Import");
    ui.label(egui::RichText::new("bring a repo bundle or a zipped project into the workspace").weak().small());
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("path");
        ui.text_edit_singleline(&mut app.import.path);
    });
    let can = !app.task.is_running() && !app.import.path.trim().is_empty();
    ui.horizontal(|ui| {
        if ui.add_enabled(can, egui::Button::new("import bundle")).clicked() {
            let p = app.import.path.clone();
            app.task.run(move || match cockpit::flow::import_bundle(&p) {
                Ok(s) => format!("✓ imported suite '{}' ({} apps)", s.name, s.apps.len()),
                Err(e) => format!("error: {e}"),
            });
        }
        if ui.add_enabled(can, egui::Button::new("import zip")).clicked() {
            let p = app.import.path.clone();
            app.task.run(move || match cockpit::flow::import(&p) {
                Ok(i) => format!("✓ imported {} → {}", i.app, i.dir.display()),
                Err(e) => format!("error: {e}"),
            });
        }
    });

    if let Some(r) = app.task.take_result() {
        app.import.note = r;
    }
    if app.task.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("importing…");
        });
        ui.ctx().request_repaint();
    }
    if !app.import.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.import.note).monospace().small());
    }
}


/// Classify whatever is at `path` and start the flow.
///
/// The classification is by SHAPE rather than extension: a `.bundle` that is
/// really a tarball, or a directory of `.patch` files with no zip around them,
/// are both things people actually send.
pub fn begin(app: &mut App) {
    let path = app.import.path.trim().to_string();
    if path.is_empty() {
        return;
    }
    let artifact = classify(&path);
    let have = |sha: &str| cockpit::ops::has_commit(sha);
    let plan = cockpit::incoming::plan(&artifact, &have);
    let subjects = match &artifact {
        cockpit::incoming::Artifact::Patches { count } =>
            (0..*count).map(|i| format!("patch {}", i + 1)).collect(),
        _ => Vec::new(),
    };
    app.import.incoming = Some(Incoming {
        artifact,
        plan,
        session: cockpit::incoming::Session::new(subjects),
    });
    app.import.step = 0;
}

fn classify(path: &str) -> cockpit::incoming::Artifact {
    let p = std::path::Path::new(path);
    if p.is_dir() {
        let n = std::fs::read_dir(p)
            .map(|d| d.flatten().filter(|e| {
                e.path().extension().map_or(false, |x| x == "patch")
            }).count())
            .unwrap_or(0);
        if n > 0 {
            return cockpit::incoming::Artifact::Patches { count: n };
        }
    }
    if path.ends_with(".bundle") {
        let (refs, requires) = cockpit::ops::bundle_refs(path);
        return cockpit::incoming::Artifact::Bundle { refs, requires };
    }
    cockpit::incoming::Artifact::Files { count: 1 }
}

/// One step at a time, with the reason for the gate when a step is closed.
fn wizard(app: &mut App, ui: &mut egui::Ui) {
    use cockpit::incoming::Step;
    let Some(inc) = &app.import.incoming else { return };
    let steps: Vec<Step> = Step::ALL.to_vec();
    let idx = app.import.step.min(steps.len() - 1);
    let step = steps[idx];

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{}/{}", idx + 1, steps.len())).weak().small());
        ui.heading(step.title());
    });
    step_body(step, inc, ui);

    let reachable = |s: Step| s.reachable(&inc.plan, &inc.session);
    ui.horizontal(|ui| {
        if idx > 0 && ui.button("back").clicked() {
            app.import.step = idx - 1;
        }
        let next = steps.get(idx + 1).copied();
        match next {
            Some(n) if reachable(n) => {
                if ui.button(format!("next: {}", n.title())).clicked() {
                    app.import.step = idx + 1;
                }
            }
            Some(n) => {
                // The gate says WHY rather than greying out silently. A
                // disabled button with no reason is the thing that makes a
                // phone UI feel broken.
                ui.add_enabled(false, egui::Button::new(format!("next: {}", n.title())));
                ui.label(egui::RichText::new(gate_reason(n, inc)).weak().small());
            }
            None => {}
        }
    });
}

/// Everything at once, for a screen that can hold it.
fn dashboard(app: &mut App, ui: &mut egui::Ui) {
    use cockpit::incoming::Step;
    let Some(inc) = &app.import.incoming else { return };
    ui.heading("Incoming");
    ui.columns(2, |cols| {
        for (i, step) in Step::ALL.iter().enumerate() {
            let ui = &mut cols[i % 2];
            let open = step.reachable(&inc.plan, &inc.session);
            ui.group(|ui| {
                ui.label(egui::RichText::new(step.title()).strong());
                if open {
                    step_body(*step, inc, ui);
                } else {
                    ui.label(egui::RichText::new(gate_reason(*step, inc)).weak().small());
                }
            });
        }
    });
}

fn gate_reason(step: cockpit::incoming::Step, inc: &Incoming) -> String {
    use cockpit::incoming::{Readiness, Step};
    match step {
        Step::Apply => match &inc.plan.readiness {
            Readiness::MissingBase(m) => format!(
                "{} prerequisite commit(s) absent — nothing to apply", m.len()),
            Readiness::Empty => "the artifact is empty".into(),
            Readiness::Ready => String::new(),
        },
        Step::Confirm => "nothing has been applied yet".into(),
        _ => String::new(),
    }
}

fn step_body(step: cockpit::incoming::Step, inc: &Incoming, ui: &mut egui::Ui) {
    use cockpit::incoming::{Artifact, Step};
    match step {
        Step::Inspect => {
            match &inc.artifact {
                Artifact::Bundle { refs, requires } => {
                    ui.label(format!("bundle · {} ref(s)", refs.len()));
                    for r in refs.iter().take(4) {
                        ui.label(egui::RichText::new(r).monospace().small());
                    }
                    if !requires.is_empty() {
                        ui.label(egui::RichText::new(
                            format!("needs {} base commit(s)", requires.len())).small());
                    }
                }
                Artifact::Patches { count } => { ui.label(format!("{count} patches")); }
                Artifact::Files { count } => { ui.label(format!("{count} file(s)")); }
            }
        }
        Step::Choose => {
            ui.label(inc.plan.route.label());
            ui.label(egui::RichText::new(&inc.plan.because).weak().small());
            if !inc.plan.route.keeps_history() && inc.plan.route != cockpit::incoming::Route::Refuse {
                // Said plainly: squashing a series into one commit is the
                // thing that makes a later bisect useless, and the operator
                // should choose it rather than discover it.
                ui.label(egui::RichText::new("the incoming history will not be kept").small());
            }
            if let Some(ask) = &inc.plan.ask_for {
                ui.label(egui::RichText::new(format!("ask for: {ask}")).small());
            }
        }
        Step::Apply => {
            ui.label(inc.session.status_line());
            if let cockpit::incoming::Apply::Conflicted { paths, .. } = &inc.session.state {
                for p in paths.iter().take(6) {
                    ui.label(egui::RichText::new(p).monospace().small());
                }
                ui.label(egui::RichText::new("continue · skip · abort").small());
            }
        }
        Step::Confirm => { ui.label(inc.session.status_line()); }
    }
}
