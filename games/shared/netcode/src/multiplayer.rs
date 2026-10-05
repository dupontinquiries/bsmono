//! multiplayer — the control-plane half of real online play, shared by every fleet game: the host
//! announces a session over the fleet DHT (the `SessionExt` seam bsgames proves in
//! `host_then_join_over_the_fleet_dht`), a friend joins by resolving it, and each game's own UDP
//! socket is the data plane — this module never touches gameplay bytes, matching the split in
//! `games/GAMES_UNIFICATION.md` ("Unify multiplayer").
//!
//! Adopted from the first consumer, `games/soldier/src/multiplayer.rs`, generalized by exactly one
//! parameter: the **game id** (`"soldier"`, `"clawbound"`, `"unendlich"`, `"blackstar"` — the same
//! ids `bsgames::centre::registry` uses), which names the session token (`<game>@<host-id>`), the
//! announced [`SessionRecord::game`], and the join-side wrong-game rejection. The original tests
//! came along verbatim (with the game passed explicitly), so soldier's proven behavior is pinned.
//!
//! Same graceful floor as the rest of the fleet: `FileStore` for both primary and fallback (two
//! separate processes — a host and a bsgames-spawned joiner — must actually see each other's
//! announcements for local/LAN testing; the `opendht` feature remains the drop-in global-reach
//! upgrade).

use crate::profile::Profile;
use crate::session::{SessionExt, SessionRecord};
use crate::store::{FileStore, ProfileService};
use std::net::UdpSocket;

/// The address to ANNOUNCE for a socket bound at `local`.
///
/// `host("0.0.0.0:0")` is the natural way to accept from anywhere, and
/// `local_addr()` then says `0.0.0.0:PORT` -- which was announced verbatim.
/// A friend on another machine resolved that and dialled 0.0.0.0. It only
/// ever worked because every test, and the FileStore fallback, is one
/// machine. An unspecified address is replaced by this host's outbound one,
/// found the dependency-free way: connect a UDP socket toward a
/// documentation address (no packet is sent) and read which local address
/// the kernel picked. No route at all falls back to loopback.
pub fn routable(local: std::net::SocketAddr) -> std::net::SocketAddr {
    if !local.ip().is_unspecified() {
        return local;
    }
    let ip = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip())
        .ok()
        .filter(|ip| !ip.is_unspecified())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    std::net::SocketAddr::new(ip, local.port())
}

pub struct Multiplayer {
    svc: ProfileService<FileStore, FileStore>,
    /// The registry id of the game this instance plays (`"soldier"`, `"clawbound"`, …).
    pub game: String,
    pub me: Profile,
    pub socket: Option<UdpSocket>,
    pub token: Option<String>,
}

impl Multiplayer {
    pub fn new(game: &str, player_id: &str, display_name: &str) -> Self {
        Self::at_store(game, player_id, display_name, FileStore::dev_default_path())
    }

    /// Same as `new`, but pointed at an explicit shared-store file rather than the real
    /// machine-wide default — every test needs its own path so parallel test runs (and repeated
    /// runs leaving stale data behind) can't interfere with each other or with a real running game.
    pub fn at_store(game: &str, player_id: &str, display_name: &str, store_path: std::path::PathBuf) -> Self {
        let mut m = Multiplayer {
            svc: ProfileService::new(FileStore::at(&store_path), FileStore::at(&store_path)),
            game: game.to_string(),
            me: Profile::new(player_id, display_name),
            socket: None,
            token: None,
        };
        m.svc.publish(&m.me); // so a friend's lookup finds us at all, even before hosting
        m
    }

    /// The shared profile service, for callers that need direct control-plane access (soldier's
    /// friends list does).
    pub fn svc(&self) -> &ProfileService<FileStore, FileStore> {
        &self.svc
    }
    pub fn svc_mut(&mut self) -> &mut ProfileService<FileStore, FileStore> {
        &mut self.svc
    }

