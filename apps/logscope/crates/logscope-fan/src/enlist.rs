//! enlist.rs — the "broadcast then await an invite" join protocol for multi-MACHINE fan-out.
//!
//! FLEET.md / `gpu_kernels::run_fleet` is the SINGLE-box fleet: many `Session`s sharing one resident
//! model on one GPU. This is the layer above it: separate MACHINES — each running an ollama API or a
//! logscope backend (`netserve`) — discover a project opening, broadcast their availability, and await
//! an invite, up to a hard cap of [`MAX_FLEET`] machines, before work is distributed across them.
//!
//! Shape (one round, tied together by a `nonce`):
//! ```text
//!   coordinator                              available machines (≤ many)
//!   ───────────                              ─────────────────────────────
//!   Opening{project,want,nonce} ──broadcast──▶  on_opening → Available{peer,cap,nonce} ──broadcast──▶
//!   gather Available (dedup) ◀───────────────
//!   select_invitees (rank, cap at want≤MAX_FLEET)
//!   Invite{peer,project,nonce}  ──direct────▶  on_invite → Accept{peer} | Decline{peer,reason}
//!   on_accept → member ; on_decline → promote next-best offer
//!   members == want ⇒ Ready (fan work out across members)
//! ```
//!
//! Like the rest of the crate (devloop, sandbox), the protocol + decisions are PURE and unit-tested;
//! the datagram/HTTP IO sits behind the [`Transport`] seam (real impl over `netserve` LAN HTTP or a UDP
//! broadcast; tests drive the handshake directly or via an in-memory transport). No GUI, no kernels.
#![allow(dead_code)]

/// Hard ceiling on machines that can join one project's fleet.
pub const MAX_FLEET: usize = 10;

/// A machine address / identity — typically `host:port` of its `netserve`/ollama endpoint.
pub type PeerId = String;

/// What a machine brings to the fleet: backend family, the models it can serve, and how many concurrent
/// workers (sessions) it offers. Kept as plain data (a `kind` string, not `backend::BackendKind`) so the
/// protocol stays frontend-free and decoupled from the engine crate.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Capability {
    pub kind: String,          // "ollama" | "wgpu" | "llamacpp" | …
    pub models: Vec<String>,   // model names this machine can serve
    pub slots: u32,            // concurrent workers it offers
}
impl Capability {
    pub fn new(kind: &str, models: &[&str], slots: u32) -> Self {
        Self { kind: kind.into(), models: models.iter().map(|s| s.to_string()).collect(), slots }
    }
}

/// The wire messages. Transport-agnostic; serialize however the `Transport` impl prefers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// Coordinator broadcasts a project opening and how many machines it wants.
    Opening { project: String, coordinator: PeerId, want: usize, nonce: u64 },
    /// A machine broadcasts that it is available for the round (and then awaits an invite).
    Available { peer: PeerId, cap: Capability, nonce: u64 },
    /// Coordinator invites one specific available machine.
    Invite { coordinator: PeerId, peer: PeerId, project: String, nonce: u64 },
    /// A machine accepts an invite — it has joined the fleet.
    Accept { peer: PeerId, project: String },
    /// A machine declines an invite (e.g. it filled up in the meantime).
    Decline { peer: PeerId, reason: String },
    /// Coordinator assigns a unit of work (an opaque `payload`) to a joined member.
    Assign { coordinator: PeerId, peer: PeerId, task: u64, payload: String },
    /// A member returns a completed task's output.
    Result { peer: PeerId, task: u64, output: String },
    /// A member could not complete a task (it will be requeued / reassigned).
    Failed { peer: PeerId, task: u64, reason: String },
}

/// Rank available offers and return up to `min(want, MAX_FLEET)` peers to invite: most `slots` first,
/// then most models, then earliest-arrived (the input order is the arrival order — a stable tiebreak so
/// the choice is deterministic and testable). Pure.
pub fn select_invitees(offers: &[(PeerId, Capability)], want: usize) -> Vec<PeerId> {
    select_invitees_for(offers, want, None, crate::workerkit::Duty::Build)
}

/// The same selection, GATED BY TRUST for the duty the fleet is being
/// raised to do.
///
/// `Trust::permits` encodes the rule that matters — "everything that
/// generates sees the prompt, which is conversation content" — and it had
/// ZERO CALLERS anywhere in the repo. It was written, documented and
/// tested, and nothing consulted it: `enlist` mentions trust nowhere, and
/// `dispatch` explicitly delegates membership to `enlist`, so a peer
/// registered as Build-only could be invited into a fleet and handed a
/// Thinker slice carrying the prompt. The registry was doing bookkeeping,
/// not enforcement.
///
/// This is that enforcement, at the admission point rather than deeper:
/// once a peer is a member, `dispatch` hands it whatever the queue holds,
/// so filtering at assignment would mean re-deciding per task with the
/// membership already granted. A peer that may not do the work should
/// never be invited to it.
///
/// `registry: None` keeps the old behaviour for callers that have no
/// registry yet — deliberately explicit rather than defaulting to
/// permissive silently, so the ungated path is visible at every call site
/// instead of being what you get by forgetting.
pub fn select_invitees_for(
    offers: &[(PeerId, Capability)],
    want: usize,
    registry: Option<&crate::trustkit::Registry>,
    duty: crate::workerkit::Duty,
) -> Vec<PeerId> {
    let offers: Vec<(PeerId, Capability)> = match registry {
        Some(reg) => offers
            .iter()
            .filter(|(id, _)| reg.level(id).permits(duty))
            .cloned()
            .collect(),
        None => offers.to_vec(),
    };
    let offers = &offers[..];
    let cap = want.min(MAX_FLEET);
    let mut idx: Vec<usize> = (0..offers.len()).collect();
    idx.sort_by(|&a, &b| {
        let (ca, cb) = (&offers[a].1, &offers[b].1);
        cb.slots.cmp(&ca.slots)
            .then_with(|| cb.models.len().cmp(&ca.models.len()))
            .then_with(|| a.cmp(&b)) // FIFO by arrival index
    });
    idx.into_iter().take(cap).map(|i| offers[i].0.clone()).collect()
}

