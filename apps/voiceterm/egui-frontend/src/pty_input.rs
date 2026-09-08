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
    // Chords the WIDGET owns (Ctrl+F find, Ctrl+P palette). It handles
    // them from the same events, and encoding them here too sent the
    // control byte to the shell as well: pressing Ctrl+F opened the find
    // bar AND printed `^F` at the prompt. Each reader was correct on its
    // own, which is why nothing caught it until a screenshot.
    if uikit_term::keys::is_widget_chord(key, m) {
        return Outcome {
            bytes: Vec::new(),
            chord: None,
            snap_to_bottom: false,
            collapses_touch_bar: false,
        };
    }
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
    drain_egui_owned(ui, mode, split_visible, None)
}

/// [`drain_egui`], told which widget id the TERMINAL itself holds.
///
/// `egui_wants_keyboard_input()` is true whenever anything has focus — a find bar, the
/// palette, a sidebar button, and now the terminal's own focus sink. That sink exists so
/// egui stops treating Tab as focus navigation (see `kbd_focus`), and adding it silently
/// turned the guard below into "the terminal never receives a key", because the terminal
/// was the thing holding focus. Typing stopped completely.
///
/// So the question the guard has to ask is not "does anything want the keyboard" but "does
/// something OTHER than the terminal want it". `owner` is the terminal's id; when it is the
/// focused widget, the keys are ours.
pub fn drain_egui_owned(
    ui: &egui::Ui,
    mode: TermMode,
    split_visible: bool,
    owner: Option<egui::Id>,
) -> Vec<Outcome> {
    drain_egui_owned_sel(ui, mode, split_visible, owner, false)
}

