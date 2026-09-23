//! The command palette: everything the shell can do, in one list, opened by
//! press-and-hold on the bottom edge. Pure: the shell rebuilds the item list
//! from its own state (modes, windows, apps, settings, actions) and executes
//! what `activate` returns.
//!
//! Layout is here, not in the view, so the hold-and-slide selection can be
//! tested: the sheet rises from the bottom; row 0 is the mode row (three
//! equal buttons: the old arc's contract, one gesture: press, slide, lift),
//! then the search field, then list rows. Lifting off any row leaves the
//! palette open for taps.
//!
//! Filtering: case-insensitive subsequence match, scored (prefix > word
//! start > consecutive > scattered), ties broken by recency. Empty query:
//! recents first, then modes, windows, apps, settings, actions.

use crate::anim::DragTransition;
use crate::hold::HoldOption;

pub const MAX_RECENTS: usize = 8;
pub const SHEET_FRAC: f32 = 0.62;
pub const MODE_ROW_H: f32 = 76.0;
pub const SEARCH_H: f32 = 44.0;
pub const ROW_H: f32 = 46.0;
pub const PAD: f32 = 12.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    Mode(HoldOption),
    ToggleAutoBrightness,
    ToggleFatCursor,
    LayoutPullDown,
    LayoutRightBar,
    ClockDigital,
    ClockAnalog,
    Window(u64),
    App(String),
    Switcher,
    Launcher,
    Settings,
    CloseAll,
    Lock,
    Sleep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    /// Stable key for recents ("mode:keyboard", "app:org.gnome.Console").
    pub key: String,
    pub label: String,
    /// Small grey text after the label ("running", "app", "setting").
    pub hint: &'static str,
    pub action: PaletteAction,
}

impl PaletteItem {
    pub fn new(key: impl Into<String>, label: impl Into<String>, hint: &'static str, action: PaletteAction) -> Self {
        PaletteItem { key: key.into(), label: label.into(), hint, action }
    }
}

/// What is under a point on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Mode(usize),
    Search,
    Row(usize),
    Sheet,
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geom {
    pub screen: (f32, f32),
    pub sheet_top: f32,
}

impl Geom {
    pub fn new(screen: (f32, f32)) -> Geom {
        Geom { screen, sheet_top: screen.1 * (1.0 - SHEET_FRAC) }
    }

    pub fn mode_rect(&self, i: usize) -> (f32, f32, f32, f32) {
        let w = (self.screen.0 - PAD * 4.0) / 3.0;
        (PAD + i as f32 * (w + PAD), self.sheet_top + PAD, w, MODE_ROW_H)
    }

    pub fn search_y(&self) -> f32 {
        self.sheet_top + PAD * 2.0 + MODE_ROW_H
    }

    pub fn row_y(&self, i: usize) -> f32 {
        self.search_y() + SEARCH_H + PAD + i as f32 * ROW_H
    }

    pub fn rows_visible(&self) -> usize {
        ((self.screen.1 - self.row_y(0) - PAD) / ROW_H).max(0.0) as usize
    }

    pub fn hit(&self, x: f32, y: f32, rows: usize) -> Hit {
        if y < self.sheet_top || x < 0.0 || x > self.screen.0 {
            return Hit::Outside;
        }
        for i in 0..3 {
            let (rx, ry, rw, rh) = self.mode_rect(i);
            if x >= rx && x < rx + rw && y >= ry && y < ry + rh {
                return Hit::Mode(i);
            }
        }
        if y >= self.search_y() && y < self.search_y() + SEARCH_H {
            return Hit::Search;
        }
        if y >= self.row_y(0) {
            let i = ((y - self.row_y(0)) / ROW_H) as usize;
            if i < rows.min(self.rows_visible()) {
                return Hit::Row(i);
            }
        }
        Hit::Sheet
    }
}

/// Score a query against a label; None = no match.
pub fn score(query: &str, label: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let l: Vec<char> = label.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut qi = 0;
    let mut s = 0;
    let mut prev_matched = false;
    for (i, &c) in l.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            let word_start = i == 0 || !l[i - 1].is_alphanumeric();
            s += 1 + if i == qi && i < q.len() { 4 } else { 0 } + if word_start { 3 } else { 0 } + if prev_matched { 2 } else { 0 };
            qi += 1;
            prev_matched = true;
        } else {
            prev_matched = false;
        }
    }
    (qi == q.len()).then_some(s * 10 - l.len() as i32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    /// Row under the finger during the hold (Mode or Row).
    pub hover: Option<Hit>,
    pub reveal: DragTransition,
    /// The three mode buttons, in row order.
    pub modes: [HoldOption; 3],
    items: Vec<PaletteItem>,
    filtered: Vec<usize>,
    recents: Vec<String>,
    /// Set by `open_by_hold`; a lift on nothing leaves the palette open.
    by_hold: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            open: false,
            query: String::new(),
            hover: None,
            reveal: DragTransition::default(),
            modes: [HoldOption::Carousel, HoldOption::Keyboard, HoldOption::Trackpad],
            items: Vec::new(),
            filtered: Vec::new(),
            recents: Vec::new(),
            by_hold: false,
        }
    }
}

