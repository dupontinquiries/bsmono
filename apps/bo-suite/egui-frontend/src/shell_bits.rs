//! shell_bits — `ControlCard`, `InfoRow`, `GroupRow`, `PageBar`.
//!
//! The vocabulary the pages are assembled from. Small pieces, but they are what
//! makes bo-suite's pages look like one app rather than four.
//!
//! ## The card's entrance is staggered, and the stagger is the feature
//!
//! Each card fades and rises into place on a delay set by its position. Pages
//! then ASSEMBLE rather than appearing all at once, which is the difference
//! between "the app is composing itself" and "the app finished loading". The
//! delay is per-card and supplied by the page, because only the page knows the
//! order the cards read in.
//!
//! An active card holds the accent in its border and gains a slow breathing
//! glow — the card "listens". Slow on purpose, 1.8s each way: a fast pulse on a
//! container reads as an alert.
//!
//! ## GroupRow's bar is relative, not absolute
//!
//! An artist with 40 tracks next to one with 2 should show a long bar and a
//! short one, whatever the numbers are. So the bar is `count / maxCount` across
//! the visible list — and the floor of 2px matters: a bar that can reach zero
//! width disappears entirely for the smallest row, and an invisible bar looks
//! like a rendering fault rather than a small number.
//!
//! ## PageBar is the named menu; PageDots is the swipe hint
//!
//! They coexist deliberately — dots at the bottom hint at position, the bar at
//! the top lets you jump. The bar scrolls when the names do not fit, and the
//! active page snaps into view, because a nav control that can hide the thing
//! it is describing is worse than no nav control.

use uikit_egui::motion::ease;

/// The card's fade-and-rise, in milliseconds.
pub const ENTRANCE_MS: f32 = 260.0;
/// How far it rises, in pixels.
pub const ENTRANCE_RISE: f32 = 8.0;
/// Half a breath of the active glow.
pub const BREATH_MS: f32 = 1800.0;
/// The glow's peak opacity — barely there, on purpose.
pub const GLOW_PEAK: f32 = 0.35;
/// The narrowest a relative bar may draw.
pub const BAR_MIN_PX: f32 = 2.0;

/// A labelled container.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlCard {
    pub label: String,
    pub active: bool,
    /// Milliseconds to wait before this card begins its entrance.
    pub entrance_delay: f32,
    elapsed: f32,
    breath: f32,
}

impl ControlCard {
    pub fn new(label: &str, entrance_delay: f32) -> Self {
        ControlCard { label: label.into(), active: false, entrance_delay,
                      elapsed: 0.0, breath: 0.0 }
    }

    pub fn tick(&mut self, dt_ms: f32) {
        self.elapsed += dt_ms;
        if self.active { self.breath = (self.breath + dt_ms) % (BREATH_MS * 2.0); }
        else { self.breath = 0.0; }
    }

    /// 0..=1 progress through the entrance, after the delay.
    fn entrance_t(&self) -> f32 {
        ((self.elapsed - self.entrance_delay) / ENTRANCE_MS).clamp(0.0, 1.0)
    }

    /// Opacity. Starts at zero so a card genuinely arrives rather than
    /// appearing dim and then brightening in place.
    pub fn opacity(&self) -> f32 { ease::out_cubic(self.entrance_t()) }

    /// Vertical offset, falling to zero as it settles.
    pub fn rise(&self) -> f32 { ENTRANCE_RISE * (1.0 - ease::out_cubic(self.entrance_t())) }

    pub fn settled(&self) -> bool { self.entrance_t() >= 1.0 }

    /// The breathing glow, 0..=GLOW_PEAK. Zero unless the card is active.
    ///
    /// Slow — 1.8 seconds each way — because a fast pulse on a container reads
    /// as an alert rather than as attention.
    pub fn glow(&self) -> f32 {
        if !self.active { return 0.0; }
        let t = self.breath / BREATH_MS;
        let phase = if t <= 1.0 { t } else { 2.0 - t };
        GLOW_PEAK * ease::out_cubic(phase.clamp(0.0, 1.0))
    }

    /// The label's presentation: wide uppercase tracking, per the B&O type.
    pub fn label_text(&self) -> String { self.label.to_uppercase() }
}

/// A key/value row for the device info card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoRow {
    pub key: String,
    pub value: String,
}

impl InfoRow {
    pub fn new(key: &str, value: &str) -> Self {
        InfoRow { key: key.into(), value: value.into() }
    }

    /// The key is a quiet uppercase micro-label.
    pub fn key_text(&self) -> String { self.key.to_uppercase() }

