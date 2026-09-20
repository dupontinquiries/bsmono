//! Shell history as a data source. Two uses: grep, and recovering how a binary
//! was last invoked (arguments, env prefix, cwd-relative path) so a debug run can
//! start from the command line that actually reproduced the problem.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub shell: Shell,
    /// Unix seconds, when the shell recorded one.
    pub when: Option<u64>,
    pub cmd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub when: Option<u64>,
    pub original: String,
}

pub fn default_files() -> Vec<(Shell, PathBuf)> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let mut v = vec![
        (Shell::Bash, home.join(".bash_history")),
        (Shell::Zsh, home.join(".zsh_history")),
        (Shell::Zsh, home.join(".zhistory")),
        (Shell::Fish, data.join("fish/fish_history")),
        // busybox ash, the default login shell on postmarketOS.
        (Shell::Bash, home.join(".ash_history")),
    ];
    if let Some(h) = std::env::var_os("HISTFILE").map(PathBuf::from) {
        let shell = if h.to_string_lossy().contains("zsh") { Shell::Zsh } else { Shell::Bash };
        if !v.iter().any(|(_, p)| *p == h) {
            v.push((shell, h));
        }
    }
    v
}

pub fn load_default() -> Vec<Entry> {
    default_files().iter().flat_map(|(s, p)| load(*s, p)).collect()
}

pub fn load(shell: Shell, path: &Path) -> Vec<Entry> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    match shell {
        Shell::Bash => parse_bash(&String::from_utf8_lossy(&bytes)),
        Shell::Zsh => parse_zsh(&String::from_utf8_lossy(&unmetafy(&bytes))),
        Shell::Fish => parse_fish(&String::from_utf8_lossy(&bytes)),
    }
}

/// zsh writes its history "metafied": any byte in 0x83..=0xA2 (and 0x00) is
/// stored as 0x83 followed by the byte XOR 0x20. Non-ASCII paths are unreadable
/// without undoing this.
fn unmetafy(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut it = b.iter().copied();
    while let Some(c) = it.next() {
        if c == 0x83 {
            if let Some(n) = it.next() {
                out.push(n ^ 0x20);
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn parse_bash(s: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut when = None;
    for line in s.lines() {
        // With HISTTIMEFORMAT set, each entry is preceded by `#<epoch>`.
        if let Some(ts) = line.strip_prefix('#').and_then(|t| t.parse::<u64>().ok()) {
            when = Some(ts);
        } else if !line.trim().is_empty() {
            out.push(Entry { shell: Shell::Bash, when: when.take(), cmd: line.to_string() });
        }
    }
    out
}

pub fn parse_zsh(s: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut continued = false;
    for line in s.lines() {
        let cont_next = line.ends_with('\\');
        let body = line.strip_suffix('\\').unwrap_or(line);
        if continued {
            if let Some(last) = out.last_mut() {
                last.cmd.push('\n');
                last.cmd.push_str(body);
            }
        } else if let Some((meta, cmd)) = line.strip_prefix(": ").and_then(|r| r.split_once(';')) {
            // EXTENDED_HISTORY: `: <epoch>:<elapsed>;<command>`
            let when = meta.split(':').next().and_then(|t| t.trim().parse().ok());
            out.push(Entry { shell: Shell::Zsh, when, cmd: cmd.strip_suffix('\\').unwrap_or(cmd).to_string() });
        } else if !body.trim().is_empty() {
            out.push(Entry { shell: Shell::Zsh, when: None, cmd: body.to_string() });
        }
        continued = cont_next;
    }
    out
}

pub fn parse_fish(s: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for line in s.lines() {
        if let Some(cmd) = line.strip_prefix("- cmd: ") {
            out.push(Entry { shell: Shell::Fish, when: None, cmd: cmd.replace("\\n", "\n").replace("\\\\", "\\") });
        } else if let Some(ts) = line.strip_prefix("  when: ") {
            if let Some(last) = out.last_mut() {
                last.when = ts.trim().parse().ok();
            }
        }
    }
    out
}

/// Case-insensitive substring match on every whitespace-separated term, newest
/// last. Deliberately not regex: on a phone keyboard, `cargo feat` is the query.
pub fn grep<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    entries
        .iter()
        .filter(|e| {
            let c = e.cmd.to_lowercase();
            terms.iter().all(|t| c.contains(t))
        })
        .collect()
}

/// Split a command line into words. Handles quotes and backslashes; stops at the
/// first control operator, so `./app --x | less` yields the `./app --x` part.
pub fn words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                have = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                have = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => cur.extend(chars.next()),
                        q => cur.push(q),
                    }
                }
            }
            '\\' => {
                have = true;
                cur.extend(chars.next());
            }
            '|' | ';' | '&' | '>' | '<' | '\n' => break,
            c if c.is_whitespace() => {
                if std::mem::take(&mut have) {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => {
                have = true;
                cur.push(c);
            }
        }
    }
    if have {
        out.push(cur);
    }
    out
}

