//! chrome — what the keyboard shows, and what belongs in another window.
//!
//! The board grew a top bar carrying tray, settings, term, dictate and a
//! status line, laid out with `ui.horizontal`, which does not wrap. On a
//! 380 px phone that is both too tall — vertical budget the keys needed — and
//! too wide, so the right-hand controls sit off screen where nothing can reach
//! them.
//!
//! ## The rule that decides placement
//!
//! Not "what is used often". That sounds right and puts everything back on the
//! bar, because everything feels frequent to whoever added it.
//!
//! The rule is: **does this need the keyboard on screen while you use it?**
//!
//! * Switching layout is answering "this key is missing" — you are mid-typing,
//!   and a control that hides the board to change the board is useless. Stays.
//! * Dictation toggles mid-sentence. Stays.
//! * Choosing a theme, a gesture threshold, a corpus path: you look at the
//!   setting, not at the keys. A separate window is BETTER, because it gets a
//!   whole screen instead of a strip.
//!
//! That rule has a useful property: it produces a small, stable set for the
//! board, and it answers the next argument about where something goes without
//! anyone re-litigating it.
//!
//! ## Where the separate window comes from
//!
//! Not a new shell. `gitmind-egui` already has the exact thing — a tabbed
//! settings page (`Tab::Settings` → `pages::settings`) backed by
//! `settings_kit::Prefs` — and rusty-board ALREADY DEPENDS on `settings-kit`
//! too, so both apps are reading and writing the same preference store today.
//! What rusty-board lacks is the surface, not the storage.
//!
//! So [`Home::Settings`] controls are the ones to lift into a
//! gitmind-shaped tabbed page: same kit, same persistence, same deployment
//! path, and a board setting changed in either place means the same thing. The
//! alternative — a bespoke settings window here — would be a second way to
//! write the same file.
//!
//! `settings_model` stays where it is and is not replaced. It is the ported
//! GTK panel's MODEL, it is what the in-keyboard options layer drives, and it
//! is the right thing for the two or three choices that belong on the board.
//! The window is for the rest.
//!
//! ## The settings button is a layout swap, not a panel
//!
//! Tapping it replaces the keys with basic options IN THE SAME SPACE, rather
//! than opening a panel that pushes the board around. A keyboard whose height
//! changes when you open something is a keyboard that moves the text field
//! under your thumb.

/// Where a control can live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// On the board itself. Costs vertical space on every frame, so the bar
    /// for being here is high.
    Board,
    /// The basic-options layer, reached by the layout swap. Still on the
    /// keyboard's surface, so still usable with the text field in view.
    Options,
    /// The separate settings window.
    Settings,
}

/// Everything the chrome can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Switch keyboard layout / language.
    LayoutSwitch,
    /// Start or stop dictation.
    Dictate,
    /// Open the basic-options layer.
    OptionsToggle,
    /// The clipboard tray.
    Tray,
    /// The embedded terminal.
    Term,
    /// Theme and colours.
    Theme,
    /// Gesture thresholds and timings.
    GestureTuning,
    /// Corpus and training paths.
    Corpus,
    /// Diagnostics and logs.
    Diagnostics,
}

impl Control {
    pub const ALL: [Control; 9] = [
        Control::LayoutSwitch,
        Control::Dictate,
        Control::OptionsToggle,
        Control::Tray,
        Control::Term,
        Control::Theme,
        Control::GestureTuning,
        Control::Corpus,
        Control::Diagnostics,
    ];

    /// Must the keyboard be visible while this is used?
    ///
    /// The whole placement rule, in one predicate, so it can be argued with
    /// once rather than per control.
    pub fn needs_keys_visible(self) -> bool {
        matches!(self, Control::LayoutSwitch | Control::Dictate | Control::OptionsToggle)
    }

    pub fn home(self) -> Home {
        if self.needs_keys_visible() {
            return Home::Board;
        }
        match self {
            // The tray pastes INTO the field you are typing in, so it wants
            // the field in view even though it does not want the keys. The
            // options layer is that compromise: same surface, keys hidden.
            Control::Tray | Control::Term => Home::Options,
            _ => Home::Settings,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Control::LayoutSwitch => "layout",
            Control::Dictate => "dictate",
            Control::OptionsToggle => "options",
            Control::Tray => "tray",
            Control::Term => "term",
            Control::Theme => "theme",
            Control::GestureTuning => "gestures",
            Control::Corpus => "corpus",
            Control::Diagnostics => "diagnostics",
        }
    }
}

