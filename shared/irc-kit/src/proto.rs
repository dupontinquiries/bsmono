//! Pure IRCv3 message parse/format. No IO, no allocation of a connection, no
//! clock — a `&str` in, a [`Message`] out, and back.
//!
//! The grammar, in the order the fields appear on the wire:
//!
//! ```text
//!   [ "@" tags SPACE ] [ ":" prefix SPACE ] command *( SPACE param ) [ SPACE ":" trailing ]
//! ```
//!
//! Three things make this worth a tested module rather than a `split(' ')`:
//!
//! * **The trailing parameter may contain spaces**, and is the only one that
//!   may. A naive split turns one `PRIVMSG` body into eight params.
//! * **Tag values use their own escape alphabet** (`\:` = `;`, `\s` = space,
//!   `\\`, `\r`, `\n`), which is *not* percent-encoding and *not* C escapes.
//!   Getting this wrong silently corrupts `time=` tags, which is exactly the
//!   field a log wants to be right.
//! * **A trailing param can be empty and still present** — `PRIVMSG #x :` is
//!   a real (if rude) message, and is distinguishable from `PRIVMSG #x`.
//!   Losing that distinction breaks round-tripping.
//!
//! Parsing is total: it never panics and never returns `Err` for a merely
//! weird line, because a client that disconnects on one malformed server line
//! is worse than one that ignores it. Only a line with no command at all is
//! rejected.

/// A parsed IRC protocol line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    /// IRCv3 message tags, in wire order. Kept as a Vec rather than a map:
    /// order is preserved for round-tripping, and there are never enough tags
    /// for lookup cost to matter.
    pub tags: Vec<(String, String)>,
    /// The source, without the leading `:` — `nick!user@host` or a server name.
    pub prefix: Option<String>,
    /// `PRIVMSG`, `JOIN`, `001`, … Upper-cased on parse so match arms can be
    /// written one way. Numerics are left as their three digits.
    pub command: String,
    /// Middle params followed by the trailing param, if any.
    pub params: Vec<String>,
    /// Was the last param written as a `:`-prefixed trailing param?
    ///
    /// This is a real bit on the wire and guessing it from the content does
    /// not work: `PING :12345` and `PING 12345` parse identically but are not
    /// the same bytes, and a round-trip that silently drops the colon is a
    /// round-trip that changes what a picky server sees. Servers accept both
    /// spellings, but every real client sends `PONG :token`, so the safe move
    /// is to preserve what we were given and let senders state their intent.
    pub trailing: bool,
}

impl Message {
    /// A message with no tags and no prefix, last param written plainly.
    pub fn new(command: &str, params: &[&str]) -> Message {
        Message {
            tags: Vec::new(),
            prefix: None,
            command: command.to_ascii_uppercase(),
            params: params.iter().map(|p| p.to_string()).collect(),
            trailing: false,
        }
    }

    /// Same, but the last param is written as a `:`-prefixed trailing param.
    /// Use this for `PONG`, `PRIVMSG`, `QUIT`, `PART` reasons and `CAP REQ` —
    /// the places where every other client puts a colon and where a bare
    /// param occasionally meets a strict server or services bot.
    pub fn trailing(command: &str, params: &[&str]) -> Message {
        Message { trailing: true, ..Message::new(command, params) }
    }

    /// The nick part of the prefix (`nick!user@host` → `nick`), if there is one.
    /// A server-name prefix has no `!`, and is not a nick — returns `None` for
    /// it rather than the whole hostname, so `Event::Message.from` is never a
    /// server.
    pub fn nick(&self) -> Option<&str> {
        let p = self.prefix.as_deref()?;
        let bang = p.find('!')?;
        Some(&p[..bang])
    }

    /// Param at `i`, or `""`. Params are addressed positionally all over the
    /// protocol and the absent case is never interesting.
    pub fn param(&self, i: usize) -> &str {
        self.params.get(i).map(|s| s.as_str()).unwrap_or("")
    }

    /// Value of an IRCv3 tag by key.
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Unescape an IRCv3 tag value. The alphabet is fixed by the spec and is not
/// any other escape scheme: `\:` → `;`, `\s` → space, `\\` → `\`, `\r`, `\n`.
/// An unrecognised escape drops the backslash and keeps the character, which
/// is what the spec says to do rather than erroring.
pub fn unescape_tag(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {} // trailing lone backslash: spec says drop it
        }
    }
    out
}

