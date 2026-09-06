//! protocol — chunked file framing over any byte stream, resumable.
//!
//! `audio_dialup.rs` (`shared/peer-handshake`) already does the "old school"
//! half of this idea well: a tiny, real, tested acoustic modem — 4-FSK,
//! Goertzel decode, CRC-checked. Its own header says exactly what it's for:
//! "the data-over-sound carrier for the peer handshake" — a small identity
//! blob, not a file. The numbers back that up: 400 samples/symbol at 8kHz is
//! 50ms/symbol, 2 bits/symbol, so **5 bytes/sec**. A 1MB photo would take
//! over two days. That's not a tuning problem, it's the audio band's ceiling.
//!
//! So this crate is the OTHER half: once two devices have exchanged trust
//! acoustically (or however), the actual bytes move over Bluetooth, which
//! has three-to-six orders of magnitude more bandwidth than sound. This file
//! is deliberately independent of *how* those bytes move — `rfcomm_io.rs`
//! wires it to a real Bluetooth stream, but nothing here knows bluer exists,
//! matching `nat-kit`'s own reasoning for staying socket-free: any transport
//! in the fleet can use this without linking a Bluetooth stack.
//!
//! ## Why resumability isn't optional here
//!
//! A rugged/nomad device is the one context where "walked out of range
//! mid-transfer" is the common case, not the edge case. `Reassembler` tracks
//! which chunks actually arrived (not just "how many"), so a dropped and
//! resumed connection asks for exactly what's missing — not a doomed retry
//! from byte zero on every Bluetooth hiccup.

use sha2::{Digest, Sha256};

/// Sized for RFCOMM's typical MTU (~1013 bytes on BR/EDR after L2CAP/RFCOMM
/// framing overhead) without forcing bluer to fragment further — one chunk
/// fits in one underlying packet in the common case. A future different
/// transport can chunk differently; nothing here assumes this exact number.
pub const DEFAULT_CHUNK_SIZE: u32 = 900;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub filename: String,
    pub size: u64,
    pub sha256: [u8; 32],
    pub chunk_size: u32,
}

impl Manifest {
    /// `filename` is sent as-is and displayed as-is by the receiver — never
    /// used as a filesystem path component without sanitizing first. This
    /// module has no opinion on where a file lands on disk; that's the
    /// receiver's problem, and treating an attacker-controlled string as a
    /// path is exactly how "receive a file" becomes "write outside the
    /// intended directory".
    pub fn for_file(filename: String, data: &[u8], chunk_size: u32) -> Manifest {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let sha256: [u8; 32] = hasher.finalize().into();
        Manifest { filename, size: data.len() as u64, sha256, chunk_size }
    }

    pub fn chunk_count(&self) -> u32 {
        if self.size == 0 {
            return 0;
        }
        ((self.size + self.chunk_size as u64 - 1) / self.chunk_size as u64) as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub seq: u32,
    pub data: Vec<u8>,
}

/// One framed unit on the wire. `ResumeFrom` is sent by the RECEIVER back to
/// the sender — the one frame type that flows the opposite direction — so a
/// reconnect can skip everything already landed instead of restarting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Manifest(Manifest),
    Chunk(Chunk),
    ResumeFrom(u32),
    Complete,
}

const TAG_MANIFEST: u8 = 1;
const TAG_CHUNK: u8 = 2;
const TAG_RESUME_FROM: u8 = 3;
const TAG_COMPLETE: u8 = 4;

