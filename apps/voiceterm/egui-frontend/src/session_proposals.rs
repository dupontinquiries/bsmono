//! session_proposals — turning what this terminal saw into `session_auto`'s vocabulary.
//!
//! `session_auto` shipped pure and tested in v0.1.658: a segmenter, a namer, and `propose`,
//! which suggests a new tab for a file, a new session for a repo, or a re-entry after a
//! reboot. The Qt build has surfaced it since v0.1.720. This frontend has never called it,
//! because `propose` wants an ACTIVITY — a kind, a target, a cwd — and all this side had was
//! command blocks.
//!
//! It also wanted a real current directory, which is why this could not have been done
//! honestly until the OSC 7 work: with every block claiming one hardcoded path, the `Cd`
//! branch could never fire and the repo it named would have been that path forever.
//!
//! ## What is claimed and what is not
//!
//! A command line is thin evidence. `cd /srv/x` is unambiguous; `vim foo.rs` is an editor
//! opening a file; `cargo build` is a build. Everything else is `Run`, which `propose`
//! ignores — and that is the right default, because a wrong proposal costs more attention
//! than a missing one. The reboot kind is NOT guessed from a command: `reboot` typed in a
//! terminal is a request, not an event, and the session that comes back is a different
//! process. That signal belongs to the app's own startup, not to a parser.
//!
//! Pure: blocks in, activities out, no clock and no filesystem.

use crate::session_auto::{Activity, ActivityKind};

/// The editors this recognises as "opening a file".
///
/// A fixed list rather than "any command with a file argument": `cat foo.rs` and
/// `rm foo.rs` both take one and neither opens an editing session, and proposing a tab for
/// them would train someone to ignore the proposals.
pub const EDITORS: &[&str] = &["vim", "nvim", "vi", "hx", "helix", "nano", "emacs", "kak", "micro"];

/// One block's activity, if it is legible as one.
///
/// `cwd` is the block's own recorded directory — where the command RAN, not where the
/// session is now.
pub fn activity_of(command: &str, cwd: &str) -> Option<Activity> {
    let mut words = command.split_whitespace();
    let head = words.next()?;
    // Arguments that are not flags: a target is a path, and `-la` is not.
    let args: Vec<&str> = words.filter(|w| !w.starts_with('-')).collect();

    let base = head.rsplit('/').next().unwrap_or(head);
    if base == "cd" {
        // A bare `cd` goes home, and this cannot know where that is — better to say nothing
        // than to name the wrong repo.
        let target = args.first()?;
        return Some(Activity::new(ActivityKind::Cd, target, cwd));
    }
    if EDITORS.contains(&base) {
        let target = args.first()?;
        return Some(Activity::new(ActivityKind::EditorOpen, target, cwd));
    }
    if matches!(base, "make" | "cargo" | "ninja" | "meson" | "cmake") {
        // `cargo test` is a test run, not a build — the two mean different things to a
        // segmenter, and `cargo` alone would swallow both.
        let kind = if args.first() == Some(&"test") { ActivityKind::Test } else { ActivityKind::Build };
        return Some(Activity::new(kind, command, cwd));
    }
    if matches!(base, "pytest" | "ctest") {
        return Some(Activity::new(ActivityKind::Test, command, cwd));
    }
    Some(Activity::new(ActivityKind::Run, command, cwd))
}

/// Every legible activity in a run of blocks, in order.
pub fn activities<'a>(blocks: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<Activity> {
    blocks.filter_map(|(cmd, cwd)| activity_of(cmd, cwd)).collect()
}

/// The files an editor was pointed at, most recent last, deduped.
///
/// `propose` takes this as "what is already open", which is a guess: nothing here knows
/// whether the editor is still running. It is the same guess the Qt side makes, and the
/// cost of being wrong is one proposal that suggests a tab for a file already in one.
pub fn open_files(acts: &[Activity]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in acts.iter().filter(|a| a.kind == ActivityKind::EditorOpen) {
        if !out.contains(&a.target) {
            out.push(a.target.clone());
        }
    }
    out
}

