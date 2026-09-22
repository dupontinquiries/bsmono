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
/// `Ord` so a `BTreeSet` can hold the enabled set — derived, so the order is
/// declaration order, which is also the order the bar draws them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Control {
    /// Open the settings page. On the board because every page needs a way
    /// there and a keyboard has no window chrome to put it in.
    Settings,
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
        Control::Settings,
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
        // LayoutSwitch is NO LONGER HERE, and that is a reversal worth stating.
        //
        // I argued it belonged on the board because switching layout is
        // answering "this key is missing" mid-typing. That was wrong about
        // what the control IS. The sidebar it opened was a picker over the
        // same two choices `settings_model` already exposes as
        // `layout.letters` and `layout.geometry` -- a second UI for one
        // setting, costing 64pt of a 380pt screen to duplicate a row in a
        // page that has to exist anyway.
        //
        // Choosing a layout is not a mid-typing act; it is something a person
        // does once and lives with. The mid-typing case -- one missing key --
        // is what a symbol layer is for, not a layout swap.
        matches!(self, Control::Dictate | Control::OptionsToggle | Control::Settings)
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
            Control::Settings => "settings",
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

/// The top bar's height once any feature is enabled.
///
/// 36pt: under the 44pt touch target, deliberately. These are not primary
/// controls — the primary ones are on the board itself — and a bar that
/// matched the board's target would claim to be as important as the keys.
/// A 36pt row is comfortably hittable for something you reach for
/// occasionally, and costs 8pt less of the keys' budget than a bar that
/// pretended otherwise.
pub const BAR_H_COMPACT: f32 = 36.0;

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
    // The layout sidebar is GONE, not collapsed.
    //
    // It was a 64pt column duplicating `layout.letters` and
    // `layout.geometry` from the settings page. Collapsing it by default (the
    // previous fix) made it cheap; removing it makes the duplication
    // impossible, which is the difference between a tidy screen and one
    // setting with one home.
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

    /// Height the top bar should take, given how many controls it must carry.
    ///
    /// ## Zero by default
    ///
    /// A keyboard's job is keys. The bar carried tray / settings / term /
    /// dictate on every frame, costing vertical space the keys needed, for
    /// controls most sessions never touch — and on a 380pt phone that space is
    /// the difference between a comfortable row and a cramped one.
    ///
    /// So nothing is enabled by default and the bar is not drawn at all. Not a
    /// thin strip: a bar with no controls in it is chrome advertising itself,
    /// and the gesture surface that reaches the options layer does not need a
    /// visible handle to exist.
    ///
    /// ## And SHRUNK, not full-size, once something is enabled
    ///
    /// When a feature is turned on the bar appears at [`BAR_H_COMPACT`], not
    /// at the comfortable height a settings page would use. The reasoning is
    /// that someone who enabled a top-bar feature wants the feature, not the
    /// bar: they have already told us the keys matter more, by keeping the
    /// other controls off.
    ///
    /// `enabled` is how many top-bar features the person switched on.
    pub fn top_bar_height(enabled: usize) -> f32 {
        if enabled == 0 {
            0.0
        } else {
            BAR_H_COMPACT
        }
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
                   vec![Control::Settings, Control::Dictate, Control::OptionsToggle]);
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

    /// THE SIDEBAR IS GONE, not collapsed. It duplicated `layout.letters` and
    /// `layout.geometry`, which the settings page already owns.
    #[test]
    fn no_board_control_opens_a_layout_picker() {
        let names: Vec<&str> = Chrome::new().board_controls().iter().map(|c| c.label()).collect();
        assert!(!names.contains(&"layout"),
                "layout belongs in settings, not on the board: {names:?}");
    }

    /// Every page needs a way to settings, and a keyboard has no window chrome
    /// to put it in -- so the button is on the board itself.
    #[test]
    fn settings_is_reachable_from_the_board() {
        assert!(Chrome::new().board_controls().contains(&Control::Settings));
        assert!(Control::Settings.needs_keys_visible(),
                "the way in must not hide the thing you are configuring");
    }

    /// A KEYBOARD'S JOB IS KEYS. Nothing enabled means no bar at all -- not a
    /// thin strip, which would be chrome advertising itself.
    #[test]
    fn the_top_bar_is_absent_until_a_feature_is_enabled() {
        assert_eq!(Chrome::top_bar_height(0), 0.0);
        assert!(Chrome::top_bar_height(1) > 0.0);
    }

    /// Someone who enabled a top-bar feature wants the FEATURE, not the bar --
    /// they already said the keys matter more by leaving the rest off. So it
    /// appears compact and stays compact however many they enable.
    #[test]
    fn the_bar_is_compact_and_does_not_grow_with_the_feature_count() {
        let one = Chrome::top_bar_height(1);
        assert_eq!(one, BAR_H_COMPACT);
        for n in [2, 3, 9] {
            assert_eq!(Chrome::top_bar_height(n), one, "{n} features changed the height");
        }
    }

    /// Under the 44pt touch target on purpose: these are not primary controls,
    /// and a bar matching the board's target would claim to be as important as
    /// the keys.
    #[test]
    fn the_compact_bar_is_shorter_than_a_primary_touch_target() {
        assert!(BAR_H_COMPACT < 44.0, "{BAR_H_COMPACT}");
        assert!(BAR_H_COMPACT >= 32.0, "still hittable: {BAR_H_COMPACT}");
    }
}
