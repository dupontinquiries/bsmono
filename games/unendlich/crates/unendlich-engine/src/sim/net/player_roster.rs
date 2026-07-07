//! sim::net::player_roster — the connected-player table.
//!
//! The foundation the rest of multiplayer reads: who is in the world, which
//! **slot** they hold, their **role** (host / admin / player), and their
//! connection **state** (connecting / active / disconnected). `session_mode`
//! decides host-vs-client and `identity` authenticates a joiner; this is the
//! live roster the host maintains once they're in — slot assignment is
//! deterministic (lowest free slot) so it's stable across peers, disconnects
//! keep a slot reserved for reconnect, and the **admin** role is the trust gate
//! that later features (e.g. admin-pooled chunk generation) check. Pure,
//! deterministic, std-only.

/// Max concurrent players in a world.
pub const MAX_PLAYERS: usize = 8;

/// A stable per-player network id (assigned at join, survives reconnect).
pub type PlayerId = u64;

/// A roster slot index, `0..MAX_PLAYERS`.
pub type Slot = usize;

/// A player's authority role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The world owner (exactly one). Full authority.
    Host,
    /// A trusted player granted elevated powers (world edit, pooling compute…).
    Admin,
    /// An ordinary player.
    Player,
}

impl Role {
    /// Host and Admin both count as privileged.
    pub fn is_privileged(self) -> bool {
        matches!(self, Role::Host | Role::Admin)
    }
}

/// Connection state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnState {
    /// Handshaking / streaming the initial world.
    Connecting,
    /// In the world and receiving updates.
    Active,
    /// Dropped, but the slot is held for a possible reconnect.
    Disconnected,
}

/// One roster entry.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerEntry {
    pub id: PlayerId,
    pub slot: Slot,
    pub name: String,
    pub role: Role,
    pub state: ConnState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// No free slot.
    RosterFull,
    /// An entry with this id already exists (use `reconnect`).
    DuplicateId,
}

/// The connected-player roster (host-authoritative).
#[derive(Clone, Debug, Default)]
pub struct Roster {
    slots: Vec<Option<PlayerEntry>>,
}

impl Roster {
    pub fn new() -> Roster {
        Roster { slots: (0..MAX_PLAYERS).map(|_| None).collect() }
    }

    fn find_id(&self, id: PlayerId) -> Option<Slot> {
        self.slots.iter().position(|e| e.as_ref().map(|p| p.id) == Some(id))
    }

    /// Add a player, assigning the lowest free slot. New entries start
    /// `Connecting`. The first player to join as `Host` is the owner.
    pub fn join(&mut self, id: PlayerId, name: impl Into<String>, role: Role) -> Result<Slot, JoinError> {
        if self.find_id(id).is_some() {
            return Err(JoinError::DuplicateId);
        }
        let slot = self.slots.iter().position(|e| e.is_none()).ok_or(JoinError::RosterFull)?;
        self.slots[slot] = Some(PlayerEntry {
            id,
            slot,
            name: name.into(),
            role,
            state: ConnState::Connecting,
        });
        Ok(slot)
    }

    /// Remove a player entirely, freeing their slot for reuse.
    pub fn leave(&mut self, id: PlayerId) -> bool {
        match self.find_id(id) {
            Some(slot) => {
                self.slots[slot] = None;
                true
            }
            None => false,
        }
    }

    /// Mark a player disconnected but keep their slot for reconnect.
    pub fn disconnect(&mut self, id: PlayerId) -> bool {
        self.set_state(id, ConnState::Disconnected)
    }

    /// Restore a disconnected player to `Active` in their held slot.
    pub fn reconnect(&mut self, id: PlayerId) -> bool {
        match self.find_id(id) {
            Some(slot) if self.slots[slot].as_ref().unwrap().state == ConnState::Disconnected => {
                self.slots[slot].as_mut().unwrap().state = ConnState::Active;
                true
            }
            _ => false,
        }
    }

    /// Set a player's connection state.
    pub fn set_state(&mut self, id: PlayerId, state: ConnState) -> bool {
        match self.find_id(id) {
            Some(slot) => {
                self.slots[slot].as_mut().unwrap().state = state;
                true
            }
            None => false,
        }
    }

    /// Change a player's role (e.g. the host promoting someone to Admin).
    pub fn set_role(&mut self, id: PlayerId, role: Role) -> bool {
        match self.find_id(id) {
            Some(slot) => {
                self.slots[slot].as_mut().unwrap().role = role;
                true
            }
            None => false,
        }
    }

    pub fn by_id(&self, id: PlayerId) -> Option<&PlayerEntry> {
        self.find_id(id).and_then(|s| self.slots[s].as_ref())
    }
    pub fn by_slot(&self, slot: Slot) -> Option<&PlayerEntry> {
        self.slots.get(slot).and_then(|e| e.as_ref())
    }
    pub fn slot_of(&self, id: PlayerId) -> Option<Slot> {
        self.find_id(id)
    }

