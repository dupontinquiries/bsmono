//! nav_chrome — `SegmentedTabStrip` and `PagePeelHint`.
//!
//! Two small surfaces that share a job: telling you where you are and where
//! else you can go.
//!
//! ## The strip
//!
//! A row of equal-width tabs with an underline on the selected one, extracted
//! in Qt from an inline implementation in SettingsPage so mailbox, contacts and
//! the other multi-section pages share one look instead of each growing their
//! own. It owns only the header row — the caller owns the bodies and the
//! wiring, which is why nothing here knows what a page is.
//!
//! Badges are a count pill on a tab, and zero REMOVES the pill rather than
//! drawing "0". A tab that says zero unread is telling you about nothing.
//!
//! ## The peel hint, which is mostly about knowing when to stop
//!
//! Curved tabs at the left and right edges suggesting that pages swipe
//! sideways. Everything interesting about it is restraint:
//!
//! * **Always shown, not touchscreen-gated.** Trackpad and mouse users swipe
//!   too, and gating on touch hides the affordance from exactly the people who
//!   would never guess it exists.
//! * **A slow breath, 3.2s a cycle.** Fast enough to notice at the edge of
//!   vision, slow enough not to pull the eye off the page.
//! * **Muted, not accent.** An affordance the size of a fingernail painted in
//!   the accent colour competes with the content it is pointing away from.
//! * **It leaves.** After four swipes the user has learned the gesture and the
//!   hint fades permanently. A teaching aid that never stops teaching is
//!   decoration.
//! * **It ducks after each swipe** and returns after 30s of stillness, so it is
//!   never in the way during the very interaction it is describing.
//!
//! The edges are asymmetric on purpose: on the first page there is nothing to
//! the left, and a hint pointing at a page that does not exist is worse than no
//! hint — the user swipes, nothing happens, and now they distrust the gesture.

/// `kMinSwipesBeforeHide` — how many swipes count as having learned it.
pub const MIN_SWIPES_BEFORE_HIDE: u32 = 4;
/// `kIdleReturnMs` — stillness before the hint comes back after a swipe.
pub const IDLE_RETURN_MS: f32 = 30_000.0;
/// One breath, in milliseconds.
pub const BREATH_MS: f32 = 3_200.0;

// ── segmented tab strip ─────────────────────────────────────────────────────

/// One tab.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        self.badge(i).map(|n| if n > 99 { "99+".into() } else { n.to_string() })
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

/// The hint's state.
#[derive(Debug, Clone)]
pub struct PeelHint {
    page: usize,
    pages: usize,
    swipes: u32,
    /// Time since the last swipe; the hint stays ducked until this passes.
    since_swipe_ms: f32,
    /// The shared oscillator. Slow and dim on purpose — see BREATH_MS.
    breath: uikit_egui::motion::Breath,
    /// Once true, permanently so: the user has learned the gesture.
    retired: bool,
    ducked: bool,
}

impl Default for PeelHint {
    fn default() -> Self {
        PeelHint { page: 0, pages: 0, swipes: 0, since_swipe_ms: 0.0,
                   breath: uikit_egui::motion::Breath::new(BREATH_MS, 0.35, 0.60),
                   retired: false, ducked: false }
    }
}

impl PeelHint {
    pub fn new(pages: usize) -> Self {
        PeelHint { pages, since_swipe_ms: IDLE_RETURN_MS,
                   breath: uikit_egui::motion::Breath::new(BREATH_MS, 0.35, 0.60),
                   ..Default::default() }
    }

    /// `setPageState`.
    pub fn set_page(&mut self, page: usize, pages: usize) {
        self.page = page;
        self.pages = pages;
    }

    /// Count a swipe and duck. `dismissIfFamiliar` folded in: four swipes and
    /// the hint is done for good.
    pub fn note_swipe(&mut self) {
        self.swipes += 1;
        self.since_swipe_ms = 0.0;
        self.ducked = true;
        if self.swipes >= MIN_SWIPES_BEFORE_HIDE { self.retired = true; }
    }

    pub fn tick(&mut self, dt_ms: f32) {
        self.breath.tick(dt_ms);
        if self.ducked {
            self.since_swipe_ms += dt_ms;
            if self.since_swipe_ms >= IDLE_RETURN_MS { self.ducked = false; }
        }
    }

    pub fn retired(&self) -> bool { self.retired }
    pub fn swipes(&self) -> u32 { self.swipes }

    /// Is the hint on screen at all?
    pub fn visible(&self) -> bool {
        !self.retired && !self.ducked && self.edges().left | self.edges().right
    }

