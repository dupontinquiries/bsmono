//! product_silhouette — `ProductSilhouette.qml`, ported.
//!
//! A small badge drawing a stylised outline of the product, keyed off a shape
//! profile.
//!
//! ## The QML records a revision, and the revision is the design
//!
//! Its header says the previous version used a 2px stroke and a wedge-reveal
//! clip, and that real-user feedback called it *"heavy and gimmicky"*. What
//! replaced it: **1.0–1.2px strokes**, a plain opacity ramp instead of the
//! inking wedge, and *"more disciplined geometry"* — B&O industrial drawings
//! being thin-lined and almost architectural.
//!
//! That history matters to a port. Reaching for a wedge reveal or a heavier
//! stroke because either looks better in isolation would undo a decision
//! somebody already made with users in front of them.
//!
//! ## The halo is a marker, not a flourish
//!
//! It blooms on a shape change and is explicitly *"half the previous intensity,
//! half the duration on the visible side"* — quick up, slow down. Modelled as a
//! two-phase envelope rather than one curve, because the asymmetry is the
//! effect: a symmetric pulse reads as a throb.

/// Motion tokens from `Bo.qml`, at `motionScale` 1.0.
pub const QUICK_MS: f32 = 180.0;
pub const SLOW_MS: f32 = 440.0;
pub const GRAND_MS: f32 = 640.0;

/// Which outline to draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape { Disc, Bar, Tower, Cube }

impl Shape {
    /// Parse a profile string. Unknown profiles fall back to `Disc`, which is
    /// the QML's default property value — a badge drawing nothing would read as
    /// a broken image rather than an unknown product.
    pub fn parse(s: &str) -> Shape {
        match s {
            "bar" => Shape::Bar,
            "tower" => Shape::Tower,
            "cube" => Shape::Cube,
            _ => Shape::Disc,
        }
    }

    /// Stroke weights this shape draws with, heaviest first.
    ///
    /// Kept as data rather than scattered through a paint function so the
    /// "1.0–1.2px, never 2px" rule is checkable — the tests assert it, which is
    /// how a future edit that thickens a line gets caught here rather than in
    /// another round of user feedback.
    pub fn strokes(self) -> &'static [f32] {
        match self {
            Shape::Disc => &[1.0, 0.7],
            Shape::Bar => &[1.0, 0.6],
            Shape::Tower => &[1.2, 0.7],
            Shape::Cube => &[1.1, 0.6],
        }
    }
}

// ── the outlines ────────────────────────────────────────────────────────────

/// One point of an outline, in a UNIT BOX: 0..1 on both axes, y down.
///
/// Normalised rather than pixels because the badge is drawn at several
/// sizes — the device strip, the now-playing header, the settings row —
/// and an outline authored at one size is wrong at the others by exactly
/// the ratio nobody notices until it ships.
pub type P = (f32, f32);

/// A stroked subpath.
#[derive(Debug, Clone, Copy)]
pub struct SubPath {
    /// Points, already flattened — no curves.
    ///
    /// Flattened in the DATA rather than at paint time. egui's painter has
    /// no quadratic, so a curve would be flattened every frame at whatever
    /// tolerance the renderer felt like; doing it once here makes the
    /// outline a fixed, reviewable list of points and the paint a
    /// transform.
    pub pts: &'static [P],
    /// Join the last point to the first.
    pub closed: bool,
    /// Stroke weight in px at any size. See [`Shape::strokes`] for why this
    /// stays between 1.0 and 1.2.
    pub weight: f32,
}

/// A rounded rectangle as a point list, since the outlines need several.
macro_rules! pts { ($($p:expr),* $(,)?) => { &[$($p),*] } }

