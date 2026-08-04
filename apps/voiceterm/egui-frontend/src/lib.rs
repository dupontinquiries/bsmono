//! voiceterm-egui — the terminal, ported to egui.
//!
//! This is the tall part of the port, started the safe way: a **portable terminal core** —
//! a screen-grid model ([`Grid`]) plus a VT/ANSI [`Parser`] — that is pure and unit-tested,
//! with no egui or PTY in it. The egui cell renderer, the PTY process I/O, and the voice/mail
//! panes layer on top. Because voiceterm is where the fleet's staged commands land (via the
//! cross-app action protocol), [`Terminal::stage`] puts a command on the input line without
//! running it.
//!
//! The parser covers the everyday subset: printable text (UTF-8), the C0 controls
//! (`\n \r \t \x08`), and CSI sequences for cursor movement, erase, and SGR colour/attrs —
//! enough to run a shell, `ls --color`, vim's basic drawing, etc. Rarer sequences are ignored
//! rather than mis-rendered.

pub mod pty_input;
pub mod table_view;
pub mod row_render;
pub mod detect;
pub mod explore;
pub mod manpage_view;
pub mod voice_routing;

/// Double-flick commands, bound to tab management.
///
/// Bottom-up twice opens a tab. The binding is voiceterm's; the recognition and
/// the settings gate are the kit's, so snapclone's camera swap and this cannot
/// disagree about what a double flick is.
pub fn shake_bindings() -> uikit_egui::glove::ShakeCommands {
    use uikit_egui::glove::{Orient, ShakeBinding, ShakeCommands};
    ShakeCommands::with(vec![
        ShakeBinding { dir: Orient::BottomUp, action: "new_tab".into(),
                       label: "New tab".into() },
        // Left-up closes, deliberately NOT bound to anything destructive
        // without a confirmation: see Confirm in the kit.
        ShakeBinding { dir: Orient::LeftUp, action: "prev_tab".into(),
                       label: "Previous tab".into() },
        ShakeBinding { dir: Orient::RightUp, action: "next_tab".into(),
                       label: "Next tab".into() },
    ])
}

/// Drive the router from glove mode's state.
///
/// One place decides, so the terminal's command path and the cursor's dictation
/// state cannot disagree about who owns the microphone. Arming a field
/// SUSPENDS the command NLU rather than leaving it running and discarding its
/// output — a filtered command is still a command that nearly fired.
pub fn sync_voice_routing(router: &mut voice_routing::VoiceRouter,
                          glove: &uikit_egui::glove::GloveCursor) {
    match glove.mode() {
        uikit_egui::glove::Mode::Dictating { field } => router.dictate_into(field),
        // Leaving dictation returns the mic to the command vocabulary rather
        // than to idle: voiceterm is a voice-driven app, and a terminal that
        // stopped listening after one dictation would need a tap to wake — the
        // gesture glove mode exists to avoid.
        uikit_egui::glove::Mode::Navigating => router.listen_for_commands(),
    }
}

#[cfg(test)]
mod voice_glove_tests {
    use super::voice_routing::{Consumer, VoiceRouter};
    use uikit_egui::glove::{ColdMode, GloveCursor, Target, TargetKind, Orient};

    fn cursor() -> GloveCursor {
        let mut g = GloveCursor::with_mode(ColdMode::On);
        g.set_bounds(400.0, 800.0);
        g
    }

    #[test]
    fn arming_a_field_suspends_the_terminals_command_path() {
        // The collision this exists for: voiceterm is already listening for
        // commands when glove mode starts dictating.
        let ts = vec![Target { id: "body".into(), kind: TargetKind::Field,
                               at: (200.0, 400.0), label: "Body".into() }];
        let mut g = cursor();
        let mut r = VoiceRouter::new();
        r.listen_for_commands();
        assert!(r.feeds(Consumer::CommandNlu));

        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        super::sync_voice_routing(&mut r, &g);
        assert!(!r.feeds(Consumer::CommandNlu), "suspended, not filtered");
        assert!(r.feeds(Consumer::DictationSink));
        assert!(r.feeds(Consumer::Fingerprint), "and training continues");
    }

