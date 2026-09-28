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
/// Sending over Bluetooth RFCOMM (BlueZ): Linux only.
#[cfg(target_os = "linux")]
pub mod bt_send;
pub mod light_kit_bridge;
pub mod handshake_audio;
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
/// The Qt build's voice brain: 5,243 lines, ~174 `Intent` variants and the recipes behind
/// them. Qt-free — the one `qt_method` mention in it is a comment — and never mounted here,
/// so this frontend understood the politeness normaliser it borrowed and nothing else.
/// `voice_intent`'s own dependencies, mounted with it. Both Qt-free — the `qt_method`
/// mentions in them are comments about how the Qt build exposed them, not code.
#[path = "../../src/async_jobs.rs"]
pub mod async_jobs;
#[path = "../../src/llm_cache.rs"]
pub mod llm_cache;
#[path = "../../src/security.rs"]
pub mod security;
#[path = "../../src/token_stats.rs"]
pub mod token_stats;
#[path = "../../src/confusion_table.rs"]
pub mod confusion_table;
#[path = "../../src/sysmon.rs"]
pub mod sysmon;
#[path = "../../src/system_info.rs"]
pub mod system_info;
#[path = "../../src/phonetic.rs"]
pub mod phonetic;
#[path = "../../src/speakers.rs"]
pub mod speakers;
/// The voice selection list over that registry. The registry was reachable
/// here already; nothing drew it -- `list_with_ids()` had no caller in this
/// frontend at all.
/// "Hey Kessler here" -- parsing a spoken claim of identity, and the
/// confidence-scored match against registered names. Zero deps, mounted
/// verbatim; the seam into `speaker_list`'s ids is `speaker_claim_view`.
#[path = "../../src/speaker_claim.rs"]
#[allow(dead_code, unused_imports)]
pub mod speaker_claim;
/// Joins the claim parser to `speaker_list`'s ids, enforcing the
/// acoustic-wins security rule at the one point that matters: where a claim
/// becomes an action.
pub mod speaker_claim_view;

/// Cleave dead air out of a voice capture -- silence detection and a
/// variable-speed compression plan, ported verbatim from the reVos/kss
/// lineage. Zero deps; nothing in this frontend trims a recording today.
#[path = "../../src/dead_air.rs"]
#[allow(dead_code, unused_imports)]
pub mod dead_air;

/// "Insert that last IP address" -- scanning terminal output for IPs, MACs,
/// hex hashes, PIDs, ports and paths, and parsing the spoken request for one.
/// Zero deps; the dev-loop TODO this exists for
/// (TODO_recent_fact_lookup.md) has no surface here yet.
#[path = "../../src/fact_extract.rs"]
#[allow(dead_code, unused_imports)]
pub mod fact_extract;
pub mod speaker_list;
/// A keyboard in use holds the microphone off for five seconds. See the
/// module header for why the command vocabulary being live while someone
/// typed was firing commands mid-sentence.
pub mod typing_grace;
/// Holds a dictation session for the frame loop: the facade plus the
/// per-frame bookkeeping an immediate-mode UI needs around it.
pub mod dictation_view;
#[path = "../../src/voice_intent.rs"]
pub mod voice_intent;
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
// Was `mod iio_accel;` over a local copy that `maintenance/C18_TODO.md`
// section 2 identified as the most complete of eight independent IIO
// readers in this fleet, and extracted verbatim into `shared/iio-source`
// (this crate's own copy was the SOURCE of that extraction, not a
// duplicate to reconcile against a pre-existing kit). Re-exported here so
// every existing `voiceterm_egui::iio_accel::X` call site keeps working
// unchanged -- the same shape `bo-controller`'s `passby_canceller`/
// `masker` re-exports used earlier this session.
pub use iio_source as iio_accel;

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
    // No keyboard state available here, so nothing is suppressed. Callers with
    // a keyboard should use `sync_voice_routing_with_grace`; this stays for
    // the gesture-only path and for the existing tests, whose behaviour is
    // unchanged.
    sync_voice_routing_with_grace(router, glove, &typing_grace::TypingGrace::new(), 0)
}