// ── coordinator ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase { Announcing, Gathering, Inviting, Ready }

/// The project owner. Drives: broadcast an opening → gather availability → invite up to `want` machines
/// → track accepts, promoting the next-best offer when an invitee declines.
#[derive(Clone, Debug)]
pub struct Coordinator {
    pub project: String,
    pub id: PeerId,
    pub want: usize,                       // clamped to MAX_FLEET at construction
    nonce: u64,
    phase: Phase,
    offers: Vec<(PeerId, Capability)>,     // gathered availability, dedup by peer (arrival order kept)
    invited: Vec<PeerId>,                  // invited, awaiting a reply
    members: Vec<PeerId>,                  // accepted — the live fleet
    declined: Vec<PeerId>,
    /// The vouched-for machines, when the caller has a registry. `None` is the
    /// ungated legacy path and is spelled at construction rather than defaulted,
    /// so an ungated fleet is a choice someone made and not one they forgot.
    registry: Option<crate::trustkit::Registry>,
    /// What this fleet is being raised to DO. Admission is gated on it because
    /// `Trust::permits` draws its meaningful line between source and conversation:
    /// a Build-only box may compile, and must never be handed a prompt.
    duty: crate::workerkit::Duty,
}

impl Coordinator {
    pub fn new(project: &str, id: &str, want: usize, nonce: u64) -> Self {
        Self { project: project.into(), id: id.into(), want: want.min(MAX_FLEET), nonce,
               phase: Phase::Announcing, offers: Vec::new(), invited: Vec::new(),
               members: Vec::new(), declined: Vec::new(),
               registry: None, duty: crate::workerkit::Duty::Build }
    }

    /// Gate admission on a trust registry for `duty`.
    ///
    /// THE GAP THIS CLOSES. `select_invitees_for` was written, documented and
    /// tested with a registry parameter, and every caller in the fleet passed
    /// `None` — so the rule could be enforced and was not. `Coordinator` is
    /// where the fleet's membership is actually decided, so this is where the
    /// registry has to arrive; anything downstream (`dispatch`) delegates
    /// membership here and inherits whatever this admitted.
    ///
    /// Gating at ADMISSION rather than at assignment is deliberate and matches
    /// `select_invitees_for`'s own reasoning: once a peer is a member, dispatch
    /// hands it whatever the queue holds, so a peer that may not do the work
    /// must never be invited to it.
    pub fn with_trust(mut self, registry: crate::trustkit::Registry, duty: crate::workerkit::Duty) -> Self {
        self.registry = Some(registry);
        self.duty = duty;
        self
    }

    /// Broadcast the opening; moves to `Gathering`.
    pub fn opening(&mut self) -> Msg {
        self.phase = Phase::Gathering;
        Msg::Opening { project: self.project.clone(), coordinator: self.id.clone(), want: self.want, nonce: self.nonce }
    }

    /// Record an `Available` broadcast. Ignores the wrong nonce, self, and duplicates (latest cap wins).
    pub fn on_available(&mut self, peer: &PeerId, cap: &Capability, nonce: u64) {
        if nonce != self.nonce || peer == &self.id { return; }
        if cap.slots == 0 { return; }
        if let Some(slot) = self.offers.iter_mut().find(|(p, _)| p == peer) { slot.1 = cap.clone(); }
        else { self.offers.push((peer.clone(), cap.clone())); }
    }

    /// Close gathering and emit invites to the selected machines; moves to `Inviting`.
    pub fn invite(&mut self) -> Vec<Msg> {
        let picks = select_invitees_for(&self.offers, self.want, self.registry.as_ref(), self.duty);
        self.invited = picks.clone();
        self.phase = Phase::Inviting;
        picks.into_iter().map(|peer| self.invite_msg(peer)).collect()
    }

    fn invite_msg(&self, peer: PeerId) -> Msg {
        Msg::Invite { coordinator: self.id.clone(), peer, project: self.project.clone(), nonce: self.nonce }
    }

    /// An invited machine accepted — it joins the fleet.
    pub fn on_accept(&mut self, peer: &PeerId) {
        if !self.invited.iter().any(|p| p == peer) || self.members.iter().any(|p| p == peer) { return; }
        self.invited.retain(|p| p != peer);
        self.members.push(peer.clone());
        if self.members.len() >= self.want { self.phase = Phase::Ready; }
    }

