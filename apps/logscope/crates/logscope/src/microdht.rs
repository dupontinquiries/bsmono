//! microdht.rs — the sc-microdht wire protocol, without a socket.
//!
//! A Rust port of `MicroDhtClient.cpp`'s protocol half. The C++ is 209 lines of Qt: `QUdpSocket`,
//! `QTimer`, and the line format tangled together. **Only the line format is worth porting** — the
//! transport is ten lines of tokio and the protocol is the part with rules in it.
//!
//! Split this way because the protocol is testable with no network at all: every case below is a
//! string in and a value out, which means the parsing can be got right before a socket exists.
//!
//! ## The wire
//!
//! ```text
//!   PING
//!   PUT <sc-id> <hex-value> <ttl-secs>
//!   GET <sc-id>
//!   STATS
//! ```
//!
//! Responses begin `OK`, `PONG`, `VALUES` or `ERR`. **Values are hex, not base64** — worth stating
//! because the C++ has a `hexEncode` helper next to Qt's `toBase64`, and reaching for the wrong one
//! produces a value the server accepts and no reader can decode.

/// A request to send.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Request {
    Ping,
    Put { sc_id: String, value: Vec<u8>, ttl_s: u32 },
    Get { sc_id: String },
    Stats,
}

/// What came back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Response {
    Pong,
    Ok,
    /// Zero or more values. **An empty VALUES is a valid answer meaning "nothing published"**, and
    /// it is not an error — conflating the two makes a peer that has never announced look like a
    /// peer that failed.
    Values(Vec<Vec<u8>>),
    Err(String),
    /// Node statistics, as free text.
    ///
    /// **Found by reading the C++ rather than by testing.** It handles a `STATS` response verb that
    /// I had not inferred from the request side, and without this arm a perfectly good reply would
    /// be rejected as a version mismatch — a false negative that would have looked like the node
    /// being incompatible.
    Stats(String),
}

/// An id may not contain a space, because the protocol is space-delimited with no quoting.
///
/// Checked rather than trusted: an id with a space produces a PUT the server parses as a different
/// command, and the failure surfaces as a value silently stored under the wrong key.
pub fn valid_id(s: &str) -> bool {
    !s.is_empty() && !s.contains(' ') && !s.contains('\n') && s.is_ascii()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None }
    (0..s.len()).step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

impl Request {
    /// The datagram body.
    pub fn encode(&self) -> Result<String, String> {
        match self {
            Request::Ping => Ok("PING".into()),
            Request::Stats => Ok("STATS".into()),
            Request::Get { sc_id } => {
                if !valid_id(sc_id) { return Err(format!("invalid id {sc_id:?}")) }
                Ok(format!("GET {sc_id}"))
            }
            Request::Put { sc_id, value, ttl_s } => {
                if !valid_id(sc_id) { return Err(format!("invalid id {sc_id:?}")) }
                if *ttl_s == 0 {
                    // A zero TTL is accepted by the server and expires immediately, so the caller
                    // sees a successful PUT and a GET that finds nothing — a failure that looks like
                    // the DHT losing data.
                    return Err("ttl of 0 expires immediately; the PUT succeeds and the value is \
already gone by the time anything reads it".into());
                }
                Ok(format!("PUT {sc_id} {} {ttl_s}", hex(value)))
            }
        }
    }
}

/// Parse a response datagram.
pub fn decode(line: &str) -> Result<Response, String> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut it = line.split(' ');
    match it.next() {
        Some("PONG") => Ok(Response::Pong),
        Some("OK") => Ok(Response::Ok),
        Some("VALUES") => {
            let mut out = Vec::new();
            for tok in it {
                if tok.is_empty() { continue }
                out.push(unhex(tok).ok_or_else(|| format!("value {tok:?} is not hex — the server \
sends hex and a base64 reader would decode this to plausible garbage"))?);
            }
            Ok(Response::Values(out))
        }
        Some("STATS") => Ok(Response::Stats(it.collect::<Vec<_>>().join(" "))),
        Some("ERR") => Ok(Response::Err(it.collect::<Vec<_>>().join(" "))),
        // NOT an error to retry. An unrecognised verb means the server is a different version or a
        // different service entirely, and retrying will not change that.
        Some(other) => Err(format!("unrecognised response verb {other:?} — this is a version or \
service mismatch rather than a transient failure, so retrying will not help")),
        None => Err("empty datagram".into()),
    }
}

