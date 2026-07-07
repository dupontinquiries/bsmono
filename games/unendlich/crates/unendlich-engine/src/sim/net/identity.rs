//! unendlich-id identity, per-world allowlisting, and the join **mailbox**.
//!
//! Pure data + a thin file layer; no sockets. The UDP [`super::transport`] and
//! the in-game UI sit on top. This is the trust surface for "who is allowed into
//! my world," so it's kept small and fully unit-tested.
//!
//! Model (matching the design):
//! - An [`UnendlichId`] is a shareable handle. You give it to people; they add it
//!   to their [`Subscribers`] and your world's server shows up for them. It never
//!   embeds your IP — the address is only learned after a join is **accepted**.
//! - Whitelisting is **on by default**. An id already in the per-world
//!   [`Whitelist`] (`whitelist.txt`) joins directly; everyone else lands in the
//!   [`Mailbox`] as a *pending request* and the host's address is withheld until
//!   the host accepts ([`JoinRequest::address_if_accepted`] returns `None` while
//!   pending). That's the public-IP protection: a request never leaks the address.
//! - Names are per-world (`names.txt`). The host approves a requested name or is
//!   renamed on conflict ([`NameRegistry::approve`]); the assigned name is what
//!   the UI tags with the "renamed" graphic.
//! - Everything per-world is loaded from the running world's save dir, so a host
//!   running splitscreen / multiple local players can't have a *global* whitelist
//!   — it's always scoped to whichever world is live.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

const WHITELIST_FILE: &str = "whitelist.txt";
const NAMES_FILE: &str = "names.txt";
const SUBSCRIBERS_FILE: &str = "subscribers.txt";

/// A shareable identity handle. Normalized to uppercase alphanumerics grouped in
/// 4s (e.g. `K7Q2-9F1A-ZX30`). Opaque: resolution to an address is the network
/// layer's job, so the raw IP is never part of the id.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct UnendlichId(String);

impl UnendlichId {
    /// Normalize arbitrary input (strip separators/whitespace, uppercase). Does
    /// not validate length — use [`UnendlichId::is_valid`] for that.
    pub fn new(s: impl AsRef<str>) -> Self {
        let raw: String = s
            .as_ref()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        UnendlichId(raw)
    }

    /// Canonical id is 12 base-36 chars (excluding easily-confused 0/O/1/I is a
    /// future nicety; for now any alphanumeric of the right length is valid).
    pub fn is_valid(&self) -> bool {
        self.0.len() == 12 && self.0.chars().all(|c| c.is_ascii_alphanumeric())
    }

    /// Bare normalized form (no separators) — what's stored in files.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Human-friendly grouped form (`XXXX-XXXX-XXXX`) for display / sharing.
    pub fn grouped(&self) -> String {
        self.0
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Deterministically derive an id from a seed (e.g. a per-instance random
    /// u64), so a fresh install can mint a stable handle without a coordinator.
    pub fn from_seed(mut seed: u64) -> Self {
        const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no 0/O/1/I
        let mut out = String::with_capacity(12);
        for _ in 0..12 {
            // xorshift step keeps it dependency-free and well-mixed.
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            out.push(ALPHABET[(seed % ALPHABET.len() as u64) as usize] as char);
        }
        UnendlichId(out)
    }
}

/// Per-world allowlist backed by `whitelist.txt` (one normalized id per line;
/// `#` comments and blank lines ignored). Whitelisting is on by default, so a
/// missing file means "nobody auto-joins" — everyone goes through the mailbox.
#[derive(Default, Clone)]
pub struct Whitelist {
    ids: BTreeSet<UnendlichId>,
}

impl Whitelist {
    pub fn load(world_dir: &Path) -> Self {
        let mut ids = BTreeSet::new();
        if let Ok(text) = std::fs::read_to_string(world_dir.join(WHITELIST_FILE)) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                ids.insert(UnendlichId::new(line));
            }
        }
        Whitelist { ids }
    }

    pub fn save(&self, world_dir: &Path) -> io::Result<()> {
        let mut s = String::from("# unendlich whitelist — one id per line\n");
        for id in &self.ids {
            s.push_str(id.as_str());
            s.push('\n');
        }
        std::fs::write(world_dir.join(WHITELIST_FILE), s)
    }

    pub fn allows(&self, id: &UnendlichId) -> bool {
        self.ids.contains(id)
    }
    pub fn add(&mut self, id: UnendlichId) {
        self.ids.insert(id);
    }
    pub fn remove(&mut self, id: &UnendlichId) {
        self.ids.remove(id);
    }
    pub fn iter(&self) -> impl Iterator<Item = &UnendlichId> {
        self.ids.iter()
    }
}

/// Per-world id→name map backed by `names.txt` (`id<TAB>name`). The host approves
/// requested names here; on conflict the joiner is renamed.
#[derive(Default, Clone)]
pub struct NameRegistry {
    names: BTreeMap<UnendlichId, String>,
}