/// Escape a tag value for the wire. Inverse of [`unescape_tag`].
pub fn escape_tag(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            ';' => out.push_str("\\:"),
            ' ' => out.push_str("\\s"),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Parse one protocol line (without its CRLF). Returns `None` only when there
/// is no command — every other malformation is tolerated.
pub fn parse(line: &str) -> Option<Message> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    let mut msg = Message::default();

    // @tags
    if let Some(stripped) = rest.strip_prefix('@') {
        let (tagpart, after) = match stripped.find(' ') {
            Some(i) => (&stripped[..i], &stripped[i + 1..]),
            None => (stripped, ""),
        };
        for t in tagpart.split(';').filter(|t| !t.is_empty()) {
            let (k, v) = match t.find('=') {
                Some(i) => (&t[..i], unescape_tag(&t[i + 1..])),
                // A valueless tag is the empty string, not absent — the spec
                // treats `foo` and `foo=` identically.
                None => (t, String::new()),
            };
            msg.tags.push((k.to_string(), v));
        }
        rest = after.trim_start_matches(' ');
    }

    // :prefix
    if let Some(stripped) = rest.strip_prefix(':') {
        let (pfx, after) = match stripped.find(' ') {
            Some(i) => (&stripped[..i], &stripped[i + 1..]),
            None => (stripped, ""),
        };
        msg.prefix = Some(pfx.to_string());
        rest = after.trim_start_matches(' ');
    }

    if rest.is_empty() {
        return None;
    }

    // command
    let (cmd, after) = match rest.find(' ') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    if cmd.is_empty() {
        return None;
    }
    msg.command = cmd.to_ascii_uppercase();
    rest = after;

    // params, stopping at the trailing
    loop {
        let r = rest.trim_start_matches(' ');
        if r.is_empty() {
            break;
        }
        if let Some(trailing) = r.strip_prefix(':') {
            // Everything after the colon is one param, spaces and all —
            // including the empty string.
            msg.params.push(trailing.to_string());
            msg.trailing = true;
            break;
        }
        match r.find(' ') {
            Some(i) => {
                msg.params.push(r[..i].to_string());
                rest = &r[i + 1..];
            }
            None => {
                msg.params.push(r.to_string());
                break;
            }
        }
    }

    Some(msg)
}

/// Format a message for the wire, without the trailing CRLF.
///
/// The last param is written as a trailing (`:`-prefixed) param when
/// [`Message::trailing`] says so, or when it must be regardless — it contains
/// a space, is empty, or begins with `:`. The "must be" cases are enforced
/// even when the flag is false, so a caller who forgets the flag still gets a
/// line that reparses to the same message.
pub fn format(msg: &Message) -> String {
    let mut out = String::new();

    if !msg.tags.is_empty() {
        out.push('@');
        for (i, (k, v)) in msg.tags.iter().enumerate() {
            if i > 0 {
                out.push(';');
            }
            out.push_str(k);
            if !v.is_empty() {
                out.push('=');
                out.push_str(&escape_tag(v));
            }
        }
        out.push(' ');
    }

    if let Some(p) = &msg.prefix {
        out.push(':');
        out.push_str(p);
        out.push(' ');
    }

    out.push_str(&msg.command);

    let n = msg.params.len();
    for (i, p) in msg.params.iter().enumerate() {
        out.push(' ');
        let last = i + 1 == n;
        if last && (msg.trailing || p.is_empty() || p.contains(' ') || p.starts_with(':')) {
            out.push(':');
        }
        out.push_str(p);
    }

    out
}

/// Split a read buffer into complete lines, returning the parsed messages and
/// the unconsumed tail. TCP does not deliver messages, it delivers bytes; the
/// caller keeps the tail and prepends it to the next read.
pub fn split_lines(buf: &str) -> (Vec<Message>, &str) {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = buf.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'\n' {
            let line = &buf[start..i];
            if let Some(m) = parse(line) {
                out.push(m);
            }
            start = i + 1;
        }
    }
    (out, &buf[start..])
}

/// Is this target a channel rather than a nick? Channel prefixes are
/// ISUPPORT-configurable in principle; the four in RFC 2811 cover every
/// network anyone here will join, and guessing wrong only mislabels a log.
pub fn is_channel(target: &str) -> bool {
    matches!(target.chars().next(), Some('#') | Some('&') | Some('+') | Some('!'))
}

