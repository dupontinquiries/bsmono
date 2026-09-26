//! Adaptive layer — touch support, device-class detection, and responsive sizing for any
//! egui app, with **good defaults that apps can override**.
//!
//! Design intent (from the portfolio brief): the kit should not dictate layout. It
//! *classifies and informs* — "this is a phone-sized touch surface, here's a sane touch
//! target and UI scale" — and the app *queries* that to make its own decisions. Every
//! default is overridable with a builder method, so an app gets a good FP5/desktop
//! experience for free but can still do something bespoke without fighting the kit.
//!
//! ```no_run
//! # use uikit_egui::adaptive::{Adaptive, DeviceClass};
//! # fn ui(ctx: &egui::Context) {
//! // defaults inferred from the running context…
//! let a = Adaptive::detect(ctx)
//!     // …then override only what this app cares about:
//!     .with_min_touch_target(56.0)
//!     .force_class(DeviceClass::Tablet);
//! a.apply_touch_defaults(ctx);          // opt-in: nudge egui spacing/hit-area
//! let cols = a.columns(ctx, 260.0);     // responsive grid the app lays out itself
//! # let _ = cols;
//! # }
//! ```

use egui::Context;

/// Coarse device class, derived from the logical (points) width of the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    /// Narrow, one-hand, touch-first (e.g. Fairphone 5 in portrait).
    Phone,
    /// Medium; touch or pointer.
    Tablet,
    /// Wide, pointer-first (Asahi/desktop).
    Desktop,
}

impl DeviceClass {
    /// Default breakpoints in logical points: <600 phone, <1024 tablet, else desktop.
    pub fn from_width(points: f32) -> DeviceClass {
        if points < 600.0 {
            DeviceClass::Phone
        } else if points < 1024.0 {
            DeviceClass::Tablet
        } else {
            DeviceClass::Desktop
        }
    }
    pub fn is_compact(self) -> bool {
        matches!(self, DeviceClass::Phone)
    }
}

/// Edge insets in points (notch / status bar / gesture bar). Apps can read these to keep
/// content out of unsafe regions; the kit fills them from platform hints when available
/// and otherwise leaves them zero.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SafeArea {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

/// The adaptive context an app queries. Construct with [`Adaptive::detect`] for good
/// defaults, then chain `with_*` / `force_*` overrides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adaptive {
    /// Device class (overridable — some apps want to force a layout).
    pub class: DeviceClass,
    /// Whether the primary input is touch (drives hit-area defaults).
    pub touch: bool,
    /// Logical UI scale the app may apply on top of egui's pixels_per_point.
    pub scale: f32,
    /// Minimum comfortable interactive size in points (44 on touch, 28 pointer).
    pub min_touch_target: f32,
    /// Base spacing unit in points.
    pub gutter: f32,
    /// Platform-reported safe-area insets.
    pub safe_area: SafeArea,
    /// Logical width/height of the surface in points (cached from detect).
    pub size: (f32, f32),
}

impl Default for Adaptive {
    fn default() -> Self {
        Adaptive {
            class: DeviceClass::Desktop,
            touch: false,
            scale: 1.0,
            min_touch_target: 28.0,
            gutter: 8.0,
            safe_area: SafeArea::default(),
            size: (1280.0, 800.0),
        }
    }
}

