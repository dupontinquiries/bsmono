//! surface.rs — which transient surface is up. At most one.
//!
//! The QML gets exclusivity for free: every one of these surfaces is a modal
//! popup (`modal: true; dim: true` — blocksDrawer main.qml:30637,
//! missingPkgDrawer :30036, paletteDrawer :31310, settingsPopup :27729,
//! favoritesPopup :30479, and cmdHistorySheet's full-screen tap-to-close
//! MouseArea :1533), so while one is open the control that would open another
//! is unreachable. egui has no modality — the port accumulated one `bool` per
//! surface instead, and nothing stopped `settings`, `workspaces` and
//! `missing_pkg` being three right-edge panels open at once, shoulder to
//! shoulder, with the terminal squeezed between them. That is the "multiple
//! conflicting sidebars" of C10-0a.
//!
//! This model IS the modality, as state rather than input blocking: exactly
//! zero or one surface, and every route to a surface — keycap chip, sidebar
//! action, voice command, palette entry, Ctrl+P — goes through it. The render
//! sites in `voiceterm_egui.rs` gate on [`Surfaces::is`]; a structural test
//! below pins that the independent flags are gone from the binary.
//!
//! What this deliberately does NOT do:
//!
//! * **No Esc-to-close.** The QML popups take focus, so Esc closes the popup
//!   and the terminal never sees the key. These egui panels are non-modal
//!   beside a LIVE terminal; a global Esc hook would steal the key from vim
//!   in the pane next door. Whether these surfaces should become truly modal
//!   (scrim + input capture) is an appearance/interaction decision for
//!   on-device work, not for a sandbox with no display.
//! * **No render-form opinions.** `settings` is a right panel here and a
//!   centered popup in the QML; collapsing that difference is the same
//!   on-device pass. This model only says WHICH surface is up.

/// The transient surfaces. The persistent tab column (`tabbar`) is chrome,
/// not a surface — it has no entry here and `tabbar_open` stays a plain flag.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Surface {
    /// Command blocks (QML blocksDrawer, left edge).
    Blocks,
    /// Missing-package list (QML missingPkgDrawer, right edge).
    MissingPkg,
    /// The workspaces page (rendered as a right panel).
    Workspaces,
    /// Settings (QML settingsPopup; rendered as a right panel).
    Settings,
    /// KDE Places favorites (QML favoritesPopup; rendered as a window).
    Favorites,
    /// Touch command history (QML cmdHistorySheet, bottom sheet).
    CmdSheet,
    /// The command palette (QML paletteDrawer; rendered as a window).
    Palette,
    /// Web rendering settings — global and per site. No permanent control
    /// opens this: the defaults suit almost everyone, and a knob on screen
    /// for something touched twice a year costs everyone who never does.
    WebSettings,
}

impl Surface {
    /// Every variant, for exhaustive tests. If a surface is added, the
    /// `all_pairs_are_exclusive` test covers it by construction — but only if
    /// it is added HERE too, which `variant_count_matches_the_enum` enforces.
    pub const ALL: [Surface; 8] = [
        Surface::Blocks,
        Surface::MissingPkg,
        Surface::Workspaces,
        Surface::Settings,
        Surface::Favorites,
        Surface::CmdSheet,
        Surface::Palette,
        Surface::WebSettings,
    ];
}

/// Zero or one surface up. `Default` is none.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Surfaces {
    active: Option<Surface>,
}

impl Surfaces {
    pub fn active(&self) -> Option<Surface> {
        self.active
    }

    pub fn is(&self, s: Surface) -> bool {
        self.active == Some(s)
    }

    /// Open `s`, replacing whatever was up. Replacement, not refusal: a voice
    /// command ("open settings") issued while the blocks drawer is open must
    /// show settings, exactly as the QML's modal open would.
    pub fn open(&mut self, s: Surface) {
        self.active = Some(s);
    }

    /// Chip semantics: the surface's own control closes it, any other
    /// surface's control replaces it.
    pub fn toggle(&mut self, s: Surface) {
        self.active = if self.active == Some(s) { None } else { Some(s) };
    }

    /// Close whatever is up. Idempotent.
    pub fn close(&mut self) {
        self.active = None;
    }