/// Beosound A1-ish: a disc seen face on, with the dome and the strap lug.
///
/// The QML's own note is that these are industrial drawings — thin,
/// architectural, and RECOGNISABLE rather than decorative. Two concentric
/// circles were neither: they read as a loading spinner, and the previous
/// version of this file drew exactly that.
const DISC: &[SubPath] = &[
    SubPath { weight: 1.0, closed: true, pts: pts![
        (0.50, 0.09), (0.66, 0.13), (0.80, 0.23), (0.88, 0.37), (0.90, 0.52),
        (0.86, 0.68), (0.75, 0.81), (0.60, 0.89), (0.44, 0.90), (0.29, 0.85),
        (0.17, 0.74), (0.11, 0.60), (0.11, 0.44), (0.17, 0.29), (0.28, 0.17),
        (0.40, 0.11),
    ] },
    // The speaker grille's inner ring, well inside the shell.
    SubPath { weight: 0.7, closed: true, pts: pts![
        (0.50, 0.26), (0.62, 0.30), (0.70, 0.40), (0.72, 0.51), (0.68, 0.62),
        (0.58, 0.70), (0.46, 0.71), (0.35, 0.66), (0.29, 0.56), (0.30, 0.44),
        (0.37, 0.33),
    ] },
    // The leather strap lug at the top — the detail that makes it this
    // product rather than a circle.
    SubPath { weight: 0.7, closed: false, pts: pts![
        (0.42, 0.10), (0.44, 0.03), (0.56, 0.03), (0.58, 0.10),
    ] },
];

/// Beosound Level-ish: a wide slab, standing, with the fabric seam and the
/// fold-out foot.
const BAR: &[SubPath] = &[
    SubPath { weight: 1.0, closed: true, pts: pts![
        (0.08, 0.30), (0.10, 0.26), (0.90, 0.26), (0.92, 0.30),
        (0.92, 0.70), (0.90, 0.74), (0.10, 0.74), (0.08, 0.70),
    ] },
    // The seam between fabric and aluminium, a third of the way down.
    SubPath { weight: 0.6, closed: false, pts: pts![(0.12, 0.40), (0.88, 0.40)] },
    // The foot, which is why it stands at this angle.
    SubPath { weight: 0.6, closed: false, pts: pts![
        (0.30, 0.74), (0.34, 0.84), (0.66, 0.84), (0.70, 0.74),
    ] },
];

/// Beolab-ish: a tall cone on a slim column, seen from the side.
const TOWER: &[SubPath] = &[
    SubPath { weight: 1.2, closed: true, pts: pts![
        (0.38, 0.06), (0.62, 0.06), (0.66, 0.16), (0.66, 0.44),
        (0.60, 0.52), (0.40, 0.52), (0.34, 0.44), (0.34, 0.16),
    ] },
    // The column.
    SubPath { weight: 0.7, closed: false, pts: pts![
        (0.47, 0.52), (0.47, 0.86), (0.53, 0.86), (0.53, 0.52),
    ] },
    // The base disc, in perspective — a flat line would read as a lamp.
    SubPath { weight: 0.7, closed: true, pts: pts![
        (0.50, 0.83), (0.66, 0.86), (0.72, 0.91), (0.66, 0.95),
        (0.50, 0.97), (0.34, 0.95), (0.28, 0.91), (0.34, 0.86),
    ] },
];

/// Beosound Balance-ish: an oak base with a fabric-wrapped cylinder above.
const CUBE: &[SubPath] = &[
    // The cylinder.
    SubPath { weight: 1.1, closed: true, pts: pts![
        (0.26, 0.14), (0.50, 0.08), (0.74, 0.14), (0.74, 0.58),
        (0.50, 0.64), (0.26, 0.58),
    ] },
    // Its top ellipse, which is what makes it a cylinder rather than a box.
    SubPath { weight: 0.6, closed: false, pts: pts![
        (0.26, 0.14), (0.50, 0.20), (0.74, 0.14),
    ] },
    // The oak base, wider than the cylinder — that overhang is the whole
    // silhouette of this product. The first version had it at 0.26..0.74,
    // the same width as the body, and the badge measured 0.48 across: a
    // dot at the size these are actually drawn.
    SubPath { weight: 0.6, closed: true, pts: pts![
        (0.12, 0.66), (0.50, 0.60), (0.88, 0.66), (0.88, 0.84),
        (0.50, 0.92), (0.12, 0.84),
    ] },
];

impl Shape {
    /// The outline, as subpaths in a unit box.
    pub fn outline(self) -> &'static [SubPath] {
        match self {
            Shape::Disc => DISC,
            Shape::Bar => BAR,
            Shape::Tower => TOWER,
            Shape::Cube => CUBE,
        }
    }
}

/// The badge's animation state.
#[derive(Clone, Debug)]
pub struct ProductSilhouette {
    shape: Shape,
    /// 0..=1 opacity ramp on first appearance.
    reveal: f32,
    reveal_ms: f32,
    /// 0..=1 accent bloom, restarted on a shape or accent change.
    halo: f32,
    halo_ms: f32,
    halo_running: bool,
    /// Scales every duration, for the reduced-motion setting.
    motion_scale: f32,
}

