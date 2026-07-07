//! Two-sided DHT mailbox: the serverless rendezvous for joins.
//!
//! No central coordinator. Every [`UnendlichId`] owns a mailbox at a
//! DHT key `H(id)`. Anyone can *drop* a message into any mailbox (`put`); the
//! owner *drains* its own (`get`). A join is two exchanges between two mailboxes:
//!
//! ```text
//!   Requester R                DHT                 Host H
//!   ----------                 ---                 ------
//!   put H(H_id)  ── Request{from:R,name} ─────────▶  (drained by H)
//!                                                      decide:
//!   (drained by R) ◀──── Accept{addr} / Deny ──  put H(R_id)
//! ```
//!
//! **IP protection** is structural: the host's address travels *only* inside an
//! `Accept`, which is only placed into the requester's mailbox after the host
//! accepts. A `Pending` request carries no address; a `Deny` carries none. So a
//! requester who is never accepted never learns the host's IP — matching the
//! mailbox invariant in [`super::identity`], now across the network.
//!
//! Layering mirrors [`super::transport`]: a [`MailboxDht`] trait with an
//! in-memory [`LocalDht`] for tests/local play; the real Kademlia-over-UDP node
//! implements the same trait later. Wire (de)serialization is hand-rolled and
//! bounds-checked — it parses untrusted bytes and must never panic.
//!
//! Hardening to layer on (documented, not yet implemented): messages should be
//! **signed** by the sender's id-key (mailboxes are world-writable, so spam/
//! spoofing is possible), and the `addr` in an `Accept` should be **sealed** to
//! the requester's public key so DHT nodes relaying it can't harvest IPs. The
//! `UnendlichId` is the natural place to carry that public key.

use super::identity::{Mailbox, NameRegistry, ReqState, UnendlichId, Whitelist};

/// A 32-byte DHT key. Production derives this with SHA-256; here it's a
/// dependency-free mix that's stable and well-distributed enough for the
/// protocol layer and its tests.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MailboxKey(pub [u8; 32]);

/// Mailbox key for an id: `H(id)`.
pub fn mailbox_key(id: &UnendlichId) -> MailboxKey {
    // FNV-1a expanded across 4 lanes with distinct offsets.
    let bytes = id.as_str().as_bytes();
    let mut out = [0u8; 32];
    for lane in 0..4u64 {
        let mut h: u64 = 0xcbf29ce484222325 ^ lane.wrapping_mul(0x100000001b3);
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        out[(lane as usize) * 8..(lane as usize) * 8 + 8].copy_from_slice(&h.to_le_bytes());
    }
    MailboxKey(out)
}

/// A message dropped into a mailbox.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MailMsg {
    /// Requester -> host's mailbox.
    Request { from: UnendlichId, requested_name: String },
    /// Host -> requester's mailbox. Carries the host address (the only place an
    /// address ever appears) — seal this field to the requester in production.
    Accept { from: UnendlichId, assigned_name: String, addr: String },
    /// Host -> requester's mailbox. No address.
    Deny { from: UnendlichId },
}

const TAG_REQUEST: u8 = 1;
const TAG_ACCEPT: u8 = 2;
const TAG_DENY: u8 = 3;
const MAX_FIELD: usize = 256; // bound every length-prefixed field

impl MailMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        match self {
            MailMsg::Request { from, requested_name } => {
                o.push(TAG_REQUEST);
                put_str(&mut o, from.as_str());
                put_str(&mut o, requested_name);
            }
            MailMsg::Accept { from, assigned_name, addr } => {
                o.push(TAG_ACCEPT);
                put_str(&mut o, from.as_str());
                put_str(&mut o, assigned_name);
                put_str(&mut o, addr);
            }
            MailMsg::Deny { from } => {
                o.push(TAG_DENY);
                put_str(&mut o, from.as_str());
            }
        }
        o
    }

    /// Parse one message. Returns `None` on any malformed/oversized input — never
    /// panics, never allocates beyond the (bounded) field lengths.
    pub fn decode(b: &[u8]) -> Option<MailMsg> {
        let mut p = 0usize;
        let tag = *b.first()?;
        p += 1;
        match tag {
            TAG_REQUEST => {
                let from = UnendlichId::new(take_str(b, &mut p)?);
                let requested_name = take_str(b, &mut p)?;
                Some(MailMsg::Request { from, requested_name })
            }
            TAG_ACCEPT => {
                let from = UnendlichId::new(take_str(b, &mut p)?);
                let assigned_name = take_str(b, &mut p)?;
                let addr = take_str(b, &mut p)?;
                Some(MailMsg::Accept { from, assigned_name, addr })
            }
            TAG_DENY => {
                let from = UnendlichId::new(take_str(b, &mut p)?);
                Some(MailMsg::Deny { from })
            }
            _ => None,
        }
    }
}

