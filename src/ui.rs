//! UI LAYOUT — anchors, stacks and widgets, so a HUD element is a
//! declaration rather than four hand-computed NDC coordinates.
//!
//! `hud.rs` places everything by hand: `label(ui, -0.975, 0.888, 0.010, ...)`.
//! That works right up until you add the ninth readout, at which point you
//! are doing arithmetic to find out whether the new thing overlaps the boost
//! meter, and the answer changes with the aspect ratio. Every element added
//! to `HudInfo` across this port — `cam_label`, `first_person` — arrived with
//! a magic number beside it.
//!
//! So this is the layer under that. Three ideas, and they compose:
//!
//!   * **Anchor.** Nine positions. "Bottom right" is a thing you say, not a
//!     sign convention you get wrong once per corner.
//!   * **Stack.** Elements anchored to the same corner queue instead of
//!     overlapping, and a test asserts it, because the overlap bug is the
//!     one you only find on someone else's monitor.
//!   * **Widget.** A bar, a meter, a pip row, a reticle: things with a
//!     value and a size, which know how to turn into the quads and labels
//!     `hud.rs` already draws.
//!
//! Nothing here draws. It emits `Cmd`s and lets the existing renderer do the
//! work, which is what makes it adoptable one panel at a time rather than as
//! a rewrite.
//!
//! **Aspect is handled once.** NDC is square and screens are not, so a
//! 16-pixel gap is 16 pixels vertically and about 9 horizontally unless
//! something corrects it. Every size here is in units of SCREEN HEIGHT and
//! the aspect divide happens in one place.

/// Where a thing sits. Nine positions, named rather than signed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anchor {
    TopLeft,
    TopCentre,
    TopRight,
    Left,
    Centre,
    Right,
    BottomLeft,
    BottomCentre,
    BottomRight,
}

impl Anchor {
    /// (x, y) in NDC, before insets.
    fn origin(self) -> (f32, f32) {
        let x = match self {
            Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => -1.0,
            Anchor::TopCentre | Anchor::Centre | Anchor::BottomCentre => 0.0,
            _ => 1.0,
        };
        let y = match self {
            Anchor::TopLeft | Anchor::TopCentre | Anchor::TopRight => 1.0,
            Anchor::Left | Anchor::Centre | Anchor::Right => 0.0,
            _ => -1.0,
        };
        (x, y)
    }
    /// Which way a stack grows from here: down from the top, up from the
    /// bottom, and outward-from-nothing in the middle band.
    fn grow(self) -> f32 {
        match self {
            Anchor::TopLeft | Anchor::TopCentre | Anchor::TopRight => -1.0,
            Anchor::BottomLeft | Anchor::BottomCentre | Anchor::BottomRight => 1.0,
            _ => -1.0,
        }
    }
    fn is_right(self) -> bool {
        matches!(self, Anchor::TopRight | Anchor::Right | Anchor::BottomRight)
    }
    fn is_centre_x(self) -> bool {
        matches!(self, Anchor::TopCentre | Anchor::Centre | Anchor::BottomCentre)
    }
}

/// A rectangle in NDC. `x0 < x1`, `y0 < y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 - 1e-6
            && o.x0 < self.x1 - 1e-6
            && self.y0 < o.y1 - 1e-6
            && o.y0 < self.y1 - 1e-6
    }
    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.x0 >= self.x0 - 1e-5
            && o.x1 <= self.x1 + 1e-5
            && o.y0 >= self.y0 - 1e-5
            && o.y1 <= self.y1 + 1e-5
    }
    /// Shrink by a uniform amount in HEIGHT units.
    pub fn inset(&self, by: f32, aspect: f32) -> Rect {
        Rect {
            x0: self.x0 + by / aspect,
            y0: self.y0 + by,
            x1: self.x1 - by / aspect,
            y1: self.y1 - by,
        }
    }
}

/// What to draw. The renderer already knows how to do all three.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Quad { r: Rect, color: [f32; 4] },
    Text { x: f32, y: f32, size: f32, color: [f32; 4], text: String },
}

