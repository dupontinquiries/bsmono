//! keys — turning key presses into the bytes a shell expects.
//!
//! The terminal could accumulate typed text and submit a line on Enter, which is
//! a text field, not a terminal. A shell needs the rest:
//!
//! * **Ctrl-C, Ctrl-D, Ctrl-Z** — the control codes. Without Ctrl-C there is no
//!   way to stop a runaway command, which makes the terminal not merely limited
//!   but unsafe to use.
//! * **Arrows** — history and line editing live behind these. A shell with no
//!   up-arrow is a shell where every command is retyped.
//! * **Tab** — completion. Typing a full path by hand is a punishment.
//! * **Home/End/Delete, Page Up/Down** — ordinary editing people do without
//!   thinking, and notice sharply when it is missing.
//!
//! These are ANSI/xterm sequences, and the encoding is not a matter of taste:
//! a shell parses exactly these bytes, so anything else is silently wrong. They
//! are written out with the standards' names attached, because "\x1b[A" is
//! unreadable and a wrong one is invisible until a user hits it.
//!
//! ## Why this is a separate module
//!
//! It is pure: keys and modifiers in, bytes out. No egui, no terminal state, so
//! every sequence can be asserted in a unit test — which matters because a
//! wrong escape is not a crash, it is a shell that behaves strangely in one
//! circumstance nobody thought to try.

/// Modifier state, in the terms the sequences care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub fn none() -> Mods { Mods::default() }
    pub fn ctrl() -> Mods { Mods { ctrl: true, ..Default::default() } }
    pub fn alt() -> Mods { Mods { alt: true, ..Default::default() } }
    pub fn shift() -> Mods { Mods { shift: true, ..Default::default() } }

    /// xterm's modifier parameter: 1 + bits for shift(1) alt(2) ctrl(4).
    /// Used in the `CSI 1 ; N X` form that carries modified arrows.
    fn param(&self) -> u8 {
        1 + (self.shift as u8) + 2 * (self.alt as u8) + 4 * (self.ctrl as u8)
    }
    fn any(&self) -> bool { self.ctrl || self.alt || self.shift }
}

/// The keys a terminal cares about beyond ordinary text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up, Down, Right, Left,
    Home, End, Insert, Delete,
    PageUp, PageDown,
    Enter, Tab, Backspace, Escape,
    F(u8),
    /// A printable character, so Ctrl-<letter> can be encoded here too.
    Char(char),
}

/// Encode a key press as the bytes a shell expects, or `None` when the press
/// carries nothing (a bare modifier, an unmapped function key).
/// The two terminal modes that change what a key means. Set by the program
/// running in the terminal, which is why encoding cannot be a pure function of
/// the key: the same arrow press is two different byte sequences depending on
/// what the foreground process asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermMode {
    /// DECCKM (CSI ? 1 h). Arrows become SS3 — `ESC O A` rather than `ESC [ A`.
    pub app_cursor: bool,
    /// DEC 2004. Pastes are wrapped in markers so a program can tell pasted
    /// text from typing.
    pub bracketed_paste: bool,
}

/// Encode with the default (reset) modes. Kept so existing callers do not have
/// to thread a mode they do not track.
pub fn encode(key: Key, m: Mods) -> Option<Vec<u8>> {
    encode_mode(key, m, TermMode::default())
}

