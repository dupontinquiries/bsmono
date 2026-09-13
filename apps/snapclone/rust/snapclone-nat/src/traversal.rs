//! High-level NAT traversal coordinator — the Rust `NatTraversal`.
//!
//! A faithful port of `NatTraversal.cpp`'s state machine, minus Qt. The
//! shape that matters, and that the previous scaffold here got wrong:
//!
//! # ONE socket
//!
//! `discover()` and `punch()` used to each call `UdpSocket::bind()` on the
//! same configured port — two sockets. The C++ binds `m_punchSocket` exactly
//! once and does STUN, hole-punching, and the SCUD data channel all through
//! it, because that is the entire mechanism: the reflexive address STUN
//! learns is the NAT mapping for *that socket's* local port, and a punch
//! has to leave from that same port or it isn't punching the mapped hole.
//! (`ShareAddress` in the C++ is so TCP can also bind 9876 — not so a
//! second UDP socket can.) A single [`NatTraversal`] now owns one
//! [`UdpSocket`] for its whole life, and one receive loop demuxes what
//! arrives on it via [`crate::wire::classify`].
//!
//! # Tested constants, not guessed ones
//!
//! Defaults come from `core/Timings.hpp`: 60 × 150 ms punch burst (9 s),
//! then maintenance punches every 20 s (AT&T CGNAT expires at ~30 s; 20 s
//! leaves headroom), 4 s per-server STUN timeout, 25 s CGNAT keepalive to
//! the STUN server. The scaffold's 8 × 250 ms / 10 s were not from the
//! C++ and would have converged far less often against a real NAT.
//!
//! # Optimistic ready, confirmed later
//!
//! As in the C++, `HolePunchReady` is emitted when the burst *completes*,
//! whether or not the peer's punch has arrived — most NATs open the
//! pinhole on the first outbound packet, and waiting for a reply would
//! stall on one-sided arrival timing. When the peer's punch does arrive,
//! `PunchSeen` reports the *actual* source address (the NAT may have
//! remapped the port) and the attempt's target is updated to it. Ready is
//! emitted at most once per peer per session (`ready_emitted`): without
//! that guard, maintenance punches re-fired it on every packet and the
//! UDP channel called connect hundreds of times.
//!
//! # What is not here
//!
//! The `AppContext`-driven burst halving on metered/idle paths (a policy
//! hook), TURN relay URL lookup, and the SCUD channel itself. SCUD frames
//! are surfaced as [`NatEvent::ScudFrame`] for a channel layer to consume.

use crate::stun::{self, ReflexiveAddr, TransactionId};
use crate::wire::{self, Inbound};
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

/// Configuration. Defaults are the values in `core/Timings.hpp` and
/// `NatTraversal.cpp`, see the module doc.
#[derive(Debug, Clone)]
pub struct NatConfig {
    /// Local ports to try binding, in order; `0` means let the OS pick.
    /// The C++ tries 9876, then 9877, then 0.
    pub bind_ports: Vec<u16>,
    /// STUN servers, `host:port`, tried in order. Hostnames are resolved;
    /// after three consecutive DNS failures remaining hostnames are skipped
    /// straight to any IP-literal entries (saves ~4 s per dead lookup).
    pub stun_servers: Vec<String>,
    /// Per-server UDP STUN timeout (`kStunTimeoutMs`).
    pub stun_timeout: Duration,
    /// STUN-over-TCP fallback for networks that drop UDP entirely. Yields an
    /// IP with port 0 (a TCP handshake can't tell us our UDP punch port).
    /// The C++ uses Jami's server by IP so no DNS is needed.
    pub tcp_stun: Option<SocketAddr>,
    /// How long after starting UDP STUN to also start TCP STUN in parallel,
    /// if UDP hasn't succeeded yet.
    pub tcp_stun_parallel_after: Duration,
    /// Packets in the initial punch burst (`kPunchBurstCount`).
    pub punch_burst_count: u32,
    /// Interval between burst packets (`kPunchBurstIntervalMs`).
    pub punch_burst_interval: Duration,
    /// Interval between maintenance punches after the burst.
    pub punch_maintenance_interval: Duration,
    /// Zero-byte keepalive to the STUN server (`kCgnatKeepaliveMs`).
    pub cgnat_keepalive_interval: Duration,
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            bind_ports: vec![9876, 9877, 0],
            stun_servers: vec![
                "stun.l.google.com:19302".to_owned(),
                "stun1.l.google.com:19302".to_owned(),
                "stun.cloudflare.com:3478".to_owned(),
            ],
            stun_timeout: Duration::from_millis(4_000),
            tcp_stun: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(158, 69, 23, 138)), 3478)),
            tcp_stun_parallel_after: Duration::from_secs(5),
            punch_burst_count: 60,
            punch_burst_interval: Duration::from_millis(150),
            punch_maintenance_interval: Duration::from_secs(20),
            cgnat_keepalive_interval: Duration::from_millis(25_000),
        }
    }
}

