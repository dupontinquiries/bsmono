//! `sim::main_menu` — the title-screen state machine.
//!
//! Pure navigation + action model: the renderer draws whichever [`Screen`] is
//! active (with [`MainMenu::item_count`] rows and [`MainMenu::cursor`]
//! highlighted) and feeds [`Nav`] events in; this core decides the next screen
//! and which [`MenuAction`] fires. The bin maps actions onto real behaviour
//! (start a world, load a save slot, apply an option, quit). The "Continue"
//! list length comes from `sim::save_slots`.
//!
//! No IO, no rendering, no RNG — deterministic and panic-free.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKind { NewOverwrite, Quit }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen { Root, Continue, Settings, Confirm(ConfirmKind), Quitting }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav { Up, Down, Select, Back }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    None,
    StartNew,
    LoadSlot(u8),
    OpenSettings,
    SetOption { id: u8, value: i32 },
    Quit,
}

/// One adjustable settings option: id, inclusive value range, and step.
struct OptSpec { min: i32, max: i32, step: i32 }
const OPTS: [OptSpec; 3] = [
    OptSpec { min: 0, max: 100, step: 5 },  // 0: master volume
    OptSpec { min: 1, max: 20, step: 1 },   // 1: look sensitivity
    OptSpec { min: 50, max: 100, step: 5 }, // 2: render scale %
];
const ROOT_ROWS: usize = 4; // New, Continue, Settings, Quit

pub struct MainMenu {
    screen: Screen,
    cursor: usize,
    slot_count: u8,
    options: [i32; 3],
}

impl MainMenu {
    pub fn new(slot_count: u8) -> Self {
        MainMenu {
            screen: Screen::Root,
            cursor: 0,
            slot_count,
            options: [80, 8, 100], // sensible defaults
        }
    }

    pub fn screen(&self) -> Screen { self.screen }
    pub fn cursor(&self) -> usize { self.cursor }
    pub fn option(&self, id: u8) -> i32 { self.options.get(id as usize).copied().unwrap_or(0) }

    /// Number of selectable rows on the active screen (for the renderer).
    pub fn item_count(&self) -> usize {
        match self.screen {
            Screen::Root => ROOT_ROWS,
            Screen::Continue => self.slot_count as usize,
            Screen::Settings => OPTS.len(),
            Screen::Confirm(_) => 2, // Yes, No
            Screen::Quitting => 0,
        }
    }

    fn go(&mut self, screen: Screen) {
        self.screen = screen;
        self.cursor = 0;
    }

    fn move_cursor(&mut self, delta: isize) {
        let n = self.item_count();
        if n == 0 { self.cursor = 0; return; }
        let cur = self.cursor as isize;
        self.cursor = (cur + delta).rem_euclid(n as isize) as usize;
    }

    /// Apply one navigation event; returns the action it emits (`None` for pure
    /// movement). Never panics, for any event on any screen.
    pub fn input(&mut self, nav: Nav) -> MenuAction {
        match self.screen {
            Screen::Root => self.input_root(nav),
            Screen::Continue => self.input_continue(nav),
            Screen::Settings => self.input_settings(nav),
            Screen::Confirm(kind) => self.input_confirm(kind, nav),
            Screen::Quitting => MenuAction::None,
        }
    }

    fn input_root(&mut self, nav: Nav) -> MenuAction {
        match nav {
            Nav::Up => { self.move_cursor(-1); MenuAction::None }
            Nav::Down => { self.move_cursor(1); MenuAction::None }
            Nav::Back => { self.go(Screen::Confirm(ConfirmKind::Quit)); MenuAction::None }
            Nav::Select => match self.cursor {
                0 => { // New
                    if self.slot_count > 0 {
                        self.go(Screen::Confirm(ConfirmKind::NewOverwrite));
                        MenuAction::None
                    } else {
                        MenuAction::StartNew
                    }
                }
                1 => { self.go(Screen::Continue); MenuAction::None } // Continue
                2 => { self.go(Screen::Settings); MenuAction::OpenSettings } // Settings
                _ => { self.go(Screen::Confirm(ConfirmKind::Quit)); MenuAction::None } // Quit
            },
        }
    }

    fn input_continue(&mut self, nav: Nav) -> MenuAction {
        match nav {
            Nav::Up => { self.move_cursor(-1); MenuAction::None }
            Nav::Down => { self.move_cursor(1); MenuAction::None }
            Nav::Back => { self.go(Screen::Root); MenuAction::None }
            Nav::Select => {
                if self.slot_count == 0 { MenuAction::None }
                else { MenuAction::LoadSlot(self.cursor as u8) }
            }
        }
    }

    fn input_settings(&mut self, nav: Nav) -> MenuAction {
        // Cursor focuses an option; Up/Down adjust its value (clamped); Select
        // moves focus to the next option; Back returns to Root.
        match nav {
            Nav::Back => { self.go(Screen::Root); MenuAction::None }
            Nav::Select => { self.move_cursor(1); MenuAction::None }
            Nav::Up | Nav::Down => {
                let id = self.cursor.min(OPTS.len() - 1);
                let spec = &OPTS[id];
                let dir = if nav == Nav::Up { 1 } else { -1 };
                let v = (self.options[id] + dir * spec.step).clamp(spec.min, spec.max);
                self.options[id] = v;
                MenuAction::SetOption { id: id as u8, value: v }
            }
        }
    }