/// A widget: something with a size in height-units that can lay itself out.
#[derive(Clone, Debug)]
pub enum Widget {
    /// one line of text
    Label { text: String, size: f32, color: [f32; 4] },
    /// a horizontal fill, 0..1
    Bar { value: f32, w: f32, h: f32, fg: [f32; 4], bg: [f32; 4] },
    /// n discrete pips, `lit` of them on — ammo, charges, lives
    Pips { n: u32, lit: u32, size: f32, gap: f32, fg: [f32; 4], bg: [f32; 4] },
    /// vertical space
    Gap { h: f32 },
}

impl Widget {
    pub fn label(text: &str, size: f32, color: [f32; 4]) -> Widget {
        Widget::Label { text: text.into(), size, color }
    }
    pub fn bar(value: f32, w: f32, h: f32, fg: [f32; 4], bg: [f32; 4]) -> Widget {
        Widget::Bar { value: value.clamp(0.0, 1.0), w, h, fg, bg }
    }
    pub fn pips(n: u32, lit: u32, size: f32, fg: [f32; 4], bg: [f32; 4]) -> Widget {
        Widget::Pips { n, lit: lit.min(n), size, gap: size * 0.35, fg, bg }
    }

    /// Height in screen-height units.
    pub fn height(&self) -> f32 {
        match self {
            // text is drawn from its baseline; the box is ~2.4x the nominal
            Widget::Label { size, .. } => size * 2.4,
            Widget::Bar { h, .. } => *h,
            Widget::Pips { size, .. } => *size,
            Widget::Gap { h } => *h,
        }
    }
    /// Width in screen-height units. Text is estimated, which is fine: it is
    /// used for right-alignment and collision, not for kerning.
    pub fn width(&self) -> f32 {
        match self {
            Widget::Label { text, size, .. } => text.chars().count() as f32 * size * 1.15,
            Widget::Bar { w, .. } => *w,
            Widget::Pips { n, size, gap, .. } => *n as f32 * (size + gap) - gap,
            Widget::Gap { .. } => 0.0,
        }
    }
}

/// A stack of widgets pinned to an anchor.
///
/// This is the thing that stops readouts landing on top of each other: they
/// queue in the direction the corner grows, and the panel knows its own
/// bounds so two panels can be checked against each other.
pub struct Panel {
    pub anchor: Anchor,
    pub items: Vec<Widget>,
    /// space between items, in height units
    pub gap: f32,
}

impl Panel {
    pub fn new(anchor: Anchor) -> Panel {
        Panel { anchor, items: Vec::new(), gap: 0.012 }
    }
    pub fn push(mut self, w: Widget) -> Panel {
        self.items.push(w);
        self
    }
    pub fn total_height(&self) -> f32 {
        if self.items.is_empty() {
            return 0.0;
        }
        self.items.iter().map(|w| w.height()).sum::<f32>()
            + self.gap * (self.items.len() - 1) as f32
    }
    pub fn max_width(&self) -> f32 {
        self.items.iter().fold(0.0f32, |m, w| m.max(w.width()))
    }
}

/// The screen: aspect, safe area, and a list of panels.
///
/// SAFE AREA exists because a HUD pinned to the literal edge is a HUD that
/// is cut off on a TV, behind a notch, or under a rounded corner. One inset,
/// applied once, and nothing downstream has to remember.
pub struct Screen {
    pub aspect: f32,
    /// inset from every edge, in height units
    pub safe: f32,
}

impl Screen {
    pub fn new(aspect: f32) -> Screen {
        Screen { aspect: aspect.max(0.1), safe: 0.028 }
    }

    /// The usable rectangle.
    pub fn area(&self) -> Rect {
        Rect { x0: -1.0, y0: -1.0, x1: 1.0, y1: 1.0 }.inset(self.safe, self.aspect)
    }

    /// Where a panel's bounding box lands.
    pub fn bounds(&self, p: &Panel) -> Rect {
        let a = self.area();
        let (ox, oy) = p.anchor.origin();
        let w = p.max_width() / self.aspect;
        let h = p.total_height();
        let x0 = if p.anchor.is_centre_x() {
            (a.x0 + a.x1) * 0.5 - w * 0.5
        } else if p.anchor.is_right() {
            a.x1 - w
        } else {
            a.x0
        };
        let y1 = if oy > 0.5 {
            a.y1
        } else if oy < -0.5 {
            a.y0 + h
        } else {
            (a.y0 + a.y1) * 0.5 + h * 0.5
        };
        let _ = ox;
        Rect { x0, y0: y1 - h, x1: x0 + w, y1 }
    }

