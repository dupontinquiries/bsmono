//! Consent state for face recognition features.
//!
//! Face recognition stores biometric features per contact. That has
//! regulatory weight in many jurisdictions:
//!
//!   - **EU GDPR Article 9**: biometric data used for unique
//!     identification requires explicit opt-in.
//!   - **Illinois BIPA**: explicit written consent + retention
//!     schedule.
//!   - **Various US state laws** following BIPA's lead.
//!
//! The consent layer captures the user's opt-in state and provides
//! the hooks that the rest of the face stack consults before doing
//! anything that would create biometric records.
//!
//! ## Default state
//!
//! `ConsentState::default()` returns `Disabled`. New users see no
//! face recognition until they explicitly enable it. The opt-in
//! prompt is the UI layer's job (this crate is server-side).
//!
//! ## What gets gated
//!
//!   - `Embedder::embed` is technically still callable when consent
//!     is off — it's a pure function over pixels — but the caller
//!     should not run it. `consent_required()` is a recommendation
//!     point for the call-site.
//!   - `IdentityMatcher::add_or_update_profile` refuses with
//!     `FaceError::ConsentDisabled` when matcher is disabled.
//!   - `IdentityMatcher::match_best` returns `None` when disabled.
//!
//! ## Wiping on consent withdrawal
//!
//! When the user toggles consent OFF:
//!   1. Call `IdentityMatcher::wipe()` to clear in-memory profiles
//!   2. Clear the persistent profile store (this crate does not
//!      provide one — the application chooses where to store, and
//!      is responsible for purging when consent goes off)
//!   3. Persist the new consent state
//!
//! All three steps should happen atomically from the user's
//! perspective; the UI should not return "consent off" until the
//! wipe completes.

use serde::{Deserialize, Serialize};

/// Whether face recognition features are permitted.
///
/// Stored persistently by the application; this crate is just the
/// type. Defaults to `Disabled` on first install — opt-in, not
/// opt-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsentState {
    /// Face recognition is disabled. Matcher returns no matches;
    /// profile additions are refused; no biometric data is stored.
    Disabled,
    /// User has explicitly opted in. Full feature set available.
    Enabled,
}

impl Default for ConsentState {
    fn default() -> Self { ConsentState::Disabled }
}

impl ConsentState {
    /// Convenience: is this state permissive?
    pub fn is_enabled(self) -> bool { matches!(self, ConsentState::Enabled) }
}

/// Trait for the application's consent storage. Different SnapClone
/// builds may persist consent state differently (QSettings on
/// desktop, Android keystore-backed prefs on mobile, etc.), so we
/// don't bake a storage choice into this crate.
pub trait ConsentStore {
    /// Read the current consent state.
    fn read(&self) -> ConsentState;
    /// Write the new consent state. Returns the previous state.
    fn write(&mut self, new: ConsentState) -> ConsentState;
}

/// In-memory consent store. Convenient for tests and for use cases
/// where the consent state lives in another data structure (e.g.
/// a settings object) and the store wrapper is just for trait
/// conformance.
pub struct InMemoryConsentStore {
    state: ConsentState,
}

impl InMemoryConsentStore {
    /// Construct with a starting state. Use `Default::default()` if
    /// you want the conservative "Disabled" starting point.
    pub fn new(state: ConsentState) -> Self { Self { state } }
}

impl Default for InMemoryConsentStore {
    fn default() -> Self { Self { state: ConsentState::default() } }
}

impl ConsentStore for InMemoryConsentStore {
    fn read(&self) -> ConsentState { self.state }
    fn write(&mut self, new: ConsentState) -> ConsentState {
        std::mem::replace(&mut self.state, new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        assert_eq!(ConsentState::default(), ConsentState::Disabled);
        assert!(!ConsentState::default().is_enabled());
    }

    #[test]
    fn in_memory_store_roundtrip() {
        let mut s = InMemoryConsentStore::default();
        assert_eq!(s.read(), ConsentState::Disabled);
        let prev = s.write(ConsentState::Enabled);
        assert_eq!(prev, ConsentState::Disabled);
        assert_eq!(s.read(), ConsentState::Enabled);
    }
}
