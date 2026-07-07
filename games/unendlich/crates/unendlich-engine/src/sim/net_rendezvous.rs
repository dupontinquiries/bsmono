//! Mailbox-rendezvous + UDP hole-punch **protocol core** (r57-03, pure).
//!
//! This owns the wire **codec**, ICE-style candidate **pairing/priority**, and
//! the hole-punch **state machine**. It deliberately touches no OpenDHT, no
//! sockets, no threads, and no clock — those are bin glue. The bin:
//! - `put`s/`get`s the [`MailboxBlob::encode`]d blob to/from a DHT key,
//! - `send`s/`recv`s the UDP datagrams the FSM asks for via [`Action`],
//! - passes wall-time in as `now_ms: u64`, and supplies the `txid`/`nonce`
//!   seed (this module draws no randomness and reads no clock).
//!
//! Determinism: for a fixed sequence of events ([`HolePunch::on_remote_blob`],
//! [`HolePunch::on_packet`], [`HolePunch::poll`]) and `now_ms` values, the
//! output is identical every run. No RNG, I/O, GPU, wall-clock; no
//! `unwrap`/`expect`/`panic!`; `decode` never panics on hostile input.

// ---------------------------------------------------------------------------
// Candidates + ICE-style priority
// ---------------------------------------------------------------------------

/// Candidate type, most-preferred first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandKind {
    Host,
    Srflx,
    Relay,
}

impl CandKind {
    /// ICE type preference (Host > Srflx > Relay).
    fn type_pref(self) -> u32 {
        match self {
            CandKind::Host => 126,
            CandKind::Srflx => 100,
            CandKind::Relay => 0,
        }
    }
    fn to_byte(self) -> u8 {
        match self {
            CandKind::Host => 0,
            CandKind::Srflx => 1,
            CandKind::Relay => 2,
        }
    }
    fn from_byte(b: u8) -> Option<CandKind> {
        match b {
            0 => Some(CandKind::Host),
            1 => Some(CandKind::Srflx),
            2 => Some(CandKind::Relay),
            _ => None,
        }
    }
}

/// A transport candidate (address + how it was learned + its priority).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub ip: [u8; 4],
    pub port: u16,
    pub kind: CandKind,
    pub priority: u32,
}

/// ICE-style candidate priority: type preference dominates (so Host > Srflx >
/// Relay regardless of `local_pref`), ties broken by `local_pref`.
pub fn candidate_priority(kind: CandKind, local_pref: u16) -> u32 {
    (kind.type_pref() << 24) | ((local_pref as u32) << 8)
}

// ---------------------------------------------------------------------------
// Mailbox blob codec (versioned, length-prefixed, little-endian)
// ---------------------------------------------------------------------------

/// Current wire version.
pub const BLOB_VERSION: u8 = 1;

/// The rendezvous payload published to / fetched from the DHT mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxBlob {
    pub version: u8,
    pub peer_id: u64,
    pub session: u64,
    pub nonce: u64,
    pub candidates: Vec<Candidate>,
}

impl MailboxBlob {
    /// Encode to bytes. Layout (all little-endian):
    /// `[u8 version][u64 peer_id][u64 session][u64 nonce][u16 count]` then
    /// `count × ([4]u8 ip | u16 port | u8 kind | u32 priority)`.
    pub fn encode(&self) -> Vec<u8> {
        let count = self.candidates.len().min(u16::MAX as usize) as u16;
        let mut out = Vec::with_capacity(27 + count as usize * 11);
        out.push(self.version);
        out.extend_from_slice(&self.peer_id.to_le_bytes());
        out.extend_from_slice(&self.session.to_le_bytes());
        out.extend_from_slice(&self.nonce.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        for c in self.candidates.iter().take(count as usize) {
            out.extend_from_slice(&c.ip);
            out.extend_from_slice(&c.port.to_le_bytes());
            out.push(c.kind.to_byte());
            out.extend_from_slice(&c.priority.to_le_bytes());
        }
        out
    }

    /// Decode bytes. Returns `None` on truncation, trailing garbage, an
    /// unknown candidate kind, or a version this build doesn't understand —
    /// never panics.
    pub fn decode(buf: &[u8]) -> Option<MailboxBlob> {
        let mut r = Reader { b: buf, pos: 0 };
        let version = r.u8()?;
        if version != BLOB_VERSION {
            return None;
        }
        let peer_id = r.u64()?;
        let session = r.u64()?;
        let nonce = r.u64()?;
        let count = r.u16()? as usize;
        let mut candidates = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            let ip = r.ip4()?;
            let port = r.u16()?;
            let kind = CandKind::from_byte(r.u8()?)?;
            let priority = r.u32()?;
            candidates.push(Candidate { ip, port, kind, priority });
        }
        // Strict: no trailing bytes.
        if r.pos != buf.len() {
            return None;
        }
        Some(MailboxBlob { version, peer_id, session, nonce, candidates })
    }
}

