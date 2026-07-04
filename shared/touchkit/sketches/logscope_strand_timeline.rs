//! ════════════════════════════════════════════════════════════════════════════
//!  ILLUSTRATIVE SKETCH — NOT COMPILED here (needs egui + eframe + egui_input.rs).
//! ════════════════════════════════════════════════════════════════════════════
//!
//! Part 2 of the side-by-side: the *reverse port*. snapclone's value isn't only
//! its photo UI — it's the touch-abstraction layer underneath it. `touchkit` is
//! UI-agnostic, so a screen that has nothing to do with photos can adopt the same
//! higher layer and get scrub / zoom / strand-switch "for free."
//!
//! logscope is the candidate. Its GUI was ported GTK -> egui, and like most desktop
//! tools it is **click-and-keyboard only**: to move along the timeline you press a
//! button, to change strand you press a button, to zoom you press a button. That is
//! exactly the gap snapclone's touch layer fills.
//!
//! THE THESIS, made literal below: the gesture handler and the baseline toolbar
//! buttons call the *same three mutators* — `scrub`, `focus_delta`, `zoom_by`.
//!   • touch layer OFF  → only the buttons call them (logscope as it is today)
//!   • touch layer ON   → `touchkit` feeds the same intents, but continuously and
//!                          with fling momentum, from trackpad / touchscreen / drag
//! Flip the checkbox in the toolbar to feel the before/after. Nothing about the
//! timeline rendering changes; only where the intents come from.
//!
//! The interactive HTML demo (`snapclone-egui-demo.html`, tab "logscope + touch")
//! is the visual reference for the strands, the nodes-on-rails, and the verifier
//! badges. This sketch shows how the same thing is wired in real egui. The layout
//! math is real but abbreviated; it has not been built in this environment.
//!
//! logscope palette (cooler than snapclone's neutral grey, to read as a different
//! app in the side-by-side):
//!   panel #161a1d · rail #2a3138 · rail-active #3d4d57 · node #232a30
//!   accent (cyan) #45c8c0 · text #dfe6ea
//!   verifier  pass #57b894 · fail #e5534b · pending #d9a13b

mod egui_input; // the same ~50-line bridge snapclone's sketch uses
use egui_input::TouchSource;
use touchkit::{Gesture, GestureRecognizer};

const PANEL: egui::Color32 = egui::Color32::from_rgb(0x16, 0x1a, 0x1d);
const RAIL: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x31, 0x38);
const RAIL_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x3d, 0x4d, 0x57);
const NODE: egui::Color32 = egui::Color32::from_rgb(0x23, 0x2a, 0x30);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x45, 0xc8, 0xc0);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xdf, 0xe6, 0xea);
const V_PASS: egui::Color32 = egui::Color32::from_rgb(0x57, 0xb8, 0x94);
const V_FAIL: egui::Color32 = egui::Color32::from_rgb(0xe5, 0x53, 0x4b);
const V_PEND: egui::Color32 = egui::Color32::from_rgb(0xd9, 0xa1, 0x3b);

const STRANDS: usize = 4;

/// Each node carries an observable render-verifier result — logscope's core idea:
/// every step in the history is a node-on-a-strand that knows whether its render
/// still passes.
#[derive(Clone, Copy, PartialEq)]
enum Verifier {
    Pass,
    Fail,
    Pending,
}

impl Verifier {
    fn color(self) -> egui::Color32 {
        match self {
            Verifier::Pass => V_PASS,
            Verifier::Fail => V_FAIL,
            Verifier::Pending => V_PEND,
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Verifier::Pass => "✓",
            Verifier::Fail => "✕",
            Verifier::Pending => "…",
        }
    }
}

struct Node {
    /// Position along the strand, in "timeline units" (scaled by zoom at draw).
    t: f32,
    label: &'static str,
    verifier: Verifier,
}

struct Strand {
    name: &'static str,
    nodes: Vec<Node>,
}

pub struct LogscopeApp {
    // ── snapclone's touch layer, dropped in unchanged ───────────────────────
    rec: GestureRecognizer,
    input: TouchSource,
    /// The reverse-port switch. OFF = baseline logscope (buttons only).
    touch_enabled: bool,

    // ── timeline view state (the three things both buttons and gestures move) ─
    scroll_x: f32, // horizontal scrub position, px
    active: usize, // focused strand, 0..STRANDS
    zoom: f32,     // node spacing multiplier, 0.5..3.0

    strands: Vec<Strand>,
    selected: Option<(usize, usize)>, // (strand, node) under the last tap
}

