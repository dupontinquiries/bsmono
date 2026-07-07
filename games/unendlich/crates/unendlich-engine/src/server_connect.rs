//! server_connect.rs — pure server address parsing/validation.
//!
//! Parse and validate a `host[:port]` server address, applying a default port,
//! plus an allow-list check helper. Pure, deterministic, panic-free
//! (R1/R11/R21). Self-contained: the allow-list check takes a slice of allowed
//! hosts (the coordinator passes `Whitelist::entries()`), so this module has no
//! crate-internal dependencies and unit-tests standalone.

/// Default port applied when an address omits one.
pub const DEFAULT_PORT: u16 = 25565;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ServerAddr {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddrError {
    EmptyHost,
    BadPort,
    TooManyColons,
}

/// Conservative host check: non-empty, ASCII letters/digits/`.`/`-` only.
/// (No DNS, no IPv6 literals — those are out of scope for the wizard field.)
fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Parse `host` or `host:port`, applying [`DEFAULT_PORT`] when the port is
/// omitted. Rejects empty/invalid hosts, ports outside `1..=65535`, and inputs
/// with more than one colon.
pub fn parse(input: &str) -> Result<ServerAddr, AddrError> {
    let s = input.trim();
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [host] => {
            if host_ok(host) {
                Ok(ServerAddr { host: (*host).to_string(), port: DEFAULT_PORT })
            } else {
                Err(AddrError::EmptyHost)
            }
        }
        [host, port_str] => {
            if !host_ok(host) {
                return Err(AddrError::EmptyHost);
            }
            match port_str.parse::<u16>() {
                Ok(p) if p > 0 => Ok(ServerAddr { host: (*host).to_string(), port: p }),
                _ => Err(AddrError::BadPort),
            }
        }
        _ => Err(AddrError::TooManyColons),
    }
}

impl ServerAddr {
    /// `host:port` string form. `parse` of this reproduces the same address.
    pub fn to_string_addr(&self) -> String {
        let mut s = self.host.clone();
        s.push(':');
        s.push_str(&self.port.to_string());
        s
    }

    /// Whether this address's host appears in `allowed` (e.g. a whitelist's
    /// sorted entries).
    pub fn is_allowed(&self, allowed: &[String]) -> bool {
        allowed.iter().any(|h| h == &self.host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(host: &str, port: u16) -> ServerAddr {
        ServerAddr { host: host.to_string(), port }
    }

    #[test]
    fn default_port_applied_when_omitted() {
        assert_eq!(parse("mc.server.net"), Ok(addr("mc.server.net", DEFAULT_PORT)));
    }

    #[test]
    fn explicit_port_parsed() {
        assert_eq!(parse("10.0.0.5:8080"), Ok(addr("10.0.0.5", 8080)));
        assert_eq!(parse("  localhost:1  "), Ok(addr("localhost", 1)));
    }

    #[test]
    fn bad_addresses_rejected() {
        assert_eq!(parse(""), Err(AddrError::EmptyHost));
        assert_eq!(parse(":25565"), Err(AddrError::EmptyHost));
        assert_eq!(parse("host:0"), Err(AddrError::BadPort));
        assert_eq!(parse("host:99999"), Err(AddrError::BadPort));
        assert_eq!(parse("host:abc"), Err(AddrError::BadPort));
        assert_eq!(parse("a:b:c"), Err(AddrError::TooManyColons));
        assert_eq!(parse("bad host!"), Err(AddrError::EmptyHost));
    }

    #[test]
    fn prop_round_trip_to_string_and_back() {
        for s in ["a.b.c:1234", "localhost:25565", "1.2.3.4:1"] {
            match parse(s) {
                Ok(a) => assert_eq!(parse(&a.to_string_addr()), Ok(a), "round-trip failed for {}", s),
                Err(_) => assert!(false, "{} should parse", s),
            }
        }
    }

    #[test]
    fn whitelist_check_helper() {
        let allowed = vec!["good.host".to_string(), "trusted.example.com".to_string()];
        match parse("trusted.example.com:25565") {
            Ok(a) => assert!(a.is_allowed(&allowed)),
            Err(_) => assert!(false, "should parse"),
        }
        match parse("evil.host") {
            Ok(a) => assert!(!a.is_allowed(&allowed)),
            Err(_) => assert!(false, "should parse"),
        }
    }

    #[test]
    fn prop_deterministic() {
        assert_eq!(parse("host:1"), parse("host:1"));
    }
}
