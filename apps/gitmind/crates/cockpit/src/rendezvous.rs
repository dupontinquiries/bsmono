//! rendezvous — reach a fleet device across networks via a DHT account + advertisement.
//!
//! On a LAN, gitmind finds peers with netshare (mDNS-ish TCP discovery). Off-LAN — a phone
//! on cellular, a desktop behind a different NAT — that fails. This module adds a **DHT
//! rendezvous**: every device has a stable **account** (derived from its identity key) and
//! periodically **advertises** its reachable endpoints (LAN address, public/NAT address,
//! relay) into the DHT under its account key. To reach a device, gitmind resolves its
//! account → advert → the best endpoint to try.
//!
//! The DHT itself is abstracted behind [`AdvertStore`] (snapcore/share-kit provide it
//! on-device); the account derivation, advert record, freshness, and connection strategy
//! are pure and unit-tested here.

use std::time::{SystemTime, UNIX_EPOCH};

/// A stable fleet account id (hex), derived from a device's identity/public key.
pub fn account_id(identity_key: &[u8]) -> String {
    // FNV-1a 64-bit — dependency-free, stable, sufficient for a rendezvous handle.
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in identity_key {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// The DHT key an account advertises under.
pub fn account_key(account: &str) -> String {
    format!("gitmind:fleet:v1:{account}")
}

/// How a device is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// Same-LAN address (only useful when the resolver is on the same network).
    Lan,
    /// Public / NAT-mapped address (works across networks if reachable).
    Public,
    /// A relay/rendezvous node the peer can be reached through when direct fails.
    Relay,
}

impl EndpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EndpointKind::Lan => "lan",
            EndpointKind::Public => "public",
            EndpointKind::Relay => "relay",
        }
    }
    pub fn parse(s: &str) -> Option<EndpointKind> {
        match s {
            "lan" => Some(EndpointKind::Lan),
            "public" => Some(EndpointKind::Public),
            "relay" => Some(EndpointKind::Relay),
            _ => None,
        }
    }
}

/// One reachable endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub kind: EndpointKind,
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn new(kind: EndpointKind, host: &str, port: u16) -> Endpoint {
        Endpoint { kind, host: host.to_string(), port }
    }
    /// `user@host` style target for ssh/scp.
    pub fn ssh_target(&self, user: &str) -> String {
        format!("{user}@{}", self.host)
    }
}

/// A device's advertisement: who it is and how to reach it, with a freshness window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advert {
    pub account: String,
    pub user: String,
    pub hostname: String,
    pub updated_at: u64,
    pub ttl_secs: u64,
    pub endpoints: Vec<Endpoint>,
}

impl Advert {
    /// Whether the advert is still within its TTL at `now` (unix secs).
    pub fn is_fresh(&self, now: u64) -> bool {
        now <= self.updated_at.saturating_add(self.ttl_secs)
    }

    /// Serialize to a single-line record (tab-separated header + comma-separated endpoints).
    pub fn to_record(&self) -> String {
        let eps: Vec<String> =
            self.endpoints.iter().map(|e| format!("{}:{}:{}", e.kind.as_str(), e.host, e.port)).collect();
        format!(
            "gmadv1\t{}\t{}\t{}\t{}\t{}\t{}",
            self.account, self.user, self.hostname, self.updated_at, self.ttl_secs, eps.join(",")
        )
    }

    /// Parse a record produced by [`Advert::to_record`].
    pub fn parse(s: &str) -> Option<Advert> {
        let f: Vec<&str> = s.trim().split('\t').collect();
        if f.len() < 7 || f[0] != "gmadv1" {
            return None;
        }
        let endpoints = f[6]
            .split(',')
            .filter(|p| !p.is_empty())
            .filter_map(|p| {
                // kind:host:port — host may itself be an IPv4 (no colons); split kind, then last = port
                let first = p.find(':')?;
                let kind = EndpointKind::parse(&p[..first])?;
                let rest = &p[first + 1..];
                let last = rest.rfind(':')?;
                let host = &rest[..last];
                let port = rest[last + 1..].parse().ok()?;
                Some(Endpoint { kind, host: host.to_string(), port })
            })
            .collect();
        Some(Advert {
            account: f[1].to_string(),
            user: f[2].to_string(),
            hostname: f[3].to_string(),
            updated_at: f[4].parse().ok()?,
            ttl_secs: f[5].parse().ok()?,
            endpoints,
        })
    }
}

