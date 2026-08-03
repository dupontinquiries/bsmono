//! fileproc — `MixerFileProcessor` and `StepKey`.
//!
//! ## Processing a file is the correctness preview
//!
//! The section at the bottom of the mixer page runs a stereo WAV through the
//! CURRENT mixer settings and writes OggOpus. It is the same DSP path the
//! realtime thread will use, which is the point: if the file comes out right,
//! the live path is right, and you can check it without a device in the room.
//!
//! ## The output path is suggested, not imposed
//!
//! `song.wav` suggests `song.mixed.opus` — beside the input, so the result is
//! where the user is already looking, and `.mixed.` so it cannot overwrite the
//! source no matter what the extension was. But it is only a suggestion: a
//! typed path wins, and once the user has typed one, changing the input must
//! not silently overwrite their choice.
//!
//! ## Every control is disabled while it runs
//!
//! Not because a second run would crash, but because it would produce a second
//! file the user did not ask for and cannot easily tell from the first. A busy
//! processor with live inputs invites exactly that.
//!
//! ## StepKey is the app's press-key language
//!
//! Spring-scale on press, a ripple from the touch point, an accent flash when
//! it acts. `sources.rs` already cites it as the interaction all the tiles
//! follow; this is the thing it was citing.
//!
//! The flash is separate from the press. A key that only flashes on release
//! cannot show a REPEAT — hold `−` and the volume steps down repeatedly, and
//! each step should flash even though there was one press.

use uikit_egui::motion::ease;

/// Opus bitrate bounds, in kbps, and the slider's step.
pub const BITRATE_MIN: u32 = 32;
pub const BITRATE_MAX: u32 = 192;
pub const BITRATE_STEP: u32 = 8;
pub const BITRATE_DEFAULT: u32 = 128;

/// Suggest an output path from an input path.
///
/// Beside the input so the result is where the user is already looking, and
/// `.mixed.` so it cannot overwrite the source whatever the extension was.
pub fn suggest_output(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() { return String::new(); }
    // Split off the directory, keeping the separator so the result sits beside
    // the source rather than in the working directory.
    let (dir, file) = match input.rfind('/') {
        Some(i) => input.split_at(i + 1),
        None => ("", input),
    };
    let stem = match file.rfind('.') {
        Some(i) if i > 0 => &file[..i],
        _ => file,
    };
    format!("{dir}{stem}.mixed.opus")
}

/// What the processor is doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProcState {
    #[default]
    Idle,
    Running,
    Done(String),
    Failed(String),
}

impl ProcState {
    pub fn running(&self) -> bool { matches!(self, ProcState::Running) }
    /// The status chip's word.
    pub fn chip(&self) -> &'static str {
        match self {
            ProcState::Idle => "ready",
            ProcState::Running => "working",
            ProcState::Done(_) => "done",
            ProcState::Failed(_) => "error",
        }
    }
}

/// What the section asks the host to do. The view runs no DSP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcAction {
    Run { input: String, output: String, bitrate: u32 },
    Preview(String),
    MixAndPlay { input: String, bitrate: u32 },
    LiveStream { input: String, bitrate: u32 },
    Spectrum(String),
}

/// The processor section.
#[derive(Debug, Clone, PartialEq)]
pub struct FileProcessor {
    pub input: String,
    /// Empty means "use the suggestion". Once the user types here it is theirs.
    output: String,
    /// True once the user has edited the output field themselves.
    output_touched: bool,
    pub bitrate: u32,
    pub state: ProcState,
    /// 0..=1 while running.
    pub progress: f32,
}

impl Default for FileProcessor {
    fn default() -> Self {
        FileProcessor { input: String::new(), output: String::new(), output_touched: false,
                        bitrate: BITRATE_DEFAULT, state: ProcState::Idle, progress: 0.0 }
    }
}

impl FileProcessor {
    /// The path that will actually be written.
    pub fn output_path(&self) -> String {
        if self.output_touched && !self.output.trim().is_empty() {
            return self.output.trim().to_string();
        }
        suggest_output(&self.input)
    }

    /// What the output field shows — the user's text, or the suggestion as a
    /// placeholder they can accept by doing nothing.
    pub fn output_field(&self) -> &str { &self.output }

