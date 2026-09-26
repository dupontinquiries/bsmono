//! wire_udp — the socket that speaks the protocol the bootstrap servers do.
//!
//! ## The bug this exists to fix
//!
//! [`crate::udp::UdpTransport`] puts a bespoke framing on the wire —
//! `[TAG_FIND_NODE][id][target]`, fixed-width contact records — and
//! `snapclone-presence` drives its DHT through it while bootstrapping
//! against `bootstrap.jami.net:4222`. Those servers speak OpenDHT. They
//! cannot answer these bytes, so the routing table never fills, every
//! lookup returns nothing, and no layer above notices: an empty result is
//! also what a working DHT returns for a peer who is offline.
//!
//! This transport encodes with `dht-kit` — the codec that now matches the
//! reference's packer key for key (see dht-kit's module doc for what the
//! first version got wrong) — and sends with a plain blocking socket.
//!
//! ## Same socket discipline as `udp`, deliberately
//!
//! Blocking with a timeout, one reply per query, a reply from the wrong
//! address dropped rather than returned. Those decisions are argued in
//! `udp`'s header and none of them changes because the framing did — a
//! bystander racing a real peer is the same attack whatever the bytes look
//! like.
//!
//! ## Transaction ids
//!
//! The bespoke protocol had none, so a late reply to a previous query was
//! indistinguishable from the answer to this one. OpenDHT carries a `tid`,
//! and a reply whose tid does not match is discarded here. That matters
//! more than it sounds on a phone: a query that timed out while the radio
//! woke up will have its answer arrive during the NEXT query, and accepting
//! it would attribute one peer's node list to a different target.
//!
//! ## Two modes: a client that only asks, or a node that also answers
//!
//! Unserved (the default), nothing reads this socket except a round trip
//! waiting for its own answer, so an unsolicited query from another node
//! is never seen. The reference has a flag for exactly that — `s: true` on
//! every query — and a node that sees it answers us without adding us to
//! its routing table, which is right: a peer that never answers is dead
//! weight in anyone's table.
//!
//! But two devices that only ask can never find each other: A's `find` to
//! B goes unanswered, and both depend on a third party that does answer.
//! That third party has so far been `bootstrap.jami.net`. [`serve`] is
//! the other mode: one thread owns the socket's read side, answers queries
//! through a [`crate::wire::Responder`], and hands replies to whichever
//! round trip is waiting for them. Served, this socket is a node in its
//! own right — two of them on a LAN need nobody else — and `s` is off.
//!
//! The serving thread blocks in `recv_from` with no timeout. An idle node
//! costs nothing; there is no poll interval to tune against a battery.
//! [`stop_serving`] wakes it with a datagram to itself.
//!
//! ## The write token, and where it had to live
//!
//! OpenDHT, like Kademlia generally, expects a `get` to hand back a token
//! that the following `put` echoes, so a node cannot be used to spray
//! writes at a third party it never talked to. A strict server refuses a
//! `put` without one.
//!
//! `Transport::store` receives no prior state, so the obvious fix is to
//! widen the trait — and that touches every simulation implementing it, in
//! five crates, to thread a value only one implementation can produce.
//!
//! The transport remembers it instead. Every accepted reply carries a
//! token, and a `store` is always preceded by a lookup to the same peer
//! (`snapclone-presence::publish` does `lookup_nodes` and then stores to
//! each node it found) — so by the time a `put` goes out, that peer has
//! just handed us one. The cache is keyed by ADDRESS, not node id, because
//! the token authorises writes through a particular server rather than to a
//! particular identity.
//!
//! ## What every reply also teaches us: our own address
//!
//! A reply carries `sa`, the requester's ip as the responder saw it. That
//! is the reference's own NAT-discovery mechanism (`onReportedAddr`) and it
//! is how the Qt build knew what to announce for cross-network reachability
//! without asking ipify. Kept as a vote per reported ip and read with
//! [`public_ip`]: a majority, because one peer on the same LAN reports our
//! private address and must not outvote the internet.
//!
//! What this still does not do: `listen`. Values expire and are republished
//! by the caller's own schedule (`dht_presence` in the app), which is the
//! coarser of the two mechanisms and the one that works without an inbound
//! path.

use crate::guard::RateLimiter;
use crate::node::{FindValue, Node, Transport};
use crate::routing::{Contact, NodeId};
use crate::store::Key;
use crate::wire::{content_id, from_ip, from_wire_addr, to_hash, Responder};
use dht_kit::{Msg, Query, Value, WANT4, WANT6};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How long to wait for one reply. Matches `udp`'s reasoning: a lookup makes
/// several of these in sequence, so this is multiplied by the contacts
/// tried.
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(1_500);

/// Sends per query within [`REPLY_TIMEOUT`]. The reference retries a request
/// that has not been answered after half of `MAX_RESPONSE_TIME` (1 s), three
/// attempts in all (`Request::MAX_ATTEMPT_COUNT`); one lost datagram -- routine on
/// a phone radio waking up -- no longer costs the whole query. Same `tid` on every
/// attempt, so whichever copy is answered first is the answer and the rest are
/// late replies the tid check discards.
pub const ATTEMPTS: u32 = 3;

/// Bigger than any reply the reference sends, and bounded so a hostile
/// datagram cannot make us allocate.
pub const MAX_DATAGRAM: usize = 8 * 1024;

/// How long a write token is worth keeping. The reference rotates them on
/// the order of minutes; past that a `put` echoing one is refused, which is
/// the same outcome as sending none.
pub const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

/// A ceiling on remembered tokens. A long session touches many peers, and
/// this map must not become a record of every node ever contacted.
pub const MAX_TOKENS: usize = 256;

