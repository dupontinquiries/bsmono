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
use crate::{apply_theme, keyboard_ui, Board};
use uikit_egui::Adaptive;

struct Feel {
    board: Board,
    /// The minimized tray, the microterm and dictation — three ported surfaces
    /// the demo could not reach. A keyboard that cannot minimise, cannot show
    /// its terminal and cannot dictate is not the keyboard being shipped, and
    /// the modules existed the whole time.
    tray: crate::tray::Tray,
    tray_open: bool,
    microterm: crate::microterm::Microterm,
    dictation: crate::dictation::Dictation,
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
    pub fn new() -> Feel {
        Feel {
            trail_style: TrailStyle::Solid,
            board: Board::new(),
            real: RealBoard::new(),
            real_active: false,
            typed: String::new(),
            // A starter word list so the demo binary can be swiped immediately; on-device the
            // host loads rusty-board's shipped dictionary into the same recognizer.
            tray: Default::default(),
            tray_open: false,
            microterm: Default::default(),
            dictation: Default::default(),
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

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_theme(ctx);
        // Drain the service queue once per frame. Without this `pending` grows
        // for the life of the process — a queue with no reader is a leak, and
        // the model cannot know whether a host exists to read it.
        //
        // In a real embedding these become `TextCommitted` signals; here they
        // are counted, which is enough to prove the wiring end to end rather
        // than only up to the commit.
        let dt_ms = ctx.input(|i| i.stable_dt) * 1000.0;
        for _ in self.service.take_pending() {
            self.committed += 1;
            // Start on the DRAIN rather than at each commit site: one flash per
            // frame however many words landed, and every commit path gets it
            // without three separate calls to keep in step.
            self.commit_flash.start();
        }
        self.commit_flash.tick(dt_ms, crate::animations::COMMIT_MS);
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

        egui::Panel::top("out").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // A diagnostic that ships switched off and is reachable without a rebuild —
                // the same bargain as snapclone's trace channels. On device this is what tells
                // you whether a gesture was read as cardinal, swipe, or nothing at all.
                // MINIMISE / MICROTERM / DICTATE — the three surfaces the demo
                // never reached. Placed on the top strip because that is where
                // the real keyboard puts its chrome, and because a control you
                // cannot find is the same as a control that does not exist.
                if ui.selectable_label(self.tray_open, "▁ tray").clicked() {
                    self.tray_open = !self.tray_open;
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
                        if listening {
                            // PartialsOnly: the last partial IS the result, so
                            // release commits without a second inference.
                            if let Some(text) = self.dictation.finish() {
                                if !self.typed.is_empty() && !self.typed.ends_with(' ') {
                                    self.typed.push(' ');
                                }
                                self.service.commit_text(&text);
                                self.typed.push_str(&text);
                            }
                        } else {
                            // The broker decides. On device the host supplies a
                            // real one; until then the state stays Idle rather
                            // than pretending a grant was obtained, because
                            // claiming to be listening when nothing is captured
                            // is exactly the dud-demo failure.
                            self.dictation.start();
                        }
                    }
                    if listening {
                        // The partial goes to the MICROTERM's voice preview,
                        // which is the surface built for it — showing it in the
                        // toolbar instead would leave that surface unreachable
                        // again by a different route.
                        self.microterm.set_content(crate::microterm::Content::VoicePreview {
                            partial: self.dictation.partial().to_string(), level: 0.4 });
                        self.microterm.show();
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
                if ui.checkbox(&mut dbg, "trail shows gesture category").changed() {
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
                ui.label("hold a key for accents · drag across letters to swipe-type · tap a sidebar tab to switch layout");
                // The service's commit count, shown because a silent
                // pass-through is indistinguishable from one that is not wired
                // — which is exactly how this module sat unreached with 19
                // passing tests.
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

        // The microterm strip and the minimized tray, drawn above the keyboard.

        // Both were ported and unreachable; a tray you cannot open is the same as

        // no tray.

        if self.microterm.is_visible() {

            egui::TopBottomPanel::bottom("microterm").show(ctx, |ui| {

                let c = uikit_egui::palette_colors(self.board.palette());

                if let Some(tapped) = crate::microterm::microterm_ui(ui, &mut self.microterm, &c) {

                    // Every commit path goes through the service, not just the
                    // interesting ones. A host that receives swiped words but
                    // not typed ones is worse than a host that receives
                    // nothing: the gap looks like dropped input rather than a
                    // missing integration.
                    self.service.commit_text(&tapped);
                    self.typed.push_str(&tapped);

                }

            });

        }

        if self.settings_open {
            egui::TopBottomPanel::bottom("settings").show(ctx, |ui| {
                ui.label(egui::RichText::new("Settings").strong());
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
                            self.setting_values.insert(setting.key.to_string(), change.value);
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
        }

        if self.tray_open {

            egui::TopBottomPanel::bottom("tray").show(ctx, |ui| {

                ui.horizontal(|ui| {

                    if ui.button("expand").clicked() { self.tray.expand(); self.tray_open = false; }

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


        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            // Layout switching lives in its OWN strip (the sidebar), never contested with a key's
            // gesture space — a whole-board swipe used to race per-key cardinal swipes (see
            // Board::poll_swipe's doc comment) and intermittently "eat" a key's swipe on real touch
            // hardware. A sidebar tap has no such race.
            ui.horizontal(|ui| {
                let sidebar_w = 64.0_f32.max(adaptive.min_touch_target * 1.3);
                ui.allocate_ui(egui::vec2(sidebar_w, ui.available_height()), |ui| {
                    // demo layouts first, then the REAL layout files (shift/symbols live)
                    let mut names: Vec<String> = self.board.layout_names().iter().map(|s| s.to_string()).collect();
                    let demo_n = names.len();
                    for f in &self.real.files {
                        if f.name != "symbols" { // symbols is reached via the ?123 key, not a tab
                            names.push(format!("{} ⌨", f.display_name));
                        }
                    }
                    let tabs: Vec<uikit_egui::vertical_tabs::Tab> = names.iter().map(|n| uikit_egui::vertical_tabs::Tab::new(n)).collect();
                    let sel = if self.real_active { demo_n + self.real.active.min(1) } else { self.board.layout_index() };
                    let colors = uikit_egui::palette_colors(self.board.palette());
                    if let Some(i) = uikit_egui::vertical_tabs::show(ui, &tabs, sel, adaptive.min_touch_target, &colors) {
                        if i < demo_n {
                            self.real_active = false;
                            self.board.set_layout(i);
                        } else {
                            self.real_active = true;
                            self.real.active = i - demo_n; // qwerty=0, dvorak=1 (symbols excluded)
                            self.real.shift = crate::real_layout::Shift::Off;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    let board_top = ui.cursor().min;
                    if self.real_active {
                        // the real layout files: render via the projection, feed emissions through
                        // the board's state machine (shift/symbols absorbed), act on the rest
                        let render = self.real.render();
                        if let Some(e) = keyboard_ui(ui, &render, &adaptive, self.board.palette()) {
                            match self.real.press(&e) {
                                Some(Press::Text(t)) => self.typed.push_str(&t),
                                Some(Press::Backspace) => { self.typed.pop(); }
                                Some(Press::Return) => self.typed.push('\n'),
                                Some(Press::Voice) | None => {}
                                Some(_) => {}
                            }
                        }
                    } else if let Some(e) = keyboard_ui(ui, self.board.current(), &adaptive, self.board.palette()) {
                        if e == "\u{8}" {
                            self.typed.pop();
                        } else {
                            self.typed.push_str(&e);
                        }
                    }
                    // ── swipe typing over whichever board is showing ──────────────────
                    let board_rect = egui::Rect::from_min_max(board_top, ui.cursor().min
                        + egui::vec2(ui.available_width(), 0.0));
                    let render = if self.real_active { self.real.render() } else { self.board.current().clone() };
                    if self.swipe.update(ui, &render, board_rect, &adaptive).is_some() {
                        // A completed swipe suppresses nothing: the candidates appear above and
                        // the user picks. Committing automatically would fight the tap that the
                        // same gesture's lift also produces on some touch stacks.
                        ctx.request_repaint();
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
        // An on-screen keyboard wants to be WIDE and SHORT, and it must not
        // steal focus from the app being typed into — a keyboard that takes
        // focus types into itself.
        viewport: uikit_egui::desktop::viewport(APP_ID, title, [900.0, 340.0])
            .with_resizable(true)
            .with_active(false),
        ..Default::default()
    };
    let r = eframe::run_native(APP_ID, opts, Box::new(|_cc| Ok(Box::new(Feel::new()))));
    // Released on the way out so a restart is immediate. The runtime dir is
    // cleared at logout anyway, so a missed release costs nothing.
    uikit_egui::single_instance::release(APP_ID);
    r
}