    /// A missing value shows an em dash rather than nothing. A blank right-hand
    /// side reads as a row that failed to load, where "—" reads as a device
    /// that did not report it.
    pub fn value_text(&self) -> String {
        if self.value.trim().is_empty() { "—".into() } else { self.value.clone() }
    }
}

/// A row in the Artists or Tags list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub name: String,
    pub count: u32,
}

impl GroupRow {
    /// The bar's width in pixels, relative to the largest row on screen.
    ///
    /// Floored at 2px: a bar that can reach zero width disappears entirely for
    /// the smallest row, and an invisible bar looks like a rendering fault
    /// rather than a small number.
    pub fn bar_width(&self, track_width: f32, max_count: u32) -> f32 {
        let max = max_count.max(1) as f32;
        let frac = (self.count as f32 / max).clamp(0.0, 1.0);
        (track_width * frac).max(BAR_MIN_PX).min(track_width.max(BAR_MIN_PX))
    }

    pub fn label(&self) -> String {
        if self.name.trim().is_empty() { "(unnamed)".into() } else { self.name.clone() }
    }

    pub fn count_text(&self) -> String {
        if self.count == 1 { "1".into() } else { self.count.to_string() }
    }
}

/// The largest count in a list, for the relative bars.
pub fn max_count(rows: &[GroupRow]) -> u32 { rows.iter().map(|r| r.count).max().unwrap_or(1) }

// ── page bar ────────────────────────────────────────────────────────────────

/// Named, tappable page navigation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageBar {
    names: Vec<String>,
    active: usize,
    /// Horizontal scroll offset, in pixels.
    pub scroll: f32,
}

impl PageBar {
    pub fn new(names: &[&str]) -> Self {
        PageBar { names: names.iter().map(|s| (*s).to_string()).collect(),
                  active: 0, scroll: 0.0 }
    }

    pub fn names(&self) -> &[String] { &self.names }
    pub fn active(&self) -> usize { self.active }
    pub fn len(&self) -> usize { self.names.len() }
    pub fn is_empty(&self) -> bool { self.names.is_empty() }

    /// Select a page. Out of range is refused rather than clamped, as
    /// everywhere else in this port — a caller asking for a page that is not
    /// there has a bug, and landing somewhere plausible hides it.
    pub fn select(&mut self, i: usize) -> bool {
        if i >= self.names.len() { return false; }
        self.active = i;
        true
    }

    /// Where a tab starts and how wide it is, given a uniform tab width.
    pub fn tab_span(&self, i: usize, tab_w: f32) -> (f32, f32) { (i as f32 * tab_w, tab_w) }

    /// Scroll so the active tab is fully visible.
    ///
    /// A nav control that can hide the page it is describing is worse than no
    /// nav control — you look at the bar to find out where you are.
    pub fn scroll_to_active(&mut self, viewport_w: f32, tab_w: f32) {
        let (x, w) = self.tab_span(self.active, tab_w);
        if x < self.scroll {
            self.scroll = x;
        } else if x + w > self.scroll + viewport_w {
            self.scroll = x + w - viewport_w;
        }
        let content = self.names.len() as f32 * tab_w;
        // Never scroll past the ends: blank space at the edge of a nav bar
        // reads as missing pages.
        self.scroll = self.scroll.clamp(0.0, (content - viewport_w).max(0.0));
    }

    /// Does the bar need to scroll at all?
    pub fn scrolls(&self, viewport_w: f32, tab_w: f32) -> bool {
        self.names.len() as f32 * tab_w > viewport_w
    }
}

/// Draw a control card's frame and run its entrance. The caller draws the
/// contents inside the returned rect.
pub fn card_frame(ui: &mut egui::Ui, card: &mut ControlCard, height: f32,
                  c: &uikit_egui::PaletteColors) -> egui::Rect {
    card.tick(ui.input(|i| i.stable_dt) * 1000.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height), egui::Sense::hover());
    let r = rect.translate(egui::vec2(0.0, card.rise()));
    let a = card.opacity();
    let fade = |col: egui::Color32| egui::Color32::from_rgba_unmultiplied(
        col.r(), col.g(), col.b(), (col.a() as f32 * a) as u8);
    ui.painter().rect_filled(r, 8.0, fade(c.bg_card));
    let border = if card.active { c.accent } else { c.border };
    ui.painter().rect_stroke(r, 8.0, egui::Stroke::new(1.0, fade(border)),
                             egui::StrokeKind::Inside);
    if card.glow() > 0.0 {
        let g = (card.glow() * a * 255.0) as u8;
        ui.painter().rect_stroke(r.expand(2.0), 10.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgba_unmultiplied(
                c.accent.r(), c.accent.g(), c.accent.b(), g)),
            egui::StrokeKind::Outside);
    }
    if !card.label.is_empty() {
        ui.painter().text(r.left_top() + egui::vec2(10.0, 8.0), egui::Align2::LEFT_TOP,
            card.label_text(), egui::FontId::proportional(10.0), fade(c.text_weak));
    }
    r
}

