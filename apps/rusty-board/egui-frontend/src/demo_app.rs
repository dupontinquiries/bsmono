//! demo_app — the runnable keyboard screen: layout tabs, the board, swipe typing, and an
//! output line.
//!
//! This used to exist TWICE — `src/bin/rusty_board_egui.rs` and `examples/feel.rs`, with a
//! comment on each saying they were "kept in sync manually". They were not: the example had
//! already lost the real-layout tabs. Two copies of a screen is the same mistake two copies
//! of a recognizer would be, so the screen lives here and both entry points are shims.

use eframe::egui;

use crate::real_layout::{Press, RealBoard};
use crate::swipe::{candidate_bar, draw_trail_styled, set_trail_debug_styling, trail_debug_styling, SwipeState, TrailStyle};
use crate::{apply_theme, keyboard_ui_styled, Board};
use uikit_egui::Adaptive;

pub struct Feel {
    /// The thumb-reach calibration, while it is open.
    ///
    /// `Some` puts the overlay over the board and takes the input. Opened by
    /// `RUSTY_BOARD_CALIBRATE=1` for now — the same env convention as
    /// `RUSTY_BOARD_PLACEMENT`, and honest about being a way in rather than a settings row it
    /// does not have yet. A flow nobody can reach is not a feature, and an env var is a way
    /// to reach it.
    pub thumb_cal: Option<crate::thumbcal::Calibration>,
    /// What the board draws with: the persisted appearance block with the panel's values
    /// folded on top.
    pub style: crate::Style,
    /// Where springboard delimiters fired, and when: `(point, time_ms)`. Drained as they
    /// expire — see `springboard::ripple`.
    pub ripples: Vec<(egui::Pos2, f64)>,
    /// When the last word was committed, for the space zones' pulse.
    word_at: Option<f64>,
    /// The last touch position, for the doc's "nearest zone pulses 30% harder".
    last_touch: Option<egui::Pos2>,
    /// The appearance block alone. Kept so a panel change re-folds from a clean base rather
    /// than accumulating onto whatever the last fold produced.
    base_style: crate::Style,
    /// Which of the three keyboard obligations (no focus, bottom edge, focus-driven show/hide)
    /// this host meets, decided once — see `platform.rs`. Shown in the status strip.
    placement: crate::platform::Placement,
    /// The in-board reveal that runs when the compositor shows us. Motion INSIDE a rectangle
    /// whose size never changes: one map, one relayout, no per-frame resize storm on a tiling
    /// compositor (a sliding WINDOW does exactly that).
    reveal: crate::animations::OneShot,
    /// Last frame's visibility, to detect the hidden→shown edge that starts the reveal.
    was_visible: bool,
    /// The last `Visible` command sent, so it is sent on change only (see `frame`).
    sent_visible: Option<bool>,
    /// The layout rail folded to a sliver, so the keys get the full width.
    pub rail_collapsed: bool,
    /// The surface height this frame's content needs -- the keys plus whatever panels are
    /// open, and nothing under them. See [`Feel::desired_height`].
    desired_h: Option<f32>,
    /// The last height asked of the window, so it is asked once per change rather than
    /// every frame (which would also undo a user's manual resize every frame).
    asked_h: Option<f32>,
    /// The accessibility scene widgets register with. Held here rather
    /// than built per frame because `A11yScene` remembers focus BY ID
    /// between frames; a per-frame scene forgets, which presents as
    /// focus jumping rather than as a missing scene.
    a11y: uikit_egui::a11y::A11yScene,
    board: Board,
    /// The minimized tray, the microterm and dictation — three ported surfaces
    /// the demo could not reach. A keyboard that cannot minimise, cannot show
    /// its terminal and cannot dictate is not the keyboard being shipped, and
    /// the modules existed the whole time.
    tray: crate::tray::Tray,
    /// Full / Tray / Dismissed. The tray widget was ported and the machine that decides
    /// when it is on screen was not, so `tray_open` was a bool a debug button set — the
    /// documented transitions (a long upward fling, a double-activate back) had nowhere to
    /// live. See `visible_mode`.
    vis: crate::visible_mode::Machine,
    /// Turns what the sink OBSERVED about focus into whether the keyboard
    /// should be on screen. Separate from `vis` (which is the user-driven
    /// Full/Tray/Dismissed machine) because this one answers to the
    /// compositor and has to cope with compositors that never answer at
    /// all — see `focus_policy`.
    focus: crate::focus_policy::FocusPolicy,
    /// Last verdict, kept for the status line so a degraded state is
    /// visible to the person holding the phone rather than only in the
    /// journal.
    focus_reason: crate::focus_policy::Reason,
    /// Process start, for `focus_policy`'s grace window. Not egui's
    /// `input.time`: the layer shell calls `pump_sink` while idle with no
    /// `Context` in hand, and a policy fed two different clocks depending
    /// on which path woke it would time out at the wrong moment. `Instant`
    /// is monotonic, which is the only property the policy needs.
    started: std::time::Instant,
    tray_open: bool,
    /// Which top-bar features the person switched on.
    ///
    /// EMPTY BY DEFAULT, which is the whole rule: the bar carried tray,
    /// settings, term and dictate on every frame, costing the keys vertical
    /// space for controls most sessions never touch. Nothing here means the
    /// panel is not drawn at all — see `chrome::Chrome::top_bar_height`.
    ///
    /// A set rather than a count so the bar can draw only what was enabled;
    /// the height does not depend on how many, but the contents do.
    topbar_features: std::collections::BTreeSet<crate::chrome::Control>,
    microterm: crate::microterm::Microterm,
    /// The dictation SERVICE, not a bare Dictation: it owns the same state
    /// machine and adds the audio-window bookkeeping the transcriber needs.
    /// Asking for "everything captured so far" each interval is what makes
    /// partials fall behind the speaker on a phone.
    dictation: crate::voice::DictationService,
    /// What the last inference cost, for the status row ("the loop is visible").
    voice_timing: Option<String>,
    /// The control socket (ctl.rs) and what it has told us.
    ctl: Option<crate::ctl::Ctl>,
    /// tideshell's cold mode: a keyboard shown while cold starts voice typing.
    cold: bool,
    /// A ctl command changed state while no frame was running; the shell
    /// should run one (the layer loop asks via `take_needs_frame`).
    needs_frame: bool,
    /// The real capture + whisper driver (feature `voice`); `Err` = why it could not load
    /// (no model, no mic backend), shown in the status row. Without the feature this field
    /// does not exist and dictation stays the preview it always was here.
    #[cfg(feature = "voice")]
    voice: Result<crate::voice_host::VoiceHost, String>,
    /// What the microphone broker said. Dictation had no audio source at all —
    /// start() set a flag — so a refusal was invisible and the user just saw
    /// dictation do nothing.
    mic: crate::mic_source::MicState,
    /// The REAL layout files (resources/layouts/*.json), with shift/caps + the symbols toggle —
    /// shown when the sidebar's real-layout tabs are active (indices past the demo layouts).
    real: RealBoard,
    real_active: bool,
    typed: String,
    /// The learned dictionary. Ported, tested, and until now reached by
    /// nothing — the reachability audit's top rusty-board finding. A swipe
    /// keyboard that never learns which of two plausible words THIS person
    /// uses is the feature working at its worst permanently.
    dict: crate::adaptive::AdaptiveDictionary,
    /// The `io.rustyboard.Keyboard1` request model. A keyboard's whole job is
    /// to hand text to whatever asked for it, and every commit path here used
    /// to end at a local String — so an embedder had nothing to read. Ported,
    /// 19 tests, and reached by nothing until now.
    service: crate::keyboard_service::KeyboardService,
    /// Where committed text goes. `Preview` on a desktop checkout, the
    /// input-method backend on device.
    ///
    /// The service has always QUEUED commits (`take_pending`) and
    /// nothing drained the queue outside its own tests — so every tap,
    /// swipe and dictation result went into a Vec nobody read, and the
    /// only visible effect was the preview string below. That is what
    /// made this a demo rather than a keyboard.
    sink: Box<dyn crate::sink::Sink>,
    /// The last failure from the sink, shown in the status line.
    sink_error: Option<String>,
    /// How many commits the service has emitted. Shown in the demo so the
    /// integration is VISIBLE — a silent pass-through is indistinguishable
    /// from one that is not wired at all, which is how this module came to be
    /// unreached in the first place.
    committed: usize,
    /// Next-word counts. `adaptive` knows what this person types AT ALL;
    /// this knows what they type AFTER what they just typed — a bigram beats a
    /// unigram exactly when two candidates are equally common on their own,
    /// which is the case the recognizer cannot break.
    ngram: crate::ngram::NgramStore,
    /// Hold-to-confirm for the one destructive button in the app.
    clear_hold: crate::tap_hold::TapHold,
    /// The in-keyboard settings panel. `settings_model` carries the groups and
    /// their help text; `settings` reads rusty-board's REAL config rather than
    /// a copy, so a field added on the GTK side cannot drift out of the port.
    settings_open: bool,
    /// Chosen values, keyed by config key. Held here rather than written
    /// through on every tap: `settings` reads rusty-board's real Config and the
    /// port has no writer for it, so persisting from the demo would mean a
    /// SECOND way to write the file — the drift the settings module exists to
    /// avoid. The panel shows and cycles; the host persists.
    setting_values: std::collections::HashMap<String, String>,
    /// Flash on the output line when text is committed. The port had four
    /// animations and used none of them, so every commit looked identical to
    /// no commit — on a keyboard whose output is a label rather than a focused
    /// field, that is the only feedback there is.
    commit_flash: crate::animations::OneShot,
    /// Continuous pulse while recording. A static "listening" label cannot
    /// distinguish a live mic from a hung one, which is the state a voice
    /// keyboard most needs to make visible.
    record_pulse: crate::animations::Pulse,
    /// Which gesture category the live trail is showing — only visible when trail debug
    /// styling is on (see the Diagnostics toggle). Recomputed as the gesture is classified.
    trail_style: TrailStyle,
    /// Swipe typing: the live path, the trail, and the ranked words (board-gestures does the
    /// recognition — the same decoder the GTK app runs).
    swipe: SwipeState,
}

