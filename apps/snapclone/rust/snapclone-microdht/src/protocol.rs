//! protocol.rs — the micro-DHT's tiny, human-readable UDP wire protocol.
//!
//! Each datagram is ONE request or ONE response, a single line of UTF-8 with
//! space-separated fields. Binary value payloads are hex-encoded so the whole
//! thing stays printable (you can literally `nc -u` a node to poke it during a
//! beta-test session). Keeping it text + dependency-free is the whole point:
//! no serde, no protobuf, no version-skew with the app's real OpenDHT codec —
//! this is a TEST harness, not the production transport.
//!
//! Requests (client -> node):
//!   PING                              liveness check
//!   PUT  <key> <hexvalue> <ttl_secs>  store a value under key for ttl seconds
//!   GET  <key>                        fetch all live values under key
//!   STATS                             node counters
//!
//! Responses (node -> client):
//!   PONG
//!   OK
//!   VALUES <key> <hexvalue1> <hexvalue2> ...   (zero or more)
//!   STATS keys=<n> values=<n> puts=<n> gets=<n> expired=<n>
//!   ERR <message>
//!
//! `key` is the sc-ID string (ASCII, no spaces). It is sent verbatim. Values
//! are opaque blobs (the app's encrypted endpoint payloads) -> hex.

/// Parsed inbound request.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Ping,
    Put { key: Vec<u8>, value: Vec<u8>, ttl_secs: u64 },
    Get { key: Vec<u8> },
    Stats,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    UnknownVerb,
    BadArity,
    BadHex,
    BadTtl,
}

/// Lowercase hex encode.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Decode lowercase/uppercase hex. Returns None on odd length or non-hex.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

/// Parse a single request line.
pub fn parse_request(line: &str) -> Result<Request, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Err(ParseError::Empty);
    }
    let mut it = line.split_whitespace();
    let verb = it.next().ok_or(ParseError::Empty)?;
    match verb {
        "PING" => Ok(Request::Ping),
        "STATS" => Ok(Request::Stats),
        "GET" => {
            let key = it.next().ok_or(ParseError::BadArity)?;
            if it.next().is_some() {
                return Err(ParseError::BadArity);
            }
            Ok(Request::Get { key: key.as_bytes().to_vec() })
        }
        "PUT" => {
            let key = it.next().ok_or(ParseError::BadArity)?;
            let hexval = it.next().ok_or(ParseError::BadArity)?;
            let ttl = it.next().ok_or(ParseError::BadArity)?;
            if it.next().is_some() {
                return Err(ParseError::BadArity);
            }
            let value = hex_decode(hexval).ok_or(ParseError::BadHex)?;
            let ttl_secs: u64 = ttl.parse().map_err(|_| ParseError::BadTtl)?;
            Ok(Request::Put { key: key.as_bytes().to_vec(), value, ttl_secs })
        }
        _ => Err(ParseError::UnknownVerb),
    }
}

/// Build the VALUES response line for a key + its values.
pub fn encode_values(key: &[u8], values: &[Vec<u8>]) -> String {
    let mut s = String::from("VALUES ");
    // key is ASCII sc-ID; send verbatim (lossy guard for safety).
    s.push_str(&String::from_utf8_lossy(key));
    for v in values {
        s.push(' ');
        s.push_str(&hex_encode(v));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let data = vec![0x00, 0xff, 0x10, 0xab, 0x7a];
        assert_eq!(hex_decode(&hex_encode(&data)).unwrap(), data);
    }

    #[test]
    fn hex_rejects_bad() {
        assert!(hex_decode("zz").is_none());
        assert!(hex_decode("abc").is_none()); // odd length
    }

    #[test]
    fn parse_ping_stats() {
        assert_eq!(parse_request("PING").unwrap(), Request::Ping);
        assert_eq!(parse_request("  STATS  ").unwrap(), Request::Stats);
    }

    #[test]
    fn parse_put() {
        let r = parse_request("PUT sc-deadbeef 68656c6c6f 3600").unwrap();
        assert_eq!(
            r,
            Request::Put {
                key: b"sc-deadbeef".to_vec(),
                value: b"hello".to_vec(),
                ttl_secs: 3600,
            }
        );
    }

    #[test]
    fn parse_get() {
        let r = parse_request("GET sc-1").unwrap();
        assert_eq!(r, Request::Get { key: b"sc-1".to_vec() });
    }

    #[test]
    fn parse_errors() {
        assert_eq!(parse_request(""), Err(ParseError::Empty));
        assert_eq!(parse_request("WAT"), Err(ParseError::UnknownVerb));
        assert_eq!(parse_request("GET"), Err(ParseError::BadArity));
        assert_eq!(parse_request("GET a b"), Err(ParseError::BadArity));
        assert_eq!(parse_request("PUT k zz 60"), Err(ParseError::BadHex));
        assert_eq!(parse_request("PUT k 6869 xx"), Err(ParseError::BadTtl));
    }

    #[test]
    fn encode_values_line() {
        let line = encode_values(b"sc-1", &[b"hi".to_vec()]);
        assert_eq!(line, "VALUES sc-1 6869");
        let empty = encode_values(b"sc-1", &[]);
        assert_eq!(empty, "VALUES sc-1");
    }
}
