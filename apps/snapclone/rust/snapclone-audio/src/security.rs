//! security.rs — two-tier voice security selection + negotiation.
//!
//! Tier 1 (Public): the default. Lightweight ChaCha20-Poly1305 per frame, no
//! obfuscation, lowest latency + smallest packets. The assumption is that voice
//! on a public/relayed network can be observed regardless, so we spend the
//! minimum needed for confidentiality + integrity.
//!
//! Tier 2 (Private): opt-in, and engaged ONLY when BOTH peers request it (e.g.
//! both on a trusted LAN, or both flip the Settings switch). It layers a
//! post-quantum hybrid key exchange (the crypto layer, via ml-kem) and playback
//! obfuscation (see obfuscate.rs) on top of the Tier-1 AEAD.
//!
//! Mutual consent: requesting private does not by itself make the channel
//! private. Both sides must request, AND the PQ handshake must complete. Until
//! it does, voice still flows over the (encrypted) public tier — never in the
//! clear — and the UI can show an "upgrading" state.

use std::os::raw::c_int;

/// The active security tier for a voice channel.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceSecurityMode {
    /// Lightweight AEAD, no obfuscation (default).
    Public = 0,
    /// Post-quantum hybrid + playback obfuscation (mutually requested + handshaked).
    Private = 1,
}

/// Tracks both peers' preferences and the handshake state, and derives the
/// active mode. Deterministic; no I/O.
#[derive(Debug, Clone, Copy)]
pub struct SecurityNegotiation {
    local_request: bool,
    remote_request: Option<bool>, // None until the peer advertises
    pq_ready: bool,               // hybrid handshake completed
}

impl SecurityNegotiation {
    /// Start with our own preference; the peer's is unknown until advertised.
    pub fn new(local_request: bool) -> Self {
        Self { local_request, remote_request: None, pq_ready: false }
    }

    /// Record the peer's advertised preference.
    pub fn set_remote_request(&mut self, wants_private: bool) {
        self.remote_request = Some(wants_private);
    }

    /// Change our own preference (e.g. the Settings switch toggled mid-call).
    pub fn set_local_request(&mut self, wants_private: bool) {
        self.local_request = wants_private;
        if !wants_private {
            // dropping the request tears down the private upgrade
            self.pq_ready = false;
        }
    }

    /// Mark the PQ hybrid handshake complete (or reset it to false).
    pub fn set_pq_ready(&mut self, ready: bool) {
        self.pq_ready = ready;
    }

    /// Both sides asked for private (independent of whether the handshake is done).
    pub fn private_agreed(&self) -> bool {
        self.local_request && self.remote_request == Some(true)
    }

    /// True while private is agreed but the PQ handshake hasn't finished — voice
    /// flows over the public tier in the meantime; the UI can show "securing…".
    pub fn awaiting_pq(&self) -> bool {
        self.private_agreed() && !self.pq_ready
    }

    /// The mode to actually use right now.
    pub fn active_mode(&self) -> VoiceSecurityMode {
        if self.private_agreed() && self.pq_ready {
            VoiceSecurityMode::Private
        } else {
            VoiceSecurityMode::Public
        }
    }

    /// Whether playback obfuscation should run (private + handshaked).
    pub fn obfuscation_active(&self) -> bool {
        self.active_mode() == VoiceSecurityMode::Private
    }
}

/// Pure negotiation: the agreed mode given both raw preferences, ignoring
/// handshake state. Private iff both want it.
pub fn negotiate(local_wants_private: bool, remote_wants_private: bool) -> VoiceSecurityMode {
    if local_wants_private && remote_wants_private {
        VoiceSecurityMode::Private
    } else {
        VoiceSecurityMode::Public
    }
}

// ─── FFI ─────────────────────────────────────────────────────────────────────

/// Create a negotiation tracker with our preference. Free with
/// `sc_voice_security_free`.
#[no_mangle]
pub extern "C" fn sc_voice_security_new(local_request: c_int) -> *mut SecurityNegotiation {
    Box::into_raw(Box::new(SecurityNegotiation::new(local_request != 0)))
}

/// # Safety
/// `handle` from `sc_voice_security_new`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_voice_security_free(handle: *mut SecurityNegotiation) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// # Safety
/// `handle` from `sc_voice_security_new`, or null (no-op).
#[no_mangle]
pub unsafe extern "C" fn sc_voice_security_set_remote(handle: *mut SecurityNegotiation, wants_private: c_int) {
    if !handle.is_null() {
        (*handle).set_remote_request(wants_private != 0);
    }
}

