//! EBML primitives.
//!
//! Matroska is a tree of elements, each `[id][size][payload]`, where both id
//! and size are variable-length integers with a unary length prefix. Two
//! details cause most of the bugs:
//!
//! - **IDs keep their marker bit, sizes strip theirs.** The id `0xA3`
//!   (SimpleBlock) is stored and compared as the byte `0xA3`; a size byte
//!   `0xA3` means 0x23. Treating them the same way makes every element after
//!   the first land at the wrong offset.
//! - **A size of all-ones means "unknown".** Live-muxed WebM — which is what a
//!   YouTube stream is before it is finalised — writes an unknown-size Segment
//!   and often unknown-size Clusters. An unknown size is not a corrupt file and
//!   must not be read as a huge number.

use vidkit_core::io::{ByteSource, Error, Result};

pub mod id {
    pub const EBML_HEADER: u32 = 0x1A45_DFA3;
    pub const SEGMENT: u32 = 0x1853_8067;
    pub const INFO: u32 = 0x1549_A966;
    pub const TIMESTAMP_SCALE: u32 = 0x2AD7_B1;
    pub const DURATION: u32 = 0x4489;
    pub const TRACKS: u32 = 0x1654_AE6B;
    pub const TRACK_ENTRY: u32 = 0xAE;
    pub const TRACK_NUMBER: u32 = 0xD7;
    pub const TRACK_TYPE: u32 = 0x83;
    pub const CODEC_ID: u32 = 0x86;
    pub const CODEC_PRIVATE: u32 = 0x63A2;
    pub const VIDEO: u32 = 0xE0;
    pub const PIXEL_WIDTH: u32 = 0xB0;
    pub const PIXEL_HEIGHT: u32 = 0xBA;
    pub const CLUSTER: u32 = 0x1F43_B675;
    pub const TIMESTAMP: u32 = 0xE7;
    pub const SIMPLE_BLOCK: u32 = 0xA3;
    pub const BLOCK_GROUP: u32 = 0xA0;
    pub const BLOCK: u32 = 0xA1;
    pub const REFERENCE_BLOCK: u32 = 0xFB;
    pub const CUES: u32 = 0x1C53_BB6B;
}

