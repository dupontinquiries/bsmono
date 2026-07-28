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
    /// The REAL layout files (resources/layouts/*.json), with shift/caps + the symbols toggle —
    /// shown when the sidebar's real-layout tabs are active (indices past the demo layouts).
    real: RealBoard,
    real_active: bool,
    typed: String,
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
            swipe: SwipeState::with_dictionary(
                include_str!("../demo_words.txt").lines().map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty()).collect()),
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

        egui::Panel::top("out").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // A diagnostic that ships switched off and is reachable without a rebuild —
                // the same bargain as snapclone's trace channels. On device this is what tells
                // you whether a gesture was read as cardinal, swipe, or nothing at all.
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
                if ui.button("clear").clicked() {
                    self.typed.clear();
                }
            });
            ui.add_space(4.0);
            let shown = if self.typed.is_empty() { "…" } else { self.typed.as_str() };
            ui.label(egui::RichText::new(shown).monospace().size(18.0));
            // The swipe candidates live with the OUTPUT, not over the keys: a strip that
            // covers the top row would hide the letters you are about to swipe next.
            if !self.swipe.candidates.is_empty() {
                let adaptive = Adaptive::detect(ctx);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("swiped “{}”", self.swipe.raw)).weak().size(12.0));
                    if let Some(word) = candidate_bar(ui, &self.swipe.candidates, &adaptive) {
                        if !self.typed.is_empty() && !self.typed.ends_with(' ') && !self.typed.ends_with('\n') {
                            self.typed.push(' ');
                        }
                        self.typed.push_str(&word);
                        self.swipe.clear_candidates();
                    }
                });
            }
            ui.add_space(4.0);
        });

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
pub fn run(title: &str) -> eframe::Result<()> {
    eframe::run_native(
        title,
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(Feel::new()))),
    )
}
