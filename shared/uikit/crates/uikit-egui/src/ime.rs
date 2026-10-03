//! Making an on-screen keyboard reach a text field.
//!
//! # The bug this is aimed at
//!
//! "Input fields accept one character on mobile" is the oldest open report in
//! this tree, and it has survived two attempts. Both looked in egui. The record
//! of them is why this module looks where it does:
//!
//! * `PORT_STATUS.md` §1: three probes did not reproduce it — a bare
//!   `TextEdit`, a real ported page, and the whole shell. Two of those probes
//!   invented the bug before the third measured it. The conclusion was that the
//!   fault is not reachable from a synthetic-event harness.
//! * `C13_TODO` §66: "the cause is below egui — the on-screen keyboard's input
//!   method — and the next step is capturing what events actually arrive, not
//!   more egui changes."
//!
//! # What is actually true about the plumbing
//!
//! On Wayland an on-screen keyboard delivers text through the text-input
//! protocol, which winit surfaces as `Event::Ime`. **winit does not enable IME
//! on a window unless something calls `set_ime_allowed(true)`.**
//!
//! `egui-winit` 0.34 does call it — at `lib.rs:1118` — but only when egui
//! reports an IME rect, and egui only reports one when a `TextEdit` ALREADY HAS
//! FOCUS. Read the ordering:
//!
//! 1. the user taps the field; focus is acquired during that frame
//! 2. at the END of the frame egui emits an IME rect, and only then does
//!    `set_ime_allowed(true)` reach the compositor
//! 3. the compositor activates text-input and attaches the keyboard
//!
//! So the window spends the whole first frame of every focus — and everything
//! before the first focus of the session — with IME disabled. Nothing in this
//! fleet calls `set_ime_allowed` or sends `ViewportCommand::IMEAllowed`
//! proactively; the grep is empty across `apps/`, `shared/` and `system/`.
//!
//! # RETRACTED: do not call `allow_ime` from a shared hook
//!
//! It was called from `fonts::install`, which every themed app reaches, so every window asked the
//! compositor for text input at startup and for good. On the Fairphone that raised the on-screen
//! keyboard over apps with NO field focused (seen: vidkit-player, an empty home screen). The premise
//! below ("the window spends the first frame of every focus with IME disabled") costs nothing in
//! practice: the keyboard appears a frame after the tap and nobody types in that frame. The function
//! stays for an app that really wants IME on from the start (a terminal), and it is not called by default.
//!
//! # What this module does, and what it does not claim
//!
//! [`allow_ime`] sends `ViewportCommand::IMEAllowed(true)` once at startup, so
//! text-input is live before the first tap rather than a frame after it. That
//! removes the ordering dependency entirely and costs one command per process.
//!
//! **This is not proof of the fix.** It is the one mechanism in the chain that
//! is demonstrably not being set up, found by reading the plumbing rather than
//! by guessing at egui again. Whether it accounts for the reported symptom
//! needs the device — and `input_probe` is the instrument for that, which is
//! why it was built first and stays.
//!
//! If a keystroke still arrives as a lone `Text` event with the field refusing
//! the second character, IME was never the problem and the next suspect is the
//! keyboard surface stealing focus. The probe distinguishes those in one
//! screenshot.

use egui::{Context, ViewportCommand};

const SENT: &str = "uikit-ime-allowed";

/// Ask the windowing layer to allow IME, once per context.
///
/// Idempotent behind a context flag, so it is free to call every frame from a
/// shared entry point — which is how it reaches every app in the fleet without
/// each one remembering.
///
/// A no-op where there is no windowing layer to hear it: a command to a
/// viewport that does not exist is dropped, so this is safe in a test harness
/// and in any embedded surface.
pub fn allow_ime(ctx: &Context) {
    if ctx.data(|d| d.get_temp::<bool>(egui::Id::new(SENT))).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(SENT), true));
    ctx.send_viewport_cmd(ViewportCommand::IMEAllowed(true));
}

/// Has [`allow_ime`] already run for this context?
///
/// For tests, and for a caller that wants to know whether it is the one that
/// did it.
pub fn ime_requested(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::new(SENT))).unwrap_or(false)
}

/// Turns an on-screen keyboard's bare commits into plain text, so a `TextEdit` keeps them.
///
/// # The fault
///
/// A Wayland on-screen keyboard (rusty-board, squeekboard, Maliit) commits text through
/// text-input-v3 without composing anything first. winit reports each as
/// `Preedit("", None)` + `Commit(text)`, and egui-winit 0.34 on Linux forwards only the
/// `Commit` (it drops `Preedit(_, None)` except on macOS, and ignores winit's `Enabled`,
/// enabling IME on a preedit instead -- which never comes). egui's `TextEdit` then inserts
/// a commit only if the cursor is where it was at the last `Enabled`/`Preedit`
/// (`builder.rs`, the `ImeEvent::Commit` arm: `cursor_range.secondary.index ==
/// state.ime_cursor_range.secondary.index`). With no such event that position is the
/// default, 0: the first character of an empty field lands, the cursor moves to 1, and
/// every later commit is silently dropped. That is "fields accept one character" on the
/// phone, and it is why desktop IBus (which DOES compose) never reproduced it.
///
/// # The repair
///
/// A `Commit` that arrives while nothing is being composed is not the end of a
/// composition -- it is typed text, so it becomes `Event::Text`, which `TextEdit` inserts
/// at the cursor unconditionally. A commit that ends a real composition (a non-empty
/// `Preedit` came first) is left for egui, whose composition handling is correct for it.
/// Newline commits are left alone: `TextEdit` takes newlines from `Key::Enter`.
///
/// Run it on the raw input before egui sees it -- `eframe::App::raw_input_hook`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImeRepair {
    composing: bool,
}

