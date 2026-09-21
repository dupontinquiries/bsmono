//! bar_pages — mounts the Qt predecessor's grammar-keyed bottom bar paging.
//!
//! ## What this closes
//!
//! `bars.rs` ports `bottom_bar.rs`'s pure model (buttons, fan-out options)
//! faithfully, and `voiceterm_egui.rs` draws it with `default_bar()` — a
//! fixed Tabs/Workspaces/QuickLaunch set that never changes. The Qt side's
//! actual behaviour is `bar_pages.rs::bar_for`: the bar's CONTENTS change with
//! what program has the terminal (an editor, a REPL, a live dictation
//! session), each page offering only what applies there, with Tabs always
//! present so navigation never disappears. That switch was reported as
//! missing — "it used to be scrolling and prioritized ... and worked much
//! better in a phone workflow" — and reading the fixed-bar call site
//! confirms it: `default_bar` has no page argument to switch on.
//!
//! `bar_pages.rs` itself was never Qt-bound — its own doc calls it "PURE:
//! composes bottom_bar + editor_voice + dictate. No QML, no IO." What kept it
//! unported was its dependency chain, mounted here in one place:
//!
//! ```text
//! bar_pages -> editor_voice   (Qt-free, no crate:: deps)
//!           -> dictate::Ctl   (Qt-free; needs code_symbols + repair)
//!           -> bottom_bar     (already mounted independently by
//!                              bars.rs and archived_view.rs — a third
//!                              mount here is fine, `#[path]` scopes each
//!                              to its own module tree)
//! ```
//!
//! `grammar_context::grammar_label` — the function that PICKS the page — also
//! needs `repl_voice`, mounted alongside it.
//!
//! ## The piece that did not exist to key off of
//!
//! `grammar_label` takes `foreground_cmd: &str`. The Qt side gets it from
//! `pty_term.rs::foreground_command`, a `tcgetpgrp` + `/proc/<pid>/comm` read
//! on the pty's master fd. This crate's `PtyHost` wraps `portable_pty`
//! instead of a raw fd, and had no equivalent — there was no foreground-
//! process signal anywhere in this crate to mount `bar_pages` against, which
//! is the deeper reason it stayed unported rather than being an oversight.
//! `PtyHost::foreground_command` (this commit, `pty_host.rs`) closes that: it
//! calls `portable_pty::MasterPty::process_group_leader`, which already wraps
//! the same `tcgetpgrp`, then reads `/proc/<pid>/comm` exactly as the Qt side
//! does — same contract, so `grammar_label`'s editor/REPL detectors see the
//! same strings on both builds.
//!
//! ## Why `include!` and not `#[path] mod`
//!
//! `bar_pages.rs` on the Qt side refers to its dependencies by CRATE-ROOT
//! paths (`crate::bottom_bar`, `crate::dictate::Ctl`, `crate::editor_voice`),
//! which is correct there because they live at that crate's root. Mounted as
//! a `#[path] mod bar_pages_core` here, those `crate::` paths resolve at THIS
//! (the egui) crate's root instead, which has no `bottom_bar`/`dictate`/
//! `editor_voice` of its own — every one of those three lines failed as an
//! unresolved import, naming the egui crate root as the search location. That
//! was tried first and is recorded here so it is not tried again.
//!
//! `include!` splices the file's text in at this point instead of creating a
//! new module scope, so its `use crate::X` lines are rewritten to `use
//! super::X` below and then resolve against the sibling modules mounted in
//! THIS file — which is what makes one mount site enough.
//!
//! ## What is NOT wired here
//!
//! The call site. `draw_bottom_bar` in `voiceterm_egui.rs` still calls
//! `default_bar`. Swapping it needs, per frame: the active pane's
//! `PtyHost::foreground_command()`, the terminal grid's last non-empty line
//! (`repl_voice::detect_repl` reads a prompt line, not a process name), and
//! the live dictation flag — three reads this module does not have access to
//! from here, and guessing at their call-site plumbing without the binary in
//! front of me is how the swipe-dispatch drift from earlier this session
//! happened. What this commit makes true is that `bar_pages::bar_for` and
//! `grammar_context::grammar_label` compile, are tested, and have a real
//! foreground-process reading to call them with — the wiring is the next,
//! smaller step.

