//! voice_popup — the voice dialog, built as an extension of the existing
//! command-palette/overlay machinery, not a bespoke window.
//!
//! ## Why this is a `Surface`, not a `Window`
//!
//! Every other QML popup in this app (Settings, Favorites, the command
//! palette itself) already ported onto `overlay::show`/`Surface` — one
//! table (`overlay::form`) maps each `Surface` to its real QML size and
//! placement, cited against the actual QML text and pinned by
//! `qml_still_says_what_the_table_copies` so drift shows up as a named
//! test failure rather than silent disagreement. Building voice as a
//! fifth bespoke `egui::Window` would have meant a second animation
//! system, a second backdrop, a second Esc/click-outside handler — all
//! already solved once, correctly, for every other popup. `Surface::Voice`
//! (`surface.rs`) plus a `Voice => Form::CenterPopup { .. }` row
//! (`overlay.rs`), cited against `voicePopup`'s real QML width
//! (`Math.min(root.width - 40, 480)`, main.qml:18727), is the whole cost
//! of reusing it.
//!
//! `self.surfaces` (the app's existing open/close tracker) is therefore
//! the source of truth for whether the popup is ON SCREEN — this module
//! does not duplicate that with its own visibility flag. What this module
//! owns is [`Stage`]: what is happening WHILE it is open (listening,
//! transcribing, done, error), which has no QML/`Surface` equivalent
//! because that state lived in `voicePopup`'s own `property bool
//! recording` / `property bool processing` in the Qt build.
//!
//! ## The visual language, read from the QML rather than invented
//!
//! `qml/main.qml`'s `voicePopup` (~line 18700) draws: a 44px round badge
//! (mic glyph while idle-with-a-result, red pulsing circle while
//! recording, spinning glyph while processing), a two-line header (bold
//! state title + smaller instruction subtitle, both text CHANGING with
//! state rather than the badge alone carrying it), then, only while
//! recording, a prominent "Done speaking" + smaller "Cancel" row. That
//! shape — badge, two-line header, state-gated action row — is what
//! [`VoicePopup::draw`] reproduces, in egui terms, rather than the plain
//! label-plus-two-buttons layout an earlier, un-reviewed draft of this
//! module had.
//!
//! ## Scope of this cut
//!
//! Manual trigger only — NOT wake-word triggered. `voice::pop_wake_event()`
//! is voiceterm's own `wake_gate`-backed function, outside
//! `voice-engine-kit`'s core (`maintenance/C18_TODO.md` sections 13/18/19).
//! Wiring "a wake fired, open the popup already listening" is a second,
//! deliberate step once this simpler path is proven on screen.
//!
//! The password gate (`security::should_suppress_capture()` in the Qt
//! build) is taken as a caller-supplied bool to [`VoicePopup::open`] rather
//! than read from voiceterm's `security` module directly — this module
//! does not need to know HOW password mode is detected, only whether it
//! is active; `voiceterm_egui.rs` already tracks this independently as
//! `self.password_mode`.
//!
//! The wake-suppression rule (suppress wake detection while this popup is
//! open, so a wake word spoken INTO the popup's own utterance does not
//! also fire a second competing capture) is the CALLER's responsibility,
//! for the same reason: `voice-engine-kit` has no hook for it (wake
//! detection is not in its core), so whatever voiceterm-side
//! `set_wake_suppressed` exists has to be called from `voiceterm_egui.rs`
//! around this popup's open/close, not from inside a crate that cannot
//! see that function.

use voice_engine_kit::{begin_capture, cancel_recording, finish_recording, listen_with_audio};

/// What the popup is doing right now. Not `Surface`/`self.surfaces`'
/// concern (that only knows open vs. closed) — this is the Qt build's
/// `recording`/`processing` pair plus the two terminal states QML
/// expressed as `intentKind === "error"` / a populated result string.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Capture running, waiting for VAD or a manual Finish.
    Listening,
    /// VAD stopped; the background thread is transcribing.
    Transcribing,
    /// A transcript is ready and shown.
    Done { text: String },
    /// A real failure (not a cancellation — see `tick`'s classification).
    Error { message: String },
}

