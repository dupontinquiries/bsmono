//! touch — phone-sized operation, and the two ways a session can be watched.
//!
//! Both source editors assume a mouse and a big screen. On a phone the useful
//! set is small and has to be unambiguous, because a misread gesture in an
//! editor destroys work:
//!
//! * **one finger drags** — scrub or move the thing under it, never zoom.
//! * **two fingers pinch** — zoom about the midpoint, so the frame between your
//!   fingers stays put; the same rule the desktop wheel-zoom follows.
//! * **two fingers slide** — pan; a pinch that is mostly translation should not
//!   silently rescale the timeline.
//! * **long press** — the context action, because there is no right button.
//! * **double tap** — zoom to fit the thing tapped, the fastest way out of a
//!   lost viewport.
//!
//! Telling pinch from pan is the whole problem: fingers never move purely. The
//! recognizer below compares the CHANGE IN SPREAD against the CHANGE IN
//! MIDPOINT and requires one to clearly dominate before committing, then keeps
//! that decision for the rest of the gesture. Committing once and holding is
//! what stops a timeline from jittering between zooming and panning mid-drag.

use egui::Pos2;
use studio_core::Ticks;

/// How a tool window should present itself on this screen.
///
/// The editor's own rule (see `Panel`): on a narrow screen, one thing at a
/// time on a solid background -- floating windows over a timeline are
/// unreadable on a six-inch screen and impossible to hit. Tool windows follow
/// it: on a narrow screen they become a full-screen SHEET; wherever a finger
/// has touched the screen, every control grows to a fingertip-sized target;
/// grids reflow to the columns that fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolLayout {
    /// Full-screen sheet instead of a floating window.
    pub sheet: bool,
    /// Minimum height of anything tappable, in points.
    pub target: f32,
    /// Side-by-side columns for paired content (the comic's two panels).
    pub pair_side_by_side: bool,
}

/// Screens narrower than this, in points, get sheets.
pub const SHEET_BELOW_PT: f32 = 600.0;
/// A fingertip target: ~9 mm, the common 44 pt guideline.
pub const TOUCH_TARGET_PT: f32 = 44.0;
/// egui's own default interact height, for mouse use.
pub const MOUSE_TARGET_PT: f32 = 18.0;

impl ToolLayout {
    /// `touch_seen`: a finger has touched this screen this session (a phone,
    /// a touch laptop) -- targets grow even on a wide screen.
    pub fn for_screen(width_pt: f32, touch_seen: bool) -> ToolLayout {
        let narrow = width_pt < SHEET_BELOW_PT;
        ToolLayout {
            sheet: narrow,
            target: if touch_seen || narrow { TOUCH_TARGET_PT } else { MOUSE_TARGET_PT },
            pair_side_by_side: width_pt >= 520.0,
        }
    }

    /// How many thumbnails of `thumb_pt` width fit across `width_pt`, at least one.
    pub fn columns(width_pt: f32, thumb_pt: f32) -> usize {
        ((width_pt / (thumb_pt + 8.0)).floor() as usize).max(1)
    }

    /// Grow every control in `ui` to this layout's target size.
    pub fn apply(&self, ui: &mut egui::Ui) {
        let sp = ui.spacing_mut();
        sp.interact_size.y = sp.interact_size.y.max(self.target);
        if self.target >= TOUCH_TARGET_PT {
            sp.button_padding = egui::vec2(sp.button_padding.x.max(12.0), sp.button_padding.y.max(10.0));
            sp.item_spacing = egui::vec2(sp.item_spacing.x.max(10.0), sp.item_spacing.y.max(10.0));
            sp.slider_width = sp.slider_width.max(200.0);
        }
    }
}

/// What the recognizer decided this gesture is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TouchKind {
    #[default]
    Undecided,
    Drag,
    Pinch,
    Pan,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchState {
    pub kind: TouchKind,
    start_spread: f32,
    start_mid: Pos2,
    last_mid: Pos2,
    last_spread: f32,
    /// Set once a two-finger gesture commits, so it cannot flip afterwards.
    locked: bool,
}

impl Default for TouchState {
    fn default() -> Self {
        TouchState { kind: TouchKind::Undecided, start_spread: 0.0,
                     start_mid: Pos2::ZERO, last_mid: Pos2::ZERO,
                     last_spread: 0.0, locked: false }
    }
}

