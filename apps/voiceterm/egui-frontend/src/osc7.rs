//! osc7 — learning the shell's working directory from the stream it already sends.
//!
//! `uikit_term::blocks` records a cwd per command block and its own doc says the value comes
//! from "OSC 7 or a PROMPT_COMMAND export, already resolved by the caller". Nobody was that
//! caller: the one `set_cwd` in this frontend is a hardcoded demo path, so every block claims
//! `/home/kessler/proj` and `session_name`'s repo half is that string forever, whatever
//! directory the command actually ran in. `TODO_session_autoname.md` records it as step 1's
//! open half.
//!
//! Shells announce their directory with OSC 7 — bash via `PROMPT_COMMAND`, zsh via
//! `chpwd`, fish by default:
//!
//! ```text
//!   ESC ] 7 ; file://HOST/PATH  ST        ST = BEL (0x07) or ESC \
//! ```
//!
//! ## Why parse the stream rather than ask the shell
//!
//! The alternative is running `pwd` in the pty, which injects a command into someone's
//! session, races whatever they are typing, and lands in their history. OSC 7 is already
//! being sent and costs nothing to read.
//!
//! ## The details that bite
//!
//! * **The path is percent-encoded.** A directory with a space arrives as `%20`, and a
//!   tracker told the literal string reports a cwd nothing matches.
//! * **The host is not ours.** `file://otherbox/tmp` is a remote shell's directory —
//!   accepting it would label local blocks with a path that does not exist here. Empty and
//!   `localhost` are ours; anything else is refused rather than stripped.
//! * **A chunk can split a sequence.** A pty read is an arbitrary byte boundary, so the
//!   trailing terminator may not have arrived. [`Scanner`] keeps the partial tail rather than
//!   dropping it — the alternative is a cwd update lost roughly whenever the path crosses a
//!   buffer edge, which looks like OSC 7 working unreliably rather than not at all.

/// Percent-decode, permissively: an invalid escape is kept verbatim rather than dropped.
///
/// A path is a name, not a URL to fetch — mangling one is worse than passing an odd byte
/// through, because a wrong path is silently wrong while an odd one is visibly odd.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| -> Option<u8> {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The path from one `file://host/path` URI, if it is this machine's.
///
/// `None` for a remote host, for a URI that is not `file://`, and for an empty path.
pub fn path_from_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        // `file://host` with no path at all says nothing about a directory.
        None => return None,
    };
    // Ours, or someone else's. A remote shell's cwd on a local block is a path that does
    // not exist here, and a tracker cannot tell the difference afterwards.
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let decoded = percent_decode(path);
    (!decoded.is_empty()).then_some(decoded)
}

/// Reads OSC 7 out of a pty byte stream, across chunk boundaries.
#[derive(Debug, Default)]
pub struct Scanner {
    /// Bytes of a sequence that began in an earlier chunk.
    partial: Vec<u8>,
}

/// Longest partial sequence worth holding. A path can be long; an unterminated escape that
/// grows without limit is a leak fed by whatever the program on the other end prints.
pub const MAX_PARTIAL: usize = 4096;

impl Scanner {
    pub fn new() -> Scanner {
        Scanner::default()
    }

    /// Feed a chunk; get every complete local path it announced, in order.
    ///
    /// Usually zero or one. More than one arrives when a script cds several times before the
    /// terminal is read, and the LAST is the current directory — returned in order so the
    /// caller can take the last rather than this deciding for it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut buf = std::mem::take(&mut self.partial);
        buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut i = 0;
        let mut consumed = 0;
        while i < buf.len() {
            // ESC, and not yet enough bytes to know what follows. A read can end mid
            // introducer — split at 1 is a lone ESC — and dropping it loses the sequence
            // that was about to arrive. Held only while it is still a possible prefix, so
            // an ESC that turns out to introduce something else is released immediately.
            if buf[i] == 0x1b && buf[i + 1..].len() < 3 && b"]7;".starts_with(&buf[i + 1..]) {
                self.partial = buf[i..].to_vec();
                return out;
            }
            // ESC ] 7 ;
            if buf[i] == 0x1b && buf[i + 1..].starts_with(b"]7;") {
                let start = i + 4;
                match terminator(&buf[start..]) {
                    Some((end, skip)) => {
                        if let Ok(uri) = std::str::from_utf8(&buf[start..start + end]) {
                            if let Some(p) = path_from_uri(uri) {
                                out.push(p);
                            }
                        }
                        i = start + end + skip;
                        consumed = i;
                        continue;
                    }
                    // Unterminated: the rest of this chunk is the start of a sequence.
                    None => {
                        self.partial = buf[i..].to_vec();
                        if self.partial.len() > MAX_PARTIAL {
                            // Not a sequence, or a hostile one. Drop it rather than grow
                            // forever on whatever the far end prints.
                            self.partial.clear();
                        }
                        return out;
                    }
                }
            }
            i += 1;
            consumed = i;
        }
        let _ = consumed;
        self.partial.clear();
        out
    }
}

