//! edit_actions — the editing commands a focused field can use, chosen by
//! what the keyboard can see of it.
//!
//! The keyboard is told two things about the field it is typing into, and
//! both used to go unused here: its PURPOSE (`content_type`: plain text,
//! password, terminal, …) and its surrounding text with the cursor and
//! ANCHOR — anchor ≠ cursor means something is selected. From those:
//!
//! * selected text → Cut / Copy first
//! * a terminal → its own chords: Copy/Paste are Ctrl+Shift+C/V there
//!   (Ctrl+C is interrupt), plus ^C ^D ^R ^L and Esc
//! * a password → Paste only (nothing may be copied out of it)
//! * otherwise → Paste, Select all, Undo, Redo, Ctrl+R
//!
//! Merged after the focused app's own declared commands (`keyboard_strip`),
//! with the app's winning where both bind the same chord. Shown in the
//! bottom strip only when there is something to act on — an app strip, a
//! selection, a terminal — or the person pinned it open (long-press the
//! rail), so an ordinary text field keeps the keyboard its usual height.

use keyboard_strip::Action;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Purpose {
    #[default]
    Unknown,
    Text,
    Password,
    Terminal,
}

impl Purpose {
    /// From the stored `content_purpose` (text-input-v1 numbering;
    /// `board_compositor::content_purpose` normalises v3 to it).
    pub fn from_v1(p: u8) -> Purpose {
        match p {
            255 => Purpose::Unknown,
            8 => Purpose::Password,
            12 => Purpose::Terminal,
            _ => Purpose::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditContext {
    pub purpose: Purpose,
    /// Something is selected in the field (surrounding text's anchor ≠
    /// cursor). False when the app does not report surrounding text.
    pub selection: bool,
}

pub fn context_actions(c: &EditContext) -> Vec<Action> {
    let a = Action::new;
    match c.purpose {
        Purpose::Terminal => vec![
            a("Copy", "Ctrl+Shift+c", "copy (a terminal's Ctrl+C interrupts)"),
            a("Paste", "Ctrl+Shift+v", "paste"),
            a("^C", "Ctrl+c", "interrupt"),
            a("^D", "Ctrl+d", "end of input"),
            a("^R", "Ctrl+r", "search history"),
            a("^L", "Ctrl+l", "clear"),
            a("Esc", "Escape", "escape"),
        ],
        Purpose::Password => vec![a("Paste", "Ctrl+v", "paste")],
        Purpose::Text | Purpose::Unknown => {
            let mut v = Vec::new();
            if c.selection {
                v.push(a("Cut", "Ctrl+x", "cut the selection"));
                v.push(a("Copy", "Ctrl+c", "copy the selection"));
            }
            v.extend([
                a("Paste", "Ctrl+v", "paste"),
                a("Select all", "Ctrl+a", "select all"),
                a("Undo", "Ctrl+z", "undo"),
                a("Redo", "Ctrl+Shift+z", "redo"),
                a("Ctrl R", "Ctrl+r", "Ctrl+R: reload, replace, run — whatever the app binds"),
            ]);
            v
        }
    }
}

/// What the bottom strip shows: the app's commands, then the field's, the
/// app winning a shared chord. Empty (strip hidden) unless there is
/// something to act on or it is pinned open.
pub fn strip(app: &[Action], c: &EditContext, pinned: bool) -> Vec<Action> {
    let worth_showing = !app.is_empty() || c.selection || c.purpose == Purpose::Terminal || pinned;
    if !worth_showing {
        return Vec::new();
    }
    let mut out: Vec<Action> = app.to_vec();
    for a in context_actions(c) {
        if !out.iter().any(|x| x.keys.eq_ignore_ascii_case(&a.keys)) {
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(purpose: Purpose, selection: bool) -> EditContext {
        EditContext { purpose, selection }
    }
    fn keys(v: &[Action]) -> Vec<&str> {
        v.iter().map(|a| a.keys.as_str()).collect()
    }

    #[test]
    fn an_ordinary_field_keeps_the_keyboard_its_usual_height() {
        assert!(strip(&[], &ctx(Purpose::Text, false), false).is_empty());
        assert!(!strip(&[], &ctx(Purpose::Text, false), true).is_empty(), "unless pinned open");
    }

    #[test]
    fn a_selection_brings_cut_and_copy_first() {
        let s = strip(&[], &ctx(Purpose::Text, true), false);
        assert_eq!(&keys(&s)[..3], ["Ctrl+x", "Ctrl+c", "Ctrl+v"]);
    }

    #[test]
    fn a_terminal_gets_its_own_chords_and_ctrl_c_is_not_copy() {
        let s = strip(&[], &ctx(Purpose::Terminal, false), false);
        let copy = s.iter().find(|a| a.label == "Copy").unwrap();
        assert_eq!(copy.keys, "Ctrl+Shift+c");
        assert!(keys(&s).contains(&"Ctrl+r"), "history search");
    }

    #[test]
    fn nothing_can_be_copied_out_of_a_password() {
        let s = strip(&[], &ctx(Purpose::Password, true), true);
        assert_eq!(keys(&s), ["Ctrl+v"]);
    }

    #[test]
    fn the_app_comes_first_and_wins_a_shared_chord() {
        let app = vec![Action::new("Find", "Ctrl+f", ""), Action::new("Rename", "Ctrl+r", "")];
        let s = strip(&app, &ctx(Purpose::Text, false), false);
        assert_eq!(s[0].label, "Find");
        assert_eq!(s.iter().filter(|a| a.keys == "Ctrl+r").count(), 1);
        assert_eq!(s.iter().find(|a| a.keys == "Ctrl+r").unwrap().label, "Rename");
    }

    #[test]
    fn purpose_numbers() {
        assert_eq!(Purpose::from_v1(12), Purpose::Terminal);
        assert_eq!(Purpose::from_v1(8), Purpose::Password);
        assert_eq!(Purpose::from_v1(255), Purpose::Unknown);
        assert_eq!(Purpose::from_v1(5), Purpose::Text);
    }
}