/// How much one axis must beat the other before a two-finger gesture commits.
/// 1.6 was picked to be forgiving of a slightly diagonal pinch while still
/// refusing to rescale when someone plainly meant to slide.
pub const COMMIT_RATIO: f32 = 1.6;

/// The result of feeding a frame of touch points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TouchUpdate {
    /// Multiply the zoom by this (1.0 = unchanged).
    pub zoom: f32,
    /// Scroll the view by this many pixels (positive = content moves left).
    pub pan_px: f32,
    /// Where to anchor a zoom.
    pub anchor_x: f32,
}

impl TouchState {
    pub fn begin(&mut self, a: Pos2, b: Pos2) {
        self.start_spread = (a - b).length().max(1.0);
        self.last_spread = self.start_spread;
        self.start_mid = a + (b - a) * 0.5;
        self.last_mid = self.start_mid;
        self.kind = TouchKind::Undecided;
        self.locked = false;
    }

    /// Feed the current two-finger positions; returns what the view should do.
    pub fn update(&mut self, a: Pos2, b: Pos2) -> TouchUpdate {
        let spread = (a - b).length().max(1.0);
        let mid = a + (b - a) * 0.5;
        if !self.locked {
            let d_spread = (spread - self.start_spread).abs();
            let d_mid = (mid - self.start_mid).length();
            // Commit only once one clearly dominates AND the gesture has moved
            // enough to mean anything — a few pixels of jitter must not decide.
            if d_spread.max(d_mid) > 12.0 {
                self.kind = if d_spread > d_mid * COMMIT_RATIO {
                    TouchKind::Pinch
                } else if d_mid > d_spread * COMMIT_RATIO {
                    TouchKind::Pan
                } else {
                    TouchKind::Undecided
                };
                self.locked = self.kind != TouchKind::Undecided;
            }
        }
        let out = match self.kind {
            TouchKind::Pinch => TouchUpdate {
                zoom: spread / self.last_spread.max(1.0),
                pan_px: 0.0,
                anchor_x: mid.x,
            },
            TouchKind::Pan => TouchUpdate {
                zoom: 1.0,
                pan_px: self.last_mid.x - mid.x,
                anchor_x: mid.x,
            },
            _ => TouchUpdate { zoom: 1.0, pan_px: 0.0, anchor_x: mid.x },
        };
        self.last_spread = spread;
        self.last_mid = mid;
        out
    }

    pub fn end(&mut self) { *self = TouchState::default(); }
}

/// Apply a touch update to a timeline view.
pub fn apply(view: &mut super::View, u: TouchUpdate, origin_x: f32) {
    if u.zoom != 1.0 {
        view.zoom_about(u.zoom, u.anchor_x, origin_x);
    }
    if u.pan_px != 0.0 {
        let secs = u.pan_px / view.px_per_sec;
        view.scroll = Ticks(view.scroll.0 + (secs * studio_core::TICKS_PER_SECOND as f32) as i64);
        if view.scroll.0 < 0 { view.scroll = Ticks::ZERO; }
    }
}

// ── how a shared session is laid out ────────────────────────────────────────

/// Theater or split — the two ways a collaborative session actually gets used.
///
/// **Theater**: one program view, everyone watching the same frame. This is the
/// review mode — a director and an editor looking at the same cut, where the
/// point is that nobody is seeing something different.
///
/// **Split**: each collaborator gets their own view and their own playhead, so
/// two editors can work opposite ends of a timeline without fighting over the
/// transport. This is divide-and-conquer, and it is the mode a long-form edit
/// spends most of its time in.
///
/// The distinction is not cosmetic: in theater mode the transport is *shared*,
/// so a peer's scrub moves your picture, and in split mode it is local. Getting
/// that backwards is the single most annoying thing a collaborative editor can
/// do, which is why it is a mode and not a preference buried in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Everyone watches one playhead — the session's, not yours.
    Theater,
    /// Everyone keeps their own playhead; peers are shown as markers.
    #[default]
    Split,
}

