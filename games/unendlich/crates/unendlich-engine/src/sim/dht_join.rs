//! `sim::dht_join` — multiplayer peer discovery + join, as a PURE protocol
//! state machine.
//!
//! The hard split the brief demands: the join *logic* is a pure, deterministic
//! state machine — [`step`]`(state, input, now) -> (state, outgoing)` — with no
//! sockets and no clock-of-record (time is passed in). Real UDP/TCP send/recv is
//! a thin shell on top (compile-on-end in the bin); it only moves [`Envelope`]s
//! between nodes and calls [`step`]. Because the protocol never touches the
//! network, the whole bootstrap → find-node → join → table-convergence flow
//! (plus dropped-message retry and timeout) is unit-testable with simulated
//! message sequences.
//!
//! Landing: a [`DhtJoin`] service registered into `sim::service_registry` (peer
//! table + state machine), and a pure [`MpMenu`] host/join flow the Multiplayer
//! menu drives (the egui screen in `app::App` is the thin compile-on-end shell).
//!
//! Pure & deterministic: no `std::net`, no wall-clock, no real RNG (node ids
//! hash deterministically); no `.unwrap()`/`.expect()` in non-test code.

use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

/// A node identity in the id space (e.g. a hash of its address/pubkey).
pub type NodeId = u64;
/// Logical time in seconds — always passed in, never read from a clock.
pub type Time = f64;

/// Max peers held in a node's routing table.
pub const PEER_CAP: usize = 16;
/// How many closest peers we query / join against.
pub const CLOSEST_K: usize = 4;
/// Seconds before an outstanding request is considered lost.
pub const TIMEOUT: Time = 2.0;
/// Total attempts (initial + retries) before the join is declared failed.
pub const MAX_RETRIES: u32 = 3;

/// Wire messages. Deliberately tiny; `from`/`to` live on the [`Envelope`].
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Ping,
    Pong,
    /// "who do you know closest to `target`?"
    FindNode { target: NodeId },
    /// response to `FindNode` — closer peers we know.
    Nodes { target: NodeId, peers: Vec<NodeId> },
    /// joining node asks an existing node to admit it.
    JoinRequest,
    /// existing node admits the joiner and seeds it with peers.
    JoinAck { peers: Vec<NodeId> },
}

/// A message in flight: who it is from, who it is for, and the payload. The I/O
/// shell is responsible only for delivering these.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub from: NodeId,
    pub to: NodeId,
    pub msg: Msg,
}

/// What drives the machine one step: kick-off, a delivered message, or a poll
/// (no message — just lets `now` advance to fire timeouts/retries).
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Start,
    Deliver(Envelope),
    Poll,
}

/// Join lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Not joining (a host/seed sits here, serving others).
    Idle,
    /// Sent `FindNode`, awaiting `Nodes`.
    Finding,
    /// Sent `JoinRequest`, awaiting `JoinAck`.
    Joining,
    /// Joined: table populated.
    Joined,
    /// Gave up after `MAX_RETRIES`.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Awaiting {
    Nothing,
    Nodes,
    Ack,
}

/// A node's join state + routing table. Construct with [`DhtState::new`].
#[derive(Clone, Debug, PartialEq)]
pub struct DhtState {
    pub self_id: NodeId,
    pub bootstrap: Vec<NodeId>,
    pub peers: Vec<NodeId>,
    pub phase: Phase,
    attempts: u32,
    deadline: Time,
    awaiting: Awaiting,
}

impl DhtState {
    /// A fresh node. `bootstrap` may be empty (a host/seed that only serves).
    pub fn new(self_id: NodeId, bootstrap: Vec<NodeId>) -> DhtState {
        let mut s = DhtState {
            self_id,
            bootstrap: Vec::new(),
            peers: Vec::new(),
            phase: Phase::Idle,
            attempts: 0,
            deadline: 0.0,
            awaiting: Awaiting::Nothing,
        };
        for b in bootstrap {
            if b != self_id && !s.bootstrap.contains(&b) {
                s.bootstrap.push(b);
            }
        }
        s
    }

