//! chords — reaching the quick actions without the mouse.
//!
//! ## From the references
//!
//! Every quick action in the atlantis and mansion-garden panels shows its key beside its name —
//! ADD CONTEXT ⌘N, SUMMARIZE ⌘S, EXTRACT INSIGHTS ⌘I, FIND RELATED ⌘R. Ours had neither the keys
//! nor the display: the suggestion chips above the composer were mouse-only, and a person working
//! from the keyboard had to leave it to accept a suggestion the app had just offered them.
//!
//! ## Positional, because the actions are not fixed
//!
//! The reference's four actions are a worked example's fixed set. Ours are generated from the
//! conversation — "Show me the code" after a reply with a fence, "What's the fault in this log?"
//! when a log is attached — so binding ⌘S to "Summarize" would bind it to nothing whenever the
//! model's last turn contained code.
//!
//! So the chord refers to the POSITION, not the content: ⌘1 takes the first chip, whatever it says.
//! That is honest about what is guaranteed, and it is the pattern every tab bar and dock already
//! uses for a list that changes.
//!
//! ## The rules are testable, which is the point of a module for four lines of matching
//!
//! A shortcut table goes wrong in ways that are invisible until someone hits one: two actions on the
//! same chord, a chord that fires while typing, a chord the OS or egui already owns. All three are
//! properties of the table rather than of the rendering, so they are tests here rather than a bug
//! report later.

use egui::{Key, Modifiers};

/// How many suggestion chips can be reached by key. Nine because ⌘0 is conventionally "reset zoom"
/// and taking it would be the third kind of collision below.
pub const MAX_INDEXED: usize = 9;

/// The digit keys, in order. `Key::Num1` is not indexable, so the table is explicit.
const DIGITS: [Key; MAX_INDEXED] = [
    Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5,
    Key::Num6, Key::Num7, Key::Num8, Key::Num9,
];

/// Chords the platform or egui already owns, which this module must never claim.
///
/// Not exhaustive — an exhaustive list of every OS binding is not knowable from here — but these
/// are the ones a text-editing surface will actually be asked for, and taking any of them makes the
/// app feel broken rather than featureful.
pub const RESERVED: [Key; 10] = [
    Key::C, Key::V, Key::X, Key::Z, Key::Y, Key::A, // clipboard, undo/redo, select-all
    Key::W, Key::Q,                                  // close, quit
    Key::F, Key::Num0,                               // find, reset zoom
];

/// The key for the nth chip, or None past the indexable range.
pub fn digit_for(index: usize) -> Option<Key> { DIGITS.get(index).copied() }

/// What to print beside the chip. Platform-correct, because a Linux build that says ⌘ is telling
/// the user to press a key their keyboard does not have.
pub fn chord_text(index: usize) -> Option<String> {
    if index >= MAX_INDEXED { return None; }
    let prefix = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
    Some(format!("{prefix}{}", index + 1))
}

/// Does this modifier state and key select chip `index`?
///
/// COMMAND rather than a bare digit: an unmodified `1` must reach the composer, and a shortcut that
/// swallows a character while someone is typing is the worst of the three failures — it is silent,
/// it corrupts their text, and it looks like a dropped keystroke.
pub fn fires(index: usize, mods: Modifiers, key: Key) -> bool {
    match digit_for(index) {
        Some(k) => k == key && mods.command && !mods.shift && !mods.alt,
        None => false,
    }
}

/// The chip index a chord selects, if any. The form a UI actually wants.
pub fn selected(mods: Modifiers, key: Key) -> Option<usize> {
    (0..MAX_INDEXED).find(|&i| fires(i, mods, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> Modifiers { Modifiers { command: true, ..Default::default() } }

    #[test]
    fn each_position_has_its_own_key_and_they_are_all_distinct() {
        let mut seen = Vec::new();
        for i in 0..MAX_INDEXED {
            let k = digit_for(i).expect("indexable");
            assert!(!seen.contains(&k), "position {i} reuses {k:?}");
            seen.push(k);
        }
        assert_eq!(digit_for(MAX_INDEXED), None, "past the range is None, not a wrap-around");
    }

    /// The worst of the three failures: silent, it corrupts text, and it reads as a dropped
    /// keystroke rather than as a feature misfiring.
    #[test]
    fn a_bare_digit_reaches_the_composer() {
        for i in 0..MAX_INDEXED {
            let k = digit_for(i).unwrap();
            assert!(!fires(i, Modifiers::default(), k), "position {i} fires while typing");
            assert!(!fires(i, Modifiers { shift: true, ..Default::default() }, k));
        }
        assert_eq!(selected(Modifiers::default(), Key::Num1), None);
    }

    #[test]
    fn the_modified_chord_selects_the_right_chip() {
        assert_eq!(selected(cmd(), Key::Num1), Some(0));
        assert_eq!(selected(cmd(), Key::Num3), Some(2));
        assert_eq!(selected(cmd(), Key::Num9), Some(8));
        assert_eq!(selected(cmd(), Key::Num0), None, "zero is reserved for reset zoom");
        assert_eq!(selected(cmd(), Key::S), None, "letters are not ours");
    }

    /// A chord with extra modifiers is a DIFFERENT chord and must not fall through to this one —
    /// ⌘⇧1 belongs to whoever binds it, not to chip one.
    #[test]
    fn extra_modifiers_make_a_different_chord() {
        let shifted = Modifiers { command: true, shift: true, ..Default::default() };
        let alted = Modifiers { command: true, alt: true, ..Default::default() };
        assert_eq!(selected(shifted, Key::Num1), None);
        assert_eq!(selected(alted, Key::Num1), None);
    }

    /// Taking a binding the platform already owns makes the app feel broken rather than featureful.
    #[test]
    fn nothing_here_claims_a_reserved_chord() {
        for i in 0..MAX_INDEXED {
            let k = digit_for(i).unwrap();
            assert!(!RESERVED.contains(&k), "position {i} claims the reserved {k:?}");
        }
    }

    /// The label must name a key the user's keyboard actually has.
    #[test]
    fn the_printed_chord_matches_the_key_and_the_platform() {
        for i in 0..MAX_INDEXED {
            let t = chord_text(i).expect("indexable");
            assert!(t.ends_with(&(i + 1).to_string()), "{t:?} does not name position {i}");
            if cfg!(target_os = "macos") { assert!(t.starts_with('⌘'), "{t:?}"); }
            else { assert!(t.starts_with("Ctrl+"), "{t:?} tells a Linux user to press a key they do not have"); }
        }
        assert_eq!(chord_text(MAX_INDEXED), None);
    }

    /// The chords must not collide with the freecam movement keys, which are live whenever the
    /// canvas is hovered. Digits are not in any layout's cluster, and this asserts it rather than
    /// assuming a future layout will keep it true.
    #[test]
    fn no_movement_key_is_a_chord_key() {
        for l in crate::navkeys::NavLayout::all() {
            let b = crate::navkeys::bindings(l);
            for k in [b.forward, b.back, b.left, b.right, b.up, b.down] {
                assert!(!DIGITS.contains(&k), "{l:?} binds {k:?}, which is also a chip chord");
            }
        }
    }
}
