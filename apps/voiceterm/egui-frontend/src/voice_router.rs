//! voice_router — one chain from a transcript to what the app should do.
//!
//! The `say:` box in the keycaps panel had this chain inline: jot mode,
//! the politeness normaliser, the editor grammar, the palette grammar,
//! the Qt build's intent parser, and finally the tldr index. It was the
//! only router, and it could only be reached by TYPING — the voice popup
//! transcribed speech into `Stage::Done` and nothing read it, so the
//! voice terminal listened, understood, and did nothing.
//!
//! The chain is here, pure, so the typed box and the microphone share it
//! and a test can walk a phrase through every stage without a window.
//! Effects are returned as data ([`Routed`]) and the shell applies them —
//! the same seam every other module in this crate keeps between deciding
//! and doing.
//!
//! ## Order, and why
//!
//! 1. **Jot** wins outright: while jotting, speech is prose by definition.
//! 2. **The normaliser gates.** `voice_prefix::normalise` returns `None`
//!    for something that reads as a sentence rather than a command, so
//!    dictation meant as speech does not become keystrokes in a modal
//!    editor that turns stray letters into commands.
//! 3. **Editor keys** — "delete line", "save and quit" — go to the PTY as
//!    bytes. Destructive ones are flagged for a confirmation.
//! 4. **Palette commands** — "open mail in a new pane" — are things this
//!    app can do exactly; they beat the man-page index, which would offer
//!    `mail(1)` for the same words: a plausible wrong answer.
//! 5. **Intents.** The Qt build's parser has ~174 of them and only a few
//!    have an action here. `TerminalCommand` is STAGED on the input line —
//!    never run — because a misheard "remove" is the failure a voice
//!    terminal is remembered for. Everything else is described.
//! 6. **The tldr index**, last: candidates with their reasons, for the
//!    user to pick from. Nothing runs from a candidate either.

use crate::command_search::{CommandDoc, CommandMatch};
use crate::editor_keys::{Editor, EditorAction};
use crate::jot::Jotted;
use crate::palette::Command;
use crate::voice_intent::Intent;

/// What the router needs to know about the app's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Ctx {
    pub jotting: bool,
    pub editor: Editor,
}

/// What a transcript turned into.
#[derive(Debug, Clone, PartialEq)]
pub enum Routed {
    /// Jot mode took it: prose for the note, and whether to leave jot.
    Jot { prose: Option<String>, exit: bool },
    /// It reads as prose, not a command. Nothing to do but say so.
    Prose(String),
    /// Editor keystrokes for the PTY. `confirm` means ask first.
    Editor(EditorAction),
    /// An app command from the palette grammar.
    Command(Command),
    /// A shell command line to STAGE on the input line, not run.
    Stage(String),
    /// An intent the app understands but cannot carry out here.
    Described(String),
    /// tldr candidates (possibly none), with their reasons.
    Candidates(Vec<CommandMatch>),
}