/// Lowercase a nick or channel for comparison using RFC 1459 casemapping,
/// where `[]\~` are the uppercase forms of `{}|^`. Networks really do treat
/// `foo[bar]` and `foo{bar}` as the same nick, and an ASCII-only lowercase
/// would file them as two people in the log.
pub fn lower(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            '~' => '^',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_privmsg() {
        let m = parse(":nick!u@h PRIVMSG #asahi-dev :hello there").unwrap();
        assert_eq!(m.prefix.as_deref(), Some("nick!u@h"));
        assert_eq!(m.nick(), Some("nick"));
        assert_eq!(m.command, "PRIVMSG");
        assert_eq!(m.params, vec!["#asahi-dev", "hello there"]);
    }

    #[test]
    fn trailing_keeps_its_spaces_and_colons() {
        let m = parse("PRIVMSG #x :a: b : c").unwrap();
        assert_eq!(m.params, vec!["#x", "a: b : c"]);
    }

    #[test]
    fn empty_trailing_is_present_not_absent() {
        let m = parse("PRIVMSG #x :").unwrap();
        assert_eq!(m.params, vec!["#x", ""]);
        let n = parse("PRIVMSG #x").unwrap();
        assert_eq!(n.params, vec!["#x"]);
        assert_ne!(m, n);
    }

    #[test]
    fn tags_parse_and_unescape() {
        let m = parse("@time=2026-08-22T15:54:22.000Z;msgid=abc :n!u@h PRIVMSG #c :hi").unwrap();
        assert_eq!(m.tag("time"), Some("2026-08-22T15:54:22.000Z"));
        assert_eq!(m.tag("msgid"), Some("abc"));
        assert_eq!(m.param(1), "hi");
    }

    #[test]
    fn tag_escape_alphabet_is_the_irc_one_not_c() {
        assert_eq!(unescape_tag(r"a\sb"), "a b");
        assert_eq!(unescape_tag(r"a\:b"), "a;b");
        assert_eq!(unescape_tag(r"a\\b"), r"a\b");
        // Unknown escape: drop the backslash, keep the char.
        assert_eq!(unescape_tag(r"a\qb"), "aqb");
        // Lone trailing backslash is dropped.
        assert_eq!(unescape_tag(r"ab\"), "ab");
        assert_eq!(escape_tag("a b;c\\d"), r"a\sb\:c\\d");
    }

    #[test]
    fn valueless_tag_is_empty_string() {
        let m = parse("@foo;bar=1 PING x").unwrap();
        assert_eq!(m.tag("foo"), Some(""));
        assert_eq!(m.tag("bar"), Some("1"));
    }

    #[test]
    fn numerics_keep_their_digits() {
        let m = parse(":server 001 mynick :Welcome to the network").unwrap();
        assert_eq!(m.command, "001");
        assert_eq!(m.param(0), "mynick");
    }

    #[test]
    fn server_prefix_has_no_nick() {
        let m = parse(":irc.oftc.net NOTICE * :*** Looking up your hostname").unwrap();
        assert_eq!(m.nick(), None);
        assert_eq!(m.prefix.as_deref(), Some("irc.oftc.net"));
    }

    #[test]
    fn round_trip_preserves_the_common_shapes() {
        for line in [
            "PING :12345",
            "PRIVMSG #chan :hello world",
            ":n!u@h JOIN #chan",
            "@time=2026-08-22T00:00:00Z :n!u@h PRIVMSG #c :body",
            "CAP REQ :server-time message-tags",
            "PRIVMSG #x :",
        ] {
            let m = parse(line).unwrap();
            assert_eq!(format(&m), line, "round trip failed for {line}");
        }
    }

    #[test]
    fn a_param_that_needs_a_colon_gets_one_even_without_the_flag() {
        let m = Message::new("PRIVMSG", &["#x", "two words"]);
        assert!(!m.trailing);
        let s = format(&m);
        assert_eq!(s, "PRIVMSG #x :two words");
        assert_eq!(parse(&s).unwrap().params, m.params);
    }

    #[test]
    fn the_trailing_flag_is_what_preserves_a_single_word_colon() {
        // The bug this bit exists for: both spellings parse the same, so
        // without the flag `PING :12345` round-trips to `PING 12345`.
        assert_eq!(format(&Message::new("PING", &["12345"])), "PING 12345");
        assert_eq!(format(&Message::trailing("PING", &["12345"])), "PING :12345");
        assert!(parse("PING :12345").unwrap().trailing);
        assert!(!parse("PING 12345").unwrap().trailing);
    }

    #[test]
    fn malformed_lines_are_tolerated_not_fatal() {
        assert!(parse("").is_none());
        assert!(parse(":only-a-prefix").is_none());
        assert!(parse("@only=tags").is_none());
        // A line with nothing but a command is fine.
        assert_eq!(parse("QUIT").unwrap().command, "QUIT");
    }

    #[test]
    fn split_lines_keeps_the_partial_tail() {
        let (msgs, tail) = split_lines("PING :1\r\nPING :2\r\nPRIVMSG #x :par");
        assert_eq!(msgs.len(), 2);
        assert_eq!(tail, "PRIVMSG #x :par");
        let (msgs2, tail2) = split_lines("tial\r\n");
        assert_eq!(msgs2.len(), 1);
        assert_eq!(tail2, "");
    }

    #[test]
    fn channel_detection_and_rfc1459_casemapping() {
        assert!(is_channel("#asahi-dev"));
        assert!(is_channel("&local"));
        assert!(!is_channel("nickname"));
        assert_eq!(lower("Foo[Bar]"), "foo{bar}");
        assert_eq!(lower("A\\B~C"), "a|b^c");
        assert_eq!(lower("#Asahi-Dev"), "#asahi-dev");
    }
}