    /// The user typed in the output box. From here the suggestion stops
    /// following the input, or changing the input would silently overwrite a
    /// path they chose deliberately.
    pub fn set_output(&mut self, s: &str) {
        self.output = s.to_string();
        self.output_touched = true;
    }

    /// Clearing the output box hands control back to the suggestion, which is
    /// the only way to undo a typo without retyping the whole path.
    pub fn clear_output(&mut self) {
        self.output.clear();
        self.output_touched = false;
    }

    pub fn set_bitrate(&mut self, kbps: u32) {
        // Snapped to the step so the readout matches the slider's positions;
        // a readout showing 131 next to a slider that can only reach 128 or 136
        // reads as a broken control.
        let clamped = kbps.clamp(BITRATE_MIN, BITRATE_MAX);
        let steps = ((clamped - BITRATE_MIN) as f32 / BITRATE_STEP as f32).round() as u32;
        self.bitrate = (BITRATE_MIN + steps * BITRATE_STEP).min(BITRATE_MAX);
    }

    /// Can anything be run? An empty input has nothing to process, and a run
    /// while running would write a second file the user cannot tell from the
    /// first.
    pub fn can_run(&self) -> bool {
        !self.input.trim().is_empty() && !self.state.running()
    }

    /// Start a run.
    pub fn run(&mut self) -> Option<ProcAction> {
        if !self.can_run() { return None; }
        let action = ProcAction::Run {
            input: self.input.trim().to_string(),
            output: self.output_path(),
            bitrate: self.bitrate,
        };
        self.state = ProcState::Running;
        self.progress = 0.0;
        Some(action)
    }

    /// Any of the other four buttons. Each needs an input and an idle
    /// processor, for the same reason Run does.
    pub fn act(&mut self, which: &str) -> Option<ProcAction> {
        if !self.can_run() { return None; }
        let input = self.input.trim().to_string();
        Some(match which {
            "preview" => ProcAction::Preview(input),
            "mix_play" => ProcAction::MixAndPlay { input, bitrate: self.bitrate },
            "stream" => ProcAction::LiveStream { input, bitrate: self.bitrate },
            "spectrum" => ProcAction::Spectrum(input),
            _ => return None,
        })
    }

    /// The host reports progress.
    pub fn set_progress(&mut self, t: f32) { self.progress = t.clamp(0.0, 1.0); }

    /// The host finished.
    pub fn finish(&mut self, result: Result<String, String>) {
        self.progress = 1.0;
        self.state = match result {
            Ok(path) => ProcState::Done(path),
            Err(e) => ProcState::Failed(e),
        };
    }

    /// The result card's text, or `None` when there is nothing to report.
    pub fn result_text(&self) -> Option<String> {
        match &self.state {
            ProcState::Done(p) => Some(format!("Wrote {p}")),
            // The error is shown verbatim: "no such file" and "unsupported
            // sample rate" need different fixes, and "failed" needs neither.
            ProcState::Failed(e) => Some(e.clone()),
            _ => None,
        }
    }
}

// ── step key ────────────────────────────────────────────────────────────────

/// `Theme` timings for the key's micro-interactions.
pub const PRESS_MS: f32 = 90.0;
pub const RIPPLE_MS: f32 = 320.0;
pub const FLASH_MS: f32 = 140.0;
/// The squash on press.
pub const PRESS_SCALE: f32 = 0.92;

/// bo-suite's press feel: an 8% dip over 90ms, ripple 320ms.
///
/// Shares `uikit_egui::motion::PressFeedback` with snapclone's GlassButton —
/// the two were written independently and came out identical. The constants
/// differ on purpose: a B&O key is firmer than a snapclone one.
pub const FEEL: uikit_egui::motion::PressFeel =
    uikit_egui::motion::PressFeel::new(PRESS_MS, RIPPLE_MS, PRESS_SCALE);

/// A press key: −, +, MUTE. Shared press behaviour, plus the FLASH, which is
/// bo-suite's own — see `flash`.
#[derive(Debug, Clone, PartialEq)]
pub struct StepKey {
    pub label: String,
    /// Pill-shaped rather than round.
    pub wide: bool,
    feedback: uikit_egui::motion::PressFeedback,
    flash_ms: Option<f32>,
}

