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
use netcode::store::{FileStore, ProfileService};
use std::net::UdpSocket;

pub struct Multiplayer {
    svc: ProfileService<FileStore, FileStore>,
    pub me: Profile,
    pub socket: Option<UdpSocket>,
    pub token: Option<String>,
}

impl Multiplayer {
    pub fn new(player_id: &str, display_name: &str) -> Self {
        Self::at_store(player_id, display_name, FileStore::dev_default_path())
    }

    /// Same as `new`, but pointed at an explicit shared-store file rather than the real
    /// machine-wide default — every test needs its own path so parallel test runs (and repeated
    /// runs leaving stale data behind) can't interfere with each other or with a real running game.
    pub fn at_store(player_id: &str, display_name: &str, store_path: std::path::PathBuf) -> Self {
        // FileStore (a shared-file stand-in for real network transport — see its doc comment in
        // netcode::store) rather than SnapcoreStore: two separate Soldier processes (or bsgames and
        // a spawned Soldier, the handoff case) need to actually see each other's announcements for
        // local/LAN testing, which an isolated in-process SnapcoreStore never could.
        let mut m = Multiplayer {
            svc: ProfileService::new(FileStore::at(&store_path), FileStore::at(&store_path)),
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

    fn tmp_path(name: &str) -> std::path::PathBuf {
        // Every test gets its own file so parallel test runs (and this crate's other tests, and a
        // real running game on the same machine) can't interfere with each other via the shared
        // dev-default path `Multiplayer::new` actually uses outside tests.
        std::env::temp_dir().join(format!("soldier-mp-test-{name}-{}.jsonl", std::process::id()))
    }

    #[test]
    fn host_announces_and_a_friend_can_resolve_the_dial_target() {
        let path = tmp_path("resolve");
        let mut host = Multiplayer::at_store("kesslerd", "Kessler", path.clone());
        let token = host.host("127.0.0.1:0").expect("bind should succeed");
        assert_eq!(token, "soldier@kesslerd");
        assert_eq!(host.token.as_deref(), Some("soldier@kesslerd"));

        // A SEPARATE Multiplayer value — standing in for a friend's own process — resolves
        // kesslerd's session over the shared FileStore, not a reused/shared `svc` instance. This is
        // the actual bsgames→soldier handoff shape: two independent processes, one file on disk.
        let friend = Multiplayer::at_store("friendclient", "Friend", path.clone());
        let dial = friend.svc.join_via_friend("kesslerd").expect("host is joinable, over the shared file store");
        assert_eq!(dial.game, "soldier");
        assert_eq!(dial.transport, "udp");
        assert!(dial.addr.starts_with("127.0.0.1:"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn joining_a_non_soldier_session_is_rejected() {
        let path = tmp_path("wrong-game");
        let mut host = Multiplayer::at_store("amy", "Amy", path.clone());
        // amy is hosting a DIFFERENT game — soldier's join() must refuse it, not silently connect.
        let rec = SessionRecord::new("blackstar", "amy", "udp", "127.0.0.1:9999");
        host.svc.announce_session(&mut host.me, "blackstar@amy", &rec);

        let mut joiner = Multiplayer::at_store("bob", "Bob", path.clone());
        let err = joiner.join("amy").unwrap_err();
        assert!(err.contains("blackstar"), "error should name the actual game: {err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn joining_someone_not_hosting_fails_cleanly() {
        let path = tmp_path("not-hosting");
        let _host = Multiplayer::at_store("zed", "Zed", path.clone()); // published, never hosts
        let mut joiner = Multiplayer::at_store("bob2", "Bob", path.clone());
        assert!(joiner.join("zed").is_err(), "zed is online but not in a game — not joinable");
        assert!(joiner.join("nobody-with-this-id").is_err(), "unknown id — not joinable");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stop_hosting_clears_local_session_state() {
        let path = tmp_path("stop-hosting");
        let mut host = Multiplayer::at_store("carla", "Carla", path.clone());
        host.host("127.0.0.1:0").unwrap();
        assert!(host.token.is_some());
        host.stop_hosting();
        assert!(host.token.is_none());
        assert!(host.socket.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_real_udp_hello_reaches_the_host_socket_via_a_separate_joiner_instance() {
        // Proves the FULL handoff shape end to end: two independent Multiplayer values (host +
        // joiner, standing in for bsgames spawning a separate Soldier process), the joiner resolves
        // the host's real address purely through the shared FileStore, then actually dials it —
        // the same path a real bsgames→soldier handoff takes.
        let path = tmp_path("udp-hello");
        let mut host = Multiplayer::at_store("dana", "Dana", path.clone());
        host.host("127.0.0.1:0").unwrap();
        let host_addr = host.socket.as_ref().unwrap().local_addr().unwrap();

        let mut joiner = Multiplayer::at_store("erin", "Erin", path.clone());
        let addr = joiner.join("dana").expect("dana is hosting, resolved via the shared FileStore");
        assert_eq!(addr, host_addr.to_string());
        joiner.send_hello().expect("joiner's connected socket can send");

        let mut buf = [0u8; 16];
        let mut got = None;
        for _ in 0..200 {
            if let Some((n, _from)) = host.poll(&mut buf) { got = Some(n); break; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let n = got.expect("host should receive the joiner's hello datagram");
        assert_eq!(&buf[..n], b"hello");
        let _ = std::fs::remove_file(&path);
    }
}
