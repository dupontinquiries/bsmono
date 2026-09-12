//! card_diagnoser — hold a card/ring/tag near a PC/SC reader, see what
//! contactless protocol it speaks, copy the summary to clipboard.
//!
//! Built for walking a store's shelf and checking what protocols are
//! actually buyable locally, not just documented online — a MIFARE
//! Classic keyfob and a DESFire access card look identical on the shelf.
//!
//! Run: cargo run --release (from apps/card-diagnoser). Needs a PC/SC
//! service (`pscsd` on Linux) and a contactless reader; without either,
//! the window explains which is missing rather than sitting blank.

use card_diagnoser::{pulse_svg, reader};
use eframe::egui;
use nfc_kit::Confidence;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct App {
    status: Arc<Mutex<reader::ReaderStatus>>,
    palette: uikit_egui::PaletteColors,
    pulse_started: Instant,
    /// The rendered pulse texture is rebuilt only when its content would
    /// actually differ — resvg rasterization every single frame for an
    /// animation most frames barely change is wasted work; egui's own
    /// texture upload has real cost too.
    pulse_texture: Option<egui::TextureHandle>,
    last_pulse_bucket: u32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let wake: reader::Waker = Arc::new(move || ctx.request_repaint());
        App {
            status: reader::spawn(wake),
            palette: uikit_egui::default_palette(),
            pulse_started: Instant::now(),
            pulse_texture: None,
            last_pulse_bucket: u32::MAX,
        }
    }

    /// Rasterizes the current pulse frame if enough time passed to look
    /// different from the last upload — bucketed to ~30fps worth of
    /// distinct frames rather than re-rasterizing at whatever rate egui
    /// happens to redraw.
    fn pulse_texture(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        let elapsed_ms = self.pulse_started.elapsed().as_millis() as u32;
        let bucket = elapsed_ms / 33;
        if self.pulse_texture.is_none() || bucket != self.last_pulse_bucket {
            let accent_hex = format!(
                "#{:02X}{:02X}{:02X}",
                (self.palette.accent.r()),
                (self.palette.accent.g()),
                (self.palette.accent.b())
            );
            let svg = pulse_svg::pulse_svg(elapsed_ms, 1400, 3, 256.0, &accent_hex);
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
                .expect("generated SVG is always well-formed — see pulse_svg's own tests");
            let size = tree.size();
            let (w, h) = (size.width().ceil() as u32, size.height().ceil() as u32);
            let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1)).unwrap();
            resvg::render(&tree, resvg::usvg::Transform::default(), &mut pixmap.as_mut());
            let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], pixmap.data());
            self.pulse_texture = Some(ctx.load_texture("pulse", image, egui::TextureOptions::LINEAR));
            self.last_pulse_bucket = bucket;
        }
        self.pulse_texture.clone().unwrap()
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe 0.34.3 changed its primary trait method from
        // `update(&mut self, ctx: &egui::Context, ...)` to this simpler
        // `ui` form (eframe now owns the CentralPanel wrapping itself) —
        // hatch-egui's Cargo.lock pins an earlier 0.34.x patch that
        // predates this change, so its own bin still implements `update`.
        // Real mid-range API churn in this ecosystem, same character as
        // gtk-rs/libadwaita churn — worth naming plainly rather than
        // silently working around, since the NEXT app built against a
        // freshly-resolved "0.34" could hit this same surprise.
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        let status = self.status.lock().map(|s| s.clone()).unwrap_or(reader::ReaderStatus::NoService("lock poisoned".into()));

        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            match &status {
                reader::ReaderStatus::NoService(detail) => {
                    ui.heading("No PC/SC service found");
                    ui.label("Is pcscd installed and running?");
                    ui.weak(detail);
                }
                reader::ReaderStatus::ServiceRunningNoReader => {
                    ui.heading("No reader attached");
                    ui.label("Plug in a contactless reader (e.g. an ACR122U).");
                    let tex = self.pulse_texture(&ctx);
                    ui.add_space(16.0);
                    ui.add(egui::Image::new(&tex).max_size(egui::vec2(180.0, 180.0)).tint(egui::Color32::from_gray(120)));
                }
                reader::ReaderStatus::WaitingForCard { reader_name } => {
                    ui.heading("Hold a card near the reader");
                    ui.weak(reader_name);
                    ui.add_space(16.0);
                    let tex = self.pulse_texture(&ctx);
                    ui.add(egui::Image::new(&tex).max_size(egui::vec2(220.0, 220.0)));
                }
                reader::ReaderStatus::CardError { reader_name, message } => {
                    ui.heading("Couldn't read that card");
                    ui.weak(reader_name);
                    ui.colored_label(egui::Color32::from_rgb(220, 90, 90), message);
                }
                reader::ReaderStatus::CardPresent { reader_name, profile } => {
                    ui.heading(&profile.family);
                    let (dot, label) = match profile.confidence {
                        Confidence::High => (egui::Color32::from_rgb(90, 200, 120), "high confidence"),
                        Confidence::Medium => (egui::Color32::from_rgb(230, 180, 60), "medium confidence"),
                        Confidence::Low => (egui::Color32::from_rgb(200, 90, 90), "low confidence"),
                    };
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 5.0, dot);
                        ui.label(label);
                    });
                    ui.add_space(12.0);
                    if !profile.protocols.is_empty() {
                        ui.label(profile.protocols.join(" · "));
                    }
                    if let Some(uid) = &profile.uid_hex {
                        ui.monospace(format!("UID  {uid}"));
                    }
                    ui.monospace(format!("ATR  {}", profile.atr_hex));
                    for note in &profile.notes {
                        ui.add_space(6.0);
                        ui.weak(note);
                    }
                    ui.add_space(16.0);
                    if ui.button("Copy summary").clicked() {
                        let summary = nfc_kit::copy_summary(profile);
                        ctx.copy_text(summary);
                    }
                    ui.add_space(8.0);
                    ui.weak(reader_name);
                }
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("card-diagnoser", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