    /// The keyboard half of modality: while any surface is up, key events
    /// must not reach the PTY. The backdrop cannot do this — the terminal
    /// reads raw `ctx.input` events, and egui does not remove events because
    /// a widget looked at them — so the `drain_egui` call site gates on this.
    /// Without it, typing into the palette also types into the shell (the
    /// double-input path existed in code before this model; found by reading,
    /// not observed on device).
    pub fn captures_input(&self) -> bool {
        self.active.is_some()
    }

    /// Close `s` if — and only if — it is the active surface.
    ///
    /// Exists for one caller: the palette's after-run cleanup. Running
    /// `ToggleSettings` from the palette replaces Palette with Settings; a
    /// blind `close()` at the end of `palette_ui` would then shut the very
    /// panel the command just opened. Under the old independent flags this
    /// bug was impossible (`palette_open = false` touched nothing else), so
    /// the unification is what creates the need — and this is the method that
    /// discharges it.
    pub fn close_if(&mut self, s: Surface) {
        if self.active == Some(s) {
            self.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard that keeps `ALL` honest: adding an enum variant without
    /// extending `ALL` silently exempts it from every loop below.
    #[test]
    fn variant_count_matches_the_enum() {
        // No std::mem::variant_count on stable; a match that fails to compile
        // when a variant is added is the enforcement.
        for s in Surface::ALL {
            match s {
                Surface::Blocks
                | Surface::MissingPkg
                | Surface::Workspaces
                | Surface::Settings
                | Surface::Favorites
                | Surface::CmdSheet
                | Surface::Palette
                | Surface::WebSettings => {}
            }
        }
        let mut sorted: Vec<_> = Surface::ALL.iter().map(|s| format!("{s:?}")).collect();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), Surface::ALL.len(), "ALL repeats a variant");
    }

    /// The core claim: for EVERY ordered pair, opening the second closes the
    /// first. Not a sample of pairs — the loop asserts it found the full
    /// grid, per the house rule that a scan proves it scanned something.
    #[test]
    fn all_pairs_are_exclusive() {
        let mut pairs = 0;
        for a in Surface::ALL {
            for b in Surface::ALL {
                if a == b {
                    continue;
                }
                let mut m = Surfaces::default();
                m.open(a);
                m.open(b);
                assert!(m.is(b) && !m.is(a), "open({b:?}) over {a:?} left {:?}", m.active());
                assert_eq!(m.active(), Some(b));
                pairs += 1;
            }
        }
        // n(n-1) ordered pairs, derived rather than written: a literal has
        // to be edited every time a surface is added, and the edit is the
        // part people get wrong.
        let n = Surface::ALL.len();
        assert_eq!(pairs, n * (n - 1), "{n} surfaces should give {} ordered pairs",
                   n * (n - 1));
    }

    #[test]
    fn toggle_of_the_open_surface_closes_it_and_of_another_replaces() {
        for a in Surface::ALL {
            let mut m = Surfaces::default();
            m.toggle(a);
            assert!(m.is(a), "toggle from closed opens");
            m.toggle(a);
            assert_eq!(m.active(), None, "toggle of the open surface closes");
            for b in Surface::ALL {
                if a == b {
                    continue;
                }
                m.open(a);
                m.toggle(b);
                assert!(m.is(b), "toggle({b:?}) over {a:?} replaces");
            }
        }
    }

    #[test]
    fn close_is_idempotent_and_total() {
        let mut m = Surfaces::default();
        m.close();
        assert_eq!(m.active(), None);
        m.open(Surface::Settings);
        m.close();
        m.close();
        assert_eq!(m.active(), None);
    }

    /// The palette-runs-a-surface-command sequence, in the order the binary
    /// performs it. This is the regression `close_if` exists to prevent; a
    /// version of `close_if` that closes unconditionally fails HERE, not on
    /// device three sessions later.
    #[test]
    fn running_toggle_settings_from_the_palette_leaves_settings_open() {
        let mut m = Surfaces::default();
        m.open(Surface::Palette);
        m.toggle(Surface::Settings); // the command runs: Palette → Settings
        m.close_if(Surface::Palette); // palette_ui's after-run cleanup
        assert!(m.is(Surface::Settings), "the panel the command opened must survive");
        // And the cleanup still works when the command did NOT move surfaces
        // (e.g. `Split`): the palette itself must close.
        let mut m = Surfaces::default();
        m.open(Surface::Palette);
        m.close_if(Surface::Palette);
        assert_eq!(m.active(), None);
    }

