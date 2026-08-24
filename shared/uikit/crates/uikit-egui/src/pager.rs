//! pager — pages as data, layout as a decided plan, swipe as the kit's gesture.
//!
//! The ask (c11 field session): an app declares its pages once with cheap
//! metadata, and the kit decides — centered single page with swipe-to-advance
//! on a phone-sized surface, several pages side by side ("dashboarded") when
//! the window can hold them. The decision is BUFFERED so a resize dribbling
//! across a breakpoint cannot shuffle the UI frame by frame.
//!
//! Three layers, separable and separately tested:
//!
//! 1. **Fit** — [`fit_cols`]: how many pages the width holds. Pure arithmetic
//!    over [`PageMeta::min_width`] and a gutter.
//! 2. **Plan** — [`PlanState::decide`]: adopts a new column count only after
//!    it has held for a dwell time, and growth additionally requires a margin
//!    (6% by default) beyond the bare fit — the two mechanisms that stop
//!    breakpoint flap. This is the "buffered so as not to create mayhem"
//!    part, and it is a pure state machine.
//! 3. **Swipe** — [`swipe_commits`] / [`settle_ms`]: the commit rule and the
//!    settle animation, ported from snapclone-qt's `SwipeNavigator.cpp`,
//!    which documents its own constants: thresholds are FRACTIONS of the
//!    page width, never absolute pixels (`kCommitFraction = 0.22` — "drag
//!    22% of the page to commit whether the screen is 390px or 1440px"), and
//!    the settle duration scales with release velocity:
//!    `clamp(pageW / (|v| * kDeceleration), minMs, maxMs=340)`.
//!
//! The [`Pager`] widget composes the three with [`crate::adaptive`] and a
//! horizontal [`crate::gesture_layer::GestureLayer`]. It draws deliberately
//! little: the app's own closure renders each page; the pager only decides
//! WHICH pages, WHERE, and how a drag moves between them.

use egui::{Rect, Ui, Vec2};

/// One page, declared once. Low-boilerplate on purpose: an id the app
/// matches on, a title for whatever strip the app draws, and the one number
/// the space math needs.
#[derive(Clone, Debug)]
pub struct PageMeta {
    pub id: usize,
    pub title: String,
    /// The narrowest this page is usable at, in points. The fit math packs
    /// columns of this width; the mobile layout centers content to it.
    pub min_width: f32,
}

/// How many whole pages fit across `avail_w`, packing greedily by each
/// page's `min_width` plus `gutter` between columns. Never below 1: a width
/// too small for any page still shows one (scrolled/cramped beats blank).
pub fn fit_cols(avail_w: f32, gutter: f32, metas: &[PageMeta]) -> usize {
    if metas.is_empty() {
        return 1;
    }
    // Pages differ in min_width; the conservative pack asks "how many of the
    // WIDEST page fit", so a plan never promises a column the widest page
    // cannot use. (Per-page packing is a later refinement; conservative
    // cannot overflow.)
    let w = metas.iter().map(|m| m.min_width).fold(0.0_f32, f32::max).max(1.0);
    let mut n = 0usize;
    let mut used = 0.0_f32;
    while used + w <= avail_w && n < metas.len() {
        n += 1;
        used += w + gutter;
    }
    n.max(1)
}

/// The buffered plan decider. Owns nothing but the current column count and
/// the pending candidate; feed it the instantaneous fit every frame.
#[derive(Clone, Copy, Debug)]
pub struct PlanState {
    cols: usize,
    /// (candidate, since_seconds) — a fit that differs from `cols` has to
    /// hold this long before it is adopted.
    pending: Option<(usize, f64)>,
}

/// Growth margin: to go WIDER, the width must fit the new count with this
/// much headroom, so sitting exactly on the boundary (or 1px either side of
/// it, forever, as window managers love to do) never oscillates.
pub const GROW_MARGIN: f32 = 0.06;
/// How long a differing fit must hold before the plan adopts it.
pub const DWELL_SECS: f64 = 0.25;