/// The language most of the opened files are in, if there is one.
pub fn dominant_lang(acts: &[Activity]) -> Option<&'static str> {
    let mut counts: std::collections::HashMap<&'static str, usize> = Default::default();
    for a in acts {
        if let Some(l) = a.lang() {
            *counts.entry(l).or_default() += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(l, _)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cd_is_the_one_unambiguous_signal() {
        let a = activity_of("cd /srv/work", "/home/k").expect("legible");
        assert_eq!(a.kind, ActivityKind::Cd);
        assert_eq!(a.target, "/srv/work");
        assert_eq!(a.cwd, "/home/k", "the cwd is where the command RAN, not where it went");

        // A bare `cd` goes home and this cannot know where that is. Saying nothing beats
        // naming the wrong repo.
        assert!(activity_of("cd", "/home/k").is_none());
        // Flags are not targets.
        assert_eq!(activity_of("cd -- /tmp", "/x").unwrap().target, "/tmp");
    }

    #[test]
    fn an_editor_opens_a_file_and_a_pager_does_not() {
        let a = activity_of("hx src/main.rs", "/p").expect("legible");
        assert_eq!(a.kind, ActivityKind::EditorOpen);
        assert_eq!(a.target, "src/main.rs");
        assert_eq!(a.lang(), Some("rust"));
        // A full path to the editor still counts.
        assert_eq!(activity_of("/usr/bin/nvim x.py", "/p").unwrap().kind, ActivityKind::EditorOpen);

        // `cat` and `rm` take a file too, and neither opens an editing session. Proposing a
        // tab for them would train someone to ignore the proposals.
        for cmd in ["cat foo.rs", "rm foo.rs", "ls foo.rs"] {
            assert_eq!(activity_of(cmd, "/p").unwrap().kind, ActivityKind::Run, "{cmd}");
        }
        // An editor with no file is not opening one.
        assert!(activity_of("vim", "/p").is_none());
    }

    /// `cargo test` is a test run, not a build. The two mean different things to a
    /// segmenter, and matching on `cargo` alone would swallow both.
    #[test]
    fn builds_and_tests_are_told_apart() {
        assert_eq!(activity_of("cargo build", "/p").unwrap().kind, ActivityKind::Build);
        assert_eq!(activity_of("cargo test --lib", "/p").unwrap().kind, ActivityKind::Test);
        assert_eq!(activity_of("make -j8", "/p").unwrap().kind, ActivityKind::Build);
        assert_eq!(activity_of("pytest", "/p").unwrap().kind, ActivityKind::Test);
    }

    /// A reboot is an EVENT, not a command. `reboot` typed in a terminal is a request, and
    /// the session that comes back is a different process — that signal belongs to the app's
    /// startup, not to a command parser.
    #[test]
    fn a_reboot_is_never_inferred_from_a_command_line() {
        for cmd in ["reboot", "sudo reboot", "systemctl reboot", "shutdown -r now"] {
            assert_eq!(activity_of(cmd, "/p").unwrap().kind, ActivityKind::Run, "{cmd}");
        }
    }

    #[test]
    fn open_files_and_the_dominant_language_come_from_the_editor_opens() {
        let acts = activities(
            [
                ("hx a.rs", "/p"),
                ("cargo build", "/p"),
                ("hx b.rs", "/p"),
                ("hx a.rs", "/p"), // reopened
                ("vim c.py", "/p"),
            ]
            .into_iter(),
        );
        assert_eq!(open_files(&acts), vec!["a.rs".to_string(), "b.rs".to_string(), "c.py".to_string()]);
        assert_eq!(dominant_lang(&acts), Some("rust"), "two rust files to one python");

        // Nothing opened, nothing claimed.
        let none = activities([("ls", "/p"), ("echo hi", "/p")].into_iter());
        assert!(open_files(&none).is_empty());
        assert_eq!(dominant_lang(&none), None);
    }

    /// The end-to-end shape: blocks in, a proposal out. This is what had no caller.
    #[test]
    fn a_cd_into_another_repo_proposes_a_session_for_it() {
        use crate::session_auto::{propose, Proposal};
        let acts = activities([("hx src/lib.rs", "/home/k/voiceterm"), ("cargo build", "/home/k/voiceterm")].into_iter());
        let next = activity_of("cd /home/k/rusty-board", "/home/k/voiceterm").unwrap();
        let got = propose(&open_files(&acts), dominant_lang(&acts), "voiceterm", &next, "hx");
        assert_eq!(got, Some(Proposal::NewSessionForRepo { repo: "rusty-board".into() }));

        // A cd WITHIN the same repo proposes nothing — a subdirectory is not a new session,
        // and suggesting one on every `cd src` would be unusable.
        let same = activity_of("cd /home/k/voiceterm", "/home/k/voiceterm").unwrap();
        assert_eq!(propose(&open_files(&acts), dominant_lang(&acts), "voiceterm", &same, "hx"), None);
    }
}