    /// An invited machine declined — drop it and promote the best not-yet-tried offer, if room remains.
    /// Returns the replacement `Invite` to send, when one is available.
    pub fn on_decline(&mut self, peer: &PeerId, _reason: &str) -> Option<Msg> {
        self.invited.retain(|p| p != peer);
        if !self.declined.iter().any(|p| p == peer) { self.declined.push(peer.clone()); }
        if self.members.len() + self.invited.len() >= self.want { return None; }
        let next = self.best_untried()?;
        self.invited.push(next.clone());
        Some(self.invite_msg(next))
    }

    /// Best offer not already invited / a member / declined (same ranking as `select_invitees`).
    fn best_untried(&self) -> Option<PeerId> {
        let pool: Vec<(PeerId, Capability)> = self.offers.iter()
            .filter(|(p, _)| !self.invited.contains(p) && !self.members.contains(p) && !self.declined.contains(p))
            .cloned().collect();
        // Gated identically to `invite`. A replacement promoted after a decline is
        // an admission like any other; gating only the first round would mean a
        // fleet could acquire an untrusted member by the side door of someone
        // else declining.
        select_invitees_for(&pool, 1, self.registry.as_ref(), self.duty).into_iter().next()
    }

    /// Offers this coordinator will not invite, each with the reason.
    ///
    /// A filter that silently shortens a list is indistinguishable from a network
    /// where nobody answered, and those want opposite responses from an operator:
    /// one is "register that machine", the other is "go find out why it is down".
    /// `Trust::why_refused` already writes the sentence; this is the seam a panel
    /// reads it through. Empty when ungated — nothing is being refused.
    pub fn refused(&self) -> Vec<(PeerId, String)> {
        let Some(reg) = self.registry.as_ref() else { return Vec::new() };
        self.offers.iter()
            .filter(|(id, _)| !reg.level(id).permits(self.duty))
            .map(|(id, _)| (id.clone(), reg.level(id).why_refused(self.duty)))
            .collect()
    }

    /// The duty this fleet is gated for.
    pub fn duty(&self) -> crate::workerkit::Duty { self.duty }

    pub fn phase(&self) -> Phase { self.phase }
    pub fn members(&self) -> &[PeerId] { &self.members }
    pub fn is_full(&self) -> bool { self.members.len() >= self.want || self.members.len() >= MAX_FLEET }
}

// ── worker (an available machine) ───────────────────────────────────────────────────────────────--

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerState { Idle, Offered { project: String, nonce: u64 }, Joined { project: String } }

/// An available machine: hears an opening, broadcasts availability, awaits an invite, and accepts while
/// it still has free slots. `used` tracks slots already committed to other projects this round.
#[derive(Clone, Debug)]
pub struct Worker {
    pub id: PeerId,
    pub cap: Capability,
    used: u32,
    state: WorkerState,
}

impl Worker {
    pub fn new(id: &str, cap: Capability) -> Self {
        Self { id: id.into(), cap, used: 0, state: WorkerState::Idle }
    }

    fn available(&self) -> bool { self.used < self.cap.slots }

    /// React to an opening: if idle and not full, broadcast availability and await an invite.
    pub fn on_opening(&mut self, project: &str, nonce: u64) -> Option<Msg> {
        if self.state != WorkerState::Idle || !self.available() { return None; }
        self.state = WorkerState::Offered { project: project.into(), nonce };
        Some(Msg::Available { peer: self.id.clone(), cap: self.cap.clone(), nonce })
    }

    /// React to an invite: accept if it matches the offer we're awaiting and we still have a slot,
    /// otherwise decline. Accepting commits one slot.
    pub fn on_invite(&mut self, project: &str, nonce: u64) -> Msg {
        let matches = matches!(&self.state, WorkerState::Offered { project: p, nonce: n } if p == project && *n == nonce);
        if matches && self.available() {
            self.used += 1;
            self.state = WorkerState::Joined { project: project.into() };
            Msg::Accept { peer: self.id.clone(), project: project.into() }
        } else {
            let reason = if !self.available() { "no free slots" } else { "stale invite" };
            Msg::Decline { peer: self.id.clone(), reason: reason.into() }
        }
    }

    pub fn is_joined(&self) -> bool { matches!(self.state, WorkerState::Joined { .. }) }
    pub fn state(&self) -> &WorkerState { &self.state }
}

// ── transport seam (real IO lives here; tests use an in-memory impl) ───────────────────────────────

/// The datagram/HTTP seam. Real impls: a UDP broadcast socket for `Opening`/`Available`, direct
/// `netserve` HTTP POSTs for `Invite`/`Accept`/`Decline`. Kept tiny so the protocol above stays pure.
pub trait Transport {
    /// Send to every reachable peer (UDP broadcast / the known LAN list).
    fn broadcast(&mut self, msg: &Msg);
    /// Send directly to one peer.
    fn send(&mut self, to: &PeerId, msg: &Msg);
    /// Drain any inbound messages (non-blocking), tagged with the sender.
    fn recv(&mut self) -> Vec<(PeerId, Msg)>;
}

// ── wire codec (dependency-free text lines, one datagram each) ─────────────────────────────────────

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c { '%' => o.push_str("%25"), '\t' => o.push_str("%09"), '\n' => o.push_str("%0A"), ',' => o.push_str("%2C"), _ => o.push(c) }
    }
    o
}
fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '%' {
            let h: String = it.clone().take(2).collect();
            if h.len() == 2 { if let Ok(b) = u8::from_str_radix(&h, 16) { o.push(b as char); it.next(); it.next(); continue; } }
        }
        o.push(c);
    }
    o
}