impl Default for LogscopeApp {
    fn default() -> Self {
        // Four strands of deterministic history, matching the HTML demo's shape:
        // mostly-passing rails with a couple of failures and a pending tip.
        let mk = |name, seed: u32| {
            let nodes = (0..9)
                .map(|i| {
                    let v = match (seed + i) % 7 {
                        3 => Verifier::Fail,
                        6 => Verifier::Pending,
                        _ => Verifier::Pass,
                    };
                    Node {
                        t: i as f32 * 90.0,
                        label: NODE_LABELS[(i as usize) % NODE_LABELS.len()],
                        verifier: v,
                    }
                })
                .collect();
            Strand { name, nodes }
        };
        Self {
            rec: GestureRecognizer::default(),
            input: TouchSource::default(),
            touch_enabled: true,
            scroll_x: 0.0,
            active: 0,
            zoom: 1.0,
            strands: vec![
                mk("render-core", 0),
                mk("scene-view", 2),
                mk("gemm-backend", 5),
                mk("ui-egui-port", 1),
            ],
            selected: None,
        }
    }
}

const NODE_LABELS: [&str; 6] = [
    "init", "diff", "render", "verify", "checkpoint", "merge",
];

impl eframe::App for LogscopeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_logscope_theme(ctx);

        // ── the reverse port, in three lines ─────────────────────────────────
        // Always poll touchkit (cheap), but only ACT on it when the layer is on.
        // When off, the gestures are simply ignored and the toolbar buttons are
        // the only way to drive `scrub` / `focus_delta` / `zoom_by`.
        let (gestures, gliding) = self.input.poll(ctx, &mut self.rec);
        if self.touch_enabled {
            self.handle_timeline_gestures(&gestures);
            if gliding {
                ctx.request_repaint(); // keep a fling scrub coasting
            }
        }

        self.toolbar(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(PANEL))
            .show(ctx, |ui| {
                self.draw_timeline(ui);
                self.draw_legend(ui);
            });
    }
}

impl LogscopeApp {
    // ── the three mutators: the ONLY way view state changes ──────────────────
    // Both the gesture handler and the baseline buttons funnel through these, so
    // "with touch" and "without touch" are provably the same intents.