/// Encode a keypress for a terminal in a given mode.
pub fn encode_mode(key: Key, m: Mods, mode: TermMode) -> Option<Vec<u8>> {
    let csi = |b: u8| -> Vec<u8> {
        if m.any() {
            // CSI 1 ; N <final> — the modified form. Emitting the unmodified
            // sequence for a modified press is how Ctrl-Left silently becomes
            // plain Left, which is maddening precisely because it half-works.
            format!("\x1b[1;{}{}", m.param(), b as char).into_bytes()
        } else if mode.app_cursor {
            // DECCKM: fzf, vim, less and readline in some configs set this, and
            // an unmodified arrow must then be SS3. voiceterm always sent CSI,
            // which broke arrow navigation inside exactly those programs — the
            // reported symptom was Ctrl-R history search where you could not
            // move through the matched text. Modified arrows keep the CSI 1;N
            // form in both modes; that is what xterm does.
            vec![0x1b, b'O', b]
        } else {
            vec![0x1b, b'[', b]
        }
    };
    // `CSI n ~` keys (Home/End/Insert/Delete/PgUp/PgDn) take their modifier
    // between the number and the tilde.
    let tilde = |n: u8| -> Vec<u8> {
        if m.any() {
            format!("\x1b[{};{}~", n, m.param()).into_bytes()
        } else {
            format!("\x1b[{}~", n).into_bytes()
        }
    };
    Some(match key {
        Key::Up => csi(b'A'),
        Key::Down => csi(b'B'),
        Key::Right => csi(b'C'),
        Key::Left => csi(b'D'),
        // Home and End have a CSI H/F form as well as the tilde form; xterm
        // sends H/F, and shells accept both.
        Key::Home => csi(b'H'),
        Key::End => csi(b'F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Enter => vec![b'\r'],
        // Tab is 0x09; SHIFT-Tab is a different sequence entirely (CSI Z), and
        // conflating them breaks reverse completion.
        // Tab is 0x09; SHIFT-Tab is CSI Z. ALT-Tab is ESC TAB, which is what
        // zsh's autosuggestion-accept and forward-word bindings expect —
        // checking only the shift bit meant every modified Tab arrived as a
        // bare completion request.
        Key::Tab => if m.shift { b"\x1b[Z".to_vec() }
                    else if m.alt { b"\x1b\t".to_vec() }
                    else { vec![0x09] },
        // Backspace sends DEL (0x7f), not BS (0x08). This is the single most
        // common terminal bug: send 0x08 and readline treats it as Ctrl-H,
        // which usually also deletes, so it looks fine until something binds
        // Ctrl-H differently.
        Key::Backspace => vec![0x7f],
        Key::Escape => vec![0x1b],
        Key::F(n) => match n {
            1..=4 => vec![0x1b, b'O', b'P' + (n - 1)],       // SS3 P/Q/R/S
            5 => tilde(15),
            6..=8 => tilde(15 + n - 5),                      // 17,18,19
            9..=12 => tilde(20 + n - 9),                     // 20,21,23,24
            _ => return None,
        },
        Key::Char(c) => {
            if m.ctrl {
                // Ctrl-<letter> is the letter with the top three bits cleared:
                // Ctrl-A is 0x01, Ctrl-C is 0x03. Without this there is no way
                // to interrupt a runaway command.
                let u = c.to_ascii_uppercase();
                match u {
                    'A'..='Z' => vec![(u as u8) - b'A' + 1],
                    '[' => vec![0x1b],
                    '\\' => vec![0x1c],
                    ']' => vec![0x1d],
                    ' ' | '@' => vec![0x00],
                    _ => return None,
                }
            } else if m.alt {
                // Alt-<key> is ESC then the key, which is how readline's
                // meta bindings (Alt-B, Alt-F for word motion) arrive.
                let mut v = vec![0x1b];
                v.extend(c.to_string().into_bytes());
                v
            } else {
                c.to_string().into_bytes()
            }
        }
    })
}

/// The control byte for Ctrl-<c>, exposed so callers can ask without going
/// through a full keypress. Case-insensitive: Ctrl+Shift+C still means 0x03 on
/// the platforms that deliver it at all.
pub fn ctrl_byte_of(c: char) -> Option<u8> {
    let u = c.to_ascii_uppercase();
    match u {
        'A'..='Z' => Some((u as u8) - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        ' ' | '@' => Some(0x00),
        _ => {
            // Some platforms hand us the control character itself.
            let n = c as u32;
            if (1..=26).contains(&n) { Some(n as u8) } else { None }
        }
    }
}

/// Normalise assembled text to the byte a physical Enter sends.
///
/// `\n` becomes `\r`, and `\r\n` collapses to one `\r`. Anything that builds a
/// command string from another source — voice dispatch, a command palette,
/// history replay — produces line feeds, and a shell handed `\n` stacks the
/// cursor instead of running the command.
pub fn canonical(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_cr = false;
    for ch in raw.chars() {
        match ch {
            '\r' => { out.push('\r'); prev_cr = true; }
            '\n' => { if !prev_cr { out.push('\r'); } prev_cr = false; }
            c => { out.push(c); prev_cr = false; }
        }
    }
    out
}

/// Encode a paste, wrapped in DEC 2004 markers when the program asked for them.
///
/// The markers are what let an editor tell pasted text from typing and skip
/// auto-indent — the difference between pasting code and pasting a staircase.
pub fn paste(text: &str, mode: TermMode) -> Vec<u8> {
    let body = canonical(text);
    if !mode.bracketed_paste { return body.into_bytes(); }
    let mut v = b"\x1b[200~".to_vec();
    v.extend_from_slice(body.as_bytes());
    v.extend_from_slice(b"\x1b[201~");
    v
}

/// Does this press interrupt the foreground command?
///
/// Worth naming rather than leaving as a byte comparison at the call site: a UI
/// wanting to show "interrupted" needs to know, and comparing to `\x03` in three
/// places is how one of them ends up wrong.
pub fn is_interrupt(key: Key, m: Mods) -> bool {
    matches!(key, Key::Char(c) if m.ctrl && c.eq_ignore_ascii_case(&'c'))
}

/// Does this press signal end-of-input (Ctrl-D)?
pub fn is_eof(key: Key, m: Mods) -> bool {
    matches!(key, Key::Char(c) if m.ctrl && c.eq_ignore_ascii_case(&'d'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(k: Key, m: Mods) -> String {
        String::from_utf8_lossy(&encode(k, m).expect("encodes")).into_owned()
    }

    #[test]
    fn the_arrows_are_the_sequences_a_shell_actually_parses() {
        // A wrong escape is not a crash: it is a shell that behaves strangely
        // in one circumstance nobody thought to try.
        assert_eq!(enc(Key::Up, Mods::none()), "\x1b[A");
        assert_eq!(enc(Key::Down, Mods::none()), "\x1b[B");
        assert_eq!(enc(Key::Right, Mods::none()), "\x1b[C");
        assert_eq!(enc(Key::Left, Mods::none()), "\x1b[D");
    }

    #[test]
    fn ctrl_c_interrupts_and_ctrl_letters_are_control_codes() {
        // Without Ctrl-C there is no way to stop a runaway command, which makes
        // a terminal not merely limited but unsafe.
        assert_eq!(encode(Key::Char('c'), Mods::ctrl()).unwrap(), vec![0x03]);
        assert_eq!(encode(Key::Char('C'), Mods::ctrl()).unwrap(), vec![0x03]);
        assert_eq!(encode(Key::Char('a'), Mods::ctrl()).unwrap(), vec![0x01]);
        assert_eq!(encode(Key::Char('d'), Mods::ctrl()).unwrap(), vec![0x04]);
        assert_eq!(encode(Key::Char('z'), Mods::ctrl()).unwrap(), vec![0x1a]);
        assert!(is_interrupt(Key::Char('c'), Mods::ctrl()));
        assert!(!is_interrupt(Key::Char('c'), Mods::none()), "plain c is a letter");
        assert!(is_eof(Key::Char('d'), Mods::ctrl()));
    }

    #[test]
    fn backspace_sends_del_not_bs() {
        // The single most common terminal bug. Sending 0x08 makes readline treat
        // it as Ctrl-H, which usually also deletes — so it looks correct until
        // something binds Ctrl-H differently, and then it is baffling.
        assert_eq!(encode(Key::Backspace, Mods::none()).unwrap(), vec![0x7f]);
        assert_ne!(encode(Key::Backspace, Mods::none()).unwrap(), vec![0x08]);
    }

    #[test]
    fn shift_tab_is_not_tab() {
        // Conflating them breaks reverse completion, which is the only reason
        // shift-tab exists.
        assert_eq!(encode(Key::Tab, Mods::none()).unwrap(), vec![0x09]);
        assert_eq!(enc(Key::Tab, Mods::shift()), "\x1b[Z");
    }

    #[test]
    fn a_modified_arrow_is_not_silently_downgraded() {
        // Emitting the plain sequence for a modified press turns Ctrl-Left into
        // Left — maddening precisely because it half-works.
        assert_eq!(enc(Key::Left, Mods::ctrl()), "\x1b[1;5D");
        assert_eq!(enc(Key::Right, Mods::ctrl()), "\x1b[1;5C");
        assert_eq!(enc(Key::Up, Mods::shift()), "\x1b[1;2A");
        assert_eq!(enc(Key::Left, Mods::alt()), "\x1b[1;3D");
        // and the parameter follows xterm's bit scheme
        let both = Mods { ctrl: true, shift: true, alt: false };
        assert_eq!(enc(Key::Left, both), "\x1b[1;6D");
    }

    #[test]
    fn the_editing_keys_use_their_tilde_forms() {
        assert_eq!(enc(Key::Delete, Mods::none()), "\x1b[3~");
        assert_eq!(enc(Key::Insert, Mods::none()), "\x1b[2~");
        assert_eq!(enc(Key::PageUp, Mods::none()), "\x1b[5~");
        assert_eq!(enc(Key::PageDown, Mods::none()), "\x1b[6~");
        // modified, the parameter goes before the tilde
        assert_eq!(enc(Key::Delete, Mods::ctrl()), "\x1b[3;5~");
    }

    #[test]
    fn alt_letters_are_escape_prefixed_for_readline_word_motion() {
        // Alt-B and Alt-F are how you move by words in every readline shell.
        assert_eq!(enc(Key::Char('b'), Mods::alt()), "\x1bb");
        assert_eq!(enc(Key::Char('f'), Mods::alt()), "\x1bf");
    }

    #[test]
    fn enter_sends_carriage_return() {
        // CR, not LF: a shell's line discipline expects \r, and sending \n
        // works in some shells and not others, which is the worst kind of bug.
        assert_eq!(encode(Key::Enter, Mods::none()).unwrap(), vec![b'\r']);
    }

    #[test]
    fn function_keys_and_unmapped_presses_behave() {
        assert_eq!(enc(Key::F(1), Mods::none()), "\x1bOP");
        assert_eq!(enc(Key::F(4), Mods::none()), "\x1bOS");
        assert_eq!(enc(Key::F(5), Mods::none()), "\x1b[15~");
        // an unmapped function key returns None rather than inventing bytes
        assert!(encode(Key::F(99), Mods::none()).is_none());
        // as does a ctrl-combination with no control code
        assert!(encode(Key::Char('1'), Mods::ctrl()).is_none());
    }

    #[test]
    fn plain_text_passes_through_including_non_ascii() {
        assert_eq!(enc(Key::Char('a'), Mods::none()), "a");
        assert_eq!(enc(Key::Char('é'), Mods::none()), "é");
        assert_eq!(enc(Key::Char('→'), Mods::none()), "→");
    }
}
