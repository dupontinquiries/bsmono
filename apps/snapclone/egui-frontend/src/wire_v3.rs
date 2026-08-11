//! wire_v3 — `SnapRouter`'s unified state-sync protocol.
//!
//! The port has had a send *state machine* since last commit and no wire under
//! it. This is the wire: 1,447 lines of C++ whose interesting part is a design
//! decision stated in its own header.
//!
//! ## Flags instead of message types, and what that buys
//!
//! ```text
//!   F_ANNOUNCE  metadata only          F_ACK   piggyback acknowledgment
//!   F_DATA      includes payload       F_SYNC  state reconciliation
//!   F_DELTA     payload is a patch     F_HINT  predictive hint
//! ```
//!
//! *"No HAVE/NEED: sender predicts and includes DATA immediately. No standalone
//! ACK: every message carries ack number (QUIC-style)."*
//!
//! Both halves remove a **round trip**, which on a phone radio is worth more
//! than the bytes: a HAVE/NEED exchange is two wakeups before any payload
//! moves, and `wake_kit` already measures what a wakeup costs. A type enum
//! could not express `ANNOUNCE|DATA|ACK` in one frame; flags can, and that
//! combination is the common case rather than an optimisation.
//!
//! ## The combinations that are meaningless must be refusable
//!
//! Flags are more expressive than an enum and therefore admit nonsense —
//! `F_DELTA` without `F_DATA` is a patch with nothing to patch, and `F_DELTA`
//! with no base hash is a patch against nothing. A parser that accepted those
//! would apply a delta to whatever it had lying around.
//!
//! ## Version negotiation falls back rather than failing
//!
//! *"v1/v2 remain supported. v3 used when both peers have negotiated it.
//! Fallback to v2 is automatic."* An old peer is not an error — refusing it
//! would split the network on an upgrade.

/// Metadata only: "this is what I have".
pub const F_ANNOUNCE: u8 = 1 << 0;
/// A payload is attached.
pub const F_DATA: u8 = 1 << 1;
/// The payload is a patch against `base_hash`.
pub const F_DELTA: u8 = 1 << 2;
/// Carries an acknowledgement number.
pub const F_ACK: u8 = 1 << 3;
/// State reconciliation: known hashes, gaps inferred.
pub const F_SYNC: u8 = 1 << 4;
/// Predictive hint so the peer can pre-allocate.
pub const F_HINT: u8 = 1 << 5;

/// How long a snap lives locally before auto-expiry, unless kept.
///
/// Thirty days, and the reason is stated: *"so the monthly export"* has
/// something to export. A shorter retention would silently make the export
/// feature produce empty archives.
pub const RETENTION_DAYS_DEFAULT: u32 = 30;

/// Protocol versions this build speaks.
pub const VERSION_MIN: u8 = 1;
pub const VERSION_MAX: u8 = 3;

/// A frame's header, before any payload.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Frame {
    pub version: u8,
    pub flags: u8,
    /// Sequence of this frame.
    pub seq: u64,
    /// Highest sequence seen from the peer — piggybacked, never standalone.
    pub ack: u64,
    /// Set when `F_DELTA`: what the patch applies to.
    pub base_hash: Option<u64>,
    pub payload_len: usize,
}

/// Why a frame was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reject {
    /// A version outside what this build speaks at all.
    UnknownVersion,
    /// `F_DELTA` without `F_DATA` — a patch with nothing to patch.
    DeltaWithoutData,
    /// `F_DELTA` without a base — a patch against nothing.
    DeltaWithoutBase,
    /// `F_DATA` with an empty payload, or the reverse.
    PayloadMismatch,
    /// No flags at all: a frame that means nothing.
    Empty,
}

impl Frame {
    pub fn has(&self, flag: u8) -> bool { self.flags & flag != 0 }

    /// Does this frame carry an acknowledgement?
    ///
    /// Every frame may; none is *only* an ack. That is the round trip the
    /// design removes.
    pub fn acks(&self) -> Option<u64> {
        if self.has(F_ACK) { Some(self.ack) } else { None }
    }