    /// True once the machine has reached a terminal phase.
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, Phase::Joined | Phase::Failed)
    }

    /// XOR distance from this node's id (Kademlia metric).
    fn dist(&self, id: NodeId) -> u64 {
        self.self_id ^ id
    }

    /// Insert a peer: never self, dedup, and when full keep the `PEER_CAP`
    /// closest to us by XOR distance (evict the farthest).
    fn learn(&mut self, id: NodeId) {
        if id == self.self_id || self.peers.contains(&id) {
            return;
        }
        if self.peers.len() < PEER_CAP {
            self.peers.push(id);
            return;
        }
        // table full: replace the farthest peer if the newcomer is closer.
        let mut farthest_idx = 0usize;
        let mut farthest_d = self.dist(self.peers[0]);
        for (i, &p) in self.peers.iter().enumerate() {
            let d = self.dist(p);
            if d > farthest_d {
                farthest_d = d;
                farthest_idx = i;
            }
        }
        if self.dist(id) < farthest_d {
            self.peers[farthest_idx] = id;
        }
    }

    /// The `k` peers closest to `target` by XOR distance (deterministic order).
    fn closest(&self, target: NodeId, k: usize) -> Vec<NodeId> {
        let mut v = self.peers.clone();
        // sort by (distance, id) so ties and order are deterministic.
        v.sort_by_key(|&p| ((target ^ p), p));
        v.truncate(k);
        v
    }
}

fn env(from: NodeId, to: NodeId, msg: Msg) -> Envelope {
    Envelope { from, to, msg }
}

/// Advance the join machine one step. Pure: same `(state, input, now)` always
/// yields the same `(state', outgoing)`. Never panics.
pub fn step(mut s: DhtState, input: Input, now: Time) -> (DhtState, Vec<Envelope>) {
    let mut out: Vec<Envelope> = Vec::new();
    match input {
        Input::Start => {
            // Begin discovery: ask every bootstrap who is closest to us.
            if s.bootstrap.is_empty() {
                // a host/seed with no bootstrap simply stays Idle and serves.
                return (s, out);
            }
            s.phase = Phase::Finding;
            s.attempts = 1;
            s.deadline = now + TIMEOUT;
            s.awaiting = Awaiting::Nodes;
            let me = s.self_id;
            for &b in &s.bootstrap {
                out.push(env(me, b, Msg::FindNode { target: me }));
            }
        }
        Input::Deliver(e) => {
            s.learn(e.from);
            match e.msg {
                Msg::Ping => out.push(env(s.self_id, e.from, Msg::Pong)),
                Msg::Pong => {}
                Msg::FindNode { target } => {
                    let peers = s.closest(target, CLOSEST_K);
                    out.push(env(s.self_id, e.from, Msg::Nodes { target, peers }));
                }
                Msg::Nodes { target: _, peers } => {
                    for p in peers {
                        s.learn(p);
                    }
                    if s.phase == Phase::Finding {
                        // Got candidates → ask the closest known peers to admit us.
                        s.phase = Phase::Joining;
                        s.attempts = 1;
                        s.deadline = now + TIMEOUT;
                        s.awaiting = Awaiting::Ack;
                        let me = s.self_id;
                        for p in s.closest(me, CLOSEST_K) {
                            out.push(env(me, p, Msg::JoinRequest));
                        }
                    }
                }
                Msg::JoinRequest => {
                    // We are an established node: admit the joiner, seed it.
                    let peers = s.closest(e.from, CLOSEST_K);
                    out.push(env(s.self_id, e.from, Msg::JoinAck { peers }));
                }
                Msg::JoinAck { peers } => {
                    for p in peers {
                        s.learn(p);
                    }
                    if !s.is_terminal() {
                        s.phase = Phase::Joined;
                        s.awaiting = Awaiting::Nothing;
                    }
                }
            }
        }
        Input::Poll => {
            // Fire a timeout only if we are waiting and the deadline has passed.
            if !s.is_terminal() && s.awaiting != Awaiting::Nothing && now >= s.deadline {
                if s.attempts < MAX_RETRIES {
                    s.attempts += 1;
                    s.deadline = now + TIMEOUT;
                    let me = s.self_id;
                    match s.awaiting {
                        Awaiting::Nodes => {
                            for &b in &s.bootstrap {
                                out.push(env(me, b, Msg::FindNode { target: me }));
                            }
                        }
                        Awaiting::Ack => {
                            for p in s.closest(me, CLOSEST_K) {
                                out.push(env(me, p, Msg::JoinRequest));
                            }
                        }
                        Awaiting::Nothing => {}
                    }
                } else {
                    s.phase = Phase::Failed;
                    s.awaiting = Awaiting::Nothing;
                }
            }
        }
    }
    (s, out)
}

// ─── Multiplayer menu flow (host / join) ────────────────────────────────────
// Pure intent state the Multiplayer screen drives; the egui rendering + real
// socket open is the compile-on-end shell in `app::App`.

/// Host (be the seed) or Join (dial a seed address).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpMode {
    Host,
    Join,
}

/// What the menu shows the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpStatus {
    Idle,
    Connecting,
    Joined,
    Failed,
}