/// Draw the page bar. Returns the index when one is tapped.
pub fn page_bar_ui(ui: &mut egui::Ui, bar: &mut PageBar, tab_w: f32,
                   c: &uikit_egui::PaletteColors) -> Option<usize> {
    if bar.is_empty() { return None; }
    let mut picked = None;
    let viewport = ui.available_width();
    bar.scroll_to_active(viewport, tab_w);
    egui::ScrollArea::horizontal().id_salt("page_bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            for (i, name) in bar.names().iter().enumerate() {
                let sel = i == bar.active();
                let text = egui::RichText::new(name)
                    .color(if sel { c.accent } else { c.text_weak });
                if ui.add_sized([tab_w, 28.0], egui::SelectableLabel::new(sel, text)).clicked() {
                    picked = Some(i);
                }
            }
        });
    });
    if let Some(i) = picked { bar.select(i); }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_arrives_rather_than_brightening_in_place() {
        // Starting at zero opacity is what makes it an entrance.
        let mut card = ControlCard::new("Volume", 0.0);
        assert_eq!(card.opacity(), 0.0);
        assert_eq!(card.rise(), ENTRANCE_RISE);
        card.tick(ENTRANCE_MS);
        assert_eq!(card.opacity(), 1.0);
        assert_eq!(card.rise(), 0.0);
        assert!(card.settled());
    }

    #[test]
    fn the_stagger_is_what_makes_a_page_assemble() {
        // A later card is still invisible when an earlier one has arrived —
        // the difference between "composing itself" and "finished loading".
        let mut first = ControlCard::new("A", 0.0);
        let mut third = ControlCard::new("C", 120.0);
        first.tick(100.0);
        third.tick(100.0);
        assert!(first.opacity() > 0.0);
        assert_eq!(third.opacity(), 0.0, "still waiting its turn");
        third.tick(ENTRANCE_MS + 200.0);
        assert_eq!(third.opacity(), 1.0);
    }

    #[test]
    fn the_rise_falls_to_zero_as_the_card_settles() {
        let mut card = ControlCard::new("A", 0.0);
        card.tick(ENTRANCE_MS / 2.0);
        let mid = card.rise();
        assert!(mid > 0.0 && mid < ENTRANCE_RISE);
        card.tick(ENTRANCE_MS);
        assert_eq!(card.rise(), 0.0);
    }

    #[test]
    fn only_an_active_card_breathes() {
        let mut card = ControlCard::new("A", 0.0);
        card.tick(BREATH_MS / 2.0);
        assert_eq!(card.glow(), 0.0);
        card.active = true;
        card.tick(BREATH_MS / 2.0);
        assert!(card.glow() > 0.0);
    }

    #[test]
    fn the_glow_stays_barely_there_and_cycles_both_ways() {
        // A fast or bright pulse on a CONTAINER reads as an alert rather than
        // as attention.
        let mut card = ControlCard::new("A", 0.0);
        card.active = true;
        let mut peak: f32 = 0.0;
        let mut trough = f32::INFINITY;
        for _ in 0..400 {
            card.tick(BREATH_MS * 2.0 / 100.0);
            peak = peak.max(card.glow());
            trough = trough.min(card.glow());
        }
        assert!(peak <= GLOW_PEAK + 0.001, "peaked at {peak}");
        assert!(trough < 0.05, "and comes back down to {trough}");
        assert!(BREATH_MS >= 1500.0, "slow enough not to read as an alert");
    }

    #[test]
    fn deactivating_a_card_stops_the_glow_immediately() {
        let mut card = ControlCard::new("A", 0.0);
        card.active = true;
        card.tick(BREATH_MS / 2.0);
        assert!(card.glow() > 0.0);
        card.active = false;
        card.tick(1.0);
        assert_eq!(card.glow(), 0.0);
    }

    #[test]
    fn the_card_label_uses_the_wide_uppercase_tracking() {
        assert_eq!(ControlCard::new("Volume", 0.0).label_text(), "VOLUME");
    }

    // ── info row ────────────────────────────────────────────────────────────

    #[test]
    fn a_missing_value_shows_a_dash_rather_than_nothing() {
        // A blank right-hand side reads as a row that failed to load; "—" reads
        // as a device that did not report it.
        assert_eq!(InfoRow::new("Firmware", "").value_text(), "—");
        assert_eq!(InfoRow::new("Firmware", "   ").value_text(), "—");
        assert_eq!(InfoRow::new("Firmware", "3.4.1").value_text(), "3.4.1");
    }

    #[test]
    fn the_info_key_is_a_quiet_uppercase_micro_label() {
        assert_eq!(InfoRow::new("Serial number", "x").key_text(), "SERIAL NUMBER");
    }

    // ── group row ───────────────────────────────────────────────────────────

    #[test]
    fn the_bar_is_relative_to_the_largest_row_on_screen() {
        // 40 tracks beside 2 should look like 40 beside 2, whatever the numbers
        // happen to be.
        let big = GroupRow { name: "Ada".into(), count: 40 };
        let small = GroupRow { name: "Bo".into(), count: 2 };
        let max = max_count(&[big.clone(), small.clone()]);
        assert_eq!(max, 40);
        assert!((big.bar_width(100.0, max) - 100.0).abs() < 0.01);
        assert!((small.bar_width(100.0, max) - 5.0).abs() < 0.01);
    }

    #[test]
    fn the_smallest_row_still_draws_a_visible_bar() {
        // A bar that can reach zero width disappears entirely, and an invisible
        // bar looks like a rendering fault rather than a small number.
        let tiny = GroupRow { name: "Cy".into(), count: 1 };
        assert!(tiny.bar_width(100.0, 100_000) >= BAR_MIN_PX);
    }

    #[test]
    fn a_zero_count_row_still_draws_the_floor() {
        let none = GroupRow { name: "Empty".into(), count: 0 };
        assert_eq!(none.bar_width(100.0, 10), BAR_MIN_PX);
    }

    #[test]
    fn the_bar_never_exceeds_its_track() {
        // A stale max from a previous list would otherwise overflow the row.
        let r = GroupRow { name: "Ada".into(), count: 999 };
        assert!(r.bar_width(100.0, 10) <= 100.0);
    }

    #[test]
    fn an_empty_list_does_not_divide_by_zero() {
        assert_eq!(max_count(&[]), 1);
        let r = GroupRow { name: "Ada".into(), count: 5 };
        assert!(r.bar_width(100.0, 0).is_finite());
    }

    #[test]
    fn an_unnamed_group_still_has_a_label() {
        assert_eq!(GroupRow { name: "  ".into(), count: 3 }.label(), "(unnamed)");
    }

    // ── page bar ────────────────────────────────────────────────────────────

    fn bar() -> PageBar { PageBar::new(&["Devices", "Spaces", "Library", "Mixer", "Settings"]) }

    #[test]
    fn selecting_out_of_range_is_refused_not_clamped() {
        let mut b = bar();
        assert!(!b.select(99));
        assert_eq!(b.active(), 0);
        assert!(b.select(3));
        assert_eq!(b.active(), 3);
    }

    #[test]
    fn the_active_tab_is_scrolled_into_view_from_the_right() {
        // A nav control that can hide the page it is describing is worse than
        // no nav control.
        let mut b = bar();
        b.select(4);
        b.scroll_to_active(200.0, 80.0);
        let (x, w) = b.tab_span(4, 80.0);
        assert!(x >= b.scroll && x + w <= b.scroll + 200.0, "tab 4 is visible");
    }

    #[test]
    fn the_active_tab_is_scrolled_into_view_from_the_left() {
        let mut b = bar();
        b.select(4);
        b.scroll_to_active(200.0, 80.0);
        b.select(0);
        b.scroll_to_active(200.0, 80.0);
        assert_eq!(b.scroll, 0.0);
    }

    #[test]
    fn scrolling_never_runs_past_the_ends() {
        // Blank space at the edge of a nav bar reads as missing pages.
        let mut b = bar();
        b.select(4);
        b.scroll_to_active(200.0, 80.0);
        let content = 5.0 * 80.0;
        assert!(b.scroll <= content - 200.0 + 0.01);
        assert!(b.scroll >= 0.0);
    }

    #[test]
    fn a_bar_that_fits_does_not_scroll_at_all() {
        let mut b = bar();
        assert!(!b.scrolls(500.0, 80.0));
        b.select(4);
        b.scroll_to_active(500.0, 80.0);
        assert_eq!(b.scroll, 0.0);
    }

    #[test]
    fn a_bar_that_does_not_fit_reports_that_it_scrolls() {
        assert!(bar().scrolls(200.0, 80.0));
    }

    #[test]
    fn an_empty_bar_is_harmless() {
        let mut b = PageBar::default();
        assert!(b.is_empty());
        assert!(!b.select(0));
        b.scroll_to_active(200.0, 80.0);
        assert_eq!(b.scroll, 0.0);
    }
}