impl ViewMode {
    /// Which playhead this client should render and follow.
    ///
    /// In theater mode we follow whoever holds the transport (the "host" — the
    /// site currently driving playback), falling back to our own if they have
    /// gone quiet, because a frozen picture is worse than a slightly wrong one.
    pub fn effective_playhead(
        self,
        local: Ticks,
        host: Option<Ticks>,
    ) -> Ticks {
        match self {
            ViewMode::Theater => host.unwrap_or(local),
            ViewMode::Split => local,
        }
    }

    /// Should a local scrub be broadcast as the session's position?
    pub fn scrub_is_shared(self) -> bool { matches!(self, ViewMode::Theater) }

    pub fn label(self) -> &'static str {
        match self { ViewMode::Theater => "Theater", ViewMode::Split => "Split" }
    }
}

/// A phone-first panel stack: on a narrow screen the editor shows ONE panel at
/// a time with a solid background, because translucent floating panels over a
/// timeline are unreadable on a 6-inch screen and impossible to hit accurately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Panel {
    #[default]
    Timeline,
    Rack,
    Mixer,
    Program,
}

impl Panel {
    pub const ALL: [Panel; 4] = [Panel::Timeline, Panel::Rack, Panel::Mixer, Panel::Program];
    pub fn label(self) -> &'static str {
        match self {
            Panel::Timeline => "Timeline",
            Panel::Rack => "Rack",
            Panel::Mixer => "Mixer",
            Panel::Program => "Program",
        }
    }
    /// Compact layouts show one panel; roomy ones can show the timeline plus
    /// one companion. Returning the SET keeps the caller from re-deriving the
    /// rule in three places.
    pub fn visible(self, compact: bool) -> Vec<Panel> {
        if compact { vec![self] }
        else if self == Panel::Timeline { vec![Panel::Timeline] }
        else { vec![Panel::Timeline, self] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::View;

    fn p(x: f32, y: f32) -> Pos2 { Pos2::new(x, y) }

    #[test]
    fn a_clear_pinch_zooms_and_does_not_pan() {
        let mut t = TouchState::default();
        t.begin(p(300.0, 400.0), p(400.0, 400.0));   // 100px apart
        let u = t.update(p(250.0, 400.0), p(450.0, 400.0)); // spread to 200
        assert_eq!(t.kind, TouchKind::Pinch);
        assert!(u.zoom > 1.5, "spreading the fingers zooms in: {}", u.zoom);
        assert_eq!(u.pan_px, 0.0, "a pinch must not also scroll");
        assert!((u.anchor_x - 350.0).abs() < 1.0, "anchored on the midpoint");
    }

    #[test]
    fn a_clear_slide_pans_and_does_not_zoom() {
        let mut t = TouchState::default();
        t.begin(p(300.0, 400.0), p(400.0, 400.0));
        let u = t.update(p(250.0, 400.0), p(350.0, 400.0)); // same spread, moved left
        assert_eq!(t.kind, TouchKind::Pan);
        assert_eq!(u.zoom, 1.0, "a slide must not rescale the timeline");
        assert!((u.pan_px - 50.0).abs() < 1.0, "panned by the midpoint delta: {}", u.pan_px);
    }

    #[test]
    fn jitter_decides_nothing() {
        // A few pixels of noise must not commit the gesture — otherwise a
        // stationary two-finger rest starts zooming on its own.
        let mut t = TouchState::default();
        t.begin(p(300.0, 400.0), p(400.0, 400.0));
        let u = t.update(p(301.0, 401.0), p(402.0, 399.0));
        assert_eq!(t.kind, TouchKind::Undecided);
        assert_eq!(u.zoom, 1.0);
        assert_eq!(u.pan_px, 0.0);
    }

    #[test]
    fn a_committed_gesture_does_not_flip_mid_drag() {
        // The jitter that matters: once you are pinching, drifting sideways
        // must not turn it into a pan and make the timeline lurch.
        let mut t = TouchState::default();
        t.begin(p(300.0, 400.0), p(400.0, 400.0));
        t.update(p(250.0, 400.0), p(450.0, 400.0));
        assert_eq!(t.kind, TouchKind::Pinch);
        for i in 0..10 {
            let d = i as f32 * 20.0;
            t.update(p(250.0 + d, 400.0), p(450.0 + d, 400.0)); // pure translation now
            assert_eq!(t.kind, TouchKind::Pinch, "the decision holds for the gesture");
        }
        t.end();
        assert_eq!(t.kind, TouchKind::Undecided, "and resets for the next one");
    }

    #[test]
    fn applying_touch_updates_moves_the_view_sanely() {
        let mut v = View::default();
        let before = v.px_per_sec;
        apply(&mut v, TouchUpdate { zoom: 2.0, pan_px: 0.0, anchor_x: 400.0 }, 0.0);
        assert!((v.px_per_sec - before * 2.0).abs() < 1e-3);
        // panning left past zero clamps instead of scrolling into negative time
        apply(&mut v, TouchUpdate { zoom: 1.0, pan_px: -100_000.0, anchor_x: 0.0 }, 0.0);
        assert_eq!(v.scroll, Ticks::ZERO, "the timeline does not start before zero");
    }

    #[test]
    fn theater_follows_the_session_and_split_keeps_your_own_playhead() {
        let local = Ticks::from_seconds(10.0);
        let host = Some(Ticks::from_seconds(42.0));
        assert_eq!(ViewMode::Theater.effective_playhead(local, host), Ticks::from_seconds(42.0),
                   "in theater everyone watches the same frame");
        assert_eq!(ViewMode::Split.effective_playhead(local, host), local,
                   "in split you keep your own");
        // if the host goes quiet, theater falls back rather than freezing
        assert_eq!(ViewMode::Theater.effective_playhead(local, None), local);
        // and only theater broadcasts a scrub — getting this backwards is the
        // most annoying thing a collaborative editor can do
        assert!(ViewMode::Theater.scrub_is_shared());
        assert!(!ViewMode::Split.scrub_is_shared());
    }

    #[test]
    fn compact_layouts_show_one_solid_panel() {
        // On a phone, one panel at a time: floating translucent panels over a
        // timeline are unreadable and impossible to hit accurately.
        assert_eq!(Panel::Rack.visible(true), vec![Panel::Rack]);
        assert_eq!(Panel::Timeline.visible(true), vec![Panel::Timeline]);
        // roomy layouts keep the timeline in view alongside a companion
        assert_eq!(Panel::Mixer.visible(false), vec![Panel::Timeline, Panel::Mixer]);
        assert_eq!(Panel::Timeline.visible(false), vec![Panel::Timeline]);
        assert_eq!(Panel::ALL.len(), 4);
    }
}

// ── Pages-mode panel navigation: the over-scroll detent ─────────────────────

/// Sustained over-scroll at a panel's edge advances to the neighbouring
/// panel — snapclone's tuned two-tier gesture (`gesture-kit`), applied to
/// the one navigation problem `responsive_nav`'s Pages mode leaves open on
/// a phone: the picker is the floor (no panel unreachable), this is the
/// flow (the next panel one thumb-drag away, no chrome hunt).
///
/// Decision layer only, deliberately mirroring `responsive_nav`: the SHELL
/// owns the scrollbars, so the shell decides when the inner view is at its
/// limit and feeds deltas here; this type answers "does that motion mean a
/// panel change, and to which one". Feeding it deltas while the inner view
/// still has room to scroll is the caller's bug — call [`PanelNav::still_scrolling`]
/// on those frames instead, so a fresh pull starts a fresh streak.
pub struct PanelNav {
    gesture: gesture_kit::TwoTierGesture,
}

impl Default for PanelNav {
    fn default() -> Self {
        PanelNav {
            // Page size feeds the flick tier's distance test; a phone panel
            // is roughly a screen, and the tuned defaults key off it.
            gesture: gesture_kit::TwoTierGesture::new(
                gesture_kit::GestureConfig::for_page_size(800.0)),
        }
    }
}

impl PanelNav {
    /// An over-scroll sample (the inner view is at its limit — caller's
    /// call). Positive delta = toward the NEXT panel. Returns the panel to
    /// switch to, already clamped: over-scrolling past the last panel is
    /// nothing, not a wrap — a wrap would make "keep pulling" ambiguous
    /// between "further" and "start over", and the detent's whole feel is
    /// that a pull means one definite thing.
    pub fn on_overscroll(&mut self, current: Panel, delta: f32, now_ms: f64)
        -> Option<Panel>
    {
        match self.gesture.on_scroll(delta as f64, now_ms) {
            gesture_kit::Outcome::DetentFired { dir } => {
                let ix = Panel::ALL.iter().position(|p| *p == current)?;
                let next = match dir {
                    gesture_kit::Direction::Next => ix + 1,
                    gesture_kit::Direction::Prev => ix.checked_sub(1)?,
                };
                Panel::ALL.get(next).copied()
            }
            _ => None,
        }
    }

    /// The inner view moved normally this frame — reset the streak so the
    /// next edge-pull starts clean instead of inheriting stale progress.
    pub fn still_scrolling(&mut self) { self.gesture.reset_streak(); }
}

#[cfg(test)]
mod panel_nav_tests {
    use super::*;

    /// Sustained over-scroll fires exactly one advance, clamped at the
    /// ends — the tuned feel imported whole from gesture-kit, with only
    /// the Panel mapping to pin down here.
    #[test]
    fn sustained_overscroll_advances_one_panel_and_clamps() {
        let mut nav = PanelNav::default();
        // Feed within the rate budget (0.75 raw/ms) until the 220 raw
        // threshold: 20 raw every 30ms clears it in ~11 samples.
        let mut fired = None;
        let mut t = 0.0;
        for _ in 0..40 {
            t += 30.0;
            if let Some(p) = nav.on_overscroll(Panel::Timeline, 20.0, t) {
                fired = Some(p);
                break;
            }
        }
        assert_eq!(fired, Some(Panel::Rack), "Timeline's forward neighbour");

        // From the LAST panel, the same pull is nothing — clamp, not wrap.
        let mut nav = PanelNav::default();
        let mut t = 0.0;
        for _ in 0..40 {
            t += 30.0;
            assert_eq!(nav.on_overscroll(Panel::Program, 20.0, t), None,
                       "past the end means nothing, never a wrap");
        }

        // Backward from the FIRST panel likewise.
        let mut nav = PanelNav::default();
        let mut t = 0.0;
        for _ in 0..40 {
            t += 30.0;
            assert_eq!(nav.on_overscroll(Panel::Timeline, -20.0, t), None);
        }
    }

    /// Normal scrolling between edge-pulls resets the streak: progress
    /// must not silently accumulate across separate gestures.
    #[test]
    fn a_reset_streak_starts_from_zero() {
        let mut nav = PanelNav::default();
        let mut t = 0.0;
        // Half a threshold's worth of pull…
        for _ in 0..5 {
            t += 30.0;
            assert_eq!(nav.on_overscroll(Panel::Rack, 20.0, t), None);
        }
        // …then the view scrolls normally (not at its limit)…
        nav.still_scrolling();
        // …and the next pull needs the FULL threshold again: five more
        // part-pulls stay silent where continuation would have fired.
        for _ in 0..5 {
            t += 30.0;
            assert_eq!(nav.on_overscroll(Panel::Rack, 20.0, t), None,
                       "stale progress must not survive a reset");
        }
    }
}

#[cfg(test)]
mod tool_layout_tests {
    use super::*;

    #[test]
    fn a_phone_gets_a_sheet_and_fingertip_targets() {
        let phone = ToolLayout::for_screen(360.0, true);
        assert!(phone.sheet && phone.target >= 44.0 && !phone.pair_side_by_side);
        // a desktop with a mouse: a floating window, mouse-sized controls
        let desk = ToolLayout::for_screen(1280.0, false);
        assert!(!desk.sheet && desk.target < 44.0 && desk.pair_side_by_side);
        // a touch laptop: a window (the screen is wide) but fingertip targets
        let laptop = ToolLayout::for_screen(1280.0, true);
        assert!(!laptop.sheet && laptop.target >= 44.0);
        // narrow but never touched (a small desktop window): still a sheet, still big targets
        assert!(ToolLayout::for_screen(500.0, false).sheet);
    }

    #[test]
    fn grids_reflow_to_what_fits() {
        assert_eq!(ToolLayout::columns(360.0, 144.0), 2);
        assert_eq!(ToolLayout::columns(1280.0, 144.0), 8);
        assert_eq!(ToolLayout::columns(100.0, 144.0), 1, "never zero columns");
    }
}