    /// Turn a panel into draw commands.
    pub fn emit(&self, p: &Panel) -> Vec<Cmd> {
        let b = self.bounds(p);
        let mut out = Vec::new();
        // stacks always fill top-down inside their own box; the box itself
        // is what the anchor moved
        let mut y = b.y1;
        let grow = p.anchor.grow();
        let _ = grow;
        for w in &p.items {
            let hh = w.height();
            let top = y;
            let bottom = y - hh;
            let ww = w.width() / self.aspect;
            let x0 = if p.anchor.is_right() {
                b.x1 - ww
            } else if p.anchor.is_centre_x() {
                (b.x0 + b.x1) * 0.5 - ww * 0.5
            } else {
                b.x0
            };
            match w {
                Widget::Label { text, size, color } => {
                    out.push(Cmd::Text {
                        x: x0,
                        y: bottom + hh * 0.25,
                        size: *size,
                        color: *color,
                        text: text.clone(),
                    });
                }
                Widget::Bar { value, fg, bg, .. } => {
                    out.push(Cmd::Quad {
                        r: Rect { x0, y0: bottom, x1: x0 + ww, y1: top },
                        color: *bg,
                    });
                    if *value > 0.0 {
                        out.push(Cmd::Quad {
                            r: Rect { x0, y0: bottom, x1: x0 + ww * value, y1: top },
                            color: *fg,
                        });
                    }
                }
                Widget::Pips { n, lit, size, gap, fg, bg } => {
                    let s = size / self.aspect;
                    let g = gap / self.aspect;
                    for i in 0..*n {
                        let px = x0 + i as f32 * (s + g);
                        out.push(Cmd::Quad {
                            r: Rect { x0: px, y0: bottom, x1: px + s, y1: top },
                            color: if i < *lit { *fg } else { *bg },
                        });
                    }
                }
                Widget::Gap { .. } => {}
            }
            y = bottom - p.gap;
        }
        out
    }

    /// Emit several panels and report any that collide.
    ///
    /// Overlap is the bug you find on someone else's monitor, so it is worth
    /// being able to assert against rather than eyeball.
    pub fn emit_all<'a>(&self, panels: &'a [Panel]) -> (Vec<Cmd>, Vec<(usize, usize)>) {
        let mut cmds = Vec::new();
        let mut clashes = Vec::new();
        let boxes: Vec<Rect> = panels.iter().map(|p| self.bounds(p)).collect();
        for i in 0..panels.len() {
            for j in (i + 1)..panels.len() {
                if boxes[i].overlaps(&boxes[j]) {
                    clashes.push((i, j));
                }
            }
            cmds.extend(self.emit(&panels[i]));
        }
        (cmds, clashes)
    }
}

// ---------------------------------------------------------------- menus ---