impl Feel {
    /// Send text to the sink and mirror it in the preview.
    ///
    /// ONE place, so a key path added later cannot quietly skip the sink
    /// the way the direct `self.typed.push_str` calls did. The preview
    /// is a mirror of what was sent, never the destination.
    /// What this screen has sent, mirrored -- the on-screen preview's text. Read-only; the
    /// tests use it to see what a tap actually produced.
    pub fn typed(&self) -> &str {
        &self.typed
    }

    fn emit_text(&mut self, text: &str) {
        if let Err(e) = self.sink.commit_text(text) {
            self.sink_error = Some(e);
        }
        self.typed.push_str(text);
    }

    /// Send a non-character key by keysym name, and mirror its effect.
    fn emit_key(&mut self, keysym: &str) {
        if let Err(e) = self.sink.press_keysym(keysym) {
            self.sink_error = Some(e);
        }
        match keysym {
            "BackSpace" => {
                self.typed.pop();
            }
            "Return" => self.typed.push('\n'),
            "Tab" => self.typed.push('\t'),
            _ => {}
        }
    }

    /// Start or stop dictation. ONE place for the toolbar button and the 🎤 key on any board,
    /// so the voice mode cannot be wired to one surface and forgotten on the next — which is
    /// how the GTK build shipped voice on qwerty only.
    pub fn toggle_dictation(&mut self) {
        if self.dictation.is_active() {
            // Stop capturing first (no final pass in PartialsOnly), keeping the audio the
            // last partial was made from for the corpus.
            #[cfg(feature = "voice")]
            let last_audio = match &mut self.voice {
                Ok(v) => v.stop(),
                Err(_) => Vec::new(),
            };
            // PartialsOnly: the last partial IS the result, so release commits without a
            // second inference.
            if let Some(text) = self.dictation.release() {
                if !self.typed.is_empty() && !self.typed.ends_with(' ') {
                    self.typed.push(' ');
                }
                self.service.commit_text(&text);
                self.typed.push_str(&text);
                // COMMITTED, so it may enter the corpus: the next time this utterance is
                // said, the fingerprint answers before whisper runs.
                #[cfg(feature = "voice")]
                if let Ok(v) = &self.voice {
                    v.remember(&last_audio, &text);
                }
            }
            self.end_voice_session();
        } else {
            // A build without the `voice` feature has no engine: say so, instead of
            // latching a microphone that can never hear anything.
            #[cfg(not(feature = "voice"))]
            {
                self.microterm.set_content(crate::microterm::Content::Status(
                    "Voice typing isn't in this build (rebuild with --features voice).".into(),
                ));
                self.microterm.show();
                return;
            }
            #[cfg(feature = "voice")]
            match &mut self.voice {
                Ok(v) => match v.start() {
                    Ok(()) => {
                        self.mic = crate::mic_source::MicState::Live { mic: "default".into(), also: Vec::new() };
                    }
                    Err(e) => {
                        tracing::warn!("voice start: {e:#}");
                        self.mic = crate::mic_source::MicState::Refused(crate::mic_source::Denied::Unavailable);
                        return;
                    }
                },
                Err(why) => {
                    tracing::warn!("dictation requested but the voice engine is not available: {why}");
                }
            }
            self.dictation.start();
            // SUPPRESSED while the voice session runs — keyboard_service's own doc names a
            // voice session as exactly the trigger. The Hidden event is dropped on purpose:
            // this app reads service STATE directly and consumes no ServiceEvent anywhere.
            let _hidden = self.service.set_suppressed(true);
        }
    }

    /// Stop dictating and throw the utterance away: nothing is typed.
    pub fn cancel_dictation(&mut self) {
        if !self.dictation.is_active() {
            return;
        }
        #[cfg(feature = "voice")]
        if let Ok(v) = &mut self.voice {
            let _ = v.stop();
        }
        self.dictation.cancel();
        self.end_voice_session();
    }

    /// What every end of a voice session shares, commit or cancel: the mic
    /// back to Idle (it stayed Live before, which kept the record pulse
    /// animating and the keyboard repainting forever), the preview put away,
    /// and the keyboard given back.
    fn end_voice_session(&mut self) {
        self.mic = crate::mic_source::MicState::Idle;
        self.voice_timing = None;
        if self.microterm.content().page() == "voice" {
            self.microterm.hide();
        }
        // RELEASED: the keyboard comes back now the voice session is over.
        let _ = self.service.set_suppressed(false);
    }

