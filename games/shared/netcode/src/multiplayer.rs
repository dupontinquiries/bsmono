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
        let local = socket.local_addr().map_err(|e| e.to_string())?.to_string();
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

    /// End hosting: tear down the control-plane record (presence flips back to `Online`, so
    /// `join_via_friend` stops offering it — see `SessionExt::end_session`'s doc for the TTL
    /// caveat on the raw record itself).
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
