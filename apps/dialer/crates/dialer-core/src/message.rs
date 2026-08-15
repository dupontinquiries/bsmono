//! The message payload protocol — plain text plus a small set of **Linux
//! handoff** payloads: a command *suggestion*, a file offer, a file-view
//! request. Two wire encodings:
//!
//! * **JSON**, for rich transports (Matrix custom event
//!   `org.ecosystem.dialer.payload`) — lossless.
//! * **compact text**, for SMS, using a URI-ish scheme so a phone *without*
//!   this app still shows something legible and a phone *with* it can parse
//!   it back. `run:` / `file:` / `view:` prefixes on otherwise-human lines.
//!
//! ## The command payload is a *suggestion*, never an instruction
//!
//! A received [`Payload::CommandSuggestion`] is data to display, copy, and —
//! only behind an explicit opt-in plus a per-command confirmation — run. The
//! type is named `CommandSuggestion` and not `Command` precisely so no call
//! site can read it as "do this". The decision of whether it may run at all
//! lives in [`crate::command_guard`], out of this module; parsing never
//! implies execution. See that module's threat model.

use serde::{Deserialize, Serialize};

/// One message's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    /// Ordinary text.
    Text { body: String },
    /// A shell command the sender *proposes*. Never run on receipt.
    CommandSuggestion {
        command: String,
        /// Optional working directory hint (display only; the guard decides
        /// whether to honour it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Why the sender sent it — shown in the confirmation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// "I have this file for you." The uri is where to fetch it (an mxc://
    /// for Matrix, or any URL); no bytes travel in the payload.
    FileOffer {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        uri: String,
    },
    /// "Open this local path on your device" — a pointer, not content. The
    /// receiving app resolves it against its own allowed roots (the guard),
    /// so a bare `view:/etc/shadow` resolves to nothing.
    FileView { path: String },
}

impl Payload {
    pub fn text(s: impl Into<String>) -> Payload {
        Payload::Text { body: s.into() }
    }

    /// A one-line human summary for the thread list / notifications.
    pub fn preview(&self) -> String {
        match self {
            Payload::Text { body } => body.lines().next().unwrap_or("").to_string(),
            Payload::CommandSuggestion { command, .. } => format!("⌘ {command}"),
            Payload::FileOffer { name, .. } => format!("📎 {name}"),
            Payload::FileView { path } => format!("👁 {path}"),
        }
    }

    /// Rich, lossless JSON (the Matrix custom-event body).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Payload always serializes")
    }

    pub fn from_json(s: &str) -> Result<Payload, String> {
        serde_json::from_str(s).map_err(|e| format!("payload json: {e}"))
    }

    /// Compact single-line text for SMS. Round-trips through [`parse_sms`]
    /// for handoff payloads; plain text passes through verbatim.
    ///
    /// Encoding is intentionally readable to a non-app recipient:
    ///   `run: <cmd>`                     (cwd/note appended as ` @<cwd>` / ` # <note>`)
    ///   `file: <name> <uri>`             (mime/size dropped — SMS is lossy)
    ///   `view: <path>`
    pub fn to_sms(&self) -> String {
        match self {
            Payload::Text { body } => body.clone(),
            Payload::CommandSuggestion { command, cwd, note } => {
                let mut s = format!("run: {command}");
                if let Some(c) = cwd {
                    s.push_str(&format!(" @{c}"));
                }
                if let Some(n) = note {
                    s.push_str(&format!(" # {n}"));
                }
                s
            }
            Payload::FileOffer { name, uri, .. } => format!("file: {name} {uri}"),
            Payload::FileView { path } => format!("view: {path}"),
        }
    }
}