    /// The host, if one is present.
    pub fn host(&self) -> Option<&PlayerEntry> {
        self.iter().find(|p| p.role == Role::Host)
    }

    /// Is this player Host or Admin?
    pub fn is_privileged(&self, id: PlayerId) -> bool {
        self.by_id(id).map(|p| p.role.is_privileged()).unwrap_or(false)
    }

    /// Iterate all occupied entries in slot order.
    pub fn iter(&self) -> impl Iterator<Item = &PlayerEntry> {
        self.slots.iter().filter_map(|e| e.as_ref())
    }
    /// Iterate the players available for work/interaction (`Active`).
    pub fn active(&self) -> impl Iterator<Item = &PlayerEntry> {
        self.iter().filter(|p| p.state == ConnState::Active)
    }
    /// Iterate the privileged (host + admin) players.
    pub fn privileged(&self) -> impl Iterator<Item = &PlayerEntry> {
        self.iter().filter(|p| p.role.is_privileged())
    }

    /// Occupied slots.
    pub fn count(&self) -> usize {
        self.iter().count()
    }
    pub fn active_count(&self) -> usize {
        self.active().count()
    }
    pub fn is_full(&self) -> bool {
        self.slots.iter().all(|e| e.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_assigns_lowest_free_slot() {
        let mut r = Roster::new();
        assert_eq!(r.join(10, "host", Role::Host), Ok(0));
        assert_eq!(r.join(20, "alice", Role::Player), Ok(1));
        assert_eq!(r.join(30, "bob", Role::Player), Ok(2));
        assert_eq!(r.count(), 3);
        assert_eq!(r.by_slot(1).unwrap().name, "alice");
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let mut r = Roster::new();
        r.join(10, "a", Role::Player).unwrap();
        assert_eq!(r.join(10, "a-again", Role::Player), Err(JoinError::DuplicateId));
    }

    #[test]
    fn roster_fills_and_rejects() {
        let mut r = Roster::new();
        for i in 0..MAX_PLAYERS as u64 {
            r.join(i, format!("p{i}"), Role::Player).unwrap();
        }
        assert!(r.is_full());
        assert_eq!(r.join(999, "late", Role::Player), Err(JoinError::RosterFull));
    }

    #[test]
    fn leaving_frees_the_slot_for_reuse() {
        let mut r = Roster::new();
        r.join(10, "a", Role::Host).unwrap();
        r.join(20, "b", Role::Player).unwrap(); // slot 1
        assert!(r.leave(10)); // frees slot 0
        assert_eq!(r.join(30, "c", Role::Player), Ok(0), "lowest free slot reused");
        assert!(!r.leave(999));
    }

    #[test]
    fn disconnect_holds_slot_then_reconnect_restores() {
        let mut r = Roster::new();
        r.join(10, "a", Role::Host).unwrap();
        let slot = r.join(20, "b", Role::Player).unwrap();
        r.set_state(20, ConnState::Active);
        assert!(r.disconnect(20));
        assert_eq!(r.by_id(20).unwrap().state, ConnState::Disconnected);
        assert_eq!(r.by_id(20).unwrap().slot, slot, "slot still reserved");
        // a new join must NOT take the reserved slot
        assert_eq!(r.join(30, "c", Role::Player), Ok(2));
        assert!(r.reconnect(20));
        assert_eq!(r.by_id(20).unwrap().state, ConnState::Active);
        assert!(!r.reconnect(999));
    }

    #[test]
    fn promotion_and_privilege() {
        let mut r = Roster::new();
        r.join(10, "host", Role::Host).unwrap();
        r.join(20, "p", Role::Player).unwrap();
        assert!(!r.is_privileged(20));
        assert!(r.set_role(20, Role::Admin));
        assert!(r.is_privileged(20), "admin is privileged");
        assert!(r.is_privileged(10), "host is privileged");
        assert_eq!(r.privileged().count(), 2);
    }

    #[test]
    fn host_and_active_queries() {
        let mut r = Roster::new();
        r.join(10, "host", Role::Host).unwrap();
        r.join(20, "a", Role::Player).unwrap();
        r.set_state(10, ConnState::Active);
        r.set_state(20, ConnState::Active);
        r.disconnect(20);
        assert_eq!(r.host().unwrap().id, 10);
        assert_eq!(r.active_count(), 1, "only the host is active");
    }

    #[test]
    fn deterministic() {
        let mut a = Roster::new();
        let mut b = Roster::new();
        for (id, name) in [(1u64, "x"), (2, "y")] {
            a.join(id, name, Role::Player).unwrap();
            b.join(id, name, Role::Player).unwrap();
        }
        assert_eq!(a.iter().map(|p| p.slot).collect::<Vec<_>>(), b.iter().map(|p| p.slot).collect::<Vec<_>>());
    }
}