/// True if two IPv4 addresses share a /24 (a cheap "same LAN?" heuristic).
pub fn same_subnet(a: &str, b: &str) -> bool {
    fn prefix(ip: &str) -> Option<String> {
        let mut it = ip.split('.');
        Some(format!("{}.{}.{}", it.next()?, it.next()?, it.next()?))
    }
    match (prefix(a), prefix(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The order to try a peer's endpoints from *our* vantage point. If we're on the same LAN
/// (`same_lan`), LAN addresses come first (fast, direct); otherwise LAN is skipped as
/// unreachable and we go Public → Relay.
pub fn resolve_order(advert: &Advert, same_lan: bool) -> Vec<Endpoint> {
    let mut out = Vec::new();
    if same_lan {
        out.extend(advert.endpoints.iter().filter(|e| e.kind == EndpointKind::Lan).cloned());
    }
    out.extend(advert.endpoints.iter().filter(|e| e.kind == EndpointKind::Public).cloned());
    out.extend(advert.endpoints.iter().filter(|e| e.kind == EndpointKind::Relay).cloned());
    out
}

/// Build our own advertisement from locally-known addresses.
pub fn build_local_advert(
    account: &str,
    user: &str,
    hostname: &str,
    lan: Option<(&str, u16)>,
    public: Option<(&str, u16)>,
    relay: Option<(&str, u16)>,
    ttl_secs: u64,
) -> Advert {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut endpoints = Vec::new();
    if let Some((h, p)) = lan {
        endpoints.push(Endpoint::new(EndpointKind::Lan, h, p));
    }
    if let Some((h, p)) = public {
        endpoints.push(Endpoint::new(EndpointKind::Public, h, p));
    }
    if let Some((h, p)) = relay {
        endpoints.push(Endpoint::new(EndpointKind::Relay, h, p));
    }
    Advert { account: account.to_string(), user: user.to_string(), hostname: hostname.to_string(), updated_at: now, ttl_secs, endpoints }
}

/// The DHT (or any shared store) an advert is published to / resolved from. snapcore's
/// `announce`/`discover` (or share-kit's mailbox) implement this on-device; tests use an
/// in-memory map.
pub trait AdvertStore {
    fn put(&mut self, key: &str, record: &str) -> Result<(), String>;
    fn get(&self, key: &str) -> Option<String>;
}

/// Publish our advert under our account key.
pub fn publish(store: &mut dyn AdvertStore, advert: &Advert) -> Result<(), String> {
    store.put(&account_key(&advert.account), &advert.to_record())
}

/// Resolve a peer account to its advert (if advertised and parseable).
pub fn resolve(store: &dyn AdvertStore, account: &str) -> Option<Advert> {
    Advert::parse(&store.get(&account_key(account))?)
}

/// Pick the best `(user, host, port)` to reach a peer from our vantage. If our LAN ip shares
/// the peer's LAN subnet we prefer the direct LAN address; otherwise we use Public → Relay.
pub fn best_target(advert: &Advert, my_lan_ip: Option<&str>) -> Option<(String, String, u16)> {
    let same_lan = match (my_lan_ip, advert.endpoints.iter().find(|e| e.kind == EndpointKind::Lan)) {
        (Some(mine), Some(lan)) => same_subnet(mine, &lan.host),
        _ => false,
    };
    resolve_order(advert, same_lan).first().map(|e| (advert.user.clone(), e.host.clone(), e.port))
}

/// A file-backed [`AdvertStore`]: one `<dir>/<key>.adv` per account. Works over a shared
/// mount and as an offline cache of DHT results; on-device the DHT store (below) is primary.
pub struct FileStore {
    dir: std::path::PathBuf,
}

impl FileStore {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> FileStore {
        FileStore { dir: dir.into() }
    }
    fn path(&self, key: &str) -> std::path::PathBuf {
        // keys contain ':' — make a filesystem-safe name
        self.dir.join(format!("{}.adv", key.replace([':', '/'], "_")))
    }
}

impl AdvertStore for FileStore {
    fn put(&mut self, key: &str, record: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        std::fs::write(self.path(key), record).map_err(|e| e.to_string())
    }
    fn get(&self, key: &str) -> Option<String> {
        std::fs::read_to_string(self.path(key)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemStore(HashMap<String, String>);
    impl AdvertStore for MemStore {
        fn put(&mut self, key: &str, record: &str) -> Result<(), String> {
            self.0.insert(key.to_string(), record.to_string());
            Ok(())
        }
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[test]
    fn account_is_stable_and_keyed() {
        let a = account_id(b"ssh-ed25519 AAAA...");
        assert_eq!(a, account_id(b"ssh-ed25519 AAAA..."), "deterministic");
        assert_ne!(a, account_id(b"other key"));
        assert_eq!(account_key(&a), format!("gitmind:fleet:v1:{a}"));
    }

    #[test]
    fn advert_record_roundtrips_including_ipv4() {
        let adv = build_local_advert(
            "acct1",
            "user",
            "fp5",
            Some(("192.168.2.223", 22)),
            Some(("203.0.113.7", 22000)),
            Some(("relay.example", 443)),
            600,
        );
        let back = Advert::parse(&adv.to_record()).unwrap();
        assert_eq!(back, adv);
        assert_eq!(back.endpoints[0].host, "192.168.2.223");
        assert_eq!(back.endpoints[1].kind, EndpointKind::Public);
    }

    #[test]
    fn resolve_order_depends_on_vantage() {
        let adv = build_local_advert(
            "a",
            "u",
            "h",
            Some(("192.168.2.223", 22)),
            Some(("203.0.113.7", 22000)),
            Some(("relay", 443)),
            600,
        );
        // same LAN → LAN first
        let same = resolve_order(&adv, true);
        assert_eq!(same[0].kind, EndpointKind::Lan);
        assert_eq!(same.len(), 3);
        // different network → LAN skipped, Public first, then Relay
        let cross = resolve_order(&adv, false);
        assert_eq!(cross[0].kind, EndpointKind::Public);
        assert_eq!(cross[1].kind, EndpointKind::Relay);
        assert!(!cross.iter().any(|e| e.kind == EndpointKind::Lan));
    }

    #[test]
    fn same_subnet_heuristic() {
        assert!(same_subnet("192.168.2.10", "192.168.2.223"));
        assert!(!same_subnet("192.168.2.10", "10.0.0.5"));
    }

    #[test]
    fn freshness_window() {
        let mut adv = build_local_advert("a", "u", "h", Some(("1.2.3.4", 22)), None, None, 300);
        adv.updated_at = 1000;
        assert!(adv.is_fresh(1200));
        assert!(adv.is_fresh(1300));
        assert!(!adv.is_fresh(1301));
    }

    #[test]
    fn publish_then_resolve_via_store() {
        let mut store = MemStore::default();
        let adv = build_local_advert("acctX", "kesslerd", "asahi", Some(("192.168.2.5", 22)), Some(("198.51.100.9", 2222)), None, 600);
        publish(&mut store, &adv).unwrap();
        let got = resolve(&store, "acctX").expect("resolved");
        assert_eq!(got.hostname, "asahi");
        // a desktop off-LAN resolves the phone and gets a public endpoint to try
        let order = resolve_order(&got, false);
        assert_eq!(order[0], Endpoint::new(EndpointKind::Public, "198.51.100.9", 2222));
    }

    #[test]
    fn best_target_picks_by_vantage() {
        let adv = build_local_advert("a", "user", "fp5", Some(("192.168.2.223", 22)), Some(("203.0.113.7", 2222)), None, 600);
        // desktop on the same LAN → direct LAN address
        assert_eq!(best_target(&adv, Some("192.168.2.10")), Some(("user".into(), "192.168.2.223".into(), 22)));
        // desktop on a different network → public address
        assert_eq!(best_target(&adv, Some("10.0.0.4")), Some(("user".into(), "203.0.113.7".into(), 2222)));
        // no vantage → treat as off-LAN
        assert_eq!(best_target(&adv, None), Some(("user".into(), "203.0.113.7".into(), 2222)));
    }

    #[test]
    fn file_store_roundtrips() {
        let dir = std::env::temp_dir().join(format!("gmrdv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = FileStore::new(&dir);
        let adv = build_local_advert("facct", "u", "h", Some(("192.168.1.2", 22)), None, None, 600);
        publish(&mut store, &adv).unwrap();
        assert_eq!(resolve(&store, "facct").unwrap().account, "facct");
        assert!(resolve(&store, "missing").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
