//! chrome_mode — trading visible controls for screen.
//!
//! ## What this is not
//!
//! It is not a fidelity fix. The port was described as unfaithful for using
//! tabs where the Qt app uses voice, and that is not what the source says:
//! `main.qml` has five `TabBar`/`TabButton` sites and drives them from voice
//! *as well* — "For programmatic switches (Ctrl-1/Ctrl-2/voice/clicks)" is a
//! comment in the QML itself. Both apps have both. Checked before building,
//! because the fix for a problem that does not exist is usually a new problem.
//!
//! ## What it is
//!
//! The observation underneath was right and is worth more than the premise: on
//! a phone, once someone's voice setup works, the tab strip and the button rows
//! are spending scarce height on affordances they have stopped using. A
//! terminal is the one app where that height converts directly into
//! information — rows of output.
//!
//! So `ChromeMode` trades them. Touch shows everything; Voice hides the
//! strip and the button rows and gives the space to the panes.
//!
//! ## Three rules that keep it from being a trap
//!
//! **1. It never switches itself.** Not on a successful recognition, not after
//! N voice commands, not on a confidence threshold. A UI that removes its own
//! controls based on a guess about the user is the worst version of this idea —
//! the one time the guess is wrong, the controls are gone and the user cannot
//! ask for them back by the means that just failed.
//!
//! **2. There is always a way back that does not need voice.** If the
//! recogniser stops working — a noisy train, a failed model load, a muted mic —
//! voice mode must not be a room with no door. The escape is a chord, because a
//! chord cannot be produced by the ambient taps that a phone in a pocket
//! generates.
//!
//! **3. Something still says where you are.** Hiding the strip removes the only
//! indicator of which tab is active, and a terminal where you cannot tell which
//! of four shells you are typing into is worse than one with a fat tab bar. A
//! single line survives: a locator, not a strip.

/// Height of the tab strip, in points.
pub const STRIP_H: f32 = 34.0;
/// Height of the button rows below it.
pub const BUTTONS_H: f32 = 44.0;
/// Height of the minimal locator that survives in voice mode.
pub const LOCATOR_H: f32 = 14.0;

/// Default width of the vertical tab column, from `main.qml`'s
/// `property real termSidebarWidth: 168` (main.qml:4015).
///
/// Width, not height, and that is the point: this module measured chrome in
/// points of HEIGHT only, so when the tab column was brought under the same
/// gate as the strip and the buttons, `reclaimed()` did not notice a 168-point
/// column disappearing. On a phone held in portrait that is the more valuable
/// axis — a terminal wraps, so a narrower pane costs every line, while a
/// shorter one costs the oldest lines.
pub const TAB_COLUMN_W: f32 = 168.0;

/// How much chrome is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChromeMode {
    /// Tab strip and button rows. The default, because a new user has not set
    /// voice up yet and cannot be assumed to have.
    #[default]
    Touch,
    /// Locator only. Everything else is spoken.
    Voice,
}

impl ChromeMode {
    pub fn toggled(self) -> ChromeMode {
        match self { ChromeMode::Touch => ChromeMode::Voice, ChromeMode::Voice => ChromeMode::Touch }
    }

    /// Chrome height in this mode.
    pub fn chrome_height(self) -> f32 {
        match self {
            ChromeMode::Touch => STRIP_H + BUTTONS_H,
            ChromeMode::Voice => LOCATOR_H,
        }
    }

    /// Height handed back to the panes by switching to voice.
    pub fn reclaimed(self) -> f32 {
        ChromeMode::Touch.chrome_height() - self.chrome_height()
    }

    /// Width handed back by hiding the vertical tab column.
    ///
    /// Separate from [`reclaimed`] rather than folded into it, because they are
    /// different axes and adding them would produce a number in no unit at all.
    pub fn reclaimed_width(self) -> f32 {
        match self {
            ChromeMode::Touch => 0.0,
            ChromeMode::Voice => TAB_COLUMN_W,
        }
    }

    /// Roughly how many terminal COLUMNS that is, at a given character width.
    ///
    /// The same honest-unit argument as `reclaimed_rows`: "168 points" means
    /// nothing, "twenty more columns before the line wraps" is the trade.
    pub fn reclaimed_cols(self, char_w: f32) -> u32 {
        if char_w <= 0.0 { return 0; }
        (self.reclaimed_width() / char_w).floor().max(0.0) as u32
    }

