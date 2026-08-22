//! The channel log: append-only JSONL, one file per (network, channel).
//!
//! Shaped after `dialer-core`'s `conversation.rs` — append-only, last-record
//! wins for a given key, tolerant of a corrupt tail — because the same
//! properties matter: a crash mid-write must lose one line, not the archive.
//!
//! ## Provenance is a recorded field, not a convention
//!
//! Every line carries [`Line::origin`]. There are exactly two legal values and
//! only one of them can be produced by this crate:
//!
//! * [`Origin::Participated`] — written by [`crate::session`] while this
//!   client was actually joined to the channel. The only kind [`append`]
//!   will write.
//! * [`Origin::Imported`] — anything else, which a *human* placed here by
//!   hand and takes responsibility for.
//!
//! The distinction exists because [`crate::sources`] refuses to feed a model
//! anything that is not `Participated`. Writing the provenance into each line
//! rather than trusting a directory convention means a mixed file cannot
//! launder imported text into a consented source: the check is per line.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use net_kit::json;

/// How a line got into the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Captured live by this client, from a channel it had joined.
    Participated,
    /// Placed here by a human from some other source. Never written by this
    /// crate; only ever read back.
    Imported,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Participated => "participated",
            Origin::Imported => "imported",
        }
    }
    /// Unknown strings become `Imported` — the restrictive value. A log line
    /// with a provenance we do not recognise must not be treated as ours.
    pub fn parse(s: &str) -> Origin {
        if s == "participated" { Origin::Participated } else { Origin::Imported }
    }
}

/// One logged line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Unix seconds. From the IRCv3 `time` tag when the server sent one,
    /// otherwise arrival time.
    pub ts: u64,
    /// The server's `time` tag verbatim, when there was one. Kept alongside
    /// `ts` rather than only parsed into it so a reader can tell a
    /// server-stamped line from a locally-stamped one.
    pub server_time: Option<String>,
    pub network: String,
    pub channel: String,
    pub nick: String,
    pub text: String,
    pub notice: bool,
    pub action: bool,
    pub origin: Origin,
}