impl Default for ProductSilhouette {
    fn default() -> Self {
        ProductSilhouette {
            shape: Shape::Disc, reveal: 0.0, reveal_ms: 0.0,
            halo: 0.0, halo_ms: 0.0, halo_running: false, motion_scale: 1.0,
        }
    }
}

/// `Easing.OutCubic` — the QML's curve for both the reveal and the halo decay.
fn out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

impl ProductSilhouette {
    pub fn new(shape: Shape) -> Self {
        ProductSilhouette { shape, ..Default::default() }
    }

    pub fn shape(&self) -> Shape { self.shape }
    pub fn reveal(&self) -> f32 { self.reveal }
    pub fn halo(&self) -> f32 { self.halo }

    /// Reduced motion. Zero is allowed here, unlike the ambient field: a badge
    /// that appears instantly is a legitimate accessibility outcome, whereas a
    /// frozen drifting background looks like a hang.
    pub fn set_motion_scale(&mut self, s: f32) { self.motion_scale = s.clamp(0.0, 4.0); }

    /// Change the product. Restarts the halo only when the shape actually
    /// differs — re-setting the same product would otherwise flash the badge
    /// every time a list refreshed.
    pub fn set_shape(&mut self, shape: Shape) -> bool {
        if shape == self.shape { return false; }
        self.shape = shape;
        self.bloom();
        true
    }

    /// The accent colour changed; the QML restarts the halo for this too.
    pub fn accent_changed(&mut self) { self.bloom(); }

    fn bloom(&mut self) { self.halo_ms = 0.0; self.halo_running = true; }

    fn dur(&self, base: f32) -> f32 { base * self.motion_scale }

    /// Advance both animations.
    pub fn tick(&mut self, dt_ms: f32) {
        if dt_ms <= 0.0 { return; }
        // A resumed suspend must not leave the badge mid-fade; clamping to a
        // long frame lets it finish rather than jump to an arbitrary point.
        let dt = dt_ms.min(GRAND_MS);

        let reveal_dur = self.dur(SLOW_MS);
        if self.reveal < 1.0 {
            self.reveal_ms += dt;
            self.reveal = if reveal_dur <= 0.0 { 1.0 }
                          else { out_cubic(self.reveal_ms / reveal_dur) };
        }

        if self.halo_running {
            self.halo_ms += dt;
            let up = self.dur(QUICK_MS);
            let down = self.dur(GRAND_MS);
            if up <= 0.0 && down <= 0.0 {
                self.halo = 0.0;
                self.halo_running = false;
            } else if self.halo_ms < up {
                // Linear up, as the QML's NumberAnimation is by default.
                self.halo = self.halo_ms / up;
            } else if self.halo_ms < up + down {
                // OutCubic down — quick up, slow away. The asymmetry IS the
                // effect; a symmetric pulse reads as a throb.
                self.halo = 1.0 - out_cubic((self.halo_ms - up) / down);
            } else {
                self.halo = 0.0;
                self.halo_running = false;
            }
        }
    }

    /// Is anything still animating? Drives whether a repaint is requested.
    pub fn is_animating(&self) -> bool { self.reveal < 1.0 || self.halo_running }
}

#[cfg(test)]
mod outline_tests {
    use super::*;

    const ALL: [Shape; 4] = [Shape::Disc, Shape::Bar, Shape::Tower, Shape::Cube];

    #[test]
    fn every_point_is_inside_the_unit_box() {
        // The badge is drawn at several sizes, so the outline is authored
        // once in 0..1 and scaled. A stray coordinate does not look wrong
        // in the source and paints outside the badge at every size.
        for sh in ALL {
            for sp in sh.outline() {
                for (x, y) in sp.pts {
                    assert!((0.0..=1.0).contains(x) && (0.0..=1.0).contains(y),
                            "{sh:?} has a point outside the box: {x},{y}");
                }
            }
        }
    }