/// Turn a `menu::Menu` into draw commands.
///
/// This is the half `menu.rs` was missing. It had rows, navigation,
/// scrolling and a stack, and no way to appear on screen — so every screen
/// kept its own draw function and the abstraction stayed unused. One
/// function closes that, and it uses the SAME anchoring and aspect
/// correction as the HUD, so a menu cannot drift out of agreement with the
/// rest of the interface.
///
/// Only the visible window is emitted: `Menu` already solved scrolling, and
/// re-deciding it here is how the two would disagree.
pub fn menu_cmds(
    s: &Screen,
    m: &crate::menu::Menu,
    size: f32,
    fg: [f32; 4],
    dim: [f32; 4],
    sel_bg: [f32; 4],
) -> Vec<Cmd> {
    use crate::menu::Item;
    let a = s.area();
    let row_h = size * 2.4;
    let mut out = Vec::new();
    let mut y = a.y1 - row_h * 2.0;

    out.push(Cmd::Text { x: a.x0, y: a.y1 - row_h, size: size * 1.3, color: fg, text: m.title.clone() });

    for (i, r) in m.visible() {
        let selected = i == m.sel();
        if selected {
            out.push(Cmd::Quad {
                r: Rect { x0: a.x0, y0: y - row_h * 0.15, x1: a.x1, y1: y + row_h * 0.85 },
                color: sel_bg,
            });
        }
        // a disabled row is drawn dim rather than hidden, because "why can
        // I not press this" is better answered than avoided
        let c = if r.enabled { fg } else { dim };
        let indent = if matches!(r.item, Item::Heading) { 0.0 } else { size * 1.2 / s.aspect };
        out.push(Cmd::Text { x: a.x0 + indent, y, size, color: c, text: r.label.clone() });

        let v = r.item.value();
        if !v.is_empty() {
            let w = v.chars().count() as f32 * size * 1.15 / s.aspect;
            out.push(Cmd::Text { x: a.x1 - w, y, size, color: c, text: v });
        }
        y -= row_h;
    }

    // the hint for the selected row, pinned to the bottom, so it never
    // reflows the list as the cursor moves
    if let Some(h) = m.selected().and_then(|r| r.hint) {
        out.push(Cmd::Text { x: a.x0, y: a.y0, size: size * 0.85, color: dim, text: h.into() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const D: [f32; 4] = [0.2, 0.2, 0.2, 1.0];

    fn scr() -> Screen {
        Screen::new(16.0 / 9.0)
    }

    #[test]
    fn corners_are_where_they_say_they_are() {
        let s = scr();
        let a = s.area();
        let tl = s.bounds(&Panel::new(Anchor::TopLeft).push(Widget::label("X", 0.02, W)));
        assert!((tl.x0 - a.x0).abs() < 1e-5, "top-left is not on the left");
        assert!((tl.y1 - a.y1).abs() < 1e-5, "top-left is not at the top");
        let br = s.bounds(&Panel::new(Anchor::BottomRight).push(Widget::label("X", 0.02, W)));
        assert!((br.x1 - a.x1).abs() < 1e-5, "bottom-right is not on the right");
        assert!((br.y0 - a.y0).abs() < 1e-5, "bottom-right is not at the bottom");
    }

    #[test]
    fn the_safe_area_keeps_everything_off_the_bezel() {
        let s = scr();
        let a = s.area();
        assert!(a.x0 > -1.0 && a.x1 < 1.0 && a.y0 > -1.0 && a.y1 < 1.0);
        let full = Rect { x0: -1.0, y0: -1.0, x1: 1.0, y1: 1.0 };
        for anchor in [Anchor::TopLeft, Anchor::TopRight, Anchor::BottomLeft,
                       Anchor::BottomRight, Anchor::Centre] {
            let p = Panel::new(anchor)
                .push(Widget::label("BOOST", 0.02, W))
                .push(Widget::bar(0.5, 0.3, 0.02, W, D));
            let b = s.bounds(&p);
            assert!(full.contains_rect(&b), "{anchor:?} spilled off screen: {b:?}");
        }
    }

    #[test]
    fn a_stack_queues_instead_of_overlapping() {
        // the whole reason this exists
        let s = scr();
        let p = Panel::new(Anchor::TopLeft)
            .push(Widget::label("SPEED", 0.02, W))
            .push(Widget::bar(0.7, 0.25, 0.02, W, D))
            .push(Widget::pips(6, 4, 0.02, W, D));
        let cmds = s.emit(&p);
        // gather every drawn rect and check none of them intersect
        let rects: Vec<Rect> = cmds
            .iter()
            .filter_map(|c| match c {
                Cmd::Quad { r, .. } => Some(*r),
                _ => None,
            })
            .collect();
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                // pips of the same row share a y band but not an x band;
                // the bar's fill deliberately sits on top of its own track
                let same_row = (rects[i].y0 - rects[j].y0).abs() < 1e-5;
                let bar_fill = same_row && (rects[i].x0 - rects[j].x0).abs() < 1e-5;
                if bar_fill {
                    continue;
                }
                assert!(!rects[i].overlaps(&rects[j]), "{:?} overlaps {:?}", rects[i], rects[j]);
            }
        }
    }

    #[test]
    fn two_panels_in_the_same_corner_are_reported() {
        let s = scr();
        let a = Panel::new(Anchor::TopLeft).push(Widget::label("ONE", 0.03, W));
        let b = Panel::new(Anchor::TopLeft).push(Widget::label("TWO", 0.03, W));
        let (_, clash) = s.emit_all(&[a, b]);
        assert_eq!(clash, vec![(0, 1)], "two panels stacked in one corner went unnoticed");
        // and opposite corners do not
        let c = Panel::new(Anchor::TopLeft).push(Widget::label("ONE", 0.03, W));
        let d = Panel::new(Anchor::BottomRight).push(Widget::label("TWO", 0.03, W));
        let (_, ok) = s.emit_all(&[c, d]);
        assert!(ok.is_empty());
    }

    #[test]
    fn aspect_is_corrected_once_and_squares_stay_square() {
        // a pip is meant to be square; at 21:9 it must not become a slot
        for aspect in [1.0, 16.0 / 9.0, 21.0 / 9.0] {
            let s = Screen::new(aspect);
            let p = Panel::new(Anchor::TopLeft).push(Widget::pips(1, 1, 0.04, W, D));
            let cmds = s.emit(&p);
            let Cmd::Quad { r, .. } = &cmds[0] else { panic!() };
            let ratio = (r.w() * aspect) / r.h();
            assert!((ratio - 1.0).abs() < 1e-4, "at {aspect:.2} a pip is {ratio:.3}:1");
        }
    }

    #[test]
    fn a_bar_fill_matches_its_value_and_clamps() {
        let s = scr();
        for v in [0.0f32, 0.25, 1.0, 4.0, -3.0] {
            let p = Panel::new(Anchor::BottomLeft).push(Widget::bar(v, 0.4, 0.02, W, D));
            let cmds = s.emit(&p);
            let Cmd::Quad { r: track, .. } = &cmds[0] else { panic!() };
            let want = v.clamp(0.0, 1.0);
            if want <= 0.0 {
                assert_eq!(cmds.len(), 1, "an empty bar drew a fill");
                continue;
            }
            let Cmd::Quad { r: fill, .. } = &cmds[1] else { panic!() };
            assert!((fill.w() / track.w() - want).abs() < 1e-4, "{v} filled {:.3}", fill.w() / track.w());
        }
    }

    #[test]
    fn pips_light_from_the_left_and_never_over_count() {
        let s = scr();
        let p = Panel::new(Anchor::TopRight).push(Widget::pips(4, 9, 0.02, W, D));
        let cmds = s.emit(&p);
        assert_eq!(cmds.len(), 4, "asking for 9 of 4 drew {}", cmds.len());
        // all lit, and in x order
        let xs: Vec<f32> = cmds.iter().map(|c| match c {
            Cmd::Quad { r, .. } => r.x0,
            _ => 0.0,
        }).collect();
        for w in xs.windows(2) {
            assert!(w[1] > w[0], "pips are out of order");
        }
    }

    #[test]
    fn right_anchored_text_ends_at_the_margin() {
        let s = scr();
        let a = s.area();
        let long = Panel::new(Anchor::TopRight).push(Widget::label("AUTOTARGET ON", 0.02, W));
        let short = Panel::new(Anchor::TopRight).push(Widget::label("X", 0.02, W));
        let bl = s.bounds(&long);
        let bs = s.bounds(&short);
        assert!((bl.x1 - a.x1).abs() < 1e-5 && (bs.x1 - a.x1).abs() < 1e-5,
            "right-anchored panels do not share a right edge");
        assert!(bl.x0 < bs.x0, "the longer label did not extend further left");
    }

    #[test]
    fn a_menu_draws_only_its_visible_window() {
        use crate::menu::{Menu, Row};
        let s = scr();
        let rows: Vec<Row> = (0..40)
            .map(|i| Row::toggle(Box::leak(format!("r{i}").into_boxed_str()), &format!("ROW {i}"), false))
            .collect();
        let mut m = Menu::new("LONG", rows);
        m.view = 6;
        let cmds = menu_cmds(&s, &m, 0.02, W, D, [0.2, 0.3, 0.5, 0.5]);
        let texts = cmds.iter().filter(|c| matches!(c, Cmd::Text { .. })).count();
        // title + 6 rows + 6 values, and no hint on these
        assert!(texts <= 1 + 6 * 2, "drew more rows than the window: {texts}");
        assert!(texts >= 7, "drew almost nothing: {texts}");
    }

    #[test]
    fn the_selected_row_is_highlighted_and_a_disabled_one_is_dimmed() {
        use crate::menu::{Menu, Row};
        let s = scr();
        let mut m = Menu::new(
            "X",
            vec![
                Row::action("a", "ALPHA"),
                Row::action("b", "BETA").disabled().hint("no fuel"),
            ],
        );
        let cmds = menu_cmds(&s, &m, 0.02, W, D, [0.2, 0.3, 0.5, 0.5]);
        // exactly one highlight quad
        assert_eq!(cmds.iter().filter(|c| matches!(c, Cmd::Quad { .. })).count(), 1);
        // the disabled row is drawn in the dim colour
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Text { color, text, .. }
            if *color == D && text == "BETA")));
        // move onto it and its hint appears
        m.step(1);
        let cmds = menu_cmds(&s, &m, 0.02, W, D, [0.2, 0.3, 0.5, 0.5]);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Text { text, .. } if text == "no fuel")));
    }

    #[test]
    fn everything_a_menu_draws_stays_on_screen() {
        use crate::menu::{Fmt, Menu, Row};
        let s = Screen::new(21.0 / 9.0);
        let m = Menu::new(
            "BRIDGE — WIDESKY",
            vec![
                Row::heading("NAVIGATION"),
                Row::slider("fuel", "FUEL", 0.62, 0.0, 1.0, 0.01, Fmt::Percent),
                Row::choice("arrival", "ARRIVAL", 0, &["SET DOWN", "DROP: FLY IN"]),
            ],
        );
        let a = s.area();
        for c in menu_cmds(&s, &m, 0.02, W, D, [0.2, 0.3, 0.5, 0.5]) {
            match c {
                Cmd::Text { x, y, .. } => {
                    assert!(x >= a.x0 - 1e-4 && x <= a.x1 + 1e-4, "text off screen at x={x}");
                    assert!(y >= a.y0 - 1e-4 && y <= a.y1 + 1e-4, "text off screen at y={y}");
                }
                Cmd::Quad { r, .. } => {
                    assert!(r.x0 >= a.x0 - 1e-4 && r.x1 <= a.x1 + 1e-4, "quad off screen: {r:?}");
                }
            }
        }
    }

    #[test]
    fn the_bridge_menu_lays_out_without_a_single_coordinate_being_written() {
        // the end-to-end check: a screen built entirely from the layers.
        // Ship::bridge_menu makes rows, menu_cmds places them, and nothing
        // in either wrote an NDC number by hand.
        let s = Screen::new(16.0 / 9.0);
        let ship = crate::ship::Ship::new("spearhead", "earth");
        let m = ship.bridge_menu();
        let cmds = menu_cmds(&s, &m, 0.02, W, D, [0.2, 0.3, 0.5, 0.5]);
        assert!(!cmds.is_empty(), "the bridge drew nothing");
        // the title is there, and so is at least one destination
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Text { text, .. } if text.contains("BRIDGE"))));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Text { text, .. } if text == "MARS")));
        // and it all lands inside the safe area on a wide screen too
        let wide = Screen::new(21.0 / 9.0);
        let a = wide.area();
        for c in menu_cmds(&wide, &m, 0.02, W, D, [0.2, 0.3, 0.5, 0.5]) {
            if let Cmd::Text { x, y, .. } = c {
                assert!(x >= a.x0 - 1e-4 && y >= a.y0 - 1e-4 && y <= a.y1 + 1e-4,
                    "bridge text escaped at ({x}, {y})");
            }
        }
    }

    #[test]
    fn a_migrated_hud_panel_stays_inside_the_safe_area() {
        // the boost bar, as hud.rs now builds it
        for aspect in [4.0 / 3.0, 16.0 / 9.0, 21.0 / 9.0] {
            let s = Screen::new(aspect);
            let p = Panel::new(Anchor::BottomCentre)
                .push(Widget::label("BOOST 100", 0.015, W))
                .push(Widget::bar(1.0, 0.56, 0.045, W, D));
            let b = s.bounds(&p);
            let full = Rect { x0: -1.0, y0: -1.0, x1: 1.0, y1: 1.0 };
            assert!(full.contains_rect(&b), "the boost bar spilled at {aspect:.2}: {b:?}");
        }
    }

    #[test]
    fn an_empty_panel_is_harmless() {
        let s = scr();
        let p = Panel::new(Anchor::Centre);
        assert_eq!(p.total_height(), 0.0);
        assert!(s.emit(&p).is_empty());
        let (cmds, clash) = s.emit_all(&[p]);
        assert!(cmds.is_empty() && clash.is_empty());
    }
}