/// A peer's address as the DHT / connect request hands it to us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAddress {
    pub ip: IpAddr,
    pub port: u16,
}

impl From<SocketAddr> for PeerAddress {
    fn from(a: SocketAddr) -> Self {
        PeerAddress { ip: a.ip(), port: a.port() }
    }
}

impl PeerAddress {
    fn socket(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }
}

/// Events emitted by `NatTraversal` — the Qt signals, as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NatEvent {
    /// STUN succeeded. `port == 0` means it came from TCP STUN: the IP is
    /// valid for DHT announcement but there is no UDP punch port.
    ReflexiveAddress { ip: IpAddr, port: u16 },
    /// The reflexive address is in CGNAT/private space — a relay is required.
    CgnatDetected { ip: IpAddr },
    /// Every STUN path was exhausted.
    StunFailed { reason: String },
    /// A punch from `peer_id` arrived: the pinhole is confirmed and `addr`
    /// is where their packets actually come from (the NAT may have remapped).
    PunchSeen { peer_id: String, addr: PeerAddress },
    /// Emitted once per peer per session: either the burst completed
    /// (optimistic) or their punch arrived, whichever is first.
    HolePunchReady { peer_id: String, addr: PeerAddress },
    /// The punch was refused before sending — see [`wire::SelfGuard`] — or
    /// the address was unusable.
    HolePunchFailed { peer_id: String },
    /// A SCUD data-channel frame arrived on the shared socket.
    ScudFrame { data: Vec<u8>, from: SocketAddr },
}

/// The one active punch attempt. Like the C++'s `m_punchAttempt`: one at a
/// time, replaced (and its sender task stopped) when a punch to a different
/// peer or a new address for the same peer starts.
#[derive(Debug)]
struct PunchAttempt {
    peer_id: String,
    addr: PeerAddress,
    /// Dropping the sender's `watch::Sender` (by replacing the attempt)
    /// ends its burst/maintenance loop; setting it true does the same
    /// without waiting for the drop.
    stop: watch::Sender<bool>,
}

#[derive(Default)]
struct State {
    stun_done: bool,
    reflexive: Option<ReflexiveAddr>,
    /// Recent STUN transaction ids, newest first. A response can arrive after
    /// `restart_stun_only()` has already issued a new id, so the C++ accepts
    /// any recent one — as does the receive loop here.
    recent_tx_ids: VecDeque<TransactionId>,
    /// Where CGNAT keepalives go once STUN has succeeded: the STUN server.
    keepalive_target: Option<SocketAddr>,
    punch: Option<PunchAttempt>,
    ready_emitted: HashSet<String>,
    /// `"ip:port"` → peer sc-ID, for resolving a punch that arrived with no
    /// id in its payload.
    punched_peers: HashMap<String, String>,
}

