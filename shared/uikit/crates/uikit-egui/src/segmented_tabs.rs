//! segmented_tabs — an equal-width tab strip with badges and an underline.
//!
//! Hoisted from snapclone's `nav_chrome` because three apps were hand-rolling
//! the same row. snapclone, bo-suite and bot-arena each iterate their page
//! list into `selectable_label`s, which gives none of the three things this
//! provides:
//!
//! * **Equal segments.** A `selectable_label` row sizes to its text, so the
//!   tabs jitter as labels change and the hit targets differ in width. Equal
//!   thirds is what makes a strip a strip.
//! * **An underline rather than a filled box.** The box fights card borders
//!   and reads as a pressed button on a surface full of them.
//! * **Badges**, with a shared overflow cap. That cap is the reason this
//!   belongs in the kit rather than being copied a fourth time: two bars in
//!   one app disagreeing about what a large count looks like is visible on a
//!   single screen, and so is two apps.
//!
//! The peel hint stays in snapclone: it is about page-swipe discovery on a
//! phone, which is that app's navigation model rather than a general widget.

use crate::PaletteColors;

/// One segment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tab {
    pub label: String,
    /// `None` draws no pill. Zero is normalised to `None` on the way in.
    pub badge: Option<u32>,
}

/// The strip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabStrip {
    tabs: Vec<Tab>,
    selected: usize,
}

impl TabStrip {
    pub fn new(labels: &[&str]) -> Self {
        TabStrip {
            tabs: labels.iter().map(|l| Tab { label: (*l).into(), badge: None }).collect(),
            selected: 0,
        }
    }

    pub fn tabs(&self) -> &[Tab] { &self.tabs }
    pub fn len(&self) -> usize { self.tabs.len() }
    pub fn is_empty(&self) -> bool { self.tabs.is_empty() }
    pub fn selected(&self) -> usize { self.selected }

    /// Select a tab. Out-of-range is ignored rather than clamped: a caller
    /// asking for tab 7 of 3 has a bug, and silently landing on tab 2 hides it.
    pub fn select(&mut self, i: usize) -> bool {
        if i >= self.tabs.len() { return false; }
        self.selected = i;
        true
    }

    /// `setBadge`. Zero removes the pill — a tab reporting zero unread is
    /// telling you about nothing, and the pill is then pure noise.
    pub fn set_badge(&mut self, i: usize, count: u32) {
        if let Some(t) = self.tabs.get_mut(i) {
            t.badge = if count == 0 { None } else { Some(count) };
        }
    }

    pub fn badge(&self, i: usize) -> Option<u32> { self.tabs.get(i).and_then(|t| t.badge) }

    /// The badge's text. Large counts cap rather than widening the pill past
    /// the tab it sits on.
    pub fn badge_text(&self, i: usize) -> Option<String> {
        // Shared cap: two bars in one app disagreeing about what a large count
        // looks like is visible on a single screen.
        self.badge(i).and_then(|n| crate::fmt::badge_count(n, 99))
    }

    /// Equal widths, which is what makes it a SEGMENTED strip rather than a row
    /// of buttons: the segments stay put as labels change length.
    pub fn tab_width(&self, total: f32) -> f32 {
        if self.tabs.is_empty() { return 0.0; }
        total / self.tabs.len() as f32
    }

    /// Where the underline sits, as (left, width).
    pub fn underline(&self, total: f32) -> (f32, f32) {
        let w = self.tab_width(total);
        (w * self.selected as f32, w)
    }
}

// ── page peel hint ──────────────────────────────────────────────────────────

/// Which edges should show a peel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edges { pub left: bool, pub right: bool }

/// Draw the strip. Returns the index if a tab was chosen.
pub fn strip_ui(ui: &mut egui::Ui, s: &TabStrip, c: &PaletteColors) -> Option<usize> {
    if s.is_empty() { return None; }
    let mut picked = None;
    let total = ui.available_width();
    let w = s.tab_width(total);
    let start = ui.cursor().min;
    ui.horizontal(|ui| {
        // Each button is `w - 4` wide, so the gap has to be exactly 4 for a
        // segment to be `w` — which is what `underline` assumes. Inheriting the
        // palette's item spacing (10) made every segment 6pt too wide: the
        // underline drifted left of its tab and, by the fifth tab, the strip was
        // 20pt past the right edge. The padding is pinned for the same reason: a
        // touch-sized button padding makes a short label wider than the width it
        // was given.
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().button_padding.x = 2.0;
        for (i, t) in s.tabs().iter().enumerate() {
            let sel = i == s.selected();
            let label = match s.badge_text(i) {
                Some(b) => format!("{}  {}", t.label, b),
                None => t.label.clone(),
            };
            let resp = ui.add_sized([w - 4.0, 28.0],
                egui::Button::selectable(sel, egui::RichText::new(label)
                    .color(if sel { c.text } else { c.text_weak })));
            if resp.clicked() { picked = Some(i); }
        }
    });
    // The underline, drawn under the selected segment rather than around it:
    // a box would fight the card borders every page already has.
    let (x, uw) = s.underline(total);
    let y = ui.cursor().min.y - 2.0;
    ui.painter().line_segment(
        [egui::pos2(start.x + x + 2.0, y), egui::pos2(start.x + x + uw - 2.0, y)],
        egui::Stroke::new(2.0_f32, c.accent));
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_probe;

    /// Lay the strip out headlessly, under the palette every app applies, and
    /// return each label's horizontal centre plus the surface width.
    fn label_centres(labels: &[&str], surface_w: f32) -> Vec<(String, f32, f32)> {
        let strip = TabStrip::new(labels);
        let mut shapes = Vec::new();
        let ctx = egui::Context::default();
        let colors = crate::apply_palette(&ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(surface_w, 200.0)));
        for _ in 0..2 {
            #[allow(deprecated)]
            let out = ctx.run(input.clone(), |ctx| {
                #[allow(deprecated)]
                egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
                    let _ = strip_ui(ui, &strip, &colors);
                });
            });
            shapes = out.shapes;
        }
        layout_probe::texts(&shapes).into_iter().filter(|t| labels.contains(&t.text.as_str())).map(|t| (t.text.clone(), t.rect.center().x, t.rect.right())).collect()
    }

    /// The strip's arithmetic says "equal segments across the width" and the
    /// underline is placed by that arithmetic. The buttons have to land where
    /// the arithmetic says, or the last tab runs off the edge and the underline
    /// sits under the wrong label. Found on a 5-tab strip at phone width, where
    /// the palette's 10pt item spacing pushed the fifth tab 20pt past the edge;
    /// with 3 tabs the same fault is 8pt and easy to miss.
    #[test]
    fn every_label_sits_inside_its_own_segment() {
        for (labels, w) in [(&["Launch", "Stack", "Vitals", "Wire", "Caches"][..], 380.0_f32), (&["One", "Two", "Three"][..], 320.0)] {
            let got = label_centres(labels, w);
            assert_eq!(got.len(), labels.len(), "every label was laid out: {got:?}");
            let seg = w / labels.len() as f32;
            for (i, (text, cx, right)) in got.iter().enumerate() {
                assert!(*right <= w + 0.5, "{text:?} runs off a {w}pt surface (right edge {right})");
                let (lo, hi) = (seg * i as f32, seg * (i + 1) as f32);
                assert!((lo..=hi).contains(cx), "{text:?} is centred at {cx}, outside its segment {lo}..{hi}");
            }
        }
    }
}
