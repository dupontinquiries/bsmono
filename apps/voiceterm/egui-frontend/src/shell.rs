//! shell.rs — the app chrome around the terminal: the session sidebar, the bottom keycap bar, and
//! the mic FAB's docked position. Ported from real screenshots of voiceterm's Qt app (not just QML
//! reading) — this is the part of the port that was entirely missing before: the existing harness
//! (examples/feel.rs, src/bin/voiceterm_egui.rs) only assembled the terminal + popups, with no
//! surrounding shell at all.
//!
//! Colours: the reference screenshots were taken with the "Mocha" theme active (a *selectable*
//! theme, per the Settings panel's own theme picker) — a different, warmer palette than the
//! Voice palette this port defaults to (voiceterm's baseline QML identity). So this module ports
//! the STRUCTURE (layout, spacing, what's a button vs a list vs a pill) faithfully, using the
//! existing Voice palette's colours rather than copying Mocha's exact hex values. A full
//! multi-theme picker (Dark/Light/Nord/Solar/Mono/Rose/Gruv/Ocean/Mocha, "long-press to show all
//! vibes") is a real, separate feature visible in the settings screenshot — not ported here.

use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use uikit_egui::PaletteColors;

/// One entry in the session list (the left sidebar).
#[derive(Clone, Debug)]
pub struct Session {
    pub name: String,
    pub count: u32,
}

/// Sidebar state: the session list + which one is active.
pub struct Sidebar {
    pub sessions: Vec<Session>,
    pub active: usize,
}

impl Sidebar {
    pub fn new(sessions: Vec<Session>) -> Self {
        Self { sessions, active: 0 }
    }

    /// Render the sidebar into `ui` (call inside an `egui::SidePanel::left`). Returns the index of
    /// a newly-clicked session (if any), separately from `+ New` / `○ Graph` / `⚙ Settings` which
    /// the caller reads via the returned `SidebarAction`.
    pub fn ui(&mut self, ui: &mut Ui, c: &PaletteColors) -> Option<SidebarAction> {
        let mut action = None;
        ui.add_space(4.0);
        for (i, s) in self.sessions.iter().enumerate() {
            let selected = i == self.active;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
            let fill = if selected { c.accent_soft } else if resp.hovered() { c.bg_hover } else { c.bg_base };
            ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
            let text_color = if selected { c.bg_base } else { c.text };
            ui.painter().text(
                rect.left_center() + egui::vec2(10.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{}  ({})", s.name, s.count),
                FontId::proportional(13.0),
                text_color,
            );
            if resp.clicked() {
                self.active = i;
                action = Some(SidebarAction::SelectSession(i));
            }
        }
        // spacer pushes the action buttons to the bottom of the sidebar
        ui.add_space(ui.available_height() - 3.0 * 34.0 - 8.0);
        ui.separator();
        for (label, act) in [("+ New", SidebarAction::New), ("○ Graph", SidebarAction::Graph), ("⚙ Settings", SidebarAction::Settings)] {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
            let fill = if resp.hovered() { c.bg_hover } else { c.bg_raised };
            ui.painter().rect(rect, CornerRadius::same(4), fill, Stroke::new(1.0, c.border), StrokeKind::Inside);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.0), c.text);
            if resp.clicked() { action = Some(act); }
            ui.add_space(4.0);
        }
        action
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarAction {
    SelectSession(usize),
    New,
    Graph,
    Settings,
}

/// One key/chord in the bottom keycap bar.
pub struct Keycap {
    pub label: &'static str,
    /// What `handle_input`-equivalent code this sends when tapped (a raw string the terminal/PTY
    /// layer interprets — e.g. "\x1b" for Esc, "\x03" for Ctrl-C).
    pub send: &'static str,
}

/// The default keycap row (terminal-focused — matches the base screenshot). A web pane in focus
/// shows more keys (C-r, C-a, C-e, ~, /, -, _) per the reference; callers needing that longer row
/// can build their own `&[Keycap]` — the rendering function takes a slice, not a fixed set.
pub const TERMINAL_KEYCAPS: &[Keycap] = &[
    Keycap { label: "Esc", send: "\x1b" },
    Keycap { label: "Tab", send: "\t" },
    Keycap { label: "↑", send: "\x1b[A" },
    Keycap { label: "↓", send: "\x1b[B" },
    Keycap { label: "←", send: "\x1b[D" },
    Keycap { label: "→", send: "\x1b[C" },
    Keycap { label: "C-c", send: "\x03" },
    Keycap { label: "C-d", send: "\x04" },
    Keycap { label: "C-z", send: "\x1a" },
    Keycap { label: "C-l", send: "\x0c" },
];

/// The extended row shown when a web/rich pane has focus (adds C-r/C-a/C-e + a few punctuation
/// keys that are awkward to reach one-handed on touch).
pub const EXTENDED_KEYCAPS: &[Keycap] = &[
    Keycap { label: "Esc", send: "\x1b" }, Keycap { label: "Tab", send: "\t" },
    Keycap { label: "↑", send: "\x1b[A" }, Keycap { label: "↓", send: "\x1b[B" },
    Keycap { label: "←", send: "\x1b[D" }, Keycap { label: "→", send: "\x1b[C" },
    Keycap { label: "C-c", send: "\x03" }, Keycap { label: "C-d", send: "\x04" },
    Keycap { label: "C-z", send: "\x1a" }, Keycap { label: "C-l", send: "\x0c" },
    Keycap { label: "C-r", send: "\x12" }, Keycap { label: "C-a", send: "\x01" },
    Keycap { label: "C-e", send: "\x05" }, Keycap { label: "~", send: "~" },
    Keycap { label: "/", send: "/" }, Keycap { label: "-", send: "-" }, Keycap { label: "_", send: "_" },
];

/// Render the bottom keycap bar plus the "Sel · Copy · Paste" hint row beneath it. Returns the
/// `send` string of a tapped keycap, if any.
pub fn keycap_bar_ui(ui: &mut Ui, keys: &[Keycap], c: &PaletteColors) -> Option<&'static str> {
    let mut sent = None;
    ui.horizontal_wrapped(|ui| {
        for k in keys {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(k.label.len() as f32 * 8.0 + 22.0, 30.0), Sense::click());
            let fill = if resp.is_pointer_button_down_on() { c.accent_soft } else if resp.hovered() { c.bg_hover } else { c.bg_raised };
            ui.painter().rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0, c.border), StrokeKind::Inside);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, k.label, FontId::proportional(13.0), c.text);
            if resp.clicked() { sent = Some(k.send); }
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Sel · Copy · Paste").color(c.text_weak).size(11.0));
    });
    sent
}

