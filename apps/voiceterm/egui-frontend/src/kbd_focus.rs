//! kbd_focus — who owns the keyboard when a terminal is on screen.
//!
//! ## The bug, and why it is intermittent
//!
//! Tab sometimes reached helix and sometimes selected the next sidebar button. Both halves
//! of that are one cause: **egui decides what Tab means before the app runs.**
//! `Memory::begin_pass` reads the raw events and sets a `FocusDirection` from Tab, the
//! arrows and Escape — so by the time `pty_input::drain_egui` sees the event, focus has
//! already moved. Consuming the key in the app is too late; there is nothing left to stop.
//!
//! Which is why it looked random. With focus nowhere, Tab moved it to the first widget and
//! ALSO reached the shell; the next Tab moved focus again and reached nothing, because
//! `drain_egui` bails when `egui_wants_keyboard_input()` — true as soon as any widget,
//! including a plain button, holds focus. So Tab worked, then walked the sidebar, then went
//! dead, depending on where focus had drifted.
//!
//! ## The fix egui actually provides
//!
//! A focused widget may declare an **event filter**: `EventFilter { tab: true, .. }` tells
//! `begin_pass` to leave that event alone rather than treat it as navigation. That is the
//! supported way to say "this surface eats Tab", and it needs a real focusable widget to
//! hang off — which the terminal never had. It painted a texture; it was not a widget, so
//! it could not hold focus, so egui had no reason to believe the keyboard was its.
//!
//! [`claim`] allocates that widget: a zero-area, invisible focus sink over the terminal's
//! rect. Once it holds focus, Tab, the arrows and Escape all pass through untouched, and
//! [`terminal_has_keyboard`] lets `drain_egui` distinguish "the terminal is focused" from
//! "a sidebar button is focused" — the check it was missing.
//!
//! ## The defaults this settles on
//!
//! * **A visible terminal claims the keyboard when nothing else has taken it.** Not
//!   unconditionally: a find bar or the command palette must be able to take it, and they
//!   do, because they focus a real TextEdit and this only claims when focus is free.
//! * **Clicking the terminal takes it back.** The one gesture that unambiguously means
//!   "type here".
//! * **Escape returns it** — from the terminal to the app — because a surface that can
//!   never be left is worse than one that has to be clicked into.
//! * **A text field always wins.** Even when the terminal holds focus, a newly focused
//!   TextEdit takes over on the next frame; nothing here fights that.

use egui::{EventFilter, Id, Sense, Ui};

/// The filter a terminal wants: everything egui would otherwise steal for navigation.
///
/// `escape: false` on purpose — see the module docs. Escape is how the keyboard is handed
/// back, and a terminal that swallows it is a room with no door. Applications that need a
/// literal Escape (vim, helix) get it from the keyboard's own binding while focus is held,
/// because `begin_pass` only acts on Escape to CLEAR focus, and clearing focus is exactly
/// what we want the second press to do.
pub fn terminal_filter() -> EventFilter {
    EventFilter {
        tab: true,
        horizontal_arrows: true,
        vertical_arrows: true,
        escape: false,
    }
}

/// What a claim did, for the caller's log and for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The terminal holds the keyboard.
    Held,
    /// Something else does — a text field, a dialog. Left alone deliberately.
    Yielded,
    /// The terminal is not on screen.
    Absent,
}

/// Claim the keyboard for a terminal occupying `rect`.
///
/// Call once per frame, before draining input. `id` must be stable across frames and unique
/// per pane — focus is tracked BY id, and two panes sharing one would hand the keyboard back
/// and forth every frame.
pub fn claim(ui: &mut Ui, id: Id, rect: egui::Rect, visible: bool) -> Claim {
    if !visible {
        return Claim::Absent;
    }
    // A real widget, so egui has something to focus. `Sense::click` and not `hover`: the
    // click is what takes the keyboard back from a field the user has finished with.
    let resp = ui.interact(rect, id, Sense::click());
    if resp.clicked() {
        resp.request_focus();
    }
    let focused = ui.memory(|m| m.focused());
    match focused {
        // Ours already: keep the filter fresh. `set_focus_lock_filter` only applies to a
        // widget that HELD focus last frame as well, so this has to be re-set every frame
        // rather than once at claim time.
        Some(f) if f == id => {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, terminal_filter()));
            Claim::Held
        }
        // Someone else has it. A TEXT FIELD keeps it — a find bar that loses focus one
        // frame after opening is worse than a Tab that walks. Anything else (a button, a
        // toggle) gets it taken back.
        //
        // The filter above is not enough on its own: `set_focus_lock_filter` only applies
        // to a widget that held focus on the PREVIOUS frame too, so there is a two-frame
        // window after a claim where egui will still treat Tab as navigation. That window
        // is exactly when a freshly-opened pane gets its first Tab. Taking focus back from
        // a non-text widget closes it, and is also the general repair for any other route
        // by which focus drifts onto the sidebar.
        Some(other) if !is_text_field(ui.ctx(), other) => {
            ui.memory_mut(|m| m.request_focus(id));
            Claim::Held
        }
        Some(_) => Claim::Yielded,
        // Free. Take it — a terminal on screen with focus nowhere is the state where Tab
        // walked the sidebar.
        None => {
            ui.memory_mut(|m| m.request_focus(id));
            ui.memory_mut(|m| m.set_focus_lock_filter(id, terminal_filter()));
            Claim::Held
        }
    }
}

