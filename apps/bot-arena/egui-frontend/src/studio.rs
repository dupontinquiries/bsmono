//! studio — `CorpusStudio`: the corpus-LM pipeline as a live stepper.
//!
//! Pick a dataset, optionally stage it and run your own preprocessing tool,
//! then run six pipeline stages in order. Each shows pending / running / done /
//! failed and streams its output into a per-step log.
//!
//! The GTK version shells out to the compiled v1.5 binaries from a worker
//! thread and streams messages back over a channel. None of that changes: the
//! view does not spawn processes and does not own the thread. What it owns is
//! the SEQUENCE — which stage is next, what a failure means for the ones after
//! it, and what the user is allowed to press.
//!
//! ## A failure stops the run
//!
//! Stage 3 cannot train on a tokenizer stage 1 failed to produce. Continuing
//! would burn minutes producing a second, more confusing failure downstream —
//! and the real error would then be six log panes up. Everything after a
//! failure stays PENDING rather than being marked failed, because they did not
//! fail; they never ran, and saying otherwise sends the user debugging the
//! wrong stage.
//!
//! ## Skipped counts as success
//!
//! Stage 0 is the user's own preprocessing tool and is optional. A stage with
//! no command to run is `Skipped`, and the run continues — which is the GTK
//! code's `StepDone(idx, true) // skipped == ok`. Marking it failed would stop
//! a pipeline for the absence of something nobody asked for.
//!
//! ## Stop is checked between stages, not inside one
//!
//! A running child process is not interrupted mid-stage; the flag is read
//! before each one starts. That is honest about what the worker can actually
//! do — claiming to abort a running `word2vec_train` and then not doing so is
//! worse than a Stop button that says it will finish this stage first.

/// The pipeline. Index 0 is the optional preprocessing tool.
pub const STAGE_NAMES: [&str; 7] = [
    "0 · Preprocess (your tool)",
    "1 · BPE tokenizer",
    "2 · N-gram model",
    "3 · Word2Vec",
    "4 · Sentence pooling",
    "5 · Transitions",
    "6 · tiny_gpt train",
];

/// Where a stage has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StageState {
    #[default]
    Pending,
    Running,
    Done,
    /// Nothing to run — the optional tool was not configured.
    Skipped,
    Failed,
}

impl StageState {
    /// Skipped counts as success: the run continues.
    pub fn ok(self) -> bool { matches!(self, StageState::Done | StageState::Skipped) }
    pub fn finished(self) -> bool { !matches!(self, StageState::Pending | StageState::Running) }

    /// The glyph beside the stage name.
    pub fn glyph(self) -> &'static str {
        match self {
            StageState::Pending => "·",
            StageState::Running => "▶",
            StageState::Done => "✓",
            // Distinct from done: a skipped stage produced nothing, and a tick
            // beside it would claim an artefact that is not there.
            StageState::Skipped => "–",
            StageState::Failed => "✗",
        }
    }
}

/// One stage's state and its log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stage {
    pub state: StageState,
    /// Streamed output, newest last.
    pub log: Vec<String>,
    /// Whether the per-step log pane is open.
    pub expanded: bool,
}

/// The whole run.
#[derive(Debug, Clone, PartialEq)]
pub struct Studio {
    stages: Vec<Stage>,
    /// The stage currently running, if any.
    running: Option<usize>,
    /// Set when the user presses Stop; read between stages.
    stop_requested: bool,
    /// True once a stage has failed — nothing after it runs.
    halted: bool,
}

impl Default for Studio {
    fn default() -> Self {
        Studio { stages: vec![Stage::default(); STAGE_NAMES.len()],
                 running: None, stop_requested: false, halted: false }
    }
}

impl Studio {
    pub fn stages(&self) -> &[Stage] { &self.stages }
    pub fn state(&self, i: usize) -> StageState {
        self.stages.get(i).map(|s| s.state).unwrap_or_default()
    }
    pub fn running(&self) -> Option<usize> { self.running }
    pub fn is_running(&self) -> bool { self.running.is_some() }
    pub fn halted(&self) -> bool { self.halted }

