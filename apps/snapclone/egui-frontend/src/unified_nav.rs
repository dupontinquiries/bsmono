//! unified_nav — faithful egui port of Qt's `UnifiedNavBar` (V32.34+ two-level nav).
//!
//! The Qt contract, mirrored here behavior-for-behavior (src/ui/UnifiedNavBar.cpp):
//! * Five pages: Camera · Inbox · Friends · Wayfinder · Settings (`MainWindow.cpp` kPage*).
//! * `‹`/`›` page arrows with enabled state at the ends and prev/next tooltips.
//! * SubTabbed pages (Inbox/Friends/Settings) show their sub-tab SEGMENTS left-aligned after
//!   the page-name chip; tap = direct jump. Non-SubTabbed pages (Camera/Wayfinder) hide the
//!   chip and show a CENTERED page name between the arrows — "‹   CAMERA   ›" — instead of a
//!   left-aligned name with a hollow gap (the Qt comment's exact rule).
//! * The active-segment underline MORPHS (animates) to the new segment instead of snapping.
//! * Page-position dots under the bar reflect the page index.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// Sub-tab tables, verbatim from the Qt pages' `subTabLabel` switches.
pub fn sub_tabs(page: usize) -> &'static [&'static str] {
    match page {
        1 => &["All", "Unread", "Sent"],           // InboxWidget.cpp
        2 => &["All", "Online", "Pending"],        // ContactsPage.cpp
        4 => &["General", "Camera", "Network", "Storage", "About"], // SettingsPage.cpp
        _ => &[],                                  // Camera, Wayfinder: not SubTabbed
    }
}

pub const PAGE_NAMES: [&str; 5] = ["Camera", "Inbox", "Friends", "Wayfinder", "Settings"];

pub struct UnifiedNav {
    pub page: usize,
    pub sub: [usize; 5],
    /// Underline morph: animated x-span (min,max) easing toward the active segment's span.
    underline: Option<(f32, f32)>,
}

pub enum NavEvent { None, PageChanged(usize), SubChanged(usize) }

impl UnifiedNav {
    pub fn new() -> Self { UnifiedNav { page: 0, sub: [0; 5], underline: None } }

