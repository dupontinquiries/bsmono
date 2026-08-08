//! lobby — announcing a tournament so people can find it.
//!
//! `netcode::tournament` runs a bracket once you have the entrants. Getting the
//! entrants is a different problem, and the crate could not do it: `join` takes
//! a `friend_id`, so you could only enter a tournament somebody had already told
//! you about out of band.
//!
//! This is the discovery half, and it deliberately mirrors `session` rather than
//! inventing a second convention: a version byte, length-prefixed strings, a
//! `kind:token` key, and the same `ProfileService` backends. A parallel wire
//! format would be one more thing to keep in step with the DHT nobody wants to
//! debug twice.
//!
//! ## Announcing is not the same as being joinable
//!
//! A record on the DHT outlives the process that put it there. If "found" meant
//! "joinable", every crashed host would leave a lobby that swallows the next
//! person who taps it — and DHT records persist for their TTL regardless of
//! whether anyone is still listening. So a `LobbyRecord` carries an explicit
//! `open` flag and a `seen_ms`, and `is_joinable` requires both: still open, and
//! recently refreshed.
//!
//! ## Full is a state, not an absence
//!
//! A tournament at capacity should still be *visible* — you want to see it, know
//! it filled, and wait for the next. Dropping full lobbies from discovery makes
//! a busy game look dead, which is precisely backwards.

use crate::session::{SessionExt, WIRE_V, put_str, take_str};
use crate::store::ProfileBackend;

/// How stale an announcement may be and still count as live.
///
/// Ninety seconds. Long enough to survive a slow refresh or a phone that slept
/// briefly, short enough that a crashed host stops advertising before somebody
/// walks into it.
pub const LOBBY_STALE_MS: i64 = 90_000;

/// A tournament looking for entrants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyRecord {
    /// Game name, matching `SessionRecord::game`.
    pub game: String,
    /// Host player id — the discovery anchor, as with sessions.
    pub host: String,
    /// Human-readable tournament name.
    pub name: String,
    /// Entrants already registered.
    pub entrants: u32,
    /// Cap. Zero means no cap, which is different from being full.
    pub capacity: u32,
    /// Whether the host is still accepting.
    pub open: bool,
    /// When the host last refreshed this record.
    pub seen_ms: i64,
}

impl LobbyRecord {
    /// Is this lobby at capacity?
    ///
    /// A zero capacity is uncapped rather than instantly full — the obvious
    /// off-by-one here would make every uncapped tournament un-joinable.
    pub fn is_full(&self) -> bool {
        self.capacity > 0 && self.entrants >= self.capacity
    }

    /// Has the host gone quiet?
    pub fn is_stale(&self, now_ms: i64) -> bool {
        // A record from the future is clock skew between peers, not freshness
        // to be trusted — but it is also not evidence of staleness, so it
        // counts as fresh rather than being discarded.
        now_ms - self.seen_ms > LOBBY_STALE_MS
    }

    /// Can somebody actually enter?
    ///
    /// Three separate conditions, none of which implies the others: a lobby can
    /// be fresh and closed, open and full, or open and abandoned.
    pub fn is_joinable(&self, now_ms: i64) -> bool {
        self.open && !self.is_full() && !self.is_stale(now_ms)
    }

    /// Why not, for a UI that has to explain a greyed-out row.
    ///
    /// "Cannot join" with no reason sends people to the support channel; each
    /// of these suggests a different response — wait, come back later, or give
    /// up on this one.
    pub fn blocked_reason(&self, now_ms: i64) -> Option<&'static str> {
        if self.is_stale(now_ms) { return Some("Host is not responding"); }
        if !self.open { return Some("Registration closed"); }
        if self.is_full() { return Some("Tournament is full"); }
        None
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![WIRE_V];
        put_str(&mut b, &self.game);
        put_str(&mut b, &self.host);
        put_str(&mut b, &self.name);
        b.extend_from_slice(&self.entrants.to_le_bytes());
        b.extend_from_slice(&self.capacity.to_le_bytes());
        b.push(self.open as u8);
        b.extend_from_slice(&self.seen_ms.to_le_bytes());
        b
    }

    pub fn decode(bytes: &[u8]) -> Option<LobbyRecord> {
        let mut b = bytes;
        // A record from a newer writer is REFUSED rather than parsed
        // optimistically: the fields may mean something else, and a lobby
        // showing a wrong entrant count is worse than one not showing at all.
        if b.first()? != &WIRE_V { return None; }
        b = &b[1..];
        let game = take_str(&mut b)?;
        let host = take_str(&mut b)?;
        let name = take_str(&mut b)?;
        if b.len() < 4 + 4 + 1 + 8 { return None; }
        let entrants = u32::from_le_bytes(b[..4].try_into().ok()?);
        let capacity = u32::from_le_bytes(b[4..8].try_into().ok()?);
        let open = b[8] != 0;
        let seen_ms = i64::from_le_bytes(b[9..17].try_into().ok()?);
        Some(LobbyRecord { game, host, name, entrants, capacity, open, seen_ms })
    }
}