    /// Once per frame, whatever chrome is shown: account for captured audio,
    /// fetch a finished partial, keep the preview current. This used to live
    /// inside the top bar's closure, which is not drawn unless a top-bar
    /// feature is enabled (none are by default) -- so on the phone the mic key
    /// recorded, and nothing was ever transcribed, previewed or typed.
    fn pump_dictation(&mut self, ctx: &egui::Context) {
        if !self.dictation.is_active() {
            return;
        }
        #[cfg(feature = "voice")]
        let mut level = 0.0;
        #[cfg(not(feature = "voice"))]
        let level = 0.0;
        #[cfg(feature = "voice")]
        if let Ok(v) = &mut self.voice {
            // Real sample counts from the ring, so the window is bounded over audio
            // that exists; then the partial in flight, if done.
            self.dictation.captured(v.captured_samples());
            let window = self.dictation.next_window();
            if let Some(text) = v.poll(window) {
                self.dictation.partial(&text);
            }
            level = v.level().clamp(0.0, 1.0);
            if let Some(t) = v.last_timing() {
                self.voice_timing = Some(format!("{} · corpus {}", t.describe(), v.corpus_len()));
            }
            // Partials and the level meter need frames while listening, at a rate
            // the eye reads as live (not the display's).
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
        self.microterm.set_content(crate::microterm::Content::VoicePreview { partial: self.dictation.preview().to_string(), level });
        self.microterm.show();
    }

    /// Advance the sink WITHOUT running a frame. A shell that sleeps between frames (the
    /// layer shell) calls this on every wake, so Activate/Deactivate are seen while idle
    /// without repainting a keyboard nobody is touching. Returns whether the compositor
    /// currently wants the keyboard shown; the shell compares that with its map state and
    /// runs a frame (which emits `Visible`) only when they differ.
    pub fn pump_sink(&mut self) -> bool {
        self.sink.pump();
        self.handle_ctl();
        self.decide_visibility()
    }

    /// Commands from the shell (ctl.rs). Handled on every wake, frame or not.
    fn handle_ctl(&mut self) {
        let cmds = match self.ctl.as_mut() {
            Some(c) => c.drain(),
            None => return,
        };
        for c in cmds {
            use crate::ctl::Cmd;
            tracing::debug!("ctl: {c:?}");
            match c {
                Cmd::VoiceStart if !self.dictation.is_active() => self.toggle_dictation(),
                Cmd::VoiceDone if self.dictation.is_active() => self.toggle_dictation(),
                Cmd::VoiceCancel => self.cancel_dictation(),
                Cmd::VoiceToggle => self.toggle_dictation(),
                Cmd::Cold(on) => self.cold = on,
                _ => {}
            }
            self.needs_frame = true;
        }
    }

    /// Whether a ctl command changed something a frame must show. Clears it.
    pub fn take_needs_frame(&mut self) -> bool {
        std::mem::take(&mut self.needs_frame)
    }

    /// The control socket's descriptor, for the layer shell to poll.
    pub fn ctl_fd(&self) -> Option<std::os::fd::RawFd> {
        self.ctl.as_ref().map(|c| c.fd())
    }

    pub fn is_cold(&self) -> bool {
        self.cold
    }

    /// The sink's descriptor, readable when `pump_sink` has work. The layer
    /// shell polls it beside its own socket; `None` means it must fall back
    /// to a short timer.
    pub fn sink_wake_fd(&self) -> Option<std::os::fd::RawFd> {
        self.sink.wake_fd()
    }

    /// The next frame this screen needs that no input will cause. The eframe
    /// shells keep their 320 ms tick (winit cannot poll the input-method
    /// socket); the layer shell gets only real deadlines, clamped to a small
    /// floor so a deadline already past cannot spin.
    fn timed_wake(&self, placement: crate::platform::Placement) -> Option<std::time::Duration> {
        use std::time::Duration;
        if placement != crate::platform::Placement::LayerShell {
            return Some(Duration::from_millis(320));
        }
        let now_ms = self.started.elapsed().as_millis() as u64;
        let grace = self.focus.next_timed_change_ms(now_ms).map(Duration::from_millis);
        let steno = crate::steno::pending_remaining();
        [grace, steno].into_iter().flatten().min().map(|d| d.max(Duration::from_millis(4)))
    }

    /// The single place the show/hide question is answered, so the idle
    /// path and the frame path cannot drift apart. They did not share one
    /// before; `pump_sink` read the sink directly and `frame` read it
    /// again, which is two copies of a rule that is about to grow
    /// fallbacks.
    fn decide_visibility(&mut self) -> bool {
        let now_ms = self.started.elapsed().as_millis() as u64;
        let d = self.focus.decide(now_ms, self.sink.focus_source());
        if d.reason != self.focus_reason {
            // Once per transition, not once per frame — this fires at
            // 60 Hz otherwise and buries the journal.
            if d.reason.is_degraded() {
                tracing::warn!("focus: {}", d.reason.describe());
            } else {
                tracing::debug!("focus: {}", d.reason.describe());
            }
            self.focus_reason = d.reason;
        }
        d.visible
    }

    /// What the status line should say about focus. Public because the
    /// degraded states are the ones a person needs to see without a
    /// serial cable.
    pub fn focus_reason(&self) -> crate::focus_policy::Reason {
        self.focus_reason
    }

    /// A screen for a shell that KNOWS its placement (the layer shell), rather than inferring
    /// it from the environment as `new` does for eframe.
    pub fn with_placement(placement: crate::platform::Placement) -> Feel {
        let mut f = Feel::new();
        f.placement = placement;
        f
    }

    pub fn new() -> Feel {
        // THE APPEARANCE SETTINGS, read once at start. `settings::ready(Appearance)` has
        // returned true all along — the section claimed to "do what it says" — while
        // `key_radius`, `opacity` and `show_magnifier` were stored, round-tripped and
        // ignored by the renderer. A control that silently does nothing is the exact
        // failure that function's own comment warns about.
        let cfg = crate::settings::load_or_default();
        let style = crate::Style::from_appearance(
            cfg.appearance.key_radius,
            cfg.appearance.opacity,
            cfg.appearance.show_magnifier,
        );
        Feel {
            // Open only when asked. See the field's note on the env var.
            thumb_cal: std::env::var("RUSTY_BOARD_CALIBRATE").is_ok()
                .then(crate::thumbcal::Calibration::default),
            style,
            base_style: style,
            ripples: Vec::new(),
            word_at: None,
            last_touch: None,
            placement: crate::platform::placement(),
            reveal: Default::default(),
            was_visible: false,
            sent_visible: None,
            rail_collapsed: false,
            desired_h: None,
            asked_h: None,
            a11y: uikit_egui::a11y::A11yScene::new(),
            trail_style: TrailStyle::Solid,
            board: Board::new(),
            real: RealBoard::new(),
            real_active: false,
            typed: String::new(),
            // A starter word list so the demo binary can be swiped immediately; on-device the
            // host loads rusty-board's shipped dictionary into the same recognizer.
            tray: Default::default(),
            vis: Default::default(),
            // Same `cfg` the appearance settings above came from, so the
            // escape hatch is the one already documented in
            // `config/mod.rs` and already present in every user's
            // config.toml — not a second setting meaning the same thing.
            focus: crate::focus_policy::FocusPolicy::from_config(
                cfg.behavior.auto_hide_on_no_focus,
            ),
            focus_reason: crate::focus_policy::Reason::AwaitingFirstActivate,
            started: std::time::Instant::now(),
            tray_open: false,
            // Nothing enabled: no top bar. A keyboard's job is keys.
            topbar_features: Default::default(),
            microterm: Default::default(),
            // 16kHz: what the transcriber expects, and what the window maths
            // is computed against.
            dictation: {
                // The user's voice filter level from the fleet settings (bsettings,
                // a11y.voice_filter). It was never loaded, so Off/Strict did nothing.
                let mut d = crate::voice::DictationService::new(16_000);
                d.set_filter_level(fleet_voice_filter());
                d
            },
            voice_timing: None,
            // Not in tests: binding replaces the live keyboard's socket, and
            // tests run on the device too.
            ctl: if cfg!(test) { None } else { crate::ctl::Ctl::bind() },
            cold: false,
            needs_frame: false,
            #[cfg(feature = "voice")]
            voice: crate::config::Config::load_or_default()
                .map_err(|e| format!("config: {e:#}"))
                .and_then(|cfg| crate::voice_host::VoiceHost::new(&cfg).map_err(|e| format!("{e:#}"))),
            mic: crate::mic_source::MicState::Idle,
            swipe: SwipeState::with_dictionary(
                include_str!("../demo_words.txt").lines().map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty()).collect()),
            // Feel has a HAND-WRITTEN constructor, so a new struct field does
            // not appear here automatically — the same trap that produced
            // gitmind's missing `bluetooth` initialiser. Every field added
            // this session is listed explicitly.
            dict: Default::default(),
            service: Default::default(),
            sink: crate::sink::detect(),
            sink_error: None,
            committed: 0,
            ngram: Default::default(),
            clear_hold: Default::default(),
            settings_open: false,
            setting_values: Default::default(),
            commit_flash: Default::default(),
            record_pulse: Default::default(),
        }
    }
}

impl Default for Feel {
    fn default() -> Self { Feel::new() }
}

impl eframe::App for Feel {
    // this eframe's App trait requires ui(); the app is driven from update() below.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

impl Feel {
    /// One frame of the keyboard screen against a bare `egui::Context`. THE SHELL SEAM: the
    /// eframe `App::update` above is one caller; a shell that owns its own surface (a
    /// layer-shell loop calling `ctx.run(raw_input, |ctx| feel.frame(ctx))`) is the other. The
    /// screen asks for show/hide through `ViewportCommand::Visible` and never touches a window
    /// handle, so which shell is running is invisible from in here.
    #[allow(deprecated)]
    /// One steno gesture into the engine, and whatever comes back out.
    ///
    /// Shared by both dispatch paths — the real-layout board and the code-built one. Two
    /// copies of this drifted once already (the second was an empty arm), and the
    /// difference is invisible until someone switches boards and finds the keys dead.
    ///
    /// `None` is not an error: most strokes go PENDING and commit later, either from the
    /// opposite thumb's own `feed` or from `poll_pending` in `update`. A handler that
    /// treated `None` as a failure would report one on every first half of a chord.
    fn on_steno(&mut self, button: &str, gesture: &str) {
        if let Some(commit) = crate::steno::feed(button, gesture) {
            // Commits verbatim, no auto-space: steno phonemes chain into syllables
            // (`k`+`a`+`t` → `kat`, not `k a t`), matching GTK's `EngineInput::StenoCommit`.
            self.emit_text(&commit.text);
            if let Some(content) = crate::steno::commit_to_microterm(&commit) {
                self.microterm.set_content(content);
            }
        }
    }

    pub fn frame(&mut self, ctx: &egui::Context) {
        apply_theme(ctx);

        // Drive whatever background event processing the active sink
        // needs (on Wayland: dispatching the input-method event queue —
        // see `Sink::pump`'s doc and `wayland.rs`'s module doc for why
        // that has to happen here rather than on some other thread).
        // Must run before `decide_visibility()` below, or this frame's
        // Activate/Deactivate event wouldn't be reflected until the
        // next one.
        self.sink.pump();

        // Maliit-style focus-driven show/hide: a real compositor focus
        // signal (Wayland input-method Activate/Deactivate), not internal
        // app state. `Preview`/no-signal sinks report
        // `FocusSource::None`, which resolves to visible, so desktop and
        // test runs behave exactly as before. What the policy adds over
        // reading the sink directly is the cases where the signal exists
        // but is not to be believed — see `focus_policy`.
        let should_show = self.decide_visibility();
        // ONLY ON CHANGE. egui's `send_viewport_cmd` requests a repaint of the viewport
        // every time it is called (egui 0.34 context.rs, `send_viewport_cmd_to`), so sending
        // this every frame made every frame schedule the next: measured on a headless
        // sway at ~14,000 UI runs a second while HIDDEN under the layer shell, and a
        // continuously redrawing window under eframe (the ~46% of a core seen on the FP5).
        if self.sent_visible != Some(should_show) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(should_show));
            self.sent_visible = Some(should_show);
        }
        if should_show && !self.was_visible {
            self.reveal.start();
            // Cold (gloves): a text field just took focus and nobody can type
            // on glass with gloves on, so listen. A flick right (tideshell) or
            // ✓ Done types it; a flick left or ✕ Cancel drops it.
            if self.cold && !self.dictation.is_active() {
                self.toggle_dictation();
            }
        }
        self.handle_ctl();
        self.was_visible = should_show;
        // Keep ticking even while hidden -- BUT ONLY IN AN EFRAME WINDOW. A
        // hidden/unfocused eframe window may not otherwise get repainted,
        // which would starve `pump()` above and mean the NEXT focus event
        // never gets noticed; winit cannot poll the input-method socket for
        // us. The layer shell can: it sleeps in poll() on BOTH sockets
        // (`wake_fd`), so a tick there is three full UI runs a second for
        // nothing, forever. There it asks only for the frames a timer
        // genuinely owns: a steno stroke's pairing deadline and the focus
        // grace window.
        if let Some(d) = self.timed_wake(self.placement) {
            ctx.request_repaint_after(d);
        }

