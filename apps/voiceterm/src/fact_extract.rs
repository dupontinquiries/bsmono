//! fact_extract: scan terminal output for generic "facts" the user
//! might want to recall by voice — IP addresses, MAC addresses, hex
//! hashes (git SHAs etc.), PIDs, ports, and file paths.
//!
//! This is the pure foundation for the recent-fact-lookup feature
//! (TODO_recent_fact_lookup.md): "insert that last IP address",
//! "insert the IP for kwin", etc. It is intentionally NOT tool-specific
//! (not git-aware, not bluetoothctl-aware) — it scans ANY text for
//! generic token shapes. The live wiring (PTY-write-at-cursor, voice
//! intents) is layered on top elsewhere; this module only finds + ranks
//! candidate facts.
//!
//! PURE + dependency-free: no regex crate, no IO. Hand-rolled matchers
//! (the codebase convention) so it stays in the sandbox-testable set.

/// The kind of fact a token represents. Named to match the words a
/// user would say ("insert the last IP", "the hash", "which port").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactKind {
    Ipv4,
    Ipv6,
    Mac,
    /// A long hex run — git SHAs, checksums, etc. (>= 7 hex chars).
    Hash,
    /// A bare integer that looks like a process id (1..=4_194_304,
    /// the Linux PID max) appearing in a PID-ish context.
    Pid,
    /// A port number (1..=65535), usually after ':' or the word "port".
    Port,
    /// A filesystem path (contains '/' or a leading './', '~/', '../').
    Path,
}

impl FactKind {
    /// Map a spoken noun to a kind. Accepts the common phrasings a user
    /// would say. Returns None for unrecognized nouns.
    pub fn from_spoken(noun: &str) -> Option<FactKind> {
        let n = noun.trim().to_lowercase();
        match n.as_str() {
            "ip" | "ip address" | "ipv4" | "address" => Some(FactKind::Ipv4),
            "ipv6" | "ip v6" | "v6 address" => Some(FactKind::Ipv6),
            "mac" | "mac address" | "hardware address" => Some(FactKind::Mac),
            "hash" | "sha" | "commit" | "checksum" | "digest" => Some(FactKind::Hash),
            "pid" | "process" | "process id" | "process number" => Some(FactKind::Pid),
            "port" => Some(FactKind::Port),
            "file" | "filename" | "path" | "file name" | "file path"
                => Some(FactKind::Path),
            _ => None,
        }
    }
}

/// What the user wants done with a referenced fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactAction {
    /// Type the fact's value into the terminal at the cursor.
    Insert,
    /// Scroll the terminal back to where the fact appears.
    ScrollTo,
}

/// A parsed "do X with the recent <fact>" voice request. The pure
/// recognizer for the recent-fact feature (TODO_recent_fact_lookup #2-3).
/// The live side then runs most_recent[_near] + the PTY action; THIS is
/// just the parse, where the subtle bugs live (distinguishing insert vs
/// scroll, pulling out the "for <keyword>" qualifier).
#[derive(Clone, Debug, PartialEq)]
pub struct FactRequest {
    pub action: FactAction,
    pub kind: FactKind,
    /// Optional keyword to disambiguate WHICH fact ("the IP for kwin" →
    /// keyword "kwin"), fed to most_recent_near. None = most recent of
    /// the kind anywhere.
    pub keyword: Option<String>,
}

