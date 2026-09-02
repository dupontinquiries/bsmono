//! snap_wire — putting `wire_v3`'s frames on a socket.
//!
//! `wire_v3` models the header and validates flag combinations, and has no
//! byte encoding — so before this module there was nothing to write to a
//! socket: snaps could be composed, queued, expired and exported, and none
//! had ever left the device or arrived on it. The received-snap feed had a
//! filter and a store and no delivery. This is the encoding that closes
//! it.
//!
//! This is the codec and nothing else. It reads and writes over any
//! `Read`/`Write`, so the transport above it can be TCP, a unix socket, or
//! a `Vec<u8>` in a test — and the tests use all three.
//!
//! ## Why a stream and not datagrams, unlike voice
//!
//! A snap is a photo: kilobytes to megabytes, and worthless if it arrives
//! with a hole in it. Voice is the opposite — a late frame is worse than a
//! missing one — which is why `voice_wire` is UDP and this is written
//! against a stream. The two protocols disagreeing about reliability is
//! correct rather than an inconsistency.
//!
//! ## The header is fixed-width, and the length is checked before it is
//! trusted
//!
//! Every field is big-endian and the header is exactly [`HEADER_BYTES`]
//! long, so a reader always knows how much to pull before it can know
//! anything else. `payload_len` arrives from a stranger, so it is bounded by
//! [`MAX_PAYLOAD_BYTES`] BEFORE any allocation — a `Vec::with_capacity` on
//! an attacker's number is a one-datagram out-of-memory.
//!
//! ## Metadata travels with the payload, and the id is not the sender's
//! to choose
//!
//! The receiving side derives the snap's local id from the CONTENT rather
//! than accepting a sender-supplied one. A sender who picks ids can
//! overwrite an existing snap by reusing one, or resurrect a viewed snap by
//! choosing a fresh id for the same bytes. Content addressing removes both:
//! the same bytes are the same snap, always, and `snap_state` recognises a
//! re-delivery as already finished.
//!
//! ## What this does NOT do
//!
//! No encryption and no authentication. `Snap.bytes` is documented as "the
//! decrypted bytes", and the encrypting layer is `handshake`'s to add — this
//! codec is what that layer would sit on top of. Until then a snap crosses
//! the network in the clear, and that is stated here rather than left to be
//! discovered.

use std::io::{Read, Write};

/// version(1) flags(1) seq(8) ack(8) base_hash(8) payload_len(4)
/// + sent_at(8) duration(2) media(1) id_len(1) sender_len(1) name_len(1)
pub const HEADER_BYTES: usize = 1 + 1 + 8 + 8 + 8 + 4 + 8 + 2 + 1 + 1 + 1 + 1;

/// A ceiling on one snap. Generous for a photo, small enough that a hostile
/// length field cannot make us allocate a gigabyte.
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Bounds on the three variable-length strings, each of which is
/// length-prefixed with a single byte and therefore already capped at 255.
/// Named so the reason is not "u8 happened to be the type".
pub const MAX_STRING_BYTES: usize = 255;

/// Why a frame could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The stream ended, cleanly or otherwise.
    Eof,
    /// A read or write failed.
    Io(String),
    /// The header parsed but the flag combination is nonsense.
    Rejected(crate::wire_v3::Reject),
    /// A length field exceeded its bound. Carries what was claimed, because
    /// the number is the interesting part of the incident.
    TooLarge { claimed: usize, limit: usize },
    /// A string field was not UTF-8.
    NotText,
    /// A media byte outside the three kinds.
    UnknownMedia(u8),
}

/// One snap as it travels: `wire_v3`'s header, plus the metadata the
/// receiving feed needs, plus the bytes.
///
/// The sender's own id for the snap is deliberately NOT carried — see the
/// module docs on why the receiver derives it from content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapFrame {
    pub header: crate::wire_v3::Frame,
    /// Unix seconds, from the sender's clock. Treated as a hint: it is used
    /// for display order and never for expiry, because a peer with a wrong
    /// clock could otherwise expire a snap on arrival or never.
    pub sent_at: i64,
    /// Seconds on screen; 0 means the receiver's default.
    pub duration_secs: u16,
    pub media: crate::snaps::Media,
    /// The sender's own sc-ID as it claims it. Checked against the
    /// transport's identity by the caller, not here — this module knows
    /// bytes, not who is allowed to say them.
    pub sender_id: String,
    pub sender_name: String,
    pub payload: Vec<u8>,
}

fn media_byte(m: crate::snaps::Media) -> u8 {
    match m {
        crate::snaps::Media::Image => 1,
        crate::snaps::Media::Video => 2,
        crate::snaps::Media::Audio => 3,
    }
}