/// State this module owns while the popup is open. Does not track
/// open/closed itself — see the module doc for why `self.surfaces` is
/// the source of truth for that.
pub struct VoicePopup {
    pub stage: Stage,
    /// The `Done` text has been handed to the app once. The popup keeps
    /// showing it; the app must not act on it twice.
    delivered: bool,
    /// Set when a future wake-open path starts this popup already
    /// listening, so the header can say "(wake)" the way QML's
    /// `openedByWake` does. Not set anywhere yet (manual-trigger only,
    /// this cut) — the field exists now so that path does not need a
    /// second state shape bolted on afterward.
    pub opened_by_wake: bool,
    pending: std::sync::Arc<std::sync::Mutex<ListenPending>>,
}

/// `running` distinguishes "haven't started" from "running but nothing
/// back yet"; `result` is `voice_engine_kit::listen_with_audio`'s real
/// return type, `(String, Vec<f32>)` — confirmed by reading that
/// function's actual signature before using it, not assumed to be a
/// `Result` the way an earlier, un-reviewed draft of this module had it.
#[derive(Default)]
struct ListenPending {
    running: bool,
    result: Option<(String, Vec<f32>)>,
}

impl Clone for ListenPending {
    // Both fields are trivially Clone (a bool, an Option<(String,
    // Vec<f32>)>) so #[derive] would work identically — spelled out only
    // because `tick()` clones the whole struct's worth of audio samples
    // every frame while a result is pending review. Fine for one
    // utterance's worth of f32s, freed once `stage` moves past `Done`;
    // would be a real cost if this pattern were copied somewhere hotter.
    fn clone(&self) -> Self {
        ListenPending { running: self.running, result: self.result.clone() }
    }
}

impl VoicePopup {
    /// Start listening. Returns `false` (does nothing) when
    /// `password_mode` is true — the non-negotiable gate from the module
    /// doc. The caller still owns actually opening `self.surfaces` and
    /// showing any refusal message; this function only decides whether
    /// to arm capture.
    pub fn start(&mut self, password_mode: bool) -> bool {
        if password_mode {
            return false;
        }
        self.opened_by_wake = false;
        self.stage = Stage::Listening;
        self.delivered = false;
        self.spawn_listen();
        true
    }

    /// The finished transcript, ONCE. `None` until `tick` reaches `Done`
    /// and `None` again after the first call — the app reads this every
    /// frame and acts on what it gets, and a transcript acted on per
    /// frame would run the same command sixty times a second.
    ///
    /// This is the seam the transcript had been missing: `Done` was
    /// displayed and nothing read it, so speech reached the screen and
    /// never the shell.
    pub fn take_result(&mut self) -> Option<String> {
        if self.delivered { return None; }
        match &self.stage {
            Stage::Done { text } => { self.delivered = true; Some(text.clone()) }
            _ => None,
        }
    }

    /// Start listening as a wake word did — same as `start`, with the
    /// header saying so.
    pub fn start_by_wake(&mut self, password_mode: bool) -> bool {
        if !self.start(password_mode) { return false; }
        self.opened_by_wake = true;
        true
    }

    /// Cancel and drop the audio. `voice_engine_kit::cancel_recording()`
    /// is the same primitive the Qt build's `voice_cancel` calls.
    pub fn cancel(&mut self) {
        cancel_recording();
    }

    /// Force-finish: stop listening now, keep the audio, transcribe it.
    /// Does not itself change `self.stage` -- the background thread
    /// reports the transition through `tick()`'s poll, the same way the
    /// Qt build learns about it through `voice_listen_dual_running`
    /// rather than a direct signal.
    pub fn finish(&mut self) {
        finish_recording();
    }