    #[test]
    fn escaping_returns_the_mic_to_commands_not_to_idle() {
        // A terminal that stopped listening after one dictation would need a
        // tap to wake — the gesture glove mode exists to avoid.
        let ts = vec![Target { id: "body".into(), kind: TargetKind::Field,
                               at: (200.0, 400.0), label: "Body".into() }];
        let mut g = cursor();
        let mut r = VoiceRouter::new();
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        super::sync_voice_routing(&mut r, &g);
        assert!(r.dictating());

        g.feed(Orient::FaceDown, 0.0, &ts);
        g.feed(Orient::FaceDown, uikit_egui::glove::ESCAPE_HOLD_MS, &ts);
        super::sync_voice_routing(&mut r, &g);
        assert!(!r.dictating());
        assert!(r.feeds(Consumer::CommandNlu), "back to listening for commands");
    }

    #[test]
    fn the_wake_prefix_still_reaches_stop_while_a_field_is_armed() {
        let ts = vec![Target { id: "body".into(), kind: TargetKind::Field,
                               at: (200.0, 400.0), label: "Body".into() }];
        let mut g = cursor();
        let mut r = VoiceRouter::new();
        g.feed(Orient::Rest, 16.0, &ts);
        g.speak("select", &ts);
        super::sync_voice_routing(&mut r, &g);
        assert_eq!(r.route("terminal, stop"),
                   super::voice_routing::Routed::Interrupt("stop".into()));
        assert_eq!(r.route("stop"), super::voice_routing::Routed::Text("stop".into()));
    }
}

#[cfg(test)]
mod shake_binding_tests {
    use uikit_egui::glove::Orient;

    #[test]
    fn a_double_flick_opens_a_tab_and_a_single_one_does_not() {
        let mut c = super::shake_bindings();
        c.enabled = true;
        assert_eq!(c.feed(Some(Orient::BottomUp), 16.0), None);
        assert_eq!(c.feed(Some(Orient::BottomUp), 150.0).as_deref(), Some("new_tab"));
    }

    #[test]
    fn tab_navigation_is_bound_both_ways() {
        let mut c = super::shake_bindings();
        c.enabled = true;
        c.feed(Some(Orient::RightUp), 16.0);
        assert_eq!(c.feed(Some(Orient::RightUp), 150.0).as_deref(), Some("next_tab"));
        c.feed(Some(Orient::LeftUp), 16.0);
        assert_eq!(c.feed(Some(Orient::LeftUp), 150.0).as_deref(), Some("prev_tab"));
    }

    #[test]
    fn nothing_destructive_is_bound_to_a_shake() {
        // A gesture you can make by accident must not close a tab with unsaved
        // work in it. Closing stays a deliberate action behind a confirmation.
        let c = super::shake_bindings();
        for dir in [Orient::BottomUp, Orient::LeftUp, Orient::RightUp] {
            let label = c.describe(dir).unwrap_or("");
            assert!(!label.to_lowercase().contains("close"), "{dir:?} -> {label}");
        }
    }

    #[test]
    fn the_gesture_is_off_until_switched_on() {
        let mut c = super::shake_bindings();
        c.feed(Some(Orient::BottomUp), 16.0);
        assert_eq!(c.feed(Some(Orient::BottomUp), 150.0), None);
    }
}
pub mod mail_view;
pub mod archived_view;
pub mod cards_view;
pub mod graph_view;
pub mod bars;
pub mod browser;
pub mod bookmarks_view;
pub mod build_view;
pub mod backlinks_view;
pub mod viz;
pub mod confirm_dialog;
pub mod chat_panel;
pub mod model_picker;
pub mod repl_voice_glow;
pub mod setup_wizard;
pub mod action_compass;
pub mod shell;
pub mod gesture_menus;

