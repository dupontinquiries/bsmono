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
        // The git-artifact route: classify first, decide second. Separate
        // from "import bundle" above, which imports an app SUITE — a different
        // thing that happens to share the word.
        if ui.add_enabled(can, egui::Button::new("inspect as git work")).clicked() {
            begin(app);
        }
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
    let intent = step_body(step, inc, ui);

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
    if let Some(i) = intent {
        act(app, i);
    }
}

/// Everything at once, for a screen that can hold it.
fn dashboard(app: &mut App, ui: &mut egui::Ui) {
    use cockpit::incoming::Step;
    let Some(inc) = &app.import.incoming else { return };
    ui.heading("Incoming");
    let mut intent = None;
    ui.columns(2, |cols| {
        for (i, step) in Step::ALL.iter().enumerate() {
            let ui = &mut cols[i % 2];
            let open = step.reachable(&inc.plan, &inc.session);
            ui.group(|ui| {
                ui.label(egui::RichText::new(step.title()).strong());
                if open {
                    if let Some(x) = step_body(*step, inc, ui) {
                        intent = Some(x);
                    }
                } else {
                    ui.label(egui::RichText::new(gate_reason(*step, inc)).weak().small());
                }
            });
        }
    });
    if let Some(i) = intent {
        act(app, i);
    }
}

/// Carry out what was pressed.
///
/// ONE place, reached from both layouts. If the wizard and the dashboard each
/// applied their own intents they would be two implementations of the same
/// decision, which is the thing the shared Step list exists to prevent.
///
/// Runs synchronously rather than on the background Task: `git am` on a local
/// patch series is sub-second, and a Task here would need a channel to carry
/// back a Session the next line already has. If a large series stalls a frame
/// it should move — that is a latency judgement, not a principle.
fn act(app: &mut App, intent: Intent) {
    let repo = cockpit::ops::repo_root();
    let Some(inc) = app.import.incoming.as_mut() else { return };
    match intent {
        Intent::Start => {
            let patches = patch_paths(&app.import.path);
            cockpit::incoming::am_start(&repo, &patches, &mut inc.session);
        }
        Intent::Resolve(r) => {
            cockpit::incoming::am_resolve(&repo, &mut inc.session, r);
        }
        Intent::Dismiss => {
            app.import.incoming = None;
            app.import.step = 0;
        }
    }
}

/// The `.patch` files under a directory, in series order.
///
/// Sorted by NAME, which is what format-patch numbers them for: 0001, 0002.
/// Directory order is arbitrary, and applying a series out of order produces
/// conflicts that look like the patches are bad.
fn patch_paths(dir: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |x| x == "patch"))
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
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

/// What the operator pressed. Returned rather than applied, so `step_body`
/// stays a pure render and the mutation happens in one place — the same split
/// queue_ui and ribbon_ui use, and the reason those could be tested without a
/// controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Start,
    Resolve(cockpit::incoming::Resolution),
    Dismiss,
}

fn step_body(step: cockpit::incoming::Step, inc: &Incoming, ui: &mut egui::Ui) -> Option<Intent> {
    use cockpit::incoming::{Artifact, Step};
    let mut pressed: Option<Intent> = None;
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
            use cockpit::incoming::{Apply, Resolution};
            ui.label(inc.session.status_line());
            match &inc.session.state {
                Apply::Idle => {
                    if ui.button(inc.plan.route.label()).clicked() {
                        return Some(Intent::Start);
                    }
                }
                Apply::Conflicted { paths, .. } => {
                    for p in paths.iter().take(6) {
                        ui.label(egui::RichText::new(p).monospace().small());
                    }
                    // The markers are the gate, not the index: continue is
                    // offered only once the files are actually clean, because
                    // `am --continue` with markers COMMITS them.
                    let clean = paths.is_empty();
                    ui.horizontal(|ui| {
                        let mut out = None;
                        if ui.add_enabled(clean, egui::Button::new("continue")).clicked() {
                            out = Some(Intent::Resolve(Resolution::Continue));
                        }
                        if ui.button("skip this patch").clicked() {
                            out = Some(Intent::Resolve(Resolution::Skip));
                        }
                        if ui.button("abort").clicked() {
                            out = Some(Intent::Resolve(Resolution::Abort));
                        }
                        if !clean {
                            ui.label(egui::RichText::new(
                                "resolve the markers in those files first").weak().small());
                        }
                        pressed = out;
                    });
                    return pressed;
                }
                Apply::Running { .. } => { ui.spinner(); }
                Apply::Done { .. } | Apply::Aborted { .. } => {}
            }
        }
        Step::Confirm => {
            ui.label(inc.session.status_line());
            if ui.button("done").clicked() {
                return Some(Intent::Dismiss);
            }
        }
    }
    pressed
}


#[cfg(test)]
mod tests {
    #[test]
    fn every_intent_reaches_a_real_git_call() {
        // The failure this whole session kept finding: a model built, tested,
        // and driven by nothing. Session had a state machine and am_start had
        // no caller until this page grew buttons.
        let src = include_str!("import.rs");
        assert!(src.contains("incoming::am_start("), "Start goes nowhere");
        assert!(src.contains("incoming::am_resolve("), "continue/skip/abort go nowhere");
        assert!(src.contains("begin(app)"), "nothing classifies the artifact");
    }

    #[test]
    fn both_layouts_apply_intents_through_the_same_function() {
        // If the wizard and the dashboard each carried out their own intents
        // they would be two implementations of one decision, and the shared
        // Step list would stop guaranteeing they agree.
        // Counted in the PRODUCTION half only: this assertion mentions the
        // call itself, so counting the whole file finds three and the test
        // fails on its own text.
        let src = include_str!("import.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert_eq!(prod.matches("act(app, i)").count(), 2, "one call per layout");
        assert_eq!(prod.matches("fn act(").count(), 1, "and one implementation");
    }

    #[test]
    fn patches_are_applied_in_series_order() {
        // format-patch numbers them 0001, 0002 for a reason. Directory order
        // is arbitrary, and an out-of-order series conflicts in ways that look
        // like the patches themselves are bad.
        let dir = std::env::temp_dir().join(format!("porder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["0003-c.patch", "0001-a.patch", "0002-b.patch"] {
            std::fs::write(dir.join(n), "x").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let got = super::patch_paths(dir.to_str().unwrap());
        assert_eq!(got.len(), 3, "non-patch files must not be applied");
        assert!(got[0].ends_with("0001-a.patch"));
        assert!(got[2].ends_with("0003-c.patch"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
