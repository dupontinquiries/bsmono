//! labels.rs — centralized, versioned HKDF domain-separation labels.
//!
//! §3(2) of the design: every HKDF `info` must be distinct and versioned. A
//! label collision across contexts is a real vulnerability (the same input key
//! material producing the same output for two different purposes), so all
//! labels live here, in one place, where a reviewer can see they're unique.
//!
//! The `v1` prefix lets us introduce a `v2` scheme later without ambiguity.

/// Sender-key chain ratchet step: `CK_{i+1} = HKDF(CK_i, RATCHET)`.
pub const RATCHET: &[u8] = b"snapclone-v1-group-ratchet";

/// Per-message key derivation: `MK_i = HKDF(CK_i, MSG_KEY)`.
pub const MSG_KEY: &[u8] = b"snapclone-v1-group-msg";

/// Sealing a sender key to a single recipient over the 1:1 X25519 secret:
/// `seal_key = HKDF(x25519_shared, SENDERKEY_SEAL)`.
pub const SENDERKEY_SEAL: &[u8] = b"snapclone-v1-group-senderkey-seal";

/// Deriving the OPEN content key of a *public* ephemeral group directly from
/// its published group_id (public groups have no confidentiality — see §4.3).
pub const PUBLIC_GROUP_KEY: &[u8] = b"snapclone-v1-public-group-key";