/// Serialize a message to a single tab-delimited line (string fields percent-escaped). Pairs with [`decode`].
pub fn encode(m: &Msg) -> String {
    match m {
        Msg::Opening { project, coordinator, want, nonce } =>
            format!("OPEN\t{}\t{}\t{}\t{}", esc(project), esc(coordinator), want, nonce),
        Msg::Available { peer, cap, nonce } =>
            format!("AVAIL\t{}\t{}\t{}\t{}\t{}", esc(peer), esc(&cap.kind),
                    cap.models.iter().map(|s| esc(s)).collect::<Vec<_>>().join(","), cap.slots, nonce),
        Msg::Invite { coordinator, peer, project, nonce } =>
            format!("INVITE\t{}\t{}\t{}\t{}", esc(coordinator), esc(peer), esc(project), nonce),
        Msg::Accept { peer, project } => format!("ACCEPT\t{}\t{}", esc(peer), esc(project)),
        Msg::Decline { peer, reason } => format!("DECLINE\t{}\t{}", esc(peer), esc(reason)),
        Msg::Assign { coordinator, peer, task, payload } =>
            format!("ASSIGN\t{}\t{}\t{}\t{}", esc(coordinator), esc(peer), task, esc(payload)),
        Msg::Result { peer, task, output } => format!("RESULT\t{}\t{}\t{}", esc(peer), task, esc(output)),
        Msg::Failed { peer, task, reason } => format!("FAILED\t{}\t{}\t{}", esc(peer), task, esc(reason)),
    }
}

/// Parse a line produced by [`encode`]. Returns `None` on a malformed/unknown line.
pub fn decode(line: &str) -> Option<Msg> {
    let f: Vec<&str> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
    match f.as_slice() {
        ["OPEN", project, coordinator, want, nonce] =>
            Some(Msg::Opening { project: unesc(project), coordinator: unesc(coordinator), want: want.parse().ok()?, nonce: nonce.parse().ok()? }),
        ["AVAIL", peer, kind, models, slots, nonce] =>
            Some(Msg::Available { peer: unesc(peer), nonce: nonce.parse().ok()?,
                cap: Capability { kind: unesc(kind), slots: slots.parse().ok()?,
                    models: if models.is_empty() { Vec::new() } else { models.split(',').map(unesc).collect() } } }),
        ["INVITE", coordinator, peer, project, nonce] =>
            Some(Msg::Invite { coordinator: unesc(coordinator), peer: unesc(peer), project: unesc(project), nonce: nonce.parse().ok()? }),
        ["ACCEPT", peer, project] => Some(Msg::Accept { peer: unesc(peer), project: unesc(project) }),
        ["DECLINE", peer, reason] => Some(Msg::Decline { peer: unesc(peer), reason: unesc(reason) }),
        ["ASSIGN", coordinator, peer, task, payload] =>
            Some(Msg::Assign { coordinator: unesc(coordinator), peer: unesc(peer), task: task.parse().ok()?, payload: unesc(payload) }),
        ["RESULT", peer, task, output] => Some(Msg::Result { peer: unesc(peer), task: task.parse().ok()?, output: unesc(output) }),
        ["FAILED", peer, task, reason] => Some(Msg::Failed { peer: unesc(peer), task: task.parse().ok()?, reason: unesc(reason) }),
        _ => None,
    }
}

// ── shared-token auth (LAN bearer token, mirrors netserve.rs's posture) ─────────────────────────────
//
// Authenticates senders so a stray box on the LAN can't inject an Accept/Assign; it does NOT encrypt or
// resist replay — run on a trusted LAN or an SSH tunnel, exactly like netserve. Empty token = open.

/// Constant-time byte equality (no early-out), so a timing observer can't recover the token byte by byte.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) { diff |= x ^ y; }
    diff == 0
}

/// Prefix an encoded line with the shared token: `"<token>\t<line>"`.
pub fn frame(token: &str, line: &str) -> String { format!("{token}\t{line}") }

/// Verify (constant-time) and strip the token, returning the inner line — or `None` if it doesn't match.
pub fn unframe(token: &str, framed: &str) -> Option<String> {
    let (tok, rest) = framed.split_once('\t')?;
    if ct_eq(tok.as_bytes(), token.as_bytes()) { Some(rest.to_string()) } else { None }
}

// ── transport-driving glue (thin: protocol above, IO below) ────────────────────────────────────────

impl Coordinator {
    /// Broadcast the opening over `t`.
    pub fn open<T: Transport>(&mut self, t: &mut T) { let m = self.opening(); t.broadcast(&m); }

    /// Drain inbound availability/replies, applying each to the state machine and sending a promotion
    /// invite when an invitee declines. Call repeatedly until `phase() == Ready` (or a timeout elapses).
    pub fn pump<T: Transport>(&mut self, t: &mut T) {
        for (_from, msg) in t.recv() {
            match msg {
                Msg::Available { peer, cap, nonce } => self.on_available(&peer, &cap, nonce),
                Msg::Accept { peer, .. } => self.on_accept(&peer),
                Msg::Decline { peer, reason } => {
                    if let Some(inv) = self.on_decline(&peer, &reason) {
                        if let Msg::Invite { peer: ref p, .. } = inv { t.send(p, &inv); }
                    }
                }
                _ => {}
            }
        }
    }