    /// Check the combination before acting on it.
    ///
    /// Flags are more expressive than an enum and admit nonsense; a parser that
    /// accepted a base-less delta would apply a patch to whatever it had.
    pub fn validate(&self) -> Result<(), Reject> {
        if self.version < VERSION_MIN { return Err(Reject::UnknownVersion); }
        if self.flags == 0 { return Err(Reject::Empty); }
        if self.has(F_DELTA) {
            if !self.has(F_DATA) { return Err(Reject::DeltaWithoutData); }
            if self.base_hash.is_none() { return Err(Reject::DeltaWithoutBase); }
        }
        if self.has(F_DATA) && self.payload_len == 0 {
            return Err(Reject::PayloadMismatch);
        }
        if !self.has(F_DATA) && self.payload_len > 0 {
            // Bytes nobody will read. Accepting them means a length field and a
            // flag disagree, and the next frame starts in the wrong place.
            return Err(Reject::PayloadMismatch);
        }
        Ok(())
    }

    /// A short description for a log line.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for (f, n) in [(F_ANNOUNCE, "ANNOUNCE"), (F_DATA, "DATA"), (F_DELTA, "DELTA"),
                       (F_ACK, "ACK"), (F_SYNC, "SYNC"), (F_HINT, "HINT")] {
            if self.has(f) { parts.push(n); }
        }
        format!("v{} {}", self.version, parts.join("|"))
    }
}

/// The version two peers will actually speak.
///
/// The lower of the two ceilings, floored at v1. An old peer is not an error —
/// refusing it would split the network on an upgrade, which is the failure a
/// negotiated protocol exists to avoid.
pub fn negotiate(ours_max: u8, theirs_max: u8) -> Option<u8> {
    let agreed = ours_max.min(theirs_max);
    if agreed < VERSION_MIN { None } else { Some(agreed) }
}

/// May a delta be sent at this version?
///
/// v3 only. Sending one to a v2 peer produces a frame it parses as a full
/// payload — silently corrupt data rather than a protocol error, which is why
/// this is checked at send time rather than hoped for.
pub fn supports_delta(version: u8) -> bool { version >= 3 }

