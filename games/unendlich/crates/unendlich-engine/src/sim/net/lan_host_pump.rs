//! `sim::net::lan_host_pump` — the reusable **host endpoint**: one UDP session
//! socket + the [`super::lan_host::LanHost`] peer table + the channel mux the
//! fullgame's inline loop pioneered, factored so the headless
//! `unendlich-server` can host joinable worlds too (the "next step" its own
//! header named). Like [`super::transport`], this is the thin I/O shell living
//! in the crate: real sockets, but every decision it makes is delegated to the
//! pure layers (`lan_host` admission/fanout, `lan_session_wire` handshake,
//! `entity_snapshot` gating, `world_cmd_wire` codec).
//!
//! Channel mux (one leading byte, identical to the fullgame's inline loop so
//! the two interoperate on a LAN):
//!
//! | byte | channel     | payload                                             |
//! |------|-------------|-----------------------------------------------------|
//! | 1    | HANDSHAKE   | `lan_session_wire::LanJoin`                         |
//! | 2    | ENTITY      | `entity_replication::seal` bytes                    |
//! | 3    | WORLDCMD    | `packet_compress::pack(world_cmd_wire::encode_batch)`|
//! | 4    | AVATAR      | `[entity_id u64 LE][appearance 12 B]`               |
//!
//! The pump is a **star-relay**: every non-handshake datagram from a peer is
//! forwarded verbatim to every *other* live peer (fanout excludes the source),
//! which is what lets joiner A see joiner B through the host. Entity/world/
//! avatar payloads are additionally decoded into [`PumpEvent`]s so the
//! embedder can apply them (the server applies `WorldCmd`s to its
//! authoritative `ChunkManager`; a windowed host feeds avatars/entities to its
//! render path).
//!
//! Verified in-sandbox by loopback tests (two real UDP sockets on 127.0.0.1):
//! join → Accept-with-assigned-id, snapshot intake, and A→host→B relay.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

use super::entity_replication::receive;
use super::entity_snapshot::{EntityState, SnapshotGate};
use super::lan_host::{Admit, DenyReason, LanHost};
use super::world_cmd_wire::decode_batch;
use crate::sim::lan_session_wire::LanJoin;
use crate::sim::world_authority::WorldCmd;

/// Channel bytes — must stay identical to the fullgame's inline loop.
pub const CH_HANDSHAKE: u8 = 1;
pub const CH_ENTITY: u8 = 2;
pub const CH_WORLDCMD: u8 = 3;
pub const CH_AVATAR: u8 = 4;
/// Host → joiner world-edit catch-up (join_sync records, packed). Joiners
/// apply; the pump does NOT relay it (it is per-peer targeted state).
pub const CH_JOINSYNC: u8 = 5;

/// Peers silent this long are pruned (matches the fullgame's avatar timeout).
pub const PEER_TIMEOUT_MS: u64 = 5000;

/// Decoded upper bound for one WORLDCMD batch (same as the inline loop).
const WORLDCMD_MAX_BYTES: usize = 64 * 1024;

/// What happened during a [`HostPump::pump`] drain, for the embedder to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum PumpEvent {
    /// A joiner was admitted (or re-admitted after a lost Accept / timeout).
    /// `addr` is the peer's session address — the target for join-sync
    /// catch-up sends (`send_to_peer`).
    PeerJoined { name: String, entity_id: u64, rejoined: bool, addr: SocketAddrV4 },
    /// A join was refused (already replied with the Deny).
    PeerDenied { name: String, reason: DenyReason },
    /// A peer went silent past the ttl and was dropped.
    PeerLeft { entity_id: u64 },
    /// Fresh (gate-passing) entity states from a peer.
    Entities(Vec<EntityState>),
    /// Decoded world commands from a peer, ready to apply to the world.
    WorldCmds(Vec<(u64, WorldCmd)>),
    /// A peer's character appearance (12-byte profile encoding).
    Avatar { entity_id: u64, appearance: [u8; 12] },
}