/// NAT traversal over one UDP socket. `Clone` is cheap and shares the
/// socket and state — the receive loop, punch tasks, and the caller all
/// hold one.
#[derive(Clone)]
pub struct NatTraversal {
    config: Arc<NatConfig>,
    self_id: Arc<str>,
    socket: Arc<UdpSocket>,
    events: mpsc::Sender<NatEvent>,
    /// STUN responses matched by the receive loop are forwarded here for a
    /// waiting `discover()`.
    stun_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<(ReflexiveAddr, SocketAddr)>>>,
    stun_tx: mpsc::Sender<(ReflexiveAddr, SocketAddr)>,
    state: Arc<Mutex<State>>,
}

impl NatTraversal {
    /// Bind the punch socket (9876 → 9877 → OS pick, with `SO_REUSEADDR`
    /// so TCP can share the port as in the C++) and start the receive loop.
    /// This is `start()`'s socket half; STUN itself is [`discover`].
    pub async fn bind(config: NatConfig, self_id: &str) -> Result<(Self, mpsc::Receiver<NatEvent>)> {
        let socket = bind_reusable(&config.bind_ports).await?;
        info!("punch socket bound to local port {}", socket.local_addr()?.port());
        let (events, events_rx) = mpsc::channel(64);
        let (stun_tx, stun_rx) = mpsc::channel(4);
        let nat = NatTraversal {
            config: Arc::new(config),
            self_id: Arc::from(self_id),
            socket: Arc::new(socket),
            events,
            stun_rx: Arc::new(tokio::sync::Mutex::new(stun_rx)),
            stun_tx,
            state: Arc::new(Mutex::new(State::default())),
        };
        tokio::spawn(nat.clone().receive_loop());
        Ok((nat, events_rx))
    }