impl Adaptive {
    /// Infer sensible defaults from the running egui context: device class from the logical
    /// width, touch-primary from whether any touch input exists this frame, and a UI scale
    /// that bumps phones up slightly for legibility. Everything here is a *default* — see
    /// the `with_*` overrides.
    pub fn detect(ctx: &Context) -> Self {
        let screen = ctx.content_rect();
        let w = screen.width();
        let h = screen.height();
        let class = DeviceClass::from_width(w);
        // STICKY. `any_touches()` is true only while a finger is DOWN, so reading it per
        // frame made every touch-driven layout two layouts: 28pt targets at rest, 44pt the
        // moment a finger landed, 28pt again on lift. On a tablet-width screen (600-1024pt,
        // which a phone in landscape or at 2x scale is) rusty-board's rows grew under the
        // thumb on every tap, pushing the rows below down -- and the release could land on
        // a different key than the press. A device that has been touched once is a touch
        // device for the rest of the session, so the answer is latched in egui memory.
        //
        // (The old expression also OR-ed in `has_pointer && is_moving && has_touch_hint`,
        // and `has_touch_hint` is `!has_pointer || any_touches`, so that arm reduced to
        // `any_touches` again: it never added a case.)
        let touching = ctx.input(|i| i.any_touches());
        let touch = ctx.data_mut(|d| {
            let seen = d.get_temp_mut_or_default::<bool>(egui::Id::new(TOUCH_SEEN));
            *seen |= touching;
            *seen
        });
        // touch surfaces get a 44pt min target (Apple/Material guidance); pointer gets 28.
        let min_touch_target = if touch || class == DeviceClass::Phone { 44.0 } else { 28.0 };
        let scale = match class {
            DeviceClass::Phone => 1.10,
            DeviceClass::Tablet => 1.05,
            DeviceClass::Desktop => 1.0,
        };
        let gutter = if class.is_compact() { 6.0 } else { 8.0 };
        Adaptive {
            class,
            touch: touch || class == DeviceClass::Phone,
            scale,
            min_touch_target,
            gutter,
            safe_area: SafeArea::default(),
            size: (w, h),
        }
    }

    // ── overrides: an app takes the defaults, then changes only what it needs ──

    /// Apply the platform's safe-area insets to a rect.
    ///
    /// `SafeArea` has existed on this struct since it was written, `detect`
    /// fills it with a default, and NOTHING read it — not the kit, not one of
    /// the ten ports. So every app draws under a notch and behind a gesture
    /// bar on the phones this fleet targets.
    ///
    /// The helper lives here rather than in each app for the reason the whole
    /// kit exists: four copies of an inset calculation is four chances to
    /// forget the bottom one, which is the inset a gesture bar actually eats.
    pub fn inset(&self, rect: egui::Rect) -> egui::Rect {
        let s = self.safe_area;
        let r = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + s.left, rect.min.y + s.top),
            egui::pos2(rect.max.x - s.right, rect.max.y - s.bottom),
        );
        // A rect the insets have inverted is worse than an un-inset one: it
        // paints nothing and reads as a blank screen rather than a layout bug.
        if r.width() <= 0.0 || r.height() <= 0.0 { rect } else { r }
    }

    /// Does this surface report any inset at all?
    ///
    /// Worth asking before laying out: a desktop window has none, and code
    /// that branches on it should say so rather than applying four zeros.
    pub fn has_insets(&self) -> bool {
        let s = self.safe_area;
        s.top > 0.0 || s.bottom > 0.0 || s.left > 0.0 || s.right > 0.0
    }

    pub fn force_class(mut self, class: DeviceClass) -> Self {
        self.class = class;
        self
    }
    pub fn with_touch(mut self, touch: bool) -> Self {
        self.touch = touch;
        self
    }
    pub fn with_min_touch_target(mut self, points: f32) -> Self {
        self.min_touch_target = points.max(1.0);
        self
    }
    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale.max(0.1);
        self
    }
    pub fn with_gutter(mut self, points: f32) -> Self {
        self.gutter = points.max(0.0);
        self
    }
    pub fn with_safe_area(mut self, safe_area: SafeArea) -> Self {
        self.safe_area = safe_area;
        self
    }

    // ── queries the app uses to lay itself out (the kit informs, app decides) ──

    /// Pick one of three values by device class — the ergonomic core of "responsive
    /// without the kit dictating layout": `a.pick(phone, tablet, desktop)`.
    pub fn pick<T>(&self, phone: T, tablet: T, desktop: T) -> T {
        match self.class {
            DeviceClass::Phone => phone,
            DeviceClass::Tablet => tablet,
            DeviceClass::Desktop => desktop,
        }
    }

    /// How many columns of `min_col_width` fit in the available content width (after
    /// safe-area), at least one. Apps drive their own grid with this.
    pub fn columns(&self, ctx: &Context, min_col_width: f32) -> usize {
        let avail = (ctx.content_rect().width() - self.safe_area.left - self.safe_area.right).max(0.0);
        ((avail + self.gutter) / (min_col_width + self.gutter)).floor().max(1.0) as usize
    }

    pub fn is_compact(&self) -> bool {
        self.class.is_compact()
    }

    /// Apply *only defaults* to egui's style: comfortable hit-areas and spacing for the
    /// detected class. Idempotent and non-destructive of app choices made afterward — call
    /// this once at startup, then customize `ctx.style_mut()` freely. Returns nothing so
    /// it composes with the app's own styling.
    pub fn apply_touch_defaults(&self, ctx: &Context) {
        let mut style = (*ctx.global_style()).clone();
        // Only widen the interaction size; never shrink what an app already set larger.
        let t = self.min_touch_target;
        style.spacing.interact_size.y = style.spacing.interact_size.y.max(t);
        style.spacing.button_padding.x = style.spacing.button_padding.x.max(self.gutter);
        style.spacing.button_padding.y = style.spacing.button_padding.y.max(self.gutter * 0.75);
        // ITEM SPACING, fixed in two ways.
        //
        // 1. It was ASSIGNED, three lines under a comment promising to
        //    "never shrink what an app already set larger" — which the two
        //    lines above honour with `.max()` and this one silently broke.
        //    An app that had chosen a roomier rhythm lost it.
        // 2. x and y were the SAME number. They are not the same quantity:
        //    x separates controls side by side (a finger needs the room),
        //    y is vertical rhythm between rows of text. Forcing them equal
        //    is why stacked labels sit in gaps that look wrong — a status
        //    line ends up with a finger-sized gap under it.
        //
        // The 0.75 ratio is not invented here; it is the one the line above
        // already uses for `button_padding.y` against `.x`.
        style.spacing.item_spacing.x = style.spacing.item_spacing.x.max(self.gutter);
        style.spacing.item_spacing.y = style.spacing.item_spacing.y.max(self.gutter * 0.75);
        if self.touch {
            // touch scrolling likes a touch more slop before a drag is a drag
            style.interaction.resize_grab_radius_side = style.interaction.resize_grab_radius_side.max(10.0);
        }
        ctx.set_global_style(style);
    }
}