impl PlanState {
    pub fn new() -> PlanState {
        PlanState { cols: 1, pending: None }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    /// One frame's decision. `fit_plain` is [`fit_cols`] at the true width;
    /// `fit_margined` the same at `width * (1 - GROW_MARGIN)` (the caller
    /// computes both — this keeps the state machine free of geometry).
    /// Shrinking uses the plain fit (a window that no longer holds the pages
    /// must shed a column promptly — after the dwell — margin or not);
    /// growing requires the margined fit to agree.
    pub fn decide(&mut self, fit_plain: usize, fit_margined: usize, now_s: f64) -> usize {
        let candidate = if fit_plain > self.cols {
            // grow only if it still fits with headroom
            if fit_margined > self.cols { fit_margined.min(fit_plain) } else { self.cols }
        } else {
            fit_plain // shrink (or hold)
        };
        if candidate == self.cols {
            self.pending = None;
        } else {
            match self.pending {
                Some((c, since)) if c == candidate => {
                    if now_s - since >= DWELL_SECS {
                        self.cols = candidate;
                        self.pending = None;
                    }
                }
                _ => self.pending = Some((candidate, now_s)),
            }
        }
        self.cols
    }
}

impl Default for PlanState {
    fn default() -> Self {
        Self::new()
    }
}

/// snapclone-qt's commit rule (`SwipeNavigator.cpp`, "ALL thresholds are
/// fractions of m_pageWidth"): a drag commits at 22% of the page width.
pub const COMMIT_FRACTION: f32 = 0.22;

/// Does a released drag commit to the next/previous page? Returns the page
/// delta: -1 (drag right, previous), +1 (drag left, next), 0 (settle back).
pub fn swipe_commits(drag_px: f32, page_w: f32) -> i32 {
    if page_w <= 0.0 {
        return 0;
    }
    if drag_px <= -COMMIT_FRACTION * page_w {
        1
    } else if drag_px >= COMMIT_FRACTION * page_w {
        -1
    } else {
        0
    }
}

/// The settle animation duration, ported formula:
/// `clamp(pageW / (|velocity| * kDeceleration), minMs, maxMs)`. The Qt file
/// documents maxMs = 340 ("gentle settle" for a slow release near the
/// threshold); kDeceleration is not quoted in the comment, so it is a
/// parameter with a default picked to give ~120ms for a brisk 1.5 px/ms
/// flick on a 400pt page — tune on device against the Qt feel.
pub const SETTLE_MIN_MS: f32 = 90.0;
pub const SETTLE_MAX_MS: f32 = 340.0;
pub fn settle_ms(page_w: f32, velocity_px_per_ms: f32, deceleration: f32) -> f32 {
    let v = velocity_px_per_ms.abs().max(1e-3);
    (page_w / (v * deceleration.max(1e-3))).clamp(SETTLE_MIN_MS, SETTLE_MAX_MS)
}
pub const DEFAULT_DECELERATION: f32 = 2.2;

/// The widget. Owns the active page, the plan state, and the in-flight drag.
pub struct Pager {
    pub metas: Vec<PageMeta>,
    pub active: usize,
    plan: PlanState,
    gesture: crate::gesture_layer::GestureLayer,
}

impl Pager {
    pub fn new(metas: Vec<PageMeta>) -> Pager {
        Pager {
            metas,
            active: 0,
            plan: PlanState::new(),
            // Sized properly on the first ui() call; 360 is only the seed.
            gesture: crate::gesture_layer::GestureLayer::horizontal_page(360.0),
        }
    }

    /// The current plan's column count (1 = mobile/swipe).
    pub fn cols(&self) -> usize {
        self.plan.cols()
    }

    /// Which pages a dashboard shows: the active page and its followers,
    /// wrapped, so tab selection still means something at every width.
    pub fn visible(&self, cols: usize) -> Vec<usize> {
        let n = self.metas.len().max(1);
        (0..cols.min(n)).map(|i| (self.active + i) % n).collect()
    }