//! ## Why the sibling mounts moved to the crate root
//!
//! An earlier version of this file mounted `editor_voice`, `repl_voice`,
//! `dictate`, `code_symbols` and `repair` as submodules HERE. That failed to
//! compile two ways: `use editor_voice::{self, Editor}` re-imports the module
//! under its own name (E0255, since `mod editor_voice` and `use
//! editor_voice::self` both bind the name `editor_voice` in this scope), and
//! `dictate.rs`/`repair.rs` reference EACH OTHER and `code_symbols` via
//! `crate::dictate` / `crate::code_symbols` — correct in the Qt binary, where
//! they are crate-root siblings, but resolving to nothing when nested under
//! `bar_pages` instead (E0432/E0433, "cannot find `dictate` in `crate`").
//!
//! They are mounted at `lib.rs`'s crate root now, which is where `crate::`
//! inside those Qt-authored files actually looks. This file needs its own
//! `grammar_context` mount, but reaches `BottomBar`/`BarButton` by
//! RE-EXPORTING `bars::bottom_bar` rather than mounting a second `#[path]`
//! copy of `bottom_bar.rs`.
//!
//! A second mount was tried first and is worth recording exactly why it
//! failed: `#[path]` scoping each mount to its own module tree means two
//! mounts of one source file produce two DISTINCT types — `bars::bottom_bar
//! ::BottomBar` and `bar_pages::bottom_bar::BottomBar` look identical and are
//! not the same type to the compiler. `bars::bar_ui` takes the former; a
//! `BottomBar` built by this module's `bar_for` would have been the latter,
//! and the call site (`bar_ui(ui, &bar)`) would not type-check no matter how
//! the caller phrased it. Re-exporting `bars`'s mount instead of adding a
//! third means there is exactly one `BottomBar` type for the whole crate to
//! agree on, which is the property three separate mounts cannot have no
//! matter how carefully each is scoped.
//!
//! `archived_view.rs` still mounts its OWN copy, unchanged by this. It draws
//! from a page list it owns and never calls `bars::bar_ui` or `bar_for`, so
//! it never needs to agree with either of the other two on the type — the
//! problem above only bites when a `BottomBar` value has to cross a module
//! boundary, and archived_view's never does.

pub use crate::bars::bottom_bar;

#[path = "../../src/grammar_context.rs"]
#[allow(dead_code, unused_imports)]
pub mod grammar_context;

use bottom_bar::{BarButton, BarButtonKind, BarOption, BottomBar};
use crate::dictate::Ctl;
use crate::editor_voice::{self, Editor};

include!("bar_pages_core.rs");

/// Derive today's [`Page`] from a live terminal session, matching
/// `main.rs::current_grammar_label` on the Qt side field for field.
///
/// This is the ONE call site that should ever compute the three inputs
/// `grammar_context::grammar_label` needs — a second call site re-deriving
/// "last non-empty line" or "is this the terminal tab" independently is
/// exactly how the swipe-dispatch rule drifted between voiceterm's own two
/// copies earlier in this crate's history, and how bar_pages' Qt
/// implementation and a hand-rolled egui equivalent would drift from each
/// other the first time either changed. Callers pass in what only they can
/// know (the active pane's process and its rendered lines, whether this tab
/// is the terminal tab, whether dictation is live) and get back the `Page`
/// grammar_label + page_for would have produced together.
///
/// `all_lines` is whatever `app.terminal.grid.all_lines()` returns for the
/// FOCUSED pane — this function does not reach into a pane store itself,
/// because doing so would tie bar_pages to this crate's particular
/// App/PtyHost shapes, and bar_pages.rs is shared, byte-identical text with
/// the Qt build (see the module doc above). The caller already has to know
/// which pane is focused to draw anything else; passing its lines here costs
/// nothing new.
pub fn current_page(
    foreground_cmd: Option<&str>,
    all_lines: &[String],
    on_terminal_tab: bool,
    dictating: bool,
) -> Page {
    let fg = foreground_cmd.unwrap_or("");
    let last_line = all_lines
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(String::as_str)
        .unwrap_or("");
    let label = grammar_context::grammar_label(fg, last_line, on_terminal_tab);
    page_for(&label, dictating)
}

