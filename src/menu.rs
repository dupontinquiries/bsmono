//! MENUS, as data.
//!
//! There are nine screens in `Mode` and each one carries its own selection
//! index, its own scroll handling, its own arrow-key branch in `key_down`
//! and its own draw function in `hud.rs`. They agree about none of it: some
//! wrap at the ends and some clamp, some have a scroll window and some
//! assume everything fits, and adding a tenth screen means writing all four
//! parts again and getting them consistent by hand.
//!
//! This is the shared part pulled out. A menu is a title and a list of rows;
//! a row is a label and an `Item` that says what it DOES. Navigation,
//! wrapping, skipping unselectable rows, clamping sliders and keeping the
//! selection inside the scroll window are solved once here, and a screen
//! becomes a function that builds rows and a match on what came back.
//!
//! Deliberately not included: drawing, and any reference to `Config`,
//! `Save` or `Bindings`. The moment this file knows about those it stops
//! being reusable and becomes the settings screen with extra steps. It
//! returns what happened and the caller applies it.

/// How a value should be written out. Formatting lives with the row rather
/// than the draw code so a screen cannot disagree with itself about whether
/// something is a percentage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fmt {
    Percent,
    Plain,
    Degrees,
    Metres,
    Seconds,
}

impl Fmt {
    pub fn write(self, v: f32) -> String {
        match self {
            Fmt::Percent => format!("{:.0}%", v * 100.0),
            Fmt::Plain => {
                if (v - v.round()).abs() < 1e-3 {
                    format!("{:.0}", v)
                } else {
                    format!("{v:.2}")
                }
            }
            Fmt::Degrees => format!("{v:.0}°"),
            Fmt::Metres => format!("{v:.2} m"),
            Fmt::Seconds => format!("{v:.2} s"),
        }
    }
}

/// What a row is.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// press to do something; the id is what comes back
    Action,
    Toggle(bool),
    Slider { v: f32, min: f32, max: f32, step: f32, fmt: Fmt },
    /// one of a list — cars, maps, presets
    Choice { idx: usize, of: Vec<String> },
    /// opens another screen
    Submenu,
    /// a heading: drawn, never selectable
    Heading,
    /// blank space: drawn, never selectable
    Separator,
}