/// Parse a spoken phrase into a FactRequest, or None if it isn't one.
/// Recognizes:
///   "insert [the|that] [last|recent] <fact-noun> [for <keyword>]"
///   "type [the] <fact-noun> ..."         → Insert
///   "scroll [back|up] to [the] <fact-noun> [for <keyword>]"
///   "find [the] <fact-noun> in scrollback"  → ScrollTo
/// The fact-noun is resolved via FactKind::from_spoken, so it accepts
/// the same vocabulary ("ip address", "hash", "port", "filename", ...).
pub fn parse_fact_request(phrase: &str) -> Option<FactRequest> {
    let p = phrase.trim().to_lowercase();

    // Determine the action from the leading verb.
    let (action, rest) = if let Some(r) = strip_any(&p, &[
        "insert ", "type ", "paste ", "put in ", "write ",
    ]) {
        (FactAction::Insert, r)
    } else if let Some(r) = strip_any(&p, &[
        "scroll back to ", "scroll up to ", "scroll to ",
        "go back to ", "jump back to ", "jump to ",
    ]) {
        (FactAction::ScrollTo, r)
    } else {
        return None;
    };

    // Strip filler determiners/recency words.
    let mut body = rest.trim();
    for filler in ["the ", "that ", "a ", "my ", "last ", "recent ", "most recent ",
                   "previous ", "latest "] {
        while let Some(s) = body.strip_prefix(filler) {
            body = s.trim_start();
        }
    }

    // Split off a trailing "for <keyword>" or "in scrollback"/"in the log".
    let (noun_part, keyword) = if let Some(idx) = body.find(" for ") {
        let kw = body[idx + 5..].trim().to_string();
        (body[..idx].trim(), if kw.is_empty() { None } else { Some(kw) })
    } else {
        (body, None)
    };

    // Drop trailing scrollback qualifiers from the noun if present.
    let noun = noun_part
        .trim_end_matches(" in scrollback")
        .trim_end_matches(" in the log")
        .trim_end_matches(" in the output")
        .trim();

    let kind = FactKind::from_spoken(noun)?;
    Some(FactRequest { action, kind, keyword })
}

/// Strip the first matching prefix from `prefixes`, returning the rest.
fn strip_any<'a>(s: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for p in prefixes {
        if let Some(r) = s.strip_prefix(p) {
            return Some(r);
        }
    }
    None
}

/// A fact found in some text, with enough context to rank it.
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub kind: FactKind,
    /// The literal token, exactly as it should be inserted.
    pub value: String,
    /// Byte offset of the token's start within the scanned text. Higher
    /// = later in the text = more recent (terminal output is appended).
    pub offset: usize,
}

// ── character-class helpers (no regex) ──

fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// Split text into tokens: maximal runs of non-whitespace, paired with
/// their byte offset. We scan the raw text (not a pre-split Vec) so
/// offsets are exact and callers can rank facts by recency.
fn tokens(text: &str) -> TokenIter<'_> {
    TokenIter { text, pos: 0 }
}

struct TokenIter<'a> {
    text: &'a str,
    pos: usize,
}
impl<'a> Iterator for TokenIter<'a> {
    type Item = (usize, &'a str);
    fn next(&mut self) -> Option<(usize, &'a str)> {
        let bytes = self.text.as_bytes();
        // skip whitespace
        while self.pos < bytes.len() {
            let c = bytes[self.pos];
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos >= bytes.len() {
            return None;
        }
        let start = self.pos;
        while self.pos < bytes.len() {
            let c = bytes[self.pos];
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                break;
            }
            self.pos += 1;
        }
        Some((start, &self.text[start..self.pos]))
    }
}

/// Trim surrounding punctuation a terminal commonly wraps tokens in:
/// parentheses, brackets, quotes, trailing commas/periods/colons/
/// semicolons. Keeps a trailing '/' (meaningful for dirs).
fn trim_token(t: &str) -> &str {
    // Asymmetric + looped-until-stable. A leading '.' is almost always
    // meaningful (./foo, ../foo, .bashrc) so we DON'T strip it; a
    // trailing '.' is usually sentence punctuation so we DO. Brackets/
    // quotes/commas/semicolons are stripped from both ends. Looping
    // handles wrapped+punctuated tokens like "(192.168.0.1)." where one
    // pass would leave the other edge. Interior dots (IPs, paths) are
    // untouched because we only ever trim the OUTER edges.
    let lead = |c: char| {
        matches!(c, '(' | '[' | '{' | '"' | '\'' | '`' | ',' | ';')
    };
    let trail = |c: char| {
        matches!(c, ')' | ']' | '}' | '"' | '\'' | '`' | ',' | ';' | '.')
    };
    let mut cur = t;
    loop {
        let trimmed = cur.trim_start_matches(lead).trim_end_matches(trail);
        if trimmed.len() == cur.len() {
            return trimmed;
        }
        cur = trimmed;
    }
}

// ── shape matchers ──

/// IPv4: four 0-255 dotted octets, nothing else attached.
fn is_ipv4(t: &str) -> bool {
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty()
            && p.len() <= 3
            && p.chars().all(|c| c.is_ascii_digit())
            && p.parse::<u16>().map(|n| n <= 255).unwrap_or(false)
    })
}

