//! dispatch — which touch handler a drag should use, decided once.
//!
//! WHY THIS IS ITS OWN MODULE
//!
//! This rule decides, at `drag_begin`, whether a gesture becomes swipe-typing,
//! a cardinal dispatch, a plain tap, or nothing at all. It is the branch that
//! determines whether every swipe on the keyboard works.
//!
//! It used to live inline in a closure in `keyboard_view.rs`, and a
//! *reimplementation* of it lived in `keyboard_behaviors_tests.rs` as a private
//! `fn select_behavior`. Six tests exercised the reimplementation. Nothing
//! exercised the real thing, and the two had drifted in three ways — two of
//! which the tests asserted BACKWARDS:
//!
//! | case                                        | live code    | the test copy       |
//! |---------------------------------------------|--------------|---------------------|
//! | on-key Normal + Character + swipe-typing on | SwipeSource  | Normal   (asserted) |
//! | on-key SwipeSource under suppress_freeform  | Normal       | not modelled        |
//! | off-key under suppress_freeform             | Some(Cardinal)| None    (asserted) |
//!
//! The first row is the entire point of swipe-typing on a QWERTY layout — drag
//! through letters, get a word — and the test suite asserted it does not
//! happen. A green suite meant nothing here, and the GTK crate cannot be
//! compiled in the authoring sandbox, so reading was the only check available.
//!
//! `board-layouts` is the right home because `ButtonBehavior` and
//! `DispatchMode` are already here, it has no GTK dependency, and it therefore
//! COMPILES AND TESTS in that sandbox. Moving the rule here is what makes it
//! checkable at all.

use crate::key::ButtonBehavior;
use crate::DispatchMode;

/// What the hit-test found under the finger at `drag_begin`.
///
/// Deliberately not the key itself: the rule needs exactly two facts about it,
/// and taking the whole key would drag `KeyAction` and the layout types into
/// every test, which is how the reimplementation came to exist in the first
/// place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// The key's own declared behaviour, from `Key::behavior()`.
    pub raw: ButtonBehavior,
    /// Whether the key emits a character (`KeyAction::Character`). Only
    /// character keys get promoted into swipe sources; promoting Backspace
    /// would start a word on a drag from it.
    pub is_character: bool,
}

