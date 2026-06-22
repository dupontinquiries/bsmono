//! snapclone-vcard — vCard 3.0 serialiser and parser.
//!
//! Rust port of VCardIO.cpp. Fully self-contained — no Qt, no network deps.
//! Reusable by any Rust project that needs to read/write vCard contact files.
//!
//! # Supported fields
//!
//! | vCard field         | Rust field          |
//! |---------------------|---------------------|
//! | FN / N              | `display_name`      |
//! | NICKNAME            | `nickname`          |
//! | NOTE                | `notes`             |
//! | UID                 | `uid`               |
//! | REV                 | `rev`               |
//! | X-SNAPCLONE-ID      | `sc_id`             |
//! | TEL                 | `phone`             |
//! | EMAIL               | `email`             |
//! | ADR                 | `address`           |
//! | PHOTO;ENCODING=BASE64 | `photo_b64`       |
//!
//! # Usage
//!
//! ```rust
//! use snapclone_vcard::{VCard, write_vcard, parse_vcard};
//!
//! let card = VCard {
//!     display_name: "Alice".to_owned(),
//!     sc_id:        Some("sc-deadbeef".to_owned()),
//!     ..Default::default()
//! };
//! let text = write_vcard(&card);
//! let parsed = parse_vcard(&text).unwrap();
//! assert_eq!(parsed.display_name, "Alice");
//! ```

pub mod escape;
pub mod parse;
pub mod write;

pub use parse::parse_vcard;
pub use write::write_vcard;

/// A contact represented as a vCard.
/// All fields are optional except `display_name`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VCard {
    pub display_name: String,
    pub nickname:     String,
    pub notes:        String,
    /// Unique identifier (UUID string). Generated if empty on write.
    pub uid:          String,
    /// Last-revised timestamp in ISO 8601 format.
    pub rev:          Option<String>,
    /// SnapClone stable sc-ID (stored as X-SNAPCLONE-ID).
    pub sc_id:        Option<String>,
    pub phone:        Vec<String>,
    pub email:        Vec<String>,
    pub address:      Option<String>,
    /// Base64-encoded JPEG/PNG photo data.
    pub photo_b64:    Option<String>,
}
