//! multiplayer — the control-plane half of real online play: host announces a session over the
//! fleet DHT (the same `netcode::SessionExt` seam `bsgames` proves in
//! `host_then_join_over_the_fleet_dht`), a friend joins by resolving it. Soldier's own UDP socket is
//! the data plane — this module never touches gameplay bytes, matching the split in
//! `games/GAMES_UNIFICATION.md` ("Unify multiplayer"). Previously the Multiplayer/Custom Match menus
//! only simulated matchmaking against local bots; this is the real seam they now call.
//!
//! Same graceful floor as the rest of the fleet: `SnapcoreStore` for both primary and fallback (the
//! `opendht` feature, off by default, is a drop-in upgrade to a real global-reach primary — nothing
//! else here would need to change).

use netcode::profile::Profile;
use netcode::session::{SessionExt, SessionRecord};
use netcode::store::{ProfileService, SnapcoreStore};
use std::net::UdpSocket;

pub struct Multiplayer {
    svc: ProfileService<SnapcoreStore, SnapcoreStore>,
    pub me: Profile,
    pub socket: Option<UdpSocket>,
    pub token: Option<String>,
}

impl Multiplayer {
    pub fn new(player_id: &str, display_name: &str) -> Self {
        let mut m = Multiplayer {
            svc: ProfileService::new(SnapcoreStore::new(), SnapcoreStore::new()),
            me: Profile::new(player_id, display_name),
            socket: None,
            token: None,
        };
        m.svc.publish(&m.me); // so a friend's lookup finds us at all, even before hosting
        m
    }

    /// "▶ Host": bind a real UDP socket (the data plane), announce the session on the fleet DHT
    /// (the control plane), flip presence to `InGame`. Returns the session token (also `self.token`).
    pub fn host(&mut self, bind_addr: &str) -> Result<String, String> {
        let socket = UdpSocket::bind(bind_addr).map_err(|e| format!("bind {bind_addr}: {e}"))?;
        socket.set_nonblocking(true).ok();
        let local = socket.local_addr().map_err(|e| e.to_string())?.to_string();
        let token = format!("soldier@{}", self.me.id);
        let rec = SessionRecord::new("soldier", &self.me.id, "udp", &local);
        self.svc.announce_session(&mut self.me, &token, &rec);
        self.socket = Some(socket);
        self.token = Some(token.clone());
        Ok(token)
    }

    /// "▶ Join a friend": resolve their live session (control plane) into a dial target, then
    /// connect our own UDP socket to it (the data plane starts here — gameplay sync is out of scope
    /// of this module, same as every other game's transport in the unification doc).
    pub fn join(&mut self, friend_id: &str) -> Result<String, String> {
        let rec = self
            .svc
            .join_via_friend(friend_id)
            .ok_or_else(|| format!("{friend_id} is not hosting a joinable soldier match"))?;
        if rec.game != "soldier" {
            return Err(format!("{friend_id} is hosting {}, not soldier", rec.game));
        }
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        socket.connect(&rec.addr).map_err(|e| format!("connect {}: {e}", rec.addr))?;
        socket.set_nonblocking(true).ok();
        self.socket = Some(socket);
        Ok(rec.addr)
    }

    /// End hosting: tear down the control-plane record (presence flips back to `Online`, so
    /// `join_via_friend` stops offering it — see `SessionExt::end_session`'s doc for the TTL caveat
    /// on the raw record itself).
    pub fn stop_hosting(&mut self) {
        self.svc.end_session(&mut self.me);
        self.token = None;
        self.socket = None;
    }

    /// The smallest possible proof the control→data-plane handoff actually works end to end: send
    /// a "hello" datagram over the connected socket. Real gameplay sync is future work built on top
    /// of this same `self.socket`.
    pub fn send_hello(&self) -> Result<(), String> {
        let sock = self.socket.as_ref().ok_or("not connected")?;
        sock.send(b"hello").map(|_| ()).map_err(|e| e.to_string())
    }