/// Decide the touch handler for a drag starting at this point.
///
/// `None` means the gesture is dropped entirely — no handler runs.
pub fn select_behavior(
    hit: Option<Hit>,
    mode: DispatchMode,
    swipe_typing_on: bool,
) -> Option<ButtonBehavior> {
    let suppress_freeform = mode.suppresses_freeform_swipe();

    if let Some(Hit { raw, is_character }) = hit {
        if suppress_freeform {
            // Cardinal and Stenotype must not run the freeform recognizer.
            // Space reports SwipeSource unconditionally (right for the QWERTY
            // springboard), so without this downgrade a swipe from the
            // cardinal/steno space bar fires swipe-typing and looks like the
            // standard keyboard took over. The key's OWN Cardinal behaviour is
            // preserved, so steno buttons and grid cells still route through
            // `dispatch_cardinal_swipe`.
            return Some(if raw == ButtonBehavior::SwipeSource {
                ButtonBehavior::Normal
            } else {
                raw
            });
        }
        if raw == ButtonBehavior::Normal && is_character && swipe_typing_on {
            // Standard mode with swipe-typing on: letter keys become swipe
            // sources so dragging through them composes a word. This is the
            // promotion the old test copy asserted did NOT happen.
            return Some(ButtonBehavior::SwipeSource);
        }
        return Some(raw);
    }

    // Nothing under the finger.
    if suppress_freeform {
        // A cardinal/steno grid is dense: a miss is almost always rounding at
        // a cell edge, not a deliberate press on background. Dispatching as
        // Cardinal lets the gesture resolve against the nearest cell rather
        // than vanishing.
        return Some(ButtonBehavior::Cardinal);
    }
    if swipe_typing_on {
        // Regression fix vs the pre-rewrite code at 151b01d^: the old code
        // never required a hit-test on the source key in non-cardinal
        // layouts, and the rewrite started requiring `Some(behavior)` — which
        // silently dropped any gesture beginning in the margin band or on a
        // gap between keys. Falling back to SwipeSource keeps the path
        // running through the recognizer at `drag_end`.
        return Some(ButtonBehavior::SwipeSource);
    }
    // Standard mode, swipe-typing off, nothing hit: there is genuinely
    // nothing to do. A tap on background is not a gesture.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(raw: ButtonBehavior, is_character: bool) -> Option<Hit> {
        Some(Hit { raw, is_character })
    }

    // ---- the three rows the old reimplementation got wrong -----------------

    #[test]
    fn a_letter_key_becomes_a_swipe_source_when_swipe_typing_is_on() {
        // THE headline behaviour of the keyboard: drag across letters, get a
        // word. The reimplementation in keyboard_behaviors_tests.rs asserted
        // this returns Normal, and passed, for as long as it existed.
        assert_eq!(
            select_behavior(hit(ButtonBehavior::Normal, true), DispatchMode::Standard, true),
            Some(ButtonBehavior::SwipeSource)
        );
    }

    #[test]
    fn a_non_character_key_is_never_promoted() {
        // Backspace, Shift, Enter are Normal but not characters. Promoting
        // them would start composing a word on a drag from Backspace.
        assert_eq!(
            select_behavior(hit(ButtonBehavior::Normal, false), DispatchMode::Standard, true),
            Some(ButtonBehavior::Normal)
        );
    }

    #[test]
    fn a_letter_key_stays_normal_when_swipe_typing_is_off() {
        // Drag-off-letter must be a clean tap-or-cancel: no recognizer pass,
        // no surprise word.
        assert_eq!(
            select_behavior(hit(ButtonBehavior::Normal, true), DispatchMode::Standard, false),
            Some(ButtonBehavior::Normal)
        );
    }

    #[test]
    fn space_is_downgraded_in_the_modes_that_suppress_freeform() {
        // Space reports SwipeSource unconditionally. In cardinal/steno that
        // would fire swipe-typing and look like the standard keyboard took
        // over the layout.
        for mode in [DispatchMode::Cardinal, DispatchMode::Stenotype] {
            assert_eq!(
                select_behavior(hit(ButtonBehavior::SwipeSource, false), mode, true),
                Some(ButtonBehavior::Normal),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_cells_own_cardinal_behaviour_survives_suppression() {
        // The downgrade above must be surgical: it targets SwipeSource only.
        // Downgrading Cardinal too would break every steno button and grid
        // cell, which is the whole input method in those modes.
        for mode in [DispatchMode::Cardinal, DispatchMode::Stenotype] {
            assert_eq!(
                select_behavior(hit(ButtonBehavior::Cardinal, true), mode, true),
                Some(ButtonBehavior::Cardinal),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_miss_in_a_grid_mode_dispatches_cardinal_rather_than_nothing() {
        // The old test copy asserted None here.
        for mode in [DispatchMode::Cardinal, DispatchMode::Stenotype] {
            assert_eq!(
                select_behavior(None, mode, true),
                Some(ButtonBehavior::Cardinal),
                "{mode:?}"
            );
            // ...and it does not depend on the swipe-typing setting, which
            // has no meaning in a mode that suppresses freeform swipe.
            assert_eq!(
                select_behavior(None, mode, false),
                Some(ButtonBehavior::Cardinal),
                "{mode:?} with swipe typing off"
            );
        }
    }

    // ---- the rest of the contract -----------------------------------------

    #[test]
    fn a_miss_in_standard_mode_still_reaches_the_recognizer() {
        // The 151b01d^ regression: a swipe starting in the margin band or on
        // a gap between keys used to be dropped entirely.
        assert_eq!(
            select_behavior(None, DispatchMode::Standard, true),
            Some(ButtonBehavior::SwipeSource)
        );
    }

    #[test]
    fn a_miss_with_swipe_typing_off_is_not_a_gesture() {
        assert_eq!(select_behavior(None, DispatchMode::Standard, false), None);
    }

    #[test]
    fn altkeyswipe_is_passed_through_untouched_in_standard_mode() {
        // Not Normal, so no promotion; not SwipeSource, so no downgrade.
        assert_eq!(
            select_behavior(hit(ButtonBehavior::AltKeySwipe, true), DispatchMode::Standard, true),
            Some(ButtonBehavior::AltKeySwipe)
        );
    }

    #[test]
    fn standard_mode_never_invents_a_cardinal_dispatch() {
        // Cardinal only ever comes out of Standard mode if the KEY declared
        // it. Returning it from a miss would route a freeform swipe into
        // `dispatch_cardinal_swipe`, which has no cell to resolve against.
        for on in [true, false] {
            assert_ne!(
                select_behavior(None, DispatchMode::Standard, on),
                Some(ButtonBehavior::Cardinal)
            );
        }
    }

    #[test]
    fn every_mode_and_hit_combination_is_decided() {
        // Guards the shape rather than a value: if a new DispatchMode or
        // ButtonBehavior is added and this rule is not extended, at least one
        // of these should stop being reachable or start panicking. Cheap
        // insurance on a function whose failure mode is a dead gesture.
        let modes = [DispatchMode::Standard, DispatchMode::Cardinal, DispatchMode::Stenotype];
        let raws = [
            ButtonBehavior::Normal,
            ButtonBehavior::Cardinal,
            ButtonBehavior::AltKeySwipe,
            ButtonBehavior::SwipeSource,
        ];
        let mut decided = 0;
        for m in modes {
            for r in raws {
                for ch in [true, false] {
                    for on in [true, false] {
                        if select_behavior(hit(r, ch), m, on).is_some() {
                            decided += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(
            decided,
            modes.len() * raws.len() * 4,
            "an on-key drag must always produce a handler"
        );
    }
}