    /// Roughly how many terminal rows that is, at a given row height.
    ///
    /// The honest unit. "Reclaims 64 points" means nothing; "four more rows of
    /// output" is the thing the user is actually trading for.
    pub fn reclaimed_rows(self, row_h: f32) -> u32 {
        if row_h <= 0.0 { return 0; }
        (self.reclaimed() / row_h).floor().max(0.0) as u32
    }

    pub fn shows_tab_strip(self) -> bool { self == ChromeMode::Touch }
    pub fn shows_buttons(self) -> bool { self == ChromeMode::Touch }
    /// True in BOTH modes. The locator is what stops voice mode from being
    /// disorienting; removing it would save fourteen points and cost the user
    /// their place.
    pub fn shows_locator(self) -> bool { true }
}

/// The escape chord, for when the recogniser has stopped working.
///
/// Ctrl+Shift+M: a chord rather than a tap or a swipe, because a phone in a
/// pocket generates taps and swipes constantly and must not be able to restore
/// the chrome — or, worse, remove it — by accident.
pub const ESCAPE_HINT: &str = "Ctrl+Shift+M";

/// The toggle, and what it needs to remember.
#[derive(Debug, Clone, PartialEq)]
pub struct Chrome {
    mode: ChromeMode,
    /// The user's stored preference, which is not the same as the live mode:
    /// an escape restores the chrome without discarding the fact that they
    /// asked for voice mode.
    preferred: ChromeMode,
    /// True while the chrome is showing only because the escape was used.
    escaped: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Chrome { mode: ChromeMode::Touch, preferred: ChromeMode::Touch, escaped: false }
    }
}

impl Chrome {
    /// Restore from settings.
    pub fn with_preference(preferred: ChromeMode) -> Chrome {
        Chrome { mode: preferred, preferred, escaped: false }
    }

    pub fn mode(&self) -> ChromeMode { self.mode }
    pub fn preferred(&self) -> ChromeMode { self.preferred }
    pub fn escaped(&self) -> bool { self.escaped }

    /// The user asked to switch. This is the ONLY way the mode changes on
    /// purpose, and it updates the stored preference.
    pub fn toggle(&mut self) {
        self.mode = self.mode.toggled();
        self.preferred = self.mode;
        self.escaped = false;
    }

    /// The escape chord: bring the chrome back without forgetting the
    /// preference.
    ///
    /// Returns false when there was nothing to escape from, so a caller does
    /// not report an escape that did nothing.
    pub fn escape(&mut self) -> bool {
        if self.mode == ChromeMode::Touch { return false; }
        self.mode = ChromeMode::Touch;
        self.escaped = true;
        true
    }

    /// Go back to the preferred mode after an escape.
    pub fn resume(&mut self) -> bool {
        if !self.escaped { return false; }
        self.mode = self.preferred;
        self.escaped = false;
        true
    }

    /// What the locator should read.
    ///
    /// Tab position, its title, and the pane count when there is more than one.
    /// The pane count is conditional because "1 pane" is noise on the common
    /// case and the number only matters once a split exists.
    pub fn locator(&self, tab_index: usize, tab_count: usize, title: &str,
                   panes: usize) -> String {
        let name = if title.trim().is_empty() { "shell" } else { title.trim() };
        let base = format!("{}/{} {}", tab_index + 1, tab_count.max(1), name);
        if panes > 1 { format!("{base} · {panes} panes") } else { base }
    }
}

