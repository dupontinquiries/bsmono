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
        let touch = ctx.input(|i| i.any_touches() || i.pointer.has_pointer() && i.pointer.is_moving() && has_touch_hint(i));
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
        style.spacing.item_spacing = egui::vec2(self.gutter, self.gutter);
        if self.touch {
            // touch scrolling likes a touch more slop before a drag is a drag
            style.interaction.resize_grab_radius_side = style.interaction.resize_grab_radius_side.max(10.0);
        }
        ctx.set_global_style(style);
    }
}

/// Heuristic: some backends don't surface touch as `any_touches`; treat a coarse pointer
/// with no hover as a touch hint. Kept tiny and conservative.
fn has_touch_hint(i: &egui::InputState) -> bool {
    !i.pointer.has_pointer() || i.any_touches()
}

#[cfg(test)]
mod tests {
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