/// Interpret one history line as "this ran a program we might debug".
pub fn invocation(e: &Entry) -> Option<Invocation> {
    let mut w = words(&e.cmd).into_iter().peekable();
    let mut env = Vec::new();
    while let Some((k, v)) = w.peek().and_then(|t| t.split_once('=')) {
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            break;
        }
        env.push((k.to_string(), v.to_string()));
        w.next();
    }
    let mut rest: Vec<String> = w.collect();
    let inv = |program: String, args: Vec<String>, env| Some(Invocation { program, args, env, when: e.when, original: e.cmd.clone() });
    let first = rest.first()?.clone();
    match first.as_str() {
        // `cargo run [--bin N | --example N] [flags] -- args`
        "cargo" if rest.get(1).map(String::as_str) == Some("run") => {
            let split = rest.iter().position(|a| a == "--").unwrap_or(rest.len());
            let args = rest.split_off((split + 1).min(rest.len()));
            let name = rest.iter().position(|a| a == "--bin" || a == "--example").and_then(|i| rest.get(i + 1)).cloned();
            inv(name.map_or_else(|| "cargo:default".to_string(), |n| format!("cargo:{n}")), args, env)
        }
        // `gdb --args prog a b`, `rust-gdb --args prog a b`, `valgrind prog a b`, `strace prog`
        "gdb" | "rust-gdb" | "lldb" | "rust-lldb" | "valgrind" | "strace" | "ltrace" | "time" => {
            let at = rest.iter().position(|a| a == "--args" || a == "--").map(|i| i + 1).or_else(|| rest.iter().skip(1).position(|a| !a.starts_with('-')).map(|i| i + 1))?;
            let mut tail = rest.split_off(at);
            if tail.is_empty() {
                return None;
            }
            let program = tail.remove(0);
            inv(program, tail, env)
        }
        p if p.contains('/') => {
            rest.remove(0);
            inv(first, rest, env)
        }
        _ => None,
    }
}

/// Most recent invocations whose program matches `binary` by file name, or by
/// cargo bin name.
pub fn invocations_of(entries: &[Entry], binary: &Path) -> Vec<Invocation> {
    let Some(name) = binary.file_name().and_then(|n| n.to_str()) else { return Vec::new() };
    let mut v: Vec<Invocation> = entries
        .iter()
        .filter_map(invocation)
        .filter(|i| match i.program.strip_prefix("cargo:") {
            // A bare `cargo run` names no binary. Treat it as matching anything
            // cargo built; wrong for multi-bin workspaces, where the --bin form
            // will be in history instead.
            Some("default") => binary.components().any(|c| c.as_os_str() == "target"),
            Some(bin) => bin == name,
            None => Path::new(&i.program).file_name().and_then(|n| n.to_str()) == Some(name),
        })
        .collect();
    // Newest first. Files are in chronological order; timestamps, where the
    // shell recorded them, settle order across files. Undated entries sort last.
    v.reverse();
    v.sort_by_key(|i| std::cmp::Reverse(i.when.unwrap_or(0)));
    v
}