/// IPv6: at least two ':' and only hex / ':'; allow '::' compression.
/// Deliberately loose (full RFC validation is overkill for recall).
fn is_ipv6(t: &str) -> bool {
    // THE ACTUAL ADDRESS STRUCTURE, not "two colons and some hex". The old
    // test accepted any token with two colons and only hex digits, so a clock
    // time -- `12:34:56`, in every `date`, `uptime` and log timestamp -- was an
    // IPv6 address, and "insert the ipv6" inserted a time of day.
    //
    // RFC 4291: eight groups of 1-4 hex digits, or fewer with exactly ONE `::`
    // standing in for the missing zero groups.
    if !t.chars().all(|c| is_hex(c) || c == ':') || !t.chars().any(is_hex) {
        return false;
    }
    let doubles = t.matches("::").count();
    if doubles > 1 || t.contains(":::") {
        return false;
    }
    let groups: Vec<&str> = t.split(':').filter(|g| !g.is_empty()).collect();
    if groups.iter().any(|g| g.len() > 4) {
        return false;
    }
    if doubles == 1 {
        groups.len() <= 7
    } else {
        // No `::` -- the full form, and a clock's 3 groups is not it.
        groups.len() == 8
    }
}

/// Strip a CIDR prefix length: `192.168.1.42/24` -> `192.168.1.42`.
///
/// `ip addr` -- the command people actually run to find their address --
/// prints every address WITH its prefix. Whole-word classification saw the
/// `/`, rejected it as an address, and filed it as a Path. Only accepts a
/// short all-digit suffix, so a real path like `/usr/lib` is untouched.
fn strip_prefix_len(t: &str) -> Option<&str> {
    let (addr, len) = t.rsplit_once('/')?;
    (!len.is_empty() && len.len() <= 3 && len.bytes().all(|b| b.is_ascii_digit())).then_some(addr)
}

/// Strip a `:port` from an IPv4 `host:port`: `127.0.0.1:8080` -> the address.
///
/// The port itself is still found by `extract_ports`; this recovers the IP,
/// which every classifier used to reject because of the colon -- so a
/// server's "listening on 127.0.0.1:8080" yielded the port and lost the
/// address.
fn strip_v4_port(t: &str) -> Option<&str> {
    let (addr, port) = t.rsplit_once(':')?;
    (is_ipv4(addr) && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then_some(addr)
}

/// MAC: six 2-hex-digit groups separated by ':' or '-'.
fn is_mac(t: &str) -> bool {
    let sep = if t.contains(':') {
        ':'
    } else if t.contains('-') {
        '-'
    } else {
        return false;
    };
    let parts: Vec<&str> = t.split(sep).collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(is_hex))
}

/// Hash: a run of >= 7 hex chars, all hex, not all decimal-looking
/// short. We require at least one a-f to avoid catching plain numbers,
/// OR length >= 12 (long all-digit runs are unusual outside hashes).
fn is_hash(t: &str) -> bool {
    let len = t.len();
    if len < 7 || len > 64 {
        return false;
    }
    if !t.chars().all(is_hex) {
        return false;
    }
    let has_alpha = t.chars().any(|c| c.is_ascii_alphabetic());
    has_alpha || len >= 12
}

/// Path: contains '/' (and isn't a URL scheme), or a leading ./ ../ ~/.
fn is_path(t: &str) -> bool {
    if t.starts_with("./") || t.starts_with("../") || t.starts_with("~/") || t.starts_with('/') {
        return true;
    }
    // a/b/c style relative path with at least one slash and no spaces,
    // but exclude things that are clearly URLs (handled as their own
    // thing if ever needed) — we keep it simple: any interior slash.
    t.contains('/') && !t.contains("://")
}

