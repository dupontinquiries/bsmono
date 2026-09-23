//! depot-auth — how a client (gitmind, gitmind-egui) writes to a depot
//! without anyone copying an admin token around.
//!
//! The admin token stays depot's bootstrap and emergency key. Everyday
//! writers are **paired clients**: each holds its own ed25519 key, depot
//! holds the public half with a set of scopes, and every write is signed.
//!
//! **Pairing** (once per client): the client sends its name, public key and
//! the scopes it wants; depot files that as pending and both sides derive
//! the same 6-digit code from the key and a depot-chosen nonce. Someone with
//! authority approves it *with the code*, so a key swapped in transit fails:
//! over ssh on the depot host (`depot pair approve <id> --code <code>`,
//! which gitmind runs through its own ssh session, the authority being ssh
//! access), or over HTTP by a client already holding the `approve` scope,
//! or with the admin token.
//!
//! **Signed requests**: `Authorization: Depot-Sig client=<name>,ts=<unix>,
//! nonce=<hex>,sig=<hex>` over [`canonical`] = method, path+query, ts,
//! nonce, content length. Depot checks the key exists, the scope covers the
//! route, the clock is within [`SKEW_S`], and the nonce is new ([`Replay`]).
//! The body is not in the signature: the uploads depot takes are content
//! addressed, and the response names the sha256 it stored, which the client
//! compares against what it sent ([`client_checks_stored_hash`]).
//!
//! Everything here is pure: no sockets, no files, no clock of its own.

use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};

pub const SKEW_S: u64 = 300;
pub const SCHEME: &str = "Depot-Sig";

/// What a client may do. Coarse on purpose: a depot has a handful of write
/// routes, and a scope per route would only be clicked through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// PUT crates, rpms, toolchains, apks, artifacts.
    Ingest,
    /// PUT /api/packs (unpack a pack into this depot).
    Packs,
    /// Approve or deny other clients' pairing requests.
    Approve,
    /// The in-browser terminal.
    Term,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Ingest => "ingest",
            Scope::Packs => "packs",
            Scope::Approve => "approve",
            Scope::Term => "term",
        }
    }
    pub fn parse(s: &str) -> Option<Scope> {
        match s.trim() {
            "ingest" => Some(Scope::Ingest),
            "packs" => Some(Scope::Packs),
            "approve" => Some(Scope::Approve),
            "term" => Some(Scope::Term),
            _ => None,
        }
    }
    pub fn parse_list(s: &str) -> Result<BTreeSet<Scope>, String> {
        s.split(',').filter(|x| !x.trim().is_empty()).map(|x| Scope::parse(x).ok_or_else(|| format!("unknown scope '{}'", x.trim()))).collect()
    }
    pub fn list(set: &BTreeSet<Scope>) -> String {
        set.iter().map(|s| s.name()).collect::<Vec<_>>().join(",")
    }
}

/// Which scope a write route needs. None = not a write route this module
/// guards (reads are open; the admin-only routes stay admin-only).
pub fn scope_for(method: &str, path: &str) -> Option<Scope> {
    let write = matches!(method, "PUT" | "POST" | "DELETE");
    if !write {
        return None;
    }
    if path.starts_with("/api/pair/") && (path.ends_with("/approve") || path.ends_with("/deny")) {
        return Some(Scope::Approve);
    }
    if path.starts_with("/api/packs") {
        return Some(Scope::Packs);
    }
    if path.starts_with("/api/term/") {
        return Some(Scope::Term);
    }
    if ["/api/crates", "/api/rpm/", "/api/toolchains", "/api/apk/", "/api/artifacts/"].iter().any(|p| path.starts_with(p)) {
        return Some(Scope::Ingest);
    }
    None
}

/// Client names: what appears in `keys/clients/<name>.pub`, so a file name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 48 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') && !name.starts_with('.')
}

