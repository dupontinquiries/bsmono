use super::{card, row, section};
use crate::shell::ShellState;
use egui::{RichText, Ui};
use uikit_egui::{text_field, PaletteColors};

pub fn ui(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors) {
    uikit_egui::scroll::area(ui, |ui| {
        section(ui, c, "PROJECT");
        card(ui, c, |ui| {
            let r = ui.add(text_field::edit("launch-root", &mut st.root_text).hint_text("project directory").desired_width(f32::INFINITY));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                st.set_root();
            }
            ui.add_space(4.0);
            match st.plan.clone() {
                Some(plan) => {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!st.building, egui::Button::new("Build")).clicked() {
                            st.run_build();
                        }
                        if st.building {
                            ui.spinner();
                        }
                    });
                    ui.label(RichText::new(plan.command_line()).monospace().small().color(c.text_weak));
                }
                None => {
                    ui.label(RichText::new("no Cargo.toml, configured meson/cmake build dir, or Makefile here").small().color(c.text_weak));
                }
            }
            if st.build_log.len() > 1 || st.building {
                let from = st.build_log.len().saturating_sub(200);
                uikit_egui::scroll::log(ui, &st.build_log[from..], 140.0);
            }
        });

        section(ui, c, "BINARY — ranked, and every row says why");
        card(ui, c, |ui| {
            if st.candidates.is_empty() {
                ui.label(RichText::new(if st.busy > 0 { "looking…" } else { "No runnable ELF found in this directory or the usual build trees. Build first, or pick another directory." }).color(c.text_weak));
            }
            let mut pick = None;
            for (i, cand) in st.candidates.iter().enumerate().take(6) {
                let name = cand.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let reasons: Vec<String> = cand.reasons.iter().map(|r| r.describe()).collect();
                let detail = format!("{} · score {}\n{}", reasons.join(" · "), cand.score(), cand.path.display());
                if row(ui, c, st.chosen == i, if st.chosen == i { "●" } else { "○" }, &name, &detail) {
                    pick = Some(i);
                }
            }
            if let Some(i) = pick {
                st.choose(i);
            }
        });

        section(ui, c, "RUN WITH");
        card(ui, c, |ui| {
            ui.label(RichText::new("arguments").small().color(c.text_weak));
            ui.add(text_field::edit("launch-args", &mut st.args_text).desired_width(f32::INFINITY));
            ui.label(RichText::new("environment (KEY=value …)").small().color(c.text_weak));
            ui.add(text_field::edit("launch-env", &mut st.env_text).desired_width(f32::INFINITY));
            ui.label(RichText::new("frame marker — a function called once per frame").small().color(c.text_weak));
            ui.add(text_field::edit("launch-marker", &mut st.marker_text).hint_text("e.g. my_app::present").desired_width(f32::INFINITY));
        });

        section(ui, c, "SHELL HISTORY");
        card(ui, c, |ui| {
            ui.add(text_field::edit("launch-history", &mut st.history_query).hint_text("search; all words must match").desired_width(f32::INFINITY));
            let mut adopt = None;
            for (cmd, inv) in st.history_hits() {
                let detail = if inv.is_some() { "tap to use these arguments and environment" } else { "" };
                if row(ui, c, false, "", &cmd, detail) {
                    adopt = inv;
                }
            }
            if let Some(inv) = adopt {
                st.adopt(&inv);
                st.toast("Arguments taken from history", uikit_egui::toast::Severity::Success);
            }
        });

        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            let b = egui::Button::new(RichText::new("Debug").strong().size(17.0).color(c.bg_base)).fill(c.accent).min_size(egui::vec2(160.0, 44.0)).corner_radius(22.0);
            if ui.add_enabled(!st.candidates.is_empty(), b).clicked() {
                st.debug_chosen();
            }
        });
        ui.add_space(12.0);
    });
}
