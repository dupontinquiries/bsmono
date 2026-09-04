//! rpc — LSP's message framing, isolated from everything else. A message is
//! `Content-Length: N\r\n\r\n` followed by exactly N bytes of UTF-8 JSON; no
//! other header is required, and servers vary on whether they send
//! `Content-Type` at all, so this only requires the one the spec actually
//! mandates.
//!
//! Split out from `client.rs` because it is the one piece with a real
//! adversary — a language server's own stdout, which is not this crate's
//! code and can send a malformed length, a truncated body, or (rust-
//! analyzer does this on startup) stray non-protocol bytes before the first
//! real message. Framing bugs are exactly the kind that only show up against
//! a real server, never against a hand-built test fixture that happens to
//! write valid frames — so the tests here build ADVERSARIAL byte streams by
//! hand rather than only round-tripping `write_message`'s own output.

use std::io::{self, BufRead, Write};

/// Write one JSON-RPC message with its `Content-Length` header.
pub fn write_message<W: Write>(w: &mut W, body: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(body)?;
    write!(w, "Content-Length: {}\r\n\r\n", bytes.len())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read one JSON-RPC message, blocking until a full frame arrives or the
/// stream ends. Returns `Ok(None)` on a clean EOF (the server exited)
/// rather than an error — that is an expected shutdown path, not a protocol
/// violation, and a caller's read loop should be able to tell the two apart
/// without matching on error kinds.
pub fn read_message<R: BufRead>(r: &mut R) -> io::Result<Option<serde_json::Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF before any header — clean shutdown
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break; // blank line ends the header block
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_length = Some(v.trim().parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, format!("bad Content-Length: {v:?}"))
            })?);
        }
        // Any other header (Content-Type, or something a future server
        // version adds) is read past and ignored — this loop's only job is
        // finding the blank line, not validating headers the spec doesn't
        // require every server to send.
    }
    let len = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "message had no Content-Length header")
    })?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let value = serde_json::from_slice(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn write_then_read_round_trips_a_message() {
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut r = BufReader::new(&buf[..]);
        let got = read_message(&mut r).unwrap().unwrap();
        assert_eq!(got, msg);
    }

    #[test]
    fn two_messages_back_to_back_read_correctly_in_sequence() {
        // The real failure mode this guards: reading exactly `len` bytes and
        // no more, so the SECOND message's header isn't accidentally
        // consumed as part of the first message's body or vice versa.
        let a = serde_json::json!({"id": 1});
        let b = serde_json::json!({"id": 2, "note": "second message, must start clean"});
        let mut buf = Vec::new();
        write_message(&mut buf, &a).unwrap();
        write_message(&mut buf, &b).unwrap();
        let mut r = BufReader::new(&buf[..]);
        assert_eq!(read_message(&mut r).unwrap().unwrap(), a);
        assert_eq!(read_message(&mut r).unwrap().unwrap(), b);
    }

    #[test]
    fn a_clean_eof_before_any_header_is_none_not_an_error() {
        let mut r = BufReader::new(&b""[..]);
        assert!(read_message(&mut r).unwrap().is_none());
    }

    #[test]
    fn a_header_with_extra_whitespace_still_parses() {
        // Real servers vary on "Content-Length: 12" vs "Content-Length:12".
        let body = br#"{"id":1}"#;
        let raw = format!("Content-Length:   {}   \r\n\r\n{}", body.len(), std::str::from_utf8(body).unwrap());
        let mut r = BufReader::new(raw.as_bytes());
        let got = read_message(&mut r).unwrap().unwrap();
        assert_eq!(got, serde_json::json!({"id": 1}));
    }

    #[test]
    fn an_unknown_header_before_content_length_is_skipped_not_fatal() {
        // e.g. a Content-Type header some servers still send.
        let body = br#"{"id":7}"#;
        let raw = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            body.len(), std::str::from_utf8(body).unwrap()
        );
        let mut r = BufReader::new(raw.as_bytes());
        assert_eq!(read_message(&mut r).unwrap().unwrap(), serde_json::json!({"id": 7}));
    }

    #[test]
    fn a_missing_content_length_is_a_clear_error_not_a_panic() {
        let raw = "Content-Type: application/vscode-jsonrpc\r\n\r\n{}";
        let mut r = BufReader::new(raw.as_bytes());
        let err = read_message(&mut r).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_truncated_body_is_an_io_error_not_a_silent_wrong_message() {
        // Length header promises more bytes than the stream actually has —
        // a server that died mid-write, or a genuinely malformed stream.
        let raw = "Content-Length: 100\r\n\r\n{\"id\":1}"; // far fewer than 100 bytes follow
        let mut r = BufReader::new(raw.as_bytes());
        assert!(read_message(&mut r).is_err());
    }

    #[test]
    fn invalid_json_in_an_otherwise_well_framed_message_is_an_error() {
        let raw = "Content-Length: 9\r\n\r\nnot json!";
        let mut r = BufReader::new(raw.as_bytes());
        let err = read_message(&mut r).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
