//! slide_board -- a one-rail-per-thumb board for landscape.
//!
//! ```text
//!   [ ' ][ , ][ . ][ p ][ y ]      [ f ][ g ][ c ][ r ][ l ]   [⇧ ][123]
//!   [ a ][ o ][ e ][ u ][ i ]      [ d ][ h ][ t ][ n ][ s ]   [⌫ ][ ⏎ ]
//!   [ ; ][ q ][ j ][ k ][ x ]      [ b ][ m ][ w ][ v ][ z ]   [ space  ]
//!   |=== left thumb rail ====|     |=== right thumb rail ===|
//! ```
//!
//! Each hand's three letter rows sit above that thumb's RAIL. A thumb that lands on the rail:
//!
//! * **slides sideways** to pick a column (the column lights up, live);
//! * **then moves vertically**, measured from where it came down: no move or a small one is the
//!   HOME row, up past half a row is the TOP row, down past a third of a row is the BOTTOM row.
//!   The pick follows the thumb, so swiping up and back down by different amounts lands on
//!   different letters;
//! * the letter is typed when the thumb LIFTS. A thumb that lifts after leaving the board (or a
//!   cancelled touch) types nothing.
//!
//! A thumb that lands on the letter grid itself types the letter under it when it lifts (the
//! ordinary keyboard behaviour, for discoverability). The two thumbs are independent: each half
//! tracks its own finger, so both can be mid-slide at once.
//!
//! This module is pure state plus a painter. Fingers come in as [`Evt`]s (egui `Touch` events, or
//! the pointer when there is no touch screen), outputs go out as [`Out`]s; the host decides what
//! typing means. Every threshold is a constant below, because they are what the first on-device
//! session will want to change.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Stroke};
use std::collections::HashMap;

/// Columns per hand.
pub const COLS: usize = 5;
/// Letter rows (top, home, bottom).
pub const ROWS: usize = 3;
/// The rail is this many letter-rows tall.
pub const RAIL_ROWS: f32 = 1.4;
/// Up past this many rows (from where the thumb came down) is the top row.
pub const UP_ROWS: f32 = 0.5;
/// Down past this many rows is the bottom row.
pub const DOWN_ROWS: f32 = 0.33;

/// The letter pages. Rows are `[top, home, bottom]`; Dvorak, as on the owner's phone.
const LEFT_LETTERS: [[&str; COLS]; ROWS] = [["'", ",", ".", "p", "y"], ["a", "o", "e", "u", "i"], [";", "q", "j", "k", "x"]];
const RIGHT_LETTERS: [[&str; COLS]; ROWS] = [["f", "g", "c", "r", "l"], ["d", "h", "t", "n", "s"], ["b", "m", "w", "v", "z"]];
const LEFT_SYMBOLS: [[&str; COLS]; ROWS] = [["1", "2", "3", "4", "5"], ["!", "@", "#", "$", "%"], ["`", "~", "-", "_", "="]];
const RIGHT_SYMBOLS: [[&str; COLS]; ROWS] = [["6", "7", "8", "9", "0"], ["^", "&", "*", "(", ")"], ["[", "]", "{", "}", "\\"]];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Half {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Top,
    Home,
    Bottom,
}

impl Row {
    fn index(self) -> usize {
        match self {
            Row::Top => 0,
            Row::Home => 1,
            Row::Bottom => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Letters,
    Symbols,
}

/// The five keys between the hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Shift,
    Page,
    Backspace,
    Enter,
    Space,
}

/// What the board asks of its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Text(String),
    Key(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Start,
    Move,
    End,
    Cancel,
}

/// One finger event, in the board's own coordinates (screen points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Evt {
    pub id: u64,
    pub phase: Phase,
    pub pos: Pos2,
}

/// Where everything is, for one board rectangle. Pure arithmetic.
#[derive(Clone, Copy, Debug)]
pub struct Geom {
    pub rect: Rect,
}

impl Geom {
    pub fn new(rect: Rect) -> Geom {
        Geom { rect }
    }

    fn center_w(&self) -> f32 {
        (self.rect.width() * 0.2).clamp(96.0, 220.0).min(self.rect.width() * 0.4)
    }

    /// One letter row's height; the rail is `RAIL_ROWS` of these under the three rows.
    pub fn cell_h(&self) -> f32 {
        self.rect.height() / (ROWS as f32 + RAIL_ROWS)
    }