impl Palette {
    /// Replace the item list (the shell rebuilds it from its state).
    pub fn set_items(&mut self, items: Vec<PaletteItem>) {
        self.items = items;
        self.refilter();
    }

    pub fn recents(&self) -> &[String] {
        &self.recents
    }

    pub fn set_recents(&mut self, keys: impl IntoIterator<Item = String>) {
        self.recents = keys.into_iter().take(MAX_RECENTS).collect();
        self.refilter();
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        self.refilter();
    }

    /// Visible rows, in order.
    pub fn rows(&self) -> impl Iterator<Item = &PaletteItem> {
        self.filtered.iter().map(|i| &self.items[*i])
    }

    pub fn row_count(&self) -> usize {
        self.filtered.len()
    }

    fn refilter(&mut self) {
        let mut scored: Vec<(i32, usize, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| score(&self.query, &it.label).map(|s| (s, i, self.recency(&it.key))))
            .collect();
        // Higher score first; among equals, more recent first (lower rank);
        // then the shell's own order.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
        self.filtered = scored.into_iter().map(|(_, i, _)| i).collect();
    }

    fn recency(&self, key: &str) -> usize {
        self.recents.iter().position(|k| k == key).unwrap_or(MAX_RECENTS)
    }

    pub fn open_by_hold(&mut self) {
        self.open = true;
        self.by_hold = true;
        self.query.clear();
        self.hover = None;
        self.reveal = DragTransition::default();
        self.reveal.animate_to(1.0);
        self.refilter();
    }

    pub fn open_by_tap(&mut self) {
        self.open_by_hold();
        self.by_hold = false;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.hover = None;
        self.reveal.animate_to(0.0);
    }

    /// The held finger moved.
    pub fn hold_move(&mut self, geom: &Geom, x: f32, y: f32) {
        self.hover = match geom.hit(x, y, self.row_count()) {
            h @ (Hit::Mode(_) | Hit::Row(_)) => Some(h),
            _ => None,
        };
    }

    /// The held finger lifted: activate what it is on, else stay open.
    pub fn hold_release(&mut self, geom: &Geom, x: f32, y: f32) -> Option<PaletteAction> {
        self.hold_move(geom, x, y);
        let h = self.hover.take()?;
        self.activate_hit(h)
    }

    /// A tap on the sheet.
    pub fn tap(&mut self, geom: &Geom, x: f32, y: f32) -> Option<PaletteAction> {
        match geom.hit(x, y, self.row_count()) {
            Hit::Outside => {
                self.close();
                None
            }
            h => self.activate_hit(h),
        }
    }

    fn activate_hit(&mut self, h: Hit) -> Option<PaletteAction> {
        match h {
            Hit::Mode(i) => {
                let m = self.modes[i];
                self.note_recent(&format!("mode:{}", m.label().to_lowercase()));
                self.close();
                Some(PaletteAction::Mode(m))
            }
            Hit::Row(i) => {
                let idx = *self.filtered.get(i)?;
                let (key, action) = (self.items[idx].key.clone(), self.items[idx].action.clone());
                self.note_recent(&key);
                self.close();
                Some(action)
            }
            _ => None,
        }
    }

    fn note_recent(&mut self, key: &str) {
        self.recents.retain(|k| k != key);
        self.recents.insert(0, key.to_string());
        self.recents.truncate(MAX_RECENTS);
    }

    pub fn tick(&mut self, dt_ms: f32) {
        self.reveal.tick(dt_ms);
    }

    pub fn is_visible(&self) -> bool {
        self.open || self.reveal.progress > 0.001
    }

    /// The bottom edge's hold: while the palette is open (by hold), the
    /// recognizer's HoldMove/HoldRelease drive it.
    pub fn is_holding(&self) -> bool {
        self.open && self.by_hold
    }

    pub fn end_hold(&mut self) {
        self.by_hold = false;
    }

    #[cfg(test)]
    fn recents_vec(&self) -> Vec<String> {
        self.recents.clone()
    }