    /// Which edges have somewhere to go.
    ///
    /// A hint pointing at a page that does not exist is worse than no hint: the
    /// user swipes, nothing happens, and now they distrust the gesture.
    pub fn edges(&self) -> Edges {
        Edges { left: self.page > 0, right: self.pages > 0 && self.page + 1 < self.pages }
    }

    /// The breathing opacity, 0..=1 over a 3.2s sine. Never reaches zero — a
    /// hint that vanishes mid-cycle reads as a rendering glitch rather than a
    /// pulse.
    pub fn opacity(&self) -> f32 {
        if !self.visible() { return 0.0; }
        self.breath.value()
    }
}

/// Draw the strip. Returns the index if a tab was chosen.
pub fn strip_ui(ui: &mut egui::Ui, s: &TabStrip, c: &uikit_egui::PaletteColors) -> Option<usize> {
    if s.is_empty() { return None; }
    let mut picked = None;
    let total = ui.available_width();
    let w = s.tab_width(total);
    let start = ui.cursor().min;
    ui.horizontal(|ui| {
        for (i, t) in s.tabs().iter().enumerate() {
            let sel = i == s.selected();
            let label = match s.badge_text(i) {
                Some(b) => format!("{}  {}", t.label, b),
                None => t.label.clone(),
            };
            let resp = ui.add_sized([w - 4.0, 28.0],
                egui::SelectableLabel::new(sel, egui::RichText::new(label)
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
        egui::Stroke::new(2.0, c.accent));
    picked
}

/// Draw the peel hints at the page edges.
pub fn peel_ui(ui: &mut egui::Ui, h: &mut PeelHint, rect: egui::Rect,
               c: &uikit_egui::PaletteColors) {
    h.tick(ui.input(|i| i.stable_dt) * 1000.0);
    let a = h.opacity();
    if a <= 0.0 { return; }
    // Muted, never the accent: a fingernail-sized affordance in the accent
    // colour competes with the content it is pointing away from.
    let col = egui::Color32::from_rgba_unmultiplied(
        c.text_weak.r(), c.text_weak.g(), c.text_weak.b(), (a * 255.0) as u8);
    let e = h.edges();
    let mid = rect.center().y;
    let p = ui.painter();
    if e.left {
        p.line_segment([egui::pos2(rect.left() + 2.0, mid - 16.0),
                        egui::pos2(rect.left() + 8.0, mid)], egui::Stroke::new(2.0, col));
        p.line_segment([egui::pos2(rect.left() + 8.0, mid),
                        egui::pos2(rect.left() + 2.0, mid + 16.0)], egui::Stroke::new(2.0, col));
    }
    if e.right {
        p.line_segment([egui::pos2(rect.right() - 2.0, mid - 16.0),
                        egui::pos2(rect.right() - 8.0, mid)], egui::Stroke::new(2.0, col));
        p.line_segment([egui::pos2(rect.right() - 8.0, mid),
                        egui::pos2(rect.right() - 2.0, mid + 16.0)], egui::Stroke::new(2.0, col));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> TabStrip { TabStrip::new(&["All", "Unread", "Sent"]) }

    #[test]
    fn a_strip_starts_on_its_first_tab() {
        assert_eq!(strip().selected(), 0);
    }

    #[test]
    fn selecting_out_of_range_is_refused_rather_than_clamped() {
        // A caller asking for tab 7 of 3 has a bug, and silently landing on
        // tab 2 hides it.
        let mut s = strip();
        assert!(!s.select(7));
        assert_eq!(s.selected(), 0);
        assert!(s.select(2));
        assert_eq!(s.selected(), 2);
    }

    #[test]
    fn a_zero_badge_removes_the_pill_instead_of_drawing_zero() {
        // A tab reporting zero unread is telling you about nothing.
        let mut s = strip();
        s.set_badge(1, 3);
        assert_eq!(s.badge_text(1).as_deref(), Some("3"));
        s.set_badge(1, 0);
        assert_eq!(s.badge(1), None);
        assert_eq!(s.badge_text(1), None);
    }

    #[test]
    fn a_large_badge_caps_rather_than_widening_past_its_tab() {
        let mut s = strip();
        s.set_badge(0, 99);
        assert_eq!(s.badge_text(0).as_deref(), Some("99"));
        s.set_badge(0, 100);
        assert_eq!(s.badge_text(0).as_deref(), Some("99+"));
        s.set_badge(0, 5000);
        assert_eq!(s.badge_text(0).as_deref(), Some("99+"));
    }

    #[test]
    fn a_badge_on_a_tab_that_does_not_exist_is_ignored() {
        let mut s = strip();
        s.set_badge(9, 4);
        assert_eq!(s.badge(9), None);
    }

    #[test]
    fn segments_are_equal_width_which_is_what_makes_it_segmented() {
        // The segments stay put as labels change length; a row of buttons
        // would shuffle every time a badge appeared.
        let s = strip();
        assert!((s.tab_width(300.0) - 100.0).abs() < 0.001);
        assert_eq!(TabStrip::default().tab_width(300.0), 0.0, "no divide by zero");
    }

    #[test]
    fn the_underline_tracks_the_selected_segment() {
        let mut s = strip();
        assert_eq!(s.underline(300.0), (0.0, 100.0));
        s.select(2);
        assert_eq!(s.underline(300.0), (200.0, 100.0));
    }

    // ── peel hint ───────────────────────────────────────────────────────────

    #[test]
    fn the_first_page_has_no_left_hint() {
        // A hint pointing at a page that does not exist is worse than no hint:
        // the user swipes, nothing happens, and now they distrust the gesture.
        let mut h = PeelHint::new(3);
        h.set_page(0, 3);
        assert_eq!(h.edges(), Edges { left: false, right: true });
    }

    #[test]
    fn the_last_page_has_no_right_hint() {
        let mut h = PeelHint::new(3);
        h.set_page(2, 3);
        assert_eq!(h.edges(), Edges { left: true, right: false });
    }

    #[test]
    fn a_middle_page_hints_both_ways() {
        let mut h = PeelHint::new(3);
        h.set_page(1, 3);
        assert_eq!(h.edges(), Edges { left: true, right: true });
    }

    #[test]
    fn a_single_page_app_shows_no_hint_at_all() {
        let mut h = PeelHint::new(1);
        h.set_page(0, 1);
        assert_eq!(h.edges(), Edges { left: false, right: false });
        assert!(!h.visible());
    }

    #[test]
    fn the_hint_retires_permanently_once_the_gesture_is_learned() {
        // A teaching aid that never stops teaching is decoration.
        let mut h = PeelHint::new(3);
        h.set_page(1, 3);
        for _ in 0..MIN_SWIPES_BEFORE_HIDE - 1 {
            h.note_swipe();
            assert!(!h.retired());
        }
        h.note_swipe();
        assert!(h.retired());
        // And it does not come back after any amount of stillness.
        h.tick(IDLE_RETURN_MS * 10.0);
        assert!(!h.visible());
    }

    #[test]
    fn the_hint_ducks_during_a_swipe_and_returns_after_stillness() {
        // Never in the way during the very interaction it describes.
        let mut h = PeelHint::new(5);
        h.set_page(2, 5);
        assert!(h.visible());
        h.note_swipe();
        assert!(!h.visible(), "ducked");
        h.tick(IDLE_RETURN_MS - 1.0);
        assert!(!h.visible(), "still ducked just short of the idle window");
        h.tick(2.0);
        assert!(h.visible(), "and back after 30s of stillness");
    }

    #[test]
    fn the_breath_never_reaches_zero_while_visible() {
        // A hint that vanishes mid-cycle reads as a rendering glitch rather
        // than a pulse.
        let mut h = PeelHint::new(3);
        h.set_page(1, 3);
        let mut lo = f32::INFINITY;
        let mut hi: f32 = 0.0;
        for _ in 0..400 {
            h.tick(BREATH_MS / 100.0);
            let a = h.opacity();
            lo = lo.min(a);
            hi = hi.max(a);
        }
        assert!(lo > 0.2, "dimmest was {lo}");
        assert!(hi < 0.7, "brightest was {hi} — a hint should not shout");
        assert!(hi - lo > 0.1, "and it should visibly breathe");
    }

    #[test]
    fn the_breath_cycle_is_the_documented_length() {
        assert_eq!(BREATH_MS, 3_200.0);
        let mut h = PeelHint::new(3);
        h.set_page(1, 3);
        let start = h.opacity();
        h.tick(BREATH_MS);
        assert!((h.opacity() - start).abs() < 0.001, "one full cycle returns to the start");
    }

    #[test]
    fn a_retired_hint_is_transparent_not_merely_hidden() {
        let mut h = PeelHint::new(3);
        h.set_page(1, 3);
        for _ in 0..MIN_SWIPES_BEFORE_HIDE { h.note_swipe(); }
        assert_eq!(h.opacity(), 0.0);
    }

    #[test]
    fn a_fresh_hint_is_visible_immediately_rather_than_after_an_idle_wait() {
        // Starting ducked would hide the affordance for the first 30 seconds —
        // exactly the window where a new user needs it.
        let mut h = PeelHint::new(3);
        h.set_page(1, 3);
        assert!(h.visible());
        assert!(h.opacity() > 0.0);
    }
}