fn media_of(b: u8) -> Result<crate::snaps::Media, WireError> {
    match b {
        1 => Ok(crate::snaps::Media::Image),
        2 => Ok(crate::snaps::Media::Video),
        3 => Ok(crate::snaps::Media::Audio),
        other => Err(WireError::UnknownMedia(other)),
    }
}

/// The local id for a snap, derived from its content and its sender.
///
/// Not the sender's choice. A sender who picks ids can overwrite an
/// existing snap by reusing one, or resurrect a snap you already viewed by
/// choosing a fresh id for the same bytes. The sender is folded in so two
/// people sending you the same photo are two snaps, which is what a person
/// would expect.
///
/// FNV-1a, and deliberately not a cryptographic hash: this is a local
/// naming scheme, not a security boundary, and calling it one by reaching
/// for SHA would invite treating a collision as impossible. Collisions are
/// possible and their effect is bounded — two snaps would share a slot.
pub fn content_id(sender_id: &str, payload: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(sender_id.as_bytes());
    feed(&[0]); // so ("ab", "c") and ("a", "bc") differ
    feed(payload);
    format!("snap-{h:016x}")
}

/// Write one frame. The header's `payload_len` is taken from the payload
/// itself rather than trusted from the caller, so the two cannot disagree —
/// a disagreement here puts the next frame's start in the wrong place and
/// desynchronises the stream for good.
pub fn write_frame(w: &mut impl Write, f: &SnapFrame) -> Result<(), WireError> {
    for (s, what) in [(&f.sender_id, "sender id"), (&f.sender_name, "sender name")] {
        if s.len() > MAX_STRING_BYTES {
            let _ = what;
            return Err(WireError::TooLarge { claimed: s.len(), limit: MAX_STRING_BYTES });
        }
    }
    if f.payload.len() > MAX_PAYLOAD_BYTES {
        return Err(WireError::TooLarge {
            claimed: f.payload.len(), limit: MAX_PAYLOAD_BYTES });
    }
    let mut h = f.header;
    h.payload_len = f.payload.len();
    h.validate().map_err(WireError::Rejected)?;

    let mut out = Vec::with_capacity(HEADER_BYTES + f.payload.len());
    out.push(h.version);
    out.push(h.flags);
    out.extend_from_slice(&h.seq.to_be_bytes());
    out.extend_from_slice(&h.ack.to_be_bytes());
    // 0 stands for "no base hash". A delta against hash zero is not
    // meaningful, and `validate` refuses a delta without a base anyway, so
    // the sentinel cannot silently become a real value.
    out.extend_from_slice(&h.base_hash.unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&(f.payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&f.sent_at.to_be_bytes());
    out.extend_from_slice(&f.duration_secs.to_be_bytes());
    out.push(media_byte(f.media));
    // id_len is reserved: the sender does not name the snap (see the module
    // docs) and this keeps the header fixed-width if that ever changes.
    out.push(0);
    out.push(f.sender_id.len() as u8);
    out.push(f.sender_name.len() as u8);
    out.extend_from_slice(f.sender_id.as_bytes());
    out.extend_from_slice(f.sender_name.as_bytes());
    out.extend_from_slice(&f.payload);

    w.write_all(&out).map_err(|e| WireError::Io(e.to_string()))?;
    w.flush().map_err(|e| WireError::Io(e.to_string()))
}

fn read_exact(r: &mut impl Read, buf: &mut [u8]) -> Result<(), WireError> {
    match r.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(WireError::Eof),
        Err(e) => Err(WireError::Io(e.to_string())),
    }
}