    pub fn local_port(&self) -> u16 {
        self.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// The reflexive address if STUN has found one.
    pub fn reflexive(&self) -> Option<ReflexiveAddr> {
        self.state.lock().unwrap().reflexive
    }

    /// `start()`: STUN from the punch socket. If STUN already succeeded this
    /// session, re-emits the cached address and returns it. Runs TCP STUN in
    /// parallel after `tcp_stun_parallel_after` if UDP is still pending, and
    /// as a last resort once every UDP server is exhausted.
    pub async fn discover(&self) -> Result<ReflexiveAddr> {
        if let Some(r) = self.reflexive() {
            if self.state.lock().unwrap().stun_done {
                let _ = self.events.send(NatEvent::ReflexiveAddress { ip: r.ip, port: r.port }).await;
                return Ok(r);
            }
        }
        // Parallel TCP STUN: after a delay, if UDP hasn't landed, try TCP too
        // — some networks (phone hotspots, corporate WiFi) drop UDP entirely
        // and waiting for every UDP server to time out is much slower than
        // learning at least our IP over TCP.
        let parallel = {
            let me = self.clone();
            tokio::spawn(async move {
                sleep(me.config.tcp_stun_parallel_after).await;
                if me.state.lock().unwrap().stun_done {
                    return;
                }
                if let Some(addr) = me.config.tcp_stun {
                    if let Ok(r) = me.tcp_stun(addr).await {
                        me.accept_reflexive(r, None).await;
                    }
                }
            })
        };

        let result = self.udp_stun_all_servers().await;
        parallel.abort();
        match result {
            Ok(r) => Ok(r),
            Err(udp_err) => {
                // UDP exhausted: TCP as the last resort (C++ tryNextStunServer).
                if let Some(addr) = self.config.tcp_stun {
                    info!("UDP STUN exhausted — trying TCP STUN fallback");
                    if let Ok(r) = self.tcp_stun(addr).await {
                        self.accept_reflexive(r, None).await;
                        return Ok(r);
                    }
                }
                if let Some(r) = self.reflexive() {
                    // A parallel TCP result may have landed meanwhile.
                    return Ok(r);
                }
                let _ = self.events.send(NatEvent::StunFailed { reason: udp_err.to_string() }).await;
                Err(udp_err)
            }
        }
    }

    /// `restartStunOnly()`: redo STUN WITHOUT rebinding the socket, so any
    /// live CGNAT mapping and in-progress punch survive. Only STUN state is
    /// reset; the old reflexive address is kept until a new one arrives.
    pub async fn restart_stun_only(&self) -> Result<ReflexiveAddr> {
        {
            let mut st = self.state.lock().unwrap();
            st.stun_done = false;
            // Do NOT clear reflexive, punch, ready_emitted, or the keepalive
            // target — that is the whole point of this over a full restart.
        }
        info!("STUN-only restart — socket preserved, punch state intact");
        self.discover().await
    }

    async fn udp_stun_all_servers(&self) -> Result<ReflexiveAddr> {
        let mut consecutive_dns_failures = 0u32;
        let mut i = 0usize;
        let servers = &self.config.stun_servers;
        while i < servers.len() {
            let server = &servers[i];
            let is_literal = server.parse::<SocketAddr>().is_ok();
            let addr = match resolve_v4(server).await {
                Some(a) => a,
                None => {
                    warn!("STUN DNS failed for {server}");
                    consecutive_dns_failures += 1;
                    i += 1;
                    if consecutive_dns_failures >= 3 {
                        // Skip the remaining hostnames straight to IP literals.
                        while i < servers.len() && servers[i].parse::<SocketAddr>().is_err() {
                            i += 1;
                        }
                    }
                    continue;
                }
            };
            if !is_literal {
                consecutive_dns_failures = 0;
            }

            let tx = TransactionId::random();
            {
                let mut st = self.state.lock().unwrap();
                st.recent_tx_ids.push_front(tx.clone());
                st.recent_tx_ids.truncate(8);
            }
            info!("STUN: trying {server} from local port {}", self.local_port());
            self.socket.send_to(&stun::build_request(&tx), addr).await?;

            let mut rx = self.stun_rx.lock().await;
            match timeout(self.config.stun_timeout, rx.recv()).await {
                Ok(Some((r, from))) => {
                    drop(rx);
                    self.accept_reflexive(r, Some(from)).await;
                    return Ok(r);
                }
                Ok(None) => return Err(anyhow!("receive loop ended")),
                Err(_) => {
                    warn!("STUN timeout from {server}");
                    i += 1;
                }
            }
        }
        Err(anyhow!("all STUN servers exhausted"))
    }

    /// Record a reflexive result and tell the caller. `stun_server` is the
    /// UDP server that answered, which becomes the CGNAT keepalive target;
    /// a TCP result has no such target and no punch port.
    async fn accept_reflexive(&self, r: ReflexiveAddr, stun_server: Option<SocketAddr>) {
        let start_keepalive = {
            let mut st = self.state.lock().unwrap();
            if st.stun_done && stun_server.is_none() {
                return; // UDP already won; a late TCP result must not downgrade it
            }
            st.stun_done = true;
            st.reflexive = Some(r);
            let fresh = st.keepalive_target.is_none() && stun_server.is_some();
            if let Some(s) = stun_server {
                st.keepalive_target = Some(s);
            }
            fresh
        };
        let cls = crate::cgnat::AddrClass::of(r.ip);
        info!("STUN reflexive: {}:{} ({:?})", r.ip, r.port, cls);
        let _ = self.events.send(NatEvent::ReflexiveAddress { ip: r.ip, port: r.port }).await;
        if cls.needs_relay() {
            let _ = self.events.send(NatEvent::CgnatDetected { ip: r.ip }).await;
        }
        if start_keepalive {
            tokio::spawn(self.clone().keepalive_loop());
        }
    }

    /// Zero-byte keepalive to the STUN server so the NAT mapping for our
    /// punch port stays open (`kCgnatKeepaliveMs`).
    async fn keepalive_loop(self) {
        loop {
            sleep(self.config.cgnat_keepalive_interval).await;
            let Some(target) = self.state.lock().unwrap().keepalive_target else { return };
            match self.socket.send_to(&[0u8], target).await {
                Ok(_) => debug!("CGNAT keepalive sent"),
                Err(e) => warn!("CGNAT keepalive failed: {e}"),
            }
        }
    }

    /// STUN over TCP. Returns the IP with port 0.
    async fn tcp_stun(&self, server: SocketAddr) -> Result<ReflexiveAddr> {
        let tx = TransactionId::random();
        let mut stream = timeout(Duration::from_secs(5), TcpStream::connect(server)).await??;
        stream.write_all(&stun::build_request(&tx)).await?;
        let mut buf = vec![0u8; 512];
        let mut got = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let n = timeout(deadline - tokio::time::Instant::now(), stream.read(&mut buf[got..])).await??;
            if n == 0 {
                return Err(anyhow!("TCP STUN: connection closed"));
            }
            got += n;
            if got >= 20 {
                let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
                if got >= 20 + len {
                    let r = stun::parse_response(&buf[..20 + len], &tx)?;
                    info!("TCP STUN reflexive: {} (port unknown over TCP)", r.ip);
                    return Ok(ReflexiveAddr { ip: r.ip, port: 0 });
                }
            }
            if got == buf.len() {
                return Err(anyhow!("TCP STUN: response too large"));
            }
        }
    }