    fn scrub(&mut self, dx: f32) {
        self.scroll_x = (self.scroll_x + dx).clamp(0.0, self.max_scroll());
    }
    fn focus_delta(&mut self, d: i32) {
        let n = STRANDS as i32;
        self.active = ((self.active as i32 + d).rem_euclid(n)) as usize;
    }
    fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.5, 3.0);
    }

    // ── touch path: map touchkit gestures onto those three mutators ──────────
    fn handle_timeline_gestures(&mut self, gestures: &[Gesture]) {
        for g in gestures {
            match g {
                // Horizontal drag scrubs the timeline; fling keeps it coasting.
                Gesture::Pan { delta, .. } => self.scrub(-delta.x),
                Gesture::MomentumTick { delta } => self.scrub(-delta.x),
                // Vertical flicks change which strand is in focus — the gesture
                // snapclone uses to flip between photos, repurposed for strands.
                Gesture::SwipeUp { .. } => self.focus_delta(-1),
                Gesture::SwipeDown { .. } => self.focus_delta(1),
                // Pinch / ctrl-scroll zooms node spacing.
                Gesture::Pinch { rel, .. } => self.zoom_by(*rel),
                // A tap that wasn't a drag selects the node under it.
                Gesture::Tap(p) if !self.rec.is_dragging() => {
                    self.selected = self.node_at(*p);
                }
                _ => {}
            }
        }
    }

    // ── view math ────────────────────────────────────────────────────────────
    fn node_dx(&self) -> f32 {
        90.0 * self.zoom // base spacing scaled by zoom
    }
    fn max_scroll(&self) -> f32 {
        let longest = self
            .strands
            .iter()
            .map(|s| s.nodes.len())
            .max()
            .unwrap_or(0) as f32;
        (longest * self.node_dx() - 600.0).max(0.0)
    }

    /// Hit-test a screen point against laid-out node dots. A real build stores the
    /// dot rects during `draw_timeline`; abbreviated here to the nearest strand+node.
    fn node_at(&self, _p: touchkit::Vec2) -> Option<(usize, usize)> {
        self.selected.or(Some((self.active, 0)))
    }

    // ── toolbar: the toggle + the baseline buttons ───────────────────────────
    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("ls_toolbar")
            .frame(egui::Frame::none().fill(PANEL).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("logscope").color(ACCENT).strong());
                    ui.separator();
                    ui.label(format!("strand: {}", self.strands[self.active].name));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // THE reverse-port switch.
                        ui.checkbox(&mut self.touch_enabled, "touch layer (snapclone)");
                        ui.separator();

                        // Baseline navigation — the only controls logscope has today.
                        // They are always visible, but they're the *whole* interface
                        // when the touch layer is off, and a fallback when it's on.
                        ui.scope(|ui| {
                            ui.set_enabled(!self.touch_enabled);
                            if ui.small_button("zoom +").clicked() {
                                self.zoom_by(1.15);
                            }
                            if ui.small_button("zoom −").clicked() {
                                self.zoom_by(1.0 / 1.15);
                            }
                            if ui.small_button("strand ▼").clicked() {
                                self.focus_delta(1);
                            }
                            if ui.small_button("strand ▲").clicked() {
                                self.focus_delta(-1);
                            }
                            if ui.small_button("scrub ▶").clicked() {
                                self.scrub(self.node_dx());
                            }
                            if ui.small_button("◀ scrub").clicked() {
                                self.scrub(-self.node_dx());
                            }
                        });
                    });
                });
            });
    }

    // ── rendering (abbreviated; HTML demo is the visual source of truth) ─────
    fn draw_timeline(&mut self, ui: &mut egui::Ui) {
        let area = ui.max_rect();
        let painter = ui.painter().clone();
        let dx = self.node_dx();
        let lane_h = (area.height() - 70.0) / STRANDS as f32;
        let x0 = area.left() + 24.0 - self.scroll_x;

        for (si, strand) in self.strands.iter().enumerate() {
            let cy = area.top() + 16.0 + lane_h * (si as f32 + 0.5);
            let is_active = si == self.active;

            // the rail
            let rail_color = if is_active { RAIL_ACTIVE } else { RAIL };
            let rail_w = if is_active { 3.0 } else { 1.5 };
            painter.line_segment(
                [egui::pos2(area.left() + 8.0, cy), egui::pos2(area.right() - 8.0, cy)],
                egui::Stroke::new(rail_w, rail_color),
            );
            // strand name gutter
            painter.text(
                egui::pos2(area.left() + 12.0, cy - 18.0),
                egui::Align2::LEFT_BOTTOM,
                strand.name,
                egui::FontId::proportional(12.0),
                if is_active { ACCENT } else { egui::Color32::from_gray(0x88) },
            );

            // nodes on the rail
            for (ni, node) in strand.nodes.iter().enumerate() {
                let x = x0 + node.t / 90.0 * dx;
                if x < area.left() - 20.0 || x > area.right() + 20.0 {
                    continue; // cheap horizontal cull
                }
                let center = egui::pos2(x, cy);
                let selected = self.selected == Some((si, ni));
                let r = if selected { 9.0 } else { 7.0 };
                painter.circle_filled(center, r, NODE);
                painter.circle_stroke(
                    center,
                    r,
                    egui::Stroke::new(if selected { 2.0 } else { 1.0 }, node.verifier.color()),
                );
                // verifier badge glyph
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    node.verifier.glyph(),
                    egui::FontId::monospace(9.0),
                    node.verifier.color(),
                );
                // label under active strand only, to avoid clutter
                if is_active {
                    painter.text(
                        center + egui::vec2(0.0, 14.0),
                        egui::Align2::CENTER_TOP,
                        node.label,
                        egui::FontId::proportional(10.0),
                        egui::Color32::from_gray(0xaa),
                    );
                }
            }
        }
        let _ = ui; // (rects would be retained here for real hit-testing)
    }

    fn draw_legend(&mut self, ui: &mut egui::Ui) {
        // Mirrors the HTML demo's input→gesture legend so the two parts read the same.
        let painter = ui.painter().clone();
        let r = ui.max_rect();
        let y = r.bottom() - 18.0;
        let mode = if self.touch_enabled {
            "touch ON · drag = scrub · flick ↑↓ = strand · pinch = zoom · fling coasts"
        } else {
            "touch OFF · baseline logscope · use the toolbar buttons to navigate"
        };
        painter.text(
            egui::pos2(r.left() + 16.0, y),
            egui::Align2::LEFT_CENTER,
            mode,
            egui::FontId::proportional(12.0),
            if self.touch_enabled { ACCENT } else { egui::Color32::from_gray(0x77) },
        );
        // verifier key
        let mut x = r.right() - 220.0;
        for (v, name) in [
            (Verifier::Pass, "pass"),
            (Verifier::Fail, "fail"),
            (Verifier::Pending, "pending"),
        ] {
            painter.circle_filled(egui::pos2(x, y), 4.0, v.color());
            painter.text(
                egui::pos2(x + 8.0, y),
                egui::Align2::LEFT_CENTER,
                name,
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(0x99),
            );
            x += 70.0;
        }
    }
}

fn apply_logscope_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = egui::Color32::from_rgb(0x10, 0x13, 0x16);
    v.selection.bg_fill = ACCENT.linear_multiply(0.4);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, RAIL);
    v.override_text_color = Some(TEXT);
    ctx.set_visuals(v);
}

// In a real example this is `fn main() -> eframe::Result<()>`:
//
//     fn main() -> eframe::Result<()> {
//         eframe::run_native(
//             "logscope + touch (egui · touchkit reverse-port)",
//             eframe::NativeOptions::default(),
//             Box::new(|_cc| Box::<LogscopeApp>::default()),
//         )
//     }