/// The string a request signature covers.
pub fn canonical(method: &str, path_and_query: &str, ts: u64, nonce: &str, content_length: u64) -> String {
    format!("DEPOT-REQ-1\n{method}\n{path_and_query}\n{ts}\n{nonce}\n{content_length}")
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// A client's identity: its name and the ed25519 seed (32 bytes). The seed
/// file is the client's secret (0600, never sent).
pub struct Identity {
    pub name: String,
    seed: [u8; 32],
}

impl Identity {
    pub fn generate(name: &str) -> Identity {
        let (sk, _) = crypto_kit::ed25519_keypair();
        Identity { name: name.to_string(), seed: *sk }
    }
    pub fn from_seed_hex(name: &str, hex_seed: &str) -> Option<Identity> {
        Some(Identity { name: name.to_string(), seed: unhex::<32>(hex_seed)? })
    }
    pub fn seed_hex(&self) -> String {
        hex(&self.seed)
    }
    pub fn public(&self) -> [u8; 32] {
        crypto_kit::ed25519_public_from_seed(&self.seed)
    }
    pub fn public_hex(&self) -> String {
        hex(&self.public())
    }

    /// The Authorization header value for one request.
    pub fn authorization(&self, method: &str, path_and_query: &str, ts: u64, nonce: &str, content_length: u64) -> String {
        let sig = crypto_kit::sign(&self.seed, canonical(method, path_and_query, ts, nonce, content_length).as_bytes());
        format!("{SCHEME} client={},ts={ts},nonce={nonce},sig={}", self.name, hex(&sig))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigHeader {
    pub client: String,
    pub ts: u64,
    pub nonce: String,
    pub sig: [u8; 64],
}

pub fn parse_header(value: &str) -> Option<SigHeader> {
    let rest = value.trim().strip_prefix(SCHEME)?.trim();
    let mut kv: HashMap<&str, &str> = HashMap::new();
    for part in rest.split(',') {
        let (k, v) = part.split_once('=')?;
        kv.insert(k.trim(), v.trim());
    }
    let nonce = kv.get("nonce")?.to_string();
    if nonce.len() < 16 || nonce.len() > 64 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(SigHeader { client: kv.get("client")?.to_string(), ts: kv.get("ts")?.parse().ok()?, nonce, sig: unhex::<64>(kv.get("sig")?)? })
}

/// A paired client as depot stores it (`keys/clients/<name>.pub`): the
/// public key, then the scopes, one line each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientKey {
    pub name: String,
    pub public: [u8; 32],
    pub scopes: BTreeSet<Scope>,
}

impl ClientKey {
    pub fn to_file(&self) -> String {
        format!("{}\nscopes={}\n", hex(&self.public), Scope::list(&self.scopes))
    }
    pub fn from_file(name: &str, text: &str) -> Option<ClientKey> {
        let mut lines = text.lines();
        let public = unhex::<32>(lines.next()?)?;
        let scopes = lines.find_map(|l| l.strip_prefix("scopes=")).map(Scope::parse_list).unwrap_or(Ok(BTreeSet::new())).ok()?;
        Some(ClientKey { name: name.to_string(), public, scopes })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    Malformed,
    UnknownClient(String),
    Clock { skew: u64 },
    Replay,
    BadSignature,
    Scope { need: Scope, have: String },
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::Malformed => write!(f, "malformed {SCHEME} header"),
            Denied::UnknownClient(c) => write!(f, "client '{c}' is not paired with this depot"),
            Denied::Clock { skew } => write!(f, "request time is {skew} s off this depot's clock (limit {SKEW_S} s): check the client's clock"),
            Denied::Replay => write!(f, "nonce already used (replayed request)"),
            Denied::BadSignature => write!(f, "signature does not verify"),
            Denied::Scope { need, have } => write!(f, "client lacks scope '{}' (has: {have})", need.name()),
        }
    }
}

/// Nonces seen within the clock window. A nonce older than the window can
/// never be accepted again anyway (the ts check refuses it), so entries past
/// 2 x SKEW_S are dropped.
#[derive(Default)]
pub struct Replay {
    seen: HashMap<String, u64>,
}

impl Replay {
    /// True the first time a (client, nonce) is seen.
    pub fn first(&mut self, client: &str, nonce: &str, now: u64) -> bool {
        self.seen.retain(|_, t| now.saturating_sub(*t) <= 2 * SKEW_S);
        self.seen.insert(format!("{client}/{nonce}"), now).is_none()
    }
}

/// Verify one request. `lookup` finds a paired client by name.
#[allow(clippy::too_many_arguments)]
pub fn verify(
    header: &str,
    method: &str,
    path_and_query: &str,
    content_length: u64,
    need: Scope,
    now: u64,
    replay: &mut Replay,
    lookup: impl Fn(&str) -> Option<ClientKey>,
) -> Result<ClientKey, Denied> {
    let h = parse_header(header).ok_or(Denied::Malformed)?;
    let key = lookup(&h.client).ok_or_else(|| Denied::UnknownClient(h.client.clone()))?;
    let skew = now.abs_diff(h.ts);
    if skew > SKEW_S {
        return Err(Denied::Clock { skew });
    }
    if !crypto_kit::verify(&key.public, canonical(method, path_and_query, h.ts, &h.nonce, content_length).as_bytes(), &h.sig) {
        return Err(Denied::BadSignature);
    }
    // Replay is checked after the signature so a forger cannot burn nonces.
    if !replay.first(&h.client, &h.nonce, now) {
        return Err(Denied::Replay);
    }
    if !key.scopes.contains(&need) {
        return Err(Denied::Scope { need, have: Scope::list(&key.scopes) });
    }
    Ok(key)
}

/// A pairing request, as depot files it (`keys/pending/<id>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairRequest {
    pub id: String,
    pub name: String,
    pub public: [u8; 32],
    pub scopes: BTreeSet<Scope>,
    /// Chosen by depot; mixed into the code so a code cannot be precomputed.
    pub nonce: String,
    pub created: u64,
}

