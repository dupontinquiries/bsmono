//! toplevel — turns a stream of `ext-foreign-toplevel-list-v1` (+
//! `zcosmic_toplevel_manager_v1` for actions) events into a clean, queryable
//! list of open windows: the model an app-switcher/task-view actually wants.
//!
//! Deliberately protocol-shaped, not wayland-shaped: `ToplevelEvent` mirrors
//! what the real protocol emits (new handle, then title/app_id/state as
//! they're known, then closed) but carries no wayland types at all — an
//! opaque `u64` id assigned by `wayland_io`'s real listener stands in for
//! the actual `wl_proxy` object identity. That's what makes this testable
//! with plain Rust values and no compositor: feed a `Vec<ToplevelEvent>` in,
//! assert on `ToplevelTracker::list()` after.
//!
//! ## Why NOT `wlr-foreign-toplevel-management`
//!
//! Recorded here so the choice doesn't get silently re-litigated: cosmic-comp
//! implements `ext-foreign-toplevel-list-v1` (info-only) paired with its own
//! `zcosmic_toplevel_manager_v1` (activate/close/minimize) — confirmed by
//! reading `cosmic-workspaces-epoch`'s actual client code. The older,
//! wlroots-specific `wlr-foreign-toplevel-management-unstable-v1` is a
//! DIFFERENT, non-interoperable protocol family for the same job — the
//! phosh/phoc side of this project's own research confirmed phosh only
//! speaks that one, not this one. This crate targets what the actual base
//! compositor (`cosmic-comp`) speaks, not what a different compositor would
//! need.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToplevelState {
    pub maximized: bool,
    pub minimized: bool,
    pub activated: bool,
    pub fullscreen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToplevelInfo {
    pub id: u64,
    /// Empty until the protocol's own `title` event arrives — never
    /// defaulted to something presentable like "Untitled", so a caller
    /// can tell "genuinely has no title yet" from "has an empty title",
    /// which matters for a switcher that wants to skip painting a tile
    /// until there's something to show.
    pub title: String,
    pub app_id: String,
    pub state: ToplevelState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToplevelEvent {
    /// A new toplevel handle appeared. Always the first event for a given
    /// id — everything else assumes an entry already exists.
    New(u64),
    Title(u64, String),
    AppId(u64, String),
    StateChanged(u64, ToplevelState),
    Closed(u64),
}

#[derive(Debug, Default)]
pub struct ToplevelTracker {
    // Insertion order preserved deliberately: a switcher's tile order
    // shouldn't jump around just because a later event touched an earlier
    // window (an id lookup by HashMap would iterate in an unspecified,
    // effectively-random order instead).
    entries: Vec<ToplevelInfo>,
}

impl ToplevelTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    /// Applies one event. An event for an id with no prior `New` (a
    /// malformed or reordered stream — shouldn't happen per the protocol's
    /// own contract, but "shouldn't happen" is not "provably can't", and a
    /// compositor bug on the other end of a socket is not this tracker's
    /// job to crash over) is dropped silently rather than panicking or
    /// synthesizing a `New` implicitly, which would hide the very stream
    /// corruption a caller might want to know about via some other signal.
    pub fn apply(&mut self, event: ToplevelEvent) {
        match event {
            ToplevelEvent::New(id) => {
                if self.index_of(id).is_none() {
                    self.entries.push(ToplevelInfo {
                        id,
                        title: String::new(),
                        app_id: String::new(),
                        state: ToplevelState::default(),
                    });
                }
                // A second New for an id already tracked is a no-op, not a
                // reset — losing an already-known title/app_id to a
                // duplicate announcement would be a regression a caller
                // has no way to detect.
            }
            ToplevelEvent::Title(id, title) => {
                if let Some(i) = self.index_of(id) {
                    self.entries[i].title = title;
                }
            }
            ToplevelEvent::AppId(id, app_id) => {
                if let Some(i) = self.index_of(id) {
                    self.entries[i].app_id = app_id;
                }
            }
            ToplevelEvent::StateChanged(id, state) => {
                if let Some(i) = self.index_of(id) {
                    self.entries[i].state = state;
                }
            }
            ToplevelEvent::Closed(id) => {
                if let Some(i) = self.index_of(id) {
                    self.entries.remove(i);
                }
            }
        }
    }

    pub fn list(&self) -> &[ToplevelInfo] {
        &self.entries
    }

    pub fn get(&self, id: u64) -> Option<&ToplevelInfo> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The single activated toplevel, if any. More than one activated
    /// entry (a compositor bug, or a race during a fast focus change) is
    /// resolved to "none" rather than picking one arbitrarily — a
    /// switcher highlighting an arbitrary window as "focused" when the
    /// tracker's own state is ambiguous would be a worse failure than
    /// highlighting nothing for one frame.
    pub fn activated(&self) -> Option<&ToplevelInfo> {
        let mut found = None;
        for e in &self.entries {
            if e.state.activated {
                if found.is_some() {
                    return None; // more than one — ambiguous, not "first wins"
                }
                found = Some(e);
            }
        }
        found
    }
}

/// A window-management request, protocol-shaped but wayland-free, so the
/// capability check below is testable without a compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToplevelRequest {
    Activate,
    Close,
    SetMaximized(bool),
    SetMinimized(bool),
    SetFullscreen(bool),
}

/// `zcosmic_toplevel_manager_v1.capabilities`: which requests the compositor
/// honours. Per the protocol it IGNORES unsupported requests silently, so a
/// shell must check first or its buttons do nothing with no error anywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagerCaps {
    caps: Vec<u32>,
    /// False until the event arrives; the protocol sends it once right after
    /// the manager is bound, so "unknown" is a short, real state.
    pub received: bool,
}