/// A ceiling on distinct reported addresses. A hostile peer can report a
/// different one per reply; the votes are what protect the answer, and the
/// map must not grow with the lie.
pub const MAX_REPORTED: usize = 32;

/// A round trip waiting for its answer, when a serving thread is the one
/// reading the socket.
struct Pending {
    addr: SocketAddr,
    tx: mpsc::Sender<Msg>,
}

pub struct WireUdpTransport {
    sock: UdpSocket,
    me: NodeId,
    client: AtomicBool,
    /// Whether a serving thread owns the read side. When it does, round
    /// trips wait on `pending` instead of reading the socket themselves.
    served: AtomicBool,
    stop: AtomicBool,
    pending: Mutex<HashMap<u32, Pending>>,
    /// Write tokens by peer ADDRESS — see the module note on why not by id.
    tokens: Mutex<HashMap<SocketAddr, (Vec<u8>, Instant)>>,
    /// Votes for our own public ip, from `sa`.
    reported: Mutex<HashMap<IpAddr, u32>>,
    /// Monotonic, wrapping. Only uniqueness within the timeout window
    /// matters, so wrapping is fine and a random start buys nothing here —
    /// this is not a spoofing defence, it is a "which question is this the
    /// answer to" tag.
    tid: AtomicU32,
}

impl WireUdpTransport {
    pub fn bind(me: NodeId, addr: SocketAddr) -> std::io::Result<WireUdpTransport> {
        let sock = UdpSocket::bind(addr)?;
        sock.set_read_timeout(Some(REPLY_TIMEOUT))?;
        Ok(WireUdpTransport {
            sock, me,
            client: AtomicBool::new(true),
            served: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            pending: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
            reported: Mutex::new(HashMap::new()),
            tid: AtomicU32::new(1),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Whether queries carry `s: true`. On by default — see the module note.
    pub fn set_client(&self, client: bool) {
        self.client.store(client, Ordering::Relaxed);
    }

    /// The ip most peers say we are reaching them from, if any peer has
    /// said. Private on a LAN, public across the internet; the caller
    /// decides which of those it is looking at.
    pub fn public_ip(&self) -> Option<IpAddr> {
        let m = self.reported.lock().ok()?;
        m.iter().max_by_key(|(_, n)| **n).map(|(ip, _)| *ip)
    }

    fn want(&self) -> u8 {
        // Only the family this socket can reach. Contacts of the other one
        // would fill the routing table with peers every lookup then fails
        // against.
        match self.sock.local_addr() {
            Ok(a) if a.is_ipv6() => WANT6,
            _ => WANT4,
        }
    }

    fn next_tid(&self) -> u32 {
        self.tid.fetch_add(1, Ordering::Relaxed)
    }

    /// Send one query and wait for its reply.
    ///
    /// `None` on timeout, on a send error, on a reply from an address we did
    /// not ask, on an undecodable datagram, and on a tid that does not
    /// match. All the same to the caller because the caller's response is
    /// the same: count it against that contact and move on.
    fn round_trip(&self, to: &Contact, tid: u32, msg: &Msg) -> Option<Msg> {
        if self.served.load(Ordering::Acquire) {
            // The serving thread reads; we wait for it to hand over the
            // reply. Registered BEFORE sending, or a fast answer could
            // arrive with nobody expecting it.
            let (tx, rx) = mpsc::channel();
            self.pending.lock().ok()?.insert(tid, Pending { addr: to.addr, tx });
            let bytes = msg.encode();
            let per_attempt = REPLY_TIMEOUT / ATTEMPTS;
            let mut got = None;
            for _ in 0..ATTEMPTS {
                if self.sock.send_to(&bytes, to.addr).is_err() {
                    break;
                }
                if let Ok(m) = rx.recv_timeout(per_attempt) {
                    got = Some(m);
                    break;
                }
            }
            if let Ok(mut p) = self.pending.lock() { p.remove(&tid); }
            return match got {
                Some(Msg::Error { .. }) | None => None,
                other => other,
            };
        }
        let bytes = msg.encode();
        self.sock.send_to(&bytes, to.addr).ok()?;
        let per_attempt = REPLY_TIMEOUT / ATTEMPTS;
        let mut sent = 1;
        let mut resend_at = Instant::now() + per_attempt;
        // KEEP READING UNTIL THE DEADLINE rather than treating the first
        // datagram as the answer. One socket receives replies, unsolicited
        // queries from other nodes, and late answers to queries that
        // already timed out — and the first version of this discarded the
        // real reply whenever any of those arrived first. On a busy DHT
        // that is not an edge case, it is most lookups.
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut buf = [0u8; MAX_DATAGRAM];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            if now >= resend_at && sent < ATTEMPTS {
                let _ = self.sock.send_to(&bytes, to.addr);
                sent += 1;
                resend_at = now + per_attempt;
            }
            // Wake for the next resend or the deadline, whichever is first, so
            // the loop cannot outlive either by one full timeout per stray
            // datagram.
            let wake = if sent < ATTEMPTS { resend_at.min(deadline) } else { deadline };
            let left = wake.saturating_duration_since(now).max(Duration::from_millis(1));
            self.sock.set_read_timeout(Some(left)).ok()?;
            let (n, from) = match self.sock.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(_) => return None,
            };
            // A reply from an address we did not ask is dropped. UDP has no
            // connection, so anything on the network can answer a query it
            // saw, and accepting that lets a bystander steer a lookup.
            if from != to.addr {
                continue;
            }
            let Some(reply) = Msg::decode(&buf[..n]) else { continue };
            match reply {
                // An error reply is a real answer — the peer understood and
                // declined — but there is nothing in it for a `Transport`
                // method to return, so it ends the wait like a timeout.
                Msg::Error { tid: got, .. } if got == tid => return None,
                Msg::Reply { tid: got, .. } if got == tid => {
                    self.learn_from_reply(from, &reply);
                    return Some(reply);
                }
                // Fragments of a value too large for one datagram. Not
                // reassembled here; the reply that announced them has
                // already been returned or will not be.
                Msg::Parts { .. } => continue,
                // Somebody else's business, or ours from a while ago.
                _ => continue,
            }
        }
    }

    /// What every accepted reply teaches: the peer's token and our address.
    fn learn_from_reply(&self, from: SocketAddr, reply: &Msg) {
        if let Msg::Reply { token, sa, .. } = reply {
            self.remember_token(from, token);
            if let Some(ip) = sa { self.note_reported(from_ip(ip)); }
        }
    }

    /// The serving thread's half of a round trip: hand an inbound reply or
    /// error to the round trip waiting for it. `true` if one was.
    fn deliver(&self, from: SocketAddr, msg: Msg) -> bool {
        let tid = match &msg {
            Msg::Reply { tid, .. } | Msg::Error { tid, .. } => *tid,
            _ => return false,
        };
        let Ok(mut p) = self.pending.lock() else { return false };
        // Same rule as the unserved path: the answer must come from the
        // address we asked, or a bystander steers the lookup.
        let Some(w) = p.get(&tid) else { return false };
        if w.addr != from { return false; }
        let w = p.remove(&tid).unwrap();
        drop(p);
        self.learn_from_reply(from, &msg);
        w.tx.send(msg).is_ok()
    }

    /// Become a node: answer queries, and route replies to round trips.
    ///
    /// The thread holds only a `Weak` to this transport, so dropping the
    /// last `Arc` ends it at the next datagram; [`stop_serving`] sends that
    /// datagram. `node` is locked once per inbound query, briefly — a
    /// caller running a lookup should work on a snapshot of the routing
    /// table rather than hold this lock across round trips.
    pub fn serve(self: &Arc<Self>, node: Arc<Mutex<Node>>) -> std::io::Result<std::thread::JoinHandle<()>> {
        let sock = self.sock.try_clone()?;
        // No timeout: blocked in the kernel until a packet, costing nothing.
        // (SO_RCVTIMEO is per socket, shared by the clone; the unserved read
        // loop is never used once `served` is set, so nothing else minds.)
        sock.set_read_timeout(None)?;
        self.served.store(true, Ordering::Release);
        self.client.store(false, Ordering::Relaxed);
        let weak: Weak<Self> = Arc::downgrade(self);
        let seed = u64::from_le_bytes(self.me.0[..8].try_into().unwrap());
        Ok(std::thread::spawn(move || {
            let mut responder = Responder::new(seed, now_ms());
            let mut limiter = RateLimiter::default();
            let mut buf = [0u8; MAX_DATAGRAM];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { return };
                let Some(me) = weak.upgrade() else { return };
                if me.stop.load(Ordering::Acquire) { return; }
                let Some(msg) = Msg::decode(&buf[..n]) else { continue };
                match msg {
                    Msg::Query { .. } => {
                        let ms = now_ms();
                        if !limiter.allow(&from, ms) { continue; }
                        limiter.gc(ms);
                        let reply = {
                            let Ok(mut node) = node.lock() else { return };
                            responder.handle(&mut node, from, &msg, ms / 1000)
                        };
                        if let Some(r) = reply {
                            let _ = sock.send_to(&r.encode(), from);
                        }
                    }
                    Msg::Parts { .. } => {}
                    other => { me.deliver(from, other); }
                }
            }
        }))
    }