/// The host endpoint: socket + peer table + snapshot gate.
pub struct HostPump {
    sock: UdpSocket,
    pub host: LanHost,
    gate: SnapshotGate,
    /// Data-channel datagrams dropped because their source was not an admitted peer (never
    /// joined, denied, or pruned and not yet re-joined). A counter rather than an event: a
    /// stranger can send as fast as it likes, and the status line is where an operator sees it.
    pub dropped_unadmitted: u64,
}

impl HostPump {
    /// Bind the session socket (preferred port first, ephemeral fallback —
    /// the fullgame's r405 behavior) and stand up the peer table. `host_name`
    /// and `host_entity_id` seed the roster's Host entry.
    pub fn bind(preferred_port: u16, host_name: &str, host_entity_id: u64) -> io::Result<HostPump> {
        let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, preferred_port))
            .or_else(|_| UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
        sock.set_nonblocking(true)?;
        Ok(HostPump { sock, host: LanHost::new(host_name, host_entity_id), gate: SnapshotGate::new(), dropped_unadmitted: 0 })
    }

    /// The actually-bound session port (what adverts should carry).
    pub fn port(&self) -> u16 {
        self.sock.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Drain every pending datagram: run admissions (replying Accept/Deny),
    /// gate + surface entity states, decode world commands, learn avatars,
    /// relay everything non-handshake to the other peers, and prune silent
    /// peers. Non-blocking; call once per tick/frame.
    ///
    /// `admit` is the access policy (e.g. `|name| whitelist.admits(name)`), and it is the ONLY
    /// way in: a data-channel datagram (entity, world command, avatar) from an address that is
    /// not an admitted peer is dropped -- not applied, not relayed. Until c21 the admission
    /// decided who got an Accept and nothing else; `note_seen`'s answer was thrown away, so a
    /// denied name, or anyone who never sent a Join, could edit the world and be relayed to
    /// every player, and the whitelist was a formality. A pruned peer is a stranger again until
    /// its next Join (the fullgame re-sends one when the host goes quiet on it).
    pub fn pump(
        &mut self,
        now_ms: u64,
        world_name: &str,
        world_seed: u64,
        world_time_ms: u64,
        admit: &dyn Fn(&str) -> bool,
    ) -> Vec<PumpEvent> {
        let mut events = Vec::new();
        let mut buf = [0u8; 1500];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, SocketAddr::V4(src))) if n >= 1 => {
                    let payload = &buf[1..n];
                    match buf[0] {
                        CH_HANDSHAKE => {
                            if let Some(LanJoin::Join { name }) = LanJoin::parse(payload) {
                                self.handle_join(
                                    &name, src, now_ms, world_name, world_seed,
                                    world_time_ms, admit, &mut events,
                                );
                            }
                        }
                        CH_ENTITY => {
                            if !self.host.note_seen(src, now_ms) {
                                self.dropped_unadmitted += 1;
                                continue;
                            }
                            if let Some(states) = receive(&mut self.gate, payload) {
                                if !states.is_empty() {
                                    events.push(PumpEvent::Entities(states));
                                }
                            }
                            self.relay(&buf[..n], src);
                        }
                        CH_WORLDCMD => {
                            if !self.host.note_seen(src, now_ms) {
                                self.dropped_unadmitted += 1;
                                continue;
                            }
                            if let Some(raw) =
                                super::packet_compress::unpack(payload, WORLDCMD_MAX_BYTES)
                            {
                                if let Some(cmds) = decode_batch(&raw) {
                                    if !cmds.is_empty() {
                                        events.push(PumpEvent::WorldCmds(cmds));
                                    }
                                }
                            }
                            self.relay(&buf[..n], src);
                        }
                        CH_AVATAR => {
                            if !self.host.note_seen(src, now_ms) {
                                self.dropped_unadmitted += 1;
                                continue;
                            }
                            if payload.len() >= 20 {
                                let mut idb = [0u8; 8];
                                idb.copy_from_slice(&payload[0..8]);
                                let mut app = [0u8; 12];
                                app.copy_from_slice(&payload[8..20]);
                                events.push(PumpEvent::Avatar {
                                    entity_id: u64::from_le_bytes(idb),
                                    appearance: app,
                                });
                            }
                            self.relay(&buf[..n], src);
                        }
                        _ => {}
                    }
                }
                Ok(_) => {} // empty datagram or non-v4 source: ignore
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        for eid in self.host.prune(now_ms, PEER_TIMEOUT_MS) {
            events.push(PumpEvent::PeerLeft { entity_id: eid });
        }
        events
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_join(
        &mut self,
        name: &str,
        src: SocketAddrV4,
        now_ms: u64,
        world_name: &str,
        world_seed: u64,
        world_time_ms: u64,
        admit: &dyn Fn(&str) -> bool,
        events: &mut Vec<PumpEvent>,
    ) {
        match self.host.admit(name, src, admit(name), now_ms) {
            Admit::Accepted { entity_id, rejoined } => {
                let reply = LanJoin::Accept {
                    world_name: world_name.to_string(),
                    world_seed,
                    entity_id: Some(entity_id),
                    world_time_ms: Some(world_time_ms),
                }
                .encode();
                self.send_framed(CH_HANDSHAKE, &reply, src);
                events.push(PumpEvent::PeerJoined { name: name.to_string(), entity_id, rejoined, addr: src });
            }
            Admit::Denied(reason) => {
                let text = match reason {
                    DenyReason::NotAllowed => "not on this server's whitelist",
                    DenyReason::Full => "world full",
                };
                let reply = LanJoin::Deny { reason: text.to_string() }.encode();
                self.send_framed(CH_HANDSHAKE, &reply, src);
                events.push(PumpEvent::PeerDenied { name: name.to_string(), reason });
            }
        }
    }

    /// Forward a raw framed datagram to every live peer except its source.
    fn relay(&self, datagram: &[u8], from: SocketAddrV4) {
        for target in self.host.fanout(Some(from)) {
            let _ = self.sock.send_to(datagram, target);
        }
    }

    /// Send `payload` on `channel` to every live peer (host-originated data:
    /// the server's own authoritative entity states, world edits, etc.).
    pub fn broadcast(&self, channel: u8, payload: &[u8]) {
        let mut dg = Vec::with_capacity(1 + payload.len());
        dg.push(channel);
        dg.extend_from_slice(payload);
        for target in self.host.fanout(None) {
            let _ = self.sock.send_to(&dg, target);
        }
    }

    /// Send a payload on a channel to ONE peer — the join-sync catch-up path
    /// (targeted, unlike broadcast). Public so the embedder can drain a
    /// `join_sync::SyncPlan` at its own pace.
    pub fn send_to_peer(&self, channel: u8, payload: &[u8], to: SocketAddrV4) {
        self.send_framed(channel, payload, to);
    }

    fn send_framed(&self, channel: u8, payload: &[u8], to: SocketAddrV4) {
        let mut dg = Vec::with_capacity(1 + payload.len());
        dg.push(channel);
        dg.extend_from_slice(payload);
        let _ = self.sock.send_to(&dg, to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::net::entity_replication::seal;
    use crate::sim::net::entity_snapshot::{AuthorityStamper, EntityKind};

    /// A minimal in-test client: real UDP socket on loopback.
    struct Client {
        sock: UdpSocket,
        host: SocketAddrV4,
    }

    impl Client {
        fn connect(host_port: u16) -> Client {
            let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            sock.set_nonblocking(true).unwrap();
            Client { sock, host: SocketAddrV4::new(Ipv4Addr::LOCALHOST, host_port) }
        }
        fn send(&self, channel: u8, payload: &[u8]) {
            let mut dg = vec![channel];
            dg.extend_from_slice(payload);
            self.sock.send_to(&dg, self.host).unwrap();
        }
        fn recv(&self) -> Option<(u8, Vec<u8>)> {
            let mut buf = [0u8; 1500];
            // Loopback delivery is fast but not instantaneous; poll briefly.
            for _ in 0..50 {
                match self.sock.recv_from(&mut buf) {
                    Ok((n, _)) if n >= 1 => return Some((buf[0], buf[1..n].to_vec())),
                    _ => std::thread::sleep(std::time::Duration::from_millis(2)),
                }
            }
            None
        }
    }

    fn pump_on_loopback() -> HostPump {
        // Port 0 = ephemeral: tests can't collide with each other or a real host.
        HostPump::bind(0, "host", 9878).expect("bind loopback pump")
    }

    #[test]
    fn join_is_accepted_with_a_host_assigned_id() {
        let mut pump = pump_on_loopback();
        let client = Client::connect(pump.port());
        client.send(CH_HANDSHAKE, &LanJoin::Join { name: "alice".into() }.encode());
        // Give loopback a moment, then drain.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(0, "TestWorld", 1234, 777_000, &|_| true);
        assert!(
            matches!(events.as_slice(), [PumpEvent::PeerJoined { name, rejoined: false, .. }] if name == "alice"),
            "expected one PeerJoined, got {events:?}"
        );
        // The client received an Accept carrying the assigned id.
        let (ch, payload) = client.recv().expect("no Accept received");
        assert_eq!(ch, CH_HANDSHAKE);
        match LanJoin::parse(&payload) {
            Some(LanJoin::Accept { world_name, world_seed, entity_id, world_time_ms }) => {
                assert_eq!(world_name, "TestWorld");
                assert_eq!(world_seed, 1234);
                let eid = entity_id.expect("Accept must carry the assigned entity id");
                assert!(eid >= crate::sim::net::lan_host::ASSIGNED_EID_BASE);
                assert_eq!(world_time_ms, Some(777_000), "join-at-any-time: the host's clock rides the Accept");
            }
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[test]
    fn a_denied_join_gets_a_deny_and_no_peer() {
        let mut pump = pump_on_loopback();
        let client = Client::connect(pump.port());
        client.send(CH_HANDSHAKE, &LanJoin::Join { name: "mallory".into() }.encode());
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(0, "W", 1, 0, &|_| false);
        assert!(matches!(
            events.as_slice(),
            [PumpEvent::PeerDenied { reason: DenyReason::NotAllowed, .. }]
        ));
        assert_eq!(pump.host.live_peer_count(), 0);
        let (_, payload) = client.recv().expect("no Deny received");
        assert!(matches!(LanJoin::parse(&payload), Some(LanJoin::Deny { .. })));
    }

    #[test]
    fn entity_states_are_gated_in_and_relayed_to_the_other_peer() {
        let mut pump = pump_on_loopback();
        let a = Client::connect(pump.port());
        let b = Client::connect(pump.port());
        for c in [&a, &b] {
            c.send(CH_HANDSHAKE, &LanJoin::Join { name: "p".into() }.encode());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        let joins = pump.pump(0, "W", 1, 0, &|_| true);
        assert_eq!(joins.len(), 2, "both should join: {joins:?}");
        let eid_a = match &joins[0] {
            PumpEvent::PeerJoined { entity_id, .. } => *entity_id,
            other => panic!("{other:?}"),
        };
        // Drain the Accepts so B's next recv is the relay.
        a.recv().unwrap();
        b.recv().unwrap();

        // A sends a sealed snapshot of itself.
        let mut stamper = AuthorityStamper::new();
        let me = EntityState {
            id: eid_a,
            kind: EntityKind::Player,
            x: Fixed::from_f64(1.5),
            y: Fixed::from_f64(70.0),
            z: Fixed::from_f64(-3.25),
            yaw: 1234,
            pitch: 0,
            seq: 0,
        };
        a.send(CH_ENTITY, &seal(&mut stamper, 1, &[me]));
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(100, "W", 1, 0, &|_| true);

        // The pump surfaced the state to the embedder…
        let surfaced = events.iter().any(|e| {
            matches!(e, PumpEvent::Entities(states)
                if states.len() == 1 && states[0].id == eid_a && states[0].yaw == 1234)
        });
        assert!(surfaced, "state not surfaced: {events:?}");

        // …and relayed the raw datagram to B (verbatim: channel byte + seal bytes).
        let (ch, payload) = b.recv().expect("relay to the other peer never arrived");
        assert_eq!(ch, CH_ENTITY);
        let mut gate = SnapshotGate::new();
        let states = receive(&mut gate, &payload).expect("relayed bytes must still decode");
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].id, eid_a);

        // A must NOT have been echoed its own datagram.
        assert!(a.recv().is_none(), "relay echoed back to the source");
    }

    /// The admission is the only way in: a stranger's edit, entity state and avatar are
    /// neither surfaced nor relayed, a denied joiner is a stranger, and so is a peer the host
    /// pruned -- until the next Join.
    #[test]
    fn data_from_an_unadmitted_address_is_dropped_not_applied_or_relayed() {
        use crate::sim::net::world_cmd_wire::encode_batch;
        let mut pump = pump_on_loopback();
        let member = Client::connect(pump.port());
        member.send(CH_HANDSHAKE, &LanJoin::Join { name: "alice".into() }.encode());
        std::thread::sleep(std::time::Duration::from_millis(10));
        pump.pump(0, "W", 1, 0, &|_| true);
        member.recv().expect("alice's Accept");

        let batch = vec![(7u64, WorldCmd::Remove { pos: (10, 64, -4) })];
        let edit = super::super::packet_compress::pack(&encode_batch(&batch), 48);
        let avatar = [0u8; 20];

        // never joined
        let stranger = Client::connect(pump.port());
        stranger.send(CH_WORLDCMD, &edit);
        stranger.send(CH_AVATAR, &avatar);
        stranger.send(CH_ENTITY, &[0u8; 8]);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(100, "W", 1, 0, &|_| true);
        assert!(events.is_empty(), "a stranger's data surfaced: {events:?}");
        assert_eq!(pump.dropped_unadmitted, 3);
        assert!(member.recv().is_none(), "a stranger's data was relayed to alice");

        // denied by the policy
        let denied = Client::connect(pump.port());
        denied.send(CH_HANDSHAKE, &LanJoin::Join { name: "mallory".into() }.encode());
        std::thread::sleep(std::time::Duration::from_millis(10));
        pump.pump(200, "W", 1, 0, &|name| name != "mallory");
        denied.recv().expect("mallory's Deny");
        denied.send(CH_WORLDCMD, &edit);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(300, "W", 1, 0, &|name| name != "mallory");
        assert!(events.is_empty(), "a denied joiner's edit surfaced: {events:?}");
        assert_eq!(pump.dropped_unadmitted, 4);
        assert!(member.recv().is_none());

        // the member's own edit still lands, so the gate is the address and not the channel
        member.send(CH_WORLDCMD, &edit);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(400, "W", 1, 0, &|_| true);
        assert!(events.iter().any(|e| matches!(e, PumpEvent::WorldCmds(c) if c == &batch)), "{events:?}");
        assert_eq!(pump.dropped_unadmitted, 4);

        // pruned: a stranger again until it re-joins
        let events = pump.pump(400 + PEER_TIMEOUT_MS + 1, "W", 1, 0, &|_| true);
        assert!(events.iter().any(|e| matches!(e, PumpEvent::PeerLeft { .. })), "{events:?}");
        member.send(CH_WORLDCMD, &edit);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t = 400 + PEER_TIMEOUT_MS + 2;
        let events = pump.pump(t, "W", 1, 0, &|_| true);
        assert!(events.is_empty(), "a pruned peer's edit surfaced: {events:?}");
        assert_eq!(pump.dropped_unadmitted, 5);
        member.send(CH_HANDSHAKE, &LanJoin::Join { name: "alice".into() }.encode());
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(t + 1, "W", 1, 0, &|_| true);
        assert!(events.iter().any(|e| matches!(e, PumpEvent::PeerJoined { rejoined: true, .. })), "{events:?}");
        member.recv().expect("the re-Accept");
        member.send(CH_WORLDCMD, &edit);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(t + 2, "W", 1, 0, &|_| true);
        assert!(events.iter().any(|e| matches!(e, PumpEvent::WorldCmds(_))), "back in after the re-join: {events:?}");
    }

    #[test]
    fn world_cmds_decode_and_relay() {
        use crate::sim::net::world_cmd_wire::encode_batch;
        let mut pump = pump_on_loopback();
        let a = Client::connect(pump.port());
        let b = Client::connect(pump.port());
        for c in [&a, &b] {
            c.send(CH_HANDSHAKE, &LanJoin::Join { name: "p".into() }.encode());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        pump.pump(0, "W", 1, 0, &|_| true);
        a.recv().unwrap();
        b.recv().unwrap();

        let batch = vec![(7u64, WorldCmd::Remove { pos: (10, 64, -4) })];
        let bytes = super::super::packet_compress::pack(&encode_batch(&batch), 48);
        a.send(CH_WORLDCMD, &bytes);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let events = pump.pump(100, "W", 1, 0, &|_| true);
        assert!(
            events.iter().any(|e| matches!(e, PumpEvent::WorldCmds(c) if c == &batch)),
            "world cmd not surfaced: {events:?}"
        );
        let (ch, _) = b.recv().expect("world cmd not relayed");
        assert_eq!(ch, CH_WORLDCMD);
    }

    #[test]
    fn join_sync_batches_reach_the_targeted_peer_only() {
        use crate::sim::net::join_sync::{decode_records, encode_records, ChunkEditRecord, StampedEdit};
        use crate::world::chunk::ChunkCoord;
        let mut pump = pump_on_loopback();
        let a = Client::connect(pump.port());
        let b = Client::connect(pump.port());
        for c in [&a, &b] {
            c.send(CH_HANDSHAKE, &LanJoin::Join { name: "p".into() }.encode());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        let joins = pump.pump(0, "W", 1, 0, &|_| true);
        let addr_a = match &joins[0] {
            PumpEvent::PeerJoined { addr, .. } => *addr,
            other => panic!("{other:?}"),
        };
        a.recv().unwrap(); // drain Accepts
        b.recv().unwrap();

        // The host's divergence from the seed, in the timebending layout.
        let records = vec![ChunkEditRecord {
            coord: ChunkCoord::new(2, 1, -3),
            edits: vec![StampedEdit { tick: 0, index: 1234, block: 7 }],
        }];
        let packed = super::super::packet_compress::pack(&encode_records(&records), 48);
        pump.send_to_peer(CH_JOINSYNC, &packed, addr_a);

        let (ch, payload) = a.recv().expect("targeted peer must receive the sync");
        assert_eq!(ch, CH_JOINSYNC);
        let raw = super::super::packet_compress::unpack(&payload, 256 * 1024).expect("unpack");
        assert_eq!(decode_records(&raw).as_deref(), Some(&records[..]), "layout survives the wire");
        assert!(b.recv().is_none(), "join-sync is targeted state — never fanned out");
    }

    #[test]
    fn silent_peers_are_pruned_with_a_leave_event() {
        let mut pump = pump_on_loopback();
        let a = Client::connect(pump.port());
        a.send(CH_HANDSHAKE, &LanJoin::Join { name: "a".into() }.encode());
        std::thread::sleep(std::time::Duration::from_millis(10));
        let joins = pump.pump(0, "W", 1, 0, &|_| true);
        let eid = match &joins[0] {
            PumpEvent::PeerJoined { entity_id, .. } => *entity_id,
            other => panic!("{other:?}"),
        };
        // No traffic past the ttl → pruned.
        let events = pump.pump(PEER_TIMEOUT_MS + 1, "W", 1, 0, &|_| true);
        assert!(events.contains(&PumpEvent::PeerLeft { entity_id: eid }), "{events:?}");
        assert_eq!(pump.host.live_peer_count(), 0);
    }
}