/// Pending requests expire; an approval after that must start over.
pub const PAIR_TTL_S: u64 = 15 * 60;

impl PairRequest {
    /// The 6-digit code both sides show. The client computes it from its own
    /// key and the nonce depot returned; depot from what it received. Equal
    /// codes = depot holds the key the client made.
    pub fn code(&self) -> String {
        pair_code(&self.public, &self.nonce)
    }
    pub fn to_file(&self) -> String {
        format!("name={}\npublic={}\nscopes={}\nnonce={}\ncreated={}\n", self.name, hex(&self.public), Scope::list(&self.scopes), self.nonce, self.created)
    }
    pub fn from_file(id: &str, text: &str) -> Option<PairRequest> {
        let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}=")));
        Some(PairRequest {
            id: id.to_string(),
            name: get("name")?.to_string(),
            public: unhex::<32>(get("public")?)?,
            scopes: Scope::parse_list(get("scopes")?).ok()?,
            nonce: get("nonce")?.to_string(),
            created: get("created")?.parse().ok()?,
        })
    }
    pub fn expired(&self, now: u64) -> bool {
        now.saturating_sub(self.created) > PAIR_TTL_S
    }
    /// Approving turns the request into a client key; refused on a wrong
    /// code or an expired request.
    pub fn approve(&self, code: &str, now: u64) -> Result<ClientKey, String> {
        if self.expired(now) {
            return Err(format!("pairing request {} expired; ask the client to pair again", self.id));
        }
        if code.trim() != self.code() {
            return Err(format!("code {} does not match request {} (the client shows the right code; do not approve if they differ)", code.trim(), self.id));
        }
        Ok(ClientKey { name: self.name.clone(), public: self.public, scopes: self.scopes.clone() })
    }
}