impl StepKey {
    pub fn new(label: &str, wide: bool) -> Self {
        StepKey { label: label.into(), wide,
                  feedback: uikit_egui::motion::PressFeedback::new(FEEL), flash_ms: None }
    }

    pub fn press(&mut self, x: f32, y: f32) { self.feedback.press(x, y); }
    pub fn release(&mut self) { self.feedback.release(); }

    /// Flash: the key PERFORMED its action.
    ///
    /// NOT in the shared type, and this is the reason the promotion stopped
    /// where it did. Hold "−" and the volume steps down repeatedly; each step
    /// should flash even though there was one press. That is a bo-suite
    /// behaviour tied to key-repeat, and snapclone's glass buttons have no
    /// equivalent — sharing it would have put a field in the kit that one
    /// consumer never sets.
    pub fn flash(&mut self) { self.flash_ms = Some(0.0); }

    pub fn tick(&mut self, dt_ms: f32) {
        self.feedback.tick(dt_ms);
        if let Some(t) = self.flash_ms {
            let t = t + dt_ms;
            self.flash_ms = if t >= FLASH_MS { None } else { Some(t) };
        }
    }

    pub fn scale(&self) -> f32 { self.feedback.scale() }
    pub fn rippling(&self) -> bool { self.feedback.rippling() }
    pub fn flashing(&self) -> bool { self.flash_ms.is_some() }
    pub fn ripple_origin(&self) -> (f32, f32) { self.feedback.ripple_origin() }
    pub fn ripple_t(&self) -> f32 { self.feedback.ripple_t() }
    pub fn ripple_alpha(&self) -> f32 { self.feedback.ripple_alpha(0.3) }
    pub fn flash_alpha(&self) -> f32 {
        self.flash_ms.map(|t| (1.0 - t / FLASH_MS).clamp(0.0, 1.0)).unwrap_or(0.0)
    }
    fn down(&self) -> bool { self.feedback.is_down() }
}

