//! The sidebar slider: the pulldown's slider turned on its side, in a strip
//! narrow enough to live at the screen edge permanently.
//!
//! ## Two icons tall, and no taller
//!
//! Collapsed, the strip is [`ICONS_TALL`] icons high. A taller strip would
//! occupy more of the edge without telling the glance anything extra — the
//! value is already legible from the fill, and a longer bar only makes the
//! same number bigger. Press and hold expands it ([`EXPAND_MS`]), which is
//! where the finer control and the second slider live; releasing collapses
//! it again.
//!
//! ## The bar never empties
//!
//! A slider that drains to nothing at zero throws away the one thing the
//! control is for: knowing where the thumb is and what it is. So the fill
//! bottoms out at [`CRESCENT`] of the track — a crescent rather than an
//! empty groove — and the icon lives inside it.
//!
//! ## Expanding covers, it does not shove
//!
//! The strip grows OVER whatever sits above and below it, drawn on top,
//! while the space it reserves in layout stays [`ICONS_TALL`] the whole
//! time ([`Render::layout_icons`] vs [`Render::height_icons`]). Reflowing
//! neighbours on a press-and-hold would animate the whole edge every time a
//! thumb rests on the volume — and an expansion that pushes things around
//! is read as a mistake, whereas one that covers is read as temporary.
//!
//! It also grows SYMMETRICALLY about its own centre, so the point under the
//! finger keeps meaning the same value. Growing downward only would slide
//! the track out from under the finger that asked for it.
//!
//! ## The icon steps aside rather than being covered
//!
//! An icon inside the fill stops being readable once the fill shrinks to
//! about its own size, and a thumb sitting on top of it hides both. So there
//! are two icons in the same design: one INSIDE the crescent and one OUTSIDE
//! it, off to the side, and they cross-fade as the fill shrinks past the
//! point where it can hold one. At any moment exactly one is readable, the
//! sum of their opacities is 1, and the user can see past the thumb.

use crate::Millis;

/// Collapsed height, in icon units.
pub const ICONS_TALL: f32 = 2.0;
/// Expanded height, in icon units.
pub const ICONS_TALL_EXPANDED: f32 = 6.0;
/// Press-and-hold before the strip expands.
pub const EXPAND_MS: Millis = 350;
/// The fill never goes below this fraction of the track.
pub const CRESCENT: f32 = 0.16;
/// Below this fill fraction the inside icon no longer fits.
const ICON_FITS_ABOVE: f32 = 0.30;
/// Fill fraction by which the outside icon is fully faded in.
const ICON_OUTSIDE_BY: f32 = 0.18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

impl Edge {
    /// Which way the outside icon steps. It goes AWAY from the screen edge,
    /// into the screen, because off-edge it would be clipped.
    pub fn outward(self) -> f32 {
        match self {
            Edge::Left => 1.0,
            Edge::Right => -1.0,
        }
    }
}

/// What to draw this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Render {
    /// Fraction of the track that is filled, never below [`CRESCENT`].
    pub fill: f32,
    /// Opacity of the icon drawn inside the fill.
    pub icon_inside: f32,
    /// Opacity of the icon drawn beside the track.
    pub icon_outside: f32,
    /// Height in icon units, animating between collapsed and expanded.
    /// What to DRAW.
    pub height_icons: f32,
    /// What to RESERVE. Constant: the strip never pushes its neighbours.
    pub layout_icons: f32,
    /// True once the hold has expanded the strip.
    pub expanded: bool,
    /// Draw above the neighbours while this is true.
    pub overlay: bool,
}

