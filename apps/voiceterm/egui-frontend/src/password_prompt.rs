//! password_prompt — `pty_term.rs`'s `foreground_is_password_prompt`, ported.
//!
//! Decides whether the terminal is currently asking for a secret. Everything
//! downstream reads it: background voice listening, token stats, the decision
//! cache, log summaries and the AI prompt builders all stop recording while it
//! is true — so it is the switch that keeps a typed password out of five
//! different stores at once.
//!
//! ## Two layers, cheapest first
//!
//! **The foreground command name** is decisive when it matches: `sudo`, `ssh`,
//! `gpg` and friends are asking for a secret whatever is on screen.
//!
//! **The last visible line** catches everything else — a script prompting by
//! hand, a program not in the list, a translated prompt.
//!
//! ## The alt-screen short-circuit is a measured fix, not a tidy-up
//!
//! The C++ note: a password prompt *"NEVER occurs inside an alt-screen TUI
//! (helix/vim/gdb-tui/less). But this method runs EVERY read tick... During
//! helix's startup redraw flood the tick fires repeatedly, so we were
//! re-rendering the full grid to a string over and over for a check whose
//! answer is trivially 'no'."*
//!
//! So the alt-screen test comes first and skips both the process read and the
//! grid render. Dropping it turns opening an editor into a latency bug.
//!
//! ## Which way to be wrong, stated
//!
//! *"False positives here mean a normal utterance gets silently dropped from
//! the typing overlay, which is annoying but not dangerous."* That is the right
//! asymmetry and it is why the marker list is *"short and high-signal"* rather
//! than exhaustive — a missed prompt records a password, and no amount of
//! annoyance costs that much.

/// Commands that are asking for a secret whatever is on screen.
pub const SECRET_COMMANDS: &[&str] = &[
    "sudo", "ssh", "scp", "sftp", "rsync",
    "passwd", "su", "doas",
    "ssh-add", "ssh-keygen",
    "gpg", "gpg2",
    "ksshaskpass", "ssh-askpass", "systemd-ask-password",
    "smbclient", "vncpasswd",
];

/// Substrings that mark a prompt line. Deliberately short and high-signal.
pub const PROMPT_MARKERS: &[&str] = &[
    "password", "passphrase", "[sudo]", "verification code",
];

/// What the caller knows about the terminal this tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct TermState<'a> {
    /// True while a full-screen program owns the terminal.
    pub alt_screen: bool,
    /// Foreground process name, basename only.
    pub foreground: Option<&'a str>,
    /// Rendered visible text. Borrowed, and only read when needed.
    pub visible: Option<&'a str>,
}

/// Is the foreground command one that asks for secrets?
///
/// Basename-defensive: the caller already strips the path, but a full path
/// slipping through would silently match nothing and turn the layer off.
pub fn command_is_secretive(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    SECRET_COMMANDS.contains(&base)
}

/// Does the last visible line look like a prompt?
///
/// Scans from the END backward because *"the cursor is at the bottom of the
/// visible area when waiting for input"* — and a terminal's scrollback above it
/// is full of old prompts that have already been answered.
pub fn last_line_is_prompt(visible: &str) -> bool {
    let Some(line) = visible.lines().rev().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    let lower = line.to_lowercase();
    let trimmed = lower.trim_end_matches([' ', ':', ']']);
    PROMPT_MARKERS.iter().any(|m| trimmed.contains(m))
}