impl NameRegistry {
    pub fn load(world_dir: &Path) -> Self {
        let mut names = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(world_dir.join(NAMES_FILE)) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((id, name)) = line.split_once('\t') {
                    names.insert(UnendlichId::new(id), name.trim().to_string());
                }
            }
        }
        NameRegistry { names }
    }

    pub fn save(&self, world_dir: &Path) -> io::Result<()> {
        let mut s = String::from("# unendlich approved names — id<TAB>name\n");
        for (id, name) in &self.names {
            s.push_str(id.as_str());
            s.push('\t');
            s.push_str(name);
            s.push('\n');
        }
        std::fs::write(world_dir.join(NAMES_FILE), s)
    }

    pub fn name_for(&self, id: &UnendlichId) -> Option<&str> {
        self.names.get(id).map(String::as_str)
    }

    /// Approve `requested` for `id`, renaming on conflict with a numeric suffix.
    /// Returns `(assigned_name, was_renamed)` — `was_renamed` drives the UI's
    /// "renamed" graphic.
    pub fn approve(&mut self, id: &UnendlichId, requested: &str) -> (String, bool) {
        let base = sanitize_name(requested);
        // Free if unused, or already held by this same id.
        let taken = |n: &str, names: &BTreeMap<UnendlichId, String>| {
            names.iter().any(|(k, v)| v == n && k != id)
        };
        let mut name = base.clone();
        let mut renamed = false;
        let mut n = 2;
        while taken(&name, &self.names) {
            name = format!("{base}{n}");
            renamed = true;
            n += 1;
        }
        self.names.insert(id.clone(), name.clone());
        (name, renamed)
    }
}

fn sanitize_name(s: &str) -> String {
    let cleaned: String = s
        .trim()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .take(24)
        .collect();
    if cleaned.is_empty() {
        "player".to_string()
    } else {
        cleaned
    }
}

/// State of a join request in the mailbox.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReqState {
    Pending,
    Accepted { assigned_name: String },
    Rejected,
}

/// A single join request. The requester's address is private and only surfaced
/// once accepted, so a pending request can be shown/logged without ever leaking
/// the host's or joiner's IP.
#[derive(Clone, Debug)]
pub struct JoinRequest {
    pub id: UnendlichId,
    pub requested_name: String,
    addr: String,
    state: ReqState,
}

impl JoinRequest {
    pub fn state(&self) -> &ReqState {
        &self.state
    }
    /// The peer address, but only after acceptance — `None` while pending or
    /// rejected. This is the IP-protection invariant.
    pub fn address_if_accepted(&self) -> Option<&str> {
        matches!(self.state, ReqState::Accepted { .. }).then_some(self.addr.as_str())
    }
}

/// The host's per-world mailbox of join requests. Whitelisting on by default:
/// an already-whitelisted id is accepted immediately; everyone else waits here.
#[derive(Default)]
pub struct Mailbox {
    reqs: Vec<JoinRequest>,
}

impl Mailbox {
    /// A joiner asks to enter. If already whitelisted, it's accepted right away
    /// (name approved + address released); otherwise it's parked as Pending and
    /// the address stays hidden until the host calls [`Mailbox::accept`].
    pub fn submit(
        &mut self,
        id: UnendlichId,
        requested_name: &str,
        addr: &str,
        wl: &Whitelist,
        nr: &mut NameRegistry,
    ) -> ReqState {
        let state = if wl.allows(&id) {
            let (assigned_name, _) = nr.approve(&id, requested_name);
            ReqState::Accepted { assigned_name }
        } else {
            ReqState::Pending
        };
        // Replace any prior request from the same id.
        self.reqs.retain(|r| r.id != id);
        self.reqs.push(JoinRequest {
            id,
            requested_name: requested_name.to_string(),
            addr: addr.to_string(),
            state: state.clone(),
        });
        state
    }

    /// Host accepts a pending id: whitelist it, approve a name, release the
    /// address. Returns the assigned name, or `None` if no such pending request.
    pub fn accept(
        &mut self,
        id: &UnendlichId,
        wl: &mut Whitelist,
        nr: &mut NameRegistry,
    ) -> Option<String> {
        let req = self.reqs.iter_mut().find(|r| &r.id == id)?;
        let (assigned_name, _) = nr.approve(id, &req.requested_name);
        wl.add(id.clone());
        req.state = ReqState::Accepted {
            assigned_name: assigned_name.clone(),
        };
        Some(assigned_name)
    }

    pub fn reject(&mut self, id: &UnendlichId) {
        if let Some(req) = self.reqs.iter_mut().find(|r| &r.id == id) {
            req.state = ReqState::Rejected;
        }
    }

    pub fn pending(&self) -> impl Iterator<Item = &JoinRequest> {
        self.reqs
            .iter()
            .filter(|r| r.state == ReqState::Pending)
    }
    pub fn all(&self) -> impl Iterator<Item = &JoinRequest> {
        self.reqs.iter()
    }
}