/// Bounds-checked little-endian cursor; every read returns `None` if the buffer
/// is too short. Defined unconditionally (used by `decode` in release builds).
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.take(4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(u64::from_le_bytes(a))
    }
    fn ip4(&mut self) -> Option<[u8; 4]> {
        let s = self.take(4)?;
        Some([s[0], s[1], s[2], s[3]])
    }
}

// ---------------------------------------------------------------------------
// Hole-punch state machine
// ---------------------------------------------------------------------------

/// UDP packet kind exchanged during the punch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketKind {
    Ping,
    Pong,
}

/// Punch timing/limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PunchConfig {
    /// Retransmit timeout: re-send a ping with the same txid after this.
    pub rto_ms: u32,
    /// Max ping attempts per pair before moving on / failing.
    pub max_rounds: u8,
    /// Keepalive ping cadence once connected.
    pub keepalive_ms: u32,
}

impl Default for PunchConfig {
    fn default() -> Self {
        PunchConfig { rto_ms: 250, max_rounds: 7, keepalive_ms: 15_000 }
    }
}

/// What the bin should do (send a datagram / report status).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SendPing { pair: usize, txid: u64 },
    SendPong { pair: usize, txid: u64 },
    Connected { pair: usize },
    Fail,
}

/// Externally observable FSM state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PunchState {
    Idle,
    Gathering,
    Probing,
    Connected,
    Failed,
}

/// One local↔remote candidate pair under test.
#[derive(Clone, Copy, Debug)]
struct Pair {
    combined: u64,
    local_idx: usize,
    remote_idx: usize,
    txid: Option<u64>, // in-flight ping txid (reused across retransmits)
    attempts: u8,
    last_send_ms: u64,
    pending_pong: Option<u64>, // a received Ping to answer next poll
}

/// The hole-punch protocol object. Drive it with `on_remote_blob` / `on_packet`
/// (inputs) and `poll(now_ms)` (which emits [`Action`]s to perform).
#[derive(Clone, Debug)]
pub struct HolePunch {
    local: Vec<Candidate>,
    remote: Vec<Candidate>,
    cfg: PunchConfig,
    pairs: Vec<Pair>,
    active: usize,           // index into `pairs` currently being probed
    connected_pair: Option<usize>,
    last_keepalive_ms: u64,
    next_txid: u64,          // seeded deterministic counter
    state: PunchState,
}

impl HolePunch {
    /// New punch with local candidates and config. The txid counter is seeded
    /// deterministically at 1; use [`HolePunch::new_seeded`] to inject a seed
    /// (e.g. derived from the session) for cross-session txid uniqueness.
    pub fn new(local: Vec<Candidate>, cfg: PunchConfig) -> HolePunch {
        HolePunch::new_seeded(local, cfg, 1)
    }

    /// As [`HolePunch::new`] but with an explicit txid seed (the "seeded
    /// counter passed in"). No RNG is used — the counter advances by 1 per
    /// fresh txid.
    pub fn new_seeded(local: Vec<Candidate>, cfg: PunchConfig, txid_seed: u64) -> HolePunch {
        HolePunch {
            local,
            remote: Vec::new(),
            cfg,
            pairs: Vec::new(),
            active: 0,
            connected_pair: None,
            last_keepalive_ms: 0,
            next_txid: txid_seed.max(1),
            state: PunchState::Gathering,
        }
    }

    fn fresh_txid(&mut self) -> u64 {
        let t = self.next_txid;
        self.next_txid = self.next_txid.wrapping_add(1);
        t
    }

    /// Ingest the remote peer's mailbox blob: build local×remote candidate
    /// pairs and sort them by descending combined (ICE pair) priority.
    pub fn on_remote_blob(&mut self, blob: &MailboxBlob) {
        self.remote = blob.candidates.clone();
        self.pairs.clear();
        for (li, l) in self.local.iter().enumerate() {
            for (ri, rc) in self.remote.iter().enumerate() {
                self.pairs.push(Pair {
                    combined: pair_priority(l.priority, rc.priority),
                    local_idx: li,
                    remote_idx: ri,
                    txid: None,
                    attempts: 0,
                    last_send_ms: 0,
                    pending_pong: None,
                });
            }
        }
        // Highest combined priority first; deterministic tie-break by indices.
        self.pairs.sort_by(|a, b| {
            b.combined
                .cmp(&a.combined)
                .then(a.local_idx.cmp(&b.local_idx))
                .then(a.remote_idx.cmp(&b.remote_idx))
        });
        self.active = 0;
        self.state = if self.pairs.is_empty() {
            PunchState::Failed
        } else {
            PunchState::Probing
        };
    }