/// egui-memory key for "a touch has been seen in this context" -- see `Adaptive::detect`.
const TOUCH_SEEN: &str = "uikit_egui::adaptive::touch_seen";

#[cfg(test)]
mod tests {
    /// A finger landing must not change the layout, and lifting it must not change it back.
    /// This is the per-frame flip that made rusty-board's rows jump under every tap.
    #[test]
    fn touch_is_latched_not_read_per_frame() {
        use super::Adaptive;
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 340.0)); // tablet width
        let touch = |phase| egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(1),
            phase,
            pos: egui::pos2(100.0, 100.0),
            force: None,
        };
        let mut frame = |events: Vec<egui::Event>| {
            let mut got = None;
            let input = egui::RawInput { events, screen_rect: Some(screen), ..Default::default() };
            let _ = ctx.run(input, |ctx| got = Some(Adaptive::detect(ctx)));
            got.unwrap()
        };
        let before = frame(vec![]);
        assert!(!before.touch, "no touch yet on a tablet-width pointer screen");
        assert_eq!(before.min_touch_target, 28.0);
        let down = frame(vec![touch(egui::TouchPhase::Start)]);
        assert!(down.touch && down.min_touch_target == 44.0, "a touch was not seen");
        let _ = frame(vec![touch(egui::TouchPhase::End)]);
        let after = frame(vec![]);
        assert!(after.touch, "lifting the finger turned touch back off");
        assert_eq!(after.min_touch_target, 44.0, "the targets shrank again after the tap");
    }

    #[test]
    fn insets_were_settable_and_never_applied() {
        // with_safe_area has existed since this struct was written and NOTHING
        // read safe_area — not the kit, not one of the ten ports. The setter
        // was there and the consumer was missing, so every app draws under a
        // notch and behind a gesture bar.
        let a = Adaptive { size: (400.0, 800.0), ..Adaptive::default() }
            .with_safe_area(SafeArea { top: 48.0, bottom: 24.0, left: 0.0, right: 0.0 });
        assert!(a.has_insets());
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 800.0));
        let safe = a.inset(full);
        assert_eq!(safe.min.y, 48.0, "the notch is not avoided");
        assert_eq!(safe.max.y, 776.0, "the gesture bar is not avoided");
        assert_eq!(safe.width(), 400.0, "nothing was inset horizontally");
    }

    #[test]
    fn a_desktop_surface_is_unchanged() {
        // Zero insets must be a no-op rather than a rect rebuild, so an app can
        // call inset unconditionally and get the right answer on both.
        let a = Adaptive { size: (1600.0, 900.0), ..Adaptive::default() };
        assert!(!a.has_insets());
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
        assert_eq!(a.inset(full), full);
    }

    #[test]
    fn insets_larger_than_the_surface_are_refused() {
        // An inverted rect paints nothing, which reads as a blank screen rather
        // than a layout bug — so a bad inset gives back the original.
        let a = Adaptive { size: (200.0, 100.0), ..Adaptive::default() }
            .with_safe_area(SafeArea { top: 90.0, bottom: 90.0, left: 0.0, right: 0.0 });
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        assert_eq!(a.inset(full), full, "an inverted rect was returned");
    }

    use super::*;

    #[test]
    fn class_breakpoints() {
        assert_eq!(DeviceClass::from_width(400.0), DeviceClass::Phone);
        assert_eq!(DeviceClass::from_width(800.0), DeviceClass::Tablet);
        assert_eq!(DeviceClass::from_width(1600.0), DeviceClass::Desktop);
        assert!(DeviceClass::Phone.is_compact() && !DeviceClass::Desktop.is_compact());
    }

    #[test]
    fn overrides_dont_lose_other_defaults() {
        // start from a phone-ish default, override only the touch target
        let base = Adaptive { class: DeviceClass::Phone, touch: true, min_touch_target: 44.0, ..Adaptive::default() };
        let tuned = base.with_min_touch_target(56.0);
        assert_eq!(tuned.min_touch_target, 56.0, "override applied");
        assert_eq!(tuned.class, DeviceClass::Phone, "other fields preserved");
        assert!(tuned.touch, "other fields preserved");
    }

    #[test]
    fn pick_selects_by_class() {
        let a = Adaptive { class: DeviceClass::Tablet, ..Adaptive::default() };
        assert_eq!(a.pick(1, 2, 3), 2);
        let p = Adaptive { class: DeviceClass::Phone, ..Adaptive::default() };
        assert_eq!(p.pick("one-col", "two-col", "sidebar"), "one-col");
    }
}