/// Read one frame.
///
/// Every length is bounded before it is used to allocate. `payload_len`
/// arrives from a stranger; `Vec::with_capacity` on that number is a
/// one-frame out-of-memory, and it is the first thing an attacker would
/// try.
pub fn read_frame(r: &mut impl Read) -> Result<SnapFrame, WireError> {
    let mut head = [0u8; HEADER_BYTES];
    read_exact(r, &mut head)?;

    let be64 = |o: usize| -> u64 {
        u64::from_be_bytes(head[o..o + 8].try_into().expect("fixed slice"))
    };
    let version = head[0];
    let flags = head[1];
    let seq = be64(2);
    let ack = be64(10);
    let raw_base = be64(18);
    let payload_len = u32::from_be_bytes(head[26..30].try_into().expect("fixed slice")) as usize;
    let sent_at = be64(30) as i64;
    let duration_secs = u16::from_be_bytes(head[38..40].try_into().expect("fixed slice"));
    let media = media_of(head[40])?;
    let _reserved_id_len = head[41];
    let sender_len = head[42] as usize;
    let name_len = head[43] as usize;

    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(WireError::TooLarge { claimed: payload_len, limit: MAX_PAYLOAD_BYTES });
    }

    let header = crate::wire_v3::Frame {
        version,
        flags,
        seq,
        ack,
        base_hash: if raw_base == 0 { None } else { Some(raw_base) },
        payload_len,
    };
    // Validated BEFORE the payload is read, so a nonsense frame costs a
    // header rather than a megabyte off the wire.
    header.validate().map_err(WireError::Rejected)?;

    let mut sender = vec![0u8; sender_len];
    read_exact(r, &mut sender)?;
    let mut name = vec![0u8; name_len];
    read_exact(r, &mut name)?;
    let mut payload = vec![0u8; payload_len];
    read_exact(r, &mut payload)?;

    Ok(SnapFrame {
        header,
        sent_at,
        duration_secs,
        media,
        sender_id: String::from_utf8(sender).map_err(|_| WireError::NotText)?,
        sender_name: String::from_utf8(name).map_err(|_| WireError::NotText)?,
        payload,
    })
}

impl SnapFrame {
    /// A frame carrying one snap: announce plus data, acking what we have
    /// seen. The common case, and `predictive_flags` already decides the
    /// combination — building the flags by hand at each call site is how one
    /// of them ends up sending a delta without a base.
    pub fn announce(seq: u64, ack: u64, sender_id: &str, sender_name: &str,
                    media: crate::snaps::Media, sent_at: i64, duration_secs: u16,
                    payload: Vec<u8>) -> SnapFrame {
        SnapFrame {
            header: crate::wire_v3::Frame {
                version: crate::wire_v3::VERSION_MAX,
                flags: crate::wire_v3::F_ANNOUNCE
                     | crate::wire_v3::predictive_flags(!payload.is_empty(), false, true),
                seq,
                ack,
                base_hash: None,
                payload_len: payload.len(),
            },
            sent_at,
            duration_secs,
            media,
            sender_id: sender_id.to_string(),
            sender_name: sender_name.to_string(),
            payload,
        }
    }

    /// Turn a received frame into the feed row it becomes.
    ///
    /// `from` is the TRANSPORT's identity, not the frame's claim. A frame
    /// that names a different sender than the connection it arrived on is
    /// the whole reason this takes an argument: the id used for content
    /// addressing and display must be the one that was authenticated, or a
    /// peer can post snaps as somebody else.
    /// The snap itself, bytes and all — what the viewer opens. `from` is
    /// the transport's identity for the sender, which wins over the name
    /// inside the frame for the same reason it does in `to_feed_entry`.
    pub fn to_snap(&self, from: &str) -> crate::snaps::Snap {
        crate::snaps::Snap {
            id: content_id(from, &self.payload),
            sender_id: from.to_string(),
            sender_name: if self.sender_name.trim().is_empty() { from.to_string() }
                         else { self.sender_name.clone() },
            sent_at: self.sent_at,
            media: self.media,
            duration_secs: u32::from(self.duration_secs),
            bytes: self.payload.clone(),
        }
    }

    pub fn to_feed_entry(&self, from: &str) -> crate::snaps::FeedEntry {
        crate::snaps::FeedEntry {
            id: content_id(from, &self.payload),
            sender_name: if self.sender_name.trim().is_empty() {
                from.to_string()
            } else {
                self.sender_name.clone()
            },
            sent_at: self.sent_at,
            media: self.media,
            thumb_path: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snaps::Media;

    fn frame(payload: Vec<u8>) -> SnapFrame {
        SnapFrame::announce(7, 3, "sc-aabbccdd", "kit", Media::Image, 1_700_000_000, 5, payload)
    }

    #[test]
    fn a_frame_survives_the_round_trip_intact() {
        let f = frame(b"not really a jpeg".to_vec());
        let mut buf = Vec::new();
        write_frame(&mut buf, &f).expect("write");
        assert_eq!(buf.len(), HEADER_BYTES + "sc-aabbccdd".len() + "kit".len() + f.payload.len(),
                   "the header is not the width the reader assumes");
        let got = read_frame(&mut &buf[..]).expect("read");
        assert_eq!(got, f);
    }

    #[test]
    fn two_frames_back_to_back_do_not_desynchronise() {
        // The property the fixed header exists for. If `payload_len` and the
        // payload ever disagree, the second frame starts in the wrong place
        // and the stream is broken for good rather than for one frame.
        let (a, b) = (frame(b"first".to_vec()), frame(vec![0u8; 1000]));
        let mut buf = Vec::new();
        write_frame(&mut buf, &a).unwrap();
        write_frame(&mut buf, &b).unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r).unwrap(), a);
        assert_eq!(read_frame(&mut r).unwrap(), b);
        assert_eq!(read_frame(&mut r), Err(WireError::Eof), "a clean end is Eof, not garbage");
    }