pub fn pair_code(public: &[u8; 32], nonce: &str) -> String {
    let d = Sha256::new().chain_update(b"depot-pair-1").chain_update(public).chain_update(nonce.as_bytes()).finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

/// After an ingest, the client checks depot stored the bytes it sent.
pub fn client_checks_stored_hash(sent: &[u8], response_sha256_hex: &str) -> bool {
    hex(&Sha256::digest(sent)) == response_sha256_hex.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired(id: &Identity, scopes: &[Scope]) -> ClientKey {
        ClientKey { name: id.name.clone(), public: id.public(), scopes: scopes.iter().copied().collect() }
    }

    #[test]
    fn a_signed_request_verifies_once_and_only_as_signed() {
        let id = Identity::generate("gitmind-egui-mac");
        let key = paired(&id, &[Scope::Ingest]);
        let look = |n: &str| (n == key.name).then(|| key.clone());
        let mut rp = Replay::default();
        let h = id.authorization("PUT", "/api/apk/fleet", 1000, "0123456789abcdef", 42);
        assert!(verify(&h, "PUT", "/api/apk/fleet", 42, Scope::Ingest, 1010, &mut rp, look).is_ok());
        assert_eq!(verify(&h, "PUT", "/api/apk/fleet", 42, Scope::Ingest, 1010, &mut rp, look), Err(Denied::Replay));
        let h2 = id.authorization("PUT", "/api/apk/fleet", 1000, "1123456789abcdef", 42);
        assert_eq!(verify(&h2, "PUT", "/api/apk/other", 42, Scope::Ingest, 1000, &mut rp, look), Err(Denied::BadSignature), "path is signed");
        assert_eq!(verify(&h2, "PUT", "/api/apk/fleet", 43, Scope::Ingest, 1000, &mut rp, look), Err(Denied::BadSignature), "length is signed");
        assert_eq!(verify(&h2, "POST", "/api/apk/fleet", 42, Scope::Ingest, 1000, &mut rp, look), Err(Denied::BadSignature), "method is signed");
        assert!(matches!(verify(&h2, "PUT", "/api/apk/fleet", 42, Scope::Ingest, 1000 + SKEW_S + 1, &mut rp, look), Err(Denied::Clock { .. })));
        let h3 = id.authorization("PUT", "/api/packs", 1000, "2123456789abcdef", 1);
        assert!(matches!(verify(&h3, "PUT", "/api/packs", 1, Scope::Packs, 1000, &mut rp, look), Err(Denied::Scope { .. })));
        let other = Identity::generate("mallory");
        let h4 = other.authorization("PUT", "/api/apk/fleet", 1000, "3123456789abcdef", 42).replace("mallory", "gitmind-egui-mac");
        assert_eq!(verify(&h4, "PUT", "/api/apk/fleet", 42, Scope::Ingest, 1000, &mut rp, look), Err(Denied::BadSignature), "a different key under the paired name");
        assert!(matches!(verify("Bearer x", "PUT", "/", 0, Scope::Ingest, 0, &mut rp, look), Err(Denied::Malformed)));
        assert!(matches!(verify(&id.authorization("PUT", "/", 0, "short", 0), "PUT", "/", 0, Scope::Ingest, 0, &mut rp, look), Err(Denied::Malformed)), "nonce too short");
    }

    #[test]
    fn a_forged_request_does_not_burn_the_nonce() {
        let id = Identity::generate("c");
        let key = paired(&id, &[Scope::Ingest]);
        let look = |_: &str| Some(key.clone());
        let mut rp = Replay::default();
        let good = id.authorization("PUT", "/api/crates", 5, "aaaaaaaaaaaaaaaa", 1);
        let forged = good.rsplit_once("sig=").map(|(a, _)| format!("{a}sig={}", "00".repeat(64))).unwrap();
        assert_eq!(verify(&forged, "PUT", "/api/crates", 1, Scope::Ingest, 5, &mut rp, look), Err(Denied::BadSignature));
        assert!(verify(&good, "PUT", "/api/crates", 1, Scope::Ingest, 5, &mut rp, look).is_ok());
    }

    #[test]
    fn pairing_codes_match_only_for_the_same_key_and_expire() {
        let id = Identity::generate("gitmind-egui");
        let req = PairRequest { id: "p1".into(), name: id.name.clone(), public: id.public(), scopes: [Scope::Ingest, Scope::Approve].into(), nonce: "n0nce".into(), created: 100 };
        let client_side = pair_code(&id.public(), "n0nce");
        assert_eq!(req.code(), client_side);
        assert_eq!(client_side.len(), 6);
        let swapped = PairRequest { public: Identity::generate("x").public(), ..req.clone() };
        assert_ne!(swapped.code(), client_side, "a key swapped in transit shows a different code");
        assert!(swapped.approve(&client_side, 200).is_err());
        let k = req.approve(&client_side, 200).unwrap();
        assert_eq!(k.scopes, [Scope::Ingest, Scope::Approve].into());
        assert!(req.approve(&client_side, 100 + PAIR_TTL_S + 1).unwrap_err().contains("expired"));
        assert_eq!(PairRequest::from_file("p1", &req.to_file()).unwrap(), req);
        assert_eq!(ClientKey::from_file(&k.name, &k.to_file()).unwrap(), k);
    }

    #[test]
    fn routes_map_to_scopes_and_names_are_file_safe() {
        assert_eq!(scope_for("PUT", "/api/crates"), Some(Scope::Ingest));
        assert_eq!(scope_for("PUT", "/api/rpm/fedora"), Some(Scope::Ingest));
        assert_eq!(scope_for("PUT", "/api/packs"), Some(Scope::Packs));
        assert_eq!(scope_for("POST", "/api/pair/p1/approve"), Some(Scope::Approve));
        assert_eq!(scope_for("POST", "/api/term/open"), Some(Scope::Term));
        assert_eq!(scope_for("GET", "/api/crates"), None, "reads are open");
        assert!(valid_name("gitmind-egui_mac.1") && !valid_name("../x") && !valid_name(".hidden") && !valid_name(""));
        let id = Identity::from_seed_hex("a", &"11".repeat(32)).unwrap();
        assert_eq!(Identity::from_seed_hex("a", &id.seed_hex()).unwrap().public(), id.public());
        assert!(client_checks_stored_hash(b"abc", "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"));
    }
}