    /// Ingest a received UDP packet on `pair`. A `Ping` is queued to be
    /// answered with a `Pong` (echoing its `txid`) on the next poll. A `Pong`
    /// whose `txid` matches that pair's in-flight ping marks the pair (and the
    /// connection) Connected.
    pub fn on_packet(&mut self, pair: usize, kind: PacketKind, txid: u64, now_ms: u64) {
        match kind {
            PacketKind::Ping => {
                if let Some(p) = self.pairs.get_mut(pair) {
                    p.pending_pong = Some(txid);
                }
            }
            PacketKind::Pong => {
                let matches = self.pairs.get(pair).map(|p| p.txid) == Some(Some(txid));
                if matches && self.state != PunchState::Failed {
                    self.connected_pair = Some(pair);
                    self.last_keepalive_ms = now_ms;
                    self.state = PunchState::Connected;
                }
            }
        }
    }

    /// Advance the machine to `now_ms`, returning the actions to perform.
    pub fn poll(&mut self, now_ms: u64) -> Vec<Action> {
        let mut actions = Vec::new();

        // Always answer any received pings first (regardless of state).
        for (i, p) in self.pairs.iter_mut().enumerate() {
            if let Some(txid) = p.pending_pong.take() {
                actions.push(Action::SendPong { pair: i, txid });
            }
        }

        match self.state {
            PunchState::Failed | PunchState::Idle | PunchState::Gathering => {
                // Nothing to drive until a remote blob arrives (or terminal).
            }
            PunchState::Connected => {
                let cp = self.connected_pair.unwrap_or(0);
                if now_ms.saturating_sub(self.last_keepalive_ms) >= self.cfg.keepalive_ms as u64 {
                    let txid = self.fresh_txid();
                    if let Some(p) = self.pairs.get_mut(cp) {
                        p.txid = Some(txid);
                        p.last_send_ms = now_ms;
                    }
                    self.last_keepalive_ms = now_ms;
                    actions.push(Action::SendPing { pair: cp, txid });
                }
            }
            PunchState::Probing => {
                self.drive_probe(now_ms, &mut actions);
            }
        }

        actions
    }

    /// Probe the active pair: first ping assigns a fresh txid; subsequent polls
    /// retransmit the SAME txid only after `rto_ms`; after `max_rounds` with no
    /// answer, advance to the next pair, or `Fail` when all are exhausted.
    fn drive_probe(&mut self, now_ms: u64, actions: &mut Vec<Action>) {
        let rto = self.cfg.rto_ms as u64;
        let max_rounds = self.cfg.max_rounds;
        loop {
            let active = self.active;
            // Read current pair state by value (ends the borrow immediately).
            let snapshot = self.pairs.get(active).map(|p| (p.txid, p.attempts, p.last_send_ms));
            let Some((txid_opt, attempts, last_send)) = snapshot else {
                self.state = PunchState::Failed;
                actions.push(Action::Fail);
                return;
            };

            match txid_opt {
                None => {
                    let txid = self.fresh_txid();
                    if let Some(p) = self.pairs.get_mut(active) {
                        p.txid = Some(txid);
                        p.attempts = 1;
                        p.last_send_ms = now_ms;
                    }
                    actions.push(Action::SendPing { pair: active, txid });
                    return;
                }
                Some(txid) => {
                    if now_ms.saturating_sub(last_send) < rto {
                        return; // not due yet — no retransmit
                    }
                    if attempts < max_rounds {
                        if let Some(p) = self.pairs.get_mut(active) {
                            p.attempts = attempts + 1;
                            p.last_send_ms = now_ms;
                        }
                        actions.push(Action::SendPing { pair: active, txid });
                        return;
                    }
                    // Exhausted this pair → advance to the next, or fail.
                    if self.active + 1 < self.pairs.len() {
                        self.active += 1;
                        continue; // assign+send on the next pair this same poll
                    } else {
                        self.state = PunchState::Failed;
                        actions.push(Action::Fail);
                        return;
                    }
                }
            }
        }
    }

    /// Current FSM state.
    pub fn state(&self) -> PunchState {
        self.state
    }
}