    #[test]
    fn no_stroke_is_heavier_than_the_design_allows() {
        // The module header records why: a 2px stroke was called "heavy and
        // gimmicky" by real users and replaced with 1.0–1.2. This is the
        // rule that stops a later edit undoing that decision, and it now
        // covers the OUTLINES as well as `strokes()`.
        for sh in ALL {
            for sp in sh.outline() {
                assert!(sp.weight >= 0.5 && sp.weight <= 1.2,
                        "{sh:?} draws a {}px line", sp.weight);
            }
            let heaviest = sh.outline().iter()
                .map(|s| s.weight).fold(0.0f32, f32::max);
            assert!((1.0..=1.2).contains(&heaviest),
                    "{sh:?}'s primary line is {heaviest}px, not 1.0–1.2");
        }
    }

    #[test]
    fn each_outline_actually_fills_its_badge() {
        // A recognisable outline is one that uses the space. A shape
        // hugging the middle reads as a dot at badge size, which is what a
        // pair of concentric circles looked like — and passed every test
        // there was, because nothing measured it.
        for sh in ALL {
            let (mut x0, mut y0, mut x1, mut y1) = (1.0f32, 1.0f32, 0.0f32, 0.0f32);
            for sp in sh.outline() {
                for (x, y) in sp.pts {
                    x0 = x0.min(*x); y0 = y0.min(*y);
                    x1 = x1.max(*x); y1 = y1.max(*y);
                }
            }
            // The LONG axis has to use the badge; the short one only has
            // to be visible. My first version demanded 0.6 both ways and
            // the Bar failed at 0.58 — correctly, because a Level really
            // is wide and short. Demanding a square silhouette of every
            // product would have meant distorting the one that is not,
            // which is the opposite of a recognisable outline.
            let (w, h) = (x1 - x0, y1 - y0);
            assert!(w.max(h) > 0.75, "{sh:?} is {w:.2}x{h:.2} — a dot at badge size");
            assert!(w.min(h) > 0.3, "{sh:?} is {w:.2}x{h:.2} — a line, not a shape");
        }
    }

    #[test]
    fn every_subpath_has_enough_points_to_be_a_shape() {
        // Two points is a line and three is a triangle; a closed subpath
        // with fewer than three draws nothing and looks like a rendering
        // bug rather than missing data.
        for sh in ALL {
            for sp in sh.outline() {
                let min = if sp.closed { 3 } else { 2 };
                assert!(sp.pts.len() >= min,
                        "{sh:?} has a {}-point {} subpath",
                        sp.pts.len(), if sp.closed { "closed" } else { "open" });
            }
        }
    }