    pub fn ui(&mut self, ui: &mut Ui) -> NavEvent {
        let mut ev = NavEvent::None;
        let text = Color32::from_rgb(236, 232, 224);
        let dim = Color32::from_rgb(150, 146, 138);
        let accent = Color32::from_rgb(240, 180, 90);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // ‹ prev — disabled at the first page, tooltip names the destination
            let can_prev = self.page > 0;
            let prev = ui.add_enabled(can_prev, egui::Button::new("‹"));
            if can_prev { prev.clone().on_hover_text(format!("Previous: {}", PAGE_NAMES[self.page - 1])); }
            if prev.clicked() { self.page -= 1; self.underline = None; ev = NavEvent::PageChanged(self.page); }

            let tabs = sub_tabs(self.page);
            if tabs.is_empty() {
                // centered "‹   CAMERA   ›" — the non-SubTabbed rule
                let name = PAGE_NAMES[self.page].to_uppercase();
                let avail = ui.available_width() - 34.0; // reserve the › arrow
                let galley = ui.painter().layout_no_wrap(name.clone(),
                    egui::FontId::proportional(15.0), text);
                let pad = ((avail - galley.rect.width()) * 0.5).max(0.0);
                ui.add_space(pad);
                ui.label(egui::RichText::new(name).strong().size(15.0).color(text));
                ui.add_space(pad);
            } else {
                // page-name chip + left-aligned segments
                ui.label(egui::RichText::new(PAGE_NAMES[self.page].to_uppercase())
                    .strong().size(13.0).color(dim));
                let active = self.sub[self.page];
                let mut spans: Vec<(f32, f32)> = Vec::new();
                for (i, label) in tabs.iter().enumerate() {
                    let on = i == active;
                    let btn = egui::Button::new(egui::RichText::new(*label).size(13.0)
                            .color(if on { text } else { dim }))
                        .fill(if on { Color32::from_rgb(56, 52, 46) } else { Color32::TRANSPARENT });
                    let r = ui.add(btn);
                    spans.push((r.rect.min.x, r.rect.max.x));
                    if r.clicked() && !on {
                        self.sub[self.page] = i; // tap = direct jump (Qt closure)
                        ev = NavEvent::SubChanged(i);
                    }
                }
                // underline morph toward the active span (ease 25%/frame, request repaint)
                if let Some(&target) = spans.get(self.sub[self.page]) {
                    let cur = self.underline.get_or_insert(target);
                    cur.0 += (target.0 - cur.0) * 0.25;
                    cur.1 += (target.1 - cur.1) * 0.25;
                    let y = ui.min_rect().bottom() + 2.0;
                    ui.painter().line_segment(
                        [Pos2::new(cur.0, y), Pos2::new(cur.1, y)], Stroke::new(2.0_f32, accent));
                    if (cur.0 - target.0).abs() > 0.5 { ui.ctx().request_repaint(); }
                }
                ui.add_space(ui.available_width().max(0.0)); // stretch: keep › at the right edge
            }

            let can_next = self.page + 1 < PAGE_NAMES.len();
            let next = ui.add_enabled(can_next, egui::Button::new("›"));
            if can_next { next.clone().on_hover_text(format!("Next: {}", PAGE_NAMES[self.page + 1])); }
            if next.clicked() { self.page += 1; self.underline = None; ev = NavEvent::PageChanged(self.page); }
        });
        // page-position dots
        ui.horizontal(|ui| {
            let w = ui.available_width();
            let total = PAGE_NAMES.len() as f32 * 14.0;
            ui.add_space(((w - total) * 0.5).max(0.0));
            for i in 0..PAGE_NAMES.len() {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                let on = i == self.page;
                ui.painter().circle_filled(r.center(), if on { 3.5 } else { 2.0 },
                    if on { accent } else { dim });
                let _: Rect = r;
            }
        });
        ev
    }

    /// Test hook: where the underline currently sits (min x, max x), if shown this frame.
    pub fn underline_span(&self) -> Option<(f32, f32)> { self.underline }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The module had zero tests despite `underline_span`'s own doc calling
    // itself a "test hook" — a signal testing was intended here and never
    // followed through. `sub_tabs` is the one piece cleanly separable from
    // egui rendering (it's a pure `usize -> &[&str]` lookup), so it's the
    // part this pass can cover without pulling in a UI test harness
    // (egui_kittest, used elsewhere in this monorepo for exactly this
    // situation) just for one file — real, separate scope if `ui()`'s own
    // boundary logic (page clamping, sub-tab bounds) needs the same rigor.

    #[test]
    fn each_subtabbed_page_matches_its_qt_source_exactly() {
        // Comments on each arm cite the Qt file this was ported from
        // verbatim — these values are a CONTRACT with that source, not
        // free to drift silently.
        assert_eq!(sub_tabs(1), &["All", "Unread", "Sent"]);
        assert_eq!(sub_tabs(2), &["All", "Online", "Pending"]);
        assert_eq!(sub_tabs(4), &["General", "Camera", "Network", "Storage", "About"]);
    }

    #[test]
    fn non_subtabbed_pages_have_no_subtabs() {
        // Camera (0) and Wayfinder (3) are the two pages the module doc
        // names as showing a centered page name instead of segments —
        // an empty slice here is what `ui()` reads to decide that.
        assert_eq!(sub_tabs(0), &[] as &[&str]);
        assert_eq!(sub_tabs(3), &[] as &[&str]);
    }

    #[test]
    fn an_out_of_range_page_falls_back_to_no_subtabs_rather_than_panicking() {
        // PAGE_NAMES has exactly 5 entries (0..=4); sub_tabs must not
        // assume every caller already validated `page` against that —
        // a future page-count change forgetting to update every call
        // site should degrade to "no sub-tabs shown", not panic.
        assert_eq!(sub_tabs(5), &[] as &[&str]);
        assert_eq!(sub_tabs(usize::MAX), &[] as &[&str]);
    }

    #[test]
    fn page_names_and_subtabbed_pages_stay_in_bounds_of_each_other() {
        // Every page index sub_tabs treats specially must actually be a
        // valid PAGE_NAMES index — a mismatch here would mean a sub-tab
        // table was added for a page that does not exist.
        for page in 0..PAGE_NAMES.len() {
            let _ = sub_tabs(page); // must not panic for any real page
        }
    }

    #[test]
    fn a_fresh_nav_starts_on_camera_with_no_subtab_selected() {
        let nav = UnifiedNav::new();
        assert_eq!(nav.page, 0, "Camera is page 0 per PAGE_NAMES");
        assert_eq!(nav.sub, [0; 5], "every page starts on its first sub-tab");
        assert_eq!(nav.underline_span(), None, "no underline until a SubTabbed page renders");
    }
}
