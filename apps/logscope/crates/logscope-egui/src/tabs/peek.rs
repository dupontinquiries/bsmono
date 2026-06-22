//! Peek tab — monospace view of the live Doc with a toolbar. Ports `ui/peek_view.rs`.
//! The "collapse repeats" action is logscope's signature move: consecutive identical
//! lines fold into a single `N× <line>` row, which is what makes a noisy log readable.
use crate::analysis::Doc;
use crate::theme;
use egui::{Color32, RichText};

pub struct Peek { syntax: bool, collapse: bool, goto_top: bool }
impl Default for Peek {
    fn default() -> Self { Self { syntax: true, collapse: false, goto_top: false } }
}

/// A folded run of identical consecutive lines: (1-based start line, text, count).
fn fold_runs(lines: &[String]) -> Vec<(usize, &str, usize)> {
    let mut out: Vec<(usize, &str, usize)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        match out.last_mut() {
            Some((_, prev, count)) if *prev == line.as_str() => *count += 1,
            _ => out.push((i + 1, line.as_str(), 1)),
        }
    }
    out
}

impl Peek {
    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &Doc) {
        ui.horizontal(|ui| {
            theme::switch_row(ui, "Syntax", &mut self.syntax, theme::CYAN());
            ui.add_space(theme::PAD_SM as f32);
            theme::switch_row(ui, "Collapse repeats", &mut self.collapse, theme::AMBER());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::action_button(ui, "⤒ Top", theme::CYAN(), true).clicked() { self.goto_top = true; }
            });
        });
        ui.add_space(theme::PAD_XS as f32);

        if doc.lines.is_empty() {
            ui.label(RichText::new("no log loaded — use the Sources tab").color(theme::FAINT()));
            return;
        }

        // Status line: line count, and how much the collapse bought when active.
        let runs = if self.collapse { Some(fold_runs(&doc.lines)) } else { None };
        let status = match &runs {
            Some(r) => format!("{} · {} lines → {} rows  ({} folded)", doc.name, doc.lines.len(), r.len(), doc.lines.len() - r.len()),
            None => format!("{} · {} lines", doc.name, doc.lines.len()),
        };
        ui.label(RichText::new(status).color(theme::DIM()).small());
        ui.add_space(theme::PAD_XS as f32);

        let mut sa = egui::ScrollArea::vertical().id_salt("peek").auto_shrink([false, false]);
        if self.goto_top { sa = sa.vertical_scroll_offset(0.0); self.goto_top = false; }
        sa.show(ui, |ui| {
            match &runs {
                Some(r) => for &(start, line, count) in r {
                    ui.horizontal(|ui| {
                        if count > 1 {
                            ui.label(RichText::new(format!("{count:>4}×")).color(theme::AMBER()).monospace().strong());
                        } else {
                            ui.label(RichText::new(format!("{start:>4} ")).color(theme::FAINT()).monospace());
                        }
                        ui.label(RichText::new(line).color(self.line_color(line)).monospace());
                    });
                },
                None => for (n, line) in doc.lines.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{:>4}", n + 1)).color(theme::FAINT()).monospace());
                        ui.label(RichText::new(line).color(self.line_color(line)).monospace());
                    });
                },
            }
        });
    }
    fn line_color(&self, line: &str) -> Color32 {
        if !self.syntax { return theme::INK(); }
        if line.contains("ERROR") { theme::RED() }
        else if line.contains("WARN") { theme::YELLOW() }
        else if line.contains("->") || line.contains("GET") { theme::AMBER_SOFT() }
        else { theme::INK() }
    }
}

#[cfg(test)]
mod tests {
    use super::fold_runs;
    #[test]
    fn collapses_consecutive_duplicates() {
        let lines: Vec<String> = ["a", "a", "a", "b", "a", "a"].iter().map(|s| s.to_string()).collect();
        let r = fold_runs(&lines);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], (1, "a", 3));   // first run of 3 starts at line 1
        assert_eq!(r[1], (4, "b", 1));   // single b at line 4
        assert_eq!(r[2], (5, "a", 2));   // non-adjacent a's do NOT merge with the first run
    }
    #[test]
    fn empty_and_single() {
        assert!(fold_runs(&[]).is_empty());
        let one = vec!["x".to_string()];
        assert_eq!(fold_runs(&one), vec![(1, "x", 1)]);
    }
}