    fn spawn_listen(&self) {
        // Called explicitly, ahead of the blocking listen on its own
        // thread, matching the Qt build's own reasoning for calling
        // `voice_begin_capture` early ("by the time they tap, audio is
        // captured") -- redundant with what `listen_with_audio`'s own
        // `record_and_transcribe_inner` does internally
        // (`maintenance/C18_TODO.md` section 19), but capture starts the
        // INSTANT this returns rather than after whatever latency exists
        // before the spawned thread's first instruction runs.
        begin_capture();

        let pending = self.pending.clone();
        {
            let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
            if p.running {
                return; // already in flight; do not spawn a second worker
            }
            p.running = true;
            p.result = None;
        }
        // A TEST HOOK, and the only one: with `UIKIT_VOICE_FAKE_TRANSCRIPT`
        // set, the popup "hears" that text after a moment instead of
        // opening a microphone. It is how the whole chain below the
        // popup — router, staging, the effect on screen — gets walked on
        // a machine with no input device, which is every CI box and this
        // sandbox. Never set in a packaged launcher.
        let fake = std::env::var("UIKIT_VOICE_FAKE_TRANSCRIPT").ok().filter(|t| !t.is_empty());
        std::thread::Builder::new()
            .name("voice-popup-listen".to_string())
            .spawn(move || {
                let (text, audio) = match fake {
                    Some(t) => {
                        std::thread::sleep(std::time::Duration::from_millis(800));
                        cancel_recording();
                        (t, Vec::new())
                    }
                    None => listen_with_audio(),
                };
                let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
                p.result = Some((text, audio));
                p.running = false;
            })
            .ok();
        // `.ok()`, matching voiceterm's own `voice_listen_dual_async`,
        // which also swallows a spawn failure silently rather than
        // propagating it. `running` is already true above, so a failed
        // spawn leaves the popup showing "listening" with nothing to
        // poll -- inherited from the Qt build rather than independently
        // decided; neither build has an explicit test for this path.
    }

    /// Call once per frame while the popup is open. Advances
    /// `Listening` -> `Transcribing` -> `Done`/`Error` (or signals close
    /// via the return value) as the background thread reports progress.
    /// Returns `true` if the popup should close NOW (a cancelled
    /// recording) -- the caller does `self.surfaces.close_if(Surface::
    /// Voice)` in that case, since this module does not own the surface
    /// tracker.
    pub fn tick(&mut self) -> bool {
        let still_running;
        let result;
        {
            let p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            still_running = p.running;
            result = p.result.clone();
        }
        if still_running {
            // VAD may still be running (Listening) or whisper may already
            // be (Transcribing) -- `listen_with_audio` reports nothing
            // until it is completely done (confirmed against the real
            // signature: `pub fn listen_with_audio() -> (String,
            // Vec<f32>)`, no partial/progress channel), so this cannot
            // tell the two apart from here. A finer split would need a
            // new signal voice-engine-kit does not emit today; approximated
            // as staying in whatever stage it already is, which is a
            // cosmetic gap (the UI may say "listening" a moment after
            // Finish was tapped) rather than a functional one.
            return false;
        }
        let Some((text, _audio)) = result else { return false };
        if text.is_empty() {
            // Empty text is `listen_with_audio`'s cancellation return
            // (confirmed against `record_and_transcribe_inner`'s real
            // cancel branch, `return (String::new(), Vec::new())`), not
            // an error. Signal close-now, matching the Qt build where
            // `voice_cancel` closes the popup itself rather than waiting
            // to see what the worker returns.
            return true;
        }
        if text.starts_with('[') {
            // voiceterm's own convention for an embedded error string
            // ("[Voice tools not available...]", "[Voice failed:
            // arecord/whisper not in PATH]") -- confirmed against
            // `listen_external`'s real return values. The QML build
            // checks the same prefix (`if(t&&!t.startsWith("["))`).
            self.stage = Stage::Error { message: text };
        } else {
            self.stage = Stage::Done { text };
        }
        false
    }

