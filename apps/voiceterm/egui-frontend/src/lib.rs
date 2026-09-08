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

pub mod iio_light;
pub mod editor_keys;
pub mod date_phrase;
pub mod cmd_risk;
pub mod snapshot_policy;
pub mod layered_graph;
pub mod summary_scope;
pub mod command_search;
pub mod corpus_tape;
pub mod acoustic_cache;
pub mod mail_selector;
/// A test-only harness for driving renderers through real egui frames.
/// Compiled out of a release build entirely — it exists because five lists
/// in this app drew rows nobody could click and every model test passed.
#[cfg(test)]
pub mod frame_probe;
pub mod mail_rfc822;
pub mod mail_store;
pub mod web_prefs;
pub mod token_log;
pub mod connections_graph;
pub mod summary_queue;
/// Satisfies `crate::data` for the `smart_cd` module included by `favorites`.
///
/// Empty lists rather than a copy of voiceterm's: a second list of project
/// markers is a second thing to keep in step, and this frontend does not call
/// the descent heuristics that read them. Empty is correct here and obviously
/// wrong the moment anything does.
#[allow(dead_code)]
pub mod data {
    pub fn project_markers() -> &'static [String] { &[] }
    pub fn skip_dirs() -> &'static [String] { &[] }
    pub fn source_exts() -> &'static [String] { &[] }
    pub fn init() {}
}
pub mod memlog;        // v0.1.896 memory-footprint poll, off unless WEB2QML_MEMLOG
pub mod favorites;     // KDE Places plus a Home anchor (favoritesPopup)
pub mod missing_pkg;   // packages recent commands reported missing (missingPkgDrawer)
pub mod cmd_sheet;     // touch-friendly command history, tap to place / drag up to star
pub mod card_preview;  // the slide-up card sheet (previewPanel)
pub mod fs_results;   // the find bar's FILES mode — path:line hits, and the SSH refusal
pub mod peer_panel;   // the acoustic handshake — the last QML surface with no egui counterpart
pub mod password_prompt;
pub mod audit_chain;
pub mod context_redact;
pub mod phonetic_store;
pub mod voice_prefix;
pub mod workspaces_view;
pub mod pty_input;
/// The other half of the terminal: a real shell on a real pty.
pub mod pty_host;
pub mod table_view;
pub mod docingest;
pub mod detect;
pub mod explore;
pub mod manpage_view;
pub mod blocks;
pub mod chrome_mode;
pub mod mail;
pub mod voice_routing;
/// One command vocabulary for the palette and the recogniser.
pub mod palette;
pub mod iio_accel;

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
    use uikit_egui::glove::{ColdMode, GloveCursor, Target, TargetKind, Orient, Tilt};

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

        g.feed(Orient::Normal, Tilt::FaceUp, 16.0, &ts);
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
        g.feed(Orient::Normal, Tilt::FaceUp, 16.0, &ts);
        g.speak("select", &ts);
        super::sync_voice_routing(&mut r, &g);
        assert!(r.dictating());

        g.feed(Orient::Normal, Tilt::FaceDown, 0.0, &ts);
        g.feed(Orient::Normal, Tilt::FaceDown, uikit_egui::glove::ESCAPE_HOLD_MS, &ts);
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
        g.feed(Orient::Normal, Tilt::FaceUp, 16.0, &ts);
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
pub mod mail_login_view;
pub mod reading_trainer;
pub mod archived_view;
pub mod cards_view;
/// Share the zettelkasten as a huge QR: pick, paint public, two gates, show. Pure.
pub mod zk_share;
/// Who this installation is: a persisted keypair, its fingerprint, and the workspace digest.
pub mod identity;
pub mod graph_view;
pub mod note_edit;
/// `link_graph.rs` is mounted Qt-free under `graph_view` (see that module's
/// doc), and its `from_niftyplot` — unused by this port, present because the
/// file is kept byte-identical to the canonical Qt-side one — refers to
/// `crate::niftyplot::parse`, which the Qt binary has at ITS crate root. The
/// parser itself is `nifty_core::niftyplot` (already this crate's dependency
/// and what the bin's graph pane calls); re-exporting it under the name the
/// mounted file expects is the one-line fix rather than forking the file.
pub use nifty_core::niftyplot;
pub mod palace;
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
pub mod surface;   // which transient surface is up — the QML's modality, as state (C10-0a)
pub mod overlay;   // how each surface draws — the QML's modal forms (drawer/sheet/popup)
pub mod pane_host; // what follows a pane: the session bundle + the focus swap (C11-6a)
pub mod gesture_menus;