/// Draw the locator line.
pub fn locator_ui(ui: &mut egui::Ui, text: &str, escaped: bool,
                  c: &uikit_egui::PaletteColors) {
    ui.horizontal(|ui| {
        ui.colored_label(c.text_weak, egui::RichText::new(text).small());
        if escaped {
            // Says WHY the chrome is back, so the user does not think the
            // setting was lost.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.warning,
                    egui::RichText::new("chrome restored").small());
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_locator_is_actually_drawn() {
        // shows_locator() is true in both modes and this module was ported
        // with the escape chord, the reclaimed-rows maths and its tests — and
        // nothing ever called locator_ui, so voice mode had no indication of
        // which tab you were in. Exactly the disorientation the module says it
        // exists to prevent.
        let bin = include_str!("bin/voiceterm_egui.rs");
        assert!(bin.contains("chrome_mode::locator_ui("), "the locator is not drawn");
        assert!(bin.contains("self.chrome.escaped()"),
                "the chrome-restored notice cannot appear");
    }

    #[test]
    fn the_locator_survives_the_mode_that_hides_everything_else() {
        // Voice mode removes the tab strip and the buttons. If the locator
        // went with them the user would have no place-marker at all, which is
        // the whole argument for it costing fourteen points.
        let mut c = Chrome::default();
        c.toggle();
        assert!(!c.mode().shows_tab_strip());
        assert!(!c.mode().shows_buttons());
        assert!(c.mode().shows_locator(), "voice mode with no locator is a room with no map");
        assert!(!c.locator(0, 3, "build", 1).is_empty());
    }

    #[test]
    fn touch_is_the_default_because_a_new_user_has_not_set_voice_up() {
        assert_eq!(ChromeMode::default(), ChromeMode::Touch);
        assert!(ChromeMode::default().shows_tab_strip());
    }

    #[test]
    fn voice_mode_gives_the_height_to_the_panes() {
        assert!(ChromeMode::Voice.chrome_height() < ChromeMode::Touch.chrome_height());
        assert_eq!(ChromeMode::Voice.reclaimed(), STRIP_H + BUTTONS_H - LOCATOR_H);
        assert_eq!(ChromeMode::Touch.reclaimed(), 0.0);
    }

    #[test]
    fn the_gain_is_reported_in_rows_not_points() {
        // "Reclaims 64 points" means nothing; "four more rows of output" is
        // what the user is actually trading for.
        assert_eq!(ChromeMode::Voice.reclaimed_rows(16.0), 4);
        assert_eq!(ChromeMode::Touch.reclaimed_rows(16.0), 0);
    }

    #[test]
    fn a_zero_row_height_does_not_divide_by_zero() {
        assert_eq!(ChromeMode::Voice.reclaimed_rows(0.0), 0);
    }

    #[test]
    fn the_mode_never_switches_itself() {
        // A UI that removes its own controls on a guess about the user is the
        // worst version of this idea: the one time the guess is wrong, the
        // controls are gone and they cannot ask for them back by the means that
        // just failed. Only toggle() changes the mode.
        let mut c = Chrome::default();
        assert_eq!(c.mode(), ChromeMode::Touch);
        // Nothing else here can move it — there is no `on_voice_success` and no
        // confidence threshold, deliberately.
        c.escape();
        assert_eq!(c.mode(), ChromeMode::Touch);
    }

    #[test]
    fn toggling_switches_and_remembers() {
        let mut c = Chrome::default();
        c.toggle();
        assert_eq!(c.mode(), ChromeMode::Voice);
        assert_eq!(c.preferred(), ChromeMode::Voice);
    }

    #[test]
    fn the_escape_restores_the_chrome_without_forgetting_the_preference() {
        // If the recogniser stops working — a noisy train, a failed model load,
        // a muted mic — voice mode must not be a room with no door. But
        // escaping is not the same as changing your mind.
        let mut c = Chrome::default();
        c.toggle();
        assert!(c.escape());
        assert_eq!(c.mode(), ChromeMode::Touch);
        assert_eq!(c.preferred(), ChromeMode::Voice, "still what they asked for");
        assert!(c.escaped());
    }

    #[test]
    fn escaping_when_the_chrome_is_already_shown_reports_nothing_happened() {
        // So a caller does not announce an escape that did nothing.
        let mut c = Chrome::default();
        assert!(!c.escape());
        assert!(!c.escaped());
    }

    #[test]
    fn resuming_returns_to_the_preferred_mode() {
        let mut c = Chrome::default();
        c.toggle();
        c.escape();
        assert!(c.resume());
        assert_eq!(c.mode(), ChromeMode::Voice);
        assert!(!c.escaped());
    }

    #[test]
    fn resuming_without_an_escape_does_nothing() {
        let mut c = Chrome::default();
        c.toggle();
        assert!(!c.resume());
        assert_eq!(c.mode(), ChromeMode::Voice);
    }

    #[test]
    fn toggling_after_an_escape_clears_the_escaped_state() {
        // Otherwise the "chrome restored" note would linger over a mode the
        // user has since chosen deliberately.
        let mut c = Chrome::default();
        c.toggle();
        c.escape();
        c.toggle();
        assert!(!c.escaped());
        assert_eq!(c.preferred(), ChromeMode::Voice);
    }

    #[test]
    fn a_stored_preference_is_restored_without_counting_as_an_escape() {
        let c = Chrome::with_preference(ChromeMode::Voice);
        assert_eq!(c.mode(), ChromeMode::Voice);
        assert!(!c.escaped());
    }

    #[test]
    fn the_locator_survives_in_both_modes() {
        // Hiding the strip removes the only indicator of which tab is active,
        // and a terminal where you cannot tell which of four shells you are
        // typing into is worse than one with a fat tab bar.
        assert!(ChromeMode::Touch.shows_locator());
        assert!(ChromeMode::Voice.shows_locator());
    }

    #[test]
    fn the_locator_names_the_position_and_the_tab() {
        let c = Chrome::default();
        assert_eq!(c.locator(0, 3, "build", 1), "1/3 build");
        assert_eq!(c.locator(2, 3, "logs", 1), "3/3 logs");
    }

    #[test]
    fn the_pane_count_appears_only_once_there_is_a_split() {
        // "1 pane" is noise on the common case; the number only matters once
        // there is more than one place your keystrokes could go.
        let c = Chrome::default();
        assert!(!c.locator(0, 1, "shell", 1).contains("pane"));
        assert!(c.locator(0, 1, "shell", 3).contains("3 panes"));
    }

    #[test]
    fn an_untitled_tab_still_reads_as_something() {
        let c = Chrome::default();
        assert_eq!(c.locator(0, 1, "   ", 1), "1/1 shell");
    }

    #[test]
    fn a_zero_tab_count_does_not_render_as_one_of_zero() {
        // Defensive: the workspace always has a tab, but "1/0" would be a
        // visible lie if it ever did not.
        let c = Chrome::default();
        assert_eq!(c.locator(0, 0, "shell", 1), "1/1 shell");
    }

    #[test]
    fn the_escape_is_a_chord_rather_than_a_tap() {
        // A phone in a pocket generates taps and swipes constantly and must not
        // be able to restore the chrome — or remove it — by accident.
        assert!(ESCAPE_HINT.contains("Ctrl"));
        assert!(ESCAPE_HINT.contains("Shift"));
    }

    /// Voice mode hides the tab COLUMN as well as the strip and the buttons,
    /// and the accounting has to say so.
    ///
    /// It did not. This module measured height only, so bringing the tab column
    /// under the same gate reclaimed 168 points of width that `reclaimed()`
    /// reported as zero — the mode was quietly giving back more than it
    /// claimed, which is the friendlier direction to be wrong in and still
    /// wrong.
    #[test]
    fn voice_mode_reclaims_width_as_well_as_height() {
        assert_eq!(ChromeMode::Touch.reclaimed_width(), 0.0);
        assert_eq!(ChromeMode::Voice.reclaimed_width(), TAB_COLUMN_W);
        assert!(ChromeMode::Voice.reclaimed() > 0.0, "and height, still");
    }

    #[test]
    fn reclaimed_columns_is_the_unit_a_user_can_act_on() {
        // ~8pt monospace advance: 168 / 8 = 21 columns.
        assert_eq!(ChromeMode::Voice.reclaimed_cols(8.0), 21);
        assert_eq!(ChromeMode::Touch.reclaimed_cols(8.0), 0);
        // Guard the divide, as reclaimed_rows does.
        assert_eq!(ChromeMode::Voice.reclaimed_cols(0.0), 0);
        assert_eq!(ChromeMode::Voice.reclaimed_cols(-3.0), 0);
    }

    /// The width constant is a citation, not a taste.
    #[test]
    fn the_tab_column_width_matches_the_qml() {
        // main.qml:4015 — `property real termSidebarWidth: 168`. If the QML
        // moves, this should move with it rather than drift.
        assert_eq!(TAB_COLUMN_W, 168.0);
    }

}

/// The left edge, measured by laying it out rather than by reading the source.
///
/// Six commits changed this surface — the column gated and sized to the QML,
/// collapse-to-chevron, `rail` deleted, sessions merged in, `keycaps` re-gated —
/// and every one was verified by compiler, tests and a panel counter. None was
/// SEEN. A counter reads source; it cannot tell you a panel is 168 points wide,
/// because that is a layout result rather than a syntactic fact.
///
/// egui lays out without a display: `Context::run` produces a full frame and
/// the panel rects are readable afterwards. So the widths this port claims to
/// have taken from `main.qml` can be asserted as WIDTHS, on the same box that
/// cannot open a window. `bars.rs` already runs frames this way; this points the
/// technique at the thing that was actually unverified.
///
/// Not a screenshot. Nothing here says the result looks right — only that the
/// left edge is one bar, that it is the width the QML specifies, and that
/// collapsing it returns the space.
#[cfg(test)]
mod headless_layout {
    /// Lay out a left panel the way voiceterm's tab column does, and return its
    /// width. `SidePanel` reports its rect through the context after a frame.
    fn left_panel_width(open: bool, touch_target: f32) -> f32 {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        // A phone-ish surface: the case the width argument is about.
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO, egui::vec2(420.0, 900.0)));

        let mut measured = 0.0f32;
        let _ = ctx.run(input, |ctx| {
            let panel = if open {
                egui::SidePanel::left("tabbar")
                    .resizable(true)
                    .default_width(super::TAB_COLUMN_W)
                    .width_range(110.0..=340.0)
            } else {
                egui::SidePanel::left("tabbar").resizable(false).exact_width(touch_target)
            };
            let out = panel.show(ctx, |ui| { ui.label("tabs"); });
            measured = out.response.rect.width();
        });
        measured
    }

    /// The fix for the above, measured the same way: content that asks for
    /// the pinned width makes the column that width, whatever its rows say.
    /// One-letter rows — the screenshot's case — at 168, then at a narrower
    /// pin, then at one the range clamps. This is what
    /// `voiceterm_egui.rs`'s tab column does with `tabbar_width`.
    #[test]
    fn content_min_width_pins_the_column_where_default_width_could_not() {
        fn pinned_width(pin: f32) -> f32 {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 900.0)));
            let mut measured = 0.0f32;
            for _ in 0..3 {
                let _ = ctx.run(input.clone(), |ctx| {
                    let out = egui::SidePanel::left("tabbar-pinned").resizable(true)
                        .default_width(pin).width_range(110.0..=340.0)
                        .show(ctx, |ui| {
                            let margin = egui::Frame::side_top_panel(ui.style()).inner_margin.sum().x;
                            ui.set_min_width((pin - margin).max(0.0));
                            for l in ["m", "k", "k"] { ui.label(l); }
                        });
                    measured = out.response.rect.width();
                });
            }
            measured
        }
        assert!((pinned_width(168.0) - 168.0).abs() < 1.5, "pinned 168, laid out {}", pinned_width(168.0));
        assert!((pinned_width(130.0) - 130.0).abs() < 1.5, "pinned 130, laid out {}", pinned_width(130.0));
        assert!((pinned_width(90.0) - 110.0).abs() < 1.5, "below the range: clamped to 110, laid out {}", pinned_width(90.0));
    }

    /// `default_width` DOES NOT PIN THE WIDTH, which the source reads as if it
    /// does.
    ///
    /// Measured, on a 420-point surface across three frames: the column laid
    /// out at 110 — the range MINIMUM — not the 168 the builder asks for. With
    /// no range at all it lays out at 96, which is the content width of a
    /// one-word label. So the panel is content-driven and clamped, and
    /// `default_size` only widens the range to admit its argument
    /// (egui-0.34 panel.rs:404): it is a hint about permitted extent, not a
    /// starting width.
    ///
    /// That matters for a fidelity claim. `main.qml` sets
    /// `width: root.termSidebarWidth` — a FIXED 168 that the user may then
    /// drag. The port's `.default_width(168.0)` reads like the same statement
    /// and is not one. Pinning it needs `exact_width`, which costs the drag the
    /// QML also has, so the trade is real rather than an oversight to fix
    /// blind: with realistic tab rows the content may well exceed 168 anyway,
    /// and only a real surface can say.
    ///
    /// So this asserts the CONTRACT that holds — the QML's range is respected —
    /// rather than a width the builder never promised.
    #[test]
    fn the_open_column_stays_inside_the_qml_drag_range() {
        let w = left_panel_width(true, 44.0);
        assert!((110.0..=340.0).contains(&w),
                "laid out at {w}, outside main.qml:4685's 110..340 drag clamp");
    }

    /// Collapsed, it is one touch target — and the difference is the space the
    /// user gets back. This is the claim the chevron exists to make good.
    #[test]
    fn collapsing_returns_the_column_to_the_panes() {
        let touch = 44.0;
        let open = left_panel_width(true, touch);
        let shut = left_panel_width(false, touch);
        assert!((shut - touch).abs() < 1.0, "collapsed to {shut}, expected {touch}");
        let freed = open - shut;
        assert!(freed > 60.0,
                "collapsing freed only {freed} points; the Qt original cannot collapse at all, \
so a collapse that frees little is worse than not having one");
    }

    /// A frame with the panel absent must give the whole surface to the panes.
    ///
    /// The Voice-mode claim, measured rather than asserted from constants: this
    /// is what `reclaimed_width` is describing, and until now nothing checked
    /// that hiding the panel actually returns its space.
    #[test]
    fn hiding_the_column_returns_all_of_it() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO, egui::vec2(420.0, 900.0)));
        let mut avail = 0.0f32;
        let _ = ctx.run(input, |ctx| { avail = ctx.available_rect().width(); });
        assert!((avail - 420.0).abs() < 1.0,
                "no panel drawn, yet only {avail} of 420 points reached the panes");
    }

    /// ONE left bar, and it stays one.
    ///
    /// The Qt original hosts "work tabs (scrollable) + a default fence + system
    /// tabs" in a single `vTabBar`. This port had three left panels — `sidebar`
    /// (160), `tabbar` (168) and `rail` — two of which rendered the same
    /// `workspace.tabs`. `rail` was deleted and the session list moved inside
    /// the tab column; this is what stops a fourth appearing.
    ///
    /// A SOURCE check, because the panels are built inside a 3000-line `update`
    /// that cannot be driven headlessly without a model, a PTY and a workspace.
    /// The layout tests above measure a panel; this one counts them.
    #[test]
    fn the_left_edge_is_a_single_panel() {
        let src = include_str!("bin/voiceterm_egui.rs");
        // Slice after the MATCH rather than after a hand-counted offset. My
        // first version used `i + 22` and every id came back empty — the
        // pattern is 23 bytes, and a magic number that is wrong by one produces
        // a test that passes vacuously if the expectation is also empty.
        const PAT: &str = "egui::SidePanel::left(\"";
        let ids: Vec<&str> = src.match_indices(PAT)
            .map(|(i, _)| {
                let rest = &src[i + PAT.len()..];
                &rest[..rest.find('"').unwrap_or(0)]
            })
            .collect();
        // `tabbar` appears twice: the if/else that picks resizable-vs-exact
        // builds it in both arms and shows it once.
        let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
        let permanent: Vec<&str> = unique.iter().copied().collect();
        assert_eq!(
            permanent, vec!["tabbar"],
            "the left edge should be the tab column alone. Found {permanent:?} — if a panel was \
added, put its contents IN the column or make it a transient surface (surface.rs + overlay.rs), \
because Qt does this with one bar and the port already carried three. The blocks drawer used to \
be excepted here as `SidePanel::left(\"blocks-drawer\")`; it is an overlay drawer now, so ANY \
second left panel is a regression."
        );
    }

    /// Collapsed, the column keeps both lists rather than dropping one.
    ///
    /// A unified bar that sheds half its contents when narrow is two bars with
    /// one hidden — the arrangement the merge existed to remove. Sessions also
    /// carry `+ New` / `Graph` / `Settings`, so dropping them strands controls
    /// whose only other route is the command palette.
    #[test]
    fn the_collapsed_column_still_shows_sessions() {
        let src = include_str!("bin/voiceterm_egui.rs");
        let collapsed = src.split("if !self.tabbar_open {").nth(1)
            .and_then(|s| s.split("return;").next())
            .expect("the collapsed branch");
        assert!(collapsed.contains("self.workspace.tabs"), "tabs must survive collapse");
        assert!(collapsed.contains("self.sidebar.sessions"),
                "sessions must survive collapse too — dropping them makes the bar unified only \
while it is open");
    }

}
