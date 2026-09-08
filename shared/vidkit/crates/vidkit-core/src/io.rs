//! Random access to media bytes, and the errors reading them can produce.
//!
//! This is a core seam, not a container detail. Both demuxers need it and so
//! does anything that fetches over HTTP, and it lived in `vidkit-mp4` only
//! because that is where the first reader happened to be written -- which left
//! `vidkit-webm` depending on the MP4 crate for a trait that has nothing to do
//! with MP4.
//!
//! `read_at` rather than `Read + Seek` on purpose: the two real sources are a
//! local file and an HTTP server answering range requests, and a range request
//! IS a `read_at`. Modelling it as a cursor invents a seek position that does
//! not exist and hides how many requests a demuxer actually makes.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(String),
    /// Ran off the end of the data.
    Truncated { at: u64, wanted: usize },
    /// Structurally invalid, with a note about where.
    Malformed(String),
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(s) => write!(f, "io: {s}"),
            Error::Truncated { at, wanted } => {
                write!(f, "truncated at {at}, wanted {wanted} bytes")
            }
            Error::Malformed(s) => write!(f, "malformed: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Random access to the media bytes.
pub trait ByteSource {
    /// Fill `buf` from `offset`. Short reads are only allowed at end of data.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize>;
    /// Total length, when known. `None` for a stream of unknown size.
    fn size(&self) -> Option<u64>;
}

/// Read exactly `len` bytes, or fail.
pub fn read_exact_at<S: ByteSource + ?Sized>(src: &mut S, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let n = src.read_at(offset, &mut buf)?;
    if n != len {
        return Err(Error::Truncated {
            at: offset,
            wanted: len,
        });
    }
    Ok(buf)
}


/// In-memory source, for tests and for small files read up front.
pub struct MemorySource(pub Vec<u8>);

impl ByteSource for MemorySource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let start = offset as usize;
        if start >= self.0.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.0.len() - start);
        buf[..n].copy_from_slice(&self.0[start..start + n]);
        Ok(n)
    }
    fn size(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }
}
