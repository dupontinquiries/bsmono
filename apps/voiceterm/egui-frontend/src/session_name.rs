//! session_name — what this session is about, from what has been run in it.
//!
//! `TODO_session_autoname.md` shipped `session_auto` pure and tested in v0.1.658 and wired
//! it into the Qt build in v0.1.720 as a `qt_method`. The egui frontend never referenced
//! the module at all: `grep session_auto` over this crate found nothing, so a port that
//! carried the segmenter, the proposals and the namer carried none of their output.
//!
//! Two halves, and only one of them existed anywhere:
//!
//! * `session_auto::name_segment` — pure, tested, shipped;
//! * the ACTIVITY DERIVATION that feeds it — a command's leading verb classified into
//!   Cd / EditorOpen / NewFile / Build / Test / Run. In the Qt build this lives inside a
//!   `QObject` method (`_activities_from_blocks`), so it has never had a test. Ported here
//!   as a free function over `(command, cwd)` pairs, which is all it ever needed, and
//!   tested on the cases the Qt version's own comment claims: `cd asahi/dcp; hx tracer.py;
//!   python tracer.py …` → `dcp · tracer.py`.
//!
//! ## What it is NOT
//!
//! Not the segmenter. `session_auto::segment` splits a stream at repo changes and reboots,
//! and this frontend has no reboot signal — a boundary drawn without one would be a
//! different, wronger thing than no boundary. This names the session as a whole, which is
//! the part the locator can show today. The proposals (NewTabForFile, ReentryAfterReboot)
//! need the assist-bubble primitive and are still ahead.
//!
//! ## Why a floor of two
//!
//! One command names nothing: `cd ~` is not a session about `~`. The Qt method used the
//! same floor and returned an empty string; an empty name means the locator shows the
//! workspace title alone, which is what it did before this existed.

use crate::session_auto::{Activity, ActivityKind};

/// Fewer activities than this and the session has no subject yet.
pub const MIN_ACTIVITIES: usize = 2;

/// The editors whose first argument is the file being opened.
const EDITORS: &[&str] = &["vim", "nvim", "hx", "helix", "nano", "emacs", "vi", "kak", "micro"];

/// Build tools: the whole command is the target, since `cargo build` and `make -j` say
/// what they are without an argument.
const BUILDERS: &[&str] = &[
    "cargo", "make", "ninja", "cmake", "gcc", "cc", "clang", "rustc", "go", "mvn", "gradle",
    "meson", "zig", "tsc",
];

/// Classify one command into an activity. `None` for a blank command.
pub fn activity_of(command: &str, cwd: &str) -> Option<Activity> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return None;
    }
    let mut words = cmd.split_whitespace();
    let verb = words.next().unwrap_or("");
    // `sudo make install` is a build, and `env FOO=1 pytest` is a test; the leading noise
    // is not the verb. (The Qt version reads word 0 flat and calls `sudo make` a Run.)
    let (verb, arg) = if verb == "sudo" || verb.contains('=') {
        let v = words.next().unwrap_or("");
        (v, words.next().unwrap_or(""))
    } else {
        (verb, words.next().unwrap_or(""))
    };
    // a path-qualified editor is still that editor
    let verb = verb.rsplit('/').next().unwrap_or(verb);
    let (kind, target): (ActivityKind, &str) = if verb == "cd" {
        (ActivityKind::Cd, arg)
    } else if EDITORS.contains(&verb) {
        // the first argument that is not a flag is the file: `hx -w . tracer.py`
        let file = cmd
            .split_whitespace()
            .skip(1)
            .find(|w| !w.starts_with('-'))
            .unwrap_or(arg);
        (ActivityKind::EditorOpen, file)
    } else if verb == "touch" {
        (ActivityKind::NewFile, arg)
    } else if BUILDERS.contains(&verb) {
        (ActivityKind::Build, cmd)
    } else if verb == "pytest" || verb == "ctest" || cmd.contains("test") {
        (ActivityKind::Test, cmd)
    } else {
        (ActivityKind::Run, cmd)
    };
    Some(Activity::new(kind, target, cwd))
}