fn put_str(o: &mut Vec<u8>, s: &str) {
    let n = s.len().min(MAX_FIELD);
    o.extend_from_slice(&(n as u16).to_le_bytes());
    o.extend_from_slice(&s.as_bytes()[..n]);
}

fn take_str(b: &[u8], p: &mut usize) -> Option<String> {
    let lo = *b.get(*p)? as usize;
    let hi = *b.get(*p + 1)? as usize;
    let n = lo | (hi << 8);
    if n > MAX_FIELD {
        return None;
    }
    let start = *p + 2;
    let end = start.checked_add(n)?;
    let slice = b.get(start..end)?;
    *p = end;
    Some(String::from_utf8_lossy(slice).into_owned())
}

/// The networked rendezvous medium. `put` drops a message into the mailbox at
/// `key`; `get` atomically drains and returns the messages waiting there.
pub trait MailboxDht {
    fn put(&self, key: MailboxKey, msg: Vec<u8>);
    fn get(&self, key: MailboxKey) -> Vec<Vec<u8>>;
}

/// In-memory DHT for tests and same-process/local play (the loopback analogue of
/// [`super::transport::LoopTransport`]).
#[derive(Default)]
pub struct LocalDht {
    boxes: std::sync::Mutex<std::collections::HashMap<MailboxKey, Vec<Vec<u8>>>>,
}

impl MailboxDht for LocalDht {
    fn put(&self, key: MailboxKey, msg: Vec<u8>) {
        self.boxes.lock().unwrap().entry(key).or_default().push(msg);
    }
    fn get(&self, key: MailboxKey) -> Vec<Vec<u8>> {
        self.boxes
            .lock()
            .unwrap()
            .get_mut(&key)
            .map(std::mem::take)
            .unwrap_or_default()
    }
}

/// Outcome a requester learns by polling its own mailbox.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum JoinOutcome {
    Accepted { assigned_name: String, addr: String },
    Denied,
}

// ── Requester side ──────────────────────────────────────────────────────────

/// Drop a join request into the host's mailbox.
pub fn send_request<D: MailboxDht>(
    dht: &D,
    host_id: &UnendlichId,
    my_id: &UnendlichId,
    requested_name: &str,
) {
    let msg = MailMsg::Request {
        from: my_id.clone(),
        requested_name: requested_name.to_string(),
    };
    dht.put(mailbox_key(host_id), msg.encode());
}

/// Poll my mailbox for the host's response (drains it). Returns the first
/// Accept/Deny found; ignores stray Request messages.
pub fn poll_response<D: MailboxDht>(dht: &D, my_id: &UnendlichId) -> Option<JoinOutcome> {
    for raw in dht.get(mailbox_key(my_id)) {
        match MailMsg::decode(&raw) {
            Some(MailMsg::Accept { assigned_name, addr, .. }) => {
                return Some(JoinOutcome::Accepted { assigned_name, addr })
            }
            Some(MailMsg::Deny { .. }) => return Some(JoinOutcome::Denied),
            _ => {}
        }
    }
    None
}

// ── Host side ───────────────────────────────────────────────────────────────

/// Drain my mailbox and fold incoming requests into the local [`Mailbox`]. An
/// already-whitelisted requester is auto-accepted: an `Accept` (with `my_addr`)
/// is dropped into their mailbox immediately. Unknown requesters become
/// `Pending` and wait for [`host_accept`]/[`host_deny`] — their request never
/// carried, and never receives, an address until then.
pub fn host_drain_requests<D: MailboxDht>(
    dht: &D,
    my_id: &UnendlichId,
    my_addr: &str,
    mailbox: &mut Mailbox,
    wl: &Whitelist,
    nr: &mut NameRegistry,
) {
    for raw in dht.get(mailbox_key(my_id)) {
        if let Some(MailMsg::Request { from, requested_name }) = MailMsg::decode(&raw) {
            // submit() auto-accepts whitelisted ids, else parks Pending. We pass
            // the requester's mailbox key as the "address" so we can reply; the
            // host's real address is only ever sent in the Accept below.
            let reply_addr = format!("dht:{}", from.as_str());
            let state = mailbox.submit(from.clone(), &requested_name, &reply_addr, wl, nr);
            if let ReqState::Accepted { assigned_name } = state {
                let accept = MailMsg::Accept {
                    from: my_id.clone(),
                    assigned_name,
                    addr: my_addr.to_string(),
                };
                dht.put(mailbox_key(&from), accept.encode());
            }
        }
    }
}