impl Item {
    pub fn selectable(&self) -> bool {
        !matches!(self, Item::Heading | Item::Separator)
    }
    /// Does left/right do anything here?
    pub fn adjustable(&self) -> bool {
        matches!(self, Item::Toggle(_) | Item::Slider { .. } | Item::Choice { .. })
    }
    /// The right-hand column.
    pub fn value(&self) -> String {
        match self {
            Item::Toggle(b) => (if *b { "ON" } else { "OFF" }).into(),
            Item::Slider { v, fmt, .. } => fmt.write(*v),
            Item::Choice { idx, of } => of.get(*idx).cloned().unwrap_or_default(),
            Item::Submenu => ">".into(),
            _ => String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    /// stable identity, so the caller matches on this and not on position —
    /// inserting a row above must not silently rebind the one below
    pub id: &'static str,
    pub label: String,
    pub item: Item,
    pub enabled: bool,
    /// one line of explanation, shown for the selected row
    pub hint: Option<&'static str>,
}

impl Row {
    pub fn action(id: &'static str, label: &str) -> Row {
        Row { id, label: label.into(), item: Item::Action, enabled: true, hint: None }
    }
    pub fn toggle(id: &'static str, label: &str, on: bool) -> Row {
        Row { id, label: label.into(), item: Item::Toggle(on), enabled: true, hint: None }
    }
    pub fn slider(id: &'static str, label: &str, v: f32, min: f32, max: f32, step: f32, fmt: Fmt) -> Row {
        Row { id, label: label.into(), item: Item::Slider { v, min, max, step, fmt }, enabled: true, hint: None }
    }
    pub fn choice(id: &'static str, label: &str, idx: usize, of: &[&str]) -> Row {
        Row {
            id,
            label: label.into(),
            item: Item::Choice { idx, of: of.iter().map(|s| s.to_string()).collect() },
            enabled: true,
            hint: None,
        }
    }
    pub fn submenu(id: &'static str, label: &str) -> Row {
        Row { id, label: label.into(), item: Item::Submenu, enabled: true, hint: None }
    }
    pub fn heading(label: &str) -> Row {
        Row { id: "", label: label.into(), item: Item::Heading, enabled: false, hint: None }
    }
    pub fn gap() -> Row {
        Row { id: "", label: String::new(), item: Item::Separator, enabled: false, hint: None }
    }
    pub fn hint(mut self, h: &'static str) -> Row {
        self.hint = Some(h);
        self
    }
    /// Greyed out but still visible — "why can't I press this" is a better
    /// experience than a row that is not there.
    pub fn disabled(mut self) -> Row {
        self.enabled = false;
        self
    }
    fn navigable(&self) -> bool {
        self.item.selectable()
    }
}

/// What the caller has to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// an Action row was pressed
    Pressed(&'static str),
    /// a Submenu row was pressed
    Open(&'static str),
    /// a value changed; the caller writes it through to wherever it lives
    Changed(&'static str, Value),
    /// back / escape
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Num(f32),
    Index(usize),
}

pub struct Menu {
    pub title: String,
    pub rows: Vec<Row>,
    sel: usize,
    scroll: usize,
    /// how many rows fit on screen
    pub view: usize,
}

impl Menu {
    pub fn new(title: &str, rows: Vec<Row>) -> Menu {
        let mut m = Menu { title: title.into(), rows, sel: 0, scroll: 0, view: 12 };
        // never open on a heading
        if !m.rows.get(0).map_or(false, |r| r.navigable()) {
            m.step(1);
        }
        m.follow();
        m
    }

    pub fn sel(&self) -> usize {
        self.sel
    }
    pub fn scroll(&self) -> usize {
        self.scroll
    }
    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.sel)
    }
    /// The rows to draw, with their true indices, so a draw function does not
    /// have to reimplement the window.
    pub fn visible(&self) -> impl Iterator<Item = (usize, &Row)> {
        self.rows.iter().enumerate().skip(self.scroll).take(self.view)
    }

    /// Move the cursor, skipping headings and gaps and wrapping at the ends.
    /// Wrapping is the choice here because every list in this game is short
    /// enough that walking off the bottom to reach the top is the fast path.
    pub fn step(&mut self, d: i32) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let mut i = self.sel as i32;
        for _ in 0..n {
            i = (i + d).rem_euclid(n as i32);
            if self.rows[i as usize].navigable() {
                self.sel = i as usize;
                break;
            }
        }
        self.follow();
    }

    /// Keep the selection inside the window, scrolling only as far as it must.
    fn follow(&mut self) {
        let v = self.view.max(1);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + v {
            self.scroll = self.sel + 1 - v;
        }
        let max_scroll = self.rows.len().saturating_sub(v);
        self.scroll = self.scroll.min(max_scroll);
    }

    /// Left/right on the selected row.
    pub fn adjust(&mut self, d: i32) -> Option<Event> {
        let row = self.rows.get_mut(self.sel)?;
        if !row.enabled {
            return None;
        }
        let id = row.id;
        match &mut row.item {
            Item::Toggle(b) => {
                *b = !*b;
                Some(Event::Changed(id, Value::Bool(*b)))
            }
            Item::Slider { v, min, max, step, .. } => {
                let nv = (*v + *step * d as f32).clamp(*min, *max);
                // step on a float drifts; snap to the grid so a slider set to
                // 0.5 reads 0.50 and not 0.4999998
                let snapped = (((nv - *min) / *step).round() * *step + *min).clamp(*min, *max);
                if (snapped - *v).abs() < 1e-6 {
                    return None;
                }
                *v = snapped;
                Some(Event::Changed(id, Value::Num(snapped)))
            }
            Item::Choice { idx, of } => {
                if of.is_empty() {
                    return None;
                }
                *idx = ((*idx as i32 + d).rem_euclid(of.len() as i32)) as usize;
                Some(Event::Changed(id, Value::Index(*idx)))
            }
            _ => None,
        }
    }

    /// Enter / click on the selected row. A toggle activates rather than
    /// needing a sideways press, because everyone expects that.
    pub fn activate(&mut self) -> Option<Event> {
        let row = self.rows.get(self.sel)?;
        if !row.enabled {
            return None;
        }
        match row.item {
            Item::Action => Some(Event::Pressed(row.id)),
            Item::Submenu => Some(Event::Open(row.id)),
            Item::Toggle(_) => self.adjust(1),
            Item::Choice { .. } => self.adjust(1),
            _ => None,
        }
    }

    /// Point at a row by index, for mouse hover and click.
    pub fn point_at(&mut self, i: usize) -> bool {
        if self.rows.get(i).map_or(false, |r| r.navigable()) {
            self.sel = i;
            self.follow();
            return true;
        }
        false
    }

    /// Set a row's value without generating an event — for syncing a menu to
    /// state that changed underneath it.
    pub fn set(&mut self, id: &str, v: Value) {
        let Some(row) = self.rows.iter_mut().find(|r| r.id == id) else { return };
        match (&mut row.item, v) {
            (Item::Toggle(b), Value::Bool(nb)) => *b = nb,
            (Item::Slider { v: sv, min, max, .. }, Value::Num(n)) => *sv = n.clamp(*min, *max),
            (Item::Choice { idx, of }, Value::Index(n)) => *idx = n.min(of.len().saturating_sub(1)),
            _ => {}
        }
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.rows.iter().find(|r| r.id == id).map(|r| &r.item)
    }
}

/// Screens on top of screens, so a submenu knows how to get home.
///
/// The current code answers "where does escape go" with `menu_from_pause`, a
/// bool that only works because the graph happens to be two deep. A stack
/// works at any depth and does not need to be reasoned about.
#[derive(Default)]
pub struct Stack {
    pub screens: Vec<Menu>,
}

impl Stack {
    pub fn new() -> Stack {
        Stack { screens: Vec::new() }
    }
    pub fn push(&mut self, m: Menu) {
        self.screens.push(m);
    }
    pub fn top(&mut self) -> Option<&mut Menu> {
        self.screens.last_mut()
    }
    pub fn peek(&self) -> Option<&Menu> {
        self.screens.last()
    }
    /// Returns true once the last screen has closed, so the caller knows to
    /// go back to play.
    pub fn back(&mut self) -> bool {
        self.screens.pop();
        self.screens.is_empty()
    }
    pub fn depth(&self) -> usize {
        self.screens.len()
    }
    pub fn is_open(&self) -> bool {
        !self.screens.is_empty()
    }
    /// The trail, for a breadcrumb.
    pub fn path(&self) -> String {
        self.screens.iter().map(|m| m.title.as_str()).collect::<Vec<_>>().join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Menu {
        Menu::new(
            "VIDEO",
            vec![
                Row::heading("DISPLAY"),
                Row::toggle("vsync", "VSYNC", true),
                Row::slider("scale", "RENDER SCALE", 1.0, 0.5, 1.0, 0.05, Fmt::Percent),
                Row::gap(),
                Row::heading("CAMERA"),
                Row::choice("cam", "MODE", 0, &["BALL", "CHASE", "FIRST"]),
                Row::slider("fov", "FOV", 90.0, 60.0, 120.0, 5.0, Fmt::Degrees),
                Row::submenu("adv", "ADVANCED"),
                Row::action("reset", "RESET TO DEFAULTS"),
            ],
        )
    }

    #[test]
    fn it_never_lands_on_a_heading() {
        let mut m = sample();
        assert_eq!(m.selected().unwrap().id, "vsync", "opened on a heading");
        // walk the whole list both ways; a heading must never be selected
        for _ in 0..40 {
            m.step(1);
            assert!(m.selected().unwrap().item.selectable());
        }
        for _ in 0..40 {
            m.step(-1);
            assert!(m.selected().unwrap().item.selectable());
        }
    }

    #[test]
    fn it_wraps_at_both_ends() {
        let mut m = sample();
        m.step(-1);
        assert_eq!(m.selected().unwrap().id, "reset", "up from the top did not wrap");
        m.step(1);
        assert_eq!(m.selected().unwrap().id, "vsync", "down from the bottom did not wrap");
    }

    #[test]
    fn sliders_clamp_and_stay_on_the_grid() {
        let mut m = sample();
        m.point_at(2);
        // walk off the top: it must stop at max, not sail past
        for _ in 0..50 {
            m.adjust(1);
        }
        let Item::Slider { v, .. } = m.get("scale").unwrap() else { panic!() };
        assert!((v - 1.0).abs() < 1e-6, "clamped to {v}");
        for _ in 0..50 {
            m.adjust(-1);
        }
        let Item::Slider { v, .. } = m.get("scale").unwrap() else { panic!() };
        assert!((v - 0.5).abs() < 1e-6, "clamped to {v}");
        // and every stop is on the step grid, so it reads 55% not 54.9999%
        m.adjust(1);
        let Item::Slider { v, .. } = m.get("scale").unwrap() else { panic!() };
        assert_eq!(Fmt::Percent.write(*v), "55%");
    }

    #[test]
    fn a_slider_at_its_limit_reports_no_change() {
        let mut m = sample();
        m.point_at(2);
        for _ in 0..50 {
            m.adjust(1);
        }
        assert!(m.adjust(1).is_none(), "pushing a maxed slider still fired an event");
    }

    #[test]
    fn choices_cycle_and_report_their_index() {
        let mut m = sample();
        m.point_at(5);
        assert_eq!(m.adjust(1), Some(Event::Changed("cam", Value::Index(1))));
        assert_eq!(m.adjust(1), Some(Event::Changed("cam", Value::Index(2))));
        // wraps
        assert_eq!(m.adjust(1), Some(Event::Changed("cam", Value::Index(0))));
        assert_eq!(m.get("cam").unwrap().value(), "BALL");
    }

    #[test]
    fn events_carry_ids_not_positions() {
        // the point of ids: inserting a row must not rebind the ones below
        let mut m = sample();
        m.point_at(8);
        assert_eq!(m.activate(), Some(Event::Pressed("reset")));
        m.rows.insert(1, Row::toggle("newthing", "NEW", false));
        m.point_at(9);
        assert_eq!(m.activate(), Some(Event::Pressed("reset")), "a row moved and the id followed it");
    }

    #[test]
    fn a_disabled_row_is_visible_but_inert() {
        let mut m = Menu::new("X", vec![Row::action("go", "GO").disabled(), Row::action("b", "B")]);
        m.point_at(0);
        assert!(m.activate().is_none(), "a disabled row fired");
        // still navigable, so the player can see it and read its hint
        assert!(m.selected().unwrap().item.selectable());
    }

    #[test]
    fn scrolling_follows_the_selection_and_no_further() {
        let rows: Vec<Row> = (0..30)
            .map(|i| Row::toggle(Box::leak(format!("r{i}").into_boxed_str()), &format!("ROW {i}"), false))
            .collect();
        let mut m = Menu::new("LONG", rows);
        m.view = 8;
        assert_eq!(m.scroll(), 0);
        for _ in 0..7 {
            m.step(1);
        }
        assert_eq!(m.scroll(), 0, "scrolled before it needed to");
        m.step(1);
        assert_eq!(m.scroll(), 1, "scrolled further than one row");
        // the window never runs off the end
        for _ in 0..40 {
            m.step(1);
        }
        assert!(m.scroll() + m.view <= 30 || m.scroll() == 0);
        assert_eq!(m.visible().count(), 8);
    }

    #[test]
    fn the_stack_knows_the_way_home() {
        let mut s = Stack::new();
        assert!(!s.is_open());
        s.push(Menu::new("SETTINGS", vec![Row::submenu("video", "VIDEO")]));
        s.push(Menu::new("VIDEO", vec![Row::submenu("adv", "ADVANCED")]));
        s.push(Menu::new("ADVANCED", vec![Row::action("x", "X")]));
        assert_eq!(s.depth(), 3);
        assert_eq!(s.path(), "SETTINGS / VIDEO / ADVANCED");
        assert!(!s.back());
        assert!(!s.back());
        assert!(s.back(), "closing the last screen did not report done");
        assert!(!s.is_open());
    }

    #[test]
    fn formatting_lives_with_the_row() {
        assert_eq!(Fmt::Percent.write(0.55), "55%");
        assert_eq!(Fmt::Degrees.write(90.0), "90°");
        assert_eq!(Fmt::Plain.write(3.0), "3");
        assert_eq!(Fmt::Plain.write(3.25), "3.25");
        assert_eq!(Fmt::Seconds.write(1.3), "1.30 s");
        assert_eq!(Item::Toggle(true).value(), "ON");
    }

    #[test]
    fn an_empty_or_headings_only_menu_does_not_panic() {
        let mut m = Menu::new("EMPTY", vec![]);
        m.step(1);
        assert!(m.activate().is_none());
        assert!(m.adjust(1).is_none());
        let mut h = Menu::new("HEADINGS", vec![Row::heading("A"), Row::gap()]);
        h.step(1);
        assert!(h.activate().is_none());
    }
}
