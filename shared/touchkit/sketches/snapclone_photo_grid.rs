//! ════════════════════════════════════════════════════════════════════════════
//!  ILLUSTRATIVE SKETCH — NOT COMPILED here (needs egui + eframe + egui_input.rs).
//! ════════════════════════════════════════════════════════════════════════════
//!
//! Part 1 of the side-by-side: snapclone's photo UI, faithful in feel, rebuilt on
//! egui and driven entirely by `touchkit`. The interactive HTML demo
//! (`snapclone-egui-demo.html`) is the visual reference; this shows how the same
//! behavior is wired in real egui. It is a sketch: the layout math is real but
//! abbreviated, and it has not been built in this environment.
//!
//! What touchkit gives this screen, that egui's raw input does not:
//!   • grid          flick-to-scroll with **momentum** (Fling -> MomentumTick)
//!   • lightbox      swipe ◀ ▶ to flip, swipe ▼ to dismiss, double-tap to zoom
//!   • lightbox      pinch / ctrl-scroll to zoom, drag to pan while zoomed
//! …all from one `GestureRecognizer`, with no rendering dependency.
//!
//! snapclone palette (matches the HTML demo / the original Qt look):
//!   panel #1b1b1b · card #232323 · stroke #333 · teal select #005c80
//!   accent #5aaaff · text #e6e6e6 · sync-ok #57b894 · syncing #d9a13b

mod egui_input; // the ~50-line bridge in this folder
use egui_input::TouchSource;
use touchkit::{Gesture, GestureRecognizer};

const PANEL: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x1b, 0x1b);
const CARD: egui::Color32 = egui::Color32::from_rgb(0x23, 0x23, 0x23);
const STROKE: egui::Color32 = egui::Color32::from_rgb(0x33, 0x33, 0x33);
const TEAL: egui::Color32 = egui::Color32::from_rgb(0x00, 0x5c, 0x80);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x5a, 0xaa, 0xff);
const SYNC_OK: egui::Color32 = egui::Color32::from_rgb(0x57, 0xb8, 0x94);
const SYNCING: egui::Color32 = egui::Color32::from_rgb(0xd9, 0xa1, 0x3b);

#[derive(Clone, Copy, PartialEq)]
enum Sync {
    Local,
    Syncing,
    Synced,
}

struct Photo {
    hue: f32,
    sync: Sync,
}

pub struct SnapcloneApp {
    rec: GestureRecognizer,
    input: TouchSource,
    photos: Vec<Photo>,

    // grid
    grid_offset: f32, // px scrolled
    cols: usize,

    // lightbox
    open: Option<usize>,
    zoom: f32,
    pan: egui::Vec2,
    // a swipe ◀▶ animates the film strip; this is the in-flight horizontal offset
    flip_dx: f32,

    // simulated snapcore backend
    peers_online: u32,
}

impl Default for SnapcloneApp {
    fn default() -> Self {
        // 30 deterministic "photos" as hue-shifted gradient cards, like the demo.
        let photos = (0..30)
            .map(|i| Photo {
                hue: (i as f32 * 37.0) % 360.0,
                sync: match i % 4 {
                    0 => Sync::Local,
                    1 => Sync::Syncing,
                    _ => Sync::Synced,
                },
            })
            .collect();
        Self {
            rec: GestureRecognizer::default(),
            input: TouchSource::default(),
            photos,
            grid_offset: 0.0,
            cols: 4,
            open: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            flip_dx: 0.0,
            peers_online: 2,
        }
    }
}

