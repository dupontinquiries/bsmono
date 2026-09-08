//! ISOBMFF box traversal.
//!
//! The byte-source seam moved to `vidkit_core::io`; it is re-exported here so
//! existing paths keep working.

pub use vidkit_core::io::{read_exact_at, ByteSource, Error, MemorySource, Result};

use std::fmt;

/// A four-character box type, kept as bytes so unknown types round-trip in
/// error messages instead of turning into replacement characters.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    pub const fn new(s: &[u8; 4]) -> Self {
        FourCC(*s)
    }
    pub fn as_str(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

impl fmt::Debug for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}'", self.as_str())
    }
}

impl PartialEq<&[u8; 4]> for FourCC {
    fn eq(&self, other: &&[u8; 4]) -> bool {
        &self.0 == *other
    }
}

#[derive(Copy, Clone, Debug)]
pub struct BoxHeader {
    pub kind: FourCC,
    /// Offset of the box's first payload byte.
    pub payload_offset: u64,
    /// Payload length, excluding the header.
    pub payload_len: u64,
}

impl BoxHeader {
    pub fn end(&self) -> u64 {
        self.payload_offset + self.payload_len
    }
}

/// Read one box header at `offset`, bounded by `limit`.
///
/// Three size encodings exist and all three appear in the wild:
/// - `size >= 8`: the ordinary case.
/// - `size == 1`: a 64-bit `largesize` follows the type. Needed past 4 GiB,
///   which any long 4K recording exceeds.
/// - `size == 0`: the box runs to the end of the file. Legal only for the last
///   box, and mostly seen on `mdat` in files written by a crashed muxer.
pub fn read_box_header<S: ByteSource + ?Sized>(
    src: &mut S,
    offset: u64,
    limit: u64,
) -> Result<BoxHeader> {
    if offset + 8 > limit {
        return Err(Error::Truncated {
            at: offset,
            wanted: 8,
        });
    }
    let hdr = read_exact_at(src, offset, 8)?;
    let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
    let kind = FourCC([hdr[4], hdr[5], hdr[6], hdr[7]]);

    let (payload_offset, total) = match size32 {
        1 => {
            let ext = read_exact_at(src, offset + 8, 8)?;
            let large = u64::from_be_bytes([
                ext[0], ext[1], ext[2], ext[3], ext[4], ext[5], ext[6], ext[7],
            ]);
            if large < 16 {
                return Err(Error::Malformed(format!(
                    "{kind:?} largesize {large} smaller than its own header"
                )));
            }
            (offset + 16, large)
        }
        0 => (offset + 8, limit.saturating_sub(offset)),
        n if n < 8 => {
            return Err(Error::Malformed(format!(
                "{kind:?} size {n} smaller than a box header"
            )))
        }
        n => (offset + 8, n),
    };

    let end = offset
        .checked_add(total)
        .ok_or_else(|| Error::Malformed(format!("{kind:?} size overflows")))?;
    if end > limit {
        return Err(Error::Malformed(format!(
            "{kind:?} at {offset} ends at {end}, past its container limit {limit}"
        )));
    }

    Ok(BoxHeader {
        kind,
        payload_offset,
        payload_len: end - payload_offset,
    })
}

/// Iterate the boxes in `[start, limit)`.
pub struct BoxIter {
    cursor: u64,
    limit: u64,
}

impl BoxIter {
    pub fn new(start: u64, limit: u64) -> Self {
        Self {
            cursor: start,
            limit,
        }
    }

    pub fn next<S: ByteSource + ?Sized>(&mut self, src: &mut S) -> Result<Option<BoxHeader>> {
        // A box header needs 8 bytes; anything less is trailing padding, which
        // real files do contain.
        if self.cursor + 8 > self.limit {
            return Ok(None);
        }
        let h = read_box_header(src, self.cursor, self.limit)?;
        // Guard against a zero-length box spinning forever. A malformed file
        // should error, not hang.
        if h.end() <= self.cursor {
            return Err(Error::Malformed(format!(
                "{:?} at {} does not advance",
                h.kind, self.cursor
            )));
        }
        self.cursor = h.end();
        Ok(Some(h))
    }
}

/// Find the first child box of `kind` within `[start, limit)`.
pub fn find_box<S: ByteSource + ?Sized>(
    src: &mut S,
    start: u64,
    limit: u64,
    kind: &[u8; 4],
) -> Result<Option<BoxHeader>> {
    let mut it = BoxIter::new(start, limit);
    while let Some(h) = it.next(src)? {
        if h.kind == kind {
            return Ok(Some(h));
        }
    }
    Ok(None)
}

/// Walk a path of nested box types, e.g. `[b"moov", b"trak", b"mdia"]`.
pub fn find_path<S: ByteSource + ?Sized>(
    src: &mut S,
    start: u64,
    limit: u64,
    path: &[&[u8; 4]],
) -> Result<Option<BoxHeader>> {
    let mut s = start;
    let mut l = limit;
    let mut found = None;
    for kind in path {
        match find_box(src, s, l, kind)? {
            Some(h) => {
                s = h.payload_offset;
                l = h.end();
                found = Some(h);
            }
            None => return Ok(None),
        }
    }
    Ok(found)
}

// ---------------------------------------------------------------------------
// small readers
// ---------------------------------------------------------------------------

pub fn be_u16(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or_else(|| Error::Malformed(format!("u16 read past end at {at}")))
}

pub fn be_u32(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| Error::Malformed(format!("u32 read past end at {at}")))
}

pub fn be_u64(b: &[u8], at: usize) -> Result<u64> {
    b.get(at..at + 8)
        .map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
        .ok_or_else(|| Error::Malformed(format!("u64 read past end at {at}")))
}