/// Per-instance list of ids you follow (their worlds' servers show up). Lives in
/// the config dir, NOT a world dir — it's *your* subscriptions, independent of
/// which local world is running.
#[derive(Default, Clone)]
pub struct Subscribers {
    ids: Vec<UnendlichId>,
}

impl Subscribers {
    pub fn load(config_dir: &Path) -> Self {
        let mut ids = Vec::new();
        if let Ok(text) = std::fs::read_to_string(config_dir.join(SUBSCRIBERS_FILE)) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let id = UnendlichId::new(line);
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Subscribers { ids }
    }
    pub fn save(&self, config_dir: &Path) -> io::Result<()> {
        let _ = std::fs::create_dir_all(config_dir);
        let mut s = String::from("# unendlich subscriptions — ids whose servers you follow\n");
        for id in &self.ids {
            s.push_str(id.as_str());
            s.push('\n');
        }
        std::fs::write(config_dir.join(SUBSCRIBERS_FILE), s)
    }
    pub fn add(&mut self, id: UnendlichId) {
        if !self.ids.contains(&id) {
            self.ids.push(id);
        }
    }
    pub fn remove(&mut self, id: &UnendlichId) {
        self.ids.retain(|x| x != id);
    }
    pub fn iter(&self) -> impl Iterator<Item = &UnendlichId> {
        self.ids.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("ue_id_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn id_normalizes_and_groups() {
        let a = UnendlichId::new("k7q2-9f1a zx30");
        assert_eq!(a.as_str(), "K7Q29F1AZX30");
        assert_eq!(a.grouped(), "K7Q2-9F1A-ZX30");
        assert!(a.is_valid());
        assert!(!UnendlichId::new("short").is_valid());
    }

    #[test]
    fn from_seed_is_stable_and_valid() {
        let a = UnendlichId::from_seed(0xDEAD_BEEF);
        let b = UnendlichId::from_seed(0xDEAD_BEEF);
        assert_eq!(a, b);
        assert!(a.is_valid());
        assert_ne!(a, UnendlichId::from_seed(1));
    }

    #[test]
    fn whitelist_roundtrips_per_world() {
        let dir = tmp("wl");
        let mut wl = Whitelist::load(&dir);
        assert!(wl.iter().next().is_none()); // missing => empty (on by default)
        let id = UnendlichId::from_seed(7);
        wl.add(id.clone());
        wl.save(&dir).unwrap();
        let wl2 = Whitelist::load(&dir);
        assert!(wl2.allows(&id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_approve_and_rename_on_conflict() {
        let mut nr = NameRegistry::default();
        let a = UnendlichId::from_seed(1);
        let b = UnendlichId::from_seed(2);
        assert_eq!(nr.approve(&a, "Aang"), ("Aang".to_string(), false));
        let (name, renamed) = nr.approve(&b, "Aang");
        assert!(renamed);
        assert_eq!(name, "Aang2");
    }

    #[test]
    fn mailbox_withholds_address_until_accepted() {
        let mut wl = Whitelist::default();
        let mut nr = NameRegistry::default();
        let mut mb = Mailbox::default();
        let joiner = UnendlichId::from_seed(42);

        // Not whitelisted -> pending, address hidden.
        let st = mb.submit(joiner.clone(), "Toph", "203.0.113.5:7777", &wl, &mut nr);
        assert_eq!(st, ReqState::Pending);
        let req = mb.all().next().unwrap();
        assert_eq!(req.address_if_accepted(), None);

        // Host accepts -> whitelisted, named, address now available.
        let assigned = mb.accept(&joiner, &mut wl, &mut nr).unwrap();
        assert_eq!(assigned, "Toph");
        assert!(wl.allows(&joiner));
        let req = mb.all().next().unwrap();
        assert_eq!(req.address_if_accepted(), Some("203.0.113.5:7777"));
    }

    #[test]
    fn whitelisted_joiner_skips_the_mailbox() {
        let mut wl = Whitelist::default();
        let mut nr = NameRegistry::default();
        let mut mb = Mailbox::default();
        let friend = UnendlichId::from_seed(99);
        wl.add(friend.clone());
        let st = mb.submit(friend.clone(), "Katara", "198.51.100.2:7777", &wl, &mut nr);
        assert!(matches!(st, ReqState::Accepted { .. }));
        // Already-allowed join gets its address immediately.
        assert!(mb.all().next().unwrap().address_if_accepted().is_some());
    }

    #[test]
    fn subscribers_dedup_and_roundtrip() {
        let dir = tmp("sub");
        let mut subs = Subscribers::default();
        let id = UnendlichId::from_seed(5);
        subs.add(id.clone());
        subs.add(id.clone()); // dedup
        subs.save(&dir).unwrap();
        let subs2 = Subscribers::load(&dir);
        assert_eq!(subs2.iter().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