impl ManagerCaps {
    /// Decode the wire array: native-endian u32s, one capability each
    /// (a list, not a bitmask — same shape as the handle's `state` array).
    pub fn from_wire(raw: &[u8]) -> ManagerCaps {
        let caps = raw.chunks_exact(4).map(|c| u32::from_ne_bytes(c.try_into().unwrap())).collect();
        ManagerCaps { caps, received: true }
    }

    pub fn supports(&self, req: ToplevelRequest) -> bool {
        let want = match req {
            ToplevelRequest::Close => 1,
            ToplevelRequest::Activate => 2,
            ToplevelRequest::SetMaximized(_) => 3,
            ToplevelRequest::SetMinimized(_) => 4,
            ToplevelRequest::SetFullscreen(_) => 5,
        };
        self.caps.contains(&want)
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    fn wire(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_ne_bytes()).collect()
    }

    #[test]
    fn caps_decode_as_a_list_not_a_bitmask() {
        // [2, 3] = activate + maximize. Read as a bitmask, 3 would also claim
        // close (bit 0) — exactly the misread this guards against.
        let c = ManagerCaps::from_wire(&wire(&[2, 3]));
        assert!(c.received);
        assert!(c.supports(ToplevelRequest::Activate));
        assert!(c.supports(ToplevelRequest::SetMaximized(true)));
        assert!(c.supports(ToplevelRequest::SetMaximized(false)));
        assert!(!c.supports(ToplevelRequest::Close));
        assert!(!c.supports(ToplevelRequest::SetFullscreen(true)));
    }

    #[test]
    fn unknown_and_trailing_bytes_are_ignored() {
        let mut raw = wire(&[1, 99]);
        raw.push(7); // truncated trailing element
        let c = ManagerCaps::from_wire(&raw);
        assert!(c.supports(ToplevelRequest::Close));
        assert!(!c.supports(ToplevelRequest::Activate));
        assert!(!ManagerCaps::default().received);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_lifecycle_produces_a_populated_then_removed_entry() {
        let mut t = ToplevelTracker::new();
        t.apply(ToplevelEvent::New(1));
        t.apply(ToplevelEvent::Title(1, "Terminal".into()));
        t.apply(ToplevelEvent::AppId(1, "org.example.Terminal".into()));
        t.apply(ToplevelEvent::StateChanged(1, ToplevelState { activated: true, ..Default::default() }));

        let e = t.get(1).expect("tracked after New");
        assert_eq!(e.title, "Terminal");
        assert_eq!(e.app_id, "org.example.Terminal");
        assert!(e.state.activated);

        t.apply(ToplevelEvent::Closed(1));
        assert!(t.get(1).is_none());
        assert!(t.list().is_empty());
    }

    #[test]
    fn events_for_an_untracked_id_are_dropped_not_panicked_on() {
        let mut t = ToplevelTracker::new();
        // No New(5) ever sent — a malformed/reordered stream.
        t.apply(ToplevelEvent::Title(5, "ghost".into()));
        t.apply(ToplevelEvent::StateChanged(5, ToplevelState::default()));
        t.apply(ToplevelEvent::Closed(5));
        assert!(t.list().is_empty(), "nothing materialized from events with no matching New");
    }

    #[test]
    fn a_duplicate_new_does_not_reset_already_known_fields() {
        let mut t = ToplevelTracker::new();
        t.apply(ToplevelEvent::New(1));
        t.apply(ToplevelEvent::Title(1, "Notes".into()));
        t.apply(ToplevelEvent::New(1)); // a duplicate announcement
        assert_eq!(t.get(1).unwrap().title, "Notes", "title must survive a duplicate New");
    }

    #[test]
    fn insertion_order_is_preserved_regardless_of_which_id_is_touched_later() {
        let mut t = ToplevelTracker::new();
        t.apply(ToplevelEvent::New(1));
        t.apply(ToplevelEvent::New(2));
        t.apply(ToplevelEvent::New(3));
        t.apply(ToplevelEvent::Title(1, "third update, first window".into()));
        let ids: Vec<u64> = t.list().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 2, 3], "a switcher's tile order shouldn't jump because of WHICH window got touched");
    }

    #[test]
    fn exactly_one_activated_toplevel_is_found() {
        let mut t = ToplevelTracker::new();
        t.apply(ToplevelEvent::New(1));
        t.apply(ToplevelEvent::New(2));
        t.apply(ToplevelEvent::StateChanged(2, ToplevelState { activated: true, ..Default::default() }));
        assert_eq!(t.activated().map(|e| e.id), Some(2));
    }

    #[test]
    fn no_activated_toplevel_is_none_not_a_panic() {
        let mut t = ToplevelTracker::new();
        t.apply(ToplevelEvent::New(1));
        assert_eq!(t.activated(), None);
    }

    #[test]
    fn two_simultaneously_activated_toplevels_resolve_to_ambiguous_not_first_wins() {
        // A real compositor shouldn't ever do this, but "shouldn't" isn't
        // "can't" — a fast focus-change race is exactly the kind of thing
        // that produces a brief inconsistent state on a real socket.
        let mut t = ToplevelTracker::new();
        t.apply(ToplevelEvent::New(1));
        t.apply(ToplevelEvent::New(2));
        t.apply(ToplevelEvent::StateChanged(1, ToplevelState { activated: true, ..Default::default() }));
        t.apply(ToplevelEvent::StateChanged(2, ToplevelState { activated: true, ..Default::default() }));
        assert_eq!(t.activated(), None, "ambiguous, not an arbitrary pick");
    }
}
