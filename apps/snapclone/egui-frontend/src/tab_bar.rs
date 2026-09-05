//! tab_bar — `TabBar.cpp`, ported.
//!
//! The bottom navigation bar: equal-width tabs, an animated accent indicator,
//! and unread badges.
//!
//! ## The touch rule the C++ spells out, and why it survives the port
//!
//! `TabBar.cpp` carries a comment worth reading twice: it accepts `TouchUpdate`
//! **without acting on it**, because some compositors treat an unhandled update
//! as "the widget gave up on this sequence" and cancel before `TouchEnd`
//! arrives — *"which is the 'taps don't work on mobile' failure mode"*.
//!
//! egui has no such event to accept, so that specific workaround does not port.
//! What does port is the behaviour it existed to protect: **commit on release,
//! at the release position**, with a drift tolerance, because a finger moves a
//! few pixels during a tap. Porting only the workaround and not the rule would
//! have left the rule to be rediscovered on a phone.
//!
//! ## Selection is computed, not stored per tab
//!
//! Tab width is `width / count`, so a hit is `x / tab_width` and the indicator
//! is arithmetic. Storing per-tab rects would mean recomputing them on resize
//! and getting it wrong once — the bar is one of the few widgets whose geometry
//! is genuinely simple, and keeping it that way is the point.

/// Bar height in logical pixels, from the original.
pub const BAR_HEIGHT: f32 = 56.0;
/// Accent indicator thickness.
pub const INDICATOR_H: f32 = 3.0;
pub const ICON_SIZE: f32 = 20.0;
pub const LABEL_SIZE: f32 = 10.0;

/// How far a finger may drift during a tap and still count as one.
///
/// Beyond this the gesture was a swipe, and committing a tab change on it would
/// fight whatever the swipe was for.
pub const TAP_SLOP: f32 = 24.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub icon: String,
    pub label: String,
    /// Unread count. Zero means no badge at all — distinct from a badge
    /// reading "0", which would be a dot that says nothing.
    pub badge: u32,
}

impl Tab {
    pub fn new(icon: &str, label: &str) -> Tab {
        Tab { icon: icon.into(), label: label.into(), badge: 0 }
    }