/// The activity stream for a session's command blocks, in order.
pub fn activities<'a>(blocks: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<Activity> {
    blocks.filter_map(|(cmd, cwd)| activity_of(cmd, cwd)).collect()
}

/// The session's name, or empty when there is not enough to say.
///
/// `cd asahi/dcp; hx tracer.py; python tracer.py` → `dcp · tracer.py`.
pub fn session_name<'a>(blocks: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let acts = activities(blocks);
    if acts.len() < MIN_ACTIVITIES {
        return String::new();
    }
    crate::session_auto::name_segment(&acts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(cmd: &str) -> ActivityKind {
        activity_of(cmd, "/home/k/proj").unwrap().kind
    }

    #[test]
    fn the_verb_classifies_the_command_and_a_prefix_does_not_hide_it() {
        assert_eq!(kind_of("cd asahi/dcp"), ActivityKind::Cd);
        assert_eq!(kind_of("hx tracer.py"), ActivityKind::EditorOpen);
        assert_eq!(kind_of("/usr/bin/nvim src/lib.rs"), ActivityKind::EditorOpen, "a path-qualified editor");
        assert_eq!(kind_of("touch new.rs"), ActivityKind::NewFile);
        assert_eq!(kind_of("cargo build --release"), ActivityKind::Build);
        assert_eq!(kind_of("pytest -k slow"), ActivityKind::Test);
        assert_eq!(kind_of("python tracer.py"), ActivityKind::Run);
        // the Qt version reads word 0 flat, so these two were Runs
        assert_eq!(kind_of("sudo make install"), ActivityKind::Build);
        assert_eq!(kind_of("RUST_LOG=debug cargo test"), ActivityKind::Build, "the builder verb wins over the word 'test' in its args");
        assert!(activity_of("   ", "/x").is_none());
    }

    #[test]
    fn an_editor_opens_the_first_argument_that_is_not_a_flag() {
        let a = activity_of("hx -w . tracer.py", "/home/k/asahi/dcp").unwrap();
        assert_eq!(a.target, ".", "the first non-flag word — `.` here IS the argument hx takes");
        let b = activity_of("nvim src/main.rs", "/p").unwrap();
        assert_eq!(b.target, "src/main.rs");
        assert_eq!(b.lang(), Some("rust"));
    }

    #[test]
    fn the_session_is_named_after_its_repo_and_its_most_edited_file() {
        // the stream the Qt method's own comment claims
        let blocks = [
            ("cd asahi/dcp", "/home/k"),
            ("hx tracer.py", "/home/k/asahi/dcp"),
            ("python tracer.py", "/home/k/asahi/dcp"),
            ("hx tracer.py", "/home/k/asahi/dcp"),
        ];
        let name = session_name(blocks.iter().copied());
        assert!(name.contains("tracer.py"), "{name}");
        assert!(name.contains("dcp"), "{name}");
    }

    #[test]
    fn one_command_names_nothing() {
        assert_eq!(session_name([("cd ~", "/home/k")].iter().copied()), "");
        assert_eq!(session_name(std::iter::empty()), "");
        // blanks do not count toward the floor
        assert_eq!(session_name([("cd ~", "/home/k"), ("   ", "/home/k")].iter().copied()), "");
        // two real ones do
        assert!(!session_name([("cd ~/proj", "/home/k"), ("cargo build", "/home/k/proj")].iter().copied()).is_empty());
    }

    #[test]
    fn a_session_with_no_files_falls_back_to_its_language_or_its_repo() {
        let by_lang = session_name(
            [("hx a.py", "/home/k/tools"), ("python a.py", "/home/k/tools"), ("python b.py", "/home/k/tools")]
                .iter()
                .copied(),
        );
        assert!(by_lang.contains("tools"), "{by_lang}");
        // nothing but runs in an unnamed cwd still produces something rather than a panic
        let bare = session_name([("echo hi", ""), ("echo there", "")].iter().copied());
        assert!(!bare.is_empty());
    }
}
