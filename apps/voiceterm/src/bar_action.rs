//! bar_action: turning a bar selection into something typed.
//!
//! `unified_bottom_bar.md` specified one handler for the whole bar
//! rather than a callback per button, and the ids `bar_pages` emits are
//! namespaced (`tab:`, `ctl:`, `say:`, `editor:`) so that handler can
//! dispatch without a lookup table. This is the parser that was missing:
//! without it the namespaces are a convention nothing enforces, and the
//! first hand-written `if id.starts_with(...)` in QML is where they
//! start to drift.
//!
//! ## The invariant worth having
//!
//! A dictation control reached by TAPPING a bar button and the same
//! control reached by SAYING its phrase must produce the same edits.
//! Not similar — the same `Vec<Edit>`, from the same function. Two paths
//! into one behaviour is how "it works when I say it but not when I tap
//! it" bugs are born, and they are miserable to track down because each
//! path looks correct in isolation. `edits_for` is the single door, and
//! a test compares it against `dictate::plan` on the spoken phrase.
//!
//! PURE: std only.

use crate::dictate::{plan, Ctl, Edit, Mode, Piece, Placement};

/// A parsed bar selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarAction {
    /// Switch to a tab by id. `tab:last` is the primary.
    Tab(String),
    Workspace(String),
    Launch(String),
    /// An editor command, by its bare name (`save`, `undo`).
    Editor(String),
    /// A dictation control verb.
    Dictation(Ctl),
    /// A phrase from the "Say" fan-out. HINT ONLY — see `is_hint`.
    ShowPhrase(String),
    /// The Say button itself was tapped: open the list.
    OpenSay,
}

impl BarAction {
    /// Whether this action should be shown rather than executed.
    ///
    /// A menu of things you can SAY that fires when TAPPED teaches the
    /// wrong model of the feature — the user learns to tap, never
    /// discovers the voice path, and the list stops being discovery and
    /// becomes a worse button row. The router returns the phrase; the
    /// caller displays it.
    pub fn is_hint(&self) -> bool {
        matches!(self, BarAction::ShowPhrase(_) | BarAction::OpenSay)
    }
}

/// Parse an action id emitted by `bar_pages`.
///
/// Returns None for anything unrecognized rather than guessing. An
/// unknown id means the bar and the router have drifted, and doing
/// nothing visible is a better failure than doing something plausible.
pub fn parse(id: &str) -> Option<BarAction> {
    let (ns, rest) = id.split_once(':')?;
    if rest.is_empty() && ns != "say" {
        return None;
    }
    Some(match ns {
        "tab" => BarAction::Tab(rest.into()),
        "workspace" | "space" => BarAction::Workspace(rest.into()),
        "launch" | "quick" => BarAction::Launch(rest.into()),
        "editor" => BarAction::Editor(rest.into()),
        "ctl" => BarAction::Dictation(ctl_from_id(rest)?),
        "say" => {
            if rest == "open" || rest.is_empty() {
                BarAction::OpenSay
            } else {
                BarAction::ShowPhrase(rest.into())
            }
        }
        _ => return None,
    })
}

/// Inverse of `bar_pages::ctl_id`'s suffix. Derived from `Ctl::phrases`
/// so a control added there is routable here without an edit — the
/// alternative is a second table that silently falls behind.
fn ctl_from_id(suffix: &str) -> Option<Ctl> {
    let phrase = suffix.replace('_', " ");
    Ctl::phrases().iter().find(|(p, c)| *p == phrase || c.label() == phrase).map(|(_, c)| *c)
}

/// The edits a tapped dictation control produces.
///
/// Goes through `dictate::plan` rather than constructing `Edit`s
/// directly, so tap and speech cannot diverge.
pub fn edits_for(a: &BarAction, mode: Mode, at: Placement) -> Vec<Edit> {
    match a {
        BarAction::Dictation(c) => plan(&[Piece::Control(*c)], mode, at),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bar_pages::{bar_for, ctl_id, Live, Page};
    use crate::dictate::segment;

    #[test]
    fn namespaces_parse() {
        assert_eq!(parse("tab:t2"), Some(BarAction::Tab("t2".into())));
        assert_eq!(parse("tab:last"), Some(BarAction::Tab("last".into())));
        assert_eq!(parse("workspace:w1"), Some(BarAction::Workspace("w1".into())));
        assert_eq!(parse("launch:q1"), Some(BarAction::Launch("q1".into())));
        assert_eq!(parse("editor:save"), Some(BarAction::Editor("save".into())));
        assert_eq!(parse("ctl:new_line"), Some(BarAction::Dictation(Ctl::NewLine)));
    }

    #[test]
    fn unknown_ids_do_nothing_rather_than_something_plausible() {
        assert_eq!(parse("bogus:x"), None);
        assert_eq!(parse("no_namespace"), None);
        assert_eq!(parse("tab:"), None);
        assert_eq!(parse("ctl:not_a_control"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn say_entries_are_hints_and_the_button_opens() {
        assert_eq!(parse("say:open"), Some(BarAction::OpenSay));
        let p = parse("say:new line").unwrap();
        assert_eq!(p, BarAction::ShowPhrase("new line".into()));
        assert!(p.is_hint());
        assert!(BarAction::OpenSay.is_hint());
        assert!(!BarAction::Dictation(Ctl::NewLine).is_hint());
        // and a hint produces no edits
        assert!(edits_for(&p, Mode::Prose, Placement::Cursor).is_empty());
    }

    #[test]
    fn every_ctl_id_round_trips() {
        for (_, c) in Ctl::phrases() {
            let id = ctl_id(*c);
            assert_eq!(parse(&id), Some(BarAction::Dictation(*c)), "{id}");
        }
    }

    #[test]
    fn every_button_the_bar_emits_is_routable() {
        // the drift check: if bar_pages grows a button, this fails
        let live = Live {
            // bare ids, as a real caller supplies them
            tabs: vec![("t1".into(), "shell".into())],
            workspaces: vec![("w1".into(), "work".into())],
            quick: vec![("q1".into(), "cargo test".into())],
        };
        for page in [
            Page::Terminal,
            Page::Web,
            Page::Editor(crate::editor_voice::Editor::Helix),
            Page::Repl("python".into()),
            Page::Dictation,
        ] {
            let bar = bar_for(&page, &live);
            for b in bar.buttons() {
                if !b.primary.is_empty() {
                    assert!(parse(&b.primary).is_some(), "{:?} primary {:?}", page, b.primary);
                }
                for o in &b.options {
                    assert!(parse(&o.id).is_some(), "{:?} option {:?}", page, o.id);
                }
            }
        }
    }

    #[test]
    fn tapping_a_control_equals_saying_it() {
        // the invariant this module exists for
        for (phrase, c) in Ctl::phrases() {
            let tapped = edits_for(
                &BarAction::Dictation(*c),
                Mode::Code,
                Placement::Cursor,
            );
            let spoken = plan(&segment(phrase), Mode::Code, Placement::Cursor);
            assert_eq!(tapped, spoken, "tap and speech diverge for {phrase:?}");
        }
    }
}