/// ICE-style pair priority from two candidate priorities: favours the larger
/// pair, with a deterministic tiebreak. Higher = better.
fn pair_priority(a: u32, b: u32) -> u64 {
    let (g, d) = (a as u64, b as u64);
    let lo = g.min(d);
    let hi = g.max(d);
    (lo << 32) + 2 * hi + if g > d { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(kind: CandKind, lp: u16) -> Candidate {
        Candidate { ip: [10, 0, 0, lp as u8], port: 5000 + lp, kind, priority: candidate_priority(kind, lp) }
    }

    #[test]
    fn priority_orders_host_srflx_relay_then_local_pref() {
        let host = candidate_priority(CandKind::Host, 0);
        let srflx = candidate_priority(CandKind::Srflx, 65535);
        let relay = candidate_priority(CandKind::Relay, 65535);
        assert!(host > srflx, "Host beats Srflx even with lower local_pref");
        assert!(srflx > relay, "Srflx beats Relay");
        // ties broken by local_pref
        assert!(candidate_priority(CandKind::Host, 10) > candidate_priority(CandKind::Host, 5));
    }

    #[test]
    fn blob_round_trips() {
        let blob = MailboxBlob {
            version: BLOB_VERSION,
            peer_id: 0xDEAD_BEEF_1234_5678,
            session: 42,
            nonce: 99,
            candidates: vec![
                cand(CandKind::Host, 7),
                cand(CandKind::Srflx, 3),
                cand(CandKind::Relay, 1),
            ],
        };
        let bytes = blob.encode();
        assert_eq!(MailboxBlob::decode(&bytes), Some(blob.clone()));
        // empty candidate list round-trips too
        let empty = MailboxBlob { candidates: vec![], ..blob };
        assert_eq!(MailboxBlob::decode(&empty.encode()), Some(empty));
    }

    #[test]
    fn decode_rejects_garbage_truncation_and_bad_version() {
        let blob = MailboxBlob {
            version: BLOB_VERSION,
            peer_id: 1,
            session: 2,
            nonce: 3,
            candidates: vec![cand(CandKind::Host, 1)],
        };
        let good = blob.encode();
        // truncated
        for cut in 0..good.len() {
            assert_eq!(MailboxBlob::decode(&good[..cut]), None, "truncation at {cut} must reject");
        }
        // trailing garbage
        let mut extra = good.clone();
        extra.push(0xFF);
        assert_eq!(MailboxBlob::decode(&extra), None);
        // wrong version
        let mut badver = good.clone();
        badver[0] = 0xEE;
        assert_eq!(MailboxBlob::decode(&badver), None);
        // bad candidate kind byte (kind sits at offset 27 + 4 + 2 = 33)
        let mut badkind = good.clone();
        badkind[27 + 6] = 0x7F;
        assert_eq!(MailboxBlob::decode(&badkind), None);
        // pure garbage
        assert_eq!(MailboxBlob::decode(&[1, 2, 3]), None);
        assert_eq!(MailboxBlob::decode(&[]), None);
    }

    fn remote_blob() -> MailboxBlob {
        MailboxBlob {
            version: BLOB_VERSION,
            peer_id: 2,
            session: 1,
            nonce: 1,
            candidates: vec![cand(CandKind::Relay, 1), cand(CandKind::Host, 9)],
        }
    }

    #[test]
    fn pairs_sorted_descending_after_remote_blob() {
        let local = vec![cand(CandKind::Host, 5), cand(CandKind::Relay, 5)];
        let mut hp = HolePunch::new(local, PunchConfig::default());
        assert_eq!(hp.state(), PunchState::Gathering);
        hp.on_remote_blob(&remote_blob());
        assert_eq!(hp.state(), PunchState::Probing);
        // combined priorities must be non-increasing
        for w in hp.pairs.windows(2) {
            assert!(w[0].combined >= w[1].combined, "pairs not sorted descending");
        }
        // top pair should be Host(local)×Host(remote)
        let top = hp.pairs[0];
        assert_eq!(hp.local[top.local_idx].kind, CandKind::Host);
        assert_eq!(hp.remote[top.remote_idx].kind, CandKind::Host);
    }

    #[test]
    fn poll_pings_top_pair_retransmits_same_txid_only_after_rto() {
        let cfg = PunchConfig { rto_ms: 100, max_rounds: 3, keepalive_ms: 1000 };
        let mut hp = HolePunch::new(vec![cand(CandKind::Host, 5)], cfg);
        hp.on_remote_blob(&remote_blob());

        let a0 = hp.poll(0);
        let txid = match a0.as_slice() {
            [Action::SendPing { pair: 0, txid }] => *txid,
            other => panic!("expected initial SendPing, got {other:?}"),
        };
        // before rto: no retransmit
        assert_eq!(hp.poll(99), vec![], "must not retransmit before rto");
        // at rto: retransmit SAME txid
        assert_eq!(hp.poll(100), vec![Action::SendPing { pair: 0, txid }], "same txid retransmit");
    }

    #[test]
    fn pong_connects_the_pair() {
        let cfg = PunchConfig { rto_ms: 100, max_rounds: 3, keepalive_ms: 1000 };
        let mut hp = HolePunch::new(vec![cand(CandKind::Host, 5)], cfg);
        hp.on_remote_blob(&remote_blob());
        let a0 = hp.poll(0);
        let txid = match a0.as_slice() {
            [Action::SendPing { txid, .. }] => *txid,
            o => panic!("{o:?}"),
        };
        hp.on_packet(0, PacketKind::Pong, txid, 10);
        assert_eq!(hp.state(), PunchState::Connected);
        // wrong txid would not have connected
    }

    #[test]
    fn received_ping_is_answered_with_pong() {
        let mut hp = HolePunch::new(vec![cand(CandKind::Host, 5)], PunchConfig::default());
        hp.on_remote_blob(&remote_blob());
        hp.on_packet(0, PacketKind::Ping, 0xABCD, 5);
        let acts = hp.poll(5);
        assert!(acts.contains(&Action::SendPong { pair: 0, txid: 0xABCD }), "must echo a Pong");
    }

    #[test]
    fn fails_after_max_rounds_with_no_answer() {
        // single pair, so exhausting it fails the whole punch.
        let cfg = PunchConfig { rto_ms: 10, max_rounds: 3, keepalive_ms: 1000 };
        let local = vec![cand(CandKind::Host, 5)];
        let one_remote = MailboxBlob { candidates: vec![cand(CandKind::Host, 5)], ..remote_blob() };
        let mut hp = HolePunch::new(local, cfg);
        hp.on_remote_blob(&one_remote);
        let mut t = 0u64;
        let mut failed = false;
        for _ in 0..20 {
            for a in hp.poll(t) {
                if a == Action::Fail {
                    failed = true;
                }
            }
            t += 10;
        }
        assert!(failed, "should emit Fail after max_rounds");
        assert_eq!(hp.state(), PunchState::Failed);
    }

    #[test]
    fn keepalive_pings_once_connected() {
        let cfg = PunchConfig { rto_ms: 100, max_rounds: 3, keepalive_ms: 500 };
        let mut hp = HolePunch::new(vec![cand(CandKind::Host, 5)], cfg);
        hp.on_remote_blob(&remote_blob());
        let txid = match hp.poll(0).as_slice() {
            [Action::SendPing { txid, .. }] => *txid,
            o => panic!("{o:?}"),
        };
        hp.on_packet(0, PacketKind::Pong, txid, 0);
        assert_eq!(hp.poll(100), vec![], "no keepalive before cadence");
        let ka = hp.poll(500);
        assert!(
            matches!(ka.as_slice(), [Action::SendPing { .. }]),
            "keepalive ping at cadence, got {ka:?}"
        );
    }

    #[test]
    fn empty_pairs_fail() {
        let mut hp = HolePunch::new(vec![], PunchConfig::default());
        hp.on_remote_blob(&MailboxBlob { candidates: vec![], ..remote_blob() });
        assert_eq!(hp.state(), PunchState::Failed);
        assert_eq!(hp.poll(0), vec![]);
    }

    #[test]
    fn unknown_pair_index_is_ignored() {
        let mut hp = HolePunch::new(vec![cand(CandKind::Host, 5)], PunchConfig::default());
        hp.on_remote_blob(&remote_blob());
        hp.on_packet(999, PacketKind::Pong, 1, 0); // must not panic
        assert_ne!(hp.state(), PunchState::Connected);
    }

    #[test]
    fn deterministic_for_fixed_event_time_sequence() {
        let run = || {
            let cfg = PunchConfig { rto_ms: 50, max_rounds: 4, keepalive_ms: 500 };
            let mut hp = HolePunch::new(vec![cand(CandKind::Host, 5), cand(CandKind::Srflx, 2)], cfg);
            hp.on_remote_blob(&remote_blob());
            let mut log = Vec::new();
            for t in (0..400).step_by(25) {
                log.push(hp.poll(t));
                if t == 75 {
                    // pretend a pong for whatever the active pair's txid is
                    if let Some(tx) = hp.pairs.get(hp.active).and_then(|p| p.txid) {
                        hp.on_packet(hp.active, PacketKind::Pong, tx, t);
                    }
                }
            }
            (log, hp.state())
        };
        let a = run();
        for _ in 0..20 {
            assert_eq!(run(), a, "non-deterministic run");
        }
    }
}