    fn half_w(&self) -> f32 {
        (self.rect.width() - self.center_w()) / 2.0
    }

    pub fn half_rect(&self, h: Half) -> Rect {
        let w = self.half_w();
        let x0 = match h {
            Half::Left => self.rect.left(),
            Half::Right => self.rect.right() - w,
        };
        Rect::from_min_size(pos2(x0, self.rect.top()), vec2(w, self.rect.height()))
    }

    pub fn center_rect(&self) -> Rect {
        let w = self.center_w();
        Rect::from_min_size(pos2(self.rect.left() + self.half_w(), self.rect.top()), vec2(w, self.rect.height()))
    }

    /// The letter grid of a half (three rows), and its rail under it.
    pub fn grid_rect(&self, h: Half) -> Rect {
        let r = self.half_rect(h);
        Rect::from_min_size(r.min, vec2(r.width(), self.cell_h() * ROWS as f32))
    }

    pub fn rail_rect(&self, h: Half) -> Rect {
        let r = self.half_rect(h);
        let top = r.top() + self.cell_h() * ROWS as f32;
        Rect::from_min_max(pos2(r.left(), top), r.max)
    }

    pub fn col_of(&self, h: Half, x: f32) -> usize {
        let r = self.half_rect(h);
        (((x - r.left()) / r.width() * COLS as f32).floor().max(0.0) as usize).min(COLS - 1)
    }

    pub fn col_rect(&self, h: Half, col: usize) -> Rect {
        let r = self.half_rect(h);
        let w = r.width() / COLS as f32;
        Rect::from_min_size(pos2(r.left() + w * col as f32, r.top()), vec2(w, r.height()))
    }

    /// The cell of the grid (row, column) as drawn.
    pub fn cell_rect(&self, h: Half, row: Row, col: usize) -> Rect {
        let c = self.col_rect(h, col);
        Rect::from_min_size(pos2(c.left(), c.top() + self.cell_h() * row.index() as f32), vec2(c.width(), self.cell_h()))
    }

    /// The center key's rectangle: shift and symbols on top, backspace and enter under them,
    /// space across the bottom.
    pub fn btn_rect(&self, b: Btn) -> Rect {
        let c = self.center_rect();
        let third = c.height() / 3.0;
        let half = c.width() / 2.0;
        let (col, row, span) = match b {
            Btn::Shift => (0, 0, 1),
            Btn::Page => (1, 0, 1),
            Btn::Backspace => (0, 1, 1),
            Btn::Enter => (1, 1, 1),
            Btn::Space => (0, 2, 2),
        };
        Rect::from_min_size(pos2(c.left() + half * col as f32, c.top() + third * row as f32), vec2(half * span as f32, third))
    }

    pub fn btn_at(&self, p: Pos2) -> Option<Btn> {
        [Btn::Shift, Btn::Page, Btn::Backspace, Btn::Enter, Btn::Space].into_iter().find(|b| self.btn_rect(*b).contains(p))
    }

    pub fn half_at(&self, p: Pos2) -> Option<Half> {
        [Half::Left, Half::Right].into_iter().find(|h| self.half_rect(*h).contains(p))
    }

    /// Which row a thumb that came down at `start` and is now at `now` is pointing at.
    ///
    /// From the rail: relative to where it came down (up = top, down = bottom, near = home).
    /// From the grid: simply the row under the finger.
    pub fn row_for(&self, h: Half, start: Pos2, now: Pos2) -> Row {
        if self.grid_rect(h).contains(start) {
            let r = ((now.y - self.rect.top()) / self.cell_h()).floor();
            return match r {
                r if r < 1.0 => Row::Top,
                r if r < 2.0 => Row::Home,
                _ => Row::Bottom,
            };
        }
        let dy = start.y - now.y; // positive = up
        if dy > UP_ROWS * self.cell_h() {
            Row::Top
        } else if dy < -DOWN_ROWS * self.cell_h() {
            Row::Bottom
        } else {
            Row::Home
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Zone {
    Half(Half),
    Btn(Btn),
    /// Landed outside everything: lifts type nothing.
    Dead,
}

#[derive(Clone, Copy, Debug)]
struct Touch {
    zone: Zone,
    start: Pos2,
    pos: Pos2,
}

/// The live selection of a thumb, for the painter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub half: Half,
    pub col: usize,
    pub row: Row,
}

#[derive(Default)]
pub struct SlideBoard {
    touches: HashMap<u64, Touch>,
    /// One-shot shift: the next letter is a capital, then it is spent.
    pub shift: bool,
    pub symbols: bool,
    /// Set the first time a real touch event arrives: from then on the pointer is the primary
    /// finger's echo and is ignored (the layer host sends both for the first finger).
    touch_seen: bool,
}

impl SlideBoard {
    pub fn page(&self) -> Page {
        if self.symbols {
            Page::Symbols
        } else {
            Page::Letters
        }
    }

    fn table(&self, h: Half) -> &'static [[&'static str; COLS]; ROWS] {
        match (h, self.symbols) {
            (Half::Left, false) => &LEFT_LETTERS,
            (Half::Right, false) => &RIGHT_LETTERS,
            (Half::Left, true) => &LEFT_SYMBOLS,
            (Half::Right, true) => &RIGHT_SYMBOLS,
        }
    }

