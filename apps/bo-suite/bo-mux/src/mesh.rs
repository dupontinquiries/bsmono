// src/mesh.rs
//
// The mesh layer: advertises this node over mDNS, browses for peer app
// instances + speakers, maintains a peer table, and drives the Election
// (protocol.rs) from incoming messages + a heartbeat timer.
//
// Transport: we use mDNS/DNS-SD for *discovery* (who's out there), and a
// small UDP multicast socket for the *coordination messages* themselves
// (Announce/Heartbeat/Claim/Assign/Start). UDP multicast on the LAN is the
// natural fit: low-latency, no connection setup, everyone hears everyone —
// exactly what the term-based election assumes.
//
// This file is a structured sketch: the message plumbing and the timing
// loop are real, but it's not compile-verified here (no toolchain in the
// build env). The pure logic it depends on — Election, resolve_assignments
// — IS verified by unit tests in protocol.rs.

use crate::protocol::*;
use std::collections::HashMap;
use std::net::{UdpSocket, SocketAddrV4, Ipv4Addr};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

const MCAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 12); // app-private group
const MCAST_PORT: u16 = 47712;
const SERVICE_TYPE: &str = "_bo-mux._udp.local.";

/// A peer we've heard from, with last-seen for liveness.
pub struct Peer {
    pub info: NodeInfo,
    pub last_seen: Instant,
}

/// Events the mesh emits up to the app (UI thread).
pub enum MeshEvent {
    PeersChanged(Vec<NodeInfo>),
    RoleChanged(String),                 // human-readable role for the UI
    Assignment(Vec<Claim>, Vec<Channel>),// final map + unclaimed
    StartPlayback { start_at_ms: u64, channel_base_url: String, my_claims: Vec<Claim> },
    Stopped,
    /// Remote control message: start overnight pass-by standby on
    /// this node, writing anti-noise to `sink_name`. The bo-muxd
    /// binary owns the actual canceller machinery; the mesh layer
    /// just routes the request.
    PassbyStandbyStart { sink_name: String },
    /// Remote control message: stop the overnight standby loop.
    PassbyStandbyStop,
    /// Mux node's broadcast of its current standby canceller state.
    /// The GUI subscribes to these to display per-node FSM phase.
    PassbyStatus {
        node_id: NodeId,
        phase: String,
        fundamental_hz: f32,
        doppler_hz_per_s: f32,
        timer_confidence: f32,
    },
}

pub struct Mesh {
    me: NodeInfo,
    election: Election,
    peers: HashMap<NodeId, Peer>,
    sock: UdpSocket,
    out: Sender<MeshEvent>,
    /// Last time we broadcast a Hello presence message.
    last_hello: Instant,
    /// Last time we (as coordinator) sent a heartbeat. Member nodes
    /// leave this at the construction instant; only coordinators
    /// update it.
    last_heartbeat: Instant,
    last_coord_seen: Instant,
    // pending claims gathered while we (as coordinator) wait to assign
    pending_claims: Vec<Claim>,
    /// Optional peer-auth registry. When `Some`, all outbound
    /// messages are HMAC-signed and inbound messages must verify
    /// against a known peer. When `None`, the mesh runs in legacy
    /// open mode — useful for tests + single-machine development,
    /// but a production deployment on a shared LAN should always
    /// populate this.
    auth: Option<crate::peer_auth::KeyRegistry>,
    /// Count of rejected inbound messages — for the UI surfacing
    /// "your mesh is under attack" warnings. Atomic so the inbound
    /// path doesn't need a write lock.
    rejected_count: std::sync::atomic::AtomicU64,
}

impl Mesh {
    pub fn new(me: NodeInfo, out: Sender<MeshEvent>) -> std::io::Result<Self> {
        Self::new_with_auth(me, out, None)
    }

