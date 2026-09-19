use super::{card, section};
use crate::shell::ShellState;
use egui::{RichText, Ui};
use stoat_core::cache::human_bytes;
use uikit_egui::PaletteColors;

pub fn ui(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors) {
    uikit_egui::scroll::area(ui, |ui| {
        section(ui, c, "FETCHING");
        card(ui, c, |ui| {
            ui.checkbox(&mut st.debuginfod, "Download debug info (debuginfod)");
            ui.label(RichText::new("Off by default: one stop inside a toolkit can fetch hundreds of MB, and gdb does not ask first under MI. Applies to the next session.").small().color(c.text_weak));
        });

        section(ui, c, "ON DISK");
        card(ui, c, |ui| {
            if st.usage.is_empty() {
                ui.label(RichText::new("no debug caches found").color(c.text_weak));
            }
            for (kind, path, u) in &st.usage {
                let planned = st.plans.iter().find(|(p, _)| p == path).map(|(_, p)| p);
                let fmt = |color| egui::TextFormat { font_id: egui::FontId::proportional(14.0), color, ..Default::default() };
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = ui.available_width();
                job.append(kind.label(), 0.0, fmt(c.text));
                job.append(&human_bytes(u.bytes), 10.0, fmt(c.accent));
                if let Some(p) = planned.filter(|p| !p.remove.is_empty()) {
                    job.append(&format!("−{} ({} files)", human_bytes(p.bytes), p.remove.len()), 10.0, fmt(c.warning));
                }
                ui.label(job);
                let note = if u.unreadable { " · not readable as this user" } else { "" };
                ui.label(RichText::new(format!("{} · {} files{note}", path.display(), u.files)).small().color(c.text_weak));
                ui.add_space(6.0);
            }
        });

        section(ui, c, "PRUNE — planning only lists; nothing goes until you confirm");
        card(ui, c, |ui| {
            ui.horizontal(|ui| {
                ui.label("unused for");
                ui.add(egui::DragValue::new(&mut st.prune_days).range(1..=365).suffix(" days"));
            });
            let (files, bytes) = st.planned();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(st.busy == 0, egui::Button::new("Plan")).clicked() {
                    st.plan_prune();
                }
                let del = egui::Button::new(RichText::new(format!("Delete {files} files · {}", human_bytes(bytes))).color(c.danger));
                if ui.add_enabled(files > 0 && st.busy == 0 && !st.toasts.awaiting_answer(), del).clicked() {
                    st.confirm_prune = true;
                }
            });
        });
    });
}