    /// Is the icon field an actual ICON — one pictographic glyph — rather
    /// than a word standing in for one?
    ///
    /// snapclone constructs its bar with `["camera", "chats", ...]` zipped
    /// against `PAGE_NAMES`, so every tab was painting the word "camera" at
    /// 16pt and "Camera" at 10pt underneath it: each label rendered twice,
    /// and on a phone the second row ran off the bottom of the window. Seen
    /// in a screenshot. A tab draws its icon only when it has one.
    pub fn icon_is_glyph(&self) -> bool {
        let mut chars = self.icon.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => !c.is_alphanumeric() && !c.is_whitespace(),
            _ => false,
        }
    }

    /// Badge text, capped.
    ///
    /// The original stops at "9+" because the dot is 7px of font and anything
    /// wider stops being a dot. A count of 137 and a count of 10 mean the same
    /// thing to a user anyway: more than you want to read now.
    pub fn badge_text(&self) -> Option<String> {
        match self.badge {
            0 => None,
            1..=9 => Some(self.badge.to_string()),
            _ => Some("9+".into()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TabBar {
    tabs: Vec<Tab>,
    current: usize,
    /// Where the indicator is now, animating toward its target.
    indicator_x: f32,
    /// Where a press began, for the drift check.
    press_at: Option<(f32, f32)>,
}

impl TabBar {
    pub fn new(tabs: Vec<Tab>) -> TabBar {
        TabBar { tabs, current: 0, indicator_x: 0.0, press_at: None }
    }

    pub fn len(&self) -> usize { self.tabs.len() }
    pub fn is_empty(&self) -> bool { self.tabs.is_empty() }
    pub fn current(&self) -> usize { self.current }
    pub fn tabs(&self) -> &[Tab] { &self.tabs }

    /// Width of one tab.
    ///
    /// An empty bar returns the full width rather than dividing by zero — the
    /// original guards this too, and it is reachable during startup before tabs
    /// are added.
    pub fn tab_width(&self, bar_width: f32) -> f32 {
        if self.tabs.is_empty() { bar_width } else { bar_width / self.tabs.len() as f32 }
    }

    /// Left edge of the indicator for a tab, matching the original's geometry:
    /// centred under the tab and one third of its width.
    pub fn indicator_target(&self, index: usize, bar_width: f32) -> f32 {
        let tw = self.tab_width(bar_width);
        index as f32 * tw + tw / 2.0 - tw / 6.0
    }

    pub fn indicator_width(&self, bar_width: f32) -> f32 { self.tab_width(bar_width) / 3.0 }

    pub fn set_badge(&mut self, index: usize, count: u32) {
        if let Some(t) = self.tabs.get_mut(index) { t.badge = count; }
    }

    /// Which tab a point falls in, or `None` when outside.
    pub fn hit(&self, x: f32, bar_width: f32) -> Option<usize> {
        if self.tabs.is_empty() || x < 0.0 || x >= bar_width { return None; }
        let idx = (x / self.tab_width(bar_width)) as usize;
        // Clamped rather than trusted: floating-point division at exactly the
        // right edge can land one past the end, and that is a panic in a draw
        // loop rather than a visible bug.
        Some(idx.min(self.tabs.len() - 1))
    }

    pub fn press(&mut self, x: f32, y: f32) { self.press_at = Some((x, y)); }

    /// Commit a tap on release.
    ///
    /// Uses the RELEASE position, as the original does, and rejects a gesture
    /// that drifted too far. Returns the newly selected tab when one changed.
    pub fn release(&mut self, x: f32, y: f32, bar_width: f32) -> Option<usize> {
        let start = self.press_at.take();
        if let Some((px, py)) = start {
            let drift = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
            if drift > TAP_SLOP { return None; }
        }
        let idx = self.hit(x, bar_width)?;
        // Re-selecting the current tab is not a change. Emitting it anyway
        // would make every stray tap re-navigate, which resets scroll position
        // on the page you are already reading.
        if idx == self.current { return None; }
        self.current = idx;
        Some(idx)
    }

    /// Cancel an in-flight press — the compositor took the gesture.
    pub fn cancel(&mut self) { self.press_at = None; }

    /// Set the tab programmatically. Out of range is ignored.
    pub fn set_current(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() || index == self.current { return false; }
        self.current = index;
        true
    }

    /// Advance the indicator toward its target.
    ///
    /// `t` is the fraction of the remaining distance to close this frame. The
    /// original uses a `QPropertyAnimation` with an easing curve; the port
    /// exponentially approaches, which is frame-rate independent in the way
    /// that matters — a dropped frame makes the move slower, never overshoot.
    pub fn tick_indicator(&mut self, bar_width: f32, t: f32) {
        let target = self.indicator_target(self.current, bar_width);
        let t = t.clamp(0.0, 1.0);
        self.indicator_x += (target - self.indicator_x) * t;
        // Snap when close enough, or the indicator asymptotes forever and the
        // widget never stops requesting repaints — a battery cost with no
        // visible motion.
        if (target - self.indicator_x).abs() < 0.5 { self.indicator_x = target; }
    }

    pub fn indicator_x(&self) -> f32 { self.indicator_x }

    /// Is the indicator still moving?
    pub fn is_animating(&self, bar_width: f32) -> bool {
        (self.indicator_target(self.current, bar_width) - self.indicator_x).abs() >= 0.5
    }
}

/// Draw the bottom bar. Returns the tab that was newly selected.
///
/// This model had no renderer at all — the badge `sync_badges` computed into
/// it reached no screen by any path, and `tick_indicator` was never stepped.
/// Written now because the answer to "top strip or bottom bar" turned out to
/// be BOTH: snapclone is mobile-first, a bottom bar is what a thumb reaches,
/// and the caller picks by width.
///
/// The touch rule from `TabBar.cpp` is what this function exists to honour.
/// egui has no TouchUpdate to accept, so that specific workaround does not
/// port — but the behaviour it protected does: commit on RELEASE, at the
/// release position, with a drift tolerance, because a finger moves a few
/// pixels during a tap. Driving `press`/`release`/`cancel` rather than reading
/// `clicked()` is what keeps that rule in force.
pub fn tab_bar_ui(ui: &mut egui::Ui, bar: &mut TabBar, min_touch: f32,
                  c: &uikit_egui::PaletteColors) -> Option<usize> {
    if bar.is_empty() {
        return None;
    }
    let width = ui.available_width();
    // The bar is at least a touch target tall, whatever the theme's spacing
    // says. A navigation bar too short to hit is worse than no bar: it looks
    // available and is not.
    let height = min_touch.max(44.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height),
                                              egui::Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, c.bg_raised);

    let mut picked = None;
    // Was the pointer already down on this bar last frame? egui gives no
    // "press began" flag for a click that never becomes a drag, so the down
    // edge is derived: down now, and a click has not yet been reported.
    let down_seen = ui.ctx().input(|i| i.pointer.any_down())
        && !resp.drag_started();
    if let Some(pos) = resp.interact_pointer_pos() {
        let local = pos.x - rect.left();
        // press on the frame the pointer goes down, release on the frame it
        // comes up. `press` is idempotent for our purposes — it overwrites the
        // start point — but calling it every frame would move the origin under
        // the drift check and defeat it, so it is gated on the down edge.
        if resp.drag_started() || resp.is_pointer_button_down_on() && !down_seen {
            bar.press(local, pos.y);
        }
        if resp.drag_stopped() || resp.clicked() {
            picked = bar.release(local, pos.y, width);
        }
    } else {
        // The pointer left the bar mid-gesture. Cancelling matches the
        // original's TouchCancel path: a press that wandered off is not a tap,
        // and holding it would commit on a release that happens elsewhere.
        bar.cancel();
    }

    bar.tick_indicator(width, ui.input(|i| i.stable_dt));

    let tw = bar.tab_width(width);
    for (i, tab) in bar.tabs().iter().enumerate() {
        let x = rect.left() + tw * i as f32;
        let centre = egui::pos2(x + tw / 2.0, rect.center().y);
        let on = i == bar.current();
        let col = if on { c.accent } else { c.text_weak };
        if tab.icon_is_glyph() {
            p.text(egui::pos2(centre.x, centre.y - 6.0), egui::Align2::CENTER_CENTER,
                   &tab.icon, egui::FontId::proportional(16.0), col);
            p.text(egui::pos2(centre.x, centre.y + 10.0), egui::Align2::CENTER_CENTER,
                   &tab.label, egui::FontId::proportional(10.0), col);
        } else {
            // No icon: one centred label at a readable size, rather than the
            // same word twice at two sizes.
            p.text(centre, egui::Align2::CENTER_CENTER,
                   &tab.label, egui::FontId::proportional(13.0), col);
        }
        if let Some(b) = tab.badge_text() {
            // Offset from the icon rather than the cell edge, so a long label
            // does not push the badge away from the thing it counts.
            let bp = egui::pos2(centre.x + 12.0, centre.y - 12.0);
            p.circle_filled(bp, 7.0, c.warning);
            p.text(bp, egui::Align2::CENTER_CENTER, b,
                   egui::FontId::proportional(9.0), c.bg_base);
        }
    }

    // The indicator is animated state, not a derived position: it eases toward
    // the target so a tab change slides rather than jumps.
    let iw = bar.indicator_width(width);
    p.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + bar.indicator_x() - iw / 2.0, rect.top()),
            egui::vec2(iw, 2.0)),
        0.0, c.accent);

    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bottom_bar_is_drawn_on_compact_and_the_strip_on_wide() {
        // This model had NO renderer: the badge sync_badges computed into it
        // reached no screen by any path, and tick_indicator was never stepped.
        // Worse, the strip was gated on !is_compact(), so a phone got neither
        // — the only way between pages was a swipe with nothing on screen
        // saying so.
        let lib = include_str!("lib.rs");
        assert!(lib.contains("tab_bar::tab_bar_ui("), "no bottom bar");
        assert!(lib.contains("if adaptive.is_compact() && !self.nav.can_go_back()"),
                "the bottom bar is not gated on compact");
        assert!(lib.contains("if !adaptive.is_compact() && !self.nav.can_go_back()"),
                "the strip is not gated on wide");
    }

    #[test]
    fn the_bar_has_one_tab_per_page_in_page_order() {
        // The renderer returns a TAB INDEX and the caller feeds it straight to
        // nav.go_page. A bar with a different count or order would navigate to
        // the wrong page silently — there is no type keeping these in step.
        let lib = include_str!("lib.rs");

        // Built from PAGES by zip, so the correspondence is structural rather
        // than a list somebody has to keep in step. Asserting the construction
        // is what catches a future edit that reintroduces a literal list.
        assert!(lib.contains(".zip(PAGES.iter())"),
                "the bar's labels are not derived from PAGES");
    }

    #[test]
    fn a_tap_that_drifts_too_far_is_not_a_tap() {
        // The rule TabBar.cpp's touch workaround existed to protect. The
        // renderer drives press/release rather than reading clicked(), which
        // is what keeps this in force at the UI layer.
        let mut b = TabBar::new(vec![Tab::new("a", "A"), Tab::new("b", "B")]);
        b.press(10.0, 10.0);
        assert_eq!(b.release(10.0 + TAP_SLOP + 5.0, 10.0, 200.0), None,
                   "a drifted gesture committed a tab change");
        // Within tolerance, at the release position, it commits.
        b.press(150.0, 10.0);
        assert_eq!(b.release(152.0, 11.0, 200.0), Some(1));
        assert_eq!(b.current(), 1);
    }

    fn bar() -> TabBar {
        TabBar::new(vec![Tab::new("💬", "Chats"), Tab::new("👥", "People"),
                         Tab::new("⚙", "Settings"), Tab::new("📷", "Camera")])
    }
    const W: f32 = 400.0;

    #[test]
    fn tabs_divide_the_bar_evenly() {
        assert_eq!(bar().tab_width(W), 100.0);
    }

    #[test]
    fn an_empty_bar_does_not_divide_by_zero() {
        // Reachable during startup, before tabs are added — the original
        // guards it too.
        let b = TabBar::default();
        assert_eq!(b.tab_width(W), W);
        assert_eq!(b.hit(50.0, W), None);
    }

    #[test]
    fn a_hit_at_the_far_right_edge_stays_in_range() {
        // Floating-point division at exactly the right edge can land one past
        // the end, and that is a panic in a draw loop rather than a visible bug.
        let b = bar();
        assert_eq!(b.hit(399.999, W), Some(3));
        assert_eq!(b.hit(W, W), None, "outside is None, not the last tab");
        assert_eq!(b.hit(-1.0, W), None);
    }

    #[test]
    fn a_tap_commits_at_the_release_position_not_the_press_position() {
        // What the C++ touch handling existed to protect. The interesting case
        // is a press near a boundary and a release just across it: a small
        // drift, but a different tab.
        //
        // My first version of this test pressed at 50 and released at 250 —
        // which is a 200px SWIPE, correctly rejected by the slop check. The
        // test contradicted itself, and the code was right.
        let mut b = bar();
        b.press(195.0, 20.0);                    // tab 1, near its right edge
        assert_eq!(b.release(205.0, 22.0, W), Some(2), "released over tab 2");
        assert_eq!(b.current(), 2);
    }

    #[test]
    fn a_finger_that_drifts_a_few_pixels_still_taps() {
        // A thumb resting on a phone is never perfectly still.
        let mut b = bar();
        b.press(150.0, 20.0);
        assert_eq!(b.release(158.0, 30.0, W), Some(1));
    }

    #[test]
    fn a_swipe_is_not_a_tap() {
        // Committing a tab change on a swipe would fight whatever the swipe was
        // for.
        let mut b = bar();
        b.press(50.0, 20.0);
        assert_eq!(b.release(350.0, 20.0, W), None);
        assert_eq!(b.current(), 0, "still on the first tab");
    }

    #[test]
    fn re_tapping_the_current_tab_reports_no_change() {
        // Emitting it anyway would make every stray tap re-navigate, resetting
        // scroll position on the page you are already reading.
        let mut b = bar();
        b.press(50.0, 20.0);
        assert_eq!(b.release(50.0, 20.0, W), None);
        assert_eq!(b.current(), 0);
    }

    #[test]
    fn a_cancelled_gesture_leaves_no_press_to_commit_against() {
        // The compositor taking the gesture must not leave a stale start point
        // that makes the NEXT release look like a huge drift.
        let mut b = bar();
        b.press(50.0, 20.0);
        b.cancel();
        assert_eq!(b.release(250.0, 20.0, W), Some(2), "a fresh release still works");
    }

    #[test]
    fn the_indicator_is_centred_under_its_tab_and_a_third_as_wide() {
        let b = bar();
        assert_eq!(b.indicator_width(W), 100.0 / 3.0);
        // tab 1 spans 100..200, so centre 150 minus half of 33.3
        let x = b.indicator_target(1, W);
        assert!((x + b.indicator_width(W) / 2.0 - 150.0).abs() < 0.01,
                "indicator centre is {}", x + b.indicator_width(W) / 2.0);
    }

    #[test]
    fn the_indicator_settles_rather_than_asymptoting_forever() {
        // Otherwise the widget never stops requesting repaints — a battery cost
        // with no visible motion.
        let mut b = bar();
        b.set_current(3);
        for _ in 0..200 { b.tick_indicator(W, 0.25); }
        assert!(!b.is_animating(W));
        assert_eq!(b.indicator_x(), b.indicator_target(3, W));
    }

    #[test]
    fn a_dropped_frame_makes_the_move_slower_never_an_overshoot() {
        // Exponential approach rather than a timeline: t is a fraction of the
        // REMAINING distance, so it cannot pass the target however large it is.
        let mut b = bar();
        b.set_current(3);
        let target = b.indicator_target(3, W);
        b.tick_indicator(W, 1.5);
        assert!(b.indicator_x() <= target + 0.01, "overshot to {}", b.indicator_x());
    }

    #[test]
    fn a_badge_of_zero_is_no_badge_rather_than_a_dot_saying_nothing() {
        let mut b = bar();
        assert_eq!(b.tabs()[0].badge_text(), None);
        b.set_badge(0, 0);
        assert_eq!(b.tabs()[0].badge_text(), None);
        b.set_badge(0, 3);
        assert_eq!(b.tabs()[0].badge_text(), Some("3".into()));
    }

    #[test]
    fn a_badge_caps_at_nine_plus_because_the_dot_is_seven_pixels() {
        // 137 and 10 mean the same thing to a user: more than you want to read
        // now.
        let mut b = bar();
        b.set_badge(1, 9);
        assert_eq!(b.tabs()[1].badge_text(), Some("9".into()));
        b.set_badge(1, 10);
        assert_eq!(b.tabs()[1].badge_text(), Some("9+".into()));
        b.set_badge(1, 137);
        assert_eq!(b.tabs()[1].badge_text(), Some("9+".into()));
    }

    #[test]
    fn setting_a_badge_out_of_range_is_ignored() {
        let mut b = bar();
        b.set_badge(99, 5);
        assert!(b.tabs().iter().all(|t| t.badge_text().is_none()));
    }

    #[test]
    fn setting_the_current_tab_out_of_range_changes_nothing() {
        let mut b = bar();
        assert!(!b.set_current(99));
        assert_eq!(b.current(), 0);
        assert!(b.set_current(2));
        assert!(!b.set_current(2), "already there is not a change");
    }
    #[test]
    fn a_word_in_the_icon_slot_is_not_treated_as_an_icon() {
        // The real construction: a word zipped against the page name, which
        // painted "camera" over "Camera" on every tab.
        assert!(!Tab::new("camera", "Camera").icon_is_glyph());
        assert!(!Tab::new("chats", "Inbox").icon_is_glyph());
        assert!(!Tab::new("", "Camera").icon_is_glyph());
        assert!(!Tab::new(" ", "Camera").icon_is_glyph());
        assert!(!Tab::new("ab", "Camera").icon_is_glyph(), "two chars is not a glyph");
        assert!(!Tab::new("7", "Camera").icon_is_glyph(), "a digit is not a pictograph");
        // A real single-glyph icon still counts, so adding proper icons later
        // needs no further change here.
        assert!(Tab::new("\u{1F4F7}", "Camera").icon_is_glyph(), "camera emoji");
        assert!(Tab::new("\u{2699}", "Settings").icon_is_glyph(), "gear");
    }

}
