//! setup_wizard — egui port of `qml/SetupWizard.qml`.
//!
//! The model setup on-ramp: a four-phase machine — **Intro** (no model yet) → **Browse** (pick/download
//! a GGUF, via the ported [`crate::model_picker`]) → **Downloading** (progress) → **Done** (a model is
//! active). It opens straight to `Done` when a model is already active. The phase transitions are pure
//! and unit-tested; each phase's rendering is egui.

use crate::model_picker::{ModelAction, ModelPicker};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Intro,
    Browse,
    Downloading,
    Done,
}

/// Where the wizard opens: straight to `Done` if a model is already active, else `Intro`. Pure.
pub fn initial_phase(has_active_model: bool) -> Phase {
    if has_active_model {
        Phase::Done
    } else {
        Phase::Intro
    }
}

/// Events that move the wizard between phases.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WizardEvent {
    Browse,
    Download(ModelAction),
    DownloadDone,
    Change,
    Finish,
}

/// The pure phase transition. Returns the next phase for `event` from `current`. Unknown/no-op
/// combinations keep the current phase.
pub fn next_phase(current: Phase, event: &WizardEvent) -> Phase {
    match (current, event) {
        (Phase::Intro, WizardEvent::Browse) => Phase::Browse,
        (Phase::Browse, WizardEvent::Download(_)) => Phase::Downloading,
        (Phase::Downloading, WizardEvent::DownloadDone) => Phase::Done,
        (Phase::Done, WizardEvent::Change) => Phase::Browse,
        _ => current,
    }
}

/// The wizard.
pub struct SetupWizard {
    pub phase: Phase,
    pub picker: ModelPicker,
    /// Active model name, when one is set.
    pub active_model: Option<String>,
}

impl SetupWizard {
    pub fn new(has_active_model: bool) -> Self {
        Self { phase: initial_phase(has_active_model), picker: ModelPicker::default(), active_model: None }
    }

    /// Render the current phase. Returns a [`WizardEvent`] the host should act on (start a download,
    /// activate a model, close the wizard); the wizard also advances its own phase for the event.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<WizardEvent> {
        let mut event = None;
        match self.phase {
            Phase::Intro => {
                ui.heading("Set up a local model");
                ui.add_space(8.0);
                ui.label("voiceterm runs a local llama.cpp model for on-device chat and command \
                          understanding. Pick one to download, or drop a .gguf in the models folder.");
                ui.add_space(12.0);
                if ui.button("Browse models →").clicked() {
                    event = Some(WizardEvent::Browse);
                }
            }
            Phase::Browse => {
                ui.heading("Choose a model");
                ui.add_space(6.0);
                if let Some(action) = self.picker.ui(ui) {
                    match &action {
                        ModelAction::Download { .. } => event = Some(WizardEvent::Download(action.clone())),
                        ModelAction::Use(name) => {
                            self.active_model = Some(name.clone());
                            event = Some(WizardEvent::Finish);
                        }
                    }
                }
            }
            Phase::Downloading => {
                ui.heading("Downloading…");
                ui.add_space(8.0);
                ui.add(egui::ProgressBar::new(self.picker.download_percent as f32 / 100.0)
                    .text(format!("{}%", self.picker.download_percent)));
                if !self.picker.download_status.is_empty() {
                    ui.add_space(4.0);
                    ui.weak(&self.picker.download_status);
                }
            }
            Phase::Done => {
                ui.heading("Ready");
                ui.add_space(8.0);
                match &self.active_model {
                    Some(m) => ui.label(format!("Active model: {m}")),
                    None => ui.label("A local model is active."),
                };
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() {
                        event = Some(WizardEvent::Finish);
                    }
                    if ui.button("Change model").clicked() {
                        event = Some(WizardEvent::Change);
                    }
                });
            }
        }
        if let Some(ev) = &event {
            self.phase = next_phase(self.phase, ev);
        }
        event
    }
}

// tiny helper so `ui.weak(text)` reads clearly across egui label helpers
trait Weak {
    fn weak(&mut self, text: &str);
}
impl Weak for egui::Ui {
    fn weak(&mut self, text: &str) {
        self.label(egui::RichText::new(text).weak());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_to_done_only_when_a_model_is_active() {
        assert_eq!(initial_phase(true), Phase::Done);
        assert_eq!(initial_phase(false), Phase::Intro);
    }

    #[test]
    fn phases_advance_intro_browse_downloading_done() {
        let mut p = Phase::Intro;
        p = next_phase(p, &WizardEvent::Browse);
        assert_eq!(p, Phase::Browse);
        p = next_phase(p, &WizardEvent::Download(ModelAction::Download { repo: "r".into(), file: "m.gguf".into() }));
        assert_eq!(p, Phase::Downloading);
        p = next_phase(p, &WizardEvent::DownloadDone);
        assert_eq!(p, Phase::Done);
        // "change model" from Done loops back to Browse
        assert_eq!(next_phase(Phase::Done, &WizardEvent::Change), Phase::Browse);
    }

    #[test]
    fn irrelevant_events_are_noops() {
        assert_eq!(next_phase(Phase::Intro, &WizardEvent::DownloadDone), Phase::Intro);
        assert_eq!(next_phase(Phase::Done, &WizardEvent::Browse), Phase::Done);
    }
}