/// Where the expanded strip actually lands, in pixels, given where its
/// collapsed self sits and how much screen there is.
///
/// Growth is symmetric until an edge is in the way, at which point the strip
/// slides rather than shrinking: a clipped control is worse than one that is
/// not quite centred, and shrinking would change the value under the finger.
pub fn bounds(center_y: f32, height_px: f32, screen_h: f32, margin: f32) -> (f32, f32) {
    let half = height_px / 2.0;
    let (mut top, mut bottom) = (center_y - half, center_y + half);
    if top < margin {
        let d = margin - top;
        top += d;
        bottom += d;
    }
    if bottom > screen_h - margin {
        let d = bottom - (screen_h - margin);
        top -= d;
        bottom -= d;
    }
    // Taller than the screen: fill what there is.
    if top < margin {
        top = margin;
    }
    (top, bottom.max(top))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sidebar {
    pub edge: Edge,
    pub value: f32,
    pressed_at: Option<Millis>,
    expanded: bool,
    height: f32,
}

impl Default for Sidebar {
    fn default() -> Sidebar {
        Sidebar { edge: Edge::Right, value: 0.5, pressed_at: None, expanded: false, height: ICONS_TALL }
    }
}

impl Sidebar {
    pub fn new(edge: Edge, value: f32) -> Sidebar {
        Sidebar { edge, value: value.clamp(0.0, 1.0), ..Default::default() }
    }

    pub fn press(&mut self, now: Millis) {
        self.pressed_at = Some(now);
    }

    /// A drag counts as using the slider, not as holding it: expanding under
    /// a finger that is already adjusting the value would move the track out
    /// from under it.
    pub fn drag_to(&mut self, value: f32, now: Millis) {
        self.value = value.clamp(0.0, 1.0);
        self.pressed_at = Some(now);
    }

    pub fn release(&mut self) {
        self.pressed_at = None;
        self.expanded = false;
    }

    /// Advance. `dt` in milliseconds.
    pub fn tick(&mut self, now: Millis, dt: f32) -> Render {
        if let Some(t) = self.pressed_at {
            if now.saturating_sub(t) >= EXPAND_MS {
                self.expanded = true;
            }
        }
        let target = if self.expanded { ICONS_TALL_EXPANDED } else { ICONS_TALL };
        // Spring-free: a short linear glide, because the strip is small and
        // an overshoot at the screen edge reads as a glitch.
        let step = dt / 180.0 * (ICONS_TALL_EXPANDED - ICONS_TALL);
        if self.height < target {
            self.height = (self.height + step).min(target);
        } else if self.height > target {
            self.height = (self.height - step).max(target);
        }
        self.render()
    }

    pub fn render(&self) -> Render {
        let v = self.value.clamp(0.0, 1.0);
        let fill = CRESCENT + (1.0 - CRESCENT) * v;
        // Cross-fade on the FILL, not on the value: the fill is what has to
        // contain the icon.
        let inside = ((fill - ICON_OUTSIDE_BY) / (ICON_FITS_ABOVE - ICON_OUTSIDE_BY)).clamp(0.0, 1.0);
        Render {
            fill,
            icon_inside: inside,
            icon_outside: 1.0 - inside,
            height_icons: self.height,
            layout_icons: ICONS_TALL,
            expanded: self.expanded,
            overlay: self.height > ICONS_TALL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_leaves_a_crescent_rather_than_an_empty_groove() {
        let s = Sidebar::new(Edge::Right, 0.0);
        let r = s.render();
        assert_eq!(r.fill, CRESCENT);
        assert!(r.fill > 0.0, "the control is still visible and still says what it is");
        // And full is full.
        assert_eq!(Sidebar::new(Edge::Right, 1.0).render().fill, 1.0);
        // Monotonic in between.
        let mut prev = 0.0;
        for i in 0..=20 {
            let f = Sidebar::new(Edge::Right, i as f32 / 20.0).render().fill;
            assert!(f >= prev, "fill went backwards at {i}");
            prev = f;
        }
    }

    #[test]
    fn exactly_one_icon_is_readable_at_any_value() {
        for i in 0..=40 {
            let r = Sidebar::new(Edge::Left, i as f32 / 40.0).render();
            let sum = r.icon_inside + r.icon_outside;
            assert!((sum - 1.0).abs() < 1e-6, "opacities must cross-fade, not add: {sum} at {i}");
            assert!(r.icon_inside >= 0.0 && r.icon_outside >= 0.0);
        }
        // Loud: the icon sits in the fill. Silent: it has stepped outside.
        let loud = Sidebar::new(Edge::Left, 1.0).render();
        assert_eq!(loud.icon_inside, 1.0);
        assert_eq!(loud.icon_outside, 0.0);
        let silent = Sidebar::new(Edge::Left, 0.0).render();
        assert_eq!(silent.icon_outside, 1.0, "at zero the icon is beside the crescent, not under the thumb");
        assert_eq!(silent.icon_inside, 0.0);
    }

    #[test]
    fn the_outside_icon_steps_into_the_screen_not_off_it() {
        assert_eq!(Edge::Left.outward(), 1.0);
        assert_eq!(Edge::Right.outward(), -1.0);
        assert_ne!(Edge::Left.outward(), Edge::Right.outward());
    }

    #[test]
    fn press_and_hold_expands_and_release_collapses() {
        let mut s = Sidebar::new(Edge::Right, 0.5);
        assert_eq!(s.tick(0, 16.0).height_icons, ICONS_TALL, "two icons tall at rest");
        s.press(100);
        assert!(!s.tick(200, 16.0).expanded, "a tap is not a hold");
        assert!(s.tick(100 + EXPAND_MS, 16.0).expanded);
        // It glides rather than jumping.
        let mid = s.tick(600, 16.0).height_icons;
        assert!(mid > ICONS_TALL && mid < ICONS_TALL_EXPANDED, "mid-glide at {mid}");
        for t in 0..40 {
            s.tick(600 + t * 16, 16.0);
        }
        assert_eq!(s.render().height_icons, ICONS_TALL_EXPANDED);
        s.release();
        for t in 0..40 {
            s.tick(2_000 + t * 16, 16.0);
        }
        let r = s.render();
        assert!(!r.expanded);
        assert_eq!(r.height_icons, ICONS_TALL, "back to two icons");
    }

    #[test]
    fn expanding_covers_its_neighbours_instead_of_moving_them() {
        let mut s = Sidebar::new(Edge::Right, 0.5);
        let collapsed = s.tick(0, 16.0);
        assert_eq!(collapsed.layout_icons, ICONS_TALL);
        assert!(!collapsed.overlay, "at rest it is just part of the strip");
        s.press(0);
        s.tick(EXPAND_MS, 16.0);
        for t in 0..40 {
            s.tick(EXPAND_MS + t * 16, 16.0);
        }
        let open = s.render();
        assert_eq!(open.height_icons, ICONS_TALL_EXPANDED, "draws tall");
        assert_eq!(open.layout_icons, ICONS_TALL, "reserves the same as before, so nothing shifts");
        assert!(open.overlay, "and is drawn over the neighbours");
    }

    #[test]
    fn it_grows_from_its_centre_and_slides_only_at_an_edge() {
        // Symmetric growth keeps the value under the finger put.
        let (t, b) = bounds(500.0, 240.0, 1000.0, 8.0);
        assert_eq!((t, b), (380.0, 620.0));
        assert!((((t + b) / 2.0) - 500.0).abs() < 1e-6, "still centred on the collapsed position");
        // Near the top it slides down rather than being clipped, and keeps
        // its full height so the value mapping does not change.
        let (t, b) = bounds(40.0, 240.0, 1000.0, 8.0);
        assert_eq!(t, 8.0);
        assert!((b - t - 240.0).abs() < 1e-6, "same height, moved");
        // Near the bottom, the other way.
        let (t, b) = bounds(960.0, 240.0, 1000.0, 8.0);
        assert_eq!(b, 992.0);
        assert!((b - t - 240.0).abs() < 1e-6);
        // Taller than the screen: fill what there is rather than going off
        // both ends.
        let (t, b) = bounds(500.0, 2_000.0, 400.0, 8.0);
        assert!(t >= 8.0 && b <= 400.0 - 8.0 + 1e-6, "clamped to {t}..{b}");
    }

    #[test]
    fn dragging_does_not_expand_under_the_finger() {
        // Expanding mid-adjustment would move the track out from under a
        // finger that is already setting a value.
        let mut s = Sidebar::new(Edge::Right, 0.5);
        s.press(0);
        for t in 1..20 {
            s.drag_to(0.5 + t as f32 * 0.02, t * 50);
            assert!(!s.tick(t * 50, 16.0).expanded, "still adjusting at {t}");
        }
        assert!(s.value > 0.8);
        // Stop moving and keep holding: now it expands.
        assert!(s.tick(19 * 50 + EXPAND_MS, 16.0).expanded);
    }
}