/// The DHT key for a lobby, mirroring `session_key`.
pub fn lobby_key(token: &str) -> String { format!("lobby:{token}") }

/// Announce and find tournaments, layered on the same backends as sessions.
pub trait LobbyExt {
    /// Publish or refresh a lobby. Refreshing is the same call — the record is
    /// last-write-wins, and a host that stops calling it goes stale on its own
    /// rather than needing a withdrawal that a crashed host can never send.
    fn announce_lobby(&mut self, token: &str, rec: &LobbyRecord) -> bool;

    /// Look up one lobby, with the reach that served it.
    ///
    /// `Reach` comes back because "found on the global DHT" and "found on the
    /// LAN fallback" mean different things to someone deciding whether a
    /// tournament is reachable — the session layer already surfaces it for the
    /// same reason, and dropping it here would make lobbies the one lookup
    /// that cannot say where its answer came from.
    fn find_lobby(&self, token: &str) -> (Option<LobbyRecord>, crate::store::Reach);
}

impl<P: ProfileBackend, F: ProfileBackend> LobbyExt for crate::store::ProfileService<P, F>
where crate::store::ProfileService<P, F>: SessionExt {
    fn announce_lobby(&mut self, token: &str, rec: &LobbyRecord) -> bool {
        self.put_raw(&lobby_key(token), rec.encode())
    }

    fn find_lobby(&self, token: &str) -> (Option<LobbyRecord>, crate::store::Reach) {
        let (bytes, reach) = self.get_raw(&lobby_key(token));
        (bytes.as_deref().and_then(LobbyRecord::decode), reach)
    }
}