/// Encodes one frame as `[tag:1][len:4 LE][payload]`. The length prefix is
/// what makes `FrameReader` able to pull complete frames out of a byte
/// stream that delivers in arbitrary-sized pieces — RFCOMM is a stream, not
/// a message transport, so "one write = one frame" is not a guarantee
/// anything downstream gets to rely on.
pub fn encode_frame(frame: &Frame) -> Vec<u8> {
    let mut payload = Vec::new();
    let tag = match frame {
        Frame::Manifest(m) => {
            let name_bytes = m.filename.as_bytes();
            payload.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            payload.extend_from_slice(name_bytes);
            payload.extend_from_slice(&m.size.to_le_bytes());
            payload.extend_from_slice(&m.sha256);
            payload.extend_from_slice(&m.chunk_size.to_le_bytes());
            TAG_MANIFEST
        }
        Frame::Chunk(c) => {
            payload.extend_from_slice(&c.seq.to_le_bytes());
            payload.extend_from_slice(&c.data);
            TAG_CHUNK
        }
        Frame::ResumeFrom(seq) => {
            payload.extend_from_slice(&seq.to_le_bytes());
            TAG_RESUME_FROM
        }
        Frame::Complete => TAG_COMPLETE,
    };
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(tag);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

fn decode_body(tag: u8, body: &[u8]) -> Option<Frame> {
    match tag {
        TAG_MANIFEST => {
            let name_len = u16::from_le_bytes(body.get(0..2)?.try_into().ok()?) as usize;
            let mut i = 2;
            let filename = String::from_utf8(body.get(i..i + name_len)?.to_vec()).ok()?;
            i += name_len;
            let size = u64::from_le_bytes(body.get(i..i + 8)?.try_into().ok()?);
            i += 8;
            let sha256: [u8; 32] = body.get(i..i + 32)?.try_into().ok()?;
            i += 32;
            let chunk_size = u32::from_le_bytes(body.get(i..i + 4)?.try_into().ok()?);
            Some(Frame::Manifest(Manifest { filename, size, sha256, chunk_size }))
        }
        TAG_CHUNK => {
            let seq = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
            Some(Frame::Chunk(Chunk { seq, data: body.get(4..)?.to_vec() }))
        }
        TAG_RESUME_FROM => {
            let seq = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
            Some(Frame::ResumeFrom(seq))
        }
        TAG_COMPLETE => Some(Frame::Complete),
        _ => None,
    }
}

/// Accumulates bytes from a stream and yields complete frames as they
/// become available. Feed it whatever a socket read returns, however it's
/// chopped up — a partial frame just waits for the rest on the next `feed`.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        FrameReader::default()
    }

    /// Appends bytes and returns every frame that became complete as a
    /// result. Frames are returned in arrival order; a corrupt length
    /// prefix (implausibly large) drops the reader into a permanently
    /// empty state rather than trying to allocate an attacker-chosen
    /// amount of memory waiting for bytes that will never arrive.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 5 {
                break;
            }
            let tag = self.buf[0];
            let len = u32::from_le_bytes(self.buf[1..5].try_into().unwrap()) as usize;
            // A chunk is header + up to chunk_size payload; nothing on this
            // wire is ever meant to be larger than a few KB. Anything
            // claiming more is either corruption or hostile input — either
            // way, refusing to buffer for it is the right response for a
            // stream with no length negotiation of its own.
            const MAX_FRAME_LEN: usize = 1 << 20; // 1 MiB, generously above any real chunk
            if len > MAX_FRAME_LEN {
                self.buf.clear();
                break;
            }
            if self.buf.len() < 5 + len {
                break;
            }
            let body = self.buf[5..5 + len].to_vec();
            self.buf.drain(0..5 + len);
            if let Some(frame) = decode_body(tag, &body) {
                out.push(frame);
            }
            // An undecodable body (unknown tag, truncated-looking innards
            // despite a plausible length) is dropped silently rather than
            // treated as a stream-framing error — the length prefix already
            // told us exactly where the NEXT frame starts, so one bad frame
            // doesn't have to desync everything after it.
        }
        out
    }
}

/// Splits a file's bytes into `Chunk`s of `chunk_size`, in order, starting
/// from sequence 0. Pure — callers doing a resume filter this down to
/// what's actually missing rather than calling a different function; there
/// is only one way chunks are numbered.
pub fn chunks_for(data: &[u8], chunk_size: u32) -> Vec<Chunk> {
    let chunk_size = chunk_size.max(1) as usize;
    data.chunks(chunk_size)
        .enumerate()
        .map(|(i, d)| Chunk { seq: i as u32, data: d.to_vec() })
        .collect()
}