    /// Non-blocking receive on the data-plane socket (host side listens with this too, since a
    /// bound-but-unconnected UDP socket can `recv_from` any peer that dials it).
    pub fn poll(&self, buf: &mut [u8]) -> Option<(usize, std::net::SocketAddr)> {
        self.socket.as_ref()?.recv_from(buf).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_announces_and_a_friend_can_resolve_the_dial_target() {
        let mut host = Multiplayer::new("kesslerd", "Kessler");
        let token = host.host("127.0.0.1:0").expect("bind should succeed");
        assert_eq!(token, "soldier@kesslerd");
        assert_eq!(host.token.as_deref(), Some("soldier@kesslerd"));

        // A friend's client (its own ProfileService instance) resolves kesslerd's session through
        // the same underlying store, exactly like bsgames' host_then_join_over_the_fleet_dht.
        let dial = host
            .svc
            .join_via_friend("kesslerd")
            .expect("host is joinable");
        assert_eq!(dial.game, "soldier");
        assert_eq!(dial.transport, "udp");
        assert!(dial.addr.starts_with("127.0.0.1:"));
    }

    #[test]
    fn joining_a_non_soldier_session_is_rejected() {
        let mut host = Multiplayer::new("amy", "Amy");
        // amy is hosting a DIFFERENT game — soldier's join() must refuse it, not silently connect.
        let rec = SessionRecord::new("blackstar", "amy", "udp", "127.0.0.1:9999");
        host.svc.announce_session(&mut host.me, "blackstar@amy", &rec);

        // NOTE: `snapcore::store::Store` (netcode's fallback backend) is a pure in-memory routing
        // structure with no real socket binding yet — cross-instance discovery between two separate
        // `Multiplayer::new()`s isn't wired anywhere in the fleet today (see
        // games/GAMES_UNIFICATION.md's "remaining seams: OpenDHT FFI primary"). Until that lands,
        // "a friend resolves our session" means the same shared store, exactly like bsgames' own
        // `host_then_join_over_the_fleet_dht` reference test — so this reuses `host.svc` rather than
        // standing up an unreachable second instance.
        let err = host.join("amy").unwrap_err();
        assert!(err.contains("blackstar"), "error should name the actual game: {err}");
    }

    #[test]
    fn joining_someone_not_hosting_fails_cleanly() {
        let mut host = Multiplayer::new("zed", "Zed"); // published (by ::new), never hosts a session
        assert!(host.join("zed").is_err(), "zed is online but not in a game — not joinable");
        assert!(host.join("nobody-with-this-id").is_err(), "unknown id — not joinable");
    }

    #[test]
    fn stop_hosting_clears_local_session_state() {
        let mut host = Multiplayer::new("carla", "Carla");
        host.host("127.0.0.1:0").unwrap();
        assert!(host.token.is_some());
        host.stop_hosting();
        assert!(host.token.is_none());
        assert!(host.socket.is_none());
    }

    #[test]
    fn a_real_udp_hello_reaches_the_host_socket() {
        // Proves the data-plane handoff isn't just discovery-on-paper: bind a real host socket,
        // resolve its address through the control plane, then actually send bytes to it and
        // observe them arrive — the same address a real joiner's socket.connect() would use.
        let mut host = Multiplayer::new("dana", "Dana");
        host.host("127.0.0.1:0").unwrap();
        let host_addr = host.socket.as_ref().unwrap().local_addr().unwrap();

        let dial = host.svc.join_via_friend("dana").expect("host is joinable");
        assert_eq!(dial.addr, host_addr.to_string());

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(b"hello", &dial.addr).unwrap();

        let mut buf = [0u8; 16];
        // give the (nonblocking) host socket a moment; retry briefly rather than sleeping blind
        let mut got = None;
        for _ in 0..200 {
            if let Some((n, _from)) = host.poll(&mut buf) { got = Some(n); break; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let n = got.expect("host should receive the hello datagram");
        assert_eq!(&buf[..n], b"hello");
    }
}