#[cfg(test)]
mod current_page_tests {
    use super::*;

    #[test]
    fn no_foreground_process_and_empty_grid_is_plain_terminal() {
        // The state a freshly spawned shell is in before anything has run.
        assert_eq!(current_page(None, &[], true, false), Page::Terminal);
    }

    #[test]
    fn an_editor_in_the_foreground_wins_regardless_of_grid_content() {
        assert_eq!(
            current_page(Some("hx"), &["$ hx notes.md".into()], true, false),
            Page::Editor(editor_voice::detect_editor("hx").unwrap())
        );
    }

    #[test]
    fn a_repl_prompt_is_read_from_the_last_nonempty_line_not_the_first() {
        // `detect_repl` requires the line to be the BARE prompt token — see
        // `prompt_is_bare`, which exists specifically to reject
        // `print('>>>')` and `x >>> y`. A first draft of this test used
        // ">>> 1 + 1" as the prompt line, which `detect_repl` correctly
        // does not match; that would have asserted a Repl page for input
        // this function cannot actually classify as one.
        let lines = vec![
            "1 + 1".to_string(),
            "2".to_string(),
            ">>>".to_string(), // the bare prompt, with nothing typed yet
        ];
        // No foreground process name distinguishes a REPL (it IS the shell,
        // interpreting its own input) — the prompt text is the only signal,
        // and it must come from the last line with content.
        assert_eq!(current_page(None, &lines, true, false), Page::Repl("python".into()));
    }

    #[test]
    fn dictation_outranks_a_live_editor() {
        // grammar_label / page_for's own precedence: dictation is session
        // state the terminal cannot see, so it is checked first regardless
        // of what is in the foreground.
        assert_eq!(current_page(Some("hx"), &[], true, true), Page::Dictation);
    }

    #[test]
    fn the_web_tab_falls_back_to_web_not_terminal() {
        assert_eq!(current_page(None, &[], false, false), Page::Web);
    }
}

#[cfg(test)]
mod drift_guard {
    /// `bar_pages_core.rs` is a VERBATIM COPY of `../../src/bar_pages.rs`
    /// with its 37-line `//!` header removed — `include!` splices text, and an
    /// inner doc comment cannot appear mid-file, which is the only reason the
    /// copy exists rather than a path to the original.
    ///
    /// A copy with no guard is a fork with a delay. Everything else this
    /// frontend shares with the Qt app is `#[path]`-mounted precisely so one
    /// fix serves both; this file is the exception, and nothing marked it as
    /// one. A change to the Qt side's paging would land in the Qt app and not
    /// here, and the symptom would be the bar behaving differently in the two
    /// front-ends with both files looking correct in isolation.
    ///
    /// So: fail loudly when they diverge. Comparing the BODIES, since the
    /// header is the one intended difference.
    #[test]
    fn the_copy_still_matches_the_shared_source() {
        let orig = include_str!("../../src/bar_pages.rs");
        let copy = include_str!("bar_pages_core.rs");
        // Drop the leading `//!` block AND the leading `use` lines; everything
        // after them must match exactly.
        //
        // THE IMPORTS ARE THE SECOND INTENDED DIFFERENCE, and this guard only
        // knew about the first. The original imports `crate::bottom_bar::..`;
        // this frontend mounts that module at a different path and the
        // includer above supplies `bottom_bar::..` itself, so the copy
        // deliberately has NO `use` lines. Comparing them made this guard fail
        // on two files whose logic was byte-identical -- a false alarm, and the
        // kind that trains people to delete a guard rather than read it.
        //
        // Skipping them is not a loophole: an import cannot change behaviour
        // without the body that uses it also changing, and the body is still
        // compared in full.
        let strip = |text: &str| -> String {
            text.lines()
                .skip_while(|l| {
                    let t = l.trim_start();
                    t.starts_with("//!") || t.starts_with("use ") || l.trim().is_empty()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let body = strip(orig);
        let copy_body = strip(copy);
        assert_eq!(
            body.trim_end(),
            copy_body.trim_end(),
            "bar_pages_core.rs has drifted from src/bar_pages.rs. They are the same \
             code by design -- re-copy the original (minus its //! header) rather \
             than patching one side."
        );
    }
}
