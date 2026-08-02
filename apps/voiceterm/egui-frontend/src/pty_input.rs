//! pty_input — keys to PTY bytes. The half of `PtyTerm` you cannot type without.
//!
//! The egui port renders a terminal grid and has no way to put anything into
//! it. This is the encoder from the QML `termKeyArea` handler and `main.rs`'s
//! `pty_write_key`, made toolkit-neutral: Qt key codes in, an enum here, the
//! same bytes out.
//!
//! Almost every rule below exists because something broke. They are worth
//! keeping in that form, because "send the obvious byte" is wrong in at least
//! six specific places:
//!
//! * **Arrows depend on a mode the program sets.** In application-cursor mode
//!   (DECCKM, set by fzf, vim, less, and readline in some configs) arrows are
//!   SS3 — `ESC O A` — not CSI `ESC [ A`. voiceterm always sent CSI, which
//!   broke arrow navigation inside exactly those programs; the reported symptom
//!   was Ctrl+R fzf history search where you could not move through the match.
//!
//! * **Backspace is DEL (0x7f), not BS (0x08).** Sending 0x08 gives you a shell
//!   that beeps instead of deleting.
//!
//! * **Alt means "prefix an ESC".** Alt+Tab is `ESC TAB` for zsh's
//!   autosuggestion accept, Alt+f/b/d are the word motions. The Alt bit was
//!   never decoded, so every Alt combination fell through to the plain-key
//!   branch and arrived as the unmodified key.
//!
//! * **Bare Tab and modified Tab are different keys.** The old code ate ALL
//!   Tabs as `\t` and returned before the modifier-aware path ever ran.
//!
//! * **Some chords must be stolen before the PTY sees them.** The terminal has
//!   keyboard focus, so Ctrl+Shift+T reaches it first and gets encoded as ^T
//!   and inserted into the shell — the dogfood bug where opening a new tab
//!   typed a control character instead. Tab management is intercepted here,
//!   ahead of encoding.
//!
//! * **`\n` is not the Enter byte.** A physical Enter sends `\r`. Command
//!   strings assembled upstream — by voice, by the palette, by history replay —
//!   arrive with `\n` or `\r\n` and must be canonicalised, or the shell sees a
//!   line feed and stacks the cursor instead of running anything.

pub use uikit_term::keys::{Key, Mods, TermMode, canonical, ctrl_byte_of as ctrl_byte, encode_mode, paste};

/// Encode for the current terminal mode. A thin alias so this module reads as
/// the input path it is, while the table itself lives in the shared kit — one
/// encoder, not two that can drift apart the first time only one is fixed.
pub fn encode(key: Key, m: Mods, mode: TermMode) -> Vec<u8> {
    encode_mode(key, m, mode).unwrap_or_default()
}

/// A chord the UI takes for itself rather than forwarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    NewTab,
    NextTab,
    PrevTab,
    FocusRight,
    FocusLeft,
}

/// Chords intercepted BEFORE encoding.
///
/// This ordering is the fix for the dogfood bug: the hidden input has focus, so
/// a global shortcut never sees Ctrl+Shift+T — the terminal encodes it as ^T
/// and types it into the shell. Anything the UI wants must be taken here.
///
/// `split_visible` gates the pane-focus chords: with no split on screen,
/// Ctrl+Shift+Right should move a tab, not focus a pane that is not there.
pub fn chord(key: Key, m: Mods, split_visible: bool) -> Option<Chord> {
    if m.ctrl && m.shift {
        return match key {
            Key::Char('t') | Key::Char('T') => Some(Chord::NewTab),
            Key::Char(']') => Some(Chord::NextTab),
            Key::Char('[') => Some(Chord::PrevTab),
            Key::Right if split_visible => Some(Chord::FocusRight),
            Key::Left if split_visible => Some(Chord::FocusLeft),
            _ => None,
        };
    }
    // Shift+arrow cycles tabs. Only the horizontal pair: Shift+Up/Down is a
    // selection gesture in most programs and stealing it would break them.
    if m.shift && !m.ctrl && !m.alt {
        return match key {
            Key::Right => Some(Chord::NextTab),
            Key::Left => Some(Chord::PrevTab),
            _ => None,
        };
    }
    None
}