    /// Like `new`, but with an explicit peer-auth registry. When
    /// `auth = Some(reg)`, outbound messages are signed with reg's
    /// self_peer key and inbound messages are dropped unless they
    /// verify against a known peer in reg.
    pub fn new_with_auth(
        me: NodeInfo,
        out: Sender<MeshEvent>,
        auth: Option<crate::peer_auth::KeyRegistry>,
    ) -> std::io::Result<Self> {
        let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, MCAST_PORT))?;
        sock.join_multicast_v4(&MCAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;
        sock.set_read_timeout(Some(Duration::from_millis(200)))?;
        sock.set_multicast_loop_v4(false)?;
        let id = me.node_id.clone();
        Ok(Self {
            election: Election::new(id),
            me,
            peers: HashMap::new(),
            sock,
            out,
            last_hello: Instant::now(),
            last_heartbeat: Instant::now(),
            last_coord_seen: Instant::now(),
            pending_claims: Vec::new(),
            auth,
            rejected_count: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Total count of messages rejected by the auth layer. Surfaces
    /// to the UI as a "mesh attack" indicator: legitimate traffic
    /// produces a near-zero number, while a flood from an
    /// unauthorized peer (or replay attempts) makes it climb.
    pub fn rejected_count(&self) -> u64 {
        self.rejected_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn send(&self, msg: &Msg) {
        let Ok(body) = serde_json::to_vec(msg) else { return; };
        // When auth is enabled, wrap the body in a signed envelope;
        // otherwise send the raw body for backward compatibility
        // with legacy/test peers.
        let bytes = match &self.auth {
            Some(reg) => match crate::peer_auth::sign(reg, &body) {
                Some(env) => env,
                None => return,  // can't sign — drop rather than send unsigned
            },
            None => body,
        };
        let dst = SocketAddrV4::new(MCAST_ADDR, MCAST_PORT);
        let _ = self.sock.send_to(&bytes, dst);
    }

    /// Local intent from the UI: start multi-speaker playback with `layout`.
    /// Pin a layer's channel to a specific speaker by USER request.
    /// Injected as a cost-0 claim, which always beats auto-assignment
    /// (auto uses cost 5-100). When this node is coordinator the claim
    /// goes straight into pending_claims; otherwise it's broadcast as a
    /// ClaimChannel for the current coordinator to merge. The next
    /// assignment resolve picks it up, so the drag-to-speaker action in
    /// the UI takes effect within one mesh tick.
    pub fn pin_claim(&mut self, channel: Channel, speaker: SpeakerId) {
        let claim = Claim {
            node_id: self.me.node_id.clone(),
            channel,
            speaker,
            cost: 0, // user pin — always wins
        };
        match &self.election.role {
            Role::Coordinator { term, .. } => {
                let term = *term;
                // De-dup: a re-pin of the same channel replaces the old.
                self.pending_claims.retain(|c| c.channel != claim.channel);
                self.pending_claims.push(claim.clone());
                self.send(&Msg::ClaimChannel { term, claim });
            }
            Role::Member { term, .. } => {
                let term = *term;
                self.send(&Msg::ClaimChannel { term, claim });
            }
            _ => {
                // No session yet — stash so it applies when one starts.
                self.pending_claims.retain(|c| c.channel != claim.channel);
                self.pending_claims.push(claim);
            }
        }
    }

    pub fn start_session(&mut self, layout: Layout) {
        let announce = self.election.announce(layout);
        self.send(&announce);
        self.send_role();
    }

    pub fn stop_session(&mut self) {
        if let Role::Coordinator { term, .. } = &self.election.role {
            self.send(&Msg::Stop { term: *term });
        }
        let _ = self.out.send(MeshEvent::Stopped);
    }

    /// Run one iteration of the mesh loop. Call repeatedly from a thread.
    pub fn tick(&mut self) {
        // 1. Drain inbound messages
        let mut buf = [0u8; 4096];
        while let Ok((n, _src)) = self.sock.recv_from(&mut buf) {
            // Under auth: verify the envelope first; drop on any
            // failure. Without auth: parse the raw body as before.
            let body_bytes: Vec<u8> = match &self.auth {
                Some(reg) => match crate::peer_auth::verify(reg, &buf[..n]) {
                    Ok((peer_id, body)) => {
                        // Per-peer rate gate: even a peer with a valid
                        // secret can't flood the mesh. A compromised
                        // peer that the attacker controls would
                        // otherwise be free to flood signed messages
                        // until the legitimate user manually rotates
                        // the key. Quarantining the flooding peer
                        // contains the blast radius.
                        if !crate::peer_auth::peer_rate_ok(&peer_id) {
                            self.rejected_count.fetch_add(
                                1, std::sync::atomic::Ordering::Relaxed);
                            continue;
                        }
                        body
                    }
                    Err(_) => {
                        // Bump the counter so the UI can surface
                        // "mesh under attack" if it climbs fast.
                        // Don't log per message — an attacker
                        // flooding would generate log spam that's
                        // itself a DoS vector.
                        self.rejected_count.fetch_add(
                            1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                },
                None => buf[..n].to_vec(),
            };
            if let Ok(msg) = serde_json::from_slice::<Msg>(&body_bytes) {
                self.handle(msg);
            }
        }

        // 2. Hello broadcast (presence) every ~2s.
        //
        // Previously this gated on `last_heartbeat`, which is only
        // updated when WE are coordinator (line below). For a member
        // node `last_heartbeat` stayed at construction time forever
        // and `.elapsed() > 2s` evaluated true on every tick after
        // the first 2 seconds — flooding the multicast group with
        // Hello messages at the 200ms-tick cadence. Split into
        // `last_hello` (rate-limits the Hello specifically) and
        // `last_heartbeat` (only updated by the coordinator path
        // below).
        if self.last_hello.elapsed() > Duration::from_secs(2) {
            self.send(&Msg::Hello(self.me.clone()));
            self.last_hello = Instant::now();
        }

        // 3. Coordinator duties: heartbeat + (once) resolve assignments
        if let Role::Coordinator { term, layout } = self.election.role.clone() {
            if self.last_heartbeat.elapsed() > Duration::from_secs(1) {
                self.send(&Msg::Heartbeat { term, coordinator: self.me.node_id.clone() });
                self.last_heartbeat = Instant::now();
            }
            // Resolve assignments. The merge here is the key
            // robustness improvement for small (2-3 speaker)
            // setups: we synthesize sensible default claims from
            // the active peer list and combine them with any
            // explicit claims, then run the same lowest-cost
            // resolver. Explicit user claims (cost ~0) always win
            // because the auto-assigner uses cost 100.
            //
            // The peer list is sorted by node_id so the
            // assignment is stable across ticks — the same peer
            // gets the same channel each time, which avoids the
            // confusing "wait, why did my left and right
            // suddenly swap?" UX after a peer-list refresh.
            let mut peer_list: Vec<NodeInfo> =
                self.peers.values().map(|p| p.info.clone()).collect();
            // Include ourselves so we participate in the
            // auto-assignment for 2-3 speakers (the user's phone
            // counts as one of the speakers).
            peer_list.push(self.me.clone());
            peer_list.sort_by(|a, b| a.node_id.cmp(&b.node_id));
            // Cap the count we auto-assign for: more than 4 peers
            // implies a deliberate configuration where guessing
            // is rude. Let explicit claims handle larger meshes.
            let auto_claims = if peer_list.len() <= 4 {
                smart_default_assignments(&layout, &peer_list)
            } else { Vec::new() };

            // Merge: explicit claims (low cost) override defaults
            // (cost 100). Empty pending_claims + non-empty
            // auto_claims = pure auto-assignment, which is the
            // typical 2-3-speaker flow.
            let mut combined: Vec<Claim> = Vec::new();
            combined.extend(self.pending_claims.iter().cloned());
            combined.extend(auto_claims);
            if !combined.is_empty() {
                let (assigned, unclaimed) = resolve_assignments(&layout, &combined);
                let _ = self.out.send(MeshEvent::Assignment(assigned.clone(), unclaimed));
                self.send(&Msg::Assign { term, map: assigned });
                // (Start is issued separately once the user confirms / after
                // a short gather window — see schedule_start.)
            }
        } else {
            // 4. Member: if coordinator went silent, drop to idle.
            if self.last_coord_seen.elapsed() > Duration::from_secs(3) {
                self.election.on_coordinator_lost();
                self.send_role();
            }
        }

        // 5. Prune dead peers.
        let before = self.peers.len();
        self.peers.retain(|_, p| p.last_seen.elapsed() < Duration::from_secs(8));
        if self.peers.len() != before { self.emit_peers(); }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Hello(info) => {
                let id = info.node_id.clone();
                self.peers.insert(id, Peer { info, last_seen: Instant::now() });
                self.emit_peers();
            }
            Msg::Announce { term, coordinator, layout } => {
                if self.election.on_announce(term, &coordinator, &layout) {
                    self.last_coord_seen = Instant::now();
                    self.send_role();
                    // As a fresh member, claim channels we can serve.
                    self.offer_claims(term, &layout);
                }
            }
            Msg::Heartbeat { term, coordinator } => {
                if coordinator != self.me.node_id {
                    self.last_coord_seen = Instant::now();
                    if self.election.on_heartbeat(term, &coordinator) {
                        self.send_role();
                    }
                }
            }
            Msg::ClaimChannel { term, claim } => {
                if let Role::Coordinator { term: my_term, .. } = &self.election.role {
                    if term == *my_term { self.pending_claims.push(claim); }
                }
            }
            Msg::Assign { term, map } => {
                // A member learns its responsibilities.
                if let Role::Member { term: my_term, .. } = &self.election.role {
                    if term == *my_term {
                        let mine: Vec<Claim> = map.iter()
                            .filter(|c| c.node_id == self.me.node_id)
                            .cloned().collect();
                        let _ = self.out.send(MeshEvent::Assignment(mine, Vec::new()));
                    }
                }
            }
            Msg::Start { term, start_at_ms, channel_base_url } => {
                if term >= self.election.highest_term_seen {
                    // Hand the UI/audio layer the scheduled start + what to play.
                    let my_claims = self.pending_claims.iter()
                        .filter(|c| c.node_id == self.me.node_id).cloned().collect();
                    let _ = self.out.send(MeshEvent::StartPlayback {
                        start_at_ms, channel_base_url, my_claims,
                    });
                }
            }
            Msg::Stop { .. } => { let _ = self.out.send(MeshEvent::Stopped); }
            Msg::StartPassbyStandby { sink_name } => {
                // Bubble the request up to the bo-muxd binary, which
                // owns the actual MicCapture + PassbyCanceller. The
                // mesh layer is transport-only — it doesn't run the
                // canceller itself.
                let _ = self.out.send(MeshEvent::PassbyStandbyStart { sink_name });
            }
            Msg::StopPassbyStandby => {
                let _ = self.out.send(MeshEvent::PassbyStandbyStop);
            }
            Msg::PassbyStatus {
                node_id, phase, fundamental_hz, doppler_hz_per_s, timer_confidence,
            } => {
                // Forward upstream so the GUI / coordinator can show
                // per-node FSM state.
                let _ = self.out.send(MeshEvent::PassbyStatus {
                    node_id, phase, fundamental_hz,
                    doppler_hz_per_s, timer_confidence,
                });
            }
        }
    }

    /// As a member, offer to drive any channel we have a speaker for. The
    /// real cost metric would factor link quality / user pinning; here we
    /// use a simple heuristic (local speakers cheap, networked dearer).
    fn offer_claims(&mut self, term: u64, layout: &Layout) {
        // Naive v1: claim channels round-robin against our speakers. A real
        // impl would map speaker physical position → channel role.
        for (i, ch) in layout.channels.iter().enumerate() {
            if let Some(spk) = self.me.speakers.get(i % self.me.speakers.len().max(1)) {
                let cost = match spk.platform {
                    Platform::Linux | Platform::Phone => 5,  // local output, low latency
                    _ => 10,                                  // networked speaker
                };
                self.send(&Msg::ClaimChannel {
                    term,
                    claim: Claim {
                        node_id: self.me.node_id.clone(),
                        channel: ch.clone(),
                        speaker: spk.id.clone(),
                        cost,
                    },
                });
            }
        }
    }

    fn emit_peers(&self) {
        let list: Vec<NodeInfo> = self.peers.values().map(|p| p.info.clone()).collect();
        let _ = self.out.send(MeshEvent::PeersChanged(list));
    }

    fn send_role(&self) {
        let role = match &self.election.role {
            Role::Idle => "idle".to_string(),
            Role::Coordinator { .. } => "coordinator".to_string(),
            Role::Member { coordinator, .. } => format!("member of {}", coordinator),
        };
        let _ = self.out.send(MeshEvent::RoleChanged(role));
    }
}

// Note on SERVICE_TYPE / mDNS: in the integrated app we reuse the existing
// mdns-sd daemon (added for speaker discovery) to also advertise + browse
// `_bo-mux._udp`. The TXT record carries node_id + platform; the actual
// coordination messages flow over the UDP multicast group above, not mDNS.
const _: &str = SERVICE_TYPE;