        // Drain the service queue once per frame. Without this `pending` grows
        // for the life of the process — a queue with no reader is a leak, and
        // the model cannot know whether a host exists to read it.
        //
        // The drained text now GOES SOMEWHERE. It used to be `for _ in
        // ...`: counted and dropped, which proved the wiring only as far
        // as the commit and no further. On a desktop the sink is a
        // preview; on device it is the input-method backend, and the same
        // line does both because the app holds a `dyn Sink` and does not
        // know which it has.
        let dt_ms = ctx.input(|i| i.stable_dt) * 1000.0;
        // A steno stroke that didn't pair with its opposite thumb waits out
        // the pairing window before committing solo — see `steno.rs`'s
        // `Pending`/`poll_pending` doc comment for why that has to be
        // polled here (once per frame, key-independent) rather than fired
        // from a timer the way the GTK build's glib callback does it.
        if let Some(commit) = crate::steno::poll_pending() {
            self.emit_text(&commit.text);
            if let Some(content) = crate::steno::commit_to_microterm(&commit) {
                self.microterm.set_content(content);
            }
        }
        self.pump_dictation(ctx);
        for text in self.service.take_pending() {
            if let Err(e) = self.sink.commit_text(&text) {
                // Said out loud rather than swallowed: a keyboard that
                // silently stops typing is indistinguishable from one
                // whose keys are not registering, and that ambiguity is
                // exactly what made this hard to diagnose from outside.
                self.sink_error = Some(e);
            }
            self.committed += 1;
            // Start on the DRAIN rather than at each commit site: one flash per
            // frame however many words landed, and every commit path gets it
            // without three separate calls to keep in step.
            self.commit_flash.start();
        }
        self.commit_flash.tick(dt_ms, crate::animations::COMMIT_MS);
        self.reveal.tick(dt_ms, crate::platform::REVEAL_MS);
        if self.reveal.is_running() { ctx.request_repaint(); }
        // The pulse follows the mic rather than being toggled alongside it —
        // two booleans for one fact drift, and the one that drifts is the one
        // the user is looking at.
        let listening = !matches!(self.mic, crate::mic_source::MicState::Idle);
        if listening != self.record_pulse.is_running() {
            if listening { self.record_pulse.start(); } else { self.record_pulse.stop(); }
        }
        self.record_pulse.tick(dt_ms);
        if self.commit_flash.is_running() || self.record_pulse.is_running() {
            // Animations need frames. Without this they advance only when
            // something else causes a repaint, which is exactly when nothing
            // is happening.
            ctx.request_repaint();
        }