/// Should this input snap the view back to the live prompt?
///
/// Typing does; navigation does not. Someone scrolled up reading earlier output
/// who presses PageUp expects to keep going, but someone who types a character
/// wants to see what they are typing.
pub fn snaps_to_bottom(key: Key, m: Mods) -> bool {
    match key {
        Key::Char(_) => true,
        Key::Enter | Key::Tab | Key::Backspace | Key::Delete => true,
        Key::Up | Key::Down | Key::Left | Key::Right
        | Key::PageUp | Key::PageDown | Key::Home | Key::End => false,
        Key::Escape => m.ctrl,
        Key::Insert | Key::F(_) => false,
    }
}

/// Does this keystroke count as "typing" for the touch-keys bar, which collapses
/// out of the way when you start?
///
/// Deliberately narrow: the volume buttons must not collapse it, which was
/// asked for specifically. Printable text, or one of Backspace/Enter/Tab/Space.
pub fn is_typing(key: Key) -> bool {
    match key {
        Key::Char(c) => (c as u32) >= 0x20,
        Key::Backspace | Key::Enter | Key::Tab => true,
        _ => false,
    }
}

/// What one keypress produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Bytes for the PTY. Empty when the chord was taken or nothing maps.
    pub bytes: Vec<u8>,
    /// A UI action instead of terminal input.
    pub chord: Option<Chord>,
    pub snap_to_bottom: bool,
    pub collapses_touch_bar: bool,
}

/// The whole path for one keypress, in the order the QML handler runs it:
/// chords first, then encoding.
pub fn press(key: Key, m: Mods, mode: TermMode, split_visible: bool) -> Outcome {
    if let Some(c) = chord(key, m, split_visible) {
        return Outcome { bytes: Vec::new(), chord: Some(c),
                         snap_to_bottom: false, collapses_touch_bar: false };
    }
    let bytes = encode(key, m, mode);
    Outcome {
        snap_to_bottom: !bytes.is_empty() && snaps_to_bottom(key, m),
        collapses_touch_bar: is_typing(key),
        chord: None,
        bytes,
    }
}