/// Whether a failed request is worth sending again.
///
/// **UDP loses datagrams, so a timeout is expected and retryable.** A protocol error is not: the
/// server understood and disagreed, and sending the same bytes again gets the same answer.
pub fn retryable(e: &str) -> bool {
    e.contains("timed out") || e.contains("no response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_PUT_round_trips_through_hex() {
        let r = Request::Put { sc_id: "sc-32a51661".into(), value: vec![0xde, 0xad], ttl_s: 600 };
        assert_eq!(r.encode().unwrap(), "PUT sc-32a51661 dead 600");
    }

    #[test]
    fn VALUES_decodes_hex_and_an_EMPTY_list_is_not_an_error() {
        // "Nothing published" and "the lookup failed" produce different next actions, and conflating
        // them makes a peer that never announced look like a peer that broke.
        assert_eq!(decode("VALUES dead beef").unwrap(),
                   Response::Values(vec![vec![0xde, 0xad], vec![0xbe, 0xef]]));
        assert_eq!(decode("VALUES").unwrap(), Response::Values(vec![]));
    }

    #[test]
    fn a_BASE64_value_is_rejected_rather_than_decoded_to_garbage() {
        // The C++ has hexEncode sitting next to Qt's toBase64. Reaching for the wrong one produces
        // something the server stores and nothing can read.
        let e = decode("VALUES 3q2+7w==").unwrap_err();
        assert!(e.contains("plausible garbage"));
    }

    #[test]
    fn an_ID_WITH_A_SPACE_is_refused_because_the_protocol_cannot_quote() {
        // Otherwise the server parses the tail as a different command and stores the value under the
        // wrong key, silently.
        let r = Request::Get { sc_id: "sc-ab cd".into() };
        assert!(r.encode().is_err());
        assert!(!valid_id("a b"));
        assert!(valid_id("sc-32a51661"));
    }

    #[test]
    fn a_ZERO_TTL_is_refused_with_the_reason() {
        // The server accepts it, the PUT succeeds, and the value is gone before anything reads it —
        // which looks like the DHT losing data rather than like a caller error.
        let e = Request::Put { sc_id: "sc-a".into(), value: vec![1], ttl_s: 0 }.encode().unwrap_err();
        assert!(e.contains("already gone by the time anything reads it"));
    }

    #[test]
    fn an_UNKNOWN_verb_is_not_retryable() {
        // A version or service mismatch, not a transient failure. Retrying gets the same answer.
        let e = decode("WAT something").unwrap_err();
        assert!(e.contains("version or service mismatch"));
        assert!(!retryable(&e));
    }

    #[test]
    fn a_TIMEOUT_is_retryable_because_UDP_loses_datagrams() {
        assert!(retryable("request timed out after 2000ms"));
        assert!(retryable("no response from node"));
    }

    #[test]
    fn a_STATS_reply_is_understood_rather_than_called_a_mismatch() {
        // The C++ handles this verb and I had not inferred it from the request side. Without the
        // arm, a good reply reads as an incompatible node — a false negative in the direction that
        // makes a working peer look broken.
        assert_eq!(decode("STATS keys=12 uptime=900").unwrap(),
                   Response::Stats("keys=12 uptime=900".into()));
    }

    #[test]
    fn ERR_carries_the_servers_whole_message() {
        // Truncating at the first space would drop the part that says what was wrong.
        assert_eq!(decode("ERR key too long").unwrap(), Response::Err("key too long".into()));
    }

    #[test]
    fn trailing_newlines_do_not_change_the_parse() {
        // A line-oriented server may or may not terminate; both must read the same.
        assert_eq!(decode("PONG\r\n").unwrap(), Response::Pong);
        assert_eq!(decode("PONG").unwrap(), Response::Pong);
    }
}
