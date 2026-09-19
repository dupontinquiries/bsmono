//! lanlobby — the state behind the LAN screen.
//!
//! Headless: `netcode::lan` does the sockets, this decides what the screen
//! shows and what its buttons mean, and `ui::draw_lan` only paints it. So the
//! screen's behaviour is testable without a window.
//!
//! WHAT THIS IS NOT: a networked match. Discovery, hosting a lobby and
//! picking one to join are real and work over a real network. Pressing JOIN
//! hands back the host's address and stops there, because the simulation is
//! not yet synchronised between machines -- it is deterministic at a fixed
//! tick with a seeded RNG, which is what lockstep needs, but no input
//! exchange exists. The screen says so rather than pretending.

use netcode::lan::{Announcer, Beacon, Browser, Found};
use std::net::SocketAddr;
use std::time::Instant;

pub const GAME: &str = "clawbound";
/// Bump when the sim changes in a way two builds could not share a match.
pub const PROTO: u16 = 1;
pub const SLOTS: u8 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub players: u8,
    pub slots: u8,
    pub addr: SocketAddr,
    pub joinable: bool,
    /// Why not, in the player's words.
    pub note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Back,
    /// The player picked a lobby. The caller owns what happens next.
    Join(SocketAddr),
}

pub struct LanLobby {
    pub my_name: String,
    my_id: String,
    browser: Result<Browser, String>,
    announcer: Option<Announcer>,
    pub hosting: bool,
    pub rows: Vec<Row>,
    /// One line for the screen's footer: what is going on, or what is wrong.
    pub status: String,
}

impl LanLobby {
    pub fn open(my_name: &str) -> LanLobby {
        Self::open_on(my_name, netcode::lan::LAN_PORT)
    }

    pub fn open_on(my_name: &str, port: u16) -> LanLobby {
        let my_id = format!("{my_name}-{:x}", std::process::id());
        let browser = Browser::on_port(GAME, PROTO, &my_id, port).map_err(|e| {
            // The one failure a player will actually hit: a second copy on
            // the same machine. Say THAT, not "os error 98".
            if e.kind() == std::io::ErrorKind::AddrInUse {
                "Another copy of the game on this machine is already listening for LAN games.".to_string()
            } else {
                format!("Cannot listen for LAN games: {e}")
            }
        });
        let status = browser.as_ref().err().cloned().unwrap_or_else(|| "Searching the local network...".into());
        LanLobby { my_name: my_name.into(), my_id, browser, announcer: None, hosting: false, rows: Vec::new(), status }
    }

    pub fn port(&self) -> Option<u16> {
        self.browser.as_ref().ok().map(|b| b.port())
    }

    pub fn set_hosting(&mut self, on: bool) {
        self.hosting = on;
        if !on {
            self.announcer = None;
        } else if self.announcer.is_none() {
            match Announcer::new() {
                Ok(a) => self.announcer = Some(a),
                Err(e) => {
                    self.hosting = false;
                    self.status = format!("Cannot announce on this network: {e}");
                }
            }
        }
    }

    /// For tests and unusual networks: announce somewhere specific.
    pub fn host_to(&mut self, to: Vec<SocketAddr>) {
        self.announcer = Announcer::to(to).ok();
        self.hosting = self.announcer.is_some();
    }

    /// Once a frame.
    pub fn tick(&mut self, now: Instant, game_port: u16, players: u8) {
        if let (true, Some(a)) = (self.hosting, self.announcer.as_mut()) {
            a.tick(&Beacon { game: GAME.into(), host_id: self.my_id.clone(), name: format!("{}'s den", self.my_name), port: game_port, players, slots: SLOTS, proto: PROTO }, now);
        }
        let Ok(b) = self.browser.as_mut() else { return };
        let found: Vec<Found> = b.poll(now).to_vec();
        self.rows = found
            .iter()
            .map(|f| {
                let joinable = b.joinable(f);
                let note = if f.beacon.proto != PROTO {
                    "different version"
                } else if f.beacon.players >= f.beacon.slots {
                    "full"
                } else {
                    ""
                };
                Row { name: f.beacon.name.clone(), players: f.beacon.players, slots: f.beacon.slots, addr: f.addr, joinable, note }
            })
            .collect();
        self.status = match (self.hosting, self.rows.len()) {
            (true, _) => format!("Hosting as \"{}'s den\" -- visible to this network.", self.my_name),
            (false, 0) => "Searching the local network... no games yet.".into(),
            (false, n) => format!("{n} game{} on this network.", if n == 1 { "" } else { "s" }),
        };
    }

    /// A click on row `i`. Unjoinable rows do nothing: the button is drawn
    /// disabled, and this is the second lock on the same door.
    pub fn click_join(&self, i: usize) -> Action {
        match self.rows.get(i) {
            Some(r) if r.joinable => Action::Join(r.addr),
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    /// Two lobbies on one machine, real sockets: one hosts, the other sees
    /// it, can join it, and loses it when the host stops.
    #[test]
    fn a_hosted_den_appears_on_the_other_screen_and_leaves_when_hosting_stops() {
        let mut seeker = LanLobby::open_on("Ash", 0);
        let mut host = LanLobby::open_on("Bramble", 0);
        let to = vec![(Ipv4Addr::LOCALHOST, seeker.port().expect("seeker is listening")).into()];
        let t0 = Instant::now();
        seeker.tick(t0, 0, 1);
        assert!(seeker.rows.is_empty() && seeker.status.contains("no games"), "{}", seeker.status);
        assert_eq!(seeker.click_join(0), Action::None, "nothing to click");

        host.host_to(to);
        host.tick(t0, 41_000, 3);
        assert!(host.status.contains("Hosting"), "{}", host.status);
        for _ in 0..50 {
            seeker.tick(t0, 0, 1);
            if !seeker.rows.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_eq!(seeker.rows.len(), 1, "{}", seeker.status);
        let r = &seeker.rows[0];
        assert_eq!((r.name.as_str(), r.players, r.slots, r.joinable, r.note), ("Bramble's den", 3, SLOTS, true, ""));
        assert_eq!(seeker.click_join(0), Action::Join((Ipv4Addr::LOCALHOST, 41_000).into()), "JOIN hands back the host's GAME socket");
        assert!(host.rows.is_empty(), "a host does not list itself");

        host.set_hosting(false);
        let later = t0 + netcode::lan::STALE_AFTER + Duration::from_millis(50);
        host.tick(later, 41_000, 3);
        seeker.tick(later, 0, 1);
        assert!(seeker.rows.is_empty(), "the den closed and the row went with it");
    }

    #[test]
    fn a_second_copy_on_the_same_port_is_told_why_in_words() {
        let first = LanLobby::open_on("Ash", 0);
        let second = LanLobby::open_on("Ash", first.port().unwrap());
        assert!(second.port().is_none());
        assert!(second.status.contains("Another copy"), "got: {}", second.status);
        assert!(!second.status.contains("os error"), "an errno is not a message");
    }
}