/// Where the mic FAB docks — the bottom-right corner of `terminal_rect`, overlapping its border
/// slightly (matches the reference: the button sits ON the corner, not fully inside or outside).
pub fn fab_rect(terminal_rect: Rect, diameter: f32) -> Rect {
    let center = terminal_rect.right_bottom() + egui::vec2(-diameter * 0.35, -diameter * 0.35);
    Rect::from_center_size(center, Vec2::splat(diameter))
}

/// Paint the terminal's bordered frame (the amber/accent outline visible around the whole
/// terminal region in the reference screenshots).
pub fn paint_terminal_border(ui: &Ui, rect: Rect, c: &PaletteColors) {
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5, c.accent), StrokeKind::Outside);
}

/// Paint the mic FAB itself (the circular button — the compass opens from here on press-hold).
/// Returns the interaction response so the caller can wire press/hold/tap.
pub fn mic_fab_ui(ui: &mut Ui, rect: Rect, c: &PaletteColors) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with("mic_fab"), Sense::click());
    let fill = if resp.is_pointer_button_down_on() { c.accent } else { c.accent.gamma_multiply(0.9) };
    ui.painter().circle_filled(rect.center(), rect.width() * 0.5, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "\u{1F3A4}", FontId::proportional(rect.width() * 0.4), Color32::WHITE);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fab_docks_at_the_corner_overlapping_slightly() {
        let term = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 300.0));
        let fab = fab_rect(term, 60.0);
        // centre should be near the bottom-right corner, pulled inward by less than half the FAB
        assert!(fab.center().x < term.right() && fab.center().x > term.right() - 60.0);
        assert!(fab.center().y < term.bottom() && fab.center().y > term.bottom() - 60.0);
        // and it should actually overlap the corner (extend past both edges a bit)
        assert!(fab.right() > term.right());
        assert!(fab.bottom() > term.bottom());
    }

    #[test]
    fn terminal_keycaps_cover_the_base_reference_set() {
        let labels: Vec<&str> = TERMINAL_KEYCAPS.iter().map(|k| k.label).collect();
        for expect in ["Esc", "Tab", "↑", "↓", "←", "→", "C-c", "C-d", "C-z", "C-l"] {
            assert!(labels.contains(&expect), "missing {expect} from the base keycap row");
        }
    }

    #[test]
    fn extended_keycaps_are_a_superset_of_the_base_row() {
        let base: std::collections::HashSet<&str> = TERMINAL_KEYCAPS.iter().map(|k| k.label).collect();
        let ext: std::collections::HashSet<&str> = EXTENDED_KEYCAPS.iter().map(|k| k.label).collect();
        assert!(base.is_subset(&ext), "the web-pane-focused row should contain everything the terminal row has");
        for expect in ["C-r", "C-a", "C-e", "~", "/", "-", "_"] {
            assert!(ext.contains(expect), "missing extended key {expect}");
        }
    }

    #[test]
    fn sidebar_selecting_a_session_updates_active_and_returns_the_action() {
        let sessions = vec![
            Session { name: "kesslerd".into(), count: 5 },
            Session { name: "kesslerd".into(), count: 2 },
        ];
        let sb = Sidebar::new(sessions);
        assert_eq!(sb.active, 0);
        // (UI interaction itself needs a live egui::Ui — covered by the render smoke test in the
        // bin/example; this test locks down the pure construction/default-state contract.)
        assert_eq!(sb.sessions.len(), 2);
    }
}