/// Translate egui's key events into this module's vocabulary and drain them.
///
/// Kept separate from `press` so the encoding is testable without an egui
/// context, which is the only reason any of the above has tests at all.
pub fn drain_egui(ui: &egui::Ui, mode: TermMode, split_visible: bool) -> Vec<Outcome> {
    let mut out = Vec::new();
    ui.input(|i| {
        for ev in &i.events {
            match ev {
                egui::Event::Text(t) => {
                    for ch in t.chars() {
                        out.push(press(Key::Char(ch), Mods::none(), mode, split_visible));
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    let m = Mods { ctrl: modifiers.ctrl || modifiers.command,
                                   alt: modifiers.alt, shift: modifiers.shift };
                    if let Some(k) = from_egui_key(*key, m) {
                        // Text events already carry unmodified printable
                        // characters; taking them here too would double every
                        // letter you type.
                        if matches!(k, Key::Char(_)) && !m.ctrl && !m.alt { continue; }
                        out.push(press(k, m, mode, split_visible));
                    }
                }
                _ => {}
            }
        }
    });
    out
}

fn from_egui_key(k: egui::Key, _m: Mods) -> Option<Key> {
    use egui::Key as E;
    Some(match k {
        E::Backspace => Key::Backspace,
        E::Enter => Key::Enter,
        E::Tab => Key::Tab,
        E::Delete => Key::Delete,
        E::Insert => Key::Insert,
        E::Escape => Key::Escape,
        E::Home => Key::Home,
        E::End => Key::End,
        E::ArrowLeft => Key::Left,
        E::ArrowRight => Key::Right,
        E::ArrowUp => Key::Up,
        E::ArrowDown => Key::Down,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::Space => Key::Char(' '),
        other => {
            let n = other.name();
            let mut cs = n.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: TermMode = TermMode { app_cursor: false, bracketed_paste: false };
    const APP: TermMode = TermMode { app_cursor: true, bracketed_paste: false };
    const BRACKET: TermMode = TermMode { app_cursor: false, bracketed_paste: true };

    #[test]
    fn arrows_change_shape_in_application_cursor_mode() {
        // THE bug: always sending CSI broke arrow navigation inside fzf, vim
        // and less — reported as Ctrl+R history search where you could not
        // move through the matched text.
        assert_eq!(encode(Key::Up, Mods::none(), PLAIN), b"\x1b[A");
        assert_eq!(encode(Key::Up, Mods::none(), APP), b"\x1bOA");
        assert_eq!(encode(Key::Down, Mods::none(), APP), b"\x1bOB");
        assert_eq!(encode(Key::Right, Mods::none(), APP), b"\x1bOC");
        assert_eq!(encode(Key::Left, Mods::none(), APP), b"\x1bOD");
    }

    #[test]
    fn the_arrow_letters_stay_put_across_both_modes() {
        // A → up in both; only the introducer changes. Swapping a letter here
        // sends the cursor the wrong way in exactly one of the two modes,
        // which is the kind of bug that survives a lot of testing.
        for (k, ltr) in [(Key::Up, b'A'), (Key::Down, b'B'), (Key::Right, b'C'), (Key::Left, b'D')] {
            assert_eq!(*encode(k, Mods::none(), PLAIN).last().unwrap(), ltr);
            assert_eq!(*encode(k, Mods::none(), APP).last().unwrap(), ltr);
        }
    }

    #[test]
    fn backspace_is_del_not_backspace() {
        // 0x08 gives you a shell that beeps instead of deleting.
        assert_eq!(encode(Key::Backspace, Mods::none(), PLAIN), vec![0x7f]);
    }

    #[test]
    fn enter_sends_carriage_return() {
        assert_eq!(encode(Key::Enter, Mods::none(), PLAIN), b"\r");
    }

    #[test]
    fn the_editing_keys_send_their_csi_sequences() {
        assert_eq!(encode(Key::Delete, Mods::none(), PLAIN), b"\x1b[3~");
        assert_eq!(encode(Key::Insert, Mods::none(), PLAIN), b"\x1b[2~");
        assert_eq!(encode(Key::Home, Mods::none(), PLAIN), b"\x1b[H");
        assert_eq!(encode(Key::End, Mods::none(), PLAIN), b"\x1b[F");
        assert_eq!(encode(Key::PageUp, Mods::none(), PLAIN), b"\x1b[5~");
        assert_eq!(encode(Key::PageDown, Mods::none(), PLAIN), b"\x1b[6~");
        assert_eq!(encode(Key::Escape, Mods::none(), PLAIN), vec![0x1b]);
    }

    #[test]
    fn bare_tab_and_alt_tab_are_different_keys() {
        // The old branch ate ALL Tabs as \t and returned before the
        // modifier-aware path ran, so zsh never saw the autosuggestion accept.
        assert_eq!(encode(Key::Tab, Mods::none(), PLAIN), vec![0x09]);
        assert_eq!(encode(Key::Tab, Mods::alt(), PLAIN), b"\x1b\t");
        assert_eq!(encode(Key::Tab, Mods::shift(), PLAIN), b"\x1b[Z", "Shift+Tab is CSI Z, not a tab with a bit set");
    }

    #[test]
    fn alt_prefixes_an_escape_because_meta_sends_escape() {
        // zsh word motions: Alt+f forward-word, Alt+b backward-word.
        assert_eq!(encode(Key::Char('f'), Mods::alt(), PLAIN), b"\x1bf");
        assert_eq!(encode(Key::Char('b'), Mods::alt(), PLAIN), b"\x1bb");
    }

    #[test]
    fn ctrl_wins_over_alt_rather_than_both_applying() {
        // Qt's guard is `alt && !ctrl`. Ctrl+Alt+C sends the control byte, not
        // an ESC-prefixed one.
        let m = Mods { ctrl: true, alt: true, shift: false };
        assert_eq!(encode(Key::Char('c'), m, PLAIN), vec![0x03]);
    }

    #[test]
    fn ctrl_letters_become_control_bytes_whatever_their_case() {
        assert_eq!(encode(Key::Char('c'), Mods::ctrl(), PLAIN), vec![0x03]);
        assert_eq!(encode(Key::Char('C'), Mods::ctrl(), PLAIN), vec![0x03],
                   "Ctrl+Shift+C still means 0x03 where the platform delivers it");
        assert_eq!(encode(Key::Char('a'), Mods::ctrl(), PLAIN), vec![0x01]);
        assert_eq!(encode(Key::Char('z'), Mods::ctrl(), PLAIN), vec![0x1a]);
        assert_eq!(encode(Key::Char('d'), Mods::ctrl(), PLAIN), vec![0x04]);
    }

    #[test]
    fn a_platform_that_hands_us_the_control_byte_already_is_not_double_encoded() {
        // Some Linux/Wayland setups deliver the control character as the text.
        assert_eq!(ctrl_byte('\u{3}'), Some(0x03));
    }

    #[test]
    fn ctrl_with_a_key_that_has_no_control_byte_sends_nothing() {
        // Ctrl+5 typing a bare "5" into the shell is worse than it doing
        // nothing at all.
        assert!(encode(Key::Char('5'), Mods::ctrl(), PLAIN).is_empty());
    }

    #[test]
    fn the_control_punctuation_is_carried_over() {
        assert_eq!(ctrl_byte('['), Some(0x1b), "Ctrl+[ is ESC");
        assert_eq!(ctrl_byte(' '), Some(0x00), "Ctrl+Space is NUL, the set-mark binding");
        assert_eq!(ctrl_byte('\\'), Some(0x1c));
    }

    #[test]
    fn a_plain_character_is_sent_as_utf8() {
        assert_eq!(encode(Key::Char('a'), Mods::none(), PLAIN), b"a");
        assert_eq!(encode(Key::Char('é'), Mods::none(), PLAIN), "é".as_bytes());
        assert_eq!(encode(Key::Char('🙂'), Mods::none(), PLAIN), "🙂".as_bytes());
    }

    #[test]
    fn function_keys_send_their_sequence_but_do_not_count_as_typing() {
        // The shared kit encodes F-keys properly (SS3 for F1-F4), which the
        // module this replaced did not. They still must not collapse the
        // touch-keys bar — and neither must the volume buttons, which was
        // asked for specifically and is why `is_typing` is a separate rule
        // from "did this produce bytes".
        assert_eq!(encode(Key::F(1), Mods::none(), PLAIN), b"\x1bOP");
        assert!(!is_typing(Key::F(1)));
        assert!(!press(Key::F(1), Mods::none(), PLAIN, false).collapses_touch_bar);
    }

    #[test]
    fn tab_chords_are_taken_before_the_terminal_can_encode_them() {
        // The dogfood bug: the hidden input has focus, so a global shortcut
        // never sees Ctrl+Shift+T — the terminal encoded it as ^T and typed it
        // into the shell.
        let out = press(Key::Char('t'), Mods { ctrl: true, shift: true, alt: false }, PLAIN, false);
        assert_eq!(out.chord, Some(Chord::NewTab));
        assert!(out.bytes.is_empty(), "and nothing reaches the shell");

        // Without the interception it would have been ^T.
        assert_eq!(encode(Key::Char('t'), Mods::ctrl(), PLAIN), vec![0x14]);
    }

    #[test]
    fn the_bracket_chords_cycle_tabs() {
        assert_eq!(chord(Key::Char(']'), Mods { ctrl: true, shift: true, alt: false }, false), Some(Chord::NextTab));
        assert_eq!(chord(Key::Char('['), Mods { ctrl: true, shift: true, alt: false }, false), Some(Chord::PrevTab));
    }

    #[test]
    fn pane_focus_chords_need_a_visible_split() {
        // With no split on screen, Ctrl+Shift+Right should not focus a pane
        // that is not there.
        assert_eq!(chord(Key::Right, Mods { ctrl: true, shift: true, alt: false }, true), Some(Chord::FocusRight));
        assert_eq!(chord(Key::Right, Mods { ctrl: true, shift: true, alt: false }, false), None);
    }

    #[test]
    fn shift_arrows_cycle_tabs_but_only_horizontally() {
        // Shift+Up/Down is a selection gesture in most programs; stealing it
        // would break them.
        assert_eq!(chord(Key::Right, Mods::shift(), false), Some(Chord::NextTab));
        assert_eq!(chord(Key::Left, Mods::shift(), false), Some(Chord::PrevTab));
        assert_eq!(chord(Key::Up, Mods::shift(), false), None);
        assert_eq!(chord(Key::Down, Mods::shift(), false), None);
    }

    #[test]
    fn an_ordinary_key_is_never_a_chord() {
        assert_eq!(chord(Key::Char('t'), Mods::none(), true), None);
        assert_eq!(chord(Key::Char('t'), Mods::ctrl(), true), None, "Ctrl+T alone is ^T");
    }

    #[test]
    fn a_bare_newline_becomes_the_byte_a_physical_enter_sends() {
        // Voice dispatch and palette execution assemble strings elsewhere; a
        // stray \n makes the shell stack the cursor instead of running it.
        assert_eq!(canonical("ls\n"), "ls\r");
        assert_eq!(canonical("ls\r\n"), "ls\r", "and \\r\\n collapses to one");
        assert_eq!(canonical("ls\r"), "ls\r", "an existing \\r is left alone");
        assert_eq!(canonical("a\nb\nc"), "a\rb\rc");
        assert_eq!(canonical("plain"), "plain");
    }

    #[test]
    fn a_lone_cr_followed_later_by_lf_is_not_swallowed() {
        // prev_cr must reset on any other character, or "a\rb\n" loses the \n.
        assert_eq!(canonical("a\rb\n"), "a\rb\r");
    }

    #[test]
    fn a_paste_is_wrapped_only_when_the_program_asked_for_it() {
        // The markers let an editor tell pasted text from typing and skip
        // auto-indent — the difference between pasting code and pasting a
        // staircase.
        assert_eq!(paste("hi", PLAIN), b"hi");
        assert_eq!(paste("hi", BRACKET), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn a_pasted_block_still_gets_its_line_endings_canonicalised() {
        assert_eq!(paste("a\r\nb", BRACKET), b"\x1b[200~a\rb\x1b[201~");
    }

    #[test]
    fn typing_snaps_to_the_prompt_and_scrolling_does_not() {
        // Someone scrolled up reading output who presses PageUp expects to keep
        // going; someone who types wants to see what they typed.
        assert!(press(Key::Char('l'), Mods::none(), PLAIN, false).snap_to_bottom);
        assert!(press(Key::Enter, Mods::none(), PLAIN, false).snap_to_bottom);
        assert!(press(Key::Backspace, Mods::none(), PLAIN, false).snap_to_bottom);
        assert!(!press(Key::PageUp, Mods::none(), PLAIN, false).snap_to_bottom);
        assert!(!press(Key::Up, Mods::none(), PLAIN, false).snap_to_bottom);
        assert!(!press(Key::Home, Mods::none(), PLAIN, false).snap_to_bottom);
    }

    #[test]
    fn a_key_that_sends_nothing_does_not_snap_the_view() {
        // Otherwise a dead key yanks you away from what you were reading.
        assert!(!press(Key::Char('5'), Mods::ctrl(), PLAIN, false).snap_to_bottom);
        assert!(!press(Key::F(1), Mods::none(), PLAIN, false).snap_to_bottom,
                "an F-key sends bytes but is navigation, not typing");
    }

    #[test]
    fn the_touch_bar_collapses_on_typing_only() {
        assert!(is_typing(Key::Char('a')));
        assert!(is_typing(Key::Char(' ')));
        assert!(is_typing(Key::Backspace));
        assert!(is_typing(Key::Enter));
        assert!(is_typing(Key::Tab));
        for k in [Key::Up, Key::PageDown, Key::Home, Key::F(1), Key::Escape] {
            assert!(!is_typing(k), "{k:?} should not collapse the bar");
        }
    }

    #[test]
    fn a_chord_neither_snaps_the_view_nor_collapses_the_bar() {
        let out = press(Key::Char('t'), Mods { ctrl: true, shift: true, alt: false }, PLAIN, false);
        assert!(!out.snap_to_bottom);
        assert!(!out.collapses_touch_bar, "opening a tab is not typing");
    }
}