impl eframe::App for SnapcloneApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        apply_snapclone_theme(ctx);

        // ── one gesture poll for the whole screen ────────────────────────────
        let (gestures, gliding) = self.input.poll(ctx, &mut self.rec);
        if self.open.is_some() {
            self.handle_lightbox_gestures(&gestures);
        } else {
            self.handle_grid_gestures(&gestures);
        }
        if gliding {
            ctx.request_repaint(); // keep the fling coasting
        }

        // ── toolbar ──────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::none().fill(PANEL).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("snapclone").color(ACCENT).strong());
                    ui.separator();
                    ui.label("Album: ");
                    egui::ComboBox::from_id_source("album")
                        .selected_text("All Photos")
                        .show_ui(ui, |ui| {
                            ui.selectable_label(true, "All Photos");
                            ui.selectable_label(false, "Camera Roll");
                            ui.selectable_label(false, "Shared");
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // DHT peer-status pill (the snapcore daemon connection).
                        let (dot, txt) = if self.peers_online > 0 {
                            (SYNC_OK, format!("{} peers", self.peers_online))
                        } else {
                            (SYNCING, "offline".to_owned())
                        };
                        peer_pill(ui, dot, &txt);
                    });
                });
            });

        // ── content ──────────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(PANEL))
            .show(ctx, |ui| {
                self.draw_grid(ui);
            });

        if self.open.is_some() {
            self.draw_lightbox(ctx);
        }
    }
}

impl SnapcloneApp {
    // ---- gesture dispatch (the interesting part) ----

    fn handle_grid_gestures(&mut self, gestures: &[Gesture]) {
        for g in gestures {
            match g {
                // Pan during drag and MomentumTick after a fling both scroll Y.
                Gesture::Pan { delta, .. } => self.grid_offset += -delta.y,
                Gesture::MomentumTick { delta } => self.grid_offset += -delta.y,
                // A tap that wasn't a drag opens the photo under the pointer.
                Gesture::Tap(p) if !self.rec.is_dragging() => {
                    if let Some(i) = self.photo_at(*p) {
                        self.open = Some(i);
                        self.zoom = 1.0;
                        self.pan = egui::Vec2::ZERO;
                    }
                }
                _ => {}
            }
        }
        self.clamp_grid();
    }

    fn handle_lightbox_gestures(&mut self, gestures: &[Gesture]) {
        for g in gestures {
            match g {
                Gesture::SwipeLeft { .. } if self.zoom <= 1.02 => self.advance(1),
                Gesture::SwipeRight { .. } if self.zoom <= 1.02 => self.advance(-1),
                Gesture::SwipeDown { .. } if self.zoom <= 1.02 => self.open = None,
                Gesture::DoubleTap(_) => {
                    self.zoom = if self.zoom > 1.02 { 1.0 } else { 2.2 };
                    self.pan = egui::Vec2::ZERO;
                }
                Gesture::Pinch { rel, .. } => {
                    self.zoom = (self.zoom * rel).clamp(1.0, 5.0);
                    if self.zoom <= 1.02 {
                        self.pan = egui::Vec2::ZERO;
                    }
                }
                // While zoomed, a drag pans the image; while not, it leans the flip.
                Gesture::Pan { delta, .. } => {
                    if self.zoom > 1.02 {
                        self.pan += egui::vec2(delta.x, delta.y);
                    } else {
                        self.flip_dx += delta.x;
                    }
                }
                Gesture::PanEnd(_) => self.flip_dx = 0.0,
                _ => {}
            }
        }
    }

    fn advance(&mut self, dir: i32) {
        if let Some(i) = self.open {
            let n = self.photos.len() as i32;
            let next = (i as i32 + dir).rem_euclid(n) as usize;
            self.open = Some(next);
            self.zoom = 1.0;
            self.pan = egui::Vec2::ZERO;
            self.flip_dx = 0.0;
        }
    }

    // ---- layout helpers ----

    fn cell(&self, ui: &egui::Ui) -> (f32, f32) {
        let gap = 6.0;
        let avail = ui.available_width();
        let size = (avail - gap * (self.cols as f32 + 1.0)) / self.cols as f32;
        (size, gap)
    }

    fn photo_at(&self, _p: touchkit::Vec2) -> Option<usize> {
        // In a real build, hit-test `_p` against the laid-out cell rects (store the
        // rects during draw_grid). Abbreviated here.
        self.open.or(Some(0))
    }