    /// The binary has no independent surface flags left. A structural check
    /// in the style of `chrome_mode`'s `the_left_edge_is_a_single_panel`:
    /// the panels are built inside a 3000-line `update` that cannot be driven
    /// headlessly, so the source is the testable artifact.
    ///
    /// The found-something guard comes first: if the binary stopped using
    /// `self.surfaces.` (say, the field was renamed), the zero-counts below
    /// would pass vacuously while the flags came back under new names.
    #[test]
    fn the_binary_routes_every_surface_through_the_model() {
        let src = include_str!("bin/voiceterm_egui.rs");
        let gates = src.matches("self.surfaces.").count();
        assert!(
            gates >= 12,
            "expected the binary to consult the surface model at every chip, \
             action, gate and close (found {gates} uses of `self.surfaces.`)"
        );
        for flag in [
            "blocks_open",
            "missing_pkg_open",
            "favorites_open",
            "workspaces_open",
            "settings_open",
            "palette_open",
        ] {
            assert_eq!(
                src.matches(flag).count(),
                0,
                "`{flag}` is back in the binary — surfaces must be exclusive, \
                 which means one model, not one bool per surface. Route it \
                 through `self.surfaces` (see surface.rs)."
            );
        }
        // Every transient right panel became an overlay drawer; a
        // `SidePanel::right` reappearing means a surface bypassed both the
        // model and its QML form.
        assert_eq!(
            src.matches("egui::SidePanel::right(").count(),
            0,
            "a right SidePanel is back — transient surfaces render via \
             overlay::show, not standing panels (C10-0a)."
        );
        // The keyboard half of modality: the PTY drain must consult the
        // model. Without this, typing into an open surface also types into
        // the shell (raw ctx.input readers see every event).
        assert_eq!(
            src.matches("if self.surfaces.captures_input()").count(),
            1,
            "the drain gate is gone — pty_input::drain_egui must be skipped \
             while a surface is up, or keys reach the shell through the \
             backdrop."
        );
        // THE LIVE TERMINAL MODE. Passing TermMode::default() hardcodes
        // "never in application-cursor mode", which makes the encoder's
        // SS3 branch and the parser's DECCKM tracking unreachable —
        // arrows stop working inside fzf, vim and less while every unit
        // test still passes, because each piece is correct on its own.
        assert_eq!(
            src.matches("pty_input::TermMode::default()").count(),
            0,
            "drain_egui is being handed a default mode again — pass \
             the session grid's key_mode(), or app-cursor arrows break."
        );
        // FROM THE FED GRID. app.terminal.grid is never fed for the
        // focused pane, so asking it returns the default forever — the
        // expression looks right and the behaviour does not change.
        assert!(
            src.contains("self.app.terminal.grid.key_mode()"),
            "the key mode must come from the grid feed_pty writes, which is \
             the DISPLAYED one."
        );
        // ONE INPUT OWNER. uikit_term::App::handle_input and
        // pty_input::drain_egui both read this frame's events; if the
        // widget is not told the host owns encoding, every keystroke
        // reaches the shell twice and every Enter runs the command twice.
        // THE SUBMITTED LINE IS A NOTIFICATION, NOT A SOURCE OF BYTES.
        // The raw key path already sent it character by character; a
        // second append is the doubling bug.
        assert!(
            !src.contains("pty_out.extend_from_slice(line.as_bytes())"),
            "the submitted line is emitting bytes again — the raw path \
             already sent them and the shell will run everything twice."
        );
        // ONE RISK GATE, IN THE KEY PATH. Asking on `submitted` is too
        // late: the Enter byte went to the shell on the same keypress.
        assert_eq!(
            src.matches("cmd_risk::withhold_enter(").count(),
            1,
            "the Enter-withholding decision is inlined again — it gates \
             destructive commands and belongs in a tested function."
        );
        assert_eq!(
            src.matches("ConfirmDialog::new(").count(),
            1,
            "a second risk-confirmation site is back — the one that can \
             actually withhold Enter lives in the key path."
        );
        assert_eq!(
            src.matches("set_input_owner(").count(),
            1,
            "the input-owner declaration is gone — the widget and the raw \
             path will both encode, and `ls` will run twice."
        );
        // THE FOCUSED PANE'S SHELL. `hosted` includes pane_focus, so the
        // reconcile loop never spawns for it, while swap_focus hands a
        // newly focused pane None. Without this call a split produces a
        // terminal that echoes locally and never runs anything.
        assert_eq!(
            src.matches("pane_host::focused_needs_shell(").count(),
            1,
            "the focused-pane shell check is gone — a split pane will have \
             no shell and no command will run in it."
        );
        // WAKING THE UI. An immediate-mode GUI draws when asked; a shell
        // answering half a frame after Enter is not an input event, so
        // without a waker the output sits in the reader's buffer until
        // the next keystroke or mouse move.
        assert!(
            src.contains("fn repaint_waker("),
            "the pty waker is gone — shell output will only appear when \
             something else happens to repaint the UI."
        );
        assert_eq!(
            src.matches("PtyHost::spawn(80, 24)").count(),
            0,
            "a PtyHost is being spawned without a waker again."
        );
        // COMMANDS TYPED BY A SURFACE MUST MIRROR INTO THE LINE BUFFER.
        // cmd_risk::withhold_enter reads app.terminal.input to decide
        // whether to hold Enter. A surface that appends only to pty_out
        // leaves the shell holding a command the gate cannot see, and
        // `rm -rf` dropped from the command sheet then runs unconfirmed.
        assert!(
            src.contains("fn stage_command("),
            "the type-into-shell funnel is gone — surfaces will append \
             commands the risk gate cannot see."
        );
        // Four surfaces stage commands; each must go through the funnel.
        assert!(
            src.matches("self.stage_command(").count() >= 4,
            "a surface stopped using stage_command — check whether it now \
             appends to pty_out directly."
        );
        // A LINE DISCARD MUST REACH BOTH COPIES. Ctrl-C/Ctrl-U empty the
        // shell's line; the widget's copy is what the risk gate reads.
        assert!(
            src.contains("keys::clears_the_line("),
            "line discards are no longer mirrored — the risk gate will \
             classify a line the shell has already thrown away."
        );
        // INPUT IS NOT OUTPUT. `Terminal::feed` PARSES bytes as terminal
        // output into a grid; the keycap bar, the pipe menu and the
        // brace menu are input affordances. Calling feed from one of
        // them draws the character on a grid nothing renders and sends
        // the shell nothing at all.
        assert_eq!(
            src.matches("self.app.terminal.feed(").count(),
            1, // feed_pty only; the background drain binds a local `app`
            "an input affordance is calling the OUTPUT path again — use \
             send_input for keys and show_line for text this app produced."
        );
        assert!(
            src.contains("fn send_input(") && src.contains("fn show_line("),
            "the input/output split is gone."
        );
        // NOTHING MAY READ THE WIDGET'S GRID. It is not fed for the
        // focused pane, so every read is empty or stale: "Clear" cleared
        // nothing, "copy screen" copied nothing, page-up paged nothing.
        // FIND SEARCHES THE GRID WITH THE TEXT. The widget's find bar
        // searches its own unfed grid, and nothing here ever rendered
        // it — the menu item set a flag no frame read.
        // The widget's find bar is CORRECT again: feed_pty mirrors the
        // shell into app.terminal, so its grid has the history. It must
        // also be rendered — open_search only sets a flag.
        assert!(
            src.contains("self.app.search_ui(ui)"),
            "open_search sets a flag that no frame renders; call search_ui."
        );
        // A SELECTION MUST COME FROM THE FED GRID. handle_gestures
        // hit-tests correctly and then reads text from the widget's
        // unfed grid, so a tap yields Some("") — which DEFEATS the copy
        // path's fallback to the screen, giving an empty clipboard.
    }
}
