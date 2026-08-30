//! Peek tab — monospace view of the live Doc with a toolbar. Ports `ui/peek_view.rs`.
//! The "collapse repeats" action is logscope's signature move: consecutive identical
//! lines fold into a single `N× <line>` row, which is what makes a noisy log readable.
use crate::analysis::Doc;
use crate::theme;
use egui::{Color32, RichText};

pub struct Peek {
    syntax: bool,
    collapse: bool,
    goto_top: bool,
    /// Touch gesture recogniser. Held on the tab rather than built per
    /// frame because it accumulates state across frames — a tap is a
    /// press AND a release, and a per-frame recogniser can never see the
    /// second half of one.
    touch: touchkit::Recognizer,
    /// (document line index, screen rect) for each row drawn last frame.
    /// Feeds gesture hit-testing; see the draw loop for why it is not
    /// cached.
    row_rects: Vec<(usize, egui::Rect)>,
}
impl Default for Peek {
    fn default() -> Self {
        Self {
            syntax: true,
            collapse: false,
            goto_top: false,
            touch: touchkit::Recognizer::new(touchkit::Config::default()),
            row_rects: Vec::new(),
        }
    }
}

/// Where a gesture happened, for hit-testing against row rects.
///
/// Only the POSITIONAL gestures have one. A swipe carries a velocity and
/// no point — it is a whole-view gesture, so resolving it against a row
/// would attach a global action to whichever line happened to be under
/// the finger.
fn gesture_pos(g: &touchkit::Gesture) -> Option<touchkit::Vec2> {
    use touchkit::Gesture as G;
    match g {
        G::Tap(p) | G::DoubleTap(p) | G::LongPress(p) | G::PanStart(p) | G::PanEnd(p) => Some(*p),
        G::Pan { pos, .. } => Some(*pos),
        _ => None,
    }
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

        // TOUCH GESTURES. `log_gestures` has resolved (mode, gesture,
        // hit) into a `LogAction` since it was written and nothing fed
        // it — the Peek surface it was designed for had NO gesture
        // handling at all, so tap-to-fold and section select existed as
        // a tested decision table with no input.
        //
        // Only the actions this tab can honour are handled. Selection
        // needs a selection model Peek does not have, and mapping those
        // to nothing would make the table look wired while half of it
        // silently did nothing — the failure this session keeps finding.
        {
            use crate::log_gestures::{map_gesture, Hit, LogAction, Mode};
            let now = ui.input(|i| i.time) * 1000.0;
            let touches: Vec<touchkit::Touch> = ui.input(|i| {
                i.pointer.interact_pos()
                    .filter(|_| i.pointer.any_down())
                    .map(|p| touchkit::Touch { id: 0, pos: touchkit::Vec2 { x: p.x, y: p.y } })
                    .into_iter()
                    .collect()
            });
            for g in self.touch.update(&touches, now) {
                // A REAL HIT, from the rects recorded last frame.
                //
                // Last frame's, necessarily: the gesture is resolved
                // before the rows are drawn, so the only rects available
                // are the previous frame's. That is correct for a tap —
                // the row was under the finger when it was pressed — and
                // it is why the rects are rebuilt rather than cached: a
                // fold or a resize moves them, and one frame of staleness
                // is the most this can carry.
                let hit = gesture_pos(&g)
                    .and_then(|p| {
                        self.row_rects.iter()
                            .find(|(_, r)| r.contains(egui::pos2(p.x, p.y)))
                            .map(|(line, _)| Hit::Line(*line))
                    })
                    .unwrap_or(Hit::Empty);
                match map_gesture(Mode::Read, &g, hit) {
                    LogAction::FoldAll => self.collapse = true,
                    LogAction::UnfoldAll => self.collapse = false,
                    // Scroll is egui's own job inside a ScrollArea;
                    // applying it here would double every drag.
                    LogAction::Scroll { .. } | LogAction::None => {}
                    // Tap a folded run to expand everything. Peek's
                    // fold is all-or-nothing (`collapse` is one flag),
                    // so a per-section toggle would need per-run state
                    // this tab does not have — expanding is the closest
                    // honest behaviour, and it is what a person taps a
                    // "12×" row expecting.
                    LogAction::ToggleFold(_) => self.collapse = false,
                    other => {
                        // Selection still has no model here. Named
                        // rather than ignored so a reader can see which
                        // part of the table remains unreachable.
                        let _ = other;
                    }
                }
            }
        }

        let mut sa = egui::ScrollArea::vertical().id_salt("peek").auto_shrink([false, false]);
        if self.goto_top { sa = sa.vertical_scroll_offset(0.0); self.goto_top = false; }
        sa.show(ui, |ui| {
            // ROW RECTS, recorded as they are drawn.
            //
            // Rebuilt every frame rather than cached: rows move when the
            // fold state changes or the window resizes, and a stale rect
            // maps a tap to the wrong line — which for `ToggleFold` folds
            // a section the person did not touch. Cheap enough (one rect
            // per visible row, and the ScrollArea only draws visible
            // ones) that caching would trade a real bug for nothing.
            //
            // The index stored is the DOCUMENT line, not the row
            // position, because a folded view has fewer rows than lines
            // and `LogAction::ToggleFold(usize)` means a line.
            self.row_rects.clear();
            match &runs {
                Some(r) => for &(start, line, count) in r {
                    let resp = ui.horizontal(|ui| {
                        if count > 1 {
                            ui.label(RichText::new(format!("{count:>4}×")).color(theme::AMBER()).monospace().strong());
                        } else {
                            ui.label(RichText::new(format!("{start:>4} ")).color(theme::FAINT()).monospace());
                        }
                        ui.label(RichText::new(line).color(self.line_color(line)).monospace());
                    });
                    self.row_rects.push((start.saturating_sub(1), resp.response.rect));
                },
                None => for (n, line) in doc.lines.iter().enumerate() {
                    let resp = ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{:>4}", n + 1)).color(theme::FAINT()).monospace());
                        ui.label(RichText::new(line).color(self.line_color(line)).monospace());
                    });
                    self.row_rects.push((n, resp.response.rect));
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