    pub fn recents_kv(&self) -> String {
        self.recents.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PaletteItem> {
        vec![
            PaletteItem::new("win:1", "GTK Demo", "running", PaletteAction::Window(1)),
            PaletteItem::new("win:2", "Widget Factory", "running", PaletteAction::Window(2)),
            PaletteItem::new("app:org.gnome.Console", "Console", "app", PaletteAction::App("org.gnome.Console".into())),
            PaletteItem::new("app:org.mozilla.firefox", "Firefox", "app", PaletteAction::App("org.mozilla.firefox".into())),
            PaletteItem::new("set:layout", "Right-hand bar", "setting", PaletteAction::LayoutRightBar),
            PaletteItem::new("act:lock", "Lock", "action", PaletteAction::Lock),
            PaletteItem::new("act:closeall", "Close all windows", "action", PaletteAction::CloseAll),
        ]
    }

    fn open() -> (Palette, Geom) {
        let mut p = Palette::default();
        p.set_items(items());
        p.open_by_hold();
        (p, Geom::new((408.0, 900.0)))
    }

    #[test]
    fn scoring_prefers_prefix_then_word_start_then_subsequence() {
        assert!(score("fi", "Firefox") > score("fi", "Widget Factory"), "prefix beats a later word start");
        assert!(score("fa", "Widget Factory").unwrap() > score("fa", "Firefox").unwrap_or(-999), "word start beats no match");
        assert!(score("wf", "Widget Factory").unwrap() > score("wf", "Wolf").unwrap_or(-999), "two word starts beat a scattered pair");
        assert!(score("cl", "Close all windows") > score("cl", "Console"));
        assert_eq!(score("zzz", "Console"), None);
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("GTK DEMO", "gtk demo"), score("gtk demo", "GTK Demo"), "case and spaces do not matter");
    }

    #[test]
    fn typed_query_filters_and_orders() {
        let (mut p, _) = open();
        p.set_query("co");
        let labels: Vec<&str> = p.rows().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["Console", "Close all windows", "Widget Factory"], "prefix matches first (shorter first), the scattered match (fa-c-t-o-ry) last");
        p.set_query("fi");
        assert_eq!(p.rows().map(|r| r.label.as_str()).collect::<Vec<_>>(), vec!["Firefox"], "no i after the f in Widget Factory");
        p.set_query("");
        assert_eq!(p.row_count(), 7, "empty query shows everything");
    }

    #[test]
    fn recents_come_first_on_an_empty_query_and_activation_records_them() {
        let (mut p, g) = open();
        // Tap the row for Console (index 2 in shell order).
        let a = p.tap(&g, 100.0, g.row_y(2) + 5.0);
        assert_eq!(a, Some(PaletteAction::App("org.gnome.Console".into())));
        assert!(!p.open, "activating closes");
        assert_eq!(p.recents_vec(), vec!["app:org.gnome.Console"]);
        p.open_by_hold();
        assert_eq!(p.rows().next().map(|r| r.label.as_str()), Some("Console"), "recent first");
        assert_eq!(p.recents_kv(), "app:org.gnome.Console");
    }

    #[test]
    fn hold_slide_lift_selects_a_mode_and_lifting_elsewhere_stays_open() {
        let (mut p, g) = open();
        let (x, y, w, h) = g.mode_rect(1);
        p.hold_move(&g, x + w / 2.0, y + h / 2.0);
        assert_eq!(p.hover, Some(Hit::Mode(1)));
        assert_eq!(p.hold_release(&g, x + w / 2.0, y + h / 2.0), Some(PaletteAction::Mode(HoldOption::Keyboard)));
        assert!(!p.open);
        let (mut p, g) = open();
        assert_eq!(p.hold_release(&g, 200.0, g.search_y() + 10.0), None, "lift on the search field");
        assert!(p.open, "stays open for taps");
        assert_eq!(p.hold_release(&g, 200.0, 50.0), None, "lift above the sheet");
        assert!(p.open);
    }

    #[test]
    fn a_tap_outside_the_sheet_closes_it() {
        let (mut p, g) = open();
        assert_eq!(p.tap(&g, 200.0, g.sheet_top - 40.0), None);
        assert!(!p.open);
    }

    #[test]
    fn geometry_hits_the_right_things() {
        let g = Geom::new((408.0, 900.0));
        assert_eq!(g.hit(10.0, 10.0, 5), Hit::Outside);
        let (x, y, ..) = g.mode_rect(2);
        assert_eq!(g.hit(x + 1.0, y + 1.0, 5), Hit::Mode(2));
        assert_eq!(g.hit(200.0, g.search_y() + 1.0, 5), Hit::Search);
        assert_eq!(g.hit(200.0, g.row_y(3) + 1.0, 5), Hit::Row(3));
        assert_eq!(g.hit(200.0, g.row_y(3) + 1.0, 2), Hit::Sheet, "past the last row");
        assert!(g.rows_visible() >= 4, "{}", g.rows_visible());
    }

    #[test]
    fn reveal_animates_open_and_closed() {
        let (mut p, _) = open();
        for _ in 0..60 {
            p.tick(11.1);
        }
        assert!((p.reveal.progress - 1.0).abs() < 1e-3);
        p.close();
        assert!(p.is_visible(), "still drawn while sliding away");
        for _ in 0..60 {
            p.tick(11.1);
        }
        assert!(!p.is_visible());
    }
}