    /// `initiateHolePunch()`: start (or retarget) the punch to `peer_id` at
    /// `addr`. Refuses self-targets at the source — see [`wire::self_guard`].
    pub async fn initiate_hole_punch(&self, peer_id: &str, addr: PeerAddress) {
        if addr.port == 0 {
            warn!("hole punch: invalid peer address for {peer_id}");
            let _ = self.events.send(NatEvent::HolePunchFailed { peer_id: peer_id.to_owned() }).await;
            return;
        }
        let self_reflexive = self.reflexive().map(|r| r.ip);
        if let Some(why) = wire::self_guard(addr.ip, peer_id, &self.self_id, self_reflexive) {
            warn!("refusing to punch self: {}:{} id={peer_id} ({why:?})", addr.ip, addr.port);
            let _ = self.events.send(NatEvent::HolePunchFailed { peer_id: peer_id.to_owned() }).await;
            return;
        }

        let (stop_tx, stop_rx) = watch::channel(false);
        {
            let mut st = self.state.lock().unwrap();
            if let Some(cur) = &st.punch {
                if cur.peer_id == peer_id {
                    if cur.addr == addr {
                        debug!("hole punch already in progress for {peer_id}");
                        return;
                    }
                    info!("hole punch: switching target for {peer_id} to {}:{}", addr.ip, addr.port);
                }
                let _ = cur.stop.send(true);
            }
            // A new active punch may re-emit ready for this peer.
            st.ready_emitted.remove(peer_id);
            st.punch = Some(PunchAttempt { peer_id: peer_id.to_owned(), addr: addr.clone(), stop: stop_tx });
        }
        info!("UDP hole punch → {peer_id} @ {}:{} from local port {}", addr.ip, addr.port, self.local_port());
        tokio::spawn(self.clone().punch_loop(peer_id.to_owned(), stop_rx));
    }

    /// `stopHolePunch()`: the UDP channel is confirmed; stop maintenance.
    pub fn stop_hole_punch(&self, peer_id: &str) {
        let mut st = self.state.lock().unwrap();
        if st.punch.as_ref().is_some_and(|p| p.peer_id == peer_id) {
            if let Some(p) = st.punch.take() {
                let _ = p.stop.send(true);
            }
            info!("maintenance punch stopped for {peer_id} — UDP channel confirmed");
        }
    }

