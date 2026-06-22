//! Chunked payload splitting and reassembly.
//!
//! Ported from SnapRouter.cpp::sendV3() chunking + UdpChannel::onPayloadReceived().
//!
//! The C++ splits large payloads into kChunkSize (8 KB) chunks and sends them
//! with a sliding window (kWindowSize = 16). This module provides:
//!   - `split()` — split a payload into indexed chunks
//!   - `Reassembler` — stateful receiver that collects chunks and yields the
//!     complete payload when all chunks arrive

pub const CHUNK_SIZE: usize = 8 * 1024; // 8 KB, matches kChunkSize in UdpChannel.hpp

/// A single chunk: index + data.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub session: u16,
    pub index:   u32,
    pub total:   u32,
    pub data:    bytes::Bytes,
}

/// Split `payload` into chunks of `CHUNK_SIZE` bytes.
pub fn split(session: u16, payload: &[u8]) -> Vec<Chunk> {
    if payload.is_empty() {
        return vec![Chunk {
            session,
            index: 0,
            total: 1,
            data: bytes::Bytes::new(),
        }];
    }

    let total = ((payload.len() + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32;
    payload
        .chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(i, slice)| Chunk {
            session,
            index: i as u32,
            total,
            data: bytes::Bytes::copy_from_slice(slice),
        })
        .collect()
}

/// Stateful reassembler that accumulates chunks and signals completion.
pub struct Reassembler {
    session:  u16,
    total:    u32,
    received: u32,
    chunks:   std::collections::HashMap<u32, bytes::Bytes>,
    last_activity: std::time::Instant,
}

impl Reassembler {
    pub fn new(session: u16, total: u32) -> Self {
        Self {
            session,
            total,
            received: 0,
            chunks: std::collections::HashMap::new(),
            last_activity: std::time::Instant::now(),
        }
    }

    /// Accept a chunk. Returns `Some(payload)` when all chunks have arrived.
    pub fn accept(&mut self, chunk: Chunk) -> Option<bytes::Bytes> {
        if chunk.session != self.session { return None; }
        if self.chunks.contains_key(&chunk.index) { return None; } // duplicate
        self.chunks.insert(chunk.index, chunk.data);
        self.received += 1;
        self.last_activity = std::time::Instant::now();

        if self.received == self.total {
            // Assemble in order
            let mut out = bytes::BytesMut::new();
            for i in 0..self.total {
                if let Some(d) = self.chunks.get(&i) {
                    out.extend_from_slice(d);
                } else {
                    return None; // gap — should not happen if received == total
                }
            }
            Some(out.freeze())
        } else {
            None
        }
    }

    /// True if this reassembler has been idle for more than `timeout`.
    pub fn is_stale(&self, timeout: std::time::Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    pub fn progress(&self) -> f32 {
        self.received as f32 / self.total.max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_reassemble() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i & 0xFF) as u8).collect();
        let chunks = split(1, &data);
        assert_eq!(chunks.len(), 3); // ceil(20000/8192) = 3
        assert!(chunks.iter().all(|c| c.total == 3));

        let mut r = Reassembler::new(1, 3);
        for (i, chunk) in chunks.into_iter().enumerate() {
            let result = r.accept(chunk);
            if i < 2 { assert!(result.is_none()); }
            else      { assert_eq!(&result.unwrap()[..], &data[..]); }
        }
    }

    #[test]
    fn duplicate_chunk_ignored() {
        let chunks = split(5, b"hello world");
        let mut r = Reassembler::new(5, chunks.len() as u32);
        let c0 = chunks[0].clone();
        r.accept(chunks[0].clone()); // first accept
        let result = r.accept(c0);  // duplicate
        assert!(result.is_none());  // should not double-count
    }
}
