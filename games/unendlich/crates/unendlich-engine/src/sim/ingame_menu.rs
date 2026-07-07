//! `sim::ingame_menu` — in-game overlays (pause, inventory, settings, map).
//!
//! The bin gates the simulation tick on [`IngameMenu::is_paused`] and draws the
//! active [`Overlay`]; this core owns navigation + which [`UiAction`] fires.
//! Distinct from `main_menu` (the title screen). Pure, deterministic, panic-free.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay { None, Pause, Inventory, Settings, Map }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent { TogglePause, OpenInventory, OpenMap, Back, Nav(i8), Select }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    None,
    Resume,
    Save,
    SaveAndQuit,
    SetOption { id: u8, value: i32 },
    UseSlot(u8),
}

struct OptSpec { min: i32, max: i32, step: i32 }
const OPTS: [OptSpec; 3] = [
    OptSpec { min: 0, max: 100, step: 5 },  // 0: music
    OptSpec { min: 0, max: 100, step: 5 },  // 1: sfx
    OptSpec { min: 30, max: 120, step: 5 }, // 2: fov
];
const PAUSE_ROWS: usize = 4; // Resume, Settings, Save, SaveAndQuit
const INVENTORY_SLOTS: usize = 9;

pub struct IngameMenu {
    overlay: Overlay,
    cursor: usize,
    options: [i32; 3],
}

impl IngameMenu {
    pub fn new() -> Self {
        IngameMenu { overlay: Overlay::None, cursor: 0, options: [70, 80, 75] }
    }

    pub fn overlay(&self) -> Overlay { self.overlay }
    pub fn cursor(&self) -> usize { self.cursor }
    pub fn option(&self, id: u8) -> i32 { self.options.get(id as usize).copied().unwrap_or(0) }

    /// True when a blocking overlay is up (Pause/Settings) — the bin should freeze
    /// the sim. Inventory and Map are playable (not paused).
    pub fn is_paused(&self) -> bool {
        matches!(self.overlay, Overlay::Pause | Overlay::Settings)
    }

    pub fn item_count(&self) -> usize {
        match self.overlay {
            Overlay::None => 0,
            Overlay::Pause => PAUSE_ROWS,
            Overlay::Inventory => INVENTORY_SLOTS,
            Overlay::Settings => OPTS.len(),
            Overlay::Map => 0,
        }
    }

    fn go(&mut self, o: Overlay) {
        self.overlay = o;
        self.cursor = 0;
    }

    fn move_cursor(&mut self, delta: i8) {
        let n = self.item_count();
        if n == 0 {
            self.cursor = 0;
            return;
        }
        let cur = self.cursor as isize;
        self.cursor = (cur + delta as isize).rem_euclid(n as isize) as usize;
    }

    pub fn event(&mut self, ev: UiEvent) -> UiAction {
        // TogglePause is global: open the pause overlay, or close whatever is up.
        if ev == UiEvent::TogglePause {
            if self.overlay == Overlay::None {
                self.go(Overlay::Pause);
            } else {
                self.go(Overlay::None);
            }
            return UiAction::None;
        }
        match self.overlay {
            Overlay::None => {
                match ev {
                    UiEvent::OpenInventory => self.go(Overlay::Inventory),
                    UiEvent::OpenMap => self.go(Overlay::Map),
                    _ => {}
                }
                UiAction::None
            }
            Overlay::Pause => self.event_pause(ev),
            Overlay::Settings => self.event_settings(ev),
            Overlay::Inventory => self.event_inventory(ev),
            Overlay::Map => {
                if ev == UiEvent::Back {
                    self.go(Overlay::None);
                }
                UiAction::None
            }
        }
    }

    fn event_pause(&mut self, ev: UiEvent) -> UiAction {
        match ev {
            UiEvent::Nav(d) => { self.move_cursor(d); UiAction::None }
            UiEvent::Back => { self.go(Overlay::None); UiAction::Resume }
            UiEvent::Select => match self.cursor {
                0 => { self.go(Overlay::None); UiAction::Resume }
                1 => { self.go(Overlay::Settings); UiAction::None }
                2 => UiAction::Save,
                _ => UiAction::SaveAndQuit,
            },
            _ => UiAction::None,
        }
    }