/// How many controls the board may show at once.
///
/// Three, and it is a hard cap rather than a guideline. At a 44 pt touch target
/// plus gutters, a fourth does not fit across 380 px without shrinking the
/// targets below what a thumb can hit — which is how the old bar ended up with
/// controls off screen rather than merely crowded.
pub const BOARD_MAX: usize = 3;

/// What the keyboard surface is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layer {
    #[default]
    Keys,
    Options,
}

/// The chrome's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Chrome {
    layer: Layer,
    /// Whether the separate settings window is open.
    settings_window: bool,
    /// Whether the layout sidebar is expanded.
    ///
    /// COLLAPSED BY DEFAULT, because it is a switcher: you change layout a few
    /// times a session and it was costing 64 px of a 380 px phone on every
    /// frame — 17% of the width, permanently, for a control that is idle
    /// almost always. Its collapsed affordance is `Control::LayoutSwitch`,
    /// which is already on the board, so nothing new has to be found room for.
    sidebar: bool,
}

impl Chrome {
    pub fn new() -> Chrome {
        Chrome::default()
    }

    pub fn layer(&self) -> Layer {
        self.layer
    }
    pub fn settings_window_open(&self) -> bool {
        self.settings_window
    }

    /// Controls to draw on the board right now.
    ///
    /// On the options layer the board's own controls stay put: the swap
    /// replaces the KEYS, not the chrome, so the way back is always where the
    /// way in was.
    pub fn board_controls(&self) -> Vec<Control> {
        Control::ALL.iter().copied().filter(|c| c.home() == Home::Board).collect()
    }

    /// Controls on the options layer. Empty while showing keys.
    pub fn options_controls(&self) -> Vec<Control> {
        if self.layer != Layer::Options {
            return Vec::new();
        }
        Control::ALL.iter().copied().filter(|c| c.home() == Home::Options).collect()
    }

    /// Controls that belong in the separate window.
    pub fn settings_controls(&self) -> Vec<Control> {
        Control::ALL.iter().copied().filter(|c| c.home() == Home::Settings).collect()
    }

    pub fn sidebar_expanded(&self) -> bool {
        self.sidebar
    }

    /// Tapping the layout control.
    pub fn toggle_sidebar(&mut self) {
        self.sidebar = !self.sidebar;
    }

    /// A layout was chosen.
    ///
    /// Collapses, because you opened the switcher to pick ONE thing and it has
    /// been picked. Leaving it open would keep the board narrow while you type
    /// with the layout you just chose, which is the worst of both.
    pub fn layout_chosen(&mut self) {
        self.sidebar = false;
    }

    /// Width the sidebar should occupy, given its expanded width.
    ///
    /// Zero when collapsed — not a narrow strip. A 12 px stub is too small to
    /// hit and still eats width, so it manages to be both useless and costly;
    /// the board control is the affordance.
    pub fn sidebar_width(&self, expanded_w: f32) -> f32 {
        if self.sidebar { expanded_w.max(0.0) } else { 0.0 }
    }

    pub fn toggle_options(&mut self) {
        self.layer = match self.layer {
            Layer::Keys => Layer::Options,
            Layer::Options => Layer::Keys,
        };
    }

    /// Open the settings window.
    ///
    /// Returns the board to KEYS. Opening a separate window means attention
    /// has left the board, and coming back to a keyboard showing an options
    /// layer you no longer remember opening is disorienting — the board should
    /// be a keyboard whenever you look at it fresh.
    pub fn open_settings_window(&mut self) {
        self.settings_window = true;
        self.layer = Layer::Keys;
        // Same reasoning: nothing half-open should be waiting when attention
        // comes back to the board.
        self.sidebar = false;
    }

    pub fn close_settings_window(&mut self) {
        self.settings_window = false;
    }