    fn clamp_grid(&mut self) {
        let rows = (self.photos.len() + self.cols - 1) / self.cols;
        let max = (rows as f32 * 150.0 - 400.0).max(0.0); // approx content height
        self.grid_offset = self.grid_offset.clamp(0.0, max);
    }

    // ---- rendering (abbreviated; HTML demo is the visual source of truth) ----

    fn draw_grid(&mut self, ui: &mut egui::Ui) {
        let (size, gap) = self.cell(ui);
        let origin = ui.min_rect().min + egui::vec2(gap, gap - self.grid_offset);
        let painter = ui.painter().clone();
        for (i, photo) in self.photos.iter().enumerate() {
            let col = (i % self.cols) as f32;
            let row = (i / self.cols) as f32;
            let pos = origin + egui::vec2(col * (size + gap), row * (size + gap));
            let rect = egui::Rect::from_min_size(pos, egui::vec2(size, size));
            if !ui.clip_rect().intersects(rect) {
                continue;
            }
            // gradient-ish fill via hue; real build would draw the decoded thumbnail
            let fill = egui::ecolor::Hsva::new(photo.hue / 360.0, 0.45, 0.30, 1.0).into();
            painter.rect_filled(rect, 6.0, fill);
            painter.rect_stroke(rect, 6.0, egui::Stroke::new(1.0, STROKE));
            // sync badge
            let (c, _) = match photo.sync {
                Sync::Synced => (SYNC_OK, "✓"),
                Sync::Syncing => (SYNCING, "⟳"),
                Sync::Local => (egui::Color32::GRAY, "·"),
            };
            painter.circle_filled(rect.right_top() + egui::vec2(-10.0, 10.0), 4.0, c);
        }
    }

    fn draw_lightbox(&mut self, ctx: &egui::Context) {
        let i = self.open.unwrap();
        egui::Area::new("lightbox".into())
            .fixed_pos(egui::Pos2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                let p = ui.painter();
                p.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(235));
                // current photo, scaled by zoom, offset by pan + in-flight flip lean
                let base = screen.shrink(40.0);
                let center = base.center() + self.pan + egui::vec2(self.flip_dx * 0.4, 0.0);
                let half = base.size() * 0.5 * self.zoom;
                let img = egui::Rect::from_center_size(center, half * 2.0);
                let hue = self.photos[i].hue;
                let fill = egui::ecolor::Hsva::new(hue / 360.0, 0.5, 0.32, 1.0).into();
                p.rect_filled(img, 4.0, fill);
                // hint line
                let hint = if self.zoom > 1.02 {
                    format!("zoom {:.1}× · drag to pan · pinch in to reset", self.zoom)
                } else {
                    "swipe ◀ ▶ to flip · swipe ▼ to close · pinch / scroll to zoom".to_owned()
                };
                p.text(
                    egui::pos2(base.center().x, base.bottom() - 8.0),
                    egui::Align2::CENTER_BOTTOM,
                    hint,
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_gray(0xcc),
                );
            });
    }
}

// ---- small UI bits ----

fn peer_pill(ui: &mut egui::Ui, dot: egui::Color32, txt: &str) {
    egui::Frame::none()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, STROKE))
        .rounding(10.0)
        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, dot);
                ui.label(egui::RichText::new(txt).size(12.0));
            });
        });
}

fn apply_snapclone_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = egui::Color32::from_rgb(0x14, 0x14, 0x14);
    v.selection.bg_fill = TEAL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, STROKE);
    v.override_text_color = Some(egui::Color32::from_rgb(0xe6, 0xe6, 0xe6));
    ctx.set_visuals(v);
}

// In a real example this is `fn main() -> eframe::Result<()>`:
//
//     fn main() -> eframe::Result<()> {
//         eframe::run_native(
//             "snapclone (egui · touchkit)",
//             eframe::NativeOptions::default(),
//             Box::new(|_cc| Box::<SnapcloneApp>::default()),
//         )
//     }
