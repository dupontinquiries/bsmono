use super::section;
use crate::shell::ShellState;
use egui::{Color32, RichText, Sense, Ui};
use stoat_core::vitals;
use uikit_egui::PaletteColors;

pub fn ui(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors) {
    let (samples, frames, summary) = {
        let v = st.vitals.lock().unwrap();
        (v.samples(), v.frames(), vitals::summarize(&v))
    };
    uikit_egui::scroll::area(ui, |ui| {
        if summary.is_empty() {
            ui.label(RichText::new("No samples yet. Memory and CPU are sampled from /proc while the program runs; frame pacing needs a frame marker set on Launch.").color(c.text_weak));
        }
        if !summary.is_empty() {
            // In a card, at body size: two small unboxed lines above three graphs
            // read as a caption, and this is the page's actual finding.
            super::card(ui, c, |ui| {
                for line in &summary {
                    ui.add(egui::Label::new(RichText::new(line).size(uikit_egui::tokens::FONT_BASE as f32)).wrap());
                }
            });
        }
        section(ui, c, "MEMORY (resident, MiB)");
        let rss: Vec<f32> = samples.iter().map(|s| s.rss_kb as f32 / 1024.0).collect();
        plot(ui, c, &rss, "MiB", Kind::Line);
        section(ui, c, "CPU (% of one core)");
        let cpu: Vec<f32> = samples.iter().map(|s| s.cpu_pct).collect();
        plot(ui, c, &cpu, "%", Kind::Line);
        section(ui, c, "FRAME INTERVAL (ms, includes marker overhead)");
        let ms: Vec<f32> = frames.iter().map(|f| f.ms).collect();
        let hitch = vitals::frame_stats(&frames).map(|s| s.p50_ms * 1.5);
        plot(ui, c, &ms, "ms", Kind::Bars { hitch_over: hitch });
    });
}

enum Kind {
    Line,
    Bars { hitch_over: Option<f32> },
}

/// The stop is at the right edge: the rings freeze there, so what is on screen is
/// the run-up to it.
fn plot(ui: &mut Ui, c: &PaletteColors, values: &[f32], unit: &str, kind: Kind) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 110.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 8.0, uikit_egui::glass::surface_fill(c.bg_card));
    let font = egui::FontId::monospace(10.0);
    if values.len() < 2 {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "no data", font, c.text_weak);
        return;
    }
    let peak = values.iter().copied().fold(f32::MIN, f32::max).max(1e-6);
    let top = peak * 1.1;
    let inner = rect.shrink2(egui::vec2(8.0, 8.0)).with_min_y(rect.min.y + 20.0);
    let x = |i: usize| inner.min.x + inner.width() * i as f32 / (values.len() - 1) as f32;
    let y = |v: f32| inner.max.y - inner.height() * (v / top).clamp(0.0, 1.0);
    match kind {
        Kind::Line => {
            let soft = Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), 40);
            let step = (inner.width() / values.len() as f32).max(1.0);
            for (i, v) in values.iter().enumerate() {
                p.rect_filled(egui::Rect::from_min_max(egui::pos2(x(i) - step * 0.5, y(*v)), egui::pos2(x(i) + step * 0.5, inner.max.y)), 0.0, soft);
            }
            let pts: Vec<egui::Pos2> = values.iter().enumerate().map(|(i, v)| egui::pos2(x(i), y(*v))).collect();
            p.add(egui::Shape::line(pts, egui::Stroke::new(1.5_f32, c.accent)));
        }
        Kind::Bars { hitch_over } => {
            let bw = (inner.width() / values.len() as f32).max(1.0);
            for (i, v) in values.iter().enumerate() {
                let hitch = hitch_over.is_some_and(|t| *v > t);
                let x0 = inner.min.x + bw * i as f32;
                let col = if hitch { c.danger } else { c.text_weak };
                p.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, y(*v)), egui::pos2(x0 + (bw - 1.0).max(1.0), inner.max.y)), 0.0, col);
            }
        }
    }
    let last = values[values.len() - 1];
    p.text(rect.min + egui::vec2(8.0, 6.0), egui::Align2::LEFT_TOP, format!("peak {peak:.1} {unit} · last {last:.1} {unit}"), font, c.text_weak);
}