    /// Draw the popup body. Call from inside `overlay::show(ctx,
    /// Surface::Voice, open_t, |ui| { ... })` -- this function IS that
    /// closure's body, not a second container. `colors` is the app's
    /// existing `PaletteColors`, same as every other ported surface.
    ///
    /// Layout follows the QML's real structure (see module doc): a round
    /// badge, a two-line header, then a state-gated action row.
    pub fn draw(&mut self, ui: &mut egui::Ui, colors: &crate::PaletteColors) {
        // KEEP FRAMES COMING while a thread is working. egui repaints on
        // input, and the transcript arrives on a thread with no input
        // attached: with the pointer still, the popup sat on "Listening…"
        // holding a finished result until the next mouse move. On a phone
        // with the finger lifted that is forever.
        if matches!(self.stage, Stage::Listening | Stage::Transcribing) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            self.draw_badge(ui, colors);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                let (title, subtitle) = self.header_text();
                ui.label(
                    egui::RichText::new(title)
                        .size(16.0)
                        .strong()
                        .color(colors.text),
                );
                if !subtitle.is_empty() {
                    ui.label(
                        egui::RichText::new(subtitle)
                            .size(11.0)
                            .color(colors.text_weak),
                    );
                }
            });
        });
        ui.add_space(12.0);
        match &self.stage {
            Stage::Listening => {
                ui.horizontal(|ui| {
                    // Prominent Finish, smaller Cancel -- matching the
                    // QML's own weighting (`_doneBtn` is `Layout.fillWidth`,
                    // the cancel control beside it is not), on the
                    // reasoning that finishing normally is the expected
                    // path and cancelling is the exception.
                    if ui
                        .add_sized([0.0, 0.0], egui::Button::new("Done speaking"))
                        .clicked()
                    {
                        self.finish();
                    }
                    if ui.button("Cancel").clicked() {
                        self.cancel();
                    }
                });
            }
            Stage::Transcribing => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("thinking…").color(colors.text_weak));
                });
            }
            Stage::Done { text } => {
                ui.label(egui::RichText::new(text.as_str()).size(15.0).color(colors.text));
            }
            Stage::Error { message } => {
                ui.label(egui::RichText::new(message.as_str()).color(colors.danger));
            }
        }
    }

    /// The round state badge: red pulsing circle while recording (approximated
    /// here as solid red, since a true pulse needs a per-frame time input
    /// this function does not take -- see the note below), a spinner glyph
    /// while transcribing, a plain accent-colored mic glyph otherwise.
    /// Matches the QML badge's THREE-way color switch
    /// (`recording ? red : processing ? highlight : accent`) and its glyph
    /// switch (`● : ⟗ : 🎤`) even though the QML's opacity pulse animation
    /// is not reproduced -- egui's `Context::animate_value` could drive one
    /// from `ui.ctx().input(|i| i.time)`, deferred rather than guessed at
    /// without seeing it on screen first.
    fn draw_badge(&self, ui: &mut egui::Ui, colors: &crate::PaletteColors) {
        let (bg, glyph) = match &self.stage {
            Stage::Listening => (colors.danger, "\u{25CF}"),
            Stage::Transcribing => (colors.accent, "\u{29D7}"),
            Stage::Done { .. } => (colors.success, "\u{1F3A4}"),
            Stage::Error { .. } => (colors.danger, "\u{1F3A4}"),
        };
        let size = egui::vec2(44.0, 44.0);
        let (rect, _resp) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 22.0, bg);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(20.0),
            egui::Color32::WHITE,
        );
    }

    /// The two-line header text, matching the QML's own state -> text
    /// table (main.qml, inside `contentColV`'s header `RowLayout`) field
    /// for field, minus the wake/wake-registration branches this cut does
    /// not implement (`opened_by_wake` is always false today -- see the
    /// module doc).
    fn header_text(&self) -> (&'static str, &'static str) {
        match &self.stage {
            Stage::Listening if self.opened_by_wake => ("Listening… (wake)", "Speak now (up to 12s, pause 1s when done)"),
            Stage::Listening => ("Listening…", "Speak now (up to 12s, pause 1s when done)"),
            Stage::Transcribing => ("Transcribing…", ""),
            Stage::Done { .. } => ("Voice", ""),
            Stage::Error { .. } => ("Couldn't hear you", ""),
        }
    }
}

impl Default for VoicePopup {
    /// `stage: Stage::Listening` here is a placeholder, not a claim that
    /// this is a meaningful "at rest" state -- `VoicePopup` is constructed
    /// once, at app startup, long before anyone opens the popup, and this
    /// value is NEVER drawn: `draw`/`tick` are only ever called by the
    /// caller after checking `self.surfaces.is(Surface::Voice)`, and the
    /// real call site (not yet wired into voiceterm_egui.rs) MUST call
    /// `start()` in the same action that opens the surface, or a reopened
    /// popup would show a stale `Done`/`Error` from the previous session
    /// instead of starting a fresh listen. Worth stating rather than
    /// hiding behind a plausible-looking default: this is one call-site
    /// discipline this module cannot enforce on its own (it has no way to
    /// know when `self.surfaces.open(Surface::Voice)` is called, since
    /// that state lives in the host app, by design -- see the module
    /// doc). `Option<Stage>` with `None` meaning "never started" was
    /// considered and rejected for now: every real caller path opens and
    /// starts together, so the extra `Option` layer would exist only to
    /// guard against a call-site mistake this comment already names.
    fn default() -> Self {
        VoicePopup {
            stage: Stage::Listening,
            opened_by_wake: false,
            delivered: false,
            pending: std::sync::Arc::new(std::sync::Mutex::new(ListenPending::default())),
        }
    }
}