/// Extract all facts of all kinds from `text`, in order of appearance.
/// Offsets are byte positions so callers can rank by recency.
pub fn extract_all(text: &str) -> Vec<Fact> {
    let mut out = Vec::new();
    // Lowercased copy for context words like "port"/"pid" lookups is not
    // needed here; per-token classification only.
    let mut prev_word = "";
    for (off, raw) in tokens(text) {
        let t = trim_token(raw);
        if t.is_empty() {
            continue;
        }
        // THE BROADCAST IS NOT YOUR ADDRESS. `ip addr` prints
        // `inet 192.168.1.42/24 brd 192.168.1.255`, and `most_recent` takes
        // the LATER address -- so even with the CIDR form parsed, "insert my
        // IP" would insert the broadcast. The word `brd` marks it exactly.
        let after_brd = prev_word == "brd";
        prev_word = t;
        if after_brd {
            continue;
        }
        // An address with its prefix length or its port attached.
        if let Some(a) = strip_prefix_len(t).filter(|a| is_ipv4(a)).or_else(|| strip_v4_port(t)) {
            out.push(Fact { kind: FactKind::Ipv4, value: a.to_string(), offset: off });
            continue;
        }
        if let Some(a) = strip_prefix_len(t).filter(|a| is_ipv6(a)) {
            out.push(Fact { kind: FactKind::Ipv6, value: a.to_string(), offset: off });
            continue;
        }
        // Order matters: MAC before IPv6 (both use ':'), IPv4 before
        // Hash (digits), Path last (most permissive).
        if is_ipv4(t) {
            out.push(Fact { kind: FactKind::Ipv4, value: t.to_string(), offset: off });
        } else if is_mac(t) {
            out.push(Fact { kind: FactKind::Mac, value: t.to_string(), offset: off });
        } else if is_ipv6(t) {
            out.push(Fact { kind: FactKind::Ipv6, value: t.to_string(), offset: off });
        } else if is_hash(t) {
            out.push(Fact { kind: FactKind::Hash, value: t.to_string(), offset: off });
        } else if is_path(t) {
            out.push(Fact { kind: FactKind::Path, value: t.to_string(), offset: off });
        }
    }
    // Ports + PIDs need lightweight context, handled in a second pass.
    extract_ports(text, &mut out);
    extract_pids(text, &mut out);
    out.sort_by_key(|f| f.offset);
    out
}

/// Is the `:` at byte `i` the separator of a real `host:port`?
///
/// `:digits` alone is not enough, and three kinds of ordinary output proved it:
///
///   * a clock, `12:34:56` -- every `date`, `uptime` and log line -- gave
///     ports 34 and 56;
///   * a compiler location, `src/main.rs:42:5`, gave ports 42 and 5, so
///     "insert the port" after a build error inserted a line number;
///   * an IPv6 address, `fe80::1ff:fe23:4567:890a`, gave ports 4567 and 890
///     from its own groups.
///
/// A real host:port has EXACTLY ONE colon in its word (`127.0.0.1:8080`,
/// `localhost:3000`), or is the bracketed IPv6 form (`[::1]:8080`). That one
/// rule rejects all three. Then the host itself must look like a host: not all
/// digits (the `12` of `12:34`) and not a path (a hostname never has a `/`).
///
/// One residual, accepted: a BARE `main.rs:42` with no directory still reads as
/// a port -- "main.rs" is a plausible hostname by shape. Compilers almost always
/// print a path, which the `/` rule catches.
fn is_host_port_colon(text: &str, i: usize) -> bool {
    let b = text.as_bytes();
    let mut start = i;
    while start > 0 && !b[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    let mut end = i;
    while end < b.len() && !b[end].is_ascii_whitespace() {
        end += 1;
    }
    let word = &text[start..end];
    let host = &text[start..i];
    let bracketed_v6 = host.starts_with('[') && host.ends_with(']');
    if !bracketed_v6 && word.bytes().filter(|&c| c == b':').count() != 1 {
        return false;
    }
    !host.is_empty() && !host.bytes().all(|c| c.is_ascii_digit()) && !host.contains('/')
}

/// Ports: a number 1..=65535 immediately after ':' (host:port) or after
/// the word "port". We scan for ":<digits>" and "port <digits>".
fn extract_ports(text: &str, out: &mut Vec<Fact>) {
    let bytes = text.as_bytes();
    // host:port — a ':' followed by 1-5 digits, where the char before
    // ':' is alphanumeric or '.' or ']' (end of host/ipv6).
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' && i > 0 {
            let prev = bytes[i - 1];
            let host_ish = prev.is_ascii_alphanumeric() || prev == b'.' || prev == b']';
            if host_ish && is_host_port_colon(text, i) {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j].is_ascii_digit() && j - start < 5 {
                    j += 1;
                }
                if j > start {
                    if let Ok(n) = text[start..j].parse::<u32>() {
                        if (1..=65535).contains(&n) {
                            out.push(Fact {
                                kind: FactKind::Port,
                                value: text[start..j].to_string(),
                                offset: start,
                            });
                        }
                    }
                }
            }
        }
        i += 1;
    }
    // "port <digits>" (case-insensitive)
    let lower = text.to_lowercase();
    let mut from = 0;
    while let Some(rel) = lower[from..].find("port ") {
        let abs = from + rel + 5;
        let rest = &text[abs..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            if let Ok(n) = digits.parse::<u32>() {
                if (1..=65535).contains(&n) {
                    out.push(Fact { kind: FactKind::Port, value: digits, offset: abs });
                }
            }
        }
        from = abs;
    }
}