/// Elements whose payload is a list of child elements. Needed because an
/// unknown-size element can only be terminated by hitting a sibling, and that
/// requires knowing which ids are containers.
pub fn is_container(element_id: u32) -> bool {
    matches!(
        element_id,
        id::SEGMENT
            | id::INFO
            | id::TRACKS
            | id::TRACK_ENTRY
            | id::VIDEO
            | id::CLUSTER
            | id::BLOCK_GROUP
            | id::CUES
    )
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ElementHeader {
    pub id: u32,
    pub payload_offset: u64,
    /// `None` when the size field was all-ones.
    pub payload_len: Option<u64>,
}

impl ElementHeader {
    /// End offset, given a fallback for unknown-size elements.
    pub fn end_or(&self, fallback: u64) -> u64 {
        match self.payload_len {
            Some(n) => self.payload_offset + n,
            None => fallback,
        }
    }
}

/// Number of bytes in a VINT, from its first byte. `None` for an invalid zero
/// first byte, which would otherwise mean a length over 8.
fn vint_len(first: u8) -> Option<usize> {
    if first == 0 {
        return None;
    }
    Some((first.leading_zeros() + 1) as usize)
}

/// Read an element id: the marker bit is **kept**, so the returned value
/// compares directly against the constants above.
pub fn read_id<S: ByteSource + ?Sized>(src: &mut S, at: u64) -> Result<(u32, usize)> {
    let mut b = [0u8; 4];
    let n = src.read_at(at, &mut b)?;
    if n == 0 {
        return Err(Error::Truncated { at, wanted: 1 });
    }
    let len = vint_len(b[0])
        .ok_or_else(|| Error::Malformed(format!("invalid EBML id length byte at {at}")))?;
    if len > 4 {
        return Err(Error::Malformed(format!(
            "EBML id of {len} bytes at {at}; the spec allows at most 4"
        )));
    }
    if n < len {
        return Err(Error::Truncated { at, wanted: len });
    }
    let mut v = 0u32;
    for byte in b.iter().take(len) {
        v = (v << 8) | *byte as u32;
    }
    Ok((v, len))
}

/// Read a size VINT: the marker bit is **stripped**. `Ok(None)` for the
/// all-ones "unknown size" encoding.
pub fn read_size<S: ByteSource + ?Sized>(src: &mut S, at: u64) -> Result<(Option<u64>, usize)> {
    let mut b = [0u8; 8];
    let n = src.read_at(at, &mut b)?;
    if n == 0 {
        return Err(Error::Truncated { at, wanted: 1 });
    }
    let len = vint_len(b[0])
        .ok_or_else(|| Error::Malformed(format!("invalid EBML size length byte at {at}")))?;
    if len > 8 {
        return Err(Error::Malformed(format!("EBML size of {len} bytes at {at}")));
    }
    if n < len {
        return Err(Error::Truncated { at, wanted: len });
    }

    // Strip the marker: the length bit in the first byte is not part of the value.
    let mut v = (b[0] as u64) & ((1u64 << (8 - len)) - 1);
    let mut all_ones = v == (1u64 << (8 - len)) - 1;
    for byte in b.iter().take(len).skip(1) {
        v = (v << 8) | *byte as u64;
        all_ones &= *byte == 0xFF;
    }
    Ok((if all_ones { None } else { Some(v) }, len))
}

pub fn read_element_header<S: ByteSource + ?Sized>(src: &mut S, at: u64) -> Result<ElementHeader> {
    let (element_id, id_len) = read_id(src, at)?;
    let (size, size_len) = read_size(src, at + id_len as u64)?;
    Ok(ElementHeader {
        id: element_id,
        payload_offset: at + (id_len + size_len) as u64,
        payload_len: size,
    })
}

/// Iterate sibling elements in `[start, limit)`.
pub struct ElementIter {
    cursor: u64,
    limit: u64,
}

impl ElementIter {
    pub fn new(start: u64, limit: u64) -> Self {
        Self {
            cursor: start,
            limit,
        }
    }

    pub fn position(&self) -> u64 {
        self.cursor
    }

    pub fn next<S: ByteSource + ?Sized>(&mut self, src: &mut S) -> Result<Option<ElementHeader>> {
        if self.cursor >= self.limit {
            return Ok(None);
        }
        let h = match read_element_header(src, self.cursor) {
            Ok(h) => h,
            // Running out of bytes at a sibling boundary is the normal way a
            // truncated-but-usable file ends, not an error worth failing on.
            Err(Error::Truncated { .. }) => return Ok(None),
            Err(e) => return Err(e),
        };

        let len = match h.payload_len {
            Some(n) => n,
            None => {
                // Unknown size. A container runs until the next element that
                // is not one of its children; anything else is unrepresentable
                // and we refuse rather than guess.
                if !is_container(h.id) {
                    return Err(Error::Malformed(format!(
                        "non-container element {:#x} at {} has unknown size",
                        h.id, self.cursor
                    )));
                }
                // Hand the caller the header and let it scan; advancing past an
                // unknown-size container is the caller's problem because only
                // it knows what a sibling looks like.
                self.cursor = self.limit;
                return Ok(Some(h));
            }
        };

        let end = h
            .payload_offset
            .checked_add(len)
            .ok_or_else(|| Error::Malformed("EBML element size overflows".into()))?;
        if end > self.limit {
            return Err(Error::Malformed(format!(
                "element {:#x} at {} ends at {end}, past its container limit {}",
                h.id, self.cursor, self.limit
            )));
        }
        if end <= self.cursor {
            return Err(Error::Malformed(format!(
                "element {:#x} at {} does not advance",
                h.id, self.cursor
            )));
        }
        self.cursor = end;
        Ok(Some(h))
    }
}

/// Big-endian unsigned integer of 1..=8 bytes, as EBML stores them.
pub fn read_uint<S: ByteSource + ?Sized>(src: &mut S, at: u64, len: u64) -> Result<u64> {
    if len == 0 || len > 8 {
        return Err(Error::Malformed(format!("uint of {len} bytes at {at}")));
    }
    let buf = vidkit_core::io::read_exact_at(src, at, len as usize)?;
    let mut v = 0u64;
    for b in buf {
        v = (v << 8) | b as u64;
    }
    Ok(v)
}

/// EBML floats are 4 or 8 bytes.
pub fn read_float<S: ByteSource + ?Sized>(src: &mut S, at: u64, len: u64) -> Result<f64> {
    let buf = vidkit_core::io::read_exact_at(src, at, len as usize)?;
    Ok(match len {
        4 => f32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as f64,
        8 => f64::from_be_bytes([
            buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
        ]),
        n => return Err(Error::Malformed(format!("float of {n} bytes at {at}"))),
    })
}

/// Signed integer, two's complement, 1..=8 bytes.
pub fn read_int<S: ByteSource + ?Sized>(src: &mut S, at: u64, len: u64) -> Result<i64> {
    let raw = read_uint(src, at, len)?;
    let bits = len * 8;
    // Sign-extend from the stored width.
    let shift = 64 - bits;
    Ok(((raw << shift) as i64) >> shift)
}