/// [`drain_egui_owned`] told whether the terminal has a SELECTION.
///
/// It decides who gets Ctrl+C: with text selected that keystroke is a copy, and with nothing
/// selected it is the interrupt. Threaded as an argument rather than read from a global
/// because this module is pure over its inputs — that is what makes the encoder testable, and
/// a selection is exactly the sort of state that would otherwise leak in through the back.
pub fn drain_egui_owned_sel(
    ui: &egui::Ui,
    mode: TermMode,
    split_visible: bool,
    owner: Option<egui::Id>,
    has_selection: bool,
) -> Vec<Outcome> {
    // A FOCUSED TEXT WIDGET OWNS THE KEYBOARD.
    //
    // This reads raw context events, so it sees keys that were typed
    // into a TextEdit — egui does not remove an event because a widget
    // consumed it. The call site gates on voiceterm's own surfaces, but
    // that does not cover egui widgets: uikit-term's find bar, the
    // command palette, any inline field. Typing a search query also
    // typed it into the shell.
    //
    // `uikit_term::App::handle_input` has always had this guard. The
    // asymmetry was the bug — two readers of one event stream, one of
    // them checking focus.
    let focused = ui.memory(|m| m.focused());
    let ours = matches!((owner, focused), (Some(o), Some(f)) if o == f);
    if !ours && ui.ctx().egui_wants_keyboard_input() {
        return Vec::new();
    }
    let mut out = Vec::new();
    ui.input(|i| {
        for ev in &i.events {
            match ev {
                egui::Event::Text(t) => {
                    for ch in t.chars() {
                        out.push(press(Key::Char(ch), Mods::none(), mode, split_visible));
                    }
                }
                // AN ON-SCREEN KEYBOARD DOES NOT SEND KEYS FOR LETTERS.
                //
                // Plasma's virtual keyboard and Maliit are Wayland text-input clients:
                // `zwp_text_input_v3` COMMITS a string for ordinary characters, and
                // synthesises real key events only for the editing keys — backspace, enter,
                // arrows. winit surfaces the commit as an IME event and egui passes it
                // through as `Ime::Commit`, which this match did not have an arm for.
                //
                // So every letter was dropped and every editing key worked, which is exactly
                // the reported symptom and reads like a broken keymap rather than a missing
                // event. A physical keyboard sends `Event::Text` and was never affected,
                // which is why this survived every test and every desktop run.
                // CTRL+C NEVER ARRIVES AS A KEY. egui-winit turns it into `Event::Copy` and
                // RETURNS — `is_copy_command` matches `modifiers.command && Key::C`, and the
                // `Event::Key` push below it never runs. So a terminal that only reads key
                // events cannot see Ctrl+C at all, and the reported "C-c does not cancel" is
                // not the encoder failing but the keystroke never reaching it. Same for
                // Ctrl+X, which is `Event::Cut`.
                //
                // A terminal has to send the interrupt: that is the one keystroke a shell
                // user reaches for when something is wrong, and "select some text first" is
                // not an answer when a runaway process is filling the screen.
                //
                // COPY STILL WORKS when there is a selection — the widget handles that before
                // this drain runs, and this only sees what was left over. With nothing
                // selected there is nothing to copy, so spending the event on the interrupt
                // costs nothing.
                egui::Event::Copy if !has_selection => {
                    out.push(press(Key::Char('c'), Mods { ctrl: true, alt: false, shift: false }, mode, split_visible));
                }
                egui::Event::Cut if !has_selection => {
                    out.push(press(Key::Char('x'), Mods { ctrl: true, alt: false, shift: false }, mode, split_visible));
                }
                egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                    for ch in t.chars() {
                        out.push(press(Key::Char(ch), Mods::none(), mode, split_visible));
                    }
                }
                // Preedit is the composing text an IME shows before it commits — a
                // half-typed CJK syllable, or a swipe keyboard's candidate. It is NOT input
                // yet, and sending it would type every intermediate guess and then the
                // final word on top of them.
                egui::Event::Ime(_) => {}
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
    fn a_widget_chord_sends_nothing_to_the_shell() {
        // Ctrl+F opens the find bar; if this path also encodes it, the
        // shell gets 0x06 and prints ^F at the prompt.
        let ctrl = Mods { ctrl: true, alt: false, shift: false };
        assert!(press(Key::Char('f'), ctrl, PLAIN, false).bytes.is_empty());
        assert!(press(Key::Char('p'), ctrl, PLAIN, false).bytes.is_empty());
        // and an ordinary ctrl chord is untouched
        assert_eq!(press(Key::Char('c'), ctrl, PLAIN, false).bytes, b"\x03");
        assert_eq!(press(Key::Char('f'), Mods::none(), PLAIN, false).bytes, b"f");
    }

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

#[cfg(test)]
mod headless_tests {
    //! The input path driven by a REAL egui context.
    //!
    //! Everything above tests the encoder with synthetic `Key` values.
    //! Six bugs in this path this session were not in the encoder — they
    //! were in who reads the events, what mode is passed, and whether
    //! anyone else read them too. None of that is reachable from a unit
    //! test on `encode`.
    //!
    //! egui needs no window to lay out a frame, so the whole reader can
    //! run here: build a `RawInput` with the events a keyboard would
    //! produce, run one frame, and look at the bytes.

    use super::*;

    const PLAIN: TermMode = TermMode { app_cursor: false, bracketed_paste: false };
    const APP: TermMode = TermMode { app_cursor: true, bracketed_paste: false };

    /// Run one frame with `events` and return what the drain produced.
    fn drive(events: Vec<egui::Event>, mode: TermMode) -> Vec<Outcome> {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.events = events;
        let got = std::cell::RefCell::new(Vec::new());
        ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                *got.borrow_mut() = drain_egui(ui, mode, false);
            });
        });
        got.into_inner()
    }

    /// THE REPORTED BUG: C-c does not cancel a running process.
    ///
    /// Not the encoder — `ctrl_byte_of('c')` has always produced 0x03. egui-winit converts
    /// Ctrl+C into `Event::Copy` and RETURNS, so the `Event::Key` push never runs and the
    /// keystroke never reaches this drain at all. A terminal that only reads key events
    /// cannot see it.
    ///
    /// Ctrl+C is the one keystroke a shell user reaches for when something is wrong, and
    /// "select some text first" is not an answer while a runaway process fills the screen.
    #[test]
    fn ctrl_c_sends_the_interrupt_when_nothing_is_selected() {
        let bytes: Vec<u8> = drive(vec![egui::Event::Copy], PLAIN)
            .iter()
            .flat_map(|o| o.bytes.clone())
            .collect();
        assert_eq!(bytes, vec![0x03], "Ctrl+C did not reach the shell as an interrupt");

        // Ctrl+X likewise: egui-winit turns it into `Event::Cut` by the same path.
        let bytes: Vec<u8> = drive(vec![egui::Event::Cut], PLAIN)
            .iter()
            .flat_map(|o| o.bytes.clone())
            .collect();
        assert_eq!(bytes, vec![0x18], "Ctrl+X did not reach the shell");
    }

    /// With a SELECTION, Ctrl+C is a copy and must not also interrupt — killing someone's
    /// build because they copied an error message is worse than the bug being fixed.
    #[test]
    fn ctrl_c_with_a_selection_is_left_to_the_copy() {
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let mut input = egui::RawInput::default();
        input.events = vec![egui::Event::Copy];
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                out = drain_egui_owned_sel(ui, PLAIN, false, None, /*has_selection=*/ true);
            });
        });
        let bytes: Vec<u8> = out.iter().flat_map(|o| o.bytes.clone()).collect();
        assert!(bytes.is_empty(), "a copy also interrupted the process: {bytes:?}");
    }

    /// THE REPORTED BUG: on Plasma's virtual keyboard and on Maliit, the editing keys worked
    /// and letters did nothing.
    ///
    /// Those are Wayland text-input clients: `zwp_text_input_v3` COMMITS a string for
    /// ordinary characters and synthesises real key events only for backspace, enter and the
    /// arrows. winit surfaces the commit as an IME event, egui passes it as `Ime::Commit`,
    /// and this drain had no arm for it — so every letter was dropped and every editing key
    /// worked, which reads like a broken keymap rather than a missing event.
    ///
    /// A physical keyboard sends `Event::Text`, which is why this survived every test and
    /// every desktop run.
    #[test]
    fn an_on_screen_keyboards_letters_reach_the_shell() {
        let commit = vec![egui::Event::Ime(egui::ImeEvent::Commit("ls".into()))];
        let bytes: Vec<u8> = drive(commit, PLAIN).iter().flat_map(|o| o.bytes.clone()).collect();
        assert_eq!(bytes, b"ls", "an IME commit did not reach the shell");

        // The editing keys still arrive as key events, and still work — this must not trade
        // one half of the keyboard for the other.
        let bs = vec![egui::Event::Key {
            key: egui::Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        assert!(!drive(bs, PLAIN).is_empty(), "backspace stopped working");

        // A physical keyboard's Text is unchanged.
        let typed = vec![egui::Event::Text("hi".into())];
        let bytes: Vec<u8> = drive(typed, PLAIN).iter().flat_map(|o| o.bytes.clone()).collect();
        assert_eq!(bytes, b"hi");
    }

    /// PREEDIT IS NOT INPUT. It is what an IME shows while composing — a half-typed CJK
    /// syllable, a swipe keyboard's running guess — and sending it would type every
    /// intermediate state and then the final word on top of them.
    #[test]
    fn composing_text_is_not_typed_until_it_is_committed() {
        let pre = vec![egui::Event::Ime(egui::ImeEvent::Preedit("ni".into()))];
        let bytes: Vec<u8> = drive(pre, PLAIN).iter().flat_map(|o| o.bytes.clone()).collect();
        assert!(bytes.is_empty(), "a preedit was typed: {bytes:?}");

        // …and the commit that follows it types once.
        let seq = vec![
            egui::Event::Ime(egui::ImeEvent::Preedit("ni".into())),
            egui::Event::Ime(egui::ImeEvent::Preedit("nih".into())),
            egui::Event::Ime(egui::ImeEvent::Commit("\u{4f60}".into())),
        ];
        let bytes: Vec<u8> = drive(seq, PLAIN).iter().flat_map(|o| o.bytes.clone()).collect();
        assert_eq!(bytes, "\u{4f60}".as_bytes(), "the composed word did not arrive once");
    }

    /// THE TERMINAL'S OWN FOCUS SINK holds focus, which is the normal state once
    /// `kbd_focus::claim` runs. Keys must still reach the shell.
    ///
    /// This is the regression test for a bug I shipped: adding the sink made
    /// `egui_wants_keyboard_input()` true every frame, the guard read that as "somebody
    /// else is typing", and voiceterm stopped accepting input entirely. The existing tests
    /// all passed, because none of them had a focused widget that was OURS — they had a
    /// focused TextEdit (someone else's) or nothing focused at all. The missing case was
    /// the one the app is in all the time.
    fn drive_with_terminal_focused(events: Vec<egui::Event>, mode: TermMode) -> Vec<Outcome> {
        let ctx = egui::Context::default();
        let got = std::cell::RefCell::new(Vec::new());
        let id = egui::Id::new("terminal-sink");
        // Frame 1: the sink takes focus, as `kbd_focus::claim` does.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = ui.interact(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0)),
                    id,
                    egui::Sense::click(),
                );
                r.request_focus();
            });
        });
        // Frame 2: the keys arrive while we hold focus.
        let mut raw = egui::RawInput::default();
        raw.events = events;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.interact(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0)),
                    id,
                    egui::Sense::click(),
                );
                *got.borrow_mut() = drain_egui_owned(ui, mode, false, Some(id));
            });
        });
        got.into_inner()
    }

    /// Typing must work when the terminal itself holds the keyboard — the state the app is
    /// in from the first frame onward.
    #[test]
    fn keys_reach_the_shell_while_the_terminals_own_sink_has_focus() {
        let text = vec![egui::Event::Text("hi".into())];
        let out = drive_with_terminal_focused(text, PLAIN);
        let bytes: Vec<u8> = out.iter().flat_map(|o| o.bytes.clone()).collect();
        assert_eq!(bytes, b"hi", "the terminal swallowed its own input");

        // Tab too — the key the sink exists for.
        let tab = vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        let out = drive_with_terminal_focused(tab, PLAIN);
        let bytes: Vec<u8> = out.iter().flat_map(|o| o.bytes.clone()).collect();
        assert_eq!(bytes, vec![0x09], "Tab did not reach the shell");

        // And the other half still holds: a focused TextEdit is NOT the terminal, so its
        // keys are not ours to read.
        let text = vec![egui::Event::Text("x".into())];
        let out = drive_with_focused_field(text, PLAIN);
        assert!(
            out.iter().all(|o| o.bytes.is_empty()),
            "a keystroke meant for a text field reached the shell"
        );
    }

    /// Same, but with a TextEdit that HAS FOCUS in the frame — the state
    /// the app is in whenever the say box, a filter or the palette is open.
    fn drive_with_focused_field(events: Vec<egui::Event>, mode: TermMode) -> Vec<Outcome> {
        let ctx = egui::Context::default();
        let got = std::cell::RefCell::new(Vec::new());
        let mut buf = String::new();
        let id = egui::Id::new("focused-field");
        // Frame 1: create the field and give it focus.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id));
                r.request_focus();
            });
        });
        // Frame 2: the field is focused; now deliver the keys.
        let mut raw = egui::RawInput::default();
        raw.events = events;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.add(egui::TextEdit::singleline(&mut buf).id(id));
                *got.borrow_mut() = drain_egui(ui, mode, false);
            });
        });
        got.into_inner()
    }

    fn bytes(outs: &[Outcome]) -> Vec<u8> {
        outs.iter().flat_map(|o| o.bytes.clone()).collect()
    }

    fn text(s: &str) -> egui::Event {
        egui::Event::Text(s.into())
    }
    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn typing_a_word_produces_each_character_exactly_once() {
        // The doubling bug's shape: egui delivers BOTH a Text event and
        // a Key event for a printable character, and taking both sends
        // every letter twice.
        let outs = drive(vec![text("l"), key(egui::Key::L), text("s"), key(egui::Key::S)], PLAIN);
        assert_eq!(bytes(&outs), b"ls", "each character exactly once");
    }

    #[test]
    fn enter_sends_carriage_return_not_a_line_feed() {
        let outs = drive(vec![key(egui::Key::Enter)], PLAIN);
        assert_eq!(bytes(&outs), b"\r", "a shell sees \\n as a line feed, not a submit");
    }

    #[test]
    fn a_whole_command_comes_out_once_through_the_real_reader() {
        let mut evs = Vec::new();
        for c in "ls -la".chars() {
            evs.push(text(&c.to_string()));
        }
        evs.push(key(egui::Key::Enter));
        assert_eq!(bytes(&drive(evs, PLAIN)), b"ls -la\r");
    }

    #[test]
    fn the_mode_argument_actually_reaches_the_encoder() {
        // The bug: the call site passed TermMode::default(), so the
        // app-cursor branch was correct and unreachable. This drives the
        // same function the binary calls, with each mode.
        assert_eq!(bytes(&drive(vec![key(egui::Key::ArrowUp)], PLAIN)), b"\x1b[A");
        assert_eq!(bytes(&drive(vec![key(egui::Key::ArrowUp)], APP)), b"\x1bOA");
    }

    #[test]
    fn backspace_is_del_through_the_real_path() {
        assert_eq!(bytes(&drive(vec![key(egui::Key::Backspace)], PLAIN)), b"\x7f");
    }

    #[test]
    fn ctrl_c_arrives_as_the_interrupt_byte() {
        let ev = egui::Event::Key {
            key: egui::Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        assert_eq!(bytes(&drive(vec![ev], PLAIN)), b"\x03");
    }

    #[test]
    fn a_key_release_sends_nothing() {
        let ev = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(bytes(&drive(vec![ev], PLAIN)).is_empty());
    }

    #[test]
    fn a_focused_text_field_takes_the_keyboard() {
        // The gate that was missing: this reads raw context events, so
        // without it a search query typed into a TextEdit also went to
        // the shell.
        let ctx = egui::Context::default();
        let mut buf = String::new();
        let id = egui::Id::new("a-field");

        // Frame 1: create the field and give it focus.
        let mut raw = egui::RawInput::default();
        ctx.run(raw.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id));
                r.request_focus();
            });
        });

        // Frame 2: type, with the field still focused.
        raw.events = vec![text("l"), text("s")];
        let got = std::cell::RefCell::new(Vec::new());
        ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut buf).id(id));
                *got.borrow_mut() = drain_egui(ui, PLAIN, false);
            });
        });
        assert!(
            bytes(&got.into_inner()).is_empty(),
            "a focused field owns the keyboard; the shell must not also see it"
        );
    }
    /// REPORTED FROM THE DEVICE: Ctrl+C does not kill a running process.
    ///
    /// The encoder is fine — `encode(Char('c'), ctrl)` is `[0x03]`, asserted
    /// elsewhere in this file — and Ctrl+C is not a widget chord. This pins
    /// where it actually goes: `drain_egui` returns EARLY, discarding the
    /// whole event batch, whenever any egui widget wants keyboard input.
    /// With a focused text field on screen the interrupt never reaches the
    /// PTY at all.
    ///
    /// Characterising, not aspirational: it asserts what the code does
    /// today, so the behaviour is a recorded fact rather than a suspicion.
    /// The guard is there for a real reason (typing a search query was also
    /// typing it into the shell), so the fix is a judgement about which
    /// chords outrank a focused field — Ctrl+C is also Copy — and belongs
    /// with someone who can try it on the device.
    #[test]
    fn ctrl_c_reaches_the_pty_with_no_focus_and_is_swallowed_by_a_focused_field() {
        let ctrl_c = vec![egui::Event::Key {
            key: egui::Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }];

        // No focused widget: the interrupt gets through, as it should.
        let free = drive(ctrl_c.clone(), PLAIN);
        assert_eq!(bytes(&free), vec![0x03], "Ctrl+C must reach the shell when nothing has focus");

        // A focused field: the whole batch is dropped, interrupt included.
        let blocked = drive_with_focused_field(ctrl_c, PLAIN);
        assert!(bytes(&blocked).is_empty(),
                "recorded current behaviour — if this now passes bytes through, \
                 the guard was changed and the device report should be re-tested");
    }

}