        // NOT DRAWN AT ALL when no top-bar feature is enabled, which is the
        // default. Previously this panel was unconditional, so every session
        // paid vertical space for tray/settings/term/dictate whether or not
        // the person used any of them -- and on a 380pt phone that space is
        // the difference between a comfortable key row and a cramped one.
        //
        // Skipped rather than drawn at zero height: an egui panel still takes
        // its frame margins at zero content height, so a "0pt" bar is not
        // free, and the gesture surface that reaches the options layer needs
        // no visible handle to exist.
        let bar_h = crate::chrome::Chrome::top_bar_height(self.topbar_features.len());
        if bar_h > 0.0 {
        egui::Panel::top("out").exact_height(bar_h).show(ctx, |ui| {
            // WRAPPED, not `horizontal`. egui's plain horizontal layout does
            // not wrap: it lays every widget on one line and lets the tail run
            // past the panel edge, where nothing can reach it. That is the
            // "buttons off screen" report — on a 380 px phone this row of
            // tray / settings / term / dictate plus a status label does not
            // fit, and the ones that did not fit were simply gone rather than
            // clipped visibly.
            //
            // Wrapping is the stopgap. The real fix is `chrome.rs`: three
            // controls stay on the board and the rest move to a separate
            // settings window, because a row that WRAPS onto three lines is
            // still eating the vertical budget the keys need.
            ui.horizontal_wrapped(|ui| {
                // A diagnostic that ships switched off and is reachable without a rebuild —
                // the same bargain as snapclone's trace channels. On device this is what tells
                // you whether a gesture was read as cardinal, swipe, or nothing at all.
                // MINIMISE / MICROTERM / DICTATE — the three surfaces the demo
                // never reached. Placed on the top strip because that is where
                // the real keyboard puts its chrome, and because a control you
                // cannot find is the same as a control that does not exist.
                // Drives the MACHINE, so the debug button and a real upward fling take the
                // same path and cannot disagree about what state the keyboard is in.
                if ui.selectable_label(self.tray_open, "▁ tray").clicked() {
                    let _ = if self.vis.mode() == crate::visible_mode::VisibleMode::Tray {
                        self.vis.expand()
                    } else {
                        // From Dismissed there is no board to minimize, so make one first —
                        // the debug button stands in for the IM activate a real session has.
                        if !self.vis.is_visible() {
                            let _ = self.vis.activate((ui.input(|i| i.time) * 1000.0) as u64);
                        }
                        self.vis.fling_up()
                    };
                    self.tray_open = self.vis.mode() == crate::visible_mode::VisibleMode::Tray;
                }
                if ui.selectable_label(self.settings_open, "⚙ settings").clicked() {
                    self.settings_open = !self.settings_open;
                }
                if ui.selectable_label(self.microterm.is_visible(), "term").clicked() {
                    if self.microterm.is_visible() { self.microterm.hide(); }
                    else {
                        // show() is a no-op on empty content by design — an
                        // empty microterm has nothing to display, so opening it
                        // would just be a blank strip. Give it the terminal.
                        self.microterm.set_content(crate::microterm::Content::Terminal);
                        self.microterm.show();
                    }
                }
                {
                    let listening = self.dictation.is_active();
                    if ui.selectable_label(listening, if listening { "● dictating" } else { "dictate" }).clicked() {
                        self.toggle_dictation();
                    }
                    if let Some(t) = &self.voice_timing {
                        ui.label(egui::RichText::new(t).small());
                    }
                }
                // The mic state, including WHY it was refused — the two
                // refusals need different fixes and a single "failed" hides
                // which.
                if !matches!(self.mic, crate::mic_source::MicState::Idle) {
                    // Pulsed, so a live mic is distinguishable from a hung one.
                    // The floor is RECORD_FLOOR rather than zero: a label that
                    // fades to invisible reads as a glitch rather than as
                    // breathing.
                    let o = self.record_pulse.opacity();
                    ui.label(egui::RichText::new(self.mic.label()).small()
                        .color(ui.visuals().text_color().gamma_multiply(o)));
                    if self.mic.retryable() && ui.small_button("retry").clicked() {
                        self.mic = crate::mic_source::MicState::Idle;
                    }
                }
                ui.separator();
                let mut dbg = trail_debug_styling();
                // `note` returns the Response, so it composes with the
                // `.changed()` this call site already depends on —
                // `semantic_toggle` would have returned a bool and forced
                // the surrounding logic to be rewritten to gain a11y.
                if uikit_egui::a11y::note(
                    &mut self.a11y, egui::Id::new("rusty_board.trail_debug").value(),
                    "trail shows gesture category", dbg,
                    ui.checkbox(&mut dbg, "trail shows gesture category")).changed() {
                    set_trail_debug_styling(dbg);
                }
                if dbg {
                    ui.label(egui::RichText::new("dots = cardinal · stripes = swiped word · grey = ignored")
                        .weak().small());
                }
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("layout: {}", self.board.current().name)).strong());
                ui.separator();
                ui.label("hold a key for accents · drag across letters to swipe-type · tap an icon at the left to switch layout");
                // The service's commit count, shown because a silent
                // pass-through is indistinguishable from one that is not wired
                // — which is exactly how this module sat unreached with 19
                // passing tests.
                // WHERE THE TEXT IS GOING. Shown always, not only after a
                // commit: "typing into this window" and "typing into the
                // focused app" look identical from outside, and that
                // ambiguity is why a keyboard that typed nowhere read as
                // a working demo for so long.
                ui.label(
                    egui::RichText::new(match &self.sink_error {
                        Some(e) => format!("sink error: {e}"),
                        None => format!("{} · {}", self.sink.describe(), self.placement.describe()),
                    })
                    .size(10.0)
                    .color(if self.sink_error.is_some() {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().weak_text_color()
                    }),
                );
                if self.committed > 0 {
                    // Flashed brighter for COMMIT_MS after each commit.
                    // commit_opacity returns >1 while running, which
                    // gamma_multiply reads as a brightening — so the line
                    // lifts and settles rather than blinking out and back.
                    let o = crate::animations::commit_opacity(&self.commit_flash);
                    ui.label(egui::RichText::new(format!(
                        "Keyboard1.TextCommitted × {}", self.committed))
                        .size(11.0)
                        .color(ui.visuals().weak_text_color().gamma_multiply(o)));
                }
                // CLEAR IS HOLD-TO-CONFIRM, because a tap wipes everything the
                // user typed and there is no undo. tap_hold exists for exactly
                // this and was reached by nothing — 270 lines, 12 tests, and
                // the one destructive button in the app fired on a stray tap.
                //
                // A hold rather than a confirm dialog: the progress bar IS the
                // confirmation, it cancels by letting go, and it does not
                // interrupt what you were doing to ask.
                let clear_resp = ui.add(egui::Button::new(
                    if self.clear_hold.is_pressing() { "hold to clear…" } else { "clear" }));
                if clear_resp.is_pointer_button_down_on() {
                    if !self.clear_hold.is_pressing() { self.clear_hold.press(); }
                } else if self.clear_hold.is_pressing() {
                    // Released early: tap_hold returns Tapped, which for a
                    // destructive action means "do nothing" rather than "do it".
                    let _ = self.clear_hold.release();
                }
                let dt = ui.input(|i| i.stable_dt) * 1000.0;
                if let Some(crate::tap_hold::Fired::HeldDown) = self.clear_hold.tick(dt) {
                    self.typed.clear();
                }
                if self.clear_hold.progress() > 0.0 {
                    // Repaint while filling, or the bar advances only when
                    // something else happens to cause a frame.
                    ui.ctx().request_repaint();
                    let p = self.clear_hold.progress();
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(48.0, 3.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(rect.min,
                            egui::vec2(rect.width() * p, rect.height())),
                        1.0, ui.visuals().selection.bg_fill);
                }
            });
            ui.add_space(4.0);
            let shown = if self.typed.is_empty() { "…" } else { self.typed.as_str() };
            ui.label(egui::RichText::new(shown).monospace().size(18.0));
            // The swipe candidates live with the OUTPUT, not over the keys: a strip that
            // covers the top row would hide the letters you are about to swipe next.
            if !self.swipe.candidates.is_empty() {
                let adaptive = Adaptive::detect(ctx);
                // PERSONALISE BEFORE SHOWING. The recognizer scores SHAPE; it
                // cannot know which of two shape-alike words this person
                // actually writes. Reranking here rather than inside the
                // recognizer keeps the shape score honest and auditable — the
                // boost is applied where it can be seen and switched off.
                //
                // board_gestures::WordCandidate already uses the same
                // convention swipe_rerank expects ("Lower is better. 0.0 ~=
                // exact match"), so the recognizer's own scores are reranked
                // directly rather than being re-derived from list position —
                // position would throw away the margin between candidates,
                // which is exactly what decides whether a boost should win.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64).unwrap_or(0);
                // The two previous words, for the n-gram context. Taken from
                // the committed text rather than kept as separate state, so
                // they cannot drift from what is actually on screen.
                let mut prior = self.typed.split_whitespace().rev();
                let prev = prior.next().map(|s| s.to_string());
                let prev_prev = prior.next().map(|s| s.to_string());
                let now_u = now.max(0) as u64;
                let mut shown = self.swipe.candidates.clone();
                for c in shown.iter_mut() {
                    // Both signals, subtracted because score is a COST. The
                    // dictionary boosts words this person uses; the n-gram
                    // boosts words that follow what they just wrote. Either
                    // alone misses half of it.
                    c.score -= self.dict.boost(&c.word, now);
                    let (ng, _) = self.ngram.score_for(
                        prev_prev.as_deref(), prev.as_deref(), &c.word, now_u);
                    c.score -= ng;
                }
                shown.sort_by(|a, b| a.score.partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal));
                // `raw` copied out: the closure below takes `self` mutably
                // (dict, typed, swipe), so reading `self.swipe.raw` inside it
                // is an immutable borrow overlapping a mutable one. Same shape
                // as the two borrow errors in gitmind's pages — the closure
                // only ever wanted the string.
                let raw = self.swipe.raw.clone();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("swiped “{raw}”")).weak().size(12.0));
                    if let Some(word) = candidate_bar(ui, &shown, &adaptive) {
                        if !self.typed.is_empty() && !self.typed.ends_with(' ') && !self.typed.ends_with('\n') {
                            self.typed.push(' ');
                        }
                        // LEARN FROM THE COMMIT, which is the only signal that
                        // says anything: the user picked this word over the
                        // ones beside it, so it is evidence in a way mere
                        // typing frequency is not.
                        self.dict.record_commit(&word, now);
                        // Learn the SEQUENCE too, with the same two words that
                        // scored it — recomputing them after the push would
                        // include the word being learned as its own context.
                        self.ngram.record(prev_prev.as_deref(), prev.as_deref(), &word, now_u);
                        self.service.commit_text(&word);
                        self.typed.push_str(&word);
                        self.swipe.clear_candidates();
                    }
                });
            }
            ui.add_space(4.0);
        });
        } // top bar: only when a feature is enabled

        // The microterm strip and the minimized tray, drawn above the keyboard.

        // Both were ported and unreachable; a tray you cannot open is the same as

        // no tray.

        if self.microterm.is_visible() {

            egui::TopBottomPanel::bottom("microterm").show(ctx, |ui| {

                let c = uikit_egui::palette_colors(self.board.palette());

                // While dictating: finish (type it) or cancel (type nothing). Big
                // targets on purpose; a gloved thumb has to hit them.
                if self.dictation.is_active() {
                    let mut done = false;
                    let mut cancel = false;
                    ui.horizontal(|ui| {
                        let h = 44.0;
                        done = ui.add(egui::Button::new("✓  Done").min_size(egui::vec2(120.0, h))).clicked();
                        cancel = ui.add(egui::Button::new("✕  Cancel").min_size(egui::vec2(120.0, h))).clicked();
                    });
                    if done {
                        self.toggle_dictation();
                    } else if cancel {
                        self.cancel_dictation();
                    }
                }
                if let Some(tapped) = crate::microterm::microterm_ui(ui, &mut self.microterm, &c) {

                    // Every candidate `microterm_ui` can return today comes
                    // from the steno suggestion strip (its match arms only
                    // set `tapped` in the `StenoSuggestions` branch).
                    // Accepting a candidate must first DELETE the raw
                    // phonemes already committed as the user stroked (e.g.
                    // "kat") before inserting the whole word, or the editor
                    // ends up with both ("katcat") — `accept_word`'s
                    // `backspaces` count is exactly that many characters,
                    // matching GTK's `spawn_microterm_bridge` tap callback.
                    let accepted = crate::steno::engine::global().accept_word(&tapped);
                    for _ in 0..accepted.backspaces {
                        self.emit_key("BackSpace");
                    }
                    self.emit_text(&accepted.text);

                }

            });

        }

        if self.settings_open {
            egui::TopBottomPanel::bottom("settings").show(ctx, |ui| {
                ui.label(egui::RichText::new("Settings").strong());
                // SCROLL, AND CONSUME THE DRAG.
                //
                // This panel had no ScrollArea at all, which cost two things
                // on a phone. The list is longer than the space, so the bottom
                // of it was simply unreachable. And a drag over the panel was
                // not consumed by anything, so it fell through to the board
                // underneath and got classified as a key swipe -- which is why
                // scrolling the settings TYPED. The keyboard was behaving
                // correctly; nothing above it had claimed the gesture.
                //
                // `drag_to_scroll` is what makes a touch drag scroll rather
                // than pass through, and it is the half that fixes the typing.
                // Capped at 45% of the screen so the panel cannot grow until
                // it pushes the board it is configuring off the bottom.
                let max_h = ctx.screen_rect().height() * 0.45;
                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    .auto_shrink([false, true])
                    .drag_to_scroll(true)
                    .show(ui, |ui| {
                // Which sections this port can actually honour. `settings::ready`
                // exists precisely so the panel does not offer a switch that
                // does nothing — a control that silently fails is worse than an
                // absent one, because the user changes it and then trusts it.
                let live: Vec<&str> = [
                    (crate::settings::Section::Appearance, "appearance"),
                    (crate::settings::Section::Behavior, "behaviour"),
                    (crate::settings::Section::Gestures, "gestures"),
                    (crate::settings::Section::Layout, "layout"),
                    (crate::settings::Section::Voice, "voice"),
                    (crate::settings::Section::Llm, "llm"),
                ].iter().filter(|(s, _)| crate::settings::ready(*s)).map(|(_, n)| *n).collect();
                ui.label(egui::RichText::new(format!("live sections: {}", live.join(", ")))
                    .weak().size(11.0));

                for g in crate::settings_model::Group::ALL {
                    let items = crate::settings_model::group(g);
                    if items.is_empty() { continue; }
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(g.title()).weak().size(11.0));
                    for setting in items {
                        let cur = self.setting_values.get(setting.key)
                            .cloned()
                            .unwrap_or_else(|| setting.values[0].to_string());
                        let idx = setting.index_of(&cur);
                        let shown = setting.options.get(idx).copied().unwrap_or("?");
                        let r = ui.button(format!("{}: {}", setting.label, shown));
                        if r.clicked() {
                            // settings_model owns the cycling AND says whether
                            // the change needs a relayout or a write — the
                            // panel does not decide that, so a new setting
                            // cannot arrive with the wrong side effects.
                            let change = crate::settings_model::toggle(setting, &cur);
                            self.setting_values.insert(setting.key.to_string(), change.value.clone());
                            // AND APPLY IT — through ONE path, not a growing list of
                            // special cases at the button. The board's mode is the one
                            // thing that is not a style, so it stays explicit; everything
                            // else folds through `Style::with_settings`, which means a new
                            // setting is applied by being added to that function rather
                            // than by remembering to add an `if` here.
                            if setting.key == "layout.experimental_mode" {
                                self.board.apply_mode(&change.value);
                            }
                            self.style = self.base_style.with_settings(&self.setting_values);
                            if change.broadcast { ctx.request_repaint(); }
                        }
                        // The help line is where the options are EXPLAINED —
                        // "staggered matches a physical keyboard; ortholinear is
                        // a strict grid" is the sentence that makes the choice
                        // meaningful, and two bare words leave the user guessing.
                        r.on_hover_text(setting.help);
                    }
                }
                    });
            });
        }

        if self.tray_open {

            egui::TopBottomPanel::bottom("tray").show(ctx, |ui| {

                ui.horizontal(|ui| {

                    if ui.button("expand").clicked() {
                        self.tray.expand();
                        let _ = self.vis.expand();
                        self.tray_open = false;
                    }

                    // The spacebar carries all three gesture vocabularies: tap for a

                    // space, double-tap to toggle selection, swipe for arrows.

                    let (rect, resp) = ui.allocate_exact_size(

                        egui::vec2(ui.available_width() - 80.0, 40.0),

                        egui::Sense::click_and_drag());

                    ui.painter().rect_filled(rect, 6.0,

                        if self.tray.selecting { egui::Color32::from_rgb(80, 70, 40) }

                        else { egui::Color32::from_gray(40) });

                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,

                        if self.tray.selecting { "select" } else { "space" },

                        egui::FontId::proportional(13.0), egui::Color32::from_gray(200));

                    if resp.drag_started() {

                        if let Some(p) = resp.interact_pointer_pos() { self.tray.press(p.x, p.y); }

                    }

                    if resp.drag_stopped() {

                        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;

                        if let Some(p) = resp.interact_pointer_pos() {

                            for ev in self.tray.release(p.x, p.y, now_ms) {

                                if let crate::tray::TrayEvent::Space = ev { self.typed.push(' '); }

                            }

                        }

                    }

                    if ui.button("close").clicked() { self.tray.close(); self.tray_open = false; }

                });

            });

        }


        let mut board_h = 0.0f32;
        let mut inner_top = 0.0f32;
        let central = egui::CentralPanel::default().show(ctx, |ui| {
            inner_top = ui.max_rect().top();
            // THE KEYBOARD'S profile, not the detected one: key height and gap pinned to
            // GTK's, the same for a pointer and a finger. See `board_adaptive`.
            let adaptive = crate::board_adaptive(ctx);
            // Layout switching lives in its OWN strip (the sidebar), never contested with a key's
            // gesture space — a whole-board swipe used to race per-key cardinal swipes (see
            // Board::poll_swipe's doc comment) and intermittently "eat" a key's swipe on real touch
            // hardware. A sidebar tap has no such race.
            // The height the keys will take, known before they are drawn -- the rail is
            // capped to it so the switcher can never be what sets the board's height.
            board_h = if self.real_active {
                crate::board_height(&self.real.render_rc(), &adaptive)
            } else {
                crate::board_height(&self.board.rendered(), &adaptive)
            };
            ui.horizontal_top(|ui| {
                let rail_w = crate::layout_rail::width(self.rail_collapsed);
                ui.allocate_ui(egui::vec2(rail_w, board_h), |ui| {
                    // demo layouts first, then the REAL layout files (shift/symbols live)
                    let mut entries: Vec<crate::layout_rail::Entry> = self
                        .board
                        .layout_names()
                        .iter()
                        .map(|n| crate::layout_rail::Entry::new(n, false))
                        .collect();
                    let demo_n = entries.len();
                    let tab_files = self.real.tab_files();
                    for &fi in &tab_files {
                        entries.push(crate::layout_rail::Entry::new(&self.real.files[fi].display_name, true));
                    }
                    let sel = if self.real_active {
                        // While on the symbols page (no tab of its own) keep the previous tab lit.
                        demo_n + tab_files.iter().position(|&fi| fi == self.real.active).unwrap_or(0)
                    } else {
                        self.board.layout_index()
                    };
                    let colors = uikit_egui::palette_colors(self.board.palette());
                    if let Some(i) = crate::layout_rail::show(ui, &entries, sel, &mut self.rail_collapsed, &colors) {
                        if i < demo_n {
                            self.real_active = false;
                            self.board.set_layout(i);
                        } else {
                            self.real_active = true;
                            self.real.active = tab_files[i - demo_n];
                            self.real.shift = crate::real_layout::Shift::Off;
                        }
                    }
                });
                ui.vertical(|ui| {
                    // The reveal: content settles up from 12px low and fades in, inside the
                    // panel. `set_opacity` scopes to this Ui, so the sidebar and status strip
                    // are already at rest when the keys arrive.
                    let t = self.reveal.progress(crate::platform::REVEAL_MS);
                    ui.add_space(crate::platform::reveal_offset(t, 12.0));
                    ui.set_opacity(0.15 + 0.85 * crate::platform::reveal_progress(t));
                    let board_top = ui.cursor().min;
                    // Whether a KEY produced something this frame. A space zone must never
                    // fire on the same release as a key -- see the zone tap below.
                    let mut key_fired = false;
                    if self.real_active {
                        // the real layout files: render via the projection, feed emissions through
                        // the board's state machine (shift/symbols absorbed), act on the rest
                        let render = self.real.render_rc();
                        if let Some(e) = keyboard_ui_styled(ui, &render, &adaptive, self.board.palette(), self.style) {
                            key_fired = true;
                            // THROUGH THE SINK, not straight into the
                            // preview string. These wrote to `self.typed`
                            // directly, which is the same bug as the
                            // commit queue one layer in: on device the
                            // letters would type and BackSpace and Return
                            // would do nothing, because only the text
                            // path had a destination.
                            //
                            // `self.typed` is still updated, but as a
                            // MIRROR of what was sent rather than as the
                            // destination — so the on-screen preview and
                            // the focused window cannot disagree.
                            match self.real.press(&e) {
                                Some(Press::Text(t)) => {
                                    self.emit_text(&t);
                                }
                                Some(Press::Backspace) => self.emit_key("BackSpace"),
                                Some(Press::Return) => self.emit_key("Return"),
                                Some(Press::Steno(button, gesture)) => {
                                    // Commits verbatim, no auto-space — steno
                                    // phonemes chain directly into syllables
                                    // (`k`+`a`+`t` -> `kat`, not `k a t`),
                                    // matching GTK's
                                    // `EngineInput::StenoCommit` handling.
                                    // A `None` here isn't an error: most
                                    // strokes go pending and commit later,
                                    // either from the opposite thumb's
                                    // `feed` call above or from
                                    // `poll_pending` in `update()`.
                                    if let Some(commit) = crate::steno::feed(&button, &gesture) {
                                        self.emit_text(&commit.text);
                                        if let Some(content) = crate::steno::commit_to_microterm(&commit) {
                                            self.microterm.set_content(content);
                                        }
                                    }
                                }
                                Some(Press::Arrow(a)) => self.emit_key(a.keysym()),
                                Some(Press::Voice) => self.toggle_dictation(),
                                None => {}
                                Some(_) => {}
                            }
                        }
                    } else if let Some(e) = {
                        // The mic key shows the microphone. Set every frame rather than on
                        // toggle, because dictation also stops on its own — a timeout, a
                        // model failure, the sink closing — and a key latched on toggle
                        // would keep pulsing over a microphone that had already closed.
                        self.board.set_voice_active(self.dictation.is_active());
                        keyboard_ui_styled(ui, &self.board.rendered(), &adaptive, self.board.palette(), self.style)
                    } {
                        key_fired = true;
                        // Same decoder as the real layouts, so a mic key or an action token
                        // on a code-built board means the same thing it means on a JSON one.
                        match crate::real_layout::classify(&e) {
                            Press::Backspace => self.emit_key("BackSpace"),
                            Press::Return => self.emit_key("Return"),
                            Press::Voice => self.toggle_dictation(),
                            Press::Text(t) if t == "\u{8}" => self.emit_key("BackSpace"),
                            Press::Text(t) if t == "\n" => self.emit_key("Return"),
                            Press::Text(t) => {
                                self.emit_text(&t);
                                // A one-shot shift is spent by the text it shifted. Caps is
                                // not — that is the whole difference between them.
                                self.board.after_text();
                            }
                            // These arrived as tokens for the first time in the last commit
                            // and fell through a `_ => {}`: better than typing "⇧", and
                            // still a key you press that does nothing.
                            Press::Shift => self.board.tap_shift(),
                            Press::SymbolsToggle => self.board.toggle_symbols(),
                            // THE SAME HANDLER as the real board's. This arm was `{}`, so
                            // the stenotype board the settings can now reach was four big
                            // keys that did nothing — the engine is mounted and tested and
                            // had never had a stroke fed to it from this path.
                            Press::Steno(button, gesture) => self.on_steno(&button, &gesture),
                            // A spacebar swipe moves the caret. The TRAY has had this since
                            // it was ported; the full board had not, so the same gesture on
                            // the same key meant different things depending on whether the
                            // keyboard was minimized.
                            Press::Arrow(a) => self.emit_key(a.keysym()),
                        }
                    }
                    // ── swipe typing over whichever board is showing ──────────────────
                    let board_rect = egui::Rect::from_min_max(board_top, ui.cursor().min
                        + egui::vec2(ui.available_width(), 0.0));
                    // The swipe overlay reads the same layout the keys were drawn from: the
                    // cached projection for a real file (no second projection), a borrow of
                    // the demo board (no clone — `swipe` and `board` are disjoint fields).
                    let real_render = if self.real_active { Some(self.real.render_rc()) } else { None };
                    let render: &crate::Layout = match &real_render {
                        Some(r) => r,
                        None => self.board.current(),
                    };
                    let mut top_edge = None;
                    let mut springboard: Vec<String> = Vec::new();
                    let mut trailing_space = false;
                    // THE TOP EDGE, before the swipe is read as a word. `single-hand-mode.md`
                    // §5/§6 put a digit arc and a minimize fling on the same edge, and its
                    // tie-break is explicit: prefer the digit, because losing your keyboard
                    // while reaching for `3` is destructive and a stray digit is one
                    // backspace. `topedge` holds that rule; this supplies the geometry.
                    if let Some((press, release, len, ms)) = self.swipe.take_finished() {
                        let m = crate::topedge::Metrics {
                            top_y: board_rect.top(),
                            row_h: adaptive.min_touch_target,
                            screen_top_y: ctx.screen_rect().top(),
                        };
                        // Which top-row column the press landed in, by POSITION — the doc's
                        // rule, so QWERTY and Dvorak need no per-layout digit table.
                        let col = render.rows.first().and_then(|row| {
                            if !board_rect.contains(press) || press.y > board_rect.top() + adaptive.min_touch_target {
                                return None;
                            }
                            let n = row.keys.len().max(1);
                            let frac = ((press.x - board_rect.left()) / board_rect.width()).clamp(0.0, 0.999);
                            Some((frac * n as f32) as usize)
                        });
                        // Decided here, ACTED ON below: `render` borrows `self.board`, and
                        // emitting text needs `&mut self`. Deciding first and acting after
                        // the borrow ends keeps both without a clone of the layout.
                        top_edge = Some(crate::topedge::classify(press, release, len, ms, col, m));
                    }
                    if self.swipe.update_half(ui, render, board_rect, &adaptive, self.style.half).is_some() {
                        // A completed swipe suppresses nothing: the candidates appear above and
                        // the user picks. Committing automatically would fight the tap that the
                        // same gesture's lift also produces on some touch stacks.
                        ctx.request_repaint();
                        // EXCEPT the words a springboard crossing already closed. Those are
                        // finished — the user ended each one by leaving the letters — and
                        // offering them as candidates would ask for confirmation of a
                        // decision already made with the thumb. Committed here, the tail
                        // still goes to the candidate bar.
                        springboard = std::mem::take(&mut self.swipe.springboard_words);
                        trailing_space = self.swipe.trailing_space;
                        // Where each crossing counted. The doc: "teaches the user that the
                        // crossing counted" — a springboard space that happens invisibly is
                        // a feature nobody learns they have.
                        let now_ms = ui.input(|i| i.time) * 1000.0;
                        for d in std::mem::take(&mut self.swipe.delimiters) {
                            self.ripples.push((d, now_ms));
                        }
                    }
                    // THE SPACE ZONES (§4). `springboard::zones` has computed these since
                    // the last commit and nothing drew or tapped them, so the strip a
                    // compacted board frees was a live region with no visible job — and the
                    // bottom zone, which every board has, was invisible too.
                    // The zones are laid around the KEYS, which is all of `board_rect` now that
                    // the board occupies exactly its rows. This subtracted one key height and
                    // called the rest "letters", so the bottom zone sat ON the bottom row: a tap
                    // on ⌫, space or ⏎ there typed the key AND a space from the zone under it --
                    // the "delete gives delete plus space" report. With the keys as the letters
                    // rect the bottom zone only exists where no key is (`springboard::zones`).
                    let letters = crate::board_area(board_rect);
                    // THE CALIBRATION TAKES THE BOARD while it is open: the overlay measures
                    // a reach against these keys, and letting a sweep also type would give the
                    // operator a screen full of letters at the moment they are concentrating
                    // on their thumb.
                    if self.thumb_cal.is_some() {
                        let colors = uikit_egui::palette_colors(self.board.palette());
                        let mut cal = self.thumb_cal.take().expect("checked");
                        // The sweep, from the same pointer the board would have used.
                        if ui.input(|i| i.pointer.primary_down()) {
                            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                                cal.sample((
                                    (p.x - board_rect.min.x) / board_rect.width().max(1.0),
                                    (p.y - board_rect.min.y) / board_rect.height().max(1.0),
                                ));
                            }
                        } else if ui.input(|i| i.pointer.any_released()) {
                            cal.finish();
                        }
                        match crate::thumbcal::calibration_ui(ui, &mut cal, board_rect, &colors) {
                            Some(crate::thumbcal::CalAction::Accept(_)) => {
                                // The REACH, not the polygon: `Style` derives the arc per
                                // board rect, so storing a shape fitted to one size would be
                                // wrong the moment the keyboard was resized or rotated.
                                if let crate::thumbcal::Step::Confirm { reach, .. } = cal.step {
                                    self.style.calibrated_reach = Some(reach);
                                    self.base_style.calibrated_reach = Some(reach);
                                }
                            }
                            Some(crate::thumbcal::CalAction::Cancel) => {}
                            // Retry keeps the overlay up; the state machine already reset.
                            Some(crate::thumbcal::CalAction::Retry) => self.thumb_cal = Some(cal),
                            None => self.thumb_cal = Some(cal),
                        }
                        return;
                    }
                    let zones = crate::springboard::zones(crate::board_area(board_rect), letters, self.style.half);
                    let swiping = self.swipe.is_active();
                    {
                        let colors = uikit_egui::palette_colors(self.board.palette());
                        let painter = ui.painter();
                        // The post-word flash: every visible zone pulses, the one nearest
                        // the last touch harder. "Teaches the multi-anchor design" — the
                        // zones are only discoverable if something points at them when a
                        // word lands.
                        let now_ms = ui.input(|i| i.time) * 1000.0;
                        let nearest = self
                            .last_touch
                            .and_then(|p| crate::springboard::nearest_zone(&zones, p));
                        for (i, z) in zones.iter().enumerate() {
                            // Faded while suppressed rather than hidden: a strip that
                            // vanishes mid-swipe reads as the keyboard flickering, and the
                            // doc asks for "faded out, non-tappable", not gone.
                            let live = !swiping || z.live_while_swiping;
                            let base = if live { 26.0f32 } else { 10.0 };
                            let (boost, scale) = self
                                .word_at
                                .and_then(|t| crate::springboard::pulse((now_ms - t) as f32, nearest == Some(i)))
                                .unwrap_or((0.0, 1.0));
                            let a = (base + boost * 60.0).clamp(0.0, 255.0) as u8;
                            let rect = egui::Rect::from_center_size(z.rect.center(), z.rect.size() * scale);
                            painter.rect_filled(
                                rect,
                                egui::CornerRadius::same(4),
                                egui::Color32::from_rgba_unmultiplied(
                                    colors.accent.r(), colors.accent.g(), colors.accent.b(), a,
                                ),
                            );
                            // The doc's "small ␣ icon at its geometric center" — the mark is
                            // what teaches the strip is a spacebar rather than dead margin.
                            painter.text(
                                z.rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "␣",
                                egui::FontId::proportional(13.0),
                                colors.text.gamma_multiply(if live { 0.55 } else { 0.25 }),
                            );
                        }
                    }
                    // The delimiter ripples, drained as they expire.
                    {
                        let colors = uikit_egui::palette_colors(self.board.palette());
                        let now_ms = ui.input(|i| i.time) * 1000.0;
                        let painter = ui.painter();
                        self.ripples.retain(|(at, born)| {
                            match crate::springboard::ripple((now_ms - born) as f32) {
                                Some((r, alpha)) => {
                                    painter.circle_stroke(
                                        *at,
                                        r,
                                        egui::Stroke::new(
                                            2.0,
                                            egui::Color32::from_rgba_unmultiplied(
                                                colors.accent.r(),
                                                colors.accent.g(),
                                                colors.accent.b(),
                                                (alpha * 200.0) as u8,
                                            ),
                                        ),
                                    );
                                    true
                                }
                                None => false,
                            }
                        });
                        if !self.ripples.is_empty() {
                            ctx.request_repaint();
                        }
                    }
                    // A tap in a live zone is a plain space -- only when the press STARTED in
                    // the zone as well, and never on a frame a key fired. Either guard alone
                    // closes the double-fire; both, because a zone is exactly the kind of
                    // region that ends up under a key again after the next layout change.
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        self.last_touch = Some(p);
                        let origin = ui.input(|i| i.pointer.press_origin());
                        if !key_fired
                            && ui.input(|i| i.pointer.primary_released())
                            && crate::springboard::zone_at(&zones, p, swiping).is_some()
                            && origin.is_some_and(|o| crate::springboard::zone_at(&zones, o, swiping).is_some())
                        {
                            self.emit_text(" ");
                        }
                    }
                    let committed = !springboard.is_empty() || trailing_space;
                    for w in springboard {
                        self.emit_text(&w);
                        self.emit_text(" ");
                    }
                    if trailing_space {
                        self.emit_text(" ");
                    }
                    if committed {
                        self.word_at = Some(ui.input(|i| i.time) * 1000.0);
                    }
                    match top_edge {
                        Some(crate::topedge::TopEdge::Digit(d)) => self.emit_text(&d.to_string()),
                        Some(crate::topedge::TopEdge::Minimize) => {
                            let _ = self.vis.fling_up();
                            self.tray_open =
                                self.vis.mode() == crate::visible_mode::VisibleMode::Tray;
                        }
                        _ => {}
                    }
                    if self.swipe.is_active() {
                        let colors = uikit_egui::palette_colors(self.board.palette());
                        // The trail carries the gesture CATEGORY when debug styling is on:
                        // dots = the cardinal grid claimed it, stripes = it decoded as a
                        // swiped word, grey = nothing useful. Off by default, so the normal
                        // case is one accent line and the style argument costs an atomic read.
                        draw_trail_styled(ui, self.swipe.path(), colors.accent, self.trail_style);
                        ctx.request_repaint();
                    }
                });
            });
        });

        // TRIM THE SURFACE TO THE CONTENT. The window was a fixed 340pt and the board is
        // four or five 50pt rows, so a strip of nothing sat under the keys. GTK sizes its
        // surface to `rows * key_h + gaps + chrome` on every rebuild
        // (`keyboard_view.rs`); this is the same, measured: whatever the panels above and
        // below take, the central panel's own margins, and the board's height -- never the
        // reveal's transient offset, which would ask for a taller window for 200ms.
        let panel = central.response.rect;
        let screen = ctx.content_rect();
        let margin = (inner_top - panel.top()).max(0.0);
        let above = panel.top() - screen.top();
        let below = screen.bottom() - panel.bottom();
        let want = (above + margin + board_h + margin + below).ceil();
        self.desired_h = Some(want);
        // Floating / ruled windows are resized through eframe; the layer-shell host reads
        // `desired_height` and resizes its surface itself (the command is not delivered
        // there). Asked once per change, so a user who drags the window taller keeps it
        // until the content changes again.
        if self.placement != crate::platform::Placement::LayerShell
            && self.asked_h.is_none_or(|a| (a - want).abs() > 0.5)
            && (screen.height() - want).abs() > 0.5
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(screen.width(), want)));
            self.asked_h = Some(want);
        }
    }

    /// The surface height the last frame's content needed: the keys and any open panels,
    /// nothing under them. `None` before the first frame. The layer-shell host sizes its
    /// surface (and exclusive zone) from this, as GTK's does from its computed height.
    pub fn desired_height(&self) -> Option<f32> {
        self.desired_h
    }
}


