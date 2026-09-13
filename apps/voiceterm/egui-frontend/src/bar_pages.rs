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
//!     bar_pages -> editor_voice   (Qt-free, no crate:: deps)
//!               -> dictate::Ctl   (Qt-free; needs code_symbols + repair)
//!               -> bottom_bar     (already mounted independently by
//!                                  bars.rs and archived_view.rs — a third
//!                                  mount here is fine, `#[path]` scopes each
//!                                  to its own module tree)
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

#[path = "../../src/bottom_bar.rs"]
#[allow(dead_code, unused_imports)]
pub mod bottom_bar;

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

#[path = "../../src/grammar_context.rs"]
#[allow(dead_code, unused_imports)]
pub mod grammar_context;

use bottom_bar::{BarButton, BarButtonKind, BarOption, BottomBar};
use dictate::Ctl;
use editor_voice::{self, Editor};

include!("bar_pages_core.rs");