/// What a sender should put in one frame for a snap the peer may not have.
///
/// The predictive rule: announce AND send, rather than announcing and waiting
/// to be asked. `hint` marks it so the peer can pre-allocate.
pub fn predictive_flags(has_payload: bool, is_delta: bool, acking: bool) -> u8 {
    let mut f = F_ANNOUNCE;
    if has_payload { f |= F_DATA | F_HINT; }
    if is_delta { f |= F_DELTA; }
    if acking { f |= F_ACK; }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(flags: u8) -> Frame {
        Frame { version: 3, flags, seq: 1, ack: 0, base_hash: None, payload_len: 0 }
    }

    #[test]
    fn the_flag_values_match_the_c_protocol_exactly() {
        // Two implementations of one wire: a bit that disagrees is a frame the
        // other side reads as a different message entirely.
        assert_eq!(F_ANNOUNCE, 0x01);
        assert_eq!(F_DATA, 0x02);
        assert_eq!(F_DELTA, 0x04);
        assert_eq!(F_ACK, 0x08);
        assert_eq!(F_SYNC, 0x10);
        assert_eq!(F_HINT, 0x20);
    }

    #[test]
    fn one_frame_can_announce_send_and_acknowledge() {
        // The whole point of flags over a type enum: "no HAVE/NEED... no
        // standalone ACK". This combination is the common case, and an enum
        // could not express it in one frame.
        let f = Frame { flags: F_ANNOUNCE | F_DATA | F_ACK, payload_len: 64, ack: 7, ..frame(0) };
        assert!(f.validate().is_ok());
        assert_eq!(f.acks(), Some(7));
        assert!(f.has(F_ANNOUNCE) && f.has(F_DATA));
    }

    #[test]
    fn a_delta_without_data_is_refused() {
        // A patch with nothing to patch. Accepting it would apply a delta to
        // whatever the receiver had lying around.
        let f = Frame { flags: F_ANNOUNCE | F_DELTA, base_hash: Some(1), ..frame(0) };
        assert_eq!(f.validate(), Err(Reject::DeltaWithoutData));
    }

    #[test]
    fn a_delta_without_a_base_is_refused() {
        // A patch against nothing.
        let f = Frame { flags: F_DATA | F_DELTA, payload_len: 32, base_hash: None, ..frame(0) };
        assert_eq!(f.validate(), Err(Reject::DeltaWithoutBase));
    }

    #[test]
    fn a_valid_delta_passes() {
        let f = Frame { flags: F_DATA | F_DELTA, payload_len: 32, base_hash: Some(0xabc), ..frame(0) };
        assert!(f.validate().is_ok());
    }

    #[test]
    fn a_length_that_disagrees_with_the_flags_is_refused_both_ways() {
        // If a length field and a flag disagree, the NEXT frame starts in the
        // wrong place — a stream desync rather than one bad message.
        let claims = Frame { flags: F_DATA, payload_len: 0, ..frame(0) };
        assert_eq!(claims.validate(), Err(Reject::PayloadMismatch));
        let sneaks = Frame { flags: F_ANNOUNCE, payload_len: 100, ..frame(0) };
        assert_eq!(sneaks.validate(), Err(Reject::PayloadMismatch));
    }

    #[test]
    fn a_frame_with_no_flags_means_nothing_and_is_refused() {
        assert_eq!(frame(0).validate(), Err(Reject::Empty));
    }

    #[test]
    fn an_announce_alone_is_valid() {
        // Metadata only, no payload — the sync path.
        assert!(frame(F_ANNOUNCE | F_SYNC).validate().is_ok());
    }

    #[test]
    fn negotiation_takes_the_lower_ceiling_rather_than_failing() {
        // "Fallback to v2 is automatic." An old peer is not an error; refusing
        // it would split the network on an upgrade.
        assert_eq!(negotiate(3, 2), Some(2));
        assert_eq!(negotiate(2, 3), Some(2));
        assert_eq!(negotiate(3, 3), Some(3));
        assert_eq!(negotiate(3, 1), Some(1));
    }

    #[test]
    fn a_peer_speaking_nothing_we_speak_is_none() {
        // Distinguishable from a successful downgrade, so the caller can say
        // "too old" rather than silently sending frames nobody parses.
        assert_eq!(negotiate(3, 0), None);
    }

    #[test]
    fn a_delta_is_never_sent_to_a_peer_that_cannot_parse_it() {
        // A v2 peer reads a delta frame as a FULL payload — silently corrupt
        // data rather than a protocol error, which is why this is checked at
        // send time rather than hoped for.
        assert!(supports_delta(3));
        assert!(!supports_delta(2));
        assert!(!supports_delta(1));
    }

    #[test]
    fn the_predictive_send_announces_and_sends_in_one_frame() {
        // "Sender predicts and includes DATA immediately" — the round trip the
        // design removes, and on a phone radio a saved round trip is a saved
        // wakeup, which wake-kit already prices.
        let f = predictive_flags(true, false, true);
        assert!(f & F_ANNOUNCE != 0 && f & F_DATA != 0 && f & F_ACK != 0);
        assert!(f & F_HINT != 0, "the peer cannot pre-allocate without the hint");
        assert!(f & F_DELTA == 0);
    }

    #[test]
    fn a_metadata_only_prediction_carries_no_hint() {
        // HINT means "make room for what follows"; sending it with no payload
        // makes the peer allocate for nothing.
        let f = predictive_flags(false, false, false);
        assert_eq!(f, F_ANNOUNCE);
    }

    #[test]
    fn every_flag_combination_describes_itself_for_a_log() {
        // A wire bug is diagnosed from logs, and "flags=0x2b" is not a
        // diagnosis.
        let f = Frame { flags: F_ANNOUNCE | F_DATA | F_ACK, payload_len: 8, ..frame(0) };
        let d = f.describe();
        assert!(d.contains("ANNOUNCE") && d.contains("DATA") && d.contains("ACK"), "{d}");
        assert!(d.starts_with("v3"));
    }

    #[test]
    fn retention_defaults_to_the_month_the_export_needs() {
        // "Defaults to 30 days so the monthly export" has something to export.
        // A shorter retention silently makes that feature produce empty
        // archives.
        assert_eq!(RETENTION_DAYS_DEFAULT, 30);
    }
}