    /// Clear everything for a fresh run.
    pub fn reset(&mut self) {
        *self = Studio::default();
    }

    /// The next stage to run, or `None` when the run is over.
    ///
    /// Returns nothing once halted or stopped: the sequence is the view's job,
    /// and a worker that asked "what next" after a failure would restart the
    /// pipeline from the middle.
    pub fn next_stage(&self) -> Option<usize> {
        if self.halted || self.stop_requested || self.is_running() { return None; }
        self.stages.iter().position(|s| s.state == StageState::Pending)
    }

    /// Mark a stage started.
    pub fn begin(&mut self, i: usize) -> bool {
        if self.halted || self.stop_requested || self.is_running() { return false; }
        let Some(s) = self.stages.get_mut(i) else { return false };
        if s.state != StageState::Pending { return false; }
        s.state = StageState::Running;
        self.running = Some(i);
        true
    }

    /// A line of output from the running stage.
    ///
    /// Routed to the stage that is running rather than to a global log, because
    /// six interleaved logs in one pane is how you lose the one error message
    /// that mattered.
    pub fn log(&mut self, line: &str) {
        if let Some(i) = self.running {
            if let Some(s) = self.stages.get_mut(i) { s.log.push(line.to_string()); }
        }
    }

    /// The worker reports a stage finished.
    ///
    /// A failure HALTS: stage 3 cannot train on a tokenizer stage 1 failed to
    /// produce, and continuing burns minutes producing a more confusing failure
    /// with the real error six panes up.
    pub fn finish(&mut self, i: usize, outcome: StageState) -> bool {
        if self.running != Some(i) { return false; }
        let Some(s) = self.stages.get_mut(i) else { return false };
        s.state = outcome;
        self.running = None;
        if outcome == StageState::Failed {
            self.halted = true;
            // The failing stage's log opens by itself. The user is about to go
            // looking for it, and it is the only pane that can explain the stop.
            s.expanded = true;
        }
        true
    }

    /// The user pressed Stop. Checked BETWEEN stages: a running child is not
    /// interrupted, and claiming otherwise would be a lie about what the worker
    /// can do.
    pub fn request_stop(&mut self) { self.stop_requested = true; }
    pub fn stop_requested(&self) -> bool { self.stop_requested }

    /// Can the run be started?
    pub fn can_start(&self) -> bool {
        !self.is_running() && !self.halted && self.next_stage().is_some()
    }

    /// Progress across the whole pipeline, 0..=1, counting finished stages.
    pub fn progress(&self) -> f32 {
        let done = self.stages.iter().filter(|s| s.state.finished()).count();
        done as f32 / self.stages.len() as f32
    }

    /// A one-line summary for the header.
    pub fn summary(&self) -> String {
        if self.halted {
            let failed = self.stages.iter().position(|s| s.state == StageState::Failed);
            // Names the stage. "Failed" alone leaves the user opening six panes
            // to find out which.
            return match failed {
                Some(i) => format!("Stopped at {}", STAGE_NAMES[i]),
                None => "Stopped".into(),
            };
        }
        if let Some(i) = self.running { return format!("Running {}", STAGE_NAMES[i]); }
        if self.stop_requested { return "Stopping after this stage".into(); }
        if self.stages.iter().all(|s| s.state.finished()) { return "Complete".into(); }
        "Ready".into()
    }

    pub fn toggle_log(&mut self, i: usize) {
        if let Some(s) = self.stages.get_mut(i) { s.expanded = !s.expanded; }
    }
}

/// What the panel asks the host to do. The view spawns nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudioAction { Start, Stop, Reset }

