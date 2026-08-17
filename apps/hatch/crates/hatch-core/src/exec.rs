//! exec — turning an `Exec=` line into a detached child process.
//!
//! Two spec details launchers routinely get wrong, both handled here:
//!
//! * **Quoting.** `Exec=env "FOO=a b" app` is three words, not four. The spec's
//!   quoting is double-quotes with backslash escapes; single quotes have no
//!   special meaning.
//! * **Field codes.** `%f %F %u %U` expand to the files/URLs being opened — a
//!   launcher opens nothing, so they expand to NOTHING (the token disappears,
//!   it does not become an empty arg — `mpv ""` starts mpv complaining about a
//!   file called ""). `%%` is a literal `%`; deprecated codes are dropped.
//!
//! The child is detached: its own process group, stdio to /dev/null. A
//! launcher that leaves children in its own group kills every app it ever
//! started the day it exits.

use std::io;
use std::process::{Command, Stdio};

/// Split an Exec value into argv, expanding field codes for the "launched
/// with no files" case.
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = exec.chars().peekable();
    let mut cur_started = false; // distinguishes "" (a real empty arg) from nothing

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur_started = true;
            }
            '\\' if in_quotes => {
                // Inside quotes: \" \` \$ \\ escape; anything else is literal.
                if let Some(&n) = chars.peek() {
                    chars.next();
                    cur.push(n);
                    cur_started = true;
                }
            }
            '%' => {
                match chars.next() {
                    Some('%') => {
                        cur.push('%');
                        cur_started = true;
                    }
                    // File/URL codes expand to nothing here; if the code was
                    // the whole token, the token disappears entirely.
                    Some('f') | Some('F') | Some('u') | Some('U') => {}
                    // Deprecated / launcher-irrelevant codes: drop. (%i would
                    // become `--icon <Icon>`; no target we run needs it.)
                    Some(_) => {}
                    None => {}
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if cur_started && !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                }
                // A token that was ONLY a field code vanishes: reset the flag
                // without pushing.
                cur.clear();
                cur_started = false;
            }
            c => {
                cur.push(c);
                cur_started = true;
            }
        }
    }
    if cur_started && !cur.is_empty() {
        args.push(cur);
    }
    args
}

/// Spawn argv detached. `terminal_wrap` is the command to prefix when the
/// entry demands a terminal (e.g. `["konsole", "-e"]`); pass `&[]` to run it
/// bare anyway.
pub fn spawn(argv: &[String], terminal: bool, terminal_wrap: &[String]) -> io::Result<u32> {
    spawn_in(argv, terminal, terminal_wrap, None)
}

/// As [`spawn`], in `workdir` when the entry declares `Path=`. A missing or
/// unreadable working directory would make `spawn` fail outright, which would
/// turn a cosmetic packaging error into an app that cannot be launched at
/// all — so a directory that isn't there is ignored rather than fatal.
pub fn spawn_in(
    argv: &[String],
    terminal: bool,
    terminal_wrap: &[String],
    workdir: Option<&std::path::Path>,
) -> io::Result<u32> {
    if argv.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty Exec"));
    }
    let mut full: Vec<&str> = Vec::new();
    if terminal {
        full.extend(terminal_wrap.iter().map(String::as_str));
    }
    full.extend(argv.iter().map(String::as_str));

    let mut cmd = Command::new(full[0]);
    if let Some(dir) = workdir.filter(|d| d.is_dir()) {
        cmd.current_dir(dir);
    }
    cmd.args(&full[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: the launcher's exit must not SIGHUP its apps.
        cmd.process_group(0);
    }
    let child = cmd.spawn()?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_split_on_whitespace() {
        assert_eq!(split_exec("firefox --new-window"), vec!["firefox", "--new-window"]);
    }

    #[test]
    fn quoted_arguments_keep_their_spaces() {
        assert_eq!(
            split_exec(r#"env "FOO=a b" app"#),
            vec!["env", "FOO=a b", "app"]
        );
    }

    #[test]
    fn file_field_codes_vanish_rather_than_become_empty_args() {
        // `mpv %U` launched bare must be `mpv`, not `mpv ""`.
        assert_eq!(split_exec("mpv %U"), vec!["mpv"]);
        assert_eq!(split_exec("app %f --flag %F"), vec!["app", "--flag"]);
    }

    #[test]
    fn double_percent_is_a_literal_percent() {
        assert_eq!(split_exec("app --gain=50%%"), vec!["app", "--gain=50%"]);
    }

    #[test]
    fn escaped_quote_inside_quotes_is_literal() {
        assert_eq!(split_exec(r#"sh -c "echo \"hi\"""#), vec!["sh", "-c", r#"echo "hi""#]);
    }

    #[test]
    fn spawning_an_empty_argv_is_an_error_not_a_panic() {
        assert!(spawn(&[], false, &[]).is_err());
    }

    #[test]
    fn a_real_spawn_detaches_and_returns_a_pid() {
        // `true` exists everywhere we run tests.
        let pid = spawn(&["true".to_string()], false, &[]).unwrap();
        assert!(pid > 0);
    }

    #[test]
    fn a_declared_working_directory_is_used_and_a_missing_one_is_survivable() {
        let dir = std::env::temp_dir().join(format!("hatch-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("proof");

        // `sh -c 'touch proof'` only lands in `dir` if cwd was honoured.
        let argv: Vec<String> = ["sh", "-c", "touch proof"].iter().map(|s| s.to_string()).collect();
        assert!(spawn_in(&argv, false, &[], Some(&dir)).unwrap() > 0);
        for _ in 0..50 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(marker.exists(), "the child ran in the declared directory");

        // A packaging error (Path= pointing nowhere) must not make an app
        // unlaunchable; it falls back to the launcher's own cwd.
        let gone = dir.join("does-not-exist");
        assert!(spawn_in(&["true".to_string()], false, &[], Some(&gone)).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