    /// The burst, then optimistic ready, then maintenance — one task per
    /// attempt, ended by its `stop` watch.
    async fn punch_loop(self, peer_id: String, mut stop: watch::Receiver<bool>) {
        let payload = wire::punch_payload(&self.self_id);
        let mut sent = 0u32;
        loop {
            if *stop.borrow() {
                return;
            }
            // Always punch the attempt's CURRENT address: PunchSeen may have
            // retargeted it to the peer's actual CGNAT source.
            let target = {
                let st = self.state.lock().unwrap();
                match &st.punch {
                    Some(p) if p.peer_id == peer_id => p.addr.socket(),
                    _ => return, // replaced by a newer attempt
                }
            };
            if let Err(e) = self.socket.send_to(&payload, target).await {
                warn!("punch send error: {e}");
            }
            sent += 1;
            debug!("punch {sent}/{} → {target}", self.config.punch_burst_count);

            let wait = if sent >= self.config.punch_burst_count {
                if sent == self.config.punch_burst_count {
                    let emit = self.state.lock().unwrap().ready_emitted.insert(peer_id.clone());
                    if emit {
                        info!("hole punch volleys complete for {peer_id} — signalling ready, maintenance every {:?}", self.config.punch_maintenance_interval);
                        let _ = self.events.send(NatEvent::HolePunchReady { peer_id: peer_id.clone(), addr: PeerAddress::from(target) }).await;
                    }
                }
                self.config.punch_maintenance_interval
            } else {
                self.config.punch_burst_interval
            };
            tokio::select! {
                _ = sleep(wait) => {}
                _ = stop.changed() => { if *stop.borrow() { return; } }
            }
        }
    }

    /// The single receive loop. Everything that arrives on the punch socket
    /// comes through here and is told apart by content.
    async fn receive_loop(self) {
        let mut buf = vec![0u8; 65535];
        loop {
            let (n, from) = match self.socket.recv_from(&mut buf).await {
                Ok(x) => x,
                Err(e) => {
                    warn!("punch socket recv error: {e}");
                    continue;
                }
            };
            let data = &buf[..n];
            debug!("punch socket rx {n} bytes from {from}");
            match wire::classify(data) {
                Inbound::StunResponse { tx_id } => {
                    let candidates: Vec<TransactionId> = self.state.lock().unwrap().recent_tx_ids.iter().cloned().collect();
                    let Some(tx) = candidates.into_iter().find(|t| t.0 == tx_id) else {
                        debug!("STUN response with unknown transaction id — ignored");
                        continue;
                    };
                    match stun::parse_response(data, &tx) {
                        Ok(r) => { let _ = self.stun_tx.send((r, from)).await; }
                        Err(e) => warn!("STUN parse error from {from}: {e}"),
                    }
                }
                Inbound::Scud(frame) => {
                    if wire::is_loopback(from.ip()) {
                        // Our own frame hairpinned back; forwarding it would
                        // make the channel answer itself, forever.
                        debug!("dropped self-loopback SCUD frame from {from}");
                        continue;
                    }
                    let _ = self.events.send(NatEvent::ScudFrame { data: frame.to_vec(), from }).await;
                }
                Inbound::Punch { sender_id } => self.on_punch(sender_id, from).await,
                Inbound::Other => {}
            }
        }
    }