    /// The board must never be taller than it has to be: a control that is not
    /// drawn costs nothing, and this is the check that the set fits.
    pub fn board_fits(&self) -> bool {
        self.board_controls().len() <= BOARD_MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE CAP. A fourth control does not fit across 380 px at a 44 pt target,
    /// which is how the old bar ended up with buttons off screen.
    #[test]
    fn the_board_shows_no_more_than_three_controls() {
        let c = Chrome::new();
        assert!(c.board_fits(), "board has {:?}", c.board_controls());
        assert_eq!(c.board_controls().len(), BOARD_MAX);
    }

    /// The rule, stated as a test so the next person arguing for "just one
    /// more button on the bar" has to argue with the rule instead.
    #[test]
    fn only_controls_that_need_the_keys_visible_live_on_the_board() {
        for c in Control::ALL {
            assert_eq!(c.home() == Home::Board, c.needs_keys_visible(),
                       "{c:?} is misplaced");
        }
        // The three that qualify, named, so a change here is deliberate.
        assert_eq!(Chrome::new().board_controls(),
                   vec![Control::LayoutSwitch, Control::Dictate, Control::OptionsToggle]);
    }

    /// Configuration goes to the window, where it gets a screen instead of a
    /// strip.
    #[test]
    fn configuration_lives_in_the_separate_window() {
        let s = Chrome::new().settings_controls();
        for c in [Control::Theme, Control::GestureTuning, Control::Corpus,
                  Control::Diagnostics] {
            assert!(s.contains(&c), "{c:?} should be in the settings window");
        }
        assert!(!s.contains(&Control::Dictate), "dictation toggles mid-sentence");
    }

    /// The tray pastes INTO the field you are typing in, so it wants the field
    /// in view even though it does not want the keys — which is exactly what
    /// the options layer is for.
    #[test]
    fn the_tray_and_terminal_sit_on_the_options_layer() {
        let mut c = Chrome::new();
        assert!(c.options_controls().is_empty(), "nothing while showing keys");
        c.toggle_options();
        let o = c.options_controls();
        assert!(o.contains(&Control::Tray) && o.contains(&Control::Term));
        assert!(!o.contains(&Control::Theme), "theme is not an in-typing concern");
    }

    /// THE SWAP REPLACES THE KEYS, NOT THE CHROME. The way back has to be
    /// where the way in was.
    #[test]
    fn the_board_controls_survive_the_layout_swap() {
        let mut c = Chrome::new();
        let before = c.board_controls();
        c.toggle_options();
        assert_eq!(c.layer(), Layer::Options);
        assert_eq!(c.board_controls(), before, "the way back must not move");
        c.toggle_options();
        assert_eq!(c.layer(), Layer::Keys);
    }

    /// Coming back to a keyboard showing an options layer you no longer
    /// remember opening is disorienting.
    #[test]
    fn opening_the_settings_window_returns_the_board_to_keys() {
        let mut c = Chrome::new();
        c.toggle_options();
        c.open_settings_window();
        assert!(c.settings_window_open());
        assert_eq!(c.layer(), Layer::Keys, "the board is a keyboard when you look back");
    }

    /// Every control has exactly one home, so nothing is duplicated or lost.
    #[test]
    fn every_control_is_placed_exactly_once() {
        let mut c = Chrome::new();
        c.toggle_options();
        let mut all: Vec<Control> = c.board_controls();
        all.extend(c.options_controls());
        all.extend(c.settings_controls());
        assert_eq!(all.len(), Control::ALL.len(), "placed {all:?}");
        for x in Control::ALL {
            assert_eq!(all.iter().filter(|y| **y == x).count(), 1, "{x:?}");
        }
    }

    /// COLLAPSED BY DEFAULT. 64 px of a 380 px phone, permanently, for a
    /// control used a few times a session.
    #[test]
    fn the_sidebar_starts_collapsed_and_costs_nothing() {
        let c = Chrome::new();
        assert!(!c.sidebar_expanded());
        assert_eq!(c.sidebar_width(64.0), 0.0, "collapsed means zero, not a stub");
    }

    /// Zero rather than a narrow strip: a 12 px stub is too small to hit and
    /// still eats width, which is both useless and costly.
    #[test]
    fn expanding_costs_only_while_it_is_open() {
        let mut c = Chrome::new();
        c.toggle_sidebar();
        assert_eq!(c.sidebar_width(64.0), 64.0);
        c.toggle_sidebar();
        assert_eq!(c.sidebar_width(64.0), 0.0);
    }

    /// You opened the switcher to pick one thing. Leaving it open keeps the
    /// board narrow while you type with the layout you just chose.
    #[test]
    fn choosing_a_layout_collapses_the_sidebar() {
        let mut c = Chrome::new();
        c.toggle_sidebar();
        c.layout_chosen();
        assert!(!c.sidebar_expanded());
    }

    /// Nothing half-open waits for you when attention comes back.
    #[test]
    fn opening_the_settings_window_collapses_everything_on_the_board() {
        let mut c = Chrome::new();
        c.toggle_sidebar();
        c.toggle_options();
        c.open_settings_window();
        assert!(!c.sidebar_expanded());
        assert_eq!(c.layer(), Layer::Keys);
    }
}