/// Host accepts a pending requester: whitelist + name them, then drop an Accept
/// (carrying `my_addr`) into their mailbox. Returns the assigned name.
pub fn host_accept<D: MailboxDht>(
    dht: &D,
    my_id: &UnendlichId,
    requester: &UnendlichId,
    my_addr: &str,
    mailbox: &mut Mailbox,
    wl: &mut Whitelist,
    nr: &mut NameRegistry,
) -> Option<String> {
    let assigned_name = mailbox.accept(requester, wl, nr)?;
    let accept = MailMsg::Accept {
        from: my_id.clone(),
        assigned_name: assigned_name.clone(),
        addr: my_addr.to_string(),
    };
    dht.put(mailbox_key(requester), accept.encode());
    Some(assigned_name)
}

/// Host denies a requester: mark rejected locally and drop a Deny (no address)
/// into their mailbox.
pub fn host_deny<D: MailboxDht>(
    dht: &D,
    my_id: &UnendlichId,
    requester: &UnendlichId,
    mailbox: &mut Mailbox,
) {
    mailbox.reject(requester);
    dht.put(mailbox_key(requester), MailMsg::Deny { from: my_id.clone() }.encode());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net::identity::{Mailbox, NameRegistry, Whitelist};

    fn id(seed: u64) -> UnendlichId {
        UnendlichId::from_seed(seed)
    }

    #[test]
    fn keys_differ_per_id_and_are_stable() {
        assert_eq!(mailbox_key(&id(1)), mailbox_key(&id(1)));
        assert_ne!(mailbox_key(&id(1)), mailbox_key(&id(2)));
    }

    #[test]
    fn codec_roundtrips_and_rejects_garbage() {
        for m in [
            MailMsg::Request { from: id(1), requested_name: "Aang".into() },
            MailMsg::Accept { from: id(2), assigned_name: "Toph".into(), addr: "203.0.113.5:7777".into() },
            MailMsg::Deny { from: id(3) },
        ] {
            assert_eq!(MailMsg::decode(&m.encode()), Some(m));
        }
        assert_eq!(MailMsg::decode(&[]), None);
        assert_eq!(MailMsg::decode(&[TAG_ACCEPT, 9, 0]), None); // length runs past end
        assert_eq!(MailMsg::decode(&[42]), None); // unknown tag
    }

    #[test]
    fn full_two_sided_accept_delivers_ip_only_after_accept() {
        let dht = LocalDht::default();
        let host = id(100);
        let req = id(200);
        let host_addr = "198.51.100.7:7777";

        let mut wl = Whitelist::default();
        let mut nr = NameRegistry::default();
        let mut mb = Mailbox::default();

        // R sends a request; before H acts, R's mailbox has nothing (no IP leak).
        send_request(&dht, &host, &req, "Sokka");
        assert_eq!(poll_response(&dht, &req), None);

        // H drains: unknown id -> Pending, NO accept emitted, so still no IP for R.
        host_drain_requests(&dht, &host, host_addr, &mut mb, &wl, &mut nr);
        assert_eq!(mb.pending().count(), 1);
        assert_eq!(poll_response(&dht, &req), None);

        // H accepts -> Accept with addr lands in R's mailbox.
        let name = host_accept(&dht, &host, &req, host_addr, &mut mb, &mut wl, &mut nr).unwrap();
        assert_eq!(name, "Sokka");
        assert_eq!(
            poll_response(&dht, &req),
            Some(JoinOutcome::Accepted { assigned_name: "Sokka".into(), addr: host_addr.into() })
        );
        assert!(wl.allows(&req)); // now whitelisted for next time
    }

    #[test]
    fn deny_carries_no_address() {
        let dht = LocalDht::default();
        let host = id(1);
        let req = id(2);
        let mut mb = Mailbox::default();
        let mut wl = Whitelist::default();
        let mut nr = NameRegistry::default();

        send_request(&dht, &host, &req, "Zuko");
        host_drain_requests(&dht, &host, "10.0.0.1:7777", &mut mb, &wl, &mut nr);
        host_deny(&dht, &host, &req, &mut mb);
        assert_eq!(poll_response(&dht, &req), Some(JoinOutcome::Denied));
    }

    #[test]
    fn whitelisted_requester_is_auto_accepted_with_ip() {
        let dht = LocalDht::default();
        let host = id(1);
        let friend = id(2);
        let mut mb = Mailbox::default();
        let mut nr = NameRegistry::default();
        let mut wl = Whitelist::default();
        wl.add(friend.clone()); // already trusted

        send_request(&dht, &host, &friend, "Iroh");
        host_drain_requests(&dht, &host, "192.0.2.9:7777", &mut mb, &wl, &mut nr);
        // Auto-accept dropped the IP straight into the friend's mailbox.
        assert_eq!(
            poll_response(&dht, &friend),
            Some(JoinOutcome::Accepted { assigned_name: "Iroh".into(), addr: "192.0.2.9:7777".into() })
        );
    }
}