    fn input_confirm(&mut self, kind: ConfirmKind, nav: Nav) -> MenuAction {
        match nav {
            Nav::Up => { self.move_cursor(-1); MenuAction::None }
            Nav::Down => { self.move_cursor(1); MenuAction::None }
            Nav::Back => { self.go(Screen::Root); MenuAction::None }
            Nav::Select => {
                if self.cursor == 0 {
                    // Yes
                    match kind {
                        ConfirmKind::NewOverwrite => { self.go(Screen::Root); MenuAction::StartNew }
                        ConfirmKind::Quit => { self.go(Screen::Quitting); MenuAction::Quit }
                    }
                } else {
                    // No
                    self.go(Screen::Root);
                    MenuAction::None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_cursor_wraps() {
        let mut m = MainMenu::new(0);
        assert_eq!(m.cursor(), 0);
        m.input(Nav::Up); // wrap to last
        assert_eq!(m.cursor(), ROOT_ROWS - 1);
        m.input(Nav::Down); // wrap to 0
        assert_eq!(m.cursor(), 0);
        for _ in 0..ROOT_ROWS { m.input(Nav::Down); }
        assert_eq!(m.cursor(), 0); // full loop
    }

    #[test]
    fn new_with_no_slots_starts_immediately() {
        let mut m = MainMenu::new(0);
        assert_eq!(m.input(Nav::Select), MenuAction::StartNew);
        assert_eq!(m.screen(), Screen::Root);
    }

    #[test]
    fn new_with_slots_confirms_then_starts() {
        let mut m = MainMenu::new(3);
        assert_eq!(m.input(Nav::Select), MenuAction::None);
        assert_eq!(m.screen(), Screen::Confirm(ConfirmKind::NewOverwrite));
        // Yes (cursor 0)
        assert_eq!(m.input(Nav::Select), MenuAction::StartNew);
        assert_eq!(m.screen(), Screen::Root);
    }

    #[test]
    fn confirm_no_returns_to_root() {
        let mut m = MainMenu::new(3);
        m.input(Nav::Select); // New -> Confirm
        m.input(Nav::Down); // cursor to No
        assert_eq!(m.input(Nav::Select), MenuAction::None);
        assert_eq!(m.screen(), Screen::Root);
    }

    #[test]
    fn continue_loads_selected_slot() {
        let mut m = MainMenu::new(4);
        m.input(Nav::Down); // cursor 1 = Continue
        assert_eq!(m.input(Nav::Select), MenuAction::None);
        assert_eq!(m.screen(), Screen::Continue);
        m.input(Nav::Down); // slot 1
        m.input(Nav::Down); // slot 2
        assert_eq!(m.input(Nav::Select), MenuAction::LoadSlot(2));
    }

    #[test]
    fn continue_with_no_slots_is_safe() {
        let mut m = MainMenu::new(0);
        m.input(Nav::Down); // Continue
        m.input(Nav::Select);
        assert_eq!(m.screen(), Screen::Continue);
        assert_eq!(m.item_count(), 0);
        assert_eq!(m.input(Nav::Select), MenuAction::None); // no panic, no load
        assert_eq!(m.cursor(), 0);
    }

    #[test]
    fn settings_adjusts_and_clamps() {
        let mut m = MainMenu::new(1);
        m.input(Nav::Down); m.input(Nav::Down); // cursor 2 = Settings
        assert_eq!(m.input(Nav::Select), MenuAction::OpenSettings);
        assert_eq!(m.screen(), Screen::Settings);
        // volume option (id 0), step 5
        let before = m.option(0);
        let a = m.input(Nav::Up);
        assert_eq!(a, MenuAction::SetOption { id: 0, value: before + 5 });
        // clamp at max
        for _ in 0..40 { m.input(Nav::Up); }
        assert_eq!(m.option(0), 100);
        // Select moves focus to next option
        m.input(Nav::Select);
        assert_eq!(m.cursor(), 1);
        let s = m.option(1);
        let b = m.input(Nav::Down);
        assert_eq!(b, MenuAction::SetOption { id: 1, value: (s - 1).max(1) });
        m.input(Nav::Back);
        assert_eq!(m.screen(), Screen::Root);
    }

    #[test]
    fn back_on_root_confirms_quit_then_quits() {
        let mut m = MainMenu::new(0);
        m.input(Nav::Back);
        assert_eq!(m.screen(), Screen::Confirm(ConfirmKind::Quit));
        assert_eq!(m.input(Nav::Select), MenuAction::Quit); // Yes
        assert_eq!(m.screen(), Screen::Quitting);
        assert_eq!(m.input(Nav::Select), MenuAction::None); // terminal
    }

    #[test]
    fn never_panics_on_any_sequence() {
        let navs = [Nav::Up, Nav::Down, Nav::Select, Nav::Back];
        let mut m = MainMenu::new(2);
        let mut state = 1u64;
        for _ in 0..4000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let n = navs[(state >> 33) as usize % navs.len()];
            let _ = m.input(n);
            assert!(m.cursor() < m.item_count().max(1));
        }
    }
}