/// PIDs: a number after "pid", "PID", or in the leading column of ps
/// output is too noisy; we only take explicit "pid <n>" / "pid=<n>" /
/// "[<n>]" forms to keep precision high.
fn extract_pids(text: &str, out: &mut Vec<Fact>) {
    let lower = text.to_lowercase();
    for marker in &["pid ", "pid=", "pid:"] {
        let mut from = 0;
        while let Some(rel) = lower[from..].find(marker) {
            let abs = from + rel + marker.len();
            let rest = &text[abs..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                if let Ok(n) = digits.parse::<u32>() {
                    if (1..=4_194_304).contains(&n) {
                        out.push(Fact { kind: FactKind::Pid, value: digits, offset: abs });
                    }
                }
            }
            from = abs.max(from + rel + 1);
        }
    }
}

/// The most recent fact of a given kind (highest offset). None if absent.
pub fn most_recent(text: &str, kind: FactKind) -> Option<Fact> {
    extract_all(text)
        .into_iter()
        .filter(|f| f.kind == kind)
        .max_by_key(|f| f.offset)
}

/// The most recent fact of `kind` that appears on a LINE containing
/// `keyword` (case-insensitive), else falls back to the most recent of
/// that kind anywhere. This powers "insert the IP for kwin": prefer an
/// IP near a line mentioning kwin, but still return *something* if no
/// keyword line has one.
pub fn most_recent_near(text: &str, kind: FactKind, keyword: &str) -> Option<Fact> {
    let kw = keyword.trim().to_lowercase();
    if kw.is_empty() {
        return most_recent(text, kind);
    }
    let facts = extract_all(text);
    // Build line spans so we can test whether a fact's offset lands on a
    // line that mentions the keyword.
    let mut best_near: Option<Fact> = None;
    for f in facts.iter().filter(|f| f.kind == kind) {
        if let Some(line) = line_containing_offset(text, f.offset) {
            if line.to_lowercase().contains(&kw) {
                match &best_near {
                    Some(b) if b.offset >= f.offset => {}
                    _ => best_near = Some(f.clone()),
                }
            }
        }
    }
    best_near.or_else(|| most_recent(text, kind))
}