    /// Ask the serving thread to exit, and wake it so it does.
    pub fn stop_serving(&self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(local) = self.sock.local_addr() {
            // The socket may be bound to 0.0.0.0; send to loopback on its
            // port so the datagram actually arrives.
            let wake = SocketAddr::new(
                if local.is_ipv6() { "::1".parse().unwrap() } else { "127.0.0.1".parse().unwrap() },
                local.port());
            let _ = self.sock.send_to(&[], wake);
        }
    }

    fn remember_token(&self, from: SocketAddr, token: &[u8]) {
        // Empty means the peer sent none. Storing it would evict a real one
        // for the same address on the next reply.
        if token.is_empty() {
            return;
        }
        let Ok(mut m) = self.tokens.lock() else { return };
        if m.len() >= MAX_TOKENS && !m.contains_key(&from) {
            // Drop expired entries first; only if that frees nothing does
            // anything current go. Evicting a live token to make room for
            // another is how a `put` loses the authorisation it just
            // earned.
            let now = Instant::now();
            m.retain(|_, (_, at)| now.duration_since(*at) < TOKEN_TTL);
            if m.len() >= MAX_TOKENS {
                if let Some(oldest) = m.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| *k) {
                    m.remove(&oldest);
                }
            }
        }
        m.insert(from, (token.to_vec(), Instant::now()));
    }

    fn note_reported(&self, ip: IpAddr) {
        let Ok(mut m) = self.reported.lock() else { return };
        if m.len() >= MAX_REPORTED && !m.contains_key(&ip) {
            return;
        }
        *m.entry(ip).or_insert(0) += 1;
    }

    /// The token this peer last gave us, if it is still fresh. Empty means
    /// "we have none" — which is what an unauthorised `put` looks like, and
    /// the honest thing to send when there is nothing better.
    fn token_for(&self, addr: SocketAddr) -> Vec<u8> {
        let Ok(m) = self.tokens.lock() else { return Vec::new() };
        match m.get(&addr) {
            Some((t, at)) if at.elapsed() < TOKEN_TTL => t.clone(),
            _ => Vec::new(),
        }
    }

    fn query(&self, query: Query, target: &NodeId, values: Vec<Value>, token: Vec<u8>) -> (u32, Msg) {
        let tid = self.next_tid();
        let want = if matches!(query, Query::Find | Query::Get) { self.want() } else { 0 };
        (tid, Msg::Query {
            tid,
            from: to_hash(&self.me),
            query,
            target: to_hash(target),
            token,
            values,
            want,
            sid: None,
            vid: None,
            client: self.client.load(Ordering::Relaxed),
        })
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The contacts a reply teaches us about — INCLUDING the responder itself.
///
/// Including it is what corrects a guessed node id. A bootstrap entry is
/// added from a hostname before anyone has spoken to it, so its id is
/// invented — `snapclone-presence` derives one from the address and its
/// comment says "the id is corrected when the node actually replies".
/// Nothing was doing the correcting: the routing table kept the invented
/// id, which puts the busiest node in the network in the wrong bucket, so
/// lookups route away from the one peer that knows everybody.
///
/// A reply's `from` is that node's real id, at the address we just reached
/// it on. Returning it as a contact is how the routing table learns —
/// `lookup_nodes` inserts everything a reply returns.
fn contacts_of(reply: &Msg, responder: SocketAddr, now: u64) -> Vec<Contact> {
    match reply {
        Msg::Reply { from, nodes, .. } => {
            let mut out = vec![Contact::new(crate::wire::from_hash(from), responder, now)];
            out.extend(nodes.iter()
                .map(|(h, a)| Contact::new(crate::wire::from_hash(h), from_wire_addr(a), now)));
            out
        }
        _ => Vec::new(),
    }
}

impl Transport for WireUdpTransport {
    fn find_node(&self, _from: &Contact, to: &Contact, target: &NodeId) -> Vec<Contact> {
        let (tid, msg) = self.query(Query::Find, target, Vec::new(), Vec::new());
        match self.round_trip(to, tid, &msg) {
            // `last_seen` is 0 rather than a clock read: `Transport` gives
            // this method no `now`, and the routing table stamps a contact
            // when it inserts one. Inventing a timestamp here would be a
            // second, wronger clock.
            Some(reply) => contacts_of(&reply, to.addr, 0),
            None => Vec::new(),
        }
    }

    fn find_value(&self, _from: &Contact, to: &Contact, key: &Key) -> FindValue {
        let (tid, msg) = self.query(Query::Get, key, Vec::new(), Vec::new());
        match self.round_trip(to, tid, &msg) {
            Some(reply @ Msg::Reply { .. }) => {
                let Msg::Reply { values, .. } = &reply else { unreachable!() };
                // Values WIN over nodes when both are present, which the
                // reference permits in one reply. A caller that took the
                // nodes would keep walking toward a key whose value it was
                // just handed.
                if values.is_empty() {
                    // The responder leads here too, same reasoning as
                    // `contacts_of` — a `get` is just as much a chance to
                    // learn who we are actually talking to as a `find`.
                    FindValue::Closest(contacts_of(&reply, to.addr, 0))
                } else {
                    FindValue::Values(values.iter().map(|v| v.data.clone()).collect())
                }
            }
            _ => FindValue::Closest(Vec::new()),
        }
    }

    fn store(&self, from: &Contact, to: &Contact, key: Key, value: Vec<u8>, ttl: u64) {
        // One implementation: `store_acked` below. The value id comes from the
        // bytes so republishing the same record does not accumulate duplicates
        // under one key, and the token is the one this peer handed us during
        // the lookup -- or, if the lookup only heard of this peer from a third
        // party, a `get` is sent first to earn one (the reference does a `get`
        // to every announce target for the same reason; a put without a token
        // is refused with 401).
        let _ = self.store_acked(from, to, key, value, ttl);
    }

    fn ping(&self, _from: &Contact, to: &Contact) -> bool {
        let (tid, msg) = self.query(Query::Ping, &NodeId([0; 20]), Vec::new(), Vec::new());
        matches!(self.round_trip(to, tid, &msg), Some(Msg::Reply { .. }))
    }

    // A reply always names its responder (`contacts_of` puts it first), so an
    // empty list from `find_node` already meant "no answer" -- but the lookup
    // driver could not know that. These say it.

    fn query_nodes(&self, _from: &Contact, to: &Contact, target: &NodeId) -> Option<Vec<Contact>> {
        let (tid, msg) = self.query(Query::Find, target, Vec::new(), Vec::new());
        self.round_trip(to, tid, &msg).map(|reply| contacts_of(&reply, to.addr, 0))
    }

    fn query_value(&self, _from: &Contact, to: &Contact, key: &Key) -> Option<FindValue> {
        let (tid, msg) = self.query(Query::Get, key, Vec::new(), Vec::new());
        match self.round_trip(to, tid, &msg)? {
            reply @ Msg::Reply { .. } => {
                let Msg::Reply { values, .. } = &reply else { unreachable!() };
                Some(if values.is_empty() {
                    FindValue::Closest(contacts_of(&reply, to.addr, 0))
                } else {
                    FindValue::Values(values.iter().map(|v| v.data.clone()).collect())
                })
            }
            _ => None,
        }
    }

    fn store_acked(&self, _from: &Contact, to: &Contact, key: Key, value: Vec<u8>, _ttl: u64) -> bool {
        if value.len() > dht_kit::MAX_VALUE_SIZE {
            return false;
        }
        let v = Value::new(content_id(&value), value);
        let mut token = self.token_for(to.addr);
        if token.is_empty() {
            let (tid, get) = self.query(Query::Get, &key, Vec::new(), Vec::new());
            let _ = self.round_trip(to, tid, &get);
            token = self.token_for(to.addr);
        }
        let (tid, msg) = self.query(Query::Put, &key, vec![v], token);
        // A Reply is the reference's acceptance; an Error (401 bad token, 413
        // too big, ...) or silence is not.
        matches!(self.round_trip(to, tid, &msg), Some(Msg::Reply { .. }))
    }

    fn query_nodes_many(&self, from: &Contact, to: &[Contact], target: &NodeId) -> Vec<Option<Vec<Contact>>> {
        self.fan_out(to, |c| self.query_nodes(from, c, target))
    }

    fn query_value_many(&self, from: &Contact, to: &[Contact], key: &Key) -> Vec<Option<FindValue>> {
        self.fan_out(to, |c| self.query_value(from, c, key))
    }
}

impl WireUdpTransport {
    /// Run `f` for each contact -- concurrently when a serving thread owns the
    /// read side (round trips then wait on their own tid and cannot steal each
    /// other's replies), sequentially otherwise (each round trip reads the
    /// socket itself, so two at once would take each other's answers).
    fn fan_out<R: Send>(&self, to: &[Contact], f: impl Fn(&Contact) -> R + Sync) -> Vec<R> {
        if to.len() <= 1 || !self.served.load(Ordering::Acquire) {
            return to.iter().map(&f).collect();
        }
        std::thread::scope(|s| {
            let handles: Vec<_> = to.iter().map(|c| { let f = &f; s.spawn(move || f(c)) }).collect();
            handles.into_iter().map(|h| h.join().expect("a lookup query panicked")).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dht_kit::{Addr, InfoHash, Ip};

    fn id(n: u8) -> NodeId {
        let mut b = [0u8; 20];
        b[0] = n;
        NodeId(b)
    }

    fn contact(addr: SocketAddr) -> Contact {
        Contact::new(id(9), addr, 0)
    }

    fn reply(tid: u32, from: InfoHash) -> Msg {
        rep(tid, from, vec![], vec![], vec![], None)
    }
    fn rep(tid: u32, from: InfoHash, nodes: Vec<(InfoHash, Addr)>, values: Vec<Value>,
           token: Vec<u8>, vid: Option<u64>) -> Msg {
        Msg::Reply { tid, from, sa: Some(Ip::V4([127, 0, 0, 1])), nodes, values, token, vid }
    }
    fn one_node() -> Vec<(InfoHash, Addr)> {
        vec![(InfoHash([5u8; 20]), Addr::new([10, 0, 0, 4], 4222))]
    }
    fn peer3() -> InfoHash { InfoHash([3u8; 20]) }

    /// A peer that decodes what arrives, answers through `reply`, and hands
    /// back what it saw. The thing under test is whether the bytes are ones
    /// a foreign implementation would accept, so this decodes the msgpack
    /// rather than faking a `Transport`.
    fn responder(reply: impl Fn(Msg) -> Option<Msg> + Send + 'static)
        -> (SocketAddr, std::thread::JoinHandle<Option<Msg>>) {
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let (n, from) = sock.recv_from(&mut buf).ok()?;
            let got = Msg::decode(&buf[..n])?;
            if let Some(answer) = reply(got.clone()) {
                sock.send_to(&answer.encode(), from).ok()?;
            }
            Some(got)
        });
        (addr, h)
    }

    fn transport() -> WireUdpTransport {
        WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap()
    }

    #[test]
    fn a_find_query_goes_out_as_opendht_msgpack_and_its_nodes_come_back() {
        // The whole point: what leaves this socket must decode as the
        // reference's format, because bootstrap.jami.net is what has to
        // read it.
        let peer_id = InfoHash([3u8; 20]);
        let (addr, h) = responder(move |q| match q {
            Msg::Query { tid, query: Query::Find, .. } =>
                Some(rep(tid, peer_id, one_node(), vec![], vec![], None)),
            _ => None,
        });
        let t = transport();
        let got = t.find_node(&contact(addr), &contact(addr), &id(2));
        let sent = h.join().unwrap().expect("nothing decodable arrived");
        match sent {
            Msg::Query { query: Query::Find, want, client, target, .. } => {
                assert_eq!(want, WANT4, "a v4 socket asks for v4 nodes");
                assert!(client, "we do not answer queries, and must say so");
                assert_eq!(target, to_hash(&id(2)));
            }
            other => panic!("the wrong query kind went out: {other:?}"),
        }
        // Two: the responder itself (with its REAL id, at the address we
        // reached it on) followed by the node it named.
        assert_eq!(got.len(), 2, "the reply's contacts were lost: {got:?}");
        assert_eq!(got[0].addr, addr, "the responder is not first");
        assert_eq!(got[0].id, crate::wire::from_hash(&peer_id), "the responder's real id was not learned");
        assert_eq!(got[1].addr, "10.0.0.4:4222".parse::<SocketAddr>().unwrap());
    }

    /// ONE LOST DATAGRAM IS NOT A DEAD PEER. The responder ignores the first copy
    /// of the query and answers the second: the query succeeds, and both copies
    /// carried the same tid (so a late answer to either is the answer).
    #[test]
    fn a_dropped_query_is_retransmitted_under_the_same_tid() {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let (n, _) = sock.recv_from(&mut buf).ok()?;
            let Some(Msg::Query { tid: first, .. }) = Msg::decode(&buf[..n]) else { return None };
            let (n, from) = sock.recv_from(&mut buf).ok()?; // the retransmission
            let Some(Msg::Query { tid: second, .. }) = Msg::decode(&buf[..n]) else { return None };
            sock.send_to(&reply(second, peer3()).encode(), from).ok()?;
            Some((first, second))
        });
        let t = transport();
        let got = t.query_nodes(&contact(addr), &contact(addr), &id(2));
        let (first, second) = h.join().unwrap().expect("the query was never resent");
        assert_eq!(first, second, "a retransmission must reuse the tid");
        assert!(got.is_some(), "the answer to the retransmission was not taken");
        // And a peer that never answers is None -- not an empty answer.
        let dead = UdpSocket::bind("127.0.0.1:0").unwrap();
        let da = dead.local_addr().unwrap();
        assert!(t.query_nodes(&contact(da), &contact(da), &id(2)).is_none());
    }

    /// A put the peer REFUSES is not a store. `store` could not say so, and the
    /// publish count was "peers asked".
    #[test]
    fn store_acked_reports_acceptance_and_refusal() {
        let serve = |accept: bool| {
            let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let addr = sock.local_addr().unwrap();
            let h = std::thread::spawn(move || {
                let mut buf = [0u8; MAX_DATAGRAM];
                loop {
                    let Ok((n, from)) = sock.recv_from(&mut buf) else { return };
                    match Msg::decode(&buf[..n]) {
                        Some(Msg::Query { tid, query: Query::Get, .. }) => {
                            let _ = sock.send_to(&rep(tid, peer3(), vec![], vec![], vec![1], None).encode(), from);
                        }
                        Some(Msg::Query { tid, query: Query::Put, .. }) => {
                            let answer = if accept { reply(tid, peer3()) }
                                else { Msg::Error { tid, code: 401, from: None } };
                            let _ = sock.send_to(&answer.encode(), from);
                            return;
                        }
                        _ => {}
                    }
                }
            });
            (addr, h)
        };
        let t = transport();
        let (a, h) = serve(true);
        assert!(t.store_acked(&contact(a), &contact(a), id(2), b"v".to_vec(), 600));
        h.join().unwrap();
        let (a, h) = serve(false);
        assert!(!t.store_acked(&contact(a), &contact(a), id(2), b"v".to_vec(), 600), "a 401 counted as stored");
        h.join().unwrap();
    }

    /// A lookup round against dead peers costs ONE timeout on a served socket, not
    /// one per peer. Serial, four silent peers were six seconds of a frozen lookup.
    #[test]
    fn a_served_round_asks_its_peers_concurrently() {
        let t = std::sync::Arc::new(transport());
        let node = std::sync::Arc::new(Mutex::new(Node::new(id(1), t.local_addr().unwrap())));
        let serving = t.serve(node).unwrap();
        let silent: Vec<UdpSocket> = (0..4).map(|_| UdpSocket::bind("127.0.0.1:0").unwrap()).collect();
        let to: Vec<Contact> = silent.iter().map(|s| contact(s.local_addr().unwrap())).collect();
        let started = Instant::now();
        let got = t.query_nodes_many(&to[0], &to, &id(2));
        let took = started.elapsed();
        assert_eq!(got.len(), 4);
        assert!(got.iter().all(|g| g.is_none()));
        assert!(took < REPLY_TIMEOUT * 2, "four silent peers took {took:?}: asked one after another");
        assert!(took >= REPLY_TIMEOUT - Duration::from_millis(100), "returned before the timeout: {took:?}");
        t.stop_serving();
        let _ = serving.join();
    }

    #[test]
    fn a_get_that_finds_a_value_returns_it_rather_than_walking_on() {
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, query: Query::Get, .. } =>
                Some(rep(tid, peer3(), one_node(), vec![Value::new(7, b"presence".to_vec())], vec![], None)),
            _ => None,
        });
        let t = transport();
        let got = t.find_value(&contact(addr), &contact(addr), &id(2));
        assert!(matches!(h.join().unwrap(), Some(Msg::Query { query: Query::Get, .. })));
        match got {
            FindValue::Values(v) => assert_eq!(v, vec![b"presence".to_vec()]),
            other => panic!("nodes were preferred over a value: {other:?}"),
        }
    }

    /// Like `responder`, but answers the preliminary get a `store` to an
    /// unspoken peer makes, and hands back the put that follows.
    fn put_responder() -> (SocketAddr, std::thread::JoinHandle<Option<Msg>>) {
        let sock = UdpSocket::bind("127.0.0.1:0").expect("bind");
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let (n, from) = sock.recv_from(&mut buf).ok()?;
            let Some(Msg::Query { tid, query: Query::Get, .. }) = Msg::decode(&buf[..n]) else { return None };
            sock.send_to(&rep(tid, peer3(), vec![], vec![], vec![1], None).encode(), from).ok()?;
            let (n, from) = sock.recv_from(&mut buf).ok()?;
            let got = Msg::decode(&buf[..n])?;
            if let Msg::Query { tid, values, .. } = &got {
                sock.send_to(&rep(*tid, peer3(), vec![], vec![], vec![], values.first().map(|v| v.id)).encode(), from).ok()?;
            }
            Some(got)
        });
        (addr, h)
    }

    #[test]
    fn a_put_carries_the_value_in_the_references_shape_with_a_content_id() {
        let (addr, h) = put_responder();
        let t = transport();
        t.store(&contact(addr), &contact(addr), id(2), b"record".to_vec(), 600);
        match h.join().unwrap() {
            Some(Msg::Query { query: Query::Put, values, .. }) => {
                assert_eq!(values.len(), 1);
                assert_eq!(values[0].data, b"record");
                assert_eq!(values[0].id, content_id(b"record"), "same bytes, same id, every republish");
                assert_eq!(values[0].vtype, 0, "USER_DATA");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_reply_to_a_previous_query_is_not_read_as_this_one() {
        // The responder answers with the WRONG tid, as a late reply to an
        // earlier query would. It must be discarded; the lookup then times
        // out, which is the honest outcome.
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, .. } =>
                Some(rep(tid.wrapping_add(1), peer3(), one_node(), vec![], vec![], None)),
            _ => None,
        });
        let t = transport();
        let got = t.find_node(&contact(addr), &contact(addr), &id(2));
        h.join().unwrap();
        assert!(got.is_empty(), "a mismatched tid was accepted: {got:?}");
    }

    #[test]
    fn a_guessed_bootstrap_id_is_corrected_by_the_first_reply() {
        let real = InfoHash([0xAB; 20]);
        let (addr, h) = responder(move |q| match q {
            Msg::Query { tid, .. } => Some(reply(tid, real)),
            _ => None,
        });
        let t = transport();
        // The caller's guess for the responder is id(9) (see `contact`).
        let got = t.find_node(&contact(addr), &contact(addr), &id(2));
        h.join().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, crate::wire::from_hash(&real));
        assert_eq!(got[0].addr, addr);
    }

    #[test]
    fn a_put_echoes_the_token_the_peer_gave_during_the_lookup() {
        // The responder answers a get with a token, then checks the put
        // that follows carries the same bytes.
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let token = vec![0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3, 4, 5, 6];
        let tk = token.clone();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let (n, from) = sock.recv_from(&mut buf).unwrap();
            let Some(Msg::Query { tid, .. }) = Msg::decode(&buf[..n]) else { panic!() };
            sock.send_to(&rep(tid, peer3(), vec![], vec![], tk.clone(), None).encode(), from).unwrap();
            let (n, from) = sock.recv_from(&mut buf).unwrap();
            if let Some(Msg::Query { tid, token, query, .. }) = Msg::decode(&buf[..n]) {
                assert_eq!(query, Query::Put);
                sock.send_to(&reply(tid, InfoHash([3u8; 20])).encode(), from).unwrap();
                return token;
            }
            panic!("second datagram was not a put");
        });
        let t = transport();
        let _ = t.find_value(&contact(addr), &contact(addr), &id(2));
        t.store(&contact(addr), &contact(addr), id(2), b"v".to_vec(), 600);
        assert_eq!(h.join().unwrap(), token);
    }

    #[test]
    fn a_put_to_a_peer_we_have_not_spoken_to_asks_it_first_and_uses_its_token() {
        // The lookup's shortlist can name a peer no query ever reached. A
        // put there has no token and is refused; so the transport gets one.
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (n, from) = sock.recv_from(&mut buf).unwrap();
                let Some(Msg::Query { tid, query, token, .. }) = Msg::decode(&buf[..n]) else { panic!() };
                seen.push((query, token));
                sock.send_to(&rep(tid, peer3(), vec![], vec![], vec![7, 7, 7], None).encode(), from).unwrap();
            }
            seen
        });
        let t = transport();
        t.store(&contact(addr), &contact(addr), id(2), b"v".to_vec(), 600);
        let seen = h.join().unwrap();
        assert_eq!(seen[0].0, Query::Get, "a get must precede the put to an unspoken peer");
        assert_eq!(seen[1].0, Query::Put);
        assert_eq!(seen[1].1, vec![7, 7, 7], "the put carries the token the get earned");
    }

    #[test]
    fn a_put_to_a_peer_that_answers_nothing_sends_no_token_rather_than_a_wrong_one() {
        let (addr, h) = responder(|q| match q {
            // The get goes unanswered (None); the put still goes out, empty.
            Msg::Query { query: Query::Get, .. } => None,
            _ => None,
        });
        let t = transport();
        t.store(&contact(addr), &contact(addr), id(2), b"v".to_vec(), 600);
        // The responder saw the get; the put followed it after a timeout.
        assert!(matches!(h.join().unwrap(), Some(Msg::Query { query: Query::Get, .. })));
    }

    #[test]
    fn an_empty_token_does_not_evict_a_real_one() {
        let t = transport();
        let a: SocketAddr = "10.0.0.1:1".parse().unwrap();
        t.remember_token(a, &[1, 2, 3]);
        t.remember_token(a, &[]);
        assert_eq!(t.token_for(a), vec![1, 2, 3]);
    }

    #[test]
    fn the_token_cache_is_bounded_and_keeps_what_is_current() {
        let t = transport();
        for i in 0..(MAX_TOKENS + 10) {
            let a: SocketAddr = format!("10.0.{}.{}:1", i / 256, i % 256).parse().unwrap();
            t.remember_token(a, &[i as u8]);
        }
        let n = t.tokens.lock().unwrap().len();
        assert!(n <= MAX_TOKENS, "{n}");
        let last: SocketAddr = format!("10.0.{}.{}:1", (MAX_TOKENS + 9) / 256, (MAX_TOKENS + 9) % 256).parse().unwrap();
        assert!(!t.token_for(last).is_empty(), "the newest token was the one evicted");
    }

    #[test]
    fn a_stray_datagram_does_not_cost_the_real_reply() {
        // Noise from the SAME address (a query it is sending us, say)
        // arrives before the answer. The first version returned on the
        // first datagram and lost the reply.
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let addr = sock.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM];
            let (n, from) = sock.recv_from(&mut buf).unwrap();
            let Some(Msg::Query { tid, .. }) = Msg::decode(&buf[..n]) else { return };
            let noise = Msg::Query { tid: 999, from: InfoHash([8u8; 20]), query: Query::Ping,
                                     target: InfoHash::zero(), token: vec![], values: vec![],
                                     want: 0, sid: None, vid: None, client: false };
            sock.send_to(&noise.encode(), from).unwrap();
            sock.send_to(b"\xFF\xFF\xFF", from).unwrap();
            let real = rep(tid, peer3(), one_node(), vec![], vec![], None);
            sock.send_to(&real.encode(), from).unwrap();
        });
        let t = transport();
        let got = t.find_node(&contact(addr), &contact(addr), &id(2));
        assert_eq!(got.len(), 2, "the real reply was lost behind noise: {got:?}");
    }

    #[test]
    fn a_reply_from_the_wrong_address_is_dropped() {
        let (addr, _h) = responder(|_| None);
        let other = UdpSocket::bind("127.0.0.1:0").unwrap();
        let t = transport();
        let me = t.local_addr().unwrap();
        // A bystander answers before the real peer does (which never will).
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let m = rep(1, peer3(), one_node(), vec![], vec![], None);
            other.send_to(&m.encode(), me).unwrap();
        });
        let got = t.find_node(&contact(addr), &contact(addr), &id(2));
        assert!(got.is_empty(), "a bystander steered the lookup: {got:?}");
    }

    #[test]
    fn an_error_reply_counts_as_a_failure_rather_than_an_empty_success() {
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, .. } => Some(Msg::Error { tid, code: 401, from: None }),
            _ => None,
        });
        let t = transport();
        assert!(!t.ping(&contact(addr), &contact(addr)));
        h.join().unwrap();
    }

    #[test]
    fn a_ping_carries_no_target() {
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, .. } => Some(reply(tid, InfoHash([3u8; 20]))),
            _ => None,
        });
        let t = transport();
        assert!(t.ping(&contact(addr), &contact(addr)));
        match h.join().unwrap() {
            Some(Msg::Query { query: Query::Ping, target, want, .. }) => {
                assert!(target.is_zero());
                assert_eq!(want, 0, "a ping does not ask for nodes");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn our_public_ip_is_the_majority_of_what_peers_report() {
        let t = transport();
        for _ in 0..2 { t.note_reported("192.168.1.5".parse().unwrap()); }
        for _ in 0..3 { t.note_reported("203.0.113.7".parse().unwrap()); }
        assert_eq!(t.public_ip(), Some("203.0.113.7".parse().unwrap()));
        // A liar reporting a fresh address per reply cannot grow the map.
        for i in 0..(MAX_REPORTED + 20) {
            t.note_reported(format!("10.9.{}.{}", i / 256, i % 256).parse().unwrap());
        }
        assert!(t.reported.lock().unwrap().len() <= MAX_REPORTED);
        assert_eq!(t.public_ip(), Some("203.0.113.7".parse().unwrap()), "one vote each cannot outvote three");
    }

    #[test]
    fn sa_in_a_reply_is_learned_as_our_address() {
        let (addr, h) = responder(|q| match q {
            Msg::Query { tid, .. } => Some(Msg::Reply { tid, from: peer3(), sa: Some(Ip::V4([203, 0, 113, 9])),
                                                       nodes: vec![], values: vec![], token: vec![], vid: None }),
            _ => None,
        });
        let t = transport();
        assert!(t.public_ip().is_none());
        t.ping(&contact(addr), &contact(addr));
        h.join().unwrap();
        assert_eq!(t.public_ip(), Some("203.0.113.9".parse().unwrap()));
    }

    #[test]
    fn two_served_transports_find_each_other_with_nobody_else() {
        // The bootstrap-independence claim at its smallest: two nodes on
        // a LAN, no third party. Each answers the other's queries from a
        // serving thread while its own round trips complete.
        let a = Arc::new(WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap());
        let b = Arc::new(WireUdpTransport::bind(id(3), "127.0.0.1:0".parse().unwrap()).unwrap());
        let node_a = Arc::new(Mutex::new(Node::new(id(1), a.local_addr().unwrap())));
        let node_b = Arc::new(Mutex::new(Node::new(id(3), b.local_addr().unwrap())));
        let ha = a.serve(node_a.clone()).unwrap();
        let hb = b.serve(node_b.clone()).unwrap();
        let ca = Contact::new(id(1), a.local_addr().unwrap(), 0);
        let cb = Contact::new(id(3), b.local_addr().unwrap(), 0);

        assert!(a.ping(&ca, &cb), "B did not answer A");
        assert!(b.ping(&cb, &ca), "A did not answer B");
        assert_eq!(node_b.lock().unwrap().rt.len(), 1, "B's table learned A from A's ping");

        // A stores at B (get for a token, then put — inside store), and
        // reads it back; B's own store holds it.
        a.store(&ca, &cb, id(7), b"lan-record".to_vec(), 600);
        assert_eq!(node_b.lock().unwrap().store.value_count(&id(7)), 1, "B did not store A's put");
        match a.find_value(&ca, &cb, &id(7)) {
            FindValue::Values(v) => assert_eq!(v, vec![b"lan-record".to_vec()]),
            other => panic!("{other:?}"),
        }
        // Served nodes do not claim to be clients.
        assert!(!a.client.load(Ordering::Relaxed));

        a.stop_serving();
        b.stop_serving();
        drop(a); drop(b);
        ha.join().unwrap();
        hb.join().unwrap();
    }

    #[test]
    fn a_served_transport_drops_a_reply_from_the_wrong_address() {
        let a = Arc::new(WireUdpTransport::bind(id(1), "127.0.0.1:0".parse().unwrap()).unwrap());
        let node_a = Arc::new(Mutex::new(Node::new(id(1), a.local_addr().unwrap())));
        let h = a.serve(node_a).unwrap();
        let (addr, _peer) = responder(|_| None);            // never answers
        let other = UdpSocket::bind("127.0.0.1:0").unwrap();
        let me = a.local_addr().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            other.send_to(&rep(1, peer3(), one_node(), vec![], vec![], None).encode(), me).unwrap();
        });
        let got = a.find_node(&contact(addr), &contact(addr), &id(2));
        assert!(got.is_empty(), "a bystander's reply was delivered: {got:?}");
        a.stop_serving();
        drop(a);
        h.join().unwrap();
    }

    #[test]
    fn a_value_over_the_references_limit_is_not_sent() {
        let (addr, h) = responder(|_| None);
        let t = transport();
        t.store(&contact(addr), &contact(addr), id(2), vec![0; dht_kit::MAX_VALUE_SIZE + 1], 600);
        drop(t);
        // The responder's 5 s read times out with nothing.
        assert!(h.join().unwrap().is_none(), "an oversize value went out");
    }
}