    #[test]
    fn the_shapes_are_distinguishable_from_each_other() {
        // Four badges that differ only in radius are four badges nobody can
        // tell apart at 24 pixels, which is the size they are drawn at.
        for a in ALL {
            for b in ALL {
                if a == b { continue; }
                let same = a.outline().len() == b.outline().len()
                    && a.outline().iter().zip(b.outline())
                        .all(|(x, y)| x.pts == y.pts);
                assert!(!same, "{a:?} and {b:?} draw the same outline");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_profile_falls_back_to_the_default_shape() {
        // A badge drawing nothing reads as a broken image rather than as an
        // unknown product.
        assert_eq!(Shape::parse("disc"), Shape::Disc);
        assert_eq!(Shape::parse("bar"), Shape::Bar);
        assert_eq!(Shape::parse("wildly-new-product"), Shape::Disc);
        assert_eq!(Shape::parse(""), Shape::Disc);
    }

    #[test]
    fn every_stroke_stays_inside_the_thin_line_rule() {
        // The QML header records that a 2px stroke was called "heavy and
        // gimmicky" by real users and replaced with 1.0-1.2px. Asserting it
        // means a future edit that thickens a line is caught here rather than
        // in another round of feedback.
        for s in [Shape::Disc, Shape::Bar, Shape::Tower, Shape::Cube] {
            for w in s.strokes() {
                assert!(*w >= 0.5 && *w <= 1.2, "{s:?} stroke {w} outside 0.5..=1.2");
            }
        }
    }

    #[test]
    fn the_reveal_is_an_opacity_ramp_that_finishes() {
        let mut p = ProductSilhouette::new(Shape::Disc);
        assert_eq!(p.reveal(), 0.0);
        p.tick(SLOW_MS / 2.0);
        assert!(p.reveal() > 0.0 && p.reveal() < 1.0);
        p.tick(SLOW_MS);
        assert_eq!(p.reveal(), 1.0);
        assert!(!p.is_animating());
    }

    #[test]
    fn the_reveal_decelerates_rather_than_arriving_linearly() {
        // "The icon arrives, it doesn't perform" — OutCubic means most of the
        // opacity lands early and it eases in.
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.tick(SLOW_MS * 0.5);
        assert!(p.reveal() > 0.5, "half the time should be past half the fade: {}", p.reveal());
    }

    #[test]
    fn the_halo_rises_quickly_and_falls_slowly() {
        // The asymmetry is the effect; a symmetric pulse reads as a throb.
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.set_shape(Shape::Bar);
        p.tick(QUICK_MS);
        let peak = p.halo();
        assert!(peak > 0.9, "should be at peak after the quick phase: {peak}");
        p.tick(GRAND_MS / 4.0);
        let early_fall = p.halo();
        assert!(early_fall < peak);
        // OutCubic decay drops most of the way in the first quarter.
        assert!(early_fall < 0.6, "decay is front-loaded: {early_fall}");
    }

    #[test]
    fn the_halo_returns_to_zero_and_stops() {
        // Ticked in frames rather than one enormous delta, because tick()
        // clamps a single step to GRAND_MS — my first version fed 821ms at once
        // and saw the clamp, not the decay. That clamp is deliberate (a resumed
        // suspend must not skip an animation), so the test drives it the way a
        // frame loop does.
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.set_shape(Shape::Tower);
        for _ in 0..80 { p.tick(16.0); }
        assert!(p.halo() < 0.001, "halo settled: {}", p.halo());
        p.tick(16.0);
        assert_eq!(p.halo(), 0.0);
        assert!(!p.is_animating());
    }

    #[test]
    fn re_setting_the_same_shape_does_not_flash_the_badge() {
        // Otherwise every list refresh blooms the halo, which is a flourish
        // where the design says marker.
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.tick(SLOW_MS * 2.0);
        assert!(!p.set_shape(Shape::Disc));
        assert!(!p.is_animating());
        assert!(p.set_shape(Shape::Cube));
        assert!(p.is_animating());
    }

    #[test]
    fn an_accent_change_blooms_the_halo_without_changing_the_shape() {
        let mut p = ProductSilhouette::new(Shape::Bar);
        p.tick(SLOW_MS * 2.0);
        p.accent_changed();
        p.tick(QUICK_MS / 2.0);
        assert!(p.halo() > 0.0);
        assert_eq!(p.shape(), Shape::Bar);
    }

    #[test]
    fn a_shape_change_mid_halo_restarts_it_rather_than_stacking() {
        // Two overlapping blooms would double the intensity, which is exactly
        // the "heavy" the revision removed.
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.set_shape(Shape::Bar);
        p.tick(QUICK_MS);
        p.set_shape(Shape::Tower);
        assert_eq!(p.halo(), 1.0, "still at the previous peak until the next tick");
        p.tick(QUICK_MS / 2.0);
        assert!(p.halo() <= 1.0, "never exceeds one: {}", p.halo());
    }

    #[test]
    fn reduced_motion_is_allowed_to_be_instant_here() {
        // Unlike the ambient field, where zero speed looks like a hang: a badge
        // that appears instantly is a legitimate accessibility outcome.
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.set_motion_scale(0.0);
        p.tick(16.0);
        assert_eq!(p.reveal(), 1.0);
        p.set_shape(Shape::Bar);
        p.tick(16.0);
        assert_eq!(p.halo(), 0.0);
        assert!(!p.is_animating());
    }

    #[test]
    fn a_resumed_suspend_lets_the_animation_finish_rather_than_jumping() {
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.tick(4_000_000.0);
        assert_eq!(p.reveal(), 1.0, "it completed rather than landing somewhere odd");
    }

    #[test]
    fn a_negative_delta_does_not_run_the_animation_backwards() {
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.tick(SLOW_MS / 2.0);
        let mid = p.reveal();
        p.tick(-1000.0);
        assert_eq!(p.reveal(), mid);
    }

    #[test]
    fn halo_never_leaves_the_zero_to_one_range() {
        let mut p = ProductSilhouette::new(Shape::Disc);
        p.set_shape(Shape::Cube);
        for _ in 0..300 {
            p.tick(7.0);
            assert!((0.0..=1.0).contains(&p.halo()), "halo {}", p.halo());
            assert!((0.0..=1.0).contains(&p.reveal()), "reveal {}", p.reveal());
        }
    }
}
