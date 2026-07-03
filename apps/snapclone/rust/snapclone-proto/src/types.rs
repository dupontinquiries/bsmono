//! Shared types used across the proto crate.
//!
//! Wire-protocol-stable enums and flag constants. Numeric values must match
//! the C++ side (see `include/core/Types.hpp` for `MediaType`).

/// Snap media type — matches the C++ MediaType enum and DB encoding.
///
/// Numeric layout is part of the wire protocol — never renumber existing
/// values. New variants always go at the end with a fresh discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MediaType {
    Image    = 0x00,
    Video    = 0x01,
    Text     = 0x02,
    Link     = 0x03,
    Location = 0x04,
}

impl MediaType {
    /// Decode a u8 from the wire. Unknown values fall back to Image (legacy default).
    pub fn from_u8(b: u8) -> Self {
        match b {
            0x00 => Self::Image,
            0x01 => Self::Video,
            0x02 => Self::Text,
            0x03 => Self::Link,
            0x04 => Self::Location,
            _    => Self::Image, // forward-compat: unknown → treat as image
        }
    }

    /// True if this media type carries a binary blob in encryptedBlob.
    /// (Image/Video carry image bytes; Text caption is in caption field;
    ///  Link/Location are metadata-only.)
    pub fn has_blob_payload(self) -> bool {
        matches!(self, Self::Image | Self::Video | Self::Text)
    }

    /// True if this snap should be displayed with a view-once timer.
    /// Location and Link snaps stay readable until expiry, like persistent posts.
    pub fn is_ephemeral(self) -> bool {
        matches!(self, Self::Image | Self::Video | Self::Text)
    }
}

/// v3 frame flags — match F_* constants in SnapRouter.cpp.
///
/// Flag bits are reserved for transport behaviour (delivery receipts, deltas).
/// Media-type information is carried separately in the typed metadata blob,
/// not in flag bits, since flag space is tight (single byte).
pub mod flags {
    pub const ANNOUNCE: u8 = 0x01; // OFFER: snap metadata
    pub const DATA:     u8 = 0x02; // full encrypted blob
    pub const ACK:      u8 = 0x04; // request delivery receipt
    pub const SYNC:     u8 = 0x08; // peer sync / missed-snap request
    pub const NEED:     u8 = 0x10; // request re-send of a specific snap
    pub const DELTA:    u8 = 0x20; // data is a binary delta (future)
    pub const VIDEO:    u8 = 0x40; // legacy: snap is a video (use MediaType instead)
    pub const HAS_LOC:  u8 = 0x80; // snap carries location metadata
}