impl ImeRepair {
    /// Rewrite this frame's events in place.
    pub fn repair(&mut self, events: &mut [egui::Event]) {
        use egui::{Event, ImeEvent};
        for e in events.iter_mut() {
            match e {
                Event::Ime(ImeEvent::Preedit(t)) => self.composing = !t.is_empty(),
                Event::Ime(ImeEvent::Disabled) => self.composing = false,
                Event::Ime(ImeEvent::Commit(t)) => {
                    let ends_composition = std::mem::replace(&mut self.composing, false);
                    if !ends_composition && !t.is_empty() && t != "\n" && t != "\r" {
                        *e = Event::Text(std::mem::take(t));
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_commit_becomes_text_and_a_composed_one_does_not() {
        use egui::{Event, ImeEvent};
        let mut r = ImeRepair::default();
        // On-screen keyboard: commit, disabled.
        let mut ev = vec![Event::Ime(ImeEvent::Commit("a".into())), Event::Ime(ImeEvent::Disabled)];
        r.repair(&mut ev);
        assert_eq!(ev[0], Event::Text("a".into()));
        // IBus-style composition: enabled, preedit, commit -- egui's to handle.
        let mut ev = vec![
            Event::Ime(ImeEvent::Enabled),
            Event::Ime(ImeEvent::Preedit("ni".into())),
            Event::Ime(ImeEvent::Commit("你".into())),
            Event::Ime(ImeEvent::Preedit(String::new())),
        ];
        r.repair(&mut ev);
        assert_eq!(ev[2], Event::Ime(ImeEvent::Commit("你".into())), "a composition's commit was rewritten");
        // The composition ended; the next bare commit is text again.
        let mut ev = vec![Event::Ime(ImeEvent::Commit("b".into()))];
        r.repair(&mut ev);
        assert_eq!(ev[0], Event::Text("b".into()));
        // Composition state carries ACROSS frames: preedit in one, commit in the next.
        let mut ev = vec![Event::Ime(ImeEvent::Preedit("k".into()))];
        r.repair(&mut ev);
        let mut ev = vec![Event::Ime(ImeEvent::Commit("k".into()))];
        r.repair(&mut ev);
        assert_eq!(ev[0], Event::Ime(ImeEvent::Commit("k".into())));
        // Newlines and empty commits stay IME events.
        let mut ev = vec![Event::Ime(ImeEvent::Commit("\n".into())), Event::Ime(ImeEvent::Commit(String::new()))];
        r.repair(&mut ev);
        assert!(matches!(ev[0], Event::Ime(_)) && matches!(ev[1], Event::Ime(_)));
    }

    /// The fault and the repair through a real `TextEdit`, independent of any app.
    #[test]
    fn a_text_edit_keeps_every_bare_commit_only_with_the_repair() {
        use egui::{Event, ImeEvent};
        let run = |repair: bool| -> String {
            let ctx = Context::default();
            let mut text = String::new();
            let mut r = ImeRepair::default();
            let id = egui::Id::new("field");
            for i in 0..8 {
                let mut events = match i {
                    0 | 1 => vec![],
                    _ => vec![Event::Ime(ImeEvent::Commit(["h", "e", "l", "l", "o", "!"][i - 2].into())), Event::Ime(ImeEvent::Disabled)],
                };
                if repair {
                    r.repair(&mut events);
                }
                let input = egui::RawInput { events, ..Default::default() };
                let _ = ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let resp = ui.add(egui::TextEdit::singleline(&mut text).id(id));
                        if i == 0 {
                            resp.request_focus();
                        }
                    });
                });
            }
            text
        };
        assert_eq!(run(true), "hello!");
        assert_eq!(run(false), "h", "the premise: egui alone keeps only the first commit");
    }

    #[test]
    fn the_request_is_sent_once_and_then_remembered() {
        // Called from a shared per-frame entry point, so re-sending a viewport
        // command sixty times a second would be the cost of putting it there.
        let ctx = Context::default();
        assert!(!ime_requested(&ctx), "a fresh context claims it already asked");
        ctx.begin_pass(egui::RawInput::default());
        allow_ime(&ctx);
        assert!(ime_requested(&ctx));
        allow_ime(&ctx);
        assert!(ime_requested(&ctx), "the flag was lost on the second call");
        let _ = ctx.end_pass();
    }

    #[test]
    fn two_contexts_do_not_share_the_flag() {
        // The flag lives in context data, so an app with a second viewport
        // must still get its own request — otherwise the second window is the
        // one that cannot be typed into, which is a worse bug to chase than
        // the first.
        let a = Context::default();
        let b = Context::default();
        a.begin_pass(egui::RawInput::default());
        allow_ime(&a);
        let _ = a.end_pass();
        assert!(ime_requested(&a));
        assert!(!ime_requested(&b), "a second context inherited the flag");
    }
}