    /// "▶ Host": bind a real UDP socket (the data plane), announce the session on the fleet DHT
    /// (the control plane), flip presence to `InGame`. Returns the session token (also `self.token`).
    pub fn host(&mut self, bind_addr: &str) -> Result<String, String> {
        let socket = UdpSocket::bind(bind_addr).map_err(|e| format!("bind {bind_addr}: {e}"))?;
        socket.set_nonblocking(true).ok();
        let local = routable(socket.local_addr().map_err(|e| e.to_string())?).to_string();
        let token = format!("{}@{}", self.game, self.me.id);
        let rec = SessionRecord::new(&self.game, &self.me.id, "udp", &local);
        self.svc.announce_session(&mut self.me, &token, &rec);
        self.socket = Some(socket);
        self.token = Some(token.clone());
        Ok(token)
    }

    /// "▶ Join a friend": resolve their live session (control plane) into a dial target, then
    /// connect our own UDP socket to it (the data plane starts here — gameplay sync is out of
    /// scope of this module, same as every other game's transport in the unification doc).
    pub fn join(&mut self, friend_id: &str) -> Result<String, String> {
        let rec = self
            .svc
            .join_via_friend(friend_id)
            .ok_or_else(|| format!("{friend_id} is not hosting a joinable {} match", self.game))?;
        if rec.game != self.game {
            return Err(format!("{friend_id} is hosting {}, not {}", rec.game, self.game));
        }
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        socket.connect(&rec.addr).map_err(|e| format!("connect {}: {e}", rec.addr))?;
        socket.set_nonblocking(true).ok();
        self.socket = Some(socket);
        Ok(rec.addr)
    }

    /// "▶ Join by address": dial `addr` (`ip:port`) directly, with no record to resolve.
    /// This is how two MACHINES play today: the shared `FileStore` reaches only machines that
    /// share its path (`opendht` is the global-reach feature), so a friend's name resolves on
    /// the same box and nowhere else, while the UDP data plane itself works across any
    /// machines that can reach the host. The host's address is in its HOST line
    /// (`host_addr`).
    pub fn join_addr(&mut self, addr: &str) -> Result<String, String> {
        let target: std::net::SocketAddr = addr.trim().parse().map_err(|_| format!("{}: not an ip:port", addr.trim()))?;
        if target.port() == 0 {
            return Err(format!("{target}: port 0 is nobody"));
        }
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        socket.connect(target).map_err(|e| format!("connect {target}: {e}"))?;
        socket.set_nonblocking(true).ok();
        self.socket = Some(socket);
        Ok(target.to_string())
    }

    /// Does `text` name an address rather than a friend? (`ip:port` parses as one.)
    pub fn looks_like_addr(text: &str) -> bool {
        text.trim().parse::<std::net::SocketAddr>().is_ok()
    }

    /// The address a joiner on the LAN should type while hosting: the routable form of the
    /// hosting socket's (`routable` turns 0.0.0.0 into the outbound interface's address).
    pub fn host_addr(&self) -> Option<std::net::SocketAddr> {
        let sock = self.socket.as_ref()?;
        if self.token.is_none() {
            return None;
        }
        sock.local_addr().ok().map(routable)
    }

    /// End hosting: tear down the control-plane record (presence flips back to `Online`, so
    /// `join_via_friend` stops offering it — see `SessionExt::end_session`'s doc for the TTL
    /// caveat on the raw record itself).
    /// Keep a live session discoverable: re-publish the record and presence.
    ///
    /// Ported back from the copy vendored into skydribble and blackstar-rs,
    /// where it was written and where it stayed. Records and presence carry
    /// TTLs on the store, so **a host that announces once and then plays for
    /// ten minutes silently falls off the friend list** — the session is alive
    /// and undiscoverable, which looks to everyone else like the host left.
    ///
    /// Call roughly every 20 seconds while hosting. It is deliberately cheap
    /// and idempotent: re-announcing an already-live session is a store write
    /// that costs nothing, and a host that skips one tick is not penalised.
    ///
    /// Does nothing when not hosting, rather than erroring — a caller ticking
    /// this on a timer should not have to know whether hosting has ended.
    pub fn reannounce(&mut self) {
        if let (Some(tok), Some(sock)) = (self.token.clone(), self.socket.as_ref()) {
            if let Ok(local) = sock.local_addr() {
                let rec = SessionRecord::new(&self.game, &self.me.id, "udp",
                                             &local.to_string());
                self.svc.announce_session(&mut self.me, &tok, &rec);
            }
        }
    }