    /// Lay out and render. `page_ui` draws ONE page's content given its id.
    /// In the 1-column plan the page is centered to its `min_width` (the
    /// mobile appearance from the ask) and a horizontal swipe with the
    /// snapclone commit rule advances `active`. In an N-column plan, N pages
    /// render side by side and the swipe is off — a dashboard pans nothing.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        gutter: f32,
        mut page_ui: impl FnMut(&mut Ui, usize),
    ) {
        let avail = ui.available_rect_before_wrap();
        let now = ui.input(|i| i.time);
        let fit_plain = fit_cols(avail.width(), gutter, &self.metas);
        let fit_margined = fit_cols(avail.width() * (1.0 - GROW_MARGIN), gutter, &self.metas);
        let cols = self.plan.decide(fit_plain, fit_margined, now);

        if cols <= 1 {
            // MOBILE: one page, centered to its min_width, swipeable. The
            // gesture model is the kit's TwoTierGesture — the SAME layer
            // every other app swipes with (the unification half of the ask);
            // its config is retuned to this surface each frame so thresholds
            // stay fractions of the page, per SwipeNavigator's rule. The
            // live drag offsets the page by the model's own accumulator.
            *self.gesture.config_mut() =
                uikit_gesture::GestureConfig::for_page_size(avail.width() as f64);
            let frame = self.gesture.update(ui, avail, true);
            let drag_px = -frame.raw; // model accumulates content-motion; the page moves with the finger
            let m = &self.metas[self.active.min(self.metas.len().saturating_sub(1))];
            let w = m.min_width.min(avail.width());
            let x = avail.min.x + (avail.width() - w) * 0.5 + drag_px;
            let page_rect = Rect::from_min_size(egui::pos2(x, avail.min.y), Vec2::new(w, avail.height()));
            let id = m.id;
            ui.scope_builder(egui::UiBuilder::new().max_rect(page_rect), |ui| page_ui(ui, id));
            if let Some(dir) = frame.page_jump {
                let n = self.metas.len() as i32;
                let delta: i32 = match dir {
                    uikit_gesture::Direction::Next => 1,
                    uikit_gesture::Direction::Prev => -1,
                };
                self.active = ((self.active as i32 + delta).rem_euclid(n)) as usize;
            }
        } else {
            // DASHBOARD: `cols` pages side by side, each in its own column.
            let ids = self.visible(cols);
            let col_w = (avail.width() - gutter * (cols as f32 - 1.0)) / cols as f32;
            for (i, id) in ids.into_iter().enumerate() {
                let x = avail.min.x + i as f32 * (col_w + gutter);
                let r = Rect::from_min_size(egui::pos2(x, avail.min.y), Vec2::new(col_w, avail.height()));
                ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| page_ui(ui, id));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metas(n: usize, w: f32) -> Vec<PageMeta> {
        (0..n).map(|i| PageMeta { id: i, title: format!("p{i}"), min_width: w }).collect()
    }

    #[test]
    fn fit_is_conservative_and_never_zero() {
        let m = metas(7, 320.0);
        assert_eq!(fit_cols(380.0, 12.0, &m), 1, "phone width holds one");
        assert_eq!(fit_cols(1355.0, 12.0, &m), 4, "desktop width from the field screenshot");
        assert_eq!(fit_cols(100.0, 12.0, &m), 1, "too-small still shows one");
        assert_eq!(fit_cols(10_000.0, 12.0, &m), 7, "capped at the page count");
    }

    /// The mayhem test: a width dribbling across the 2-column boundary must
    /// not flap the plan. The margin refuses the grow at the bare boundary;
    /// the dwell refuses anything that has not held for DWELL_SECS.
    #[test]
    fn jitter_at_the_boundary_never_flaps() {
        let m = metas(4, 320.0);
        let g = 12.0;
        let mut st = PlanState::new();
        // settle into 2 columns at a comfortably wide width first
        let wide = 800.0;
        st.decide(fit_cols(wide, g, &m), fit_cols(wide * (1.0 - GROW_MARGIN), g, &m), 0.0);
        st.decide(fit_cols(wide, g, &m), fit_cols(wide * (1.0 - GROW_MARGIN), g, &m), 1.0);
        assert_eq!(st.cols(), 2);
        // now jitter ±4px around the exact 2-col boundary for two seconds
        let boundary = 320.0 * 2.0 + g; // used = w+g per column in the pack
        let mut changes = 0;
        let mut last = st.cols();
        for i in 0..120 {
            let t = 2.0 + i as f64 * (1.0 / 60.0);
            let w = boundary + if i % 2 == 0 { 4.0 } else { -4.0 };
            let c = st.decide(fit_cols(w, g, &m), fit_cols(w * (1.0 - GROW_MARGIN), g, &m), t);
            if c != last {
                changes += 1;
                last = c;
            }
        }
        assert!(changes <= 1, "the plan flapped {changes} times under boundary jitter");
    }

    #[test]
    fn a_held_change_is_adopted_after_the_dwell() {
        let m = metas(4, 320.0);
        let mut st = PlanState::new();
        // wide enough for 3 with margin, held
        let w = 1200.0;
        let fp = fit_cols(w, 12.0, &m);
        let fm = fit_cols(w * (1.0 - GROW_MARGIN), 12.0, &m);
        assert_eq!(st.decide(fp, fm, 0.0), 1, "not yet — dwell");
        assert_eq!(st.decide(fp, fm, 0.1), 1, "still dwelling");
        assert_eq!(st.decide(fp, fm, 0.3), 3, "held past DWELL_SECS: adopted");
        // and a shrink also dwells (no instant collapse on a transient)
        assert_eq!(st.decide(1, 1, 0.31), 3);
        assert_eq!(st.decide(1, 1, 0.7), 1);
    }

    /// snapclone-qt's rule, both directions and the settle-back middle.
    #[test]
    fn the_commit_fraction_is_the_qt_one() {
        let w = 400.0;
        assert_eq!(swipe_commits(-0.22 * w, w), 1, "22% left = next");
        assert_eq!(swipe_commits(0.22 * w, w), -1, "22% right = previous");
        assert_eq!(swipe_commits(-0.21 * w, w), 0, "just under settles back");
        assert_eq!(swipe_commits(0.0, 0.0), 0, "degenerate width commits nothing");
    }

    #[test]
    fn settle_scales_with_velocity_and_clamps_at_the_qt_max() {
        let w = 400.0;
        let slow = settle_ms(w, 0.05, DEFAULT_DECELERATION);
        let fast = settle_ms(w, 3.0, DEFAULT_DECELERATION);
        assert_eq!(slow, SETTLE_MAX_MS, "slow release: the documented 340ms gentle settle");
        assert!(fast < slow && fast >= SETTLE_MIN_MS);
    }

    #[test]
    fn visible_wraps_from_the_active_page() {
        let mut p = Pager::new(metas(5, 320.0));
        p.active = 3;
        assert_eq!(p.visible(3), vec![3, 4, 0]);
        assert_eq!(p.visible(9), vec![3, 4, 0, 1, 2], "capped at the page count");
    }
}
