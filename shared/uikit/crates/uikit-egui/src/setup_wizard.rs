//! setup_wizard — egui port of `qml/SetupWizard.qml`.
//!
//! The model setup on-ramp: a four-phase machine — **Intro** (no model yet) → **Browse** (pick/download
//! a GGUF, via the ported [`crate::model_picker`]) → **Downloading** (progress) → **Done** (a model is
//! active). It opens straight to `Done` when a model is already active. The phase transitions are pure
//! and unit-tested; each phase's rendering is egui.

use crate::model_picker::{ModelAction, ModelPicker};
use egui::RichText;

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
    /// Dismiss the wizard. The HOST closes it; the wizard does not
    /// change phase, so reopening lands where it was — a download hidden
    /// mid-flight comes back to Downloading rather than restarting at
    /// Intro, which is what "continues in the background" promises.
    Close,
    /// Fetch the speech models (~88 MB, recommended pair).
    DownloadVoice,
    /// Turn on the Ollama fallback for a machine that cannot run a
    /// local GGUF.
    EnableOllama,
}

/// The pure phase transition. Returns the next phase for `event` from `current`. Unknown/no-op
/// combinations keep the current phase.
pub fn next_phase(current: Phase, event: &WizardEvent) -> Phase {
    match (current, event) {
        (Phase::Intro, WizardEvent::Browse) => Phase::Browse,
        (Phase::Browse, WizardEvent::Download(_)) => Phase::Downloading,
        (Phase::Downloading, WizardEvent::DownloadDone) => Phase::Done,
        (Phase::Done, WizardEvent::Change) => Phase::Browse,
        // Close deliberately has NO transition: dismissing is the host
        // hiding the view, not the wizard forgetting what it was doing.
        _ => current,
    }
}

/// The wizard.
pub struct SetupWizard {
    pub phase: Phase,
    pub picker: ModelPicker,
    /// Active model name, when one is set.
    pub active_model: Option<String>,
    /// Speech models present on disk. The wizard sets up the VOICE half
    /// of voiceterm as well as the text half, and the port had no
    /// mention of speech at all — so this was unreachable from the one
    /// screen whose job is setup.
    pub voice_installed: bool,
    /// True while the speech download runs, so the button can say so
    /// rather than looking unresponsive on an ~88 MB fetch.
    pub voice_downloading: bool,
    /// Ollama fallback enabled. The escape hatch for a machine that
    /// cannot run a local GGUF at all — without it such a machine
    /// finishes setup with nothing working.
    pub ollama_fallback: bool,
}

impl SetupWizard {
    pub fn new(has_active_model: bool) -> Self {
        Self {
            phase: initial_phase(has_active_model),
            picker: ModelPicker::default(),
            active_model: None,
            // Default FALSE for both: the wizard must not claim voice is
            // installed or Ollama is on until the host says so. Guessing
            // optimistically here would hide the very offers this screen
            // exists to make.
            voice_installed: false,
            voice_downloading: false,
            ollama_fallback: false,
        }
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
                // VOICE INPUT — the other half of setting up voiceterm,
                // and absent from the port entirely. Shown only when the
                // models are missing: an offer to install what is already
                // installed is noise, and the QML gates it the same way.
                if !self.voice_installed {
                    ui.add_space(12.0);
                    ui.separator();
                    ui.label(RichText::new("🎤 Voice input").strong().small());
                    ui.label("Speech models aren't installed yet. Download the recommended pair \
                              (~88 MB) to enable voice.");
                    ui.add_space(4.0);
                    if self.voice_downloading {
                        // An 88 MB fetch with a live-looking button reads
                        // as a click that did nothing.
                        ui.add_enabled(false, egui::Button::new("Downloading voice models…"));
                    } else if ui.button("Download voice models").clicked() {
                        event = Some(WizardEvent::DownloadVoice);
                    }
                }

                // OLLAMA FALLBACK — for a machine that cannot run a local
                // GGUF at all. Without it such a machine finishes setup
                // with nothing working and no hint that anything else
                // exists.
                ui.add_space(12.0);
                ui.separator();
                if self.ollama_fallback {
                    ui.label(RichText::new("Ollama fallback is on.").weak().small());
                } else {
                    ui.label(RichText::new("Already run Ollama? Enable it as a fallback.")
                        .weak().small());
                    if ui.button("Enable Ollama").clicked() {
                        event = Some(WizardEvent::EnableOllama);
                    }
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Browse models →").clicked() {
                        event = Some(WizardEvent::Browse);
                    }
                    // "Later" rather than "Close" while setup is still
                    // required, matching the QML: the word tells you
                    // whether you are postponing something or finishing
                    // it, and those are different promises.
                    let dismiss = if self.active_model.is_none() { "Later" } else { "Close" };
                    if ui.button(dismiss).clicked() {
                        event = Some(WizardEvent::Close);
                    }
                });
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
                // A WAY OUT OF A MULTI-GIGABYTE DOWNLOAD. The QML has
                // this and the port did not: "You can keep using the app
                // — this continues in the background" plus a Hide
                // button. Being unable to leave the screen is worst
                // exactly here, during the longest phase, and it is also
                // the moment the reassurance matters — without the line,
                // Hide reads like it might cancel the download.
                ui.add_space(8.0);
                ui.weak("You can keep using the app — this continues in the background.");
                ui.add_space(4.0);
                if ui.button("Hide").clicked() {
                    event = Some(WizardEvent::Close);
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

// The `Weak` shim that used to live here was dead and had always been dead: egui::Ui has an
// inherent `weak()`, and inherent methods win over trait methods, so the `ui.weak(..)` call above
// has always resolved to egui's. dead_code caught it only because the trait lost that race. The
// sibling in model_picker.rs avoids the collision by naming its helper `weak_label_compat`, which
// is why that one is actually reachable.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_and_ollama_do_not_move_the_model_setup_along() {
        // Both are SIDE offers on the Intro screen, not steps in the
        // model flow. Advancing the phase for them would skip the person
        // past choosing a model because they enabled a fallback, which is
        // the opposite of what they asked for.
        for e in [WizardEvent::DownloadVoice, WizardEvent::EnableOllama] {
            assert_eq!(next_phase(Phase::Intro, &e), Phase::Intro, "{e:?}");
            assert_eq!(next_phase(Phase::Downloading, &e), Phase::Downloading, "{e:?}");
        }
    }

    #[test]
    fn hiding_a_download_keeps_the_phase_so_it_resumes_where_it_was() {
        // "You can keep using the app — this continues in the
        // background" is a promise: reopening must not restart the
        // wizard at Intro and lose a half-finished multi-gigabyte
        // download's context.
        assert_eq!(next_phase(Phase::Downloading, &WizardEvent::Close), Phase::Downloading);
        assert_eq!(next_phase(Phase::Intro, &WizardEvent::Close), Phase::Intro);
        assert_eq!(next_phase(Phase::Done, &WizardEvent::Close), Phase::Done);
    }

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