/// voiceterm's tab + pane model, shared with the QML frontend rather than
/// reimplemented here. It is pure `std` (no Qt, no `crate::`) and carries 28
/// tests in the app crate, so `#[path]` costs nothing and guarantees the two
/// frontends cannot drift. Lifting it into `uikit-term` alongside the block
/// tracking (8e93585) is the natural follow-up once a host that can build
/// egui 0.34 is available to validate the move.
#[path = "../../src/terminal_workspace.rs"]
pub mod terminal_workspace;


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
/// Structured-output detection for the (still unwired, in this frontend)
/// notebook view — see TODO_notebook_view.md. Pure and already tested;
/// depends on table_ops, already above it, so `#[path]` costs nothing new.
#[path = "../../src/notebook.rs"]
pub mod notebook;
/// The notebook pane's per-cell state: a classification cache and the folds.
pub mod notebook_cells;
/// Auto-naming a session from what has been run in it (`TODO_session_autoname.md`).
#[path = "../../src/session_auto.rs"]
pub mod session_auto;
pub mod session_name;
/// Zellij voice/assisted control — PURE core (phrase → `zellij action`
/// argv, zellij-session detection). Fully self-contained (no `use` of any
/// other module in this crate), tested, and — like notebook.rs above —
/// has never been re-exported here, so its 12 tests never ran as part of
/// this crate and the module was unreachable from the egui binary.
///
/// NOT wired into the voice pipeline in this commit: `in_zellij_session`
/// takes the env var as a plain parameter specifically so it stays pure,
/// but the real question — whether zellij is running inside the PTY's
/// child shell — isn't something this app's own process env can answer.
/// A child process's environment doesn't propagate back to its parent;
/// checking `std::env::var("ZELLIJ")` here would report whether the EGUI
/// APP is in a zellij session, not whether the shell it's hosting is.
/// That needs either a real detection mechanism (parsing shell output,
/// a manual opt-in toggle — the TODO's own "OPTIONAL, opt-in" framing
/// would fit that) or an accepted approximation, and shipping a
/// confidently-labeled feature that's silently checking the wrong
/// process's environment would be exactly the "looks wired, isn't"
/// shape this session spent most of its time finding and fixing
/// elsewhere. Filed as the reason to decide before wiring, not silently
/// worked around.
#[path = "../../src/zellij_voice.rs"]
pub mod zellij_voice;
/// Zettelkasten `[[wiki-link]]` resolution — PURE core (index a set of
/// note paths by basename, classify + resolve each `[[...]]` token in a
/// body against that index, rewrite a whole markdown document). 19
/// tests, self-contained (only `std::collections::HashMap`), never
/// re-exported here — same shape as notebook.rs and zellij_voice.rs
/// above.
///
/// Backlinks are UNAFFECTED by any of this: `src/backlinks.rs` (already
/// wired, via `backlinks_view.rs`) does its OWN `[[...]]` extraction
/// internally to build the "what links here" index — a note that types
/// `[[Other Note]]` already shows up in Other Note's backlinks today,
/// with zero help from this module. What zbrackets uniquely adds is
/// RENDERING: turning `[[Other Note]]` into an actual clickable link
/// instead of literal bracket text, when a note's body is displayed.
///
/// NOT wired into `cards_view::deck_ui`'s body rendering in this commit.
/// That needs care this pass didn't have room for: `deck_ui` renders
/// preview bodies for every card in the list (not just the selected
/// one) via a single `ui.label(RichText::new(body)...)` call per card,
/// and making `[[...]]` clickable there means restructuring that into a
/// per-card scan-and-split render (the same technique this session's
/// `inline_md()` used for the Docs pane's bold/code/link spans) PLUS
/// deciding how a resolved target (a file path, per zbrackets' own
/// model) maps back to one of THIS app's in-memory cards, which are
/// keyed by title, not a real notes-directory path — `deck_ui` is a
/// small, working, tested widget, and reshaping its rendering contract
/// deserved more attention than this pass had left to give it correctly
/// the first time.
#[path = "../../src/zbrackets.rs"]
pub mod zbrackets;
/// Modeless vim-style count-prefix accumulator (`{N}b` = cycle N forward,
/// `{N}B` = N back) — the state machine behind TODO_tab_cycle_shortcut.md,
/// fully done and wired in the Qt frontend since v0.1.719. Pure,
/// self-contained (no `use` of anything), 8 tests, never re-exported here.
/// Reusable for any modeless count command, not just tab cycling.
#[path = "../../src/count_prefix.rs"]
pub mod count_prefix;
/// Where `{N}b` lands: `cycle_target(current, count, step, forward)`. Pure, tested, wired in
/// the Qt build since v0.1.719 and never mounted here — the other half of `count_prefix`.
#[path = "../../src/tab_cycle.rs"]
pub mod tab_cycle;
/// Which documentation a command should open: bundled man page, system man, tldr, `--help`,
/// or a web search. 11 tests, probe-driven, and never mounted here
/// (`TODO_doc_pane.md`'s "implicit-mode brain").
#[path = "../../src/doc_resolver.rs"]
pub mod doc_resolver;
/// The availability facts `doc_resolver` asks for, answered from what this frontend holds.
pub mod doc_avail;
/// Note clipping's data model + markdown serialization — see
/// TODO_notes_clip.md. Pure and tested since v0.1.641; had no re-export
/// and no caller in this frontend before now, same shape as notebook.rs
/// just above.
#[path = "../../src/note_clip.rs"]
pub mod note_clip;
/// Clipped notes on disk, as the markdown they already render to.
pub mod note_store;
/// Jot mode: dictation into the note instead of the command parser.
pub mod jot;
/// Which `note_clip::Source` a clip should be filed under.
pub mod clip_source;
/// Finding the notes a piece of text belongs with.
pub mod note_search;
/// Command clustering: k-means over behavioural features, measured to 1.00 purity on the
/// planted set (`TODO_command_clustering.md`). Wired in the Qt build since v0.1.720 and
/// never mounted here.
#[path = "../../src/cmd_cluster.rs"]
pub mod cmd_cluster;
/// The dominant cluster label for a session, for naming it.
pub mod session_cluster;
/// "open <thing> on <host> at line N in <editor>" — slot parser, ssh-config hosts, argv.
/// Pure and tested since v0.1.654, wired into the Qt build, never mounted here.
#[path = "../../src/editor_open.rs"]
pub mod editor_open;
/// The I/O around `editor_open`: known hosts from ~/.ssh/config, and a default editor.
pub mod editor_open_env;
/// Is a clipped selection code or prose, and in what language? (`TODO_notes_clip.md`.)
pub mod clip_kind;
/// Who owns the keyboard when a terminal is on screen (Tab, the arrows, and the sidebar).
pub mod kbd_focus;
/// Learning the shell's working directory from OSC 7 in the pty stream.
pub mod osc7;
/// Turning this terminal's blocks into `session_auto`'s activity vocabulary.
pub mod session_proposals;
// The GestureHost+SlidePopup replacement (grid gesture menus with QML-parity variants) lives in
// the shared kit; re-exported under a voiceterm-familiar name.
pub use uikit_egui::slide_menu;
// `web` (HTML → egui RichDoc renderer) now lives in the shared uikit-web kit; re-export as `web`.
pub use uikit_web as web;

// The terminal (grid model + VT/ANSI parser + themed egui renderer + App) now lives in the
// shared `uikit-term` kit so other egui apps can embed it. Re-exported here so voiceterm's own
// code and callers keep using `voiceterm_egui::{Terminal, Grid, render_grid, App, …}` unchanged.
pub use uikit_term::*;