/// Deterministic node id from a textual address (FNV-1a/64). Pure — no DNS.
pub fn node_id_from_addr(addr: &str) -> NodeId {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in addr.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Multiplayer menu state: mode, the address being typed, and a status line.
#[derive(Clone, Debug, PartialEq)]
pub struct MpMenu {
    pub mode: MpMode,
    pub address_buf: String,
    pub status: MpStatus,
}

impl MpMenu {
    pub fn new() -> MpMenu {
        MpMenu { mode: MpMode::Host, address_buf: String::new(), status: MpStatus::Idle }
    }

    pub fn set_mode(&mut self, mode: MpMode) {
        self.mode = mode;
    }

    pub fn type_address(&mut self, addr: &str) {
        self.address_buf = addr.to_string();
    }

    /// Produce the initial [`DhtState`] for the chosen mode. `self_addr` is this
    /// player's own address (→ id). Join uses the typed address as the sole
    /// bootstrap; Host starts with no bootstrap (it *is* the seed). Returns
    /// `None` for Join with an empty address. The caller then drives [`step`]
    /// with [`Input::Start`].
    pub fn begin(&mut self, self_addr: &str) -> Option<DhtState> {
        let self_id = node_id_from_addr(self_addr);
        match self.mode {
            MpMode::Host => {
                self.status = MpStatus::Idle; // listening as seed
                Some(DhtState::new(self_id, Vec::new()))
            }
            MpMode::Join => {
                let a = self.address_buf.trim();
                if a.is_empty() {
                    self.status = MpStatus::Failed;
                    return None;
                }
                self.status = MpStatus::Connecting;
                Some(DhtState::new(self_id, vec![node_id_from_addr(a)]))
            }
        }
    }

    /// Reflect a join phase back into the menu status line.
    pub fn on_phase(&mut self, phase: Phase) {
        self.status = match phase {
            Phase::Joined => MpStatus::Joined,
            Phase::Failed => MpStatus::Failed,
            Phase::Idle => MpStatus::Idle,
            _ => MpStatus::Connecting,
        };
    }
}

impl Default for MpMenu {
    fn default() -> Self {
        MpMenu::new()
    }
}

// ─── service_registry wiring (co-located, per the SHARED_RULES template) ─────
// A DhtJoin service: an Idle host node (empty bootstrap) carrying its own time.
// With no bootstrap and no Start it stays Idle, so `Poll` ticks are panic-free
// and a no-op — exactly the neutral wiring the smoke test wants. Real bootstrap
// peers + message delivery come from the bin's socket shell.

/// The service-owned DHT node: state + an accumulated clock.
pub struct DhtJoin {
    state: DhtState,
    now: Time,
}

impl DhtJoin {
    /// Default service node: a seed with id 0 and no bootstrap.
    pub fn new() -> DhtJoin {
        DhtJoin { state: DhtState::new(0, Vec::new()), now: 0.0 }
    }

    /// Advance time and poll the machine (the socket shell would instead feed
    /// `Input::Deliver` for real packets). Returns any outgoing envelopes.
    pub fn tick(&mut self, dt: f32) -> Vec<Envelope> {
        let dt = if dt.is_finite() && dt > 0.0 { dt as Time } else { 0.0 };
        self.now += dt;
        let prev = std::mem::replace(
            &mut self.state,
            DhtState::new(0, Vec::new()),
        );
        let (next, out) = step(prev, Input::Poll, self.now);
        self.state = next;
        out
    }

    pub fn phase(&self) -> Phase {
        self.state.phase
    }
    pub fn peers(&self) -> &[NodeId] {
        &self.state.peers
    }
}

impl Default for DhtJoin {
    fn default() -> Self {
        DhtJoin::new()
    }
}

struct DhtJoinService(DhtJoin);
impl Service for DhtJoinService {
    fn name(&self) -> &'static str {
        "dht_join"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let _ = self.0.tick(ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static DHT_JOIN_SERVICE: ServiceEntry = ServiceEntry {
    name: "dht_join",
    make: || Box::new(DhtJoinService(DhtJoin::new())),
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn n(id: NodeId, boot: Vec<NodeId>) -> DhtState {
        DhtState::new(id, boot)
    }

    #[test]
    fn bootstrap_emits_find_node_to_each_bootstrap() {
        let s = n(10, vec![1, 2, 3]);
        let (s, out) = step(s, Input::Start, 0.0);
        assert_eq!(s.phase, Phase::Finding);
        assert_eq!(out.len(), 3);
        for e in &out {
            assert_eq!(e.from, 10);
            assert!(matches!(e.msg, Msg::FindNode { target } if target == 10));
        }
        // empty-bootstrap host stays Idle and emits nothing.
        let (hs, hout) = step(n(99, vec![]), Input::Start, 0.0);
        assert_eq!(hs.phase, Phase::Idle);
        assert!(hout.is_empty());
    }

    #[test]
    fn find_node_gets_a_nodes_reply_and_learns_sender() {
        // an established node with some peers answers FindNode.
        let mut s = n(100, vec![]);
        for p in [1u64, 2, 3, 4, 5] {
            s.learn(p);
        }
        let (s, out) = step(s, Input::Deliver(env(7, 100, Msg::FindNode { target: 3 })), 0.0);
        assert!(s.peers.contains(&7), "should learn the requester");
        assert_eq!(out.len(), 1);
        match &out[0].msg {
            Msg::Nodes { target, peers } => {
                assert_eq!(*target, 3);
                assert!(peers.len() <= CLOSEST_K);
                // closest-to-3 should be 2 and 4 ahead of far ids
                assert!(peers.contains(&2) || peers.contains(&4));
            }
            _ => panic!("expected Nodes reply"),
        }
    }

    #[test]
    fn join_handshake_reaches_joined() {
        let joiner = n(50, vec![1]);
        let (joiner, out) = step(joiner, Input::Start, 0.0);
        assert_eq!(joiner.phase, Phase::Finding);
        assert_eq!(out.len(), 1); // FindNode -> bootstrap 1

        // bootstrap answers with some peers.
        let (joiner, out) = step(
            joiner,
            Input::Deliver(env(1, 50, Msg::Nodes { target: 50, peers: vec![1, 8, 9] })),
            0.1,
        );
        assert_eq!(joiner.phase, Phase::Joining);
        assert!(!out.is_empty()); // JoinRequest(s)
        assert!(out.iter().all(|e| e.msg == Msg::JoinRequest));

        // a peer admits us.
        let (joiner, _out) = step(
            joiner,
            Input::Deliver(env(8, 50, Msg::JoinAck { peers: vec![8, 9, 1, 12] })),
            0.2,
        );
        assert_eq!(joiner.phase, Phase::Joined);
        assert!(joiner.peers.contains(&8) && joiner.peers.contains(&12));
    }

    #[test]
    fn responding_node_admits_a_joinrequest() {
        let s = n(3, vec![]);
        let (s, out) = step(s, Input::Deliver(env(50, 3, Msg::JoinRequest)), 0.0);
        assert!(s.peers.contains(&50));
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].msg, Msg::JoinAck { .. }));
    }

    // Drive a whole little network until the joiner converges. A pure in-test
    // router shuttles envelopes between node states — no sockets.
    #[test]
    fn table_converges_with_n_peers() {
        let net_ids: Vec<NodeId> = vec![1, 2, 3, 4, 5, 6];
        let mut nodes: HashMap<NodeId, DhtState> = HashMap::new();
        // node 1 is the bootstrap and already knows everyone else.
        let mut boot = n(1, vec![]);
        for &p in &net_ids[1..] {
            boot.learn(p);
        }
        nodes.insert(1, boot);
        for &p in &net_ids[1..] {
            // the others know each other (so JoinAck seeds the joiner well).
            let mut s = n(p, vec![]);
            for &q in &net_ids {
                if q != p {
                    s.learn(q);
                }
            }
            nodes.insert(p, s);
        }
        // the joiner knows only the bootstrap address.
        let joiner_id: NodeId = 1000;
        nodes.insert(joiner_id, n(joiner_id, vec![1]));

        // kick off, then run a delivery loop.
        let mut queue: Vec<Envelope> = Vec::new();
        let mut t = 0.0;
        if let Some(j) = nodes.remove(&joiner_id) {
            let (j, out) = step(j, Input::Start, t);
            nodes.insert(joiner_id, j);
            queue.extend(out);
        }
        for _ in 0..200 {
            if queue.is_empty() {
                break;
            }
            t += 0.01;
            let e = queue.remove(0);
            let to = e.to;
            if let Some(node) = nodes.remove(&to) {
                let (node, out) = step(node, Input::Deliver(e), t);
                nodes.insert(to, node);
                queue.extend(out);
            }
        }
        let j = nodes.get(&joiner_id).expect("joiner exists");
        assert_eq!(j.phase, Phase::Joined, "joiner should have joined");
        // converged: learned several distinct network peers.
        let learned = j.peers.iter().filter(|p| net_ids.contains(p)).count();
        assert!(learned >= 3, "expected convergence, learned {}: {:?}", learned, j.peers);
    }

    #[test]
    fn dropped_message_triggers_retry() {
        let s = n(50, vec![1, 2]);
        let (s, _out) = step(s, Input::Start, 0.0); // deadline = TIMEOUT
        let before = s.attempts;
        // poll before the deadline: nothing happens.
        let (s, out) = step(s, Input::Poll, TIMEOUT * 0.5);
        assert!(out.is_empty());
        assert_eq!(s.attempts, before);
        // poll after the deadline: re-emit FindNode, attempts++.
        let (s, out) = step(s, Input::Poll, TIMEOUT + 0.01);
        assert_eq!(s.attempts, before + 1);
        assert_eq!(s.phase, Phase::Finding);
        assert!(!out.is_empty());
        assert!(out.iter().all(|e| matches!(e.msg, Msg::FindNode { .. })));
    }

    #[test]
    fn timeout_exhausts_to_failed() {
        let mut s = n(50, vec![1]);
        let (s0, _) = step(s, Input::Start, 0.0);
        s = s0;
        let mut t = 0.0;
        // keep polling past each deadline; after MAX_RETRIES, Failed.
        for _ in 0..MAX_RETRIES + 2 {
            t += TIMEOUT + 0.01;
            let (s1, _) = step(s, Input::Poll, t);
            s = s1;
        }
        assert_eq!(s.phase, Phase::Failed);
        // once failed, further polls are inert.
        let (s2, out) = step(s, Input::Poll, t + 100.0);
        assert_eq!(s2.phase, Phase::Failed);
        assert!(out.is_empty());
    }

    #[test]
    fn peer_table_bounded_no_self_dedup() {
        let mut s = n(0, vec![]);
        // self never inserted
        s.learn(0);
        assert!(s.peers.is_empty());
        // dedup
        s.learn(5);
        s.learn(5);
        assert_eq!(s.peers.iter().filter(|&&p| p == 5).count(), 1);
        // bounded at PEER_CAP, keeping closest to self (id 0 → smallest ids)
        for id in 1..100u64 {
            s.learn(id);
        }
        assert_eq!(s.peers.len(), PEER_CAP);
        let maxp = *s.peers.iter().max().unwrap_or(&0);
        // farthest kept should be <= PEER_CAP-ish small ids (closest to 0)
        assert!(maxp <= 64, "table should keep closest ids, got max {}", maxp);
    }

    #[test]
    fn closest_orders_by_xor_distance() {
        let mut s = n(0, vec![]);
        for id in [1u64, 2, 4, 8, 16, 32] {
            s.learn(id);
        }
        let c = s.closest(0, 3);
        assert_eq!(c, vec![1, 2, 4]); // smallest XOR distance to 0
        let c2 = s.closest(16, 2);
        assert_eq!(c2[0], 16); // distance 0 to itself
    }

    #[test]
    fn step_is_deterministic() {
        let a = step(n(50, vec![1, 2]), Input::Start, 0.0);
        let b = step(n(50, vec![1, 2]), Input::Start, 0.0);
        assert_eq!(a, b);
    }

    #[test]
    fn mp_menu_host_and_join_flow() {
        let mut m = MpMenu::new();
        // host: no bootstrap, idle.
        m.set_mode(MpMode::Host);
        let host = m.begin("me:7777").expect("host state");
        assert!(host.bootstrap.is_empty());
        assert_eq!(m.status, MpStatus::Idle);

        // join: address becomes the bootstrap; Start emits FindNode.
        m.set_mode(MpMode::Join);
        m.type_address("seed.example:7777");
        let joiner = m.begin("me:7777").expect("join state");
        assert_eq!(joiner.bootstrap.len(), 1);
        assert_eq!(m.status, MpStatus::Connecting);
        let (joiner, out) = step(joiner, Input::Start, 0.0);
        assert_eq!(out.len(), 1);
        assert_eq!(joiner.phase, Phase::Finding);

        // empty join address → failure, no state.
        m.type_address("   ");
        assert!(m.begin("me:7777").is_none());
        assert_eq!(m.status, MpStatus::Failed);

        // phase mirroring
        m.on_phase(Phase::Joined);
        assert_eq!(m.status, MpStatus::Joined);
    }

    #[test]
    fn service_tick_is_panic_free() {
        let mut svc = DhtJoin::new();
        for _ in 0..240 {
            let out = svc.tick(1.0 / 60.0);
            assert!(out.is_empty()); // idle seed, no bootstrap → no traffic
        }
        assert_eq!(svc.phase(), Phase::Idle);
        assert!(svc.peers().is_empty());
    }
}