/// Draw the stepper. Returns an action when a button is pressed.
pub fn studio_ui(ui: &mut egui::Ui, s: &mut Studio,
                 c: &uikit_egui::PaletteColors) -> Option<StudioAction> {
    let mut act = None;
    ui.horizontal(|ui| {
        ui.strong(s.summary());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(s.can_start(), egui::Button::new("Run")).clicked() {
                act = Some(StudioAction::Start);
            }
            // Stop is only live while something is running, and says what it
            // will actually do.
            if ui.add_enabled(s.is_running() && !s.stop_requested(),
                              egui::Button::new("Stop"))
                .on_hover_text("Finishes the current stage first").clicked() {
                act = Some(StudioAction::Stop);
            }
            if ui.add_enabled(!s.is_running(), egui::Button::new("Reset")).clicked() {
                act = Some(StudioAction::Reset);
            }
        });
    });

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, c.bg_raised);
    let mut fill = rect;
    fill.set_width(rect.width() * s.progress());
    ui.painter().rect_filled(fill, 2.0, if s.halted() { c.danger } else { c.accent });

    for (i, name) in STAGE_NAMES.iter().enumerate() {
        let st = s.state(i);
        ui.horizontal(|ui| {
            let col = match st {
                StageState::Failed => c.danger,
                StageState::Running => c.accent,
                StageState::Done => c.success,
                _ => c.text_weak,
            };
            ui.colored_label(col, st.glyph());
            if ui.add(egui::Button::new(*name).frame(false)).clicked() { s.toggle_log(i); }
        });
        if s.stages()[i].expanded {
            let log = s.stages()[i].log.clone();
            egui::ScrollArea::vertical().id_salt(("studio_log", i)).max_height(120.0)
                .show(ui, |ui| {
                    for line in &log {
                        ui.colored_label(c.text_weak, egui::RichText::new(line).monospace());
                    }
                    if log.is_empty() { ui.colored_label(c.text_weak, "(no output)"); }
                });
        }
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_stage(s: &mut Studio, i: usize, outcome: StageState) {
        assert!(s.begin(i), "stage {i} should start");
        s.finish(i, outcome);
    }

    #[test]
    fn a_fresh_studio_starts_at_the_first_stage() {
        let s = Studio::default();
        assert_eq!(s.next_stage(), Some(0));
        assert!(s.can_start());
        assert_eq!(s.summary(), "Ready");
        assert_eq!(s.progress(), 0.0);
    }

    #[test]
    fn stages_run_in_order() {
        let mut s = Studio::default();
        for i in 0..STAGE_NAMES.len() {
            assert_eq!(s.next_stage(), Some(i));
            run_stage(&mut s, i, StageState::Done);
        }
        assert_eq!(s.next_stage(), None);
        assert_eq!(s.summary(), "Complete");
        assert_eq!(s.progress(), 1.0);
    }

    #[test]
    fn a_failure_halts_the_run() {
        // Stage 3 cannot train on a tokenizer stage 1 failed to produce, and
        // continuing burns minutes producing a more confusing failure with the
        // real error six panes up.
        let mut s = Studio::default();
        run_stage(&mut s, 0, StageState::Done);
        run_stage(&mut s, 1, StageState::Failed);
        assert!(s.halted());
        assert_eq!(s.next_stage(), None);
        assert!(!s.can_start());
    }

    #[test]
    fn stages_after_a_failure_stay_pending_rather_than_failed() {
        // They did not fail; they never ran. Marking them failed sends the user
        // debugging the wrong stage.
        let mut s = Studio::default();
        run_stage(&mut s, 0, StageState::Done);
        run_stage(&mut s, 1, StageState::Failed);
        for i in 2..STAGE_NAMES.len() {
            assert_eq!(s.state(i), StageState::Pending, "stage {i}");
        }
    }

    #[test]
    fn the_failing_stages_log_opens_by_itself() {
        // The user is about to go looking for it, and it is the only pane that
        // can explain the stop.
        let mut s = Studio::default();
        s.begin(0);
        s.log("boom");
        s.finish(0, StageState::Failed);
        assert!(s.stages()[0].expanded);
    }

    #[test]
    fn the_summary_names_the_stage_that_failed() {
        // "Failed" alone leaves the user opening six panes to find out which.
        let mut s = Studio::default();
        run_stage(&mut s, 0, StageState::Done);
        run_stage(&mut s, 1, StageState::Failed);
        assert!(s.summary().contains("BPE"), "got {:?}", s.summary());
    }

    #[test]
    fn a_skipped_stage_is_a_success_and_the_run_continues() {
        // Stage 0 is the user's own tool and is optional; failing the pipeline
        // for the absence of something nobody asked for would be absurd.
        let mut s = Studio::default();
        run_stage(&mut s, 0, StageState::Skipped);
        assert!(!s.halted());
        assert_eq!(s.next_stage(), Some(1));
        assert!(StageState::Skipped.ok());
    }

    #[test]
    fn a_skipped_stage_does_not_claim_an_artefact_it_never_produced() {
        // A tick beside it would say a file exists that does not.
        assert_ne!(StageState::Skipped.glyph(), StageState::Done.glyph());
    }

    #[test]
    fn stop_is_honoured_between_stages_not_inside_one() {
        // Claiming to abort a running word2vec_train and then not doing so is
        // worse than a button that says it will finish this stage first.
        let mut s = Studio::default();
        s.begin(0);
        s.request_stop();
        assert!(s.is_running(), "the current stage keeps going");
        assert_eq!(s.summary(), "Running 0 · Preprocess (your tool)");
        s.finish(0, StageState::Done);
        assert_eq!(s.next_stage(), None, "and nothing starts after it");
        assert_eq!(s.summary(), "Stopping after this stage");
    }

    #[test]
    fn a_stage_cannot_start_while_another_is_running() {
        // Two stages writing the same output directory is a corrupt run that
        // looks like a bad model.
        let mut s = Studio::default();
        assert!(s.begin(0));
        assert!(!s.begin(1));
        assert_eq!(s.running(), Some(0));
    }

    #[test]
    fn a_stage_cannot_be_started_twice() {
        let mut s = Studio::default();
        assert!(s.begin(0));
        s.finish(0, StageState::Done);
        assert!(!s.begin(0), "already done");
    }

    #[test]
    fn a_finish_for_a_stage_that_is_not_running_is_refused() {
        // A late message from an aborted worker must not overwrite the state of
        // a run that has moved on.
        let mut s = Studio::default();
        s.begin(0);
        assert!(!s.finish(3, StageState::Failed));
        assert_eq!(s.state(3), StageState::Pending);
        assert!(!s.halted());
    }

    #[test]
    fn output_goes_to_the_stage_that_produced_it() {
        // Six interleaved logs in one pane is how you lose the one error
        // message that mattered.
        let mut s = Studio::default();
        s.begin(0);
        s.log("from stage 0");
        s.finish(0, StageState::Done);
        s.begin(1);
        s.log("from stage 1");
        assert_eq!(s.stages()[0].log, vec!["from stage 0".to_string()]);
        assert_eq!(s.stages()[1].log, vec!["from stage 1".to_string()]);
    }

    #[test]
    fn output_with_nothing_running_is_dropped_rather_than_misfiled() {
        let mut s = Studio::default();
        s.log("stray line");
        assert!(s.stages().iter().all(|st| st.log.is_empty()));
    }

    #[test]
    fn progress_counts_finished_stages_including_skipped_ones() {
        let mut s = Studio::default();
        run_stage(&mut s, 0, StageState::Skipped);
        run_stage(&mut s, 1, StageState::Done);
        assert!((s.progress() - 2.0 / 7.0).abs() < 0.001);
    }

    #[test]
    fn a_halted_run_can_be_reset_and_started_again() {
        let mut s = Studio::default();
        run_stage(&mut s, 0, StageState::Failed);
        assert!(!s.can_start());
        s.reset();
        assert!(s.can_start());
        assert_eq!(s.next_stage(), Some(0));
        assert!(s.stages()[0].log.is_empty(), "and the old logs are gone");
    }

    #[test]
    fn the_stage_names_are_numbered_so_a_log_line_can_be_placed() {
        // "[3 · Word2Vec] failed" is findable in a scrollback; "failed" is not.
        assert_eq!(STAGE_NAMES.len(), 7);
        for (i, n) in STAGE_NAMES.iter().enumerate() {
            assert!(n.starts_with(&i.to_string()), "{n} should lead with its index");
        }
    }

    #[test]
    fn toggling_a_log_pane_does_not_disturb_the_run() {
        let mut s = Studio::default();
        s.begin(2);
        s.toggle_log(5);
        assert!(s.stages()[5].expanded);
        assert_eq!(s.running(), Some(2));
    }
}
