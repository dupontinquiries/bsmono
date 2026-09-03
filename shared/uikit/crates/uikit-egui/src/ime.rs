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

#[cfg(test)]
mod tests {
    use super::*;

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
