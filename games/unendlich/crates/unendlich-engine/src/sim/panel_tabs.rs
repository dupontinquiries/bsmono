//! sim::panel_tabs — the character screen's top-level tab bar.
//!
//! From `character_panel_reference §2.2/§7`: the header's right side holds the
//! navigation tabs that switch which full-screen panel is shown — CHARACTER,
//! CLAN (dojo), COLLECTIONS (formation lexicon + mob viewer), JOURNEY (quest
//! chains), INVENTORY. This is the small state machine that tracks the active
//! tab and cycles between them. It sits *above* `sim::menu_nav` (which drives the
//! cursor *within* a panel): tabs switch panels, menu_nav navigates one. Some
//! tabs can be unavailable (CLAN before joining a dojo), so cycling can skip
//! disabled tabs via a caller-supplied predicate. Deterministic, std-only.

/// The five top-level panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PanelTab {
    Character,
    Clan,
    Collections,
    Journey,
    Inventory,
}

impl PanelTab {
    /// Tabs in display order (left to right).
    pub const ALL: [PanelTab; 5] = [
        PanelTab::Character,
        PanelTab::Clan,
        PanelTab::Collections,
        PanelTab::Journey,
        PanelTab::Inventory,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PanelTab::Character => "CHARACTER",
            PanelTab::Clan => "CLAN",
            PanelTab::Collections => "COLLECTIONS",
            PanelTab::Journey => "JOURNEY",
            PanelTab::Inventory => "INVENTORY",
        }
    }

    pub fn index(self) -> usize {
        PanelTab::ALL.iter().position(|&t| t == self).unwrap()
    }

    pub fn from_index(i: usize) -> Option<PanelTab> {
        PanelTab::ALL.get(i).copied()
    }
}

/// Tracks the active tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabBar {
    active: PanelTab,
}

impl Default for TabBar {
    fn default() -> Self {
        TabBar { active: PanelTab::Character }
    }
}

impl TabBar {
    pub fn new() -> TabBar {
        TabBar::default()
    }

    pub fn active(&self) -> PanelTab {
        self.active
    }
    pub fn index(&self) -> usize {
        self.active.index()
    }

    /// Jump to a specific tab.
    pub fn set(&mut self, tab: PanelTab) {
        self.active = tab;
    }

    /// Move to the next tab, wrapping.
    pub fn next(&mut self) {
        let n = PanelTab::ALL.len();
        self.active = PanelTab::from_index((self.active.index() + 1) % n).unwrap();
    }

    /// Move to the previous tab, wrapping.
    pub fn prev(&mut self) {
        let n = PanelTab::ALL.len();
        self.active = PanelTab::from_index((self.active.index() + n - 1) % n).unwrap();
    }

    /// Next *enabled* tab, skipping disabled ones (wraps). No-op if nothing else
    /// is enabled.
    pub fn next_enabled(&mut self, is_enabled: impl Fn(PanelTab) -> bool) {
        self.step_enabled(1, is_enabled);
    }

    /// Previous *enabled* tab, skipping disabled ones (wraps).
    pub fn prev_enabled(&mut self, is_enabled: impl Fn(PanelTab) -> bool) {
        self.step_enabled(PanelTab::ALL.len() - 1, is_enabled);
    }

    fn step_enabled(&mut self, delta: usize, is_enabled: impl Fn(PanelTab) -> bool) {
        let n = PanelTab::ALL.len();
        let start = self.active.index();
        for k in 1..=n {
            let cand = PanelTab::from_index((start + delta * k) % n).unwrap();
            if is_enabled(cand) {
                self.active = cand;
                return;
            }
        }
        // none enabled elsewhere ⇒ stay put
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_in_order_with_labels() {
        assert_eq!(PanelTab::ALL[0], PanelTab::Character);
        assert_eq!(PanelTab::ALL[4], PanelTab::Inventory);
        for t in PanelTab::ALL {
            assert!(!t.label().is_empty());
            assert_eq!(PanelTab::from_index(t.index()), Some(t));
        }
        assert_eq!(PanelTab::from_index(5), None);
    }

    #[test]
    fn default_is_character() {
        assert_eq!(TabBar::new().active(), PanelTab::Character);
    }

    #[test]
    fn next_and_prev_wrap() {
        let mut b = TabBar::new();
        b.next();
        assert_eq!(b.active(), PanelTab::Clan);
        b.set(PanelTab::Inventory);
        b.next();
        assert_eq!(b.active(), PanelTab::Character, "wraps to start");
        b.prev();
        assert_eq!(b.active(), PanelTab::Inventory, "wraps back to end");
    }

    #[test]
    fn set_jumps_directly() {
        let mut b = TabBar::new();
        b.set(PanelTab::Journey);
        assert_eq!(b.active(), PanelTab::Journey);
        assert_eq!(b.index(), 3);
    }

    #[test]
    fn enabled_cycling_skips_disabled_tabs() {
        let mut b = TabBar::new(); // Character
        // CLAN and COLLECTIONS disabled ⇒ next enabled from Character is JOURNEY.
        let enabled = |t: PanelTab| !matches!(t, PanelTab::Clan | PanelTab::Collections);
        b.next_enabled(enabled);
        assert_eq!(b.active(), PanelTab::Journey);
        // prev enabled from Journey skips back over the disabled pair to Character.
        b.prev_enabled(enabled);
        assert_eq!(b.active(), PanelTab::Character);
    }

    #[test]
    fn enabled_cycling_stays_when_nothing_else_enabled() {
        let mut b = TabBar::new();
        b.next_enabled(|t| t == PanelTab::Character); // only current enabled
        assert_eq!(b.active(), PanelTab::Character);
    }

    #[test]
    fn deterministic() {
        let mut a = TabBar::new();
        let mut c = TabBar::new();
        a.next();
        c.next();
        assert_eq!(a, c);
    }
}