/// Parse an inbound SMS body into a payload. Unknown/plain text becomes
/// [`Payload::Text`] — the prefixes are the *only* thing that upgrades a
/// message to a handoff, so a normal SMS is never misread as a command.
pub fn parse_sms(body: &str) -> Payload {
    let trimmed = body.trim_start();
    if let Some(rest) = trimmed.strip_prefix("run:") {
        // Split off ` # note` first (from the right), then ` @cwd`.
        let (head, note) = split_last(rest, " # ");
        let (cmd, cwd) = split_last(head, " @");
        let command = cmd.trim().to_string();
        if command.is_empty() {
            return Payload::text(body);
        }
        return Payload::CommandSuggestion {
            command,
            cwd: cwd.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            note: note.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        };
    }
    if let Some(rest) = trimmed.strip_prefix("file:") {
        let rest = rest.trim();
        // Last whitespace-separated token is the uri; the rest is the name.
        if let Some(idx) = rest.rfind(char::is_whitespace) {
            let (name, uri) = rest.split_at(idx);
            let name = name.trim();
            let uri = uri.trim();
            if !name.is_empty() && !uri.is_empty() {
                return Payload::FileOffer {
                    name: name.to_string(),
                    mime: None,
                    size: None,
                    uri: uri.to_string(),
                };
            }
        }
        return Payload::text(body);
    }
    if let Some(rest) = trimmed.strip_prefix("view:") {
        let path = rest.trim();
        if !path.is_empty() {
            return Payload::FileView { path: path.to_string() };
        }
    }
    Payload::text(body)
}

/// Split on the last occurrence of `sep`, returning (head, Some(tail)) or
/// (whole, None). Used so a note or cwd containing the *other* separator does
/// not get chopped in the wrong place.
fn split_last<'a>(s: &'a str, sep: &str) -> (&'a str, Option<&'a str>) {
    match s.rfind(sep) {
        Some(i) => (&s[..i], Some(&s[i + sep.len()..])),
        None => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_never_upgraded_to_a_command() {
        // The whole safety of the SMS path: only the prefix upgrades.
        assert_eq!(parse_sms("let's run: the marathon"), Payload::text("let's run: the marathon"));
        assert_eq!(parse_sms("hello there"), Payload::text("hello there"));
    }

    #[test]
    fn command_round_trips_through_sms_with_cwd_and_note() {
        let p = Payload::CommandSuggestion {
            command: "git pull --rebase".into(),
            cwd: Some("~/monorepo".into()),
            note: Some("before you push".into()),
        };
        let sms = p.to_sms();
        assert_eq!(sms, "run: git pull --rebase @~/monorepo # before you push");
        assert_eq!(parse_sms(&sms), p);
    }

    #[test]
    fn command_without_extras_parses() {
        assert_eq!(
            parse_sms("run: ls -la"),
            Payload::CommandSuggestion { command: "ls -la".into(), cwd: None, note: None }
        );
        // an empty command is not a command
        assert_eq!(parse_sms("run:   "), Payload::text("run:   "));
    }

    #[test]
    fn file_offer_and_view_round_trip() {
        let offer = Payload::FileOffer {
            name: "log.txt".into(),
            mime: None,
            size: None,
            uri: "mxc://x.org/abc".into(),
        };
        assert_eq!(offer.to_sms(), "file: log.txt mxc://x.org/abc");
        assert_eq!(parse_sms(&offer.to_sms()), offer);
        assert_eq!(parse_sms("view: /var/log/syslog"),
                   Payload::FileView { path: "/var/log/syslog".into() });
    }

    #[test]
    fn json_is_lossless_where_sms_is_lossy() {
        let p = Payload::FileOffer {
            name: "shot.png".into(),
            mime: Some("image/png".into()),
            size: Some(20_480),
            uri: "https://x/y".into(),
        };
        let back = Payload::from_json(&p.to_json()).unwrap();
        assert_eq!(back, p, "json keeps mime+size that sms drops");
    }

    #[test]
    fn previews_are_distinct_and_single_line() {
        assert_eq!(Payload::text("a\nb").preview(), "a");
        assert!(Payload::CommandSuggestion { command: "x".into(), cwd: None, note: None }
            .preview()
            .starts_with('⌘'));
    }
}