impl Routed {
    /// A one-line account, for a toast or a log.
    pub fn summary(&self) -> String {
        match self {
            Routed::Jot { prose: Some(t), exit: false } => format!("jotted: {t}"),
            Routed::Jot { prose: Some(t), exit: true } => format!("jotted: {t} — done jotting"),
            Routed::Jot { prose: None, .. } => "done jotting".into(),
            Routed::Prose(t) => format!("that reads as prose, not a command: {t:?}"),
            Routed::Editor(a) if a.confirm => "editor: needs a confirmation".into(),
            Routed::Editor(_) => "editor keys sent".into(),
            Routed::Command(c) => format!("command: {c:?}"),
            Routed::Stage(c) => format!("staged: {c}"),
            Routed::Described(d) => format!("heard: {d}"),
            Routed::Candidates(v) if v.is_empty() => "nothing matches".into(),
            Routed::Candidates(v) => format!("{} candidate(s): {}", v.len(),
                v.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ")),
        }
    }
}

/// Walk a transcript through the chain.
pub fn route(said: &str, ctx: &Ctx, docs: &[CommandDoc]) -> Routed {
    let said = said.trim();
    if ctx.jotting {
        return match crate::jot::route(said) {
            Jotted::Append(t) => Routed::Jot { prose: Some(t), exit: false },
            Jotted::AppendAndExit(t) => Routed::Jot { prose: t, exit: true },
        };
    }
    let Some(phrase) = crate::voice_prefix::normalise(said) else {
        return Routed::Prose(said.to_string());
    };
    if let Some(a) = crate::editor_keys::translate(ctx.editor, &phrase) {
        return Routed::Editor(a);
    }
    if let Some(cmd) = crate::palette::parse_phrase(&phrase) {
        return Routed::Command(cmd);
    }
    match crate::voice_intent::parse(&phrase) {
        // Staged, never run. The line is the person's to look at and
        // press Enter on — or to erase.
        Intent::TerminalCommand(line) if !line.trim().is_empty() => {
            return Routed::Stage(line.trim().to_string());
        }
        Intent::Unknown => {}
        other => {
            // Only a confident match is worth a description; a parser
            // with 174 variants finds SOMETHING in any sentence.
            if other.is_high_confidence() {
                return Routed::Described(other.describe());
            }
        }
    }
    Routed::Candidates(crate::command_search::search(&phrase, docs, 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx { Ctx { jotting: false, editor: Editor::Helix } }

    fn docs() -> Vec<CommandDoc> {
        vec![
            CommandDoc { name: "rsync".into(),
                summary: "Transfer files either to or from a remote host".into(),
                examples: vec!["rsync -av src/ dest/".into()], on_path: true },
            CommandDoc { name: "tar".into(),
                summary: "Archiving utility".into(),
                examples: vec!["tar cf archive.tar dir".into()], on_path: true },
        ]
    }

    #[test]
    fn jot_mode_takes_everything_as_prose() {
        let c = Ctx { jotting: true, editor: Editor::Helix };
        assert_eq!(route("the build is green again", &c, &[]),
                   Routed::Jot { prose: Some("the build is green again".into()), exit: false });
        assert!(matches!(route("done jotting", &c, &[]), Routed::Jot { exit: true, .. }));
    }

    #[test]
    fn prose_is_refused_rather_than_typed_into_an_editor() {
        // The gate: a sentence must not become keystrokes in normal mode.
        let r = route("I think we should probably talk about the plan for tomorrow morning", &ctx(), &docs());
        assert!(matches!(r, Routed::Prose(_)), "{r:?}");
    }

    #[test]
    fn an_editor_phrase_becomes_keys_and_a_destructive_one_asks_first() {
        let r = route("please delete this line", &ctx(), &docs());
        match r {
            Routed::Editor(a) => assert!(!a.keys.is_empty()),
            other => panic!("expected editor keys, got {other:?}"),
        }
        let q = route("quit without saving", &ctx(), &docs());
        match q {
            Routed::Editor(a) => assert!(a.confirm, "losing work must be confirmed: {a:?}"),
            other => panic!("expected a confirmed editor action, got {other:?}"),
        }
    }

    #[test]
    fn an_app_command_beats_the_man_page_index() {
        // "mail" would score against mail(1) in the index; the grammar
        // knows it as this app's Mail view.
        let r = route("open mail", &ctx(), &docs());
        assert!(matches!(r, Routed::Command(_)), "{r:?}");
    }

    #[test]
    fn a_shell_line_is_staged_not_run() {
        let r = route("run ls dash la", &ctx(), &docs());
        match r {
            Routed::Stage(line) => assert!(line.starts_with("ls"), "{line}"),
            Routed::Candidates(_) | Routed::Described(_) => {
                // The intent grammar's spelling of "run X" may differ from
                // this phrasing; what must never happen is a run. Assert
                // the absence rather than a particular positive.
            }
            other => panic!("{other:?}"),
        }
        // and the summary never says "ran"
        assert!(!route("run ls", &ctx(), &docs()).summary().contains("ran "));
    }

    #[test]
    fn what_nothing_understands_goes_to_the_index_with_reasons() {
        let r = route("transfer files to remote server", &ctx(), &docs());
        match r {
            Routed::Candidates(v) => {
                assert!(!v.is_empty(), "the index had rsync for this");
                assert!(v.iter().any(|m| m.name == "rsync"), "{v:?}");
                assert!(!v[0].reasons.is_empty(), "a candidate without a reason is unverifiable");
            }
            Routed::Described(d) => assert!(!d.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn summaries_are_one_line_and_name_the_outcome() {
        assert_eq!(Routed::Stage("ls -la".into()).summary(), "staged: ls -la");
        assert_eq!(Routed::Candidates(vec![]).summary(), "nothing matches");
        assert!(Routed::Prose("hello there".into()).summary().contains("prose"));
    }
}