impl Line {
    fn to_json(&self) -> String {
        let mut s = String::with_capacity(self.text.len() + 160);
        s.push('{');
        s.push_str(&format!(r#""ts":{},"#, self.ts));
        if let Some(st) = &self.server_time {
            s.push_str(&format!(r#""server_time":"{}","#, json::escape(st)));
        }
        s.push_str(&format!(r#""network":"{}","#, json::escape(&self.network)));
        s.push_str(&format!(r#""channel":"{}","#, json::escape(&self.channel)));
        s.push_str(&format!(r#""nick":"{}","#, json::escape(&self.nick)));
        s.push_str(&format!(r#""text":"{}","#, json::escape(&self.text)));
        s.push_str(&format!(r#""notice":{},"#, self.notice));
        s.push_str(&format!(r#""action":{},"#, self.action));
        s.push_str(&format!(r#""origin":"{}""#, self.origin.as_str()));
        s.push('}');
        s
    }

    /// Parse one JSONL record. `None` for a line that is not a usable record —
    /// a truncated tail from a crashed write is skipped, not fatal.
    pub fn from_json(line: &str) -> Option<Line> {
        let t = line.trim();
        if !t.starts_with('{') || !t.ends_with('}') {
            return None;
        }
        let text = json::find_string_field(t, "text")?;
        let nick = json::find_string_field(t, "nick")?;
        let channel = json::find_string_field(t, "channel")?;
        let network = json::find_string_field(t, "network").unwrap_or_default();
        Some(Line {
            ts: num_field(t, "ts").unwrap_or(0),
            server_time: json::find_string_field(t, "server_time"),
            network,
            channel,
            nick,
            text,
            notice: bool_field(t, "notice"),
            action: bool_field(t, "action"),
            // A record with no origin field predates the field or was
            // hand-written: treat as Imported, the restrictive value.
            origin: json::find_string_field(t, "origin")
                .map(|s| Origin::parse(&s))
                .unwrap_or(Origin::Imported),
        })
    }
}

fn num_field(json_str: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\":");
    let i = json_str.find(&pat)? + pat.len();
    let rest = json_str[i..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn bool_field(json_str: &str, key: &str) -> bool {
    let pat = format!("\"{key}\":");
    match json_str.find(&pat) {
        Some(i) => json_str[i + pat.len()..].trim_start().starts_with("true"),
        None => false,
    }
}

/// Filesystem-safe file stem for a channel. `#asahi-dev` on OFTC becomes
/// `oftc__asahi-dev`. Anything outside `[a-z0-9._-]` becomes `_`, so a channel
/// name can never walk out of the log directory or collide with a path
/// separator.
pub fn log_stem(network: &str, channel: &str) -> String {
    // `.` is kept because network names contain it (`irc.oftc.net`), but a
    // RUN of dots is collapsed: otherwise `../../etc/passwd` cleans to
    // `.._.._etc_passwd`, which is a legal filename but still reads as a
    // traversal attempt to anything that later splits on `_`.
    let clean = |s: &str| -> String {
        let mapped: String = crate::proto::lower(s)
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
            .collect();
        let mut out = String::with_capacity(mapped.len());
        let mut prev_dot = false;
        for c in mapped.chars() {
            if c == '.' {
                if prev_dot { out.push('_'); } else { out.push('.'); }
                prev_dot = true;
            } else {
                out.push(c);
                prev_dot = false;
            }
        }
        out
    };
    format!("{}_{}", clean(network), clean(channel))
}

/// Default log directory: `$XDG_DATA_HOME/irc-kit/logs` (or `~/.local/share/…`).
pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("irc-kit").join("logs")
}

/// Full path for a channel's log inside `dir`.
pub fn log_path(dir: &Path, network: &str, channel: &str) -> PathBuf {
    dir.join(format!("{}.jsonl", log_stem(network, channel)))
}

/// Append one line. Always writes `origin: participated` regardless of what
/// the caller put in the struct — this function is only reachable from the
/// live session, and hardcoding it here means no call site can mislabel a
/// line as self-captured by mistake.
pub fn append(dir: &Path, line: &Line) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = log_path(dir, &line.network, &line.channel);
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let rec = Line { origin: Origin::Participated, ..line.clone() };
    writeln!(f, "{}", rec.to_json()).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Read a channel log back. Unparseable lines are skipped and counted; the
/// count is returned so a caller can say "3 damaged lines" instead of
/// silently showing a short archive.
pub fn read(path: &Path) -> Result<(Vec<Line>, usize), String> {
    let f = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for l in BufReader::new(f).lines() {
        let l = l.map_err(|e| format!("read {}: {e}", path.display()))?;
        if l.trim().is_empty() {
            continue;
        }
        match Line::from_json(&l) {
            Some(rec) => out.push(rec),
            None => skipped += 1,
        }
    }
    Ok((out, skipped))
}

/// Every channel log present in `dir`, as `(path, stem)`.
pub fn list(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                out.push((p.clone(), stem.to_string()));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("irc-kit-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn line(text: &str) -> Line {
        Line {
            ts: 1787427353,
            server_time: Some("2026-08-22T15:54:22.000Z".into()),
            network: "oftc".into(),
            channel: "#asahi-dev".into(),
            nick: "tester".into(),
            text: text.into(),
            notice: false,
            action: false,
            origin: Origin::Participated,
        }
    }

    #[test]
    fn round_trips_through_jsonl() {
        let l = line("hello");
        let back = Line::from_json(&l.to_json()).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn quotes_newlines_and_control_chars_survive() {
        let l = line("he said \"hi\"\nthen \\left\ttabbed\u{1}");
        let back = Line::from_json(&l.to_json()).unwrap();
        assert_eq!(back.text, l.text);
    }

    #[test]
    fn append_forces_participated_even_if_the_caller_says_otherwise() {
        let d = tmpdir("origin");
        let mut l = line("x");
        l.origin = Origin::Imported; // a caller lying, or just wrong
        append(&d, &l).unwrap();
        let (got, _) = read(&log_path(&d, "oftc", "#asahi-dev")).unwrap();
        assert_eq!(got[0].origin, Origin::Participated);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_record_with_no_origin_reads_back_as_imported() {
        // r##"…"## because the payload contains `"#` (a channel name), which
        // would terminate a single-hash raw string.
        let j = r##"{"ts":1,"network":"n","channel":"#c","nick":"a","text":"t","notice":false,"action":false}"##;
        assert_eq!(Line::from_json(j).unwrap().origin, Origin::Imported);
    }

    #[test]
    fn an_unknown_origin_string_reads_back_as_imported() {
        assert_eq!(Origin::parse("participated_honest"), Origin::Imported);
        assert_eq!(Origin::parse(""), Origin::Imported);
        assert_eq!(Origin::parse("participated"), Origin::Participated);
    }

    #[test]
    fn a_truncated_tail_is_skipped_and_counted_not_fatal() {
        let d = tmpdir("trunc");
        append(&d, &line("one")).unwrap();
        append(&d, &line("two")).unwrap();
        let p = log_path(&d, "oftc", "#asahi-dev");
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "{{\"ts\":3,\"text\":\"cut off her").unwrap();
        drop(f);

        let (lines, skipped) = read(&p).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(skipped, 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn channel_names_cannot_escape_the_log_directory() {
        assert_eq!(log_stem("oftc", "#asahi-dev"), "oftc__asahi-dev");
        for evil in ["../../etc/passwd", "..", ".", "/absolute", "a\\b"] {
            let stem = log_stem("oftc", evil);
            // The property that matters: the stem is exactly one path
            // component, and not a relative-directory name.
            let p = Path::new(&stem);
            assert_eq!(p.components().count(), 1, "{evil} -> {stem}");
            assert!(!stem.contains(".."), "{evil} -> {stem}");
            let full = log_path(Path::new("/logs"), "oftc", evil);
            assert!(full.starts_with("/logs"), "{evil} -> {}", full.display());
            assert_eq!(full.components().count(), 3, "{evil} -> {}", full.display());
        }
    }

    #[test]
    fn dots_in_a_network_name_survive_because_they_are_legitimate() {
        assert!(log_stem("irc.oftc.net", "#c").starts_with("irc.oftc.net"));
    }

    #[test]
    fn stems_are_casemapped_so_one_channel_is_one_file() {
        assert_eq!(log_stem("OFTC", "#Asahi-Dev"), log_stem("oftc", "#asahi-dev"));
    }

    #[test]
    fn list_finds_written_logs() {
        let d = tmpdir("list");
        append(&d, &line("a")).unwrap();
        let mut other = line("b");
        other.channel = "#asahi".into();
        append(&d, &other).unwrap();
        let found = list(&d);
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|(_, s)| s == "oftc__asahi-dev"));
        let _ = fs::remove_dir_all(&d);
    }
}