/// Where a sequence's terminator starts, and how many bytes it takes: BEL is one, `ESC \`
/// is two.
fn terminator(rest: &[u8]) -> Option<(usize, usize)> {
    for (i, b) in rest.iter().enumerate() {
        if *b == 0x07 {
            return Some((i, 1));
        }
        if *b == 0x1b && rest.get(i + 1) == Some(&b'\\') {
            return Some((i, 2));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn osc(uri: &str) -> Vec<u8> {
        let mut v = b"\x1b]7;".to_vec();
        v.extend_from_slice(uri.as_bytes());
        v.push(0x07);
        v
    }

    #[test]
    fn a_shell_announcing_its_directory_is_heard() {
        let mut s = Scanner::new();
        assert_eq!(s.feed(&osc("file://localhost/home/k/proj")), vec!["/home/k/proj".to_string()]);
        // An empty host is this machine too — bash's PROMPT_COMMAND often omits it.
        assert_eq!(s.feed(&osc("file:///srv/work")), vec!["/srv/work".to_string()]);
        // ESC \ terminates as well as BEL.
        let mut v = b"\x1b]7;file:///tmp".to_vec();
        v.extend_from_slice(b"\x1b\\");
        assert_eq!(s.feed(&v), vec!["/tmp".to_string()]);
        // Ordinary output says nothing.
        assert!(s.feed(b"hello\nworld\n").is_empty());
    }

    /// A directory with a space arrives percent-encoded, and a tracker told the literal
    /// string reports a cwd nothing matches.
    #[test]
    fn the_path_is_percent_decoded() {
        let mut s = Scanner::new();
        assert_eq!(s.feed(&osc("file:///home/k/My%20Docs")), vec!["/home/k/My Docs".to_string()]);
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        // An invalid escape is kept verbatim: a path is a name, not a URL to fetch, and a
        // wrong path is silently wrong while an odd one is visibly odd.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%2"), "%2");
    }

    /// `file://otherbox/tmp` is a REMOTE shell's directory. Accepting it labels local blocks
    /// with a path that does not exist here, and nothing downstream can tell afterwards.
    #[test]
    fn another_hosts_directory_is_refused_not_stripped() {
        let mut s = Scanner::new();
        assert!(s.feed(&osc("file://buildbox/home/k")).is_empty());
        assert_eq!(path_from_uri("file://LOCALHOST/x"), Some("/x".into()), "case-insensitive");
        // Not a file URI, and a URI with no path.
        assert_eq!(path_from_uri("http://example/x"), None);
        assert_eq!(path_from_uri("file://localhost"), None);
        assert_eq!(path_from_uri(""), None);
    }

    /// A pty read is an arbitrary byte boundary. Dropping a split sequence loses a cwd update
    /// roughly whenever the path crosses a buffer edge, which looks like OSC 7 working
    /// unreliably rather than not at all.
    #[test]
    fn a_sequence_split_across_reads_still_arrives() {
        let full = osc("file:///home/k/deep/path");
        for cut in 1..full.len() {
            let mut s = Scanner::new();
            let first = s.feed(&full[..cut]);
            let second = s.feed(&full[cut..]);
            let all: Vec<String> = first.into_iter().chain(second).collect();
            assert_eq!(all, vec!["/home/k/deep/path".to_string()], "split at {cut}");
        }
    }

    /// A script that cds several times before the terminal is read announces each. They come
    /// back in order so the caller takes the last, rather than this deciding.
    #[test]
    fn several_announcements_in_one_chunk_come_back_in_order() {
        let mut s = Scanner::new();
        let mut chunk = osc("file:///a");
        chunk.extend_from_slice(b"some output\n");
        chunk.extend_from_slice(&osc("file:///b"));
        chunk.extend_from_slice(&osc("file://remote/c"));
        chunk.extend_from_slice(&osc("file:///d"));
        assert_eq!(s.feed(&chunk), vec!["/a".to_string(), "/b".to_string(), "/d".to_string()]);
    }

    /// An unterminated escape must not grow forever on whatever the far end prints.
    #[test]
    fn a_runaway_sequence_is_dropped_rather_than_buffered() {
        let mut s = Scanner::new();
        let mut junk = b"\x1b]7;file:///".to_vec();
        junk.extend(std::iter::repeat(b'x').take(MAX_PARTIAL + 100));
        assert!(s.feed(&junk).is_empty());
        // And the scanner is usable afterwards rather than stuck holding it.
        assert_eq!(s.feed(&osc("file:///home/k")), vec!["/home/k".to_string()]);
    }
}