/// Return the line (without newline) that contains byte `offset`.
fn line_containing_offset(text: &str, offset: usize) -> Option<&str> {
    if offset > text.len() {
        return None;
    }
    let start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[offset..].find('\n').map(|i| offset + i).unwrap_or(text.len());
    Some(&text[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_insert_requests() {
        assert_eq!(parse_fact_request("insert the last IP address"),
            Some(FactRequest { action: FactAction::Insert, kind: FactKind::Ipv4, keyword: None }));
        assert_eq!(parse_fact_request("type the hash"),
            Some(FactRequest { action: FactAction::Insert, kind: FactKind::Hash, keyword: None }));
        // "for <keyword>" qualifier captured
        assert_eq!(parse_fact_request("insert the ip for kwin"),
            Some(FactRequest { action: FactAction::Insert, kind: FactKind::Ipv4,
                keyword: Some("kwin".to_string()) }));
        // multiple filler words stacked
        assert_eq!(parse_fact_request("paste that most recent port"),
            Some(FactRequest { action: FactAction::Insert, kind: FactKind::Port, keyword: None }));
    }

    #[test]
    fn parse_scroll_requests() {
        assert_eq!(parse_fact_request("scroll back to the hash"),
            Some(FactRequest { action: FactAction::ScrollTo, kind: FactKind::Hash, keyword: None }));
        assert_eq!(parse_fact_request("scroll up to the port"),
            Some(FactRequest { action: FactAction::ScrollTo, kind: FactKind::Port, keyword: None }));
        // scrollback qualifier stripped from the noun
        assert_eq!(parse_fact_request("scroll back to the filename in scrollback"),
            Some(FactRequest { action: FactAction::ScrollTo, kind: FactKind::Path, keyword: None }));
    }

    #[test]
    fn parse_does_not_over_capture_bare_find() {
        // v0.1.605: bare "find" is too overloaded (helix search, find-in-
        // page, file-open) to mean "scroll to a recent fact". It must NOT
        // trigger a fact request — "find the file" should fall through to
        // the generic find handlers, not become a scroll-to-path.
        assert_eq!(parse_fact_request("find the file"), None);
        assert_eq!(parse_fact_request("find the port"), None);
        assert_eq!(parse_fact_request("find the hash"), None);
        // a DIRECTIONAL scroll phrase still works
        assert_eq!(parse_fact_request("scroll back to the file").map(|r| r.action),
            Some(FactAction::ScrollTo));
    }

    #[test]
    fn parse_strips_my_filler() {
        // v0.1.605: "insert my last ip" — "my" is natural and must be
        // stripped so the noun resolves.
        assert_eq!(parse_fact_request("insert my last ip"),
            Some(FactRequest { action: FactAction::Insert, kind: FactKind::Ipv4, keyword: None }));
        assert_eq!(parse_fact_request("type my hash"),
            Some(FactRequest { action: FactAction::Insert, kind: FactKind::Hash, keyword: None }));
    }

    #[test]
    fn parse_fact_request_rejects_non_requests() {
        // no action verb
        assert_eq!(parse_fact_request("the last IP address"), None);
        // action verb but unrecognized noun
        assert_eq!(parse_fact_request("insert the banana"), None);
        // empty
        assert_eq!(parse_fact_request(""), None);
        // verb but no noun
        assert_eq!(parse_fact_request("insert the last"), None);
    }

    #[test]
    fn parse_fact_request_keyword_not_swallowed_into_noun() {
        // "insert the ip for the server" — keyword is "the server", noun
        // is clean "ip". The "for" split must happen before noun lookup.
        let r = parse_fact_request("insert the ip for the server").unwrap();
        assert_eq!(r.kind, FactKind::Ipv4);
        assert_eq!(r.keyword, Some("the server".to_string()));
    }

    #[test]
    fn extracts_ipv4() {
        let f = most_recent("connecting to 192.168.1.42 now", FactKind::Ipv4).unwrap();
        assert_eq!(f.value, "192.168.1.42");
    }

    #[test]
    fn rejects_bad_ipv4_octets() {
        // 999 is not a valid octet.
        assert!(most_recent("ping 999.1.1.1", FactKind::Ipv4).is_none());
        // three parts is not an IPv4
        assert!(most_recent("v 1.2.3", FactKind::Ipv4).is_none());
    }

    #[test]
    fn most_recent_picks_latest_by_offset() {
        let txt = "first 10.0.0.1 then later 10.0.0.2";
        let f = most_recent(txt, FactKind::Ipv4).unwrap();
        assert_eq!(f.value, "10.0.0.2", "should pick the later one");
    }

    #[test]
    fn extracts_mac_both_separators() {
        assert_eq!(
            most_recent("dev aa:bb:cc:dd:ee:ff up", FactKind::Mac).unwrap().value,
            "aa:bb:cc:dd:ee:ff");
        assert_eq!(
            most_recent("dev AA-BB-CC-DD-EE-FF", FactKind::Mac).unwrap().value,
            "AA-BB-CC-DD-EE-FF");
    }

    #[test]
    fn mac_not_confused_with_ipv6() {
        // a MAC has 6 two-char groups; classify as MAC, not IPv6.
        let f = most_recent("link aa:bb:cc:dd:ee:ff", FactKind::Mac);
        assert!(f.is_some());
        // and it should NOT also be returned as an IPv6
        assert!(most_recent("link aa:bb:cc:dd:ee:ff", FactKind::Ipv6).is_none());
    }

    #[test]
    fn extracts_ipv6() {
        let f = most_recent("addr fe80::1ff:fe23:4567:890a here", FactKind::Ipv6);
        assert!(f.is_some(), "should find an ipv6");
        assert!(f.unwrap().value.contains("fe80"));
    }

    #[test]
    fn extracts_git_sha_hash() {
        let f = most_recent("commit a1b2c3d4e5f6 done", FactKind::Hash).unwrap();
        assert_eq!(f.value, "a1b2c3d4e5f6");
    }

    #[test]
    fn short_decimal_is_not_a_hash() {
        // "12345" is 5 digits, all decimal, < 7 chars → not a hash.
        assert!(most_recent("count 12345", FactKind::Hash).is_none());
    }

    #[test]
    fn extracts_host_port() {
        let f = most_recent("listening on 127.0.0.1:8080", FactKind::Port).unwrap();
        assert_eq!(f.value, "8080");
    }

    #[test]
    fn extracts_word_port() {
        let f = most_recent("bound to port 5432 ok", FactKind::Port).unwrap();
        assert_eq!(f.value, "5432");
    }

    #[test]
    fn rejects_out_of_range_port() {
        // 99999 > 65535
        assert!(most_recent("port 99999", FactKind::Port).is_none());
    }

    #[test]
    fn extracts_pid_only_with_marker() {
        assert_eq!(most_recent("killed pid 7793", FactKind::Pid).unwrap().value, "7793");
        assert_eq!(most_recent("[pid=4242] hung", FactKind::Pid).unwrap().value, "4242");
        // a bare number with no marker is NOT a pid (precision over recall)
        assert!(most_recent("total 8192", FactKind::Pid).is_none());
    }

    #[test]
    fn extracts_paths() {
        assert_eq!(
            most_recent("editing /home/k/notes.txt now", FactKind::Path).unwrap().value,
            "/home/k/notes.txt");
        assert_eq!(
            most_recent("cd ./build/out", FactKind::Path).unwrap().value,
            "./build/out");
    }

    #[test]
    fn path_excludes_urls() {
        // a URL contains "://" — not treated as a plain path.
        assert!(most_recent("see https://example.com/x", FactKind::Path).is_none());
    }

    #[test]
    fn keyword_filter_prefers_matching_line() {
        let txt = "\
            server alpha 10.0.0.1\n\
            server kwin 10.0.0.99\n\
            server gamma 10.0.0.3\n";
        // "insert the IP for kwin" → 10.0.0.99, not the most-recent 10.0.0.3
        let f = most_recent_near(txt, FactKind::Ipv4, "kwin").unwrap();
        assert_eq!(f.value, "10.0.0.99");
    }

    #[test]
    fn keyword_filter_falls_back_when_no_match() {
        let txt = "server alpha 10.0.0.1\nserver beta 10.0.0.2\n";
        // keyword "zeta" appears nowhere → fall back to most recent.
        let f = most_recent_near(txt, FactKind::Ipv4, "zeta").unwrap();
        assert_eq!(f.value, "10.0.0.2");
    }

    #[test]
    fn from_spoken_maps_common_nouns() {
        assert_eq!(FactKind::from_spoken("IP address"), Some(FactKind::Ipv4));
        assert_eq!(FactKind::from_spoken("the hash"), None); // "the" not stripped here
        assert_eq!(FactKind::from_spoken("hash"), Some(FactKind::Hash));
        assert_eq!(FactKind::from_spoken("port"), Some(FactKind::Port));
        assert_eq!(FactKind::from_spoken("filename"), Some(FactKind::Path));
        assert_eq!(FactKind::from_spoken("banana"), None);
    }

    #[test]
    fn trims_surrounding_punctuation() {
        // IP wrapped in parens + trailing period
        let f = most_recent("host (192.168.0.1).", FactKind::Ipv4).unwrap();
        assert_eq!(f.value, "192.168.0.1");
    }

    #[test]
    fn empty_text_yields_nothing() {
        assert!(extract_all("").is_empty());
        assert!(most_recent("", FactKind::Ipv4).is_none());
    }

#[cfg(test)]
mod real_output_tests {
    use super::*;

    fn of(text: &str, kind: FactKind) -> Vec<String> {
        extract_all(text).into_iter().filter(|f| f.kind == kind).map(|f| f.value).collect()
    }

    /// THE HEADLINE. `ip addr` is how people find their address, and "insert
    /// my IP" inserted the BROADCAST: the host address carried its /24 and was
    /// filed as a Path, leaving `192.168.1.255` the only IPv4 in sight.
    #[test]
    fn ip_addr_yields_the_host_address_not_the_broadcast() {
        let out = "inet 192.168.1.42/24 brd 192.168.1.255 scope global wlan0";
        assert_eq!(of(out, FactKind::Ipv4), vec!["192.168.1.42"]);
        assert_eq!(most_recent(out, FactKind::Ipv4).map(|f| f.value).as_deref(), Some("192.168.1.42"));
        assert!(of(out, FactKind::Path).is_empty(), "the address is not a path");
    }

    /// A server's "listening on" line lost its address to the colon.
    #[test]
    fn host_port_yields_both() {
        let out = "listening on 127.0.0.1:8080";
        assert_eq!(of(out, FactKind::Ipv4), vec!["127.0.0.1"]);
        assert_eq!(of(out, FactKind::Port), vec!["8080"]);
    }

    /// A clock is not an IPv6 address and has no ports in it.
    #[test]
    fn a_time_of_day_is_nothing() {
        for t in ["12:34:56", "uptime 12:34:56 up 3 days", "2026-09-20 12:34:56", "at 09:05"] {
            assert!(of(t, FactKind::Ipv6).is_empty(), "{t}");
            assert!(of(t, FactKind::Port).is_empty(), "{t}");
        }
    }

    /// Real IPv6 -- with its prefix length, as `ip addr` prints it -- is found,
    /// and its own groups are not ports.
    #[test]
    fn ipv6_with_a_prefix_is_found_and_has_no_ports() {
        let out = "inet6 fe80::1ff:fe23:4567:890a/64 scope link";
        assert_eq!(of(out, FactKind::Ipv6), vec!["fe80::1ff:fe23:4567:890a"]);
        assert!(of(out, FactKind::Port).is_empty());
        assert!(!of("::1", FactKind::Ipv6).is_empty(), "loopback");
        assert!(!of("2001:db8:0:0:0:0:0:1", FactKind::Ipv6).is_empty(), "full form");
    }

    /// "Insert the port" after a build error must not insert a line number.
    #[test]
    fn a_compiler_location_has_no_ports() {
        assert!(of("error at /home/u/src/main.rs:42:5", FactKind::Port).is_empty());
    }

    /// Ports that ARE ports still are.
    #[test]
    fn real_ports_survive() {
        assert_eq!(of("connect localhost:3000", FactKind::Port), vec!["3000"]);
        assert_eq!(of("bound [::1]:8080", FactKind::Port), vec!["8080"]);
        assert_eq!(of("using port 5432", FactKind::Port), vec!["5432"]);
    }

    /// Structure, not character set: these are not addresses.
    #[test]
    fn malformed_ipv6_is_rejected() {
        for t in ["1:2:3", "::1::2", "12345::1", ":::"] {
            assert!(of(t, FactKind::Ipv6).is_empty(), "{t}");
        }
    }

    /// A real path keeps its slash-number tail -- only an address sheds it.
    #[test]
    fn a_path_ending_in_a_number_stays_a_path() {
        assert!(of("/usr/lib/64", FactKind::Ipv4).is_empty());
    }
}
}