    /// The face of a key as drawn (capital while shift is armed).
    pub fn face(&self, h: Half, row: Row, col: usize) -> String {
        let s = self.table(h)[row.index()][col];
        if self.shift && !self.symbols {
            shifted(s)
        } else {
            s.to_string()
        }
    }

    /// What every active thumb is pointing at right now.
    pub fn picks(&self, g: &Geom) -> Vec<Pick> {
        self.touches
            .values()
            .filter_map(|t| match t.zone {
                Zone::Half(h) => Some(Pick { half: h, col: g.col_of(h, t.pos.x), row: g.row_for(h, t.start, t.pos) }),
                _ => None,
            })
            .collect()
    }

    pub fn pressed_btn(&self) -> Vec<Btn> {
        self.touches.values().filter_map(|t| if let Zone::Btn(b) = t.zone { Some(b) } else { None }).collect()
    }

    /// Feed one finger event; returns what the host should do now.
    pub fn on_event(&mut self, g: &Geom, e: Evt) -> Vec<Out> {
        match e.phase {
            Phase::Start => {
                let zone = if let Some(h) = g.half_at(e.pos) {
                    Zone::Half(h)
                } else if let Some(b) = g.btn_at(e.pos) {
                    Zone::Btn(b)
                } else {
                    Zone::Dead
                };
                self.touches.insert(e.id, Touch { zone, start: e.pos, pos: e.pos });
                Vec::new()
            }
            Phase::Move => {
                if let Some(t) = self.touches.get_mut(&e.id) {
                    t.pos = e.pos;
                }
                Vec::new()
            }
            Phase::Cancel => {
                self.touches.remove(&e.id);
                Vec::new()
            }
            Phase::End => {
                let Some(mut t) = self.touches.remove(&e.id) else { return Vec::new() };
                t.pos = e.pos;
                match t.zone {
                    Zone::Dead => Vec::new(),
                    Zone::Btn(b) => {
                        // A button fires only if the finger is still on it.
                        if g.btn_rect(b).contains(t.pos) {
                            self.press_btn(b)
                        } else {
                            Vec::new()
                        }
                    }
                    Zone::Half(h) => {
                        // Lifted off the board entirely: not a letter.
                        if !g.rect.expand(g.cell_h()).contains(t.pos) {
                            return Vec::new();
                        }
                        let col = g.col_of(h, t.pos.x);
                        let row = g.row_for(h, t.start, t.pos);
                        let s = self.face(h, row, col);
                        if self.shift {
                            self.shift = false;
                        }
                        vec![Out::Text(s)]
                    }
                }
            }
        }
    }

    fn press_btn(&mut self, b: Btn) -> Vec<Out> {
        match b {
            Btn::Shift => {
                self.shift = !self.shift;
                Vec::new()
            }
            Btn::Page => {
                self.symbols = !self.symbols;
                self.shift = false;
                Vec::new()
            }
            Btn::Backspace => vec![Out::Key("BackSpace")],
            Btn::Enter => vec![Out::Key("Return")],
            Btn::Space => {
                self.shift = false;
                vec![Out::Text(" ".to_string())]
            }
        }
    }