/// Reassembles chunks into a file, tracking exactly which sequence numbers
/// have landed so a resume can ask for precisely what's missing rather than
/// "everything after however-much-I-think-I-got".
pub struct Reassembler {
    manifest: Manifest,
    // Indexed by seq; `None` until that chunk arrives. A Vec, not a HashMap,
    // because seq is dense (0..chunk_count) and this is simpler and cheaper
    // for that shape than a lookup structure earning nothing here.
    slots: Vec<Option<Vec<u8>>>,
}

impl Reassembler {
    pub fn new(manifest: Manifest) -> Self {
        let n = manifest.chunk_count() as usize;
        Reassembler { manifest, slots: vec![None; n] }
    }

    /// Accepts a chunk. Out-of-range or duplicate sequence numbers are
    /// ignored rather than erroring — a resumed sender may legitimately
    /// re-send a chunk the receiver already has (it doesn't know exactly
    /// what arrived before the drop, only what it was asked to resume
    /// from), and that must be a no-op, not a protocol violation.
    pub fn accept(&mut self, chunk: Chunk) {
        if let Some(slot) = self.slots.get_mut(chunk.seq as usize) {
            *slot = Some(chunk.data);
        }
    }

    pub fn is_complete(&self) -> bool {
        self.slots.iter().all(Option::is_some)
    }

    /// The sequence number to resume from: the first missing slot, or past
    /// the end if nothing is missing. A sender resumes by re-chunking from
    /// this index — not by trusting the receiver's byte count, since a
    /// receiver's disk write could itself have failed partway.
    pub fn resume_from(&self) -> u32 {
        self.slots.iter().position(Option::is_none).map_or(self.slots.len() as u32, |i| i as u32)
    }