/// Order lobbies for a browse list.
///
/// Joinable first, then the merely visible. Within each group, fuller lobbies
/// come first — a tournament with seven of eight starts sooner than one with
/// one, and starting sooner is what a person browsing is after.
pub fn browse(mut lobbies: Vec<LobbyRecord>, now_ms: i64) -> Vec<LobbyRecord> {
    lobbies.sort_by(|a, b| {
        b.is_joinable(now_ms).cmp(&a.is_joinable(now_ms))
            .then_with(|| b.entrants.cmp(&a.entrants))
            // Name last so the order is stable: two identical lobbies must not
            // swap places between refreshes on a list people are tapping.
            .then_with(|| a.name.cmp(&b.name))
    });
    lobbies
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> LobbyRecord {
        LobbyRecord { game: "echelon".into(), host: "p0".into(),
                      name: "Invitational I".into(), entrants: 3, capacity: 8,
                      open: true, seen_ms: 1_000 }
    }

    #[test]
    fn a_record_round_trips_through_the_wire_format() {
        let r = rec();
        assert_eq!(LobbyRecord::decode(&r.encode()), Some(r));
    }

    #[test]
    fn a_record_from_a_newer_writer_is_refused_rather_than_guessed_at() {
        // The fields may mean something else, and a lobby showing a wrong
        // entrant count is worse than one not showing at all.
        let mut b = rec().encode();
        b[0] = WIRE_V + 1;
        assert_eq!(LobbyRecord::decode(&b), None);
    }

    #[test]
    fn a_truncated_record_is_refused() {
        // DHT values can arrive clipped, and a half-read entrant count would
        // render as a plausible number.
        let b = rec().encode();
        for cut in [0, 1, 5, b.len() - 1] {
            assert_eq!(LobbyRecord::decode(&b[..cut]), None, "cut at {cut}");
        }
    }

    #[test]
    fn announcing_is_not_the_same_as_being_joinable() {
        // A DHT record outlives the process that put it there, so a crashed
        // host would otherwise leave a lobby that swallows the next tap.
        let mut r = rec();
        assert!(r.is_joinable(1_000));
        r.seen_ms = 1_000;
        assert!(!r.is_joinable(1_000 + LOBBY_STALE_MS + 1));
    }

    #[test]
    fn a_full_lobby_is_still_visible() {
        // You want to see it, know it filled, and wait for the next. Dropping
        // full lobbies makes a busy game look dead.
        let mut r = rec();
        r.entrants = 8;
        assert!(r.is_full());
        assert!(!r.is_joinable(1_000));
        assert_eq!(r.blocked_reason(1_000), Some("Tournament is full"));
    }

    #[test]
    fn an_uncapped_lobby_is_never_full() {
        // The obvious off-by-one here would make every uncapped tournament
        // un-joinable.
        let mut r = rec();
        r.capacity = 0;
        r.entrants = 9_999;
        assert!(!r.is_full());
        assert!(r.is_joinable(1_000));
    }

    #[test]
    fn the_three_blocking_conditions_are_independent() {
        // A lobby can be fresh and closed, open and full, or open and
        // abandoned; none implies the others.
        let mut closed = rec();
        closed.open = false;
        assert_eq!(closed.blocked_reason(1_000), Some("Registration closed"));

        let mut abandoned = rec();
        abandoned.seen_ms = 0;
        assert_eq!(abandoned.blocked_reason(LOBBY_STALE_MS + 1),
                   Some("Host is not responding"));

        assert_eq!(rec().blocked_reason(1_000), None);
    }

    #[test]
    fn staleness_outranks_the_other_reasons() {
        // A closed, full, abandoned lobby should say the host is gone — that is
        // the one the user can do nothing about, and the other two invite
        // waiting for a host who will never return.
        let mut r = rec();
        r.open = false;
        r.entrants = 8;
        r.seen_ms = 0;
        assert_eq!(r.blocked_reason(LOBBY_STALE_MS + 1), Some("Host is not responding"));
    }

    #[test]
    fn a_record_from_the_future_counts_as_fresh() {
        // Clock skew between peers is normal; it is not evidence of staleness,
        // and discarding those records would hide live lobbies from anyone
        // whose clock runs slow.
        let mut r = rec();
        r.seen_ms = 10_000_000;
        assert!(!r.is_stale(1_000));
        assert!(r.is_joinable(1_000));
    }

    #[test]
    fn browsing_puts_joinable_lobbies_first() {
        let mut full = rec();
        full.name = "Full".into();
        full.entrants = 8;
        let mut open = rec();
        open.name = "Open".into();
        open.entrants = 2;
        let list = browse(vec![full, open], 1_000);
        assert_eq!(list[0].name, "Open");
    }

    #[test]
    fn fuller_joinable_lobbies_come_first_because_they_start_sooner() {
        let mut nearly = rec();
        nearly.name = "Nearly".into();
        nearly.entrants = 7;
        let mut empty = rec();
        empty.name = "Empty".into();
        empty.entrants = 1;
        let list = browse(vec![empty, nearly], 1_000);
        assert_eq!(list[0].name, "Nearly");
    }

    #[test]
    fn the_browse_order_is_stable_between_refreshes() {
        // Two identical lobbies must not swap places on a list people are
        // tapping.
        let mut a = rec(); a.name = "Bravo".into();
        let mut b = rec(); b.name = "Alpha".into();
        let l1 = browse(vec![a.clone(), b.clone()], 1_000);
        let l2 = browse(vec![b, a], 1_000);
        assert_eq!(l1[0].name, "Alpha");
        assert_eq!(l1, l2);
    }

    #[test]
    fn an_empty_browse_is_empty_rather_than_a_placeholder() {
        assert!(browse(Vec::new(), 1_000).is_empty());
    }

    #[test]
    fn the_key_scheme_mirrors_sessions() {
        // A parallel convention would be one more thing to keep in step with
        // the DHT nobody wants to debug twice.
        assert_eq!(lobby_key("abc"), "lobby:abc");
        assert_ne!(lobby_key("abc"), crate::session::session_key("abc"));
    }
}