/// Run the keyboard app. Both `src/bin/rusty_board_egui.rs` (the deploy binary) and
/// `examples/feel.rs` (the dev harness) are one call to this.
/// Must match `packaging/org.rustyboard.RustyBoard.desktop` exactly. A mismatch
/// fails silently: the keyboard runs and has no icon or dock grouping.
pub const APP_ID: &str = "org.rustyboard.RustyBoard";

pub fn run(title: &str) -> eframe::Result<()> {
    // A SECOND keyboard is worse than untidy: it takes the input focus the
    // first one was serving, so the user ends up typing into nothing. Refusing
    // the second launch is the correct behaviour here even though it would be
    // merely tidy for an ordinary app.
    let claim = uikit_egui::single_instance::claim(APP_ID);
    if !uikit_egui::single_instance::should_start(&claim) {
        if let uikit_egui::single_instance::Claim::AlreadyRunning(pid) = claim {
            eprintln!("rusty-board is already running (pid {pid})");
        }
        return Ok(());
    }
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        // An on-screen keyboard wants to be WIDE and SHORT, and it must not
        // steal focus from the app being typed into — a keyboard that takes
        // focus types into itself.
        viewport: {
            let v = uikit_egui::desktop::viewport(APP_ID, title, [900.0, 340.0])
                .with_resizable(true)
                .with_active(false);
            match crate::platform::placement() {
                // Docked-by-rule: no titlebar to grab, and ask for the top level. On Wayland
                // the level is the rule's job (winit cannot set it); on X11 it is honoured.
                crate::platform::Placement::RuledToplevel | crate::platform::Placement::LayerShell =>
                    v.with_decorations(false).with_always_on_top(),
                crate::platform::Placement::Floating => v,
            }
        },
        ..Default::default()
    };
    let r = eframe::run_native(APP_ID, opts, Box::new(|_cc| Ok(Box::new(Feel::new()))));
    // Released on the way out so a restart is immediate. The runtime dir is
    // cleared at logout anyway, so a missed release costs nothing.
    uikit_egui::single_instance::release(APP_ID);
    r
}