    /// Turn this frame's egui input into finger events, in order.
    pub fn events_from_input(&mut self, i: &egui::InputState) -> Vec<Evt> {
        let mut out = Vec::new();
        for ev in &i.events {
            match ev {
                egui::Event::Touch { id, phase, pos, .. } => {
                    self.touch_seen = true;
                    let phase = match phase {
                        egui::TouchPhase::Start => Phase::Start,
                        egui::TouchPhase::Move => Phase::Move,
                        egui::TouchPhase::End => Phase::End,
                        egui::TouchPhase::Cancel => Phase::Cancel,
                    };
                    out.push(Evt { id: id.0, phase, pos: *pos });
                }
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, .. } if !self.touch_seen => {
                    out.push(Evt { id: u64::MAX, phase: if *pressed { Phase::Start } else { Phase::End }, pos: *pos });
                }
                egui::Event::PointerMoved(pos) if !self.touch_seen => out.push(Evt { id: u64::MAX, phase: Phase::Move, pos: *pos }),
                _ => {}
            }
        }
        out
    }

    /// Read this frame's input, update, paint, and say what to type.
    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, colors: &uikit_egui::PaletteColors) -> Vec<Out> {
        let g = Geom::new(rect);
        let evts = ui.input(|i| self.events_from_input_ref(i));
        let mut outs = Vec::new();
        for e in evts {
            outs.extend(self.on_event(&g, e));
        }
        ui.allocate_rect(rect, egui::Sense::hover());
        self.paint(ui, &g, colors);
        if !self.touches.is_empty() {
            ui.ctx().request_repaint();
        }
        outs
    }

    // `events_from_input` needs `&mut self` (it learns `touch_seen`); `ui.input` hands out a
    // shared borrow of the context, not of self, so this indirection only exists to satisfy the
    // closure signature.
    fn events_from_input_ref(&mut self, i: &egui::InputState) -> Vec<Evt> {
        self.events_from_input(i)
    }

    fn paint(&self, ui: &egui::Ui, g: &Geom, colors: &uikit_egui::PaletteColors) {
        let p = ui.painter_at(g.rect.expand(1.0));
        let picks = self.picks(g);
        let font = FontId::proportional((g.cell_h() * 0.5).clamp(12.0, 28.0));
        let small = FontId::proportional((g.cell_h() * 0.34).clamp(10.0, 18.0));
        for h in [Half::Left, Half::Right] {
            let pick = picks.iter().find(|k| k.half == h);
            for row in [Row::Top, Row::Home, Row::Bottom] {
                for col in 0..COLS {
                    let r = g.cell_rect(h, row, col).shrink(2.0);
                    let hot = pick.is_some_and(|k| k.col == col && k.row == row);
                    let in_col = pick.is_some_and(|k| k.col == col);
                    let fill = if hot { colors.accent } else if in_col { colors.bg_hover } else { colors.bg_raised };
                    p.rect_filled(r, 8.0, fill);
                    let ink = if hot { colors.bg_base } else { colors.text };
                    p.text(r.center(), Align2::CENTER_CENTER, self.face(h, row, col), font.clone(), ink);
                }
            }
            // The rail: a groove with a tick per column and the thumb's dot where it is.
            let rail = g.rail_rect(h).shrink2(vec2(2.0, 4.0));
            p.rect_filled(rail, rail.height() / 2.0, colors.bg_card);
            p.rect_stroke(rail, rail.height() / 2.0, Stroke::new(1.0, colors.border), egui::StrokeKind::Inside);
            for col in 0..COLS {
                let c = g.col_rect(h, col);
                p.circle_filled(pos2(c.center().x, rail.center().y), 2.5, colors.text_weak);
            }
            if let Some(k) = pick {
                let c = g.col_rect(h, k.col);
                p.circle_filled(pos2(c.center().x, rail.center().y), rail.height() * 0.32, colors.accent);
            }
        }
        let down = self.pressed_btn();
        for (b, label) in [(Btn::Shift, "⇧"), (Btn::Page, if self.symbols { "abc" } else { "123" }), (Btn::Backspace, "⌫"), (Btn::Enter, "⏎"), (Btn::Space, "space")] {
            let r = g.btn_rect(b).shrink(2.0);
            let on = (b == Btn::Shift && self.shift) || down.contains(&b);
            p.rect_filled(r, 8.0, if on { colors.accent } else { colors.bg_card });
            let ink: Color32 = if on { colors.bg_base } else { colors.text };
            p.text(r.center(), Align2::CENTER_CENTER, label, if b == Btn::Space { small.clone() } else { font.clone() }, ink);
        }
    }
}