/// voiceterm's tab + pane model, shared with the QML frontend rather than
/// reimplemented here. It is pure `std` (no Qt, no `crate::`) and carries 28
/// tests in the app crate, so `#[path]` costs nothing and guarantees the two
/// frontends cannot drift. Lifting it into `uikit-term` alongside the block
/// tracking (8e93585) is the natural follow-up once a host that can build
/// egui 0.34 is available to validate the move.
#[path = "../../src/terminal_workspace.rs"]
pub mod terminal_workspace;

/// The table engine, on the same terms. `table_ops` holds TableOp and the apply
/// pipeline, `table_nlp` turns natural language into ops and meta-commands.
/// Both are plain Rust with no Qt in them, and `table_view` is the surface over
/// them — the view was what was missing, not the engine. Included rather than
/// re-implemented so the two frontends parse the same vocabulary; a second
/// parser would accept a slightly different English within a week.
/// The dirty-row tracker, on the same terms — pure, dependency-free, and fed by
/// the grid. `row_render` is the egui-side use of it; see that module for why
/// what it saves here is text LAYOUT rather than serialisation.
#[path = "../../src/row_cache.rs"]
pub mod row_cache;

/// The include chain terminates here: entity_extract needs clog!, log needs
/// util, and util is std-only with no crate:: references at all. Worth naming
/// as a cost — including a module drags its crate-internal dependencies with
/// it. It is still the right trade against a second copy of a 1,900-line
/// extractor that would accept slightly different output within a month.
#[path = "../../src/util.rs"]
pub mod util;

/// entity_extract logs through `crate::clog!`, so the logger comes with it.
/// log.rs is std-only as well, and including it rather than defining a stand-in
/// macro keeps the same categories and the same VOICETERM_LOG filtering — a
/// second logger would mean the two frontends disagreed about what is worth
/// recording.
#[macro_use]
#[path = "../../src/log.rs"]
pub mod log;

/// The detection engines, same terms again: `entity_extract` is
/// terminal-output-as-menu, `dumb_summary` spots ASCII progress bars. Both are
/// Qt-free and both had no surface at all — `detect` is the views they were
/// missing.
/// session_walk's own header says the QML side consumes a
/// wire string from it and "the camera animation + slider live in main.qml" —
/// so the PLAYER is exactly what did not come across. It pulls in nlp for the
/// word-smush, which is std-only apart from util::fnv1a — already here.
///
/// `sitemap` is deliberately NOT included: its link extractor uses the
/// `scraper` crate, which is in the app's manifest and not this one, and adding
/// html5ever to a frontend whose cold build is already ~97% dependencies is a
/// real cost for one function. explore.rs mirrors its two small row types
/// instead and says so — a documented duplication with a stated reason, which
/// is a different thing from the sink picker I duplicated by accident.
#[path = "../../src/nlp.rs"]
pub mod nlp;
#[path = "../../src/session_walk.rs"]
pub mod session_walk;

#[path = "../../src/entity_extract.rs"]
pub mod entity_extract;
#[path = "../../src/dumb_summary.rs"]
pub mod dumb_summary;

#[path = "../../src/table_ops.rs"]
pub mod table_ops;
#[path = "../../src/table_nlp.rs"]
pub mod table_nlp;
// The GestureHost+SlidePopup replacement (grid gesture menus with QML-parity variants) lives in
// the shared kit; re-exported under a voiceterm-familiar name.
pub use uikit_egui::slide_menu;
// `web` (HTML → egui RichDoc renderer) now lives in the shared uikit-web kit; re-export as `web`.
pub use uikit_web as web;

// The terminal (grid model + VT/ANSI parser + themed egui renderer + App) now lives in the
// shared `uikit-term` kit so other egui apps can embed it. Re-exported here so voiceterm's own
// code and callers keep using `voiceterm_egui::{Terminal, Grid, render_grid, App, …}` unchanged.
pub use uikit_term::*;