/// # Safety
/// `handle` from `sc_voice_security_new`, or null (no-op).
#[no_mangle]
pub unsafe extern "C" fn sc_voice_security_set_local(handle: *mut SecurityNegotiation, wants_private: c_int) {
    if !handle.is_null() {
        (*handle).set_local_request(wants_private != 0);
    }
}

/// # Safety
/// `handle` from `sc_voice_security_new`, or null (no-op).
#[no_mangle]
pub unsafe extern "C" fn sc_voice_security_set_pq_ready(handle: *mut SecurityNegotiation, ready: c_int) {
    if !handle.is_null() {
        (*handle).set_pq_ready(ready != 0);
    }
}

/// Active mode: 0 = Public, 1 = Private. Returns 0 (Public) on a null handle.
///
/// # Safety
/// `handle` from `sc_voice_security_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_voice_security_mode(handle: *const SecurityNegotiation) -> c_int {
    if handle.is_null() {
        return VoiceSecurityMode::Public as c_int;
    }
    (*handle).active_mode() as c_int
}

/// 1 while private is agreed but the handshake is pending, else 0.
///
/// # Safety
/// `handle` from `sc_voice_security_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_voice_security_awaiting_pq(handle: *const SecurityNegotiation) -> c_int {
    if handle.is_null() {
        return 0;
    }
    if (*handle).awaiting_pq() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiate_requires_both() {
        assert_eq!(negotiate(false, false), VoiceSecurityMode::Public);
        assert_eq!(negotiate(true, false), VoiceSecurityMode::Public);
        assert_eq!(negotiate(false, true), VoiceSecurityMode::Public);
        assert_eq!(negotiate(true, true), VoiceSecurityMode::Private);
    }

    #[test]
    fn private_needs_consent_and_handshake() {
        let mut n = SecurityNegotiation::new(true);
        // peer unknown → public
        assert_eq!(n.active_mode(), VoiceSecurityMode::Public);
        assert!(!n.private_agreed());

        // peer also wants private, but handshake not done → public + awaiting
        n.set_remote_request(true);
        assert!(n.private_agreed());
        assert!(n.awaiting_pq());
        assert_eq!(n.active_mode(), VoiceSecurityMode::Public);
        assert!(!n.obfuscation_active());

        // handshake completes → private + obfuscation on
        n.set_pq_ready(true);
        assert!(!n.awaiting_pq());
        assert_eq!(n.active_mode(), VoiceSecurityMode::Private);
        assert!(n.obfuscation_active());
    }

    #[test]
    fn peer_declines_stays_public() {
        let mut n = SecurityNegotiation::new(true);
        n.set_remote_request(false);
        n.set_pq_ready(true); // even if a handshake somehow ran
        assert_eq!(n.active_mode(), VoiceSecurityMode::Public);
        assert!(!n.private_agreed());
    }

    #[test]
    fn dropping_local_request_tears_down() {
        let mut n = SecurityNegotiation::new(true);
        n.set_remote_request(true);
        n.set_pq_ready(true);
        assert_eq!(n.active_mode(), VoiceSecurityMode::Private);
        n.set_local_request(false); // flip the switch off mid-call
        assert_eq!(n.active_mode(), VoiceSecurityMode::Public);
        assert!(!n.private_agreed());
    }

    #[test]
    fn ffi_roundtrip() {
        unsafe {
            let h = sc_voice_security_new(1); // we want private
            assert_eq!(sc_voice_security_mode(h), 0); // public until peer + handshake
            sc_voice_security_set_remote(h, 1);
            assert_eq!(sc_voice_security_awaiting_pq(h), 1);
            assert_eq!(sc_voice_security_mode(h), 0);
            sc_voice_security_set_pq_ready(h, 1);
            assert_eq!(sc_voice_security_mode(h), 1); // private
            assert_eq!(sc_voice_security_awaiting_pq(h), 0);
            sc_voice_security_set_local(h, 0); // toggle off
            assert_eq!(sc_voice_security_mode(h), 0);
            sc_voice_security_free(h);
            // null-safety
            assert_eq!(sc_voice_security_mode(std::ptr::null()), 0);
        }
    }
}