/// Raise egui's `TextStyle::Small` (9 pt by default) to the readability
/// floor on a compact surface. Every `.small()` label in every app is 9 pt
/// otherwise — the single biggest source of "tiny" findings the layout
/// probe reports on a phone. Call once per frame after the palette is
/// applied; idempotent.
pub fn apply_text_floor(ctx: &egui::Context, adaptive: &Adaptive) {
    if !adaptive.is_compact() { return; }
    let floor = crate::layout_probe::MIN_READABLE_PT;
    let needs = ctx.global_style().text_styles.get(&egui::TextStyle::Small).map(|f| f.size < floor).unwrap_or(false);
    if needs {
        ctx.global_style_mut(|st| {
            if let Some(f) = st.text_styles.get_mut(&egui::TextStyle::Small) { f.size = floor; }
        });
    }
    #[test]
    fn spacing_never_shrinks_what_an_app_already_chose() {
        // The promise three lines above the old assignment, which it broke.
        let ctx = egui::Context::default();
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(20.0, 18.0);
        ctx.set_global_style(style);
        Adaptive::default().with_gutter(8.0).apply_touch_defaults(&ctx);
        let got = ctx.style().spacing.item_spacing;
        assert_eq!(got, egui::vec2(20.0, 18.0), "a roomier app rhythm was overwritten");
    }

    #[test]
    fn vertical_rhythm_is_tighter_than_the_horizontal_gutter() {
        // Different quantities: x separates controls a finger must hit,
        // y is the gap between rows of text. Equal is what made stacked
        // labels sit in finger-sized gaps.
        let ctx = egui::Context::default();
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(0.0, 0.0);
        ctx.set_global_style(style);
        Adaptive::default().with_gutter(8.0).apply_touch_defaults(&ctx);
        let got = ctx.style().spacing.item_spacing;
        assert_eq!(got.x, 8.0);
        assert!(got.y < got.x, "y {} should be tighter than x {}", got.y, got.x);
        assert!((got.y - 6.0).abs() < 1e-6, "0.75 of the gutter, as button_padding uses");
    }

}