/// The whole decision.
pub fn is_password_prompt(state: &TermState) -> bool {
    // First, and before anything expensive: a password prompt is line-based
    // and never appears inside a full-screen TUI. Skipping here avoids both
    // the process read and the grid render on every tick of an editor's
    // startup redraw flood.
    if state.alt_screen { return false; }
    if state.foreground.map(command_is_secretive).unwrap_or(false) { return true; }
    state.visible.map(last_line_is_prompt).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state<'a>(alt: bool, fg: Option<&'a str>, vis: Option<&'a str>) -> TermState<'a> {
        TermState { alt_screen: alt, foreground: fg, visible: vis }
    }

    #[test]
    fn a_secretive_foreground_command_is_enough_on_its_own() {
        // sudo is asking for a secret whatever is on screen — the prompt may
        // not have been drawn yet when the tick fires.
        assert!(is_password_prompt(&state(false, Some("sudo"), Some("$ "))));
        assert!(is_password_prompt(&state(false, Some("gpg"), None)));
        assert!(!is_password_prompt(&state(false, Some("cargo"), Some("$ "))));
    }

    #[test]
    fn a_full_path_still_matches_the_command() {
        // The caller strips it, but a path slipping through would silently
        // match nothing and turn the whole layer off.
        assert!(command_is_secretive("/usr/bin/sudo"));
        assert!(command_is_secretive("/run/current-system/sw/bin/ssh"));
    }

    #[test]
    fn a_hand_written_prompt_is_caught_by_the_second_layer() {
        // A script prompting by hand, or a program not in the list.
        assert!(is_password_prompt(&state(false, Some("bash"),
                Some("$ ./deploy.sh\nEnter password: "))));
        assert!(is_password_prompt(&state(false, None,
                Some("Enter passphrase for key '/home/kd/.ssh/id_ed25519': "))));
    }

    #[test]
    fn an_alt_screen_tui_short_circuits_before_anything_expensive() {
        // The measured fix: "this method runs EVERY read tick... during helix's
        // startup redraw flood the tick fires repeatedly, so we were
        // re-rendering the full grid to a string over and over for a check
        // whose answer is trivially 'no'."
        //
        // Even with a foreground command AND a prompt-shaped line, alt-screen
        // wins — a password prompt is line-based and never appears there.
        assert!(!is_password_prompt(&state(true, Some("sudo"), Some("Password: "))));
    }

    #[test]
    fn the_last_non_empty_line_is_what_counts() {
        // The cursor sits at the bottom when waiting for input, and the
        // scrollback above is full of prompts already answered.
        let scrollback = "Password: \n\
                          Login successful\n\
                          $ ls\n\
                          notes.txt\n";
        assert!(!last_line_is_prompt(scrollback), "an old prompt is not a live one");
        let live = "$ ls\nnotes.txt\n[sudo] password for kd: ";
        assert!(last_line_is_prompt(live));
    }

    #[test]
    fn trailing_blank_lines_do_not_hide_a_live_prompt() {
        // A terminal emits them freely, and stopping at the first line from the
        // end would see an empty string and miss the prompt entirely.
        assert!(last_line_is_prompt("Password: \n\n   \n"));
    }

    #[test]
    fn matching_is_case_insensitive_and_ignores_prompt_punctuation() {
        // Programs print "Password:", "PASSWORD:", "password for kd:" and
        // "[sudo] password for kd: " — one table has to catch all of them.
        for line in ["Password:", "PASSWORD: ", "password for kd:",
                     "[sudo] password for kd: ", "Verification code: "] {
            assert!(last_line_is_prompt(line), "missed: {line:?}");
        }
    }

    #[test]
    fn ordinary_output_is_not_a_prompt() {
        // False positives silently drop utterances from the typing overlay —
        // annoying rather than dangerous, but the list stays high-signal for a
        // reason.
        for line in ["$ cargo test", "test result: ok. 695 passed",
                     "Compiling voiceterm v0.1.0"] {
            assert!(!last_line_is_prompt(line), "false positive: {line:?}");
        }
    }

    #[test]
    fn an_empty_terminal_is_not_a_prompt() {
        assert!(!last_line_is_prompt(""));
        assert!(!last_line_is_prompt("   \n\n"));
        assert!(!is_password_prompt(&state(false, None, None)));
    }

    #[test]
    fn the_word_password_inside_a_sentence_still_trips_it() {
        // Deliberate: "which way to be wrong" is stated in the original, and a
        // missed prompt records a password while a false one drops an
        // utterance. This is the acceptable side.
        assert!(last_line_is_prompt("error: password must be at least 8 characters"));
    }

    #[test]
    fn every_marker_is_lowercase_so_the_comparison_is_symmetric() {
        // The line is lowercased before matching; an uppercase entry would
        // never fire — a silent hole in a security check.
        for m in PROMPT_MARKERS {
            assert_eq!(*m, m.to_lowercase(), "{m} would never match");
        }
        for c in SECRET_COMMANDS {
            assert_eq!(*c, c.to_lowercase(), "{c} would never match a basename");
        }
    }

    #[test]
    fn the_command_list_covers_the_families_that_prompt() {
        // One per family: privilege escalation, remote access, key management,
        // and the askpass helpers a desktop routes through.
        for c in ["sudo", "ssh", "ssh-add", "systemd-ask-password"] {
            assert!(command_is_secretive(c), "{c} missing");
        }
    }
}