    async fn on_punch(&self, sender_id: &str, from: SocketAddr) {
        if let Some(why) = wire::self_guard(from.ip(), sender_id, &self.self_id, None) {
            debug!("dropped self-originated punch from {from} ({why:?})");
            return;
        }
        debug!("received punch from {from} id={sender_id:?} — pinhole open, replying");
        // Reply with our own id so the sender can confirm us.
        let _ = self.socket.send_to(&wire::punch_payload(&self.self_id), from).await;

        let confirmed = PeerAddress::from(from);
        // Resolve an id-less punch by its source, as the C++ does.
        let sender_id = if sender_id.is_empty() {
            let st = self.state.lock().unwrap();
            let key = format!("{}:{}", from.ip(), from.port());
            st.punched_peers.get(&key).cloned().or_else(|| {
                let prefix = format!("{}:", from.ip());
                st.punched_peers.iter().find(|(k, _)| k.starts_with(&prefix)).map(|(_, v)| v.clone())
            })
        } else {
            Some(sender_id.to_owned())
        };
        let Some(peer_id) = sender_id else {
            info!("late punch from unknown sender {from} — ignoring");
            return;
        };

        let emit = {
            let mut st = self.state.lock().unwrap();
            st.punched_peers.insert(format!("{}:{}", from.ip(), from.port()), peer_id.clone());
            // Retarget an active attempt for this peer to their actual source
            // — the NAT may have remapped the port they announced.
            if let Some(p) = st.punch.as_mut() {
                if p.peer_id == peer_id && p.addr != confirmed {
                    info!("updating punch target to actual source {from}");
                    p.addr = confirmed.clone();
                }
            }
            st.ready_emitted.insert(peer_id.clone())
        };
        let _ = self.events.send(NatEvent::PunchSeen { peer_id: peer_id.clone(), addr: confirmed.clone() }).await;
        if emit {
            info!("punch from {peer_id} — confirming pinhole {from}");
            let _ = self.events.send(NatEvent::HolePunchReady { peer_id, addr: confirmed }).await;
        } else {
            debug!("passive punch from {peer_id} (already ready) — pinhole maintained");
        }
    }

    /// Send raw bytes to a punched peer over the shared socket (the C++'s
    /// `sendUdpData`, used by the SCUD channel).
    pub async fn send_udp(&self, to: SocketAddr, data: &[u8]) -> Result<()> {
        self.socket.send_to(data, to).await?;
        Ok(())
    }
}

/// Bind with `SO_REUSEADDR` (the C++'s `ShareAddress | ReuseAddressHint`, so
/// TCP can share the port for the post-punch connect), trying each port in
/// order and failing over on `AddrInUse`, as `start()` does 9876 → 9877 → 0.
async fn bind_reusable(ports: &[u16]) -> Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let mut last: Option<std::io::Error> = None;
    for &port in ports {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_reuse_address(true)?;
        sock.set_nonblocking(true)?;
        let addr: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        if let Err(e) = sock.bind(&addr.into()) {
            warn!("punch port {port} unavailable: {e}");
            last = Some(e);
            continue;
        }
        return Ok(UdpSocket::from_std(sock.into())?);
    }
    Err(anyhow!("could not bind any punch port: {last:?}"))
}