/// As [`sync_voice_routing`], but a keyboard in use holds the command
/// vocabulary off.
///
/// The bug this closes: the `Navigating` arm below re-arms the command
/// vocabulary the instant dictation ends, and its reasoning is sound — a
/// terminal that needed a tap to wake would defeat glove mode. But it never
/// knew whether a keyboard was in use, so the vocabulary was live while
/// someone typed, and a word heard mid-sentence fired a command.
///
/// `now_ms` is the caller's timebase; see `typing_grace` for why the clock
/// lives outside.
pub fn sync_voice_routing_with_grace(router: &mut voice_routing::VoiceRouter,
                                     glove: &uikit_egui::glove::GloveCursor,
                                     grace: &typing_grace::TypingGrace,
                                     now_ms: u64) {
    match glove.mode() {
        // An armed field outranks the keyboard: dictation is an explicit
        // request, and typing in one field while speaking into another is
        // ordinary.
        uikit_egui::glove::Mode::Dictating { field } => router.dictate_into(field),
        // Leaving dictation returns the mic to the command vocabulary rather
        // than to idle: voiceterm is a voice-driven app, and a terminal that
        // stopped listening after one dictation would need a tap to wake — the
        // gesture glove mode exists to avoid.
        //
        // ...unless a keyboard is in use, in which case it goes to Idle and
        // comes back on its own once the grace lapses. Idle rather than
        // suspended, because `Listening::Idle` already means "nothing is
        // interpreting, passive taps still run", which is exactly right here.
        uikit_egui::glove::Mode::Navigating => {
            if typing_grace::should_listen_for_commands(grace, false, now_ms) {
                router.listen_for_commands();
            } else {
                router.idle();
            }
        }
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

    /// THE TYPING BUG, at the integration point. Leaving dictation used to
    /// re-arm the command vocabulary unconditionally, so it was live while
    /// someone typed.
    #[test]
    fn a_keyboard_in_use_holds_the_command_vocabulary_off() {
        use super::typing_grace::{Input, TypingGrace};
        // A fresh cursor is already Navigating -- the arm this fix touches.
        let g = cursor();
        let mut r = VoiceRouter::new();
        let mut grace = TypingGrace::new();
        grace.on_input(Input::Text, 1_000);

        super::sync_voice_routing_with_grace(&mut r, &g, &grace, 1_500);
        assert_eq!(r.state(), &super::voice_routing::Listening::Idle,
            "typing must not leave the command vocabulary live");

        // ...and it comes back on its own, with no tap.
        super::sync_voice_routing_with_grace(&mut r, &g, &grace,
            1_000 + super::typing_grace::GRACE_MS);
        assert_eq!(r.state(), &super::voice_routing::Listening::Commands);
    }

    /// An idle keyboard must behave exactly as before this change.
    #[test]
    fn an_idle_keyboard_does_not_change_the_old_behaviour() {
        use super::typing_grace::TypingGrace;
        let g = cursor();
        let (mut a, mut b) = (VoiceRouter::new(), VoiceRouter::new());
        super::sync_voice_routing(&mut a, &g);
        super::sync_voice_routing_with_grace(&mut b, &g, &TypingGrace::new(), 9_999);
        assert_eq!(a.state(), b.state());
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
// The five mounts below back bar_pages, immediately after it. They live at
// the crate ROOT rather than nested under bar_pages because the Qt-authored
// files themselves (dictate.rs, repair.rs) use `crate::code_symbols`,
// `crate::dictate` etc. to reach each other — correct in the Qt binary,
// where they ARE crate-root siblings. Nesting them under bar_pages instead
// would make `crate::dictate` resolve to nothing from inside dictate.rs's
// own sibling `repair.rs`, which is exactly the E0432 this arrangement
// avoids. See bar_pages.rs's module doc for the fuller account, including
// the E0255 (`editor_voice` reimported under its own name) this also fixes.
#[path = "../../src/editor_voice.rs"]
#[allow(dead_code, unused_imports)]
pub mod editor_voice;
#[path = "../../src/repl_voice.rs"]
#[allow(dead_code, unused_imports)]
pub mod repl_voice;
#[path = "../../src/code_symbols.rs"]
#[allow(dead_code, unused_imports)]
pub mod code_symbols;
#[path = "../../src/repair.rs"]
#[allow(dead_code, unused_imports)]
pub mod repair;
#[path = "../../src/dictate.rs"]
#[allow(dead_code, unused_imports)]
pub mod dictate;

// ── the dictation stack, mounted whole ───────────────────────────────────────
//
// `repair`, `dictate` and `code_symbols` were already here; the frontend had
// been assembling a dictation session out of them BY HAND. `dictation_engine`
// exists precisely to stop that, and says so in its own header: "Leaving the
// composition to the caller means writing it again in QML, in whatever the CLI
// does, and in every test -- and getting it subtly different each time." This
// frontend was that caller.
//
// Its two remaining dependencies close against modules already mounted, so the
// whole facade arrives as three #[path] lines and no reimplementation.
#[path = "../../src/dictation_session.rs"]
#[allow(dead_code, unused_imports)]
pub mod dictation_session;
#[path = "../../src/voice_status.rs"]
#[allow(dead_code, unused_imports)]
pub mod voice_status;
#[path = "../../src/filler_words.rs"]
#[allow(dead_code, unused_imports)]
pub mod filler_words;
#[path = "../../src/dictation_engine.rs"]
#[allow(dead_code, unused_imports)]
pub mod dictation_engine;
pub mod bar_pages;
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
/// The voice popup, or an inert stand-in for it.
///
/// `voice` is on in `app` because this is a voice terminal. It is separable
/// because dictation is the only thing here that needs whisper-rs, which needs
/// bindgen, which must dlopen libclang at build time — impossible on a
/// static-musl target, reported from aarch64-unknown-linux-musl as "Dynamic
/// loading not supported".
///
/// The stub carries the SAME API rather than the call sites being gated. Nine
/// `#[cfg]`s through a draw loop is how a build configuration nobody runs
/// stops compiling without anyone noticing; one stub is checked by the
/// compiler on every build.
#[cfg(feature = "voice")]
pub use uikit_egui::voice_popup;

#[cfg(not(feature = "voice"))]
pub mod voice_popup {
    /// Inert stand-in. Every method is the "nothing happened" answer, which is
    /// what the callers already do when a dictation attempt fails.
    #[derive(Debug, Default)]
    pub struct VoicePopup;

    impl VoicePopup {
        /// `false` — the same answer as a microphone that would not open, so
        /// the caller's existing failure path handles it.
        pub fn start(&mut self, _password_mode: bool) -> bool { false }
        pub fn cancel(&mut self) {}
        pub fn finish(&mut self) {}
        /// `false` — nothing became ready.
        pub fn tick(&mut self) -> bool { false }
        /// Nothing was ever transcribed.
        pub fn take_result(&mut self) -> Option<String> { None }
        pub fn start_by_wake(&mut self, _password_mode: bool) -> bool { false }
        /// Draws nothing. Not even a "voice unavailable" notice: this build
        /// has no voice controls to press, so there is nothing to explain.
        pub fn draw(&mut self, _ui: &mut egui::Ui, _colors: &uikit_egui::PaletteColors) {}
    }
}

// ctrl_p_gesture and voice_popup moved to shared/uikit/crates/uikit-egui
// (voice-popup feature) -- generic, zero voiceterm-specific dependencies,
// so every egui app gets the same API instead of a per-app copy. Used
// here as uikit_egui::ctrl_p_gesture / uikit_egui::voice_popup.
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

/// The Proton mail engine, mounted from the Qt build. Four modules and
/// all of them Qt-free: typed errors, the hand-rolled JSON, the crypto
/// seams (with the SRP handshake behind `mail-proton-srp`), and the
/// `ureq` client for `mail.proton.me/api`. `mail.rs` — the MailHub that
/// drives QML generators — stays behind; `mail_signin` below is its
/// egui-side counterpart for the one flow that matters first: sign in
/// and list the inbox.
#[cfg(feature = "mail-proton")]
#[path = "../../src/mail_error.rs"]
pub mod mail_error;
#[cfg(feature = "mail-proton")]
#[path = "../../src/mail_json.rs"]
pub mod mail_json;
#[cfg(feature = "mail-proton")]
#[path = "../../src/mail_crypto.rs"]
pub mod mail_crypto;
#[cfg(feature = "mail-proton")]
#[path = "../../src/mail_proton.rs"]
pub mod mail_proton;
/// Sign in and list the inbox, on a worker thread, for the Mail pane.
pub mod mail_signin;

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
/// NOT MOUNTED: the connections graph, and why.
///
/// `connections` is the Qt build's Obsidian-style node view over the archive, the voice
/// corpus, chat history and speakers — 1,556 lines, Qt-free, and the natural next port after
/// `voice_intent`. It does not mount, and the obstacle is worth writing down because it is
/// not the one you would guess.
///
/// It needs `crate::chat`, which needs `crate::token_log::parse_anthropic_usage` and
/// `append_event`. THIS CRATE ALREADY HAS A `token_log` — its own, ten functions to the Qt
/// one's twenty-three. So mounting the chain does not add a module, it collides with one, and
/// the two are not interchangeable: the Qt version is a superset written for a different
/// caller.
///
/// That is a reconciliation, not a mount — someone has to decide which `token_log` is
/// canonical and move the callers of the other. Doing it halfway, by renaming one to get a
/// compile, would leave two token ledgers writing to the same place and no way to tell which
/// number is right.
///
/// The rest of the chain (`layout_cache`, `archive`, `user_tags`) is clean and mounts
/// without complaint; `archive` and `layout_cache` also want `scraper`, which is the heaviest
/// dependency this crate would take on and is worth pricing deliberately rather than
/// acquiring as a side effect of a graph view.
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

/// The note editor's live-preview blend, from `src/note_edit.rs`.
///
/// Mounted as `note_blend` rather than `note_edit` because this crate already
/// has an unrelated `note_edit` (the palace's Fork and Tag actions). Same
/// name, different module — a collision, not a duplicate, and renaming the
/// mount is cheaper than renaming either original.
///
/// It is what gives `zbrackets` a caller. `zbrackets` has been mounted here
/// with NOTHING calling it: the frontend could rewrite `[[name]]` into a link
/// and never did, because the piece that decides WHICH lines to rewrite was
/// the unported half. `blend_lines` is that decision — the cursor's line stays
/// raw markdown so you can edit the syntax, every other line renders — and it
/// deliberately does not do the rewriting itself, which is why it mounts
/// with no dependencies at all.
#[path = "../../src/note_edit.rs"]
#[allow(dead_code, unused_imports)]
pub mod note_blend;

/// Per-diagram node pins, stored inline in the markdown under each mermaid
/// block.
///
/// The frontend draws graphs — `graph_view`, `layered_graph`,
/// `connections_graph` — and can persist NOTHING about them: drag a node and
/// the position is gone on reload, because nowhere in this crate writes a pin.
/// `src/transforms.rs` is that layer, 670 lines and 30 tests, with zero
/// `crate::` references of any kind, so it mounts with one line.
///
/// Its storage decision is recorded in its own header as a user direction:
/// "link transforms to the file with inline markdowns under the diagrams so
/// that it is human readable in a text editor but also functional" — a fenced
/// ```mermaid-transforms``` block following each ```mermaid``` block. A
/// sidecar file would have been easier and would go stale the first time
/// someone moved the note.
#[path = "../../src/transforms.rs"]
#[allow(dead_code, unused_imports)]
pub mod transforms;

/// Joins `note_blend`'s active-line decision to `zbrackets`' rewriting. Both
/// were here and neither had a caller; the seam between them is the feature.
pub mod note_blend_view;

// `moment` is NOT mounted, and the attempt is worth recording.
//
// I mounted it on a scan that reported zero dependencies. The scan matched
// `use crate::` lines only, and `moment.rs` reaches its neighbours through
// INLINE paths -- `crate::audio_profile::noise_floor()`,
// `crate::scroll_signals::is_reading_paced()` -- which the scan never saw. The
// real closure is audio_profile -> clog -> log -> util, plus security and
// scroll_signals: five more modules, and `clog!` expands to `$crate::log::emit`
// so the macro drags the logger in behind it.
//
// Mounting five unverified modules to reach a prompt-timing classifier is the
// wrong trade, and the build said so. The classifier is still worth having --
// see `typing_grace` for the same question asked about the keyboard -- but it
// needs its neighbours cut loose first, which is upstream work in `src/`.
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
/// The one chain from a transcript to an effect — typed or spoken.
pub mod voice_router;
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

/// Serializes tests (in `mail_store.rs` and `editor_open_env.rs`) that mutate
/// `HOME`/`MAIL` via `std::env::set_var`/`remove_var`. Both files' tests were
/// commented "SAFETY: single-threaded test" -- but `cargo test` runs this
/// crate's tests concurrently by default, and those calls mutate
/// process-global state. One test's HOME landing mid-run of the other can
/// make `mail_store::discover()` see a real mailbox under the wrong HOME (or
/// `ssh_config_hosts()` see the wrong `.ssh`), which is exactly the shape of
/// `mail_store::tests::an_empty_pane_says_where_it_looked`'s observed
/// failure: `empty_reason()` returning `None` (a store WAS found) when the
/// test's own freshly-created temp dir has nothing in it.
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// The GestureHost+SlidePopup replacement (grid gesture menus with QML-parity variants) lives in
// the shared kit; re-exported under a voiceterm-familiar name.
pub use uikit_egui::slide_menu;
// `web` (HTML → egui RichDoc renderer) now lives in the shared uikit-web kit; re-export as `web`.
pub use uikit_web as web;

// The terminal (grid model + VT/ANSI parser + themed egui renderer + App) now lives in the
// shared `uikit-term` kit so other egui apps can embed it. Re-exported here so voiceterm's own
// code and callers keep using `voiceterm_egui::{Terminal, Grid, render_grid, App, …}` unchanged.
pub use uikit_term::*;

