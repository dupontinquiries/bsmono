//! Tokens + Blocks tabs — native egui bar charts in analytics-style cards,
//! with a stat-tile row, computed from the live Doc.
use crate::analysis::{self, Doc};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, pos2, vec2};

fn bar_chart(ui: &mut egui::Ui, bars: &[(String, f64, bool)]) {
    if bars.is_empty() {
        ui.label(RichText::new("no data — load a log in the Sources tab").color(theme::FAINT()));
        return;
    }
    let w = ui.available_width().min(720.0);
    let (resp, p) = ui.allocate_painter(vec2(w, 220.0), Sense::hover());
    let rect = resp.rect;
    let n = bars.len().max(1);
    let maxv = bars.iter().map(|b| b.1).fold(0.0_f64, f64::max).max(1.0);
    let gap = 5.0;
    let bw = ((rect.width() - gap * (n as f32 + 1.0)) / n as f32).max(2.0);
    let base = rect.bottom() - 16.0;
    for (i, (label, v, hl)) in bars.iter().enumerate() {
        let x = rect.left() + gap + i as f32 * (bw + gap);
        let bh = (rect.height() - 22.0) * (*v / maxv) as f32;
        let r = Rect::from_min_size(pos2(x, base - bh), vec2(bw, bh));
        let c: Color32 = if *hl { theme::AMBER() } else { theme::CYAN() };
        p.rect_filled(r, CornerRadius::same(2), c);
        p.text(pos2(x + bw / 2.0, base - bh - 2.0), Align2::CENTER_BOTTOM,
            format!("{}", *v as u64), FontId::monospace(8.0), theme::DIM());
        if bw >= 13.0 {
            p.text(pos2(x + bw / 2.0, base + 2.0), Align2::CENTER_TOP, label, FontId::monospace(8.0), theme::DIM());
        }
    }
}

fn errorish(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    ["error", "warn", "fail", "timed", "timeout", "500", "502", "503", "504"].iter().any(|k| l.contains(k))
}

fn stat_row(ui: &mut egui::Ui, doc: &Doc) {
    let lines = doc.lines.len();
    let errs = doc.lines.iter().filter(|l| l.contains("ERROR")).count();
    let warns = doc.lines.iter().filter(|l| l.contains("WARN")).count();
    let templates = analysis::repeated_templates(&doc.lines, usize::MAX).len();
    theme::section(ui, "corpus");
    let tiles = [
        ("lines", lines, theme::CYAN()),
        ("patterns", templates, theme::VIOLET()),
        ("errors", errs, theme::RED()),
        ("warnings", warns, theme::YELLOW()),
    ];
    theme::grid_fit(ui, 132.0, 8.0, tiles.len(), |ui, i| {
        let (label, value, accent) = tiles[i];
        theme::stat_tile(ui, label, &value.to_string(), accent);
    });
    ui.add_space(8.0);
}

pub fn tokens_ui(ui: &mut egui::Ui, doc: &Doc) {
    stat_row(ui, doc);
    let freqs = analysis::token_frequencies(&doc.lines, 12);
    let bars: Vec<(String, f64, bool)> = freqs.iter().map(|(t, c)| (t.clone(), *c as f64, errorish(t))).collect();
    let title = format!("Token frequency — top {} · {} · highlighted: error-class", bars.len(), doc.name);
    theme::card(ui, theme::AMBER(), Some(&title), |ui| bar_chart(ui, &bars));
}

pub fn blocks_ui(ui: &mut egui::Ui, doc: &Doc) {
    stat_row(ui, doc);
    let temps = analysis::repeated_templates(&doc.lines, 24);
    let bars: Vec<(String, f64, bool)> =
        temps.iter().enumerate().map(|(i, (_, c))| (format!("{i}"), *c as f64, *c > 1)).collect();
    theme::card(ui, theme::AMBER(), Some("Repeated-pattern histogram (XxHash64 over digit-normalized templates)"),
        |ui| bar_chart(ui, &bars));
    theme::card(ui, theme::VIOLET(), Some("Top patterns"), |ui| {
        for (t, c) in temps.iter().take(6) {
            let col = if *c > 1 { theme::AMBER_SOFT() } else { theme::DIM() };
            ui.label(RichText::new(format!("{c:>3}×  {t}")).monospace().color(col).small());
        }
    });
}