    fn event_settings(&mut self, ev: UiEvent) -> UiAction {
        match ev {
            UiEvent::Back => { self.go(Overlay::Pause); UiAction::None }
            UiEvent::Select => { self.move_cursor(1); UiAction::None }
            UiEvent::Nav(d) => {
                let id = self.cursor.min(OPTS.len() - 1);
                let spec = &OPTS[id];
                let dir = if d >= 0 { 1 } else { -1 };
                let v = (self.options[id] + dir * spec.step).clamp(spec.min, spec.max);
                self.options[id] = v;
                UiAction::SetOption { id: id as u8, value: v }
            }
            _ => UiAction::None,
        }
    }

    fn event_inventory(&mut self, ev: UiEvent) -> UiAction {
        match ev {
            UiEvent::Nav(d) => { self.move_cursor(d); UiAction::None }
            UiEvent::Back | UiEvent::OpenInventory => { self.go(Overlay::None); UiAction::None }
            UiEvent::Select => UiAction::UseSlot(self.cursor as u8),
            _ => UiAction::None,
        }
    }
}

impl Default for IngameMenu {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_paused_matches_overlay_class() {
        let mut m = IngameMenu::new();
        assert!(!m.is_paused()); // None
        m.event(UiEvent::TogglePause);
        assert!(m.is_paused()); // Pause
        m.event(UiEvent::Nav(1)); // cursor to Settings
        m.event(UiEvent::Select);
        assert_eq!(m.overlay(), Overlay::Settings);
        assert!(m.is_paused());
        m.event(UiEvent::Back); // back to pause
        m.event(UiEvent::TogglePause); // close
        assert_eq!(m.overlay(), Overlay::None);

        let mut inv = IngameMenu::new();
        inv.event(UiEvent::OpenInventory);
        assert_eq!(inv.overlay(), Overlay::Inventory);
        assert!(!inv.is_paused(), "inventory is playable");
        let mut map = IngameMenu::new();
        map.event(UiEvent::OpenMap);
        assert!(!map.is_paused(), "map is playable");
    }

    #[test]
    fn toggle_pause_round_trips() {
        let mut m = IngameMenu::new();
        m.event(UiEvent::TogglePause);
        assert_eq!(m.overlay(), Overlay::Pause);
        m.event(UiEvent::TogglePause);
        assert_eq!(m.overlay(), Overlay::None);
    }

    #[test]
    fn pause_select_fires_each_action() {
        let mut m = IngameMenu::new();
        m.event(UiEvent::TogglePause); // Pause, cursor 0 = Resume
        assert_eq!(m.event(UiEvent::Select), UiAction::Resume);
        m.event(UiEvent::TogglePause); // Pause again
        m.event(UiEvent::Nav(2)); // cursor 2 = Save
        assert_eq!(m.event(UiEvent::Select), UiAction::Save);
        m.event(UiEvent::Nav(1)); // cursor 3 = SaveAndQuit
        assert_eq!(m.event(UiEvent::Select), UiAction::SaveAndQuit);
    }

    #[test]
    fn settings_back_unwinds_to_pause() {
        let mut m = IngameMenu::new();
        m.event(UiEvent::TogglePause);
        m.event(UiEvent::Nav(1));
        m.event(UiEvent::Select); // -> Settings
        m.event(UiEvent::Back);
        assert_eq!(m.overlay(), Overlay::Pause);
    }

    #[test]
    fn settings_adjusts_and_clamps() {
        let mut m = IngameMenu::new();
        m.event(UiEvent::TogglePause);
        m.event(UiEvent::Nav(1));
        m.event(UiEvent::Select); // Settings, cursor 0 = music
        let v0 = m.option(0);
        let a = m.event(UiEvent::Nav(1));
        assert_eq!(a, UiAction::SetOption { id: 0, value: v0 + 5 });
        for _ in 0..40 { m.event(UiEvent::Nav(1)); }
        assert_eq!(m.option(0), 100);
    }

    #[test]
    fn inventory_select_uses_slot() {
        let mut m = IngameMenu::new();
        m.event(UiEvent::OpenInventory);
        m.event(UiEvent::Nav(1));
        m.event(UiEvent::Nav(1));
        assert_eq!(m.event(UiEvent::Select), UiAction::UseSlot(2));
    }

    #[test]
    fn cursor_wraps_and_never_panics() {
        let events = [
            UiEvent::TogglePause, UiEvent::OpenInventory, UiEvent::OpenMap,
            UiEvent::Back, UiEvent::Select, UiEvent::Nav(1), UiEvent::Nav(-1),
        ];
        let mut m = IngameMenu::new();
        let mut s = 1u64;
        for _ in 0..4000 {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            let _ = m.event(events[(s >> 33) as usize % events.len()]);
            assert!(m.cursor() < m.item_count().max(1));
        }
    }
}