/// A letter's capital; the punctuation of the letter page shifts the way a US keyboard does.
pub fn shifted(s: &str) -> String {
    match s {
        "'" => "\"".into(),
        "," => "<".into(),
        "." => ">".into(),
        ";" => ":".into(),
        _ => s.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Geom {
        Geom::new(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 220.0)))
    }

    fn touch(b: &mut SlideBoard, g: &Geom, id: u64, from: Pos2, to: &[Pos2]) -> Vec<Out> {
        let mut out = b.on_event(g, Evt { id, phase: Phase::Start, pos: from });
        for p in to {
            out.extend(b.on_event(g, Evt { id, phase: Phase::Move, pos: *p }));
        }
        let end = to.last().copied().unwrap_or(from);
        out.extend(b.on_event(g, Evt { id, phase: Phase::End, pos: end }));
        out
    }

    /// A point on the rail of `h`, in column `col`.
    fn rail(g: &Geom, h: Half, col: usize) -> Pos2 {
        let r = g.rail_rect(h);
        pos2(g.col_rect(h, col).center().x, r.top() + r.height() * 0.45)
    }

    fn text(s: &str) -> Vec<Out> {
        vec![Out::Text(s.to_string())]
    }

    #[test]
    fn the_halves_the_centre_and_the_rails_tile_the_board_without_overlap() {
        let g = g();
        let (l, c, r) = (g.half_rect(Half::Left), g.center_rect(), g.half_rect(Half::Right));
        assert!((l.right() - c.left()).abs() < 0.01 && (c.right() - r.left()).abs() < 0.01);
        assert!((l.width() - r.width()).abs() < 0.01);
        assert!((g.rect.left() - l.left()).abs() < 0.01 && (g.rect.right() - r.right()).abs() < 0.01);
        let (grid, rail) = (g.grid_rect(Half::Left), g.rail_rect(Half::Left));
        assert!((grid.bottom() - rail.top()).abs() < 0.01 && (rail.bottom() - g.rect.bottom()).abs() < 0.01);
        assert!((rail.height() / g.cell_h() - RAIL_ROWS).abs() < 0.01, "the rail is the documented height");
    }

    #[test]
    fn a_tap_on_the_rail_types_the_home_row_letter_of_that_column() {
        let (g, mut b) = (g(), SlideBoard::default());
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Left, 0), &[]), text("a"));
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Left, 2), &[]), text("e"));
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Right, 3), &[]), text("n"));
    }

    #[test]
    fn sliding_along_the_rail_picks_the_column_where_the_thumb_lifts() {
        let (g, mut b) = (g(), SlideBoard::default());
        let path: Vec<Pos2> = (0..5).map(|c| rail(&g, Half::Left, c)).collect();
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Left, 0), &path), text("i"), "ended on the last column");
        let back = [rail(&g, Half::Left, 4), rail(&g, Half::Left, 1)];
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Left, 0), &back), text("o"), "and slid back to column 1");
    }

    #[test]
    fn up_then_down_by_different_amounts_lands_on_different_rows() {
        let (g, mut b) = (g(), SlideBoard::default());
        let (c, h) = (2, g.cell_h());
        let start = rail(&g, Half::Left, c);
        let up = |dy: f32| pos2(start.x, start.y - dy);
        assert_eq!(touch(&mut b, &g, 1, start, &[up(0.2 * h)]), text("e"), "a small move is still home");
        assert_eq!(touch(&mut b, &g, 1, start, &[up(0.8 * h)]), text("."), "up past half a row is the top row");
        assert_eq!(touch(&mut b, &g, 1, start, &[up(-0.5 * h)]), text("j"), "down is the bottom row");
        // up, then back down: the pick follows the thumb, the LAST position decides.
        assert_eq!(touch(&mut b, &g, 1, start, &[up(0.8 * h), up(0.1 * h)]), text("e"), "up then back to rest");
        assert_eq!(touch(&mut b, &g, 1, start, &[up(0.8 * h), up(-0.5 * h)]), text("j"), "up then past rest, down");
    }

    #[test]
    fn a_touch_on_the_letter_grid_types_the_letter_under_the_finger() {
        let (g, mut b) = (g(), SlideBoard::default());
        let top_p = g.cell_rect(Half::Left, Row::Top, 3).center();
        assert_eq!(touch(&mut b, &g, 1, top_p, &[]), text("p"));
        let bottom_q = g.cell_rect(Half::Left, Row::Bottom, 1).center();
        assert_eq!(touch(&mut b, &g, 1, bottom_q, &[]), text("q"));
        // sliding from one cell to another on the grid types where it lifts
        let from = g.cell_rect(Half::Right, Row::Home, 0).center();
        let to = g.cell_rect(Half::Right, Row::Top, 4).center();
        assert_eq!(touch(&mut b, &g, 1, from, &[to]), text("l"));
    }

    #[test]
    fn the_two_thumbs_are_independent_and_can_be_mid_slide_together() {
        let (g, mut b) = (g(), SlideBoard::default());
        let (l, r) = (rail(&g, Half::Left, 1), rail(&g, Half::Right, 2));
        assert!(b.on_event(&g, Evt { id: 1, phase: Phase::Start, pos: l }).is_empty());
        assert!(b.on_event(&g, Evt { id: 2, phase: Phase::Start, pos: r }).is_empty());
        let picks = b.picks(&g);
        assert_eq!(picks.len(), 2, "both thumbs have a live pick: {picks:?}");
        assert!(picks.iter().any(|p| p.half == Half::Left && p.col == 1 && p.row == Row::Home));
        assert!(picks.iter().any(|p| p.half == Half::Right && p.col == 2 && p.row == Row::Home));
        // lift the right first: only its letter, and the left is still down
        assert_eq!(b.on_event(&g, Evt { id: 2, phase: Phase::End, pos: r }), text("t"));
        assert_eq!(b.picks(&g).len(), 1);
        assert_eq!(b.on_event(&g, Evt { id: 1, phase: Phase::End, pos: l }), text("o"));
    }

    #[test]
    fn lifting_off_the_board_or_cancelling_types_nothing() {
        let (g, mut b) = (g(), SlideBoard::default());
        let start = rail(&g, Half::Left, 1);
        assert!(touch(&mut b, &g, 1, start, &[pos2(start.x, -400.0)]).is_empty(), "flung off the top");
        b.on_event(&g, Evt { id: 1, phase: Phase::Start, pos: start });
        assert!(b.on_event(&g, Evt { id: 1, phase: Phase::Cancel, pos: start }).is_empty());
        assert!(b.on_event(&g, Evt { id: 1, phase: Phase::End, pos: start }).is_empty(), "nothing left to lift");
        assert!(touch(&mut b, &g, 1, pos2(5000.0, 5000.0), &[]).is_empty(), "landed outside everything");
    }

    #[test]
    fn the_centre_keys_work_and_a_finger_that_slides_off_one_does_not_fire_it() {
        let (g, mut b) = (g(), SlideBoard::default());
        let at = |x: Btn| g.btn_rect(x).center();
        assert_eq!(touch(&mut b, &g, 1, at(Btn::Space), &[]), text(" "));
        assert_eq!(touch(&mut b, &g, 1, at(Btn::Backspace), &[]), vec![Out::Key("BackSpace")]);
        assert_eq!(touch(&mut b, &g, 1, at(Btn::Enter), &[]), vec![Out::Key("Return")]);
        assert!(touch(&mut b, &g, 1, at(Btn::Enter), &[rail(&g, Half::Left, 0)]).is_empty(), "slid off Enter");
    }

    #[test]
    fn shift_is_one_shot_and_shows_in_the_faces() {
        let (g, mut b) = (g(), SlideBoard::default());
        assert_eq!(b.face(Half::Left, Row::Home, 0), "a");
        touch(&mut b, &g, 1, g.btn_rect(Btn::Shift).center(), &[]);
        assert!(b.shift);
        assert_eq!(b.face(Half::Left, Row::Home, 0), "A");
        assert_eq!(b.face(Half::Left, Row::Top, 0), "\"", "the apostrophe key shifts to a quote");
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Left, 0), &[]), text("A"));
        assert!(!b.shift, "spent by the letter it shifted");
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Left, 0), &[]), text("a"));
    }

    #[test]
    fn the_page_key_swaps_in_digits_and_symbols_and_back() {
        let (g, mut b) = (g(), SlideBoard::default());
        touch(&mut b, &g, 1, g.btn_rect(Btn::Page).center(), &[]);
        assert_eq!(b.page(), Page::Symbols);
        let up = |h: Half, c: usize, dy: f32| {
            let s = rail(&g, h, c);
            (s, pos2(s.x, s.y - dy))
        };
        let (s, e) = up(Half::Left, 0, g.cell_h());
        assert_eq!(touch(&mut b, &g, 1, s, &[e]), text("1"), "top row, first column is a digit");
        let (s, e) = up(Half::Right, 4, g.cell_h());
        assert_eq!(touch(&mut b, &g, 1, s, &[e]), text("0"));
        assert_eq!(touch(&mut b, &g, 1, rail(&g, Half::Left, 0), &[]), text("!"));
        touch(&mut b, &g, 1, g.btn_rect(Btn::Page).center(), &[]);
        assert_eq!(b.page(), Page::Letters);
    }

    #[test]
    fn every_letter_of_the_dvorak_pages_is_reachable_exactly_once() {
        let (g, mut b) = (g(), SlideBoard::default());
        let mut seen = std::collections::BTreeMap::new();
        for h in [Half::Left, Half::Right] {
            for row in [Row::Top, Row::Home, Row::Bottom] {
                for col in 0..COLS {
                    let start = rail(&g, h, col);
                    let dy = match row {
                        Row::Top => g.cell_h(),
                        Row::Home => 0.0,
                        Row::Bottom => -g.cell_h() * 0.6,
                    };
                    let out = touch(&mut b, &g, 1, start, &[pos2(start.x, start.y - dy)]);
                    let Out::Text(t) = &out[0] else { panic!("no text for {h:?} {row:?} {col}") };
                    *seen.entry(t.clone()).or_insert(0) += 1;
                }
            }
        }
        assert_eq!(seen.len(), 30, "thirty distinct keys: {seen:?}");
        for c in 'a'..='z' {
            assert_eq!(seen.get(&c.to_string()), Some(&1), "letter {c}");
        }
    }

    #[test]
    fn the_pointer_stands_in_only_until_a_real_touch_has_been_seen() {
        let mut b = SlideBoard::default();
        let ctx = egui::Context::default();
        let run = |b: &mut SlideBoard, events: Vec<egui::Event>| {
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), ..Default::default() };
            let mut got = Vec::new();
            let _ = ctx.run(input, |ctx| got = ctx.input(|i| b.events_from_input(i)));
            got
        };
        let btn = |pressed| egui::Event::PointerButton { pos: pos2(10.0, 10.0), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        assert_eq!(run(&mut b, vec![btn(true)]).len(), 1, "a mouse press counts with no touch screen");
        let t = egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(7), phase: egui::TouchPhase::Start, pos: pos2(10.0, 10.0), force: None };
        assert_eq!(run(&mut b, vec![t, btn(true)]).len(), 1, "the touch counts and its pointer echo does not");
        assert_eq!(run(&mut b, vec![btn(false)]).len(), 0, "from now on the pointer is ignored");
    }

    #[test]
    fn it_paints_into_a_phone_sized_rectangle_and_shows_a_live_pick() {
        let mut b = SlideBoard::default();
        let ctx = egui::Context::default();
        let colors = uikit_egui::palette_colors("system");
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 200.0));
        let g = Geom::new(rect);
        let start = rail(&g, Half::Left, 1);
        let mut shapes = 0usize;
        let mut outs = Vec::new();
        for (k, (phase, pos)) in [(Phase::Start, start), (Phase::Move, pos2(start.x, start.y - g.cell_h())), (Phase::End, pos2(start.x, start.y - g.cell_h()))].into_iter().enumerate() {
            let ev = egui::Event::Touch {
                device_id: egui::TouchDeviceId(0),
                id: egui::TouchId(1),
                phase: match phase { Phase::Start => egui::TouchPhase::Start, Phase::Move => egui::TouchPhase::Move, _ => egui::TouchPhase::End },
                pos,
                force: None,
            };
            let input = egui::RawInput { events: vec![ev], screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 300.0))), time: Some(10.0 + k as f64 * 0.05), ..Default::default() };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    outs.extend(b.show(ui, rect, &colors));
                });
            });
            shapes = shapes.max(out.shapes.len());
        }
        assert_eq!(outs, text(","), "up from the rail in column 1 is the top row's comma");
        assert!(shapes > 40, "the grid, rails and centre keys were painted: {shapes} shapes");
    }
}