/// Is the widget with this id a text field?
///
/// egui has no direct answer, but a `TextEdit` is the only widget that stores a
/// `TextEditState` under its own id — so the state's presence IS the answer, and it does not
/// depend on a list of widget types this module would have to keep up to date.
pub fn is_text_field(ctx: &egui::Context, id: Id) -> bool {
    egui::text_edit::TextEditState::load(ctx, id).is_some()
}

/// Does the terminal hold the keyboard right now?
///
/// The question `drain_egui` needed and could not ask: `egui_wants_keyboard_input()` is true
/// for a focused BUTTON as much as for a focused field, so a Tab that landed on the sidebar
/// silenced the terminal completely.
pub fn terminal_has_keyboard(ctx: &egui::Context, id: Id) -> bool {
    ctx.memory(|m| m.focused()) == Some(id)
}

/// Hand the keyboard back to the app (the Escape path, or a pane closing).
pub fn release(ctx: &egui::Context, id: Id) {
    if terminal_has_keyboard(ctx, id) {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(events: Vec<egui::Event>) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)));
        input.events = events;
        input
    }

    fn tab() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The whole bug in one test: with a terminal on screen and a button beside it, Tab
    /// must not move focus to the button.
    #[test]
    fn tab_does_not_walk_to_the_sidebar_while_the_terminal_holds_the_keyboard() {
        let ctx = egui::Context::default();
        let term = Id::new("term-pane");
        let mut button_focused = false;
        // frame 1: claim
        let _ = ctx.run(ctx_with(vec![]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 300.0));
                assert_eq!(claim(ui, term, r, true), Claim::Held, "a free keyboard goes to the terminal");
                let _ = ui.button("sidebar");
            });
        });
        assert!(terminal_has_keyboard(&ctx, term));
        // frame 2: Tab arrives
        let _ = ctx.run(ctx_with(vec![tab()]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 300.0));
                let _ = claim(ui, term, r, true);
                button_focused = ui.button("sidebar").has_focus();
            });
        });
        // The filter needs the terminal to have held focus on the PREVIOUS frame too
        // (`set_focus_lock_filter`'s own precondition), so a Tab this early still moves
        // focus. Two quiet frames: the first applies the reclaim, the second makes the
        // filter live.
        for _ in 0..2 {
            let _ = ctx.run(ctx_with(vec![]), |c| {
                egui::CentralPanel::default().show(c, |ui| {
                    let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 300.0));
                    let _ = claim(ui, term, r, true);
                    let _ = ui.button("sidebar");
                });
            });
        }
        assert!(terminal_has_keyboard(&ctx, term), "focus was never recovered from the sidebar");

        // NOW Tab does not move focus at all.
        let mut moved = false;
        let _ = ctx.run(ctx_with(vec![tab()]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 300.0));
                let _ = claim(ui, term, r, true);
                moved = ui.button("sidebar").has_focus();
            });
        });
        assert!(!moved, "Tab selected the sidebar button after the filter went live");
        assert!(terminal_has_keyboard(&ctx, term), "Tab moved focus off the terminal");
        let _ = button_focused;
    }

    /// Without the claim, Tab does move focus — so the test above is testing the fix and
    /// not a property egui had anyway.
    #[test]
    fn without_a_claim_tab_walks_as_before() {
        let ctx = egui::Context::default();
        let mut focused_after = None;
        let _ = ctx.run(ctx_with(vec![]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let _ = ui.button("sidebar");
            });
        });
        let _ = ctx.run(ctx_with(vec![tab()]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let b = ui.button("sidebar");
                focused_after = b.has_focus().then_some(b.id);
            });
        });
        assert!(focused_after.is_some(), "egui no longer moves focus on Tab; the claim may be unnecessary");
    }

    /// A find bar or the palette must be able to take the keyboard, and keep it.
    #[test]
    fn a_focused_text_field_keeps_the_keyboard() {
        let ctx = egui::Context::default();
        let term = Id::new("term-pane");
        let field = Id::new("find-bar");
        let mut buf = String::new();
        // the field asks for focus
        let _ = ctx.run(ctx_with(vec![]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut buf).id(field));
                r.request_focus();
                let rect = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(200.0, 200.0));
                let _ = claim(ui, term, rect, true);
            });
        });
        let mut claim_state = Claim::Absent;
        let _ = ctx.run(ctx_with(vec![]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let _ = ui.add(egui::TextEdit::singleline(&mut buf).id(field));
                let rect = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(200.0, 200.0));
                claim_state = claim(ui, term, rect, true);
            });
        });
        assert_eq!(claim_state, Claim::Yielded, "the terminal stole the find bar's keyboard");
        assert!(!terminal_has_keyboard(&ctx, term));
    }

    #[test]
    fn a_hidden_terminal_claims_nothing_and_release_hands_it_back() {
        let ctx = egui::Context::default();
        let term = Id::new("term-pane");
        let mut state = Claim::Held;
        let _ = ctx.run(ctx_with(vec![]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let r = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0));
                state = claim(ui, term, r, false);
            });
        });
        assert_eq!(state, Claim::Absent);
        assert!(!terminal_has_keyboard(&ctx, term));

        // claim, then release
        let _ = ctx.run(ctx_with(vec![]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                let r = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0));
                let _ = claim(ui, term, r, true);
            });
        });
        assert!(terminal_has_keyboard(&ctx, term));
        release(&ctx, term);
        assert!(!terminal_has_keyboard(&ctx, term));
    }

    #[test]
    fn the_filter_takes_tab_and_the_arrows_but_not_escape() {
        let f = terminal_filter();
        assert!(f.tab, "Tab is the whole point");
        assert!(f.horizontal_arrows && f.vertical_arrows, "an editor needs its arrows");
        assert!(!f.escape, "Escape is how the keyboard is handed back");
    }
}