    /// Select from the gathered offers and send the invites.
    pub fn close_and_invite<T: Transport>(&mut self, t: &mut T) {
        for inv in self.invite() {
            if let Msg::Invite { peer: ref p, .. } = inv { t.send(p, &inv); }
        }
    }
}

impl Worker {
    /// Drain inbound: broadcast availability in response to an opening, and accept/decline an invite
    /// addressed to us — replying to the coordinator named in the invite. Call repeatedly.
    pub fn serve<T: Transport>(&mut self, t: &mut T) {
        for (_from, msg) in t.recv() {
            match msg {
                Msg::Opening { project, nonce, .. } => {
                    if let Some(av) = self.on_opening(&project, nonce) { t.broadcast(&av); }
                }
                Msg::Invite { coordinator, peer, project, nonce } if peer == self.id => {
                    let reply = self.on_invite(&project, nonce);
                    t.send(&coordinator, &reply);
                }
                _ => {}
            }
        }
    }
}

// ── real LAN transport over UDP (integration-tested on boxes) ───────────────────────────────────────

/// `broadcast` sends to the subnet broadcast address; `send` is a unicast datagram to the peer's
/// `host:port` (a `PeerId` is exactly that). One `encode`d line per datagram. Best-effort: no acks or
/// retransmit yet — for a lossy LAN the coordinator simply re-broadcasts the opening and re-pumps, and
/// unanswered invites are re-issued on the next `close_and_invite`. (Acked delivery is the next step.)
pub struct UdpTransport { sock: std::net::UdpSocket, bcast: std::net::SocketAddr, token: String }

impl UdpTransport {
    /// Bind to `addr` (e.g. "0.0.0.0:48710") and target `broadcast` (e.g. "255.255.255.255:48710").
    pub fn bind(addr: &str, broadcast: &str) -> std::io::Result<Self> {
        let sock = std::net::UdpSocket::bind(addr)?;
        sock.set_broadcast(true)?;
        sock.set_nonblocking(true)?;
        let bcast = broadcast.parse()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad broadcast address"))?;
        Ok(Self { sock, bcast, token: String::new() })
    }

    /// Require a shared token on every datagram (sent framed, verified+stripped on receipt).
    pub fn with_token(mut self, token: &str) -> Self { self.token = token.into(); self }
}