async fn resolve_v4(server: &str) -> Option<SocketAddr> {
    if let Ok(a) = server.parse::<SocketAddr>() {
        return Some(a);
    }
    tokio::net::lookup_host(server).await.ok()?.find(|a| a.is_ipv4())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two NatTraversals on loopback punching each other. No STUN, no NAT —
    /// this exercises the shared-socket demux, the punch reply, PunchSeen
    /// with the real source, and the once-per-peer ready guard. It also
    /// shows why the loopback self-guards have to key on *IP*, not port:
    /// both ends here are loopback, so the guard is bypassed by giving the
    /// two peers distinct ids and checking by id — which is exactly the
    /// C++'s reasoning for carrying the sc-ID in the payload.
    #[tokio::test]
    async fn two_local_peers_confirm_each_others_pinhole_once() {
        let cfg = NatConfig {
            bind_ports: vec![0],
            punch_burst_count: 3,
            punch_burst_interval: Duration::from_millis(10),
            punch_maintenance_interval: Duration::from_secs(60),
            ..NatConfig::default()
        };
        let (a, mut a_ev) = NatTraversal::bind(cfg.clone(), "sc-alice").await.unwrap();
        let (b, mut b_ev) = NatTraversal::bind(cfg, "sc-bob").await.unwrap();

        // Loopback is refused by the self-guard, so these two can only talk
        // if we address them by a non-loopback local IP. Use the host's
        // primary v4 via a UDP-connect trick. Failing loudly rather than
        // returning early: a silently-skipped exchange would report this
        // test green without having exercised anything.
        let local_ip = local_non_loopback_v4().expect("this test needs a non-loopback IPv4 interface");
        let started = std::time::Instant::now();
        let a_addr = PeerAddress { ip: local_ip, port: a.local_port() };
        let b_addr = PeerAddress { ip: local_ip, port: b.local_port() };

        a.initiate_hole_punch("sc-bob", b_addr.clone()).await;
        b.initiate_hole_punch("sc-alice", a_addr.clone()).await;

        let mut a_ready = 0;
        let mut a_seen = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline && (a_ready == 0 || !a_seen) {
            tokio::select! {
                Some(ev) = a_ev.recv() => match ev {
                    NatEvent::HolePunchReady { peer_id, .. } => { assert_eq!(peer_id, "sc-bob"); a_ready += 1; }
                    NatEvent::PunchSeen { peer_id, addr } => { assert_eq!(peer_id, "sc-bob"); assert_eq!(addr.port, b.local_port()); a_seen = true; }
                    NatEvent::HolePunchFailed { .. } => panic!("punch refused"),
                    _ => {}
                },
                Some(_) = b_ev.recv() => {}
                _ = sleep(Duration::from_millis(50)) => {}
            }
        }
        assert!(a_seen, "alice never saw bob's punch");
        assert_eq!(a_ready, 1, "ready must fire exactly once per peer");
        // Both peers punch at once, so bob's first packet confirms the pinhole
        // within microseconds and ready fires from the receive path — before
        // alice's own burst completes. The guard that matters is that the
        // burst completing LATER does not fire it again: keep draining well
        // past the 3 × 10 ms burst and count.
        let _ = started;
        let quiet_until = tokio::time::Instant::now() + Duration::from_millis(120);
        while tokio::time::Instant::now() < quiet_until {
            tokio::select! {
                Some(ev) = a_ev.recv() => if let NatEvent::HolePunchReady { .. } = ev { a_ready += 1; },
                Some(_) = b_ev.recv() => {}
                _ = sleep(Duration::from_millis(10)) => {}
            }
        }
        assert_eq!(a_ready, 1, "burst completion re-fired ready after the peer's punch already had");
    }

    #[tokio::test]
    async fn punching_yourself_is_refused_at_the_source() {
        let cfg = NatConfig { bind_ports: vec![0], ..NatConfig::default() };
        let (a, mut ev) = NatTraversal::bind(cfg, "sc-alice").await.unwrap();
        a.initiate_hole_punch("sc-bob", PeerAddress { ip: IpAddr::V4(Ipv4Addr::LOCALHOST), port: 9876 }).await;
        assert_eq!(ev.recv().await, Some(NatEvent::HolePunchFailed { peer_id: "sc-bob".into() }));
        a.initiate_hole_punch("sc-alice", PeerAddress { ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), port: 9876 }).await;
        assert_eq!(ev.recv().await, Some(NatEvent::HolePunchFailed { peer_id: "sc-alice".into() }));
    }

    #[test]
    fn defaults_are_the_tested_cpp_constants() {
        let c = NatConfig::default();
        assert_eq!(c.punch_burst_count, 60);
        assert_eq!(c.punch_burst_interval, Duration::from_millis(150));
        assert_eq!(c.punch_maintenance_interval, Duration::from_secs(20));
        assert_eq!(c.stun_timeout, Duration::from_millis(4_000));
        assert_eq!(c.cgnat_keepalive_interval, Duration::from_millis(25_000));
        assert_eq!(c.bind_ports, vec![9876, 9877, 0]);
    }

    fn local_non_loopback_v4() -> Option<IpAddr> {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect("10.255.255.255:9").ok()?;
        let ip = s.local_addr().ok()?.ip();
        (!wire::is_loopback(ip) && ip.is_ipv4()).then_some(ip)
    }
}