    /// Concatenates every slot and checks the result against the
    /// manifest's hash. `None` on incompleteness OR a hash mismatch — the
    /// caller doesn't need to distinguish "still missing pieces" from
    /// "reassembled something that isn't the file" at this layer; either
    /// way, nothing here is safe to write to disk yet.
    pub fn finish(&self) -> Option<Vec<u8>> {
        if !self.is_complete() {
            return None;
        }
        let mut out = Vec::with_capacity(self.manifest.size as usize);
        for slot in &self.slots {
            out.extend_from_slice(slot.as_ref()?);
        }
        let mut hasher = Sha256::new();
        hasher.update(&out);
        let got: [u8; 32] = hasher.finalize().into();
        if got != self.manifest.sha256 {
            return None;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest(data: &[u8], chunk_size: u32) -> Manifest {
        Manifest::for_file("photo.jpg".to_string(), data, chunk_size)
    }

    #[test]
    fn every_frame_kind_round_trips_through_encode_and_a_reader() {
        let manifest = sample_manifest(b"hello bluetooth", 4);
        let frames = vec![
            Frame::Manifest(manifest),
            Frame::Chunk(Chunk { seq: 0, data: b"abcd".to_vec() }),
            Frame::ResumeFrom(3),
            Frame::Complete,
        ];
        let mut reader = FrameReader::new();
        let mut got = Vec::new();
        for f in &frames {
            got.extend(reader.feed(&encode_frame(f)));
        }
        assert_eq!(got, frames);
    }

    #[test]
    fn a_frame_arriving_split_across_many_small_feeds_still_decodes() {
        // The whole reason FrameReader exists rather than "just decode what
        // you read": a real socket read has no obligation to hand back a
        // whole frame, or even a whole header, at once.
        let frame = Frame::Chunk(Chunk { seq: 7, data: vec![9u8; 50] });
        let bytes = encode_frame(&frame);
        let mut reader = FrameReader::new();
        let mut got = Vec::new();
        for byte in &bytes {
            got.extend(reader.feed(std::slice::from_ref(byte)));
        }
        assert_eq!(got, vec![frame]);
    }

    #[test]
    fn two_frames_delivered_in_one_feed_both_come_back() {
        let a = Frame::ResumeFrom(1);
        let b = Frame::ResumeFrom(2);
        let mut bytes = encode_frame(&a);
        bytes.extend(encode_frame(&b));
        let mut reader = FrameReader::new();
        assert_eq!(reader.feed(&bytes), vec![a, b]);
    }

    #[test]
    fn an_absurd_length_prefix_is_dropped_rather_than_allocated_for() {
        let mut evil = vec![TAG_CHUNK];
        evil.extend_from_slice(&(u32::MAX).to_le_bytes());
        evil.extend_from_slice(b"not remotely that many bytes");
        let mut reader = FrameReader::new();
        assert_eq!(reader.feed(&evil), Vec::new(), "no panic, no multi-GB allocation attempt");
    }

    #[test]
    fn chunking_and_full_reassembly_round_trips_a_file() {
        let data: Vec<u8> = (0u32..10_000).map(|i| (i % 251) as u8).collect();
        let manifest = sample_manifest(&data, 900);
        let chunks = chunks_for(&data, manifest.chunk_size);
        assert_eq!(chunks.len() as u32, manifest.chunk_count());

        let mut r = Reassembler::new(manifest);
        for c in chunks {
            r.accept(c);
        }
        assert!(r.is_complete());
        assert_eq!(r.finish(), Some(data));
    }

    #[test]
    fn a_tampered_reassembly_fails_the_hash_rather_than_landing_on_disk() {
        let data = b"the actual file contents".to_vec();
        let manifest = sample_manifest(&data, 8);
        let mut chunks = chunks_for(&data, manifest.chunk_size);
        chunks[0].data[0] ^= 0xFF; // corrupt one byte of one chunk in flight

        let mut r = Reassembler::new(manifest);
        for c in chunks {
            r.accept(c);
        }
        assert!(r.is_complete(), "every slot filled — corruption, not loss");
        assert_eq!(r.finish(), None, "the hash must catch what completeness alone cannot");
    }

    #[test]
    fn resume_from_finds_the_first_gap_not_the_count_of_pieces_held() {
        let data: Vec<u8> = (0u8..=250).collect();
        let manifest = sample_manifest(&data, 25); // 251 bytes / 25 = 11 chunks (0..=10)
        let chunks = chunks_for(&data, manifest.chunk_size);
        let mut r = Reassembler::new(manifest);
        for c in chunks.iter().filter(|c| c.seq != 3).cloned() {
            r.accept(c); // every chunk except #3 — a hole in the middle, not at the end
        }
        assert!(!r.is_complete());
        assert_eq!(r.resume_from(), 3, "the gap, not len()-1 or a count");

        r.accept(chunks[3].clone());
        assert!(r.is_complete());
        assert_eq!(r.resume_from(), chunks.len() as u32);
    }

    #[test]
    fn resending_an_already_held_chunk_on_resume_is_a_harmless_no_op() {
        let data = b"idempotent resume behavior matters here".to_vec();
        let manifest = sample_manifest(&data, 6);
        let chunks = chunks_for(&data, manifest.chunk_size);
        let mut r = Reassembler::new(manifest);
        for c in chunks.iter().cloned() {
            r.accept(c.clone());
            r.accept(c); // the sender re-sends the same chunk again
        }
        assert!(r.is_complete());
        assert_eq!(r.finish(), Some(data));
    }

    #[test]
    fn an_empty_file_has_zero_chunks_and_is_immediately_complete() {
        let manifest = sample_manifest(b"", 900);
        assert_eq!(manifest.chunk_count(), 0);
        let r = Reassembler::new(manifest);
        assert!(r.is_complete(), "vacuously — there is nothing to wait for");
        assert_eq!(r.finish(), Some(Vec::new()));
    }
}
