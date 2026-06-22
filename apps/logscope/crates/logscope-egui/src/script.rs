//! script.rs — the headless command-script format the CLI runs to build a test suite. One step per line:
//!
//! ```text
//! # a comment
//! > write a function to reverse a Vec     (or:  say write a function …)
//! :3ca                                    a colon command — same language as the GUI
//! :best
//! :export md
//! ```
//!
//! Blank lines and `#` comments are skipped; a `>`/`say ` prefix (or a bare line) is a user turn; a `:`
//! prefix is a command (parsed by `vimmode::parse_command`). Pure parser; the runner executes the steps.

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Say(String), // send a user turn (model replies)
    Cmd(String), // a `:command` line, parsed + run by the runner
}

pub fn parse_script(text: &str) -> Vec<Step> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Some(rest) = line.strip_prefix('>') {
            out.push(Step::Say(rest.trim().to_string()));
        } else if let Some(rest) = line.strip_prefix("say ") {
            out.push(Step::Say(rest.trim().to_string()));
        } else if line.starts_with(':') {
            // command lines may carry a trailing `# comment` (e.g. `:goto 13   # pickup r133b`).
            // strip it (respecting quotes, so a `#` inside a :fa template survives).
            out.push(Step::Cmd(strip_inline_comment(line).to_string()));
        } else {
            out.push(Step::Say(line.to_string())); // a bare line is a user message (kept verbatim — may contain #)
        }
    }
    out
}

/// Truncate a line at the first `#` that is not inside a single- or double-quoted span.
fn strip_inline_comment(line: &str) -> &str {
    let (mut in_s, mut in_d) = (false, false);
    for (i, c) in line.char_indices() {
        match c {
            '\'' if !in_d => in_s = !in_s,
            '"' if !in_s => in_d = !in_d,
            '#' if !in_s && !in_d => return line[..i].trim_end(),
            _ => {}
        }
    }
    line.trim_end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steps_and_skips_noise() {
        let s = "# suite: sort\n\n> reverse a Vec<i32>\nsay now make it generic\n:3ca\n:best\n:export md\nplain user line\n";
        let steps = parse_script(s);
        assert_eq!(steps, vec![
            Step::Say("reverse a Vec<i32>".into()),
            Step::Say("now make it generic".into()),
            Step::Cmd(":3ca".into()),
            Step::Cmd(":best".into()),
            Step::Cmd(":export md".into()),
            Step::Say("plain user line".into()),
        ]);
    }

    #[test]
    fn strips_inline_comments_on_command_lines_only() {
        let s = "> Build r130 — player bending.\n:goto 13   # pickup: patch before r133b\n:2fa 6 'harden #edge cases# now: {lastmsg}'   # trailing note\n";
        let steps = parse_script(s);
        assert_eq!(steps[0], Step::Say("Build r130 — player bending.".into()), "user text keeps any #");
        assert_eq!(steps[1], Step::Cmd(":goto 13".into()), "goto comment stripped");
        assert_eq!(steps[2], Step::Cmd(":2fa 6 'harden #edge cases# now: {lastmsg}'".into()), "quoted # survives, trailing note stripped");
    }

    #[test]
    fn empty_script_is_empty() {
        assert!(parse_script("\n# only comments\n\n").is_empty());
    }
}
