//! tinypack-kit — fit the most into a tiny fixed byte budget.
//!
//! The constraint that shaped this: an NFC tag holds tens to hundreds of
//! bytes (NTAG213 144 B, NTAG215 504 B, MIFARE Classic 1K 752 B usable), a
//! DHT value a few hundred, a QR code a couple of thousand. Files never fit;
//! the card carries a pointer. But a *short message* or a *thumbnail* can,
//! if the codec has no header and no warm-up — and general-purpose codecs
//! (deflate, zstd without a dictionary) have both.
//!
//! Three layers, each usable alone:
//!
//! * [`text`] — a static-vocabulary token codec. English chat: ~3.5 bits per
//!   byte where raw deflate *expands* the same lines. No header, any bytes.
//! * [`bac`] — an adaptive binary arithmetic coder with a 12-bit probability
//!   model. No tables to transmit; learns from the first symbol.
//! * [`image`] — a headerless 8×8-DCT image codec on top of `bac` that
//!   binary-searches its quantiser to land under a budget, and picks a
//!   resolution by comparing decoded results against a common reference.
//!
//! [`Budget`] lists the tag sizes this was built around. The `tinypack` CLI
//! prints the whole fitting table so the size/quality trade is visible rather
//! than chosen for you.

pub mod bac;
pub mod drop;
pub mod huffman;
pub mod image;
pub mod ledger;
pub mod text;

/// Usable user-memory budgets of common NFC tags, in bytes. These are what
/// is left after the tag's own structures; NDEF framing on top costs a few
/// more bytes (a bare `application/x` record header is ~7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// MIFARE Ultralight (EV1 48-byte variant).
    UltralightEv1 = 48,
    /// NTAG213.
    Ntag213 = 144,
    /// NTAG215 (the amiibo tag).
    Ntag215 = 504,
    /// MIFARE Classic 1K: 16 sectors × 3 data blocks × 16 B, minus sector 0
    /// block 0 (manufacturer) → 47 × 16 = 752.
    Classic1k = 752,
    /// NTAG216.
    Ntag216 = 888,
    /// MIFARE Classic 4K.
    Classic4k = 3440,
}

impl Budget {
    pub const ALL: [Budget; 6] = [
        Budget::UltralightEv1,
        Budget::Ntag213,
        Budget::Ntag215,
        Budget::Classic1k,
        Budget::Ntag216,
        Budget::Classic4k,
    ];
    pub fn bytes(self) -> usize {
        self as usize
    }
    pub fn name(self) -> &'static str {
        match self {
            Budget::UltralightEv1 => "Ultralight EV1",
            Budget::Ntag213 => "NTAG213",
            Budget::Ntag215 => "NTAG215",
            Budget::Classic1k => "Classic 1K",
            Budget::Ntag216 => "NTAG216",
            Budget::Classic4k => "Classic 4K",
        }
    }
}