impl Transport for UdpTransport {
    fn broadcast(&mut self, m: &Msg) { let _ = self.sock.send_to(frame(&self.token, &encode(m)).as_bytes(), self.bcast); }
    fn send(&mut self, to: &PeerId, m: &Msg) {
        if let Ok(addr) = to.parse::<std::net::SocketAddr>() {
            let _ = self.sock.send_to(frame(&self.token, &encode(m)).as_bytes(), addr);
        }
    }
    fn recv(&mut self) -> Vec<(PeerId, Msg)> {
        let mut out = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                        // drop anything that fails the token check before parsing
                        if let Some(line) = unframe(&self.token, s) {
                            if let Some(m) = decode(&line) { out.push((from.to_string(), m)); }
                        }
                    }
                }
                Err(_) => break, // WouldBlock (drained) or a transient error — stop draining this tick
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(kind: &str, n_models: usize, slots: u32) -> Capability {
        Capability { kind: kind.into(), models: (0..n_models).map(|i| format!("m{i}")).collect(), slots }
    }

    #[test]
    fn a_build_only_peer_is_never_invited_to_generate() {
        // THE GAP THIS CLOSES. `Trust::permits` says "everything that
        // generates sees the prompt, which is conversation content" — and
        // it had zero callers repo-wide. A Build-only machine could be
        // invited into a fleet and handed a Thinker slice carrying the
        // prompt, because enlist mentioned trust nowhere and dispatch
        // delegates membership to enlist.
        use crate::trustkit::{Registry, Trust};
        use crate::workerkit::Duty;
        let offers = vec![
            ("buildbox".to_string(), cap("cpu", 2, 8)),   // most slots: would rank FIRST
            ("trusted".to_string(), cap("gpu", 1, 2)),
        ];
        let mut reg = Registry::new();
        reg.register("buildbox", Trust::Build);
        reg.register("trusted", Trust::Compute);

        // Ungated, the build box wins on capacity — which is exactly how
        // it would have received conversation work.
        assert_eq!(select_invitees(&offers, 2)[0], "buildbox");

        // Gated for a generating duty, it is not invited at all.
        let picked = select_invitees_for(&offers, 2, Some(&reg), Duty::Thinker);
        assert_eq!(picked, vec!["trusted".to_string()], "a Build peer was invited to generate");

        // But it IS invited for the duty it is trusted with, or the gate
        // would be a ban rather than a boundary.
        let picked = select_invitees_for(&offers, 2, Some(&reg), Duty::Build);
        assert!(picked.contains(&"buildbox".to_string()), "a Build peer was refused a build");
        assert_eq!(picked.len(), 2, "both may build: {picked:?}");
    }

    #[test]
    fn an_unregistered_peer_is_invited_to_nothing() {
        // Unknown is the level a machine has by merely existing on the
        // network. It must not clear even the lowest duty, or discovery
        // alone becomes admission.
        use crate::trustkit::Registry;
        use crate::workerkit::Duty;
        let offers = vec![("stranger".to_string(), cap("gpu", 4, 16))];
        let reg = Registry::new();
        for duty in [Duty::Build, Duty::Thinker] {
            assert!(
                select_invitees_for(&offers, 4, Some(&reg), duty).is_empty(),
                "an unregistered peer was invited to {duty:?}"
            );
        }
    }

    #[test]
    fn no_registry_keeps_the_old_behaviour_visibly() {
        // `None` is spelled at the call site rather than defaulting,
        // so an ungated selection is something a reader can see.
        use crate::workerkit::Duty;
        let offers = vec![("a".to_string(), cap("cpu", 1, 4))];
        assert_eq!(select_invitees_for(&offers, 4, None, Duty::Thinker).len(), 1);
    }

    #[test]
    fn select_ranks_by_slots_then_models_and_caps_at_max_fleet() {
        let offers: Vec<(PeerId, Capability)> = (0..15)
            .map(|i| (format!("p{i:02}"), cap("ollama", i % 3, (i % 5) as u32 + 1))).collect();
        let picks = select_invitees(&offers, 100);
        assert_eq!(picks.len(), MAX_FLEET, "capped at the fleet ceiling regardless of want");
        // first pick has the max slots present (5)
        let top = &offers.iter().find(|(p, _)| p == &picks[0]).unwrap().1;
        assert_eq!(top.slots, 5);
    }

    #[test]
    fn want_is_clamped_to_max_fleet() {
        let c = Coordinator::new("proj", "coord", 50, 1);
        assert_eq!(c.want, MAX_FLEET);
    }

    #[test]
    fn full_handshake_reaches_target_and_goes_ready() {
        let mut coord = Coordinator::new("build", "coord:7000", 3, 42);
        let opening = coord.opening();
        let (project, nonce) = match &opening {
            Msg::Opening { project, nonce, .. } => (project.clone(), *nonce), _ => unreachable!(),
        };
        // five machines hear the opening and broadcast availability
        let mut workers: Vec<Worker> = (0..5)
            .map(|i| Worker::new(&format!("box{i}:7000"), cap("wgpu", 2, (i + 1) as u32))).collect();
        for w in workers.iter_mut() {
            if let Some(Msg::Available { peer, cap, nonce }) = w.on_opening(&project, nonce) {
                coord.on_available(&peer, &cap, nonce);
            }
        }
        assert_eq!(coord.phase(), Phase::Gathering);
        // coordinator invites the top 3 (most slots: box4, box3, box2)
        let invites = coord.invite();
        assert_eq!(invites.len(), 3);
        assert_eq!(coord.phase(), Phase::Inviting);
        for inv in invites {
            if let Msg::Invite { peer, project: pj, nonce: nn, .. } = inv {
                let w = workers.iter_mut().find(|w| w.id == peer).unwrap();
                match w.on_invite(&pj, nn) {
                    Msg::Accept { peer, .. } => coord.on_accept(&peer),
                    Msg::Decline { peer, reason } => { coord.on_decline(&peer, &reason); }
                    _ => unreachable!(),
                }
            }
        }
        assert_eq!(coord.members().len(), 3);
        assert_eq!(coord.phase(), Phase::Ready);
        assert!(coord.is_full());
        // the three highest-slot boxes joined
        assert!(coord.members().contains(&"box4:7000".to_string()));
        assert!(coord.members().contains(&"box3:7000".to_string()));
        assert!(coord.members().contains(&"box2:7000".to_string()));
    }

    #[test]
    fn decline_promotes_the_next_best_offer() {
        let mut coord = Coordinator::new("p", "c", 2, 7);
        // four offers with distinct slot counts so ranking is unambiguous
        for (id, slots) in [("a", 4u32), ("b", 3), ("d", 2), ("e", 1)] {
            coord.on_available(&id.to_string(), &cap("ollama", 1, slots), 7);
        }
        let invites = coord.invite();                       // invites a (4 slots) and b (3 slots)
        assert_eq!(invites.len(), 2);
        coord.on_accept(&"a".to_string());                  // a joins
        // b declines → coordinator should promote d (next best, 2 slots)
        let promote = coord.on_decline(&"b".to_string(), "busy");
        match promote { Some(Msg::Invite { peer, .. }) => assert_eq!(peer, "d"), other => panic!("{other:?}") }
        coord.on_accept(&"d".to_string());
        assert_eq!(coord.members(), &["a".to_string(), "d".to_string()]);
        assert_eq!(coord.phase(), Phase::Ready);
    }

    #[test]
    fn worker_declines_when_out_of_slots() {
        // one slot, already used → not available: offers nothing, and a (stale) invite is declined
        let mut w = Worker::new("box:7000", cap("wgpu", 1, 1));
        assert!(w.on_opening("p", 1).is_some());            // first offer ok
        match w.on_invite("p", 1) { Msg::Accept { .. } => {}, m => panic!("{m:?}") }
        assert!(w.is_joined());
        // a second opening for the same box now finds no free slot
        assert!(w.on_opening("p2", 2).is_none(), "no slots left to offer");
    }

    #[test]
    fn stale_or_mismatched_invite_is_declined() {
        let mut w = Worker::new("box:7000", cap("ollama", 1, 4));
        w.on_opening("alpha", 10);
        // wrong project
        assert!(matches!(w.on_invite("beta", 10), Msg::Decline { .. }));
        // wrong nonce
        assert!(matches!(w.on_invite("alpha", 11), Msg::Decline { .. }));
        // correct → accept
        assert!(matches!(w.on_invite("alpha", 10), Msg::Accept { .. }));
    }

    #[test]
    fn codec_roundtrips_all_messages() {
        // fields carry the escaped characters (tab, comma, percent, newline) to exercise esc/unesc
        let msgs = vec![
            Msg::Opening { project: "a\tb,c%x\n".into(), coordinator: "c:1".into(), want: 7, nonce: 99 },
            Msg::Available { peer: "box:1".into(), nonce: 99,
                cap: Capability { kind: "ollama".into(), models: vec!["llama3,8b".into(), "q\tr".into()], slots: 4 } },
            Msg::Available { peer: "empty:1".into(), nonce: 1, cap: Capability { kind: "wgpu".into(), models: vec![], slots: 1 } },
            Msg::Invite { coordinator: "c:1".into(), peer: "box:1".into(), project: "p".into(), nonce: 99 },
            Msg::Accept { peer: "box:1".into(), project: "p".into() },
            Msg::Decline { peer: "box:1".into(), reason: "no free slots".into() },
            Msg::Assign { coordinator: "c:1".into(), peer: "box:1".into(), task: 5, payload: "tokens\t1,2,3".into() },
            Msg::Result { peer: "box:1".into(), task: 5, output: "done\nlines".into() },
            Msg::Failed { peer: "box:1".into(), task: 5, reason: "oom".into() },
        ];
        for m in msgs { assert_eq!(decode(&encode(&m)), Some(m.clone()), "roundtrip {m:?}"); }
        assert_eq!(decode("GARBAGE\tx"), None);
    }

    #[test]
    fn auth_frames_roundtrip_and_reject_bad_token() {
        let line = encode(&Msg::Accept { peer: "b:1".into(), project: "p".into() });
        let f = frame("s3cret", &line);
        assert_eq!(unframe("s3cret", &f).as_deref(), Some(line.as_str()), "right token strips back to the line");
        assert_eq!(unframe("wrong", &f), None, "wrong token rejected");
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd") && !ct_eq(b"ab", b"abc"));
        // empty token = open (no auth)
        assert_eq!(unframe("", &frame("", &line)).as_deref(), Some(line.as_str()));
    }

    // In-memory transport: a shared bus delivering broadcasts to all peers and unicasts to one.
    struct Bus { inbox: std::collections::HashMap<PeerId, Vec<(PeerId, Msg)>>, peers: Vec<PeerId> }
    struct Loop { me: PeerId, bus: std::rc::Rc<std::cell::RefCell<Bus>> }
    impl Transport for Loop {
        fn broadcast(&mut self, m: &Msg) {
            let peers = self.bus.borrow().peers.clone();
            let mut b = self.bus.borrow_mut();
            for p in peers { if p != self.me { b.inbox.entry(p).or_default().push((self.me.clone(), m.clone())); } }
        }
        fn send(&mut self, to: &PeerId, m: &Msg) {
            self.bus.borrow_mut().inbox.entry(to.clone()).or_default().push((self.me.clone(), m.clone()));
        }
        fn recv(&mut self) -> Vec<(PeerId, Msg)> {
            self.bus.borrow_mut().inbox.get_mut(&self.me).map(std::mem::take).unwrap_or_default()
        }
    }

    #[test]
    fn loopback_handshake_over_transport_reaches_ready() {
        // 1 coordinator + 5 machines on one in-memory bus, driven entirely through the Transport seam
        // (open → serve → pump → invite → serve → pump). The three highest-slot boxes should join.
        use std::{rc::Rc, cell::RefCell};
        let cid = "coord:1".to_string();
        let wids: Vec<PeerId> = (0..5).map(|i| format!("box{i}:1")).collect();
        let bus = Rc::new(RefCell::new(Bus {
            inbox: std::collections::HashMap::new(),
            peers: std::iter::once(cid.clone()).chain(wids.clone()).collect(),
        }));
        let mut ct = Loop { me: cid.clone(), bus: bus.clone() };
        let mut coord = Coordinator::new("build", &cid, 3, 42);
        let mut workers: Vec<(Worker, Loop)> = wids.iter().enumerate()
            .map(|(i, id)| (Worker::new(id, cap("wgpu", 2, (i + 1) as u32)), Loop { me: id.clone(), bus: bus.clone() }))
            .collect();

        coord.open(&mut ct);                                  // broadcast opening
        for (w, wt) in workers.iter_mut() { w.serve(wt); }    // each broadcasts availability
        coord.pump(&mut ct);                                  // gather offers
        coord.close_and_invite(&mut ct);                      // invite top 3
        for (w, wt) in workers.iter_mut() { w.serve(wt); }    // invited boxes accept
        coord.pump(&mut ct);                                  // collect accepts

        assert_eq!(coord.phase(), Phase::Ready);
        assert_eq!(coord.members().len(), 3);
        assert!(coord.members().contains(&"box4:1".to_string()));
        assert!(coord.members().contains(&"box3:1".to_string()));
        assert!(coord.members().contains(&"box2:1".to_string()));
        let joined = workers.iter().filter(|(w, _)| w.is_joined()).count();
        assert_eq!(joined, 3);
    }

    // ── the coordinator actually consults the registry (C12-23f's open half) ──
    //
    // `select_invitees_for` took a registry and every caller in the fleet passed
    // `None`, so the rule could be enforced and was not. These pin that the
    // COORDINATOR — where membership is really decided — now passes one.

    /// A registry with the shape that matters: the biggest box is Build-only,
    /// so ungated ranking puts it FIRST and a gate has to actively remove it.
    fn build_box_ranks_first() -> (Vec<(PeerId, Capability)>, crate::trustkit::Registry) {
        use crate::trustkit::{Registry, Trust};
        let offers = vec![
            ("buildbox".to_string(), cap("cpu", 2, 8)),   // most slots: ranks first ungated
            ("trusted".to_string(), cap("gpu", 1, 2)),
        ];
        let mut reg = Registry::new();
        reg.register("buildbox", Trust::Build);
        reg.register("trusted", Trust::Compute);
        (offers, reg)
    }

    fn gather(c: &mut Coordinator, offers: &[(PeerId, Capability)], nonce: u64) {
        c.opening();
        for (p, cp) in offers { c.on_available(p, cp, nonce); }
    }

    #[test]
    fn a_gated_coordinator_refuses_to_invite_a_build_box_to_think() {
        use crate::workerkit::Duty;
        let (offers, reg) = build_box_ranks_first();
        let mut c = Coordinator::new("p", "coord", 2, 9).with_trust(reg, Duty::Thinker);
        gather(&mut c, &offers, 9);
        let invited: Vec<PeerId> = c.invite().into_iter()
            .filter_map(|m| match m { Msg::Invite { peer, .. } => Some(peer), _ => None }).collect();
        assert_eq!(invited, vec!["trusted".to_string()],
            "a Build-only peer must not be invited to a Thinker fleet even when it ranks first");
    }

    #[test]
    fn the_same_build_box_is_welcome_on_a_build_fleet() {
        // THE COMPLEMENT. A gate that refuses everything passes the test above
        // for the wrong reason; this is the half that says the filter still
        // discriminates rather than having simply stopped admitting anyone.
        use crate::workerkit::Duty;
        let (offers, reg) = build_box_ranks_first();
        let mut c = Coordinator::new("p", "coord", 2, 9).with_trust(reg, Duty::Build);
        gather(&mut c, &offers, 9);
        let invited: Vec<PeerId> = c.invite().into_iter()
            .filter_map(|m| match m { Msg::Invite { peer, .. } => Some(peer), _ => None }).collect();
        assert_eq!(invited.len(), 2, "Build duty admits both; got {invited:?}");
        assert_eq!(invited[0], "buildbox", "and ranking is unchanged inside the permitted set");
    }

    #[test]
    fn an_untrusted_peer_cannot_arrive_by_the_side_door_of_a_decline() {
        // Gating only `invite` would leave `on_decline`'s promotion ungated, so
        // a fleet could acquire the very peer it refused the moment someone else
        // dropped out. Ranking guarantees the refused box is the one promotion
        // would reach for: it has the most slots.
        use crate::workerkit::Duty;
        let (offers, reg) = build_box_ranks_first();
        let mut c = Coordinator::new("p", "coord", 2, 9).with_trust(reg, Duty::Thinker);
        gather(&mut c, &offers, 9);
        let _ = c.invite();
        let replacement = c.on_decline(&"trusted".to_string(), "full");
        assert!(replacement.is_none(),
            "the only remaining offer is Build-only; promoting it would hand a prompt to a peer \
             admission just refused, got {replacement:?}");
    }

    #[test]
    fn refusals_are_reported_rather_than_silently_shortening_the_list() {
        // A filtered list and an empty network look identical to an operator and
        // want opposite responses: "register that machine" vs "find out why it
        // is down". The reason string comes from Trust::why_refused, so the
        // wording lives with the rule.
        use crate::workerkit::Duty;
        let (offers, reg) = build_box_ranks_first();
        let mut c = Coordinator::new("p", "coord", 2, 9).with_trust(reg, Duty::Thinker);
        gather(&mut c, &offers, 9);
        let refused = c.refused();
        assert_eq!(refused.len(), 1, "exactly one offer is impermissible, got {refused:?}");
        assert_eq!(refused[0].0, "buildbox");
        assert!(refused[0].1.contains("Build") && refused[0].1.contains("Compute"),
            "the reason must name the level held and the level required: {}", refused[0].1);
    }

    #[test]
    fn an_ungated_coordinator_behaves_exactly_as_before() {
        // What did this change take away, and from whom? Callers with no
        // registry: they must be untouched, and `refused` must not invent
        // refusals for a coordinator that is not filtering.
        let (offers, _) = build_box_ranks_first();
        let mut c = Coordinator::new("p", "coord", 2, 9);
        gather(&mut c, &offers, 9);
        let invited: Vec<PeerId> = c.invite().into_iter()
            .filter_map(|m| match m { Msg::Invite { peer, .. } => Some(peer), _ => None }).collect();
        assert_eq!(invited.len(), 2);
        assert_eq!(invited[0], "buildbox");
        assert!(c.refused().is_empty(), "nothing is being refused when nothing is gating");
    }
}