/// The user's voice filter strength from the fleet settings file settingsd
/// persists (bsettings edits it): `$XDG_CONFIG_HOME/gitmind/settings.conf`,
/// `k = v` lines. Standard when there is no file or no key.
pub fn fleet_voice_filter() -> settings_kit::VoiceFilter {
    let path = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|b| b.join("gitmind").join("settings.conf"));
    let text = path.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    voice_filter_from_settings(&text)
}

pub fn voice_filter_from_settings(text: &str) -> settings_kit::VoiceFilter {
    let kv: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    settings_kit::Prefs::from_kv(&kv).a11y.voice_filter
}

#[cfg(test)]
mod voice_session_tests {
    use super::*;

    #[test]
    fn the_filter_level_comes_from_the_fleet_settings() {
        assert_eq!(voice_filter_from_settings("a11y.voice_filter = off\n"), settings_kit::VoiceFilter::Off);
        assert_eq!(voice_filter_from_settings(""), settings_kit::Prefs::default().a11y.voice_filter);
    }

    #[test]
    fn cancel_types_nothing_and_every_stop_idles_the_mic_and_gives_the_keyboard_back() {
        let mut f = Feel::new();
        // Without the voice engine the key must not pretend to listen.
        f.toggle_dictation();
        #[cfg(not(feature = "voice"))]
        {
            assert!(!f.dictation.is_active());
            assert!(matches!(f.microterm.content(), crate::microterm::Content::Status(s) if s.contains("isn't in this build")));
            return;
        }
        #[allow(unreachable_code)]
        {
            f.dictation.start();
            f.dictation.partial("hello there");
            f.cancel_dictation();
            assert!(!f.dictation.is_active());
            assert!(matches!(f.mic, crate::mic_source::MicState::Idle));
            assert!(f.service.take_pending().is_empty(), "cancel typed nothing");
        }
    }
}