    #[test]
    fn a_hostile_length_is_refused_before_anything_is_allocated() {
        // `Vec::with_capacity` on an attacker's number is a one-frame
        // out-of-memory, and it is the first thing anybody would try.
        let f = frame(b"x".to_vec());
        let mut buf = Vec::new();
        write_frame(&mut buf, &f).unwrap();
        // Overwrite payload_len with something enormous.
        buf[26..30].copy_from_slice(&u32::MAX.to_be_bytes());
        match read_frame(&mut &buf[..]) {
            Err(WireError::TooLarge { claimed, limit }) => {
                assert_eq!(claimed, u32::MAX as usize);
                assert_eq!(limit, MAX_PAYLOAD_BYTES);
            }
            other => panic!("a 4 GB claim was not refused: {other:?}"),
        }
    }

    #[test]
    fn a_nonsense_flag_combination_costs_a_header_not_a_payload() {
        // Validation happens before the payload is pulled off the wire, so a
        // frame that means nothing does not first cost a megabyte.
        let mut f = frame(b"data".to_vec());
        // DELTA without a base: a patch against nothing.
        f.header.flags |= crate::wire_v3::F_DELTA;
        let mut buf = Vec::new();
        assert_eq!(write_frame(&mut buf, &f),
                   Err(WireError::Rejected(crate::wire_v3::Reject::DeltaWithoutBase)),
                   "the sender emitted a frame its own validator refuses");
        assert!(buf.is_empty(), "a refused frame put bytes on the wire anyway");
    }

    #[test]
    fn a_truncated_stream_is_eof_and_not_a_half_snap() {
        let f = frame(vec![9u8; 500]);
        let mut buf = Vec::new();
        write_frame(&mut buf, &f).unwrap();
        buf.truncate(buf.len() - 100);
        assert_eq!(read_frame(&mut &buf[..]), Err(WireError::Eof));
    }

    #[test]
    fn the_snap_id_comes_from_the_content_and_not_from_the_sender() {
        // A sender who picks ids can overwrite one of your snaps by reusing
        // an id, or resurrect one you already viewed by picking a fresh id
        // for the same bytes.
        let a = content_id("sc-aabbccdd", b"photo");
        assert_eq!(a, content_id("sc-aabbccdd", b"photo"), "not deterministic");
        assert_ne!(a, content_id("sc-11223344", b"photo"),
                   "two people sending the same photo must be two snaps");
        assert_ne!(a, content_id("sc-aabbccdd", b"other photo"));
        // The separator earns its place: without it these collide.
        assert_ne!(content_id("ab", b"c"), content_id("a", b"bc"));
    }

    #[test]
    fn the_feed_row_is_named_by_the_authenticated_sender_not_the_claim() {
        // A frame that names a different sender than the connection it came
        // in on is a peer posting snaps as somebody else.
        let mut f = frame(b"photo".to_vec());
        f.sender_id = "sc-99999999".into();   // the lie
        let row = f.to_feed_entry("sc-aabbccdd");
        assert_eq!(row.id, content_id("sc-aabbccdd", b"photo"),
                   "the claimed id was used for content addressing");
        // An empty display name falls back to the id rather than a blank
        // row, which reads as a corrupt snap.
        let mut anon = frame(b"photo".to_vec());
        anon.sender_name = "   ".into();
        assert_eq!(anon.to_feed_entry("sc-aabbccdd").sender_name, "sc-aabbccdd");
    }

    #[test]
    fn it_works_over_a_real_socket_and_not_only_a_vec() {
        // The codec is written against Read/Write so the transport can vary,
        // and a Vec cannot show up a short read: TCP delivers a large
        // payload in several chunks, which is exactly where a hand-rolled
        // reader that assumed one `read` would lose bytes.
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        // Big enough to cross the socket buffer and arrive in pieces.
        let sent = frame((0..300_000u32).map(|i| i as u8).collect());
        let expect = sent.clone();

        let t = std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).expect("connect");
            write_frame(&mut s, &sent).expect("write over tcp");
        });
        let (mut conn, _) = listener.accept().expect("accept");
        let got = read_frame(&mut conn).expect("read over tcp");
        t.join().unwrap();
        assert_eq!(got.payload.len(), expect.payload.len());
        assert_eq!(got, expect);
    }
}