    pub fn stop_hosting(&mut self) {
        self.svc.end_session(&mut self.me);
        self.token = None;
        self.socket = None;
    }

    /// The smallest possible proof the control→data-plane handoff actually works end to end: send
    /// a "hello" datagram over the connected socket. Real gameplay sync is future work built on
    /// top of this same `self.socket`.
    pub fn send_hello(&self) -> Result<(), String> {
        let sock = self.socket.as_ref().ok_or("not connected")?;
        sock.send(b"hello").map(|_| ()).map_err(|e| e.to_string())
    }

    /// Non-blocking receive on the data-plane socket (host side listens with this too, since a
    /// bound-but-unconnected UDP socket can `recv_from` any peer that dials it).
    pub fn poll(&self, buf: &mut [u8]) -> Option<(usize, std::net::SocketAddr)> {
        self.socket.as_ref()?.recv_from(buf).ok()
    }

    /// Apply a startup [`crate::handoff::Handoff`]: host or join immediately, returning a human
    /// status line either way — the wiring every game's entry point shares, so per-game startup
    /// code stays a two-liner. `Ok(status)` means the game should jump past its menu into the
    /// match; `Err(status)` means show the menu with the failure surfaced.
    pub fn apply_handoff(&mut self, h: &crate::handoff::Handoff) -> Result<String, String> {
        match h {
            crate::handoff::Handoff::Host { .. } => {
                self.host("0.0.0.0:0").map(|token| format!("hosting: {token}"))
            }
            crate::handoff::Handoff::Join { peer_id, .. } => {
                self.join(peer_id).map(|addr| format!("connected to {peer_id} at {addr}"))
            }
        }
    }
}

