//! auth.rs — LAN shared-token authentication, the posture netserve and the
//! logscope fleet already use: authenticate senders so a stray box on the LAN
//! can't inject traffic. It does NOT encrypt or resist replay — run on a trusted
//! LAN or an SSH tunnel. An empty token / empty keyring means OPEN (no auth),
//! only for fully trusted setups; callers should warn when starting open.
//!
//! Lifted as the union of two byte-identical copies: netserve's
//! `ct_eq`/`generate_key`/`Keyring` and the logscope enlist `ct_eq`/`frame`/`unframe`.
//! Std only (OS entropy via `/dev/urandom`).

use std::io::Read;

/// Constant-time byte-slice equality — no early return on the first mismatch, so
/// a timing observer can't recover a token byte by byte.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// A random 32-byte token as a 64-char hex string. Uses OS entropy
/// (`/dev/urandom`); falls back to a time/pid mix only if that's unavailable
/// (weaker — note it in logs). For cryptographic keys use `crypto-kit`; this is
/// for LAN bearer tokens.
pub fn generate_key() -> String {
    let mut buf = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return core_kit::hex::encode(&buf);
        }
    }
    let mut s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64).rotate_left(17);
    for b in buf.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *b = s as u8;
    }
    core_kit::hex::encode(&buf)
}

/// The set of accepted tokens. Empty keyring = OPEN (no auth).
#[derive(Clone, Default)]
pub struct Keyring {
    keys: Vec<Vec<u8>>,
}

impl Keyring {
    /// Build from token strings; empty strings are dropped.
    pub fn new(keys: Vec<String>) -> Self {
        Self {
            keys: keys
                .into_iter()
                .filter(|k| !k.is_empty())
                .map(|k| k.into_bytes())
                .collect(),
        }
    }
    /// Open keyring (no tokens) accepts everything — for fully trusted setups.
    pub fn is_open(&self) -> bool {
        self.keys.is_empty()
    }
    /// Constant-time membership: checks every key (so timing doesn't reveal which
    /// matched), and accepts anything when open.
    pub fn accepts(&self, token: &[u8]) -> bool {
        if self.is_open() {
            return true;
        }
        let mut ok = false;
        for k in &self.keys {
            ok |= ct_eq(k, token);
        }
        ok
    }
}

/// Prefix an encoded line with a shared token: `"<token>\t<line>"`. Pairs with
/// [`unframe`]. An empty token frames as a leading tab (still parses).
pub fn frame(token: &str, line: &str) -> String {
    format!("{token}\t{line}")
}

/// Verify (constant-time) and strip the token, returning the inner line — or
/// `None` if it doesn't match.
pub fn unframe(token: &str, framed: &str) -> Option<String> {
    let (tok, rest) = framed.split_once('\t')?;
    if ct_eq(tok.as_bytes(), token.as_bytes()) {
        Some(rest.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_and_rejects() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"ab", b"abc")); // length mismatch
    }

    #[test]
    fn frame_unframe_roundtrip_and_reject_bad_token() {
        let line = "ASSIGN\tpeer\t5\tpayload";
        let f = frame("s3cret", line);
        assert_eq!(unframe("s3cret", &f).as_deref(), Some(line), "right token strips back");
        assert_eq!(unframe("wrong", &f), None, "wrong token rejected");
        // empty token = open (no auth)
        assert_eq!(unframe("", &frame("", line)).as_deref(), Some(line));
    }

    #[test]
    fn keyring_accepts_members_and_is_open_when_empty() {
        let kr = Keyring::new(vec!["k1".into(), "k2".into(), String::new()]);
        assert!(kr.accepts(b"k1") && kr.accepts(b"k2"));
        assert!(!kr.accepts(b"nope"));
        assert!(!kr.is_open(), "the empty string was dropped, two real keys remain");
        let open = Keyring::default();
        assert!(open.is_open() && open.accepts(b"anything"));
    }

    #[test]
    fn generate_key_is_64_hex_and_varies() {
        let a = generate_key();
        let b = generate_key();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two keys should not collide");
    }
}