/// Draw a step key. Returns true when it is clicked.
pub fn step_key_ui(ui: &mut egui::Ui, k: &mut StepKey, d: f32,
                   c: &uikit_egui::PaletteColors) -> bool {
    k.tick(ui.input(|i| i.stable_dt) * 1000.0);
    let size = if k.wide { egui::vec2(d * 2.0, d) } else { egui::vec2(d, d) };
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.is_pointer_button_down_on() {
        if !k.down() {
            let p = resp.interact_pointer_pos().unwrap_or(rect.center());
            k.press(p.x - rect.left(), p.y - rect.top());
        }
    } else { k.release(); }

    let r = rect.shrink((1.0 - k.scale()) * d * 0.5);
    let radius = if k.wide { d * 0.5 } else { r.width() * 0.5 };
    let p = ui.painter();
    p.rect_filled(r, radius, c.bg_raised);
    if k.flashing() {
        let a = (k.flash_alpha() * 140.0) as u8;
        p.rect_filled(r, radius, egui::Color32::from_rgba_unmultiplied(
            c.accent.r(), c.accent.g(), c.accent.b(), a));
    }
    if k.rippling() {
        let (ox, oy) = k.ripple_origin();
        let a = (k.ripple_alpha() * 255.0) as u8;
        p.circle_filled(rect.left_top() + egui::vec2(ox, oy), d * k.ripple_t(),
            egui::Color32::from_rgba_unmultiplied(
                c.accent.r(), c.accent.g(), c.accent.b(), a));
    }
    p.text(r.center(), egui::Align2::CENTER_CENTER, &k.label,
           egui::FontId::proportional(d * 0.38), c.text);
    if resp.clicked() { k.flash(); }
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_suggested_output_sits_beside_the_input() {
        // So the result is where the user is already looking.
        assert_eq!(suggest_output("/music/song.wav"), "/music/song.mixed.opus");
        assert_eq!(suggest_output("song.wav"), "song.mixed.opus");
    }

    #[test]
    fn the_suggestion_cannot_overwrite_the_source_whatever_the_extension() {
        // ".mixed." is what guarantees it, not the ".opus".
        assert_eq!(suggest_output("/m/song.opus"), "/m/song.mixed.opus");
        assert_eq!(suggest_output("/m/song"), "/m/song.mixed.opus");
        assert_ne!(suggest_output("/m/song.opus"), "/m/song.opus");
    }

    #[test]
    fn a_dotfile_is_not_mistaken_for_an_extension() {
        assert_eq!(suggest_output("/m/.hidden"), "/m/.hidden.mixed.opus");
    }

    #[test]
    fn an_empty_input_suggests_nothing() {
        assert_eq!(suggest_output(""), "");
        assert_eq!(suggest_output("   "), "");
    }

    #[test]
    fn a_typed_output_wins_and_stops_following_the_input() {
        // Otherwise changing the input silently overwrites a path the user
        // chose deliberately.
        let mut f = FileProcessor::default();
        f.input = "/m/a.wav".into();
        assert_eq!(f.output_path(), "/m/a.mixed.opus");
        f.set_output("/elsewhere/final.opus");
        f.input = "/m/b.wav".into();
        assert_eq!(f.output_path(), "/elsewhere/final.opus");
    }

    #[test]
    fn clearing_the_output_box_hands_control_back_to_the_suggestion() {
        // The only way to undo a typo without retyping the whole path.
        let mut f = FileProcessor::default();
        f.input = "/m/a.wav".into();
        f.set_output("/typo");
        f.clear_output();
        assert_eq!(f.output_path(), "/m/a.mixed.opus");
    }

    #[test]
    fn the_bitrate_snaps_to_the_sliders_own_steps() {
        // A readout showing 131 next to a slider that can only reach 128 or 136
        // reads as a broken control.
        let mut f = FileProcessor::default();
        f.set_bitrate(131);
        assert_eq!(f.bitrate, 128);
        f.set_bitrate(133);
        assert_eq!(f.bitrate, 136);
        assert_eq!((f.bitrate - BITRATE_MIN) % BITRATE_STEP, 0);
    }

    #[test]
    fn the_bitrate_is_clamped_to_its_range() {
        let mut f = FileProcessor::default();
        f.set_bitrate(5);
        assert_eq!(f.bitrate, BITRATE_MIN);
        f.set_bitrate(9999);
        assert_eq!(f.bitrate, BITRATE_MAX);
    }

    #[test]
    fn nothing_runs_without_an_input() {
        let mut f = FileProcessor::default();
        assert!(!f.can_run());
        assert_eq!(f.run(), None);
        assert_eq!(f.act("preview"), None);
        f.input = "  ".into();
        assert!(!f.can_run(), "whitespace is not a path");
    }

    #[test]
    fn nothing_runs_while_something_is_running() {
        // Not because a second run would crash, but because it would produce a
        // second file the user did not ask for and cannot tell from the first.
        let mut f = FileProcessor::default();
        f.input = "/m/a.wav".into();
        assert!(f.run().is_some());
        assert!(f.state.running());
        assert_eq!(f.run(), None);
        assert_eq!(f.act("stream"), None);
    }

    #[test]
    fn a_run_carries_the_resolved_output_and_the_current_bitrate() {
        let mut f = FileProcessor::default();
        f.input = "/m/a.wav".into();
        f.set_bitrate(96);
        assert_eq!(f.run(), Some(ProcAction::Run {
            input: "/m/a.wav".into(), output: "/m/a.mixed.opus".into(), bitrate: 96 }));
    }

    #[test]
    fn the_four_side_actions_each_carry_what_they_need() {
        let mut f = FileProcessor::default();
        f.input = "/m/a.wav".into();
        assert_eq!(f.act("preview"), Some(ProcAction::Preview("/m/a.wav".into())));
        assert_eq!(f.act("spectrum"), Some(ProcAction::Spectrum("/m/a.wav".into())));
        assert!(matches!(f.act("mix_play"), Some(ProcAction::MixAndPlay { .. })));
        assert!(matches!(f.act("stream"), Some(ProcAction::LiveStream { .. })));
        assert_eq!(f.act("nonsense"), None);
    }

    #[test]
    fn the_status_chip_names_each_state() {
        let mut f = FileProcessor::default();
        assert_eq!(f.state.chip(), "ready");
        f.input = "/m/a.wav".into();
        f.run();
        assert_eq!(f.state.chip(), "working");
        f.finish(Ok("/m/a.mixed.opus".into()));
        assert_eq!(f.state.chip(), "done");
        f.finish(Err("no such file".into()));
        assert_eq!(f.state.chip(), "error");
    }

    #[test]
    fn an_error_is_shown_verbatim_rather_than_as_failed() {
        // "no such file" and "unsupported sample rate" need different fixes,
        // and "failed" needs neither.
        let mut f = FileProcessor::default();
        f.finish(Err("unsupported sample rate: 44100".into()));
        assert_eq!(f.result_text().as_deref(), Some("unsupported sample rate: 44100"));
    }

    #[test]
    fn a_success_names_the_file_it_wrote() {
        let mut f = FileProcessor::default();
        f.finish(Ok("/m/a.mixed.opus".into()));
        assert_eq!(f.result_text().as_deref(), Some("Wrote /m/a.mixed.opus"));
    }

    #[test]
    fn an_idle_processor_reports_nothing() {
        assert_eq!(FileProcessor::default().result_text(), None);
    }

    #[test]
    fn progress_is_clamped() {
        let mut f = FileProcessor::default();
        f.set_progress(5.0);
        assert_eq!(f.progress, 1.0);
        f.set_progress(-1.0);
        assert_eq!(f.progress, 0.0);
    }

    #[test]
    fn finishing_frees_the_controls_again() {
        let mut f = FileProcessor::default();
        f.input = "/m/a.wav".into();
        f.run();
        assert!(!f.can_run());
        f.finish(Ok("/m/a.mixed.opus".into()));
        assert!(f.can_run(), "a second run is fine once the first is done");
    }

    // ── step key ────────────────────────────────────────────────────────────

    #[test]
    fn the_key_squashes_on_press_and_springs_back() {
        let mut k = StepKey::new("−", false);
        assert_eq!(k.scale(), 1.0);
        k.press(5.0, 5.0);
        k.tick(PRESS_MS);
        assert!((k.scale() - PRESS_SCALE).abs() < 0.01);
        k.release();
        k.tick(PRESS_MS);
        assert!((k.scale() - 1.0).abs() < 0.01);
    }

    #[test]
    fn the_ripple_blooms_from_the_touch_point() {
        let mut k = StepKey::new("+", false);
        k.press(3.0, 17.0);
        assert_eq!(k.ripple_origin(), (3.0, 17.0));
        assert!(k.rippling());
    }

    #[test]
    fn the_flash_is_separate_from_the_press_so_a_repeat_can_show() {
        // Hold "−" and the volume steps down repeatedly; each step should flash
        // even though there was one press. A key that only flashes on release
        // cannot express that.
        let mut k = StepKey::new("−", false);
        k.press(0.0, 0.0);
        assert!(!k.flashing(), "pressing is not acting");
        k.flash();
        assert!(k.flashing());
        k.tick(FLASH_MS);
        assert!(!k.flashing());
        // Still held, and it can flash again.
        k.flash();
        assert!(k.flashing());
    }

    #[test]
    fn the_flash_fades_rather_than_cutting_out() {
        let mut k = StepKey::new("+", false);
        k.flash();
        let a0 = k.flash_alpha();
        k.tick(FLASH_MS / 2.0);
        assert!(k.flash_alpha() < a0 && k.flash_alpha() > 0.0);
    }

    #[test]
    fn the_ripple_expands_while_fading() {
        let mut k = StepKey::new("+", false);
        k.press(5.0, 5.0);
        k.tick(RIPPLE_MS * 0.2);
        let (t1, a1) = (k.ripple_t(), k.ripple_alpha());
        k.tick(RIPPLE_MS * 0.4);
        assert!(k.ripple_t() > t1);
        assert!(k.ripple_alpha() < a1);
    }

    #[test]
    fn the_ripple_outlives_the_release() {
        // Otherwise a quick tap produces no visible feedback.
        let mut k = StepKey::new("+", false);
        k.press(5.0, 5.0);
        k.tick(20.0);
        k.release();
        k.tick(20.0);
        assert!(k.rippling());
    }

    #[test]
    fn a_settled_key_reports_nothing_running() {
        let k = StepKey::new("MUTE", true);
        assert!(!k.rippling() && !k.flashing());
        assert_eq!(k.ripple_alpha(), 0.0);
        assert_eq!(k.flash_alpha(), 0.0);
        assert!(k.wide, "MUTE is a pill");
    }
}