/// Build the right Multiplayer for a game's startup: handoff identity when present (the launched
/// process presents as the SAME player bsgames already published), else the locally-saved name.
pub fn for_startup(game: &str, local_name: &str, handoff: Option<&crate::handoff::Handoff>) -> Multiplayer {
    match handoff {
        Some(crate::handoff::Handoff::Host { me_id, me_name })
        | Some(crate::handoff::Handoff::Join { me_id, me_name, .. }) => Multiplayer::new(game, me_id, me_name),
        None => Multiplayer::new(game, local_name, local_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("netcode-mp-test-{name}-{}.jsonl", std::process::id()))
    }

    #[test]
    fn host_announces_and_a_friend_can_resolve_the_dial_target() {
        let path = tmp_path("resolve");
        let mut host = Multiplayer::at_store("soldier", "kesslerd", "Kessler", path.clone());
        let token = host.host("127.0.0.1:0").expect("bind should succeed");
        assert_eq!(token, "soldier@kesslerd");
        assert_eq!(host.token.as_deref(), Some("soldier@kesslerd"));

        let friend = Multiplayer::at_store("soldier", "friendclient", "Friend", path.clone());
        let dial = friend.svc().join_via_friend("kesslerd").expect("host is joinable, over the shared file store");
        assert_eq!(dial.game, "soldier");
        assert_eq!(dial.transport, "udp");
        assert!(dial.addr.starts_with("127.0.0.1:"));
        let _ = std::fs::remove_file(&path);
    }

    /// Join by address: no store between the two, the joiner dials what the host's HOST line
    /// says, and a datagram crosses. A name is not an address; port 0 is nobody.
    #[test]
    fn join_by_address_dials_without_a_store_and_a_datagram_crosses() {
        let host_path = tmp_path("addr-host");
        let join_path = tmp_path("addr-join"); // a DIFFERENT store: nothing to resolve through
        let mut host = Multiplayer::at_store("skydribble", "ann", "Ann", host_path.clone());
        host.host("127.0.0.1:0").unwrap();
        let addr = host.host_addr().expect("hosting has an address");
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert!(addr.port() > 0);
        let mut joiner = Multiplayer::at_store("skydribble", "bo", "Bo", join_path.clone());
        assert!(joiner.join("ann").is_err(), "ann is on another store: the name does not resolve");
        let dialled = joiner.join_addr(&addr.to_string()).expect("the address dials");
        assert_eq!(dialled, addr.to_string());
        joiner.socket.as_ref().unwrap().send(b"hello").unwrap();
        let mut buf = [0u8; 16];
        let mut got = None;
        for _ in 0..200 {
            if let Some((n, from)) = host.poll(&mut buf) {
                got = Some((buf[..n].to_vec(), from));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (bytes, _from) = got.expect("the host received the joiner's datagram");
        assert_eq!(bytes, b"hello");
        assert!(Multiplayer::looks_like_addr(" 10.0.0.2:4000 "));
        assert!(!Multiplayer::looks_like_addr("ann"));
        assert!(!Multiplayer::looks_like_addr("10.0.0.2"), "no port is no address");
        assert!(joiner.join_addr("ann").unwrap_err().contains("not an ip:port"));
        assert!(joiner.join_addr("127.0.0.1:0").unwrap_err().contains("port 0"));
        assert!(joiner.host_addr().is_none(), "a joiner has no host address");
        let _ = std::fs::remove_file(&host_path);
        let _ = std::fs::remove_file(&join_path);
    }

    #[test]
    fn joining_a_session_of_a_different_game_is_rejected() {
        let path = tmp_path("wrong-game");
        let mut host = Multiplayer::at_store("clawbound", "amy", "Amy", path.clone());
        host.host("127.0.0.1:0").unwrap(); // amy hosts CLAWBOUND

        let mut joiner = Multiplayer::at_store("soldier", "bob", "Bob", path.clone());
        let err = joiner.join("amy").unwrap_err();
        assert!(err.contains("clawbound"), "error should name the actual game: {err}");
        let _ = std::fs::remove_file(&path);
    }

    /// Hosting on the wildcard must not ANNOUNCE the wildcard.
    #[test]
    fn a_wildcard_bind_announces_an_address_someone_can_dial() {
        let path = tmp_path("routable");
        let mut host = Multiplayer::at_store("clawbound", "wren", "Wren", path.clone());
        host.host("0.0.0.0:0").unwrap();
        let port = host.socket.as_ref().unwrap().local_addr().unwrap().port();
        let mut joiner = Multiplayer::at_store("clawbound", "fox", "Fox", path.clone());
        let addr: std::net::SocketAddr = joiner.join("wren").expect("wren is hosting").parse().expect("an address");
        assert!(!addr.ip().is_unspecified(), "announced {addr}: nobody can dial that");
        assert_eq!(addr.port(), port, "and it is the port that was actually bound");
        let pinned: std::net::SocketAddr = "10.1.2.3:4000".parse().unwrap();
        assert_eq!(routable(pinned), pinned, "an explicit bind is announced as given");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn joining_someone_not_hosting_fails_cleanly() {
        let path = tmp_path("not-hosting");
        let _host = Multiplayer::at_store("soldier", "zed", "Zed", path.clone()); // published, never hosts
        let mut joiner = Multiplayer::at_store("soldier", "bob2", "Bob", path.clone());
        assert!(joiner.join("zed").is_err(), "zed is online but not in a game — not joinable");
        assert!(joiner.join("nobody-with-this-id").is_err(), "unknown id — not joinable");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stop_hosting_clears_local_session_state() {
        let path = tmp_path("stop-hosting");
        let mut host = Multiplayer::at_store("unendlich", "carla", "Carla", path.clone());
        host.host("127.0.0.1:0").unwrap();
        assert!(host.token.is_some());
        host.stop_hosting();
        assert!(host.token.is_none());
        assert!(host.socket.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_real_udp_hello_reaches_the_host_socket_via_a_separate_joiner_instance() {
        let path = tmp_path("udp-hello");
        let mut host = Multiplayer::at_store("clawbound", "dana", "Dana", path.clone());
        host.host("127.0.0.1:0").unwrap();
        let host_addr = host.socket.as_ref().unwrap().local_addr().unwrap();

        let mut joiner = Multiplayer::at_store("clawbound", "erin", "Erin", path.clone());
        let addr = joiner.join("dana").expect("dana is hosting, resolved via the shared FileStore");
        assert_eq!(addr, host_addr.to_string());
        joiner.send_hello().expect("joiner's connected socket can send");

        let mut buf = [0u8; 16];
        let mut got = None;
        for _ in 0..200 {
            if let Some((n, _from)) = host.poll(&mut buf) {
                got = Some(n);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let n = got.expect("host should receive the joiner's hello datagram");
        assert_eq!(&buf[..n], b"hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn for_startup_uses_the_handoff_identity_and_apply_handoff_hosts_or_joins() {
        let path = tmp_path("startup");
        // handoff identity overrides the local name for this launch
        let h = crate::handoff::Handoff::Host { me_id: "me@0".into(), me_name: "Kessler".into() };
        let mut mp = match &h {
            crate::handoff::Handoff::Host { me_id, me_name } | crate::handoff::Handoff::Join { me_id, me_name, .. } => {
                Multiplayer::at_store("clawbound", me_id, me_name, path.clone())
            }
        };
        assert_eq!(mp.me.id, "me@0");
        let status = mp.apply_handoff(&h).expect("host handoff should bind + announce");
        assert_eq!(status, "hosting: clawbound@me@0");

        // and the join side resolves it via the same store
        let hj = crate::handoff::Handoff::Join { me_id: "f@1".into(), me_name: "F".into(), peer_id: "me@0".into() };
        let mut joiner = Multiplayer::at_store("clawbound", "f@1", "F", path.clone());
        let status = joiner.apply_handoff(&hj).expect("join handoff should resolve + dial");
        assert!(status.starts_with("connected to me@0 at "), "{status}");
        // no-handoff startup keeps the local name
        let plain = for_startup("clawbound", "LocalCat", None);
        assert_eq!(plain.me.id, "LocalCat");
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod reannounce_tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("netcode-re-{name}-{}.jsonl", std::process::id()))
    }

    #[test]
    fn reannouncing_keeps_a_host_resolvable() {
        // The bug it exists for: records carry a TTL, so a host that announces
        // once and plays for ten minutes falls off the friend list while still
        // hosting. Alive and undiscoverable looks exactly like "they left".
        let path = tmp_path("keeps");
        let mut host = Multiplayer::at_store("soldier", "kesslerd", "Kessler", path.clone());
        host.host("127.0.0.1:0").expect("bind");
        host.reannounce();

        let friend = Multiplayer::at_store("soldier", "f", "F", path);
        assert!(friend.svc().join_via_friend("kesslerd").is_some(),
                "still resolvable after a re-announce");
    }

    #[test]
    fn reannouncing_while_not_hosting_does_nothing_rather_than_failing() {
        // A caller ticking this on a timer should not have to know whether
        // hosting has ended — and a panic or an error there would make the
        // timer the thing that has to carry the state machine.
        let path = tmp_path("idle");
        let mut idle = Multiplayer::at_store("soldier", "nobody", "Nobody", path.clone());
        idle.reannounce();
        let friend = Multiplayer::at_store("soldier", "f", "F", path);
        assert!(friend.svc().join_via_friend("nobody").is_none(),
                "and it did not announce a session that was never started");
    }

    #[test]
    fn reannouncing_after_stopping_does_not_resurrect_the_session() {
        // The ordering that matters: stop_hosting clears the token, so a timer
        // firing one last time after the host quit must not put the session
        // back on the store for others to dial into.
        let path = tmp_path("stopped");
        let mut host = Multiplayer::at_store("soldier", "quitter", "Q", path.clone());
        host.host("127.0.0.1:0").expect("bind");
        host.stop_hosting();
        host.reannounce();

        let friend = Multiplayer::at_store("soldier", "f", "F", path);
        assert!(friend.svc().join_via_friend("quitter").is_none());
    }
}
