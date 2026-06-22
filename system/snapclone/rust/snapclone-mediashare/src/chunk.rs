//! chunk.rs — content-addressed chunking for shared-conversation media dedup.
//!
//! A media blob is split into fixed-size pieces, each identified by the SHA-256
//! of its bytes (a `ChunkId`). The small `MediaManifest` lists those ids in
//! order. Because identical bytes always hash to the same id, two pieces of
//! media that share content (a re-sent image, a meme template, a video's
//! repeated frames) share chunks — and a member who already holds a chunk (from
//! ANY earlier item in the conversation) never receives it again. The receiver
//! asks only for the `missing` ids, then `reassemble`s with hash verification.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Default chunk size (bytes). 16 KiB balances dedup granularity vs manifest size.
pub const DEFAULT_CHUNK_SIZE: usize = 16 * 1024;

/// A chunk's content address: SHA-256 of its bytes.
pub type ChunkId = [u8; 32];

/// SHA-256 content id of a byte slice.
pub fn chunk_id(bytes: &[u8]) -> ChunkId {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

/// An ordered, content-addressed description of a blob. Small (32 B per chunk),
/// so it is what travels first; chunks are fetched on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaManifest {
    pub total_len: u64,
    pub chunk_size: u32, // nominal; the final chunk may be shorter
    pub chunks: Vec<ChunkId>,
}

impl MediaManifest {
    /// Compact wire encoding:
    /// [total_len u64 BE][chunk_size u32 BE][count u32 BE][id (32 B) * count]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.chunks.len() * 32);
        out.extend_from_slice(&self.total_len.to_be_bytes());
        out.extend_from_slice(&self.chunk_size.to_be_bytes());
        out.extend_from_slice(&(self.chunks.len() as u32).to_be_bytes());
        for id in &self.chunks {
            out.extend_from_slice(id);
        }
        out
    }

    /// Parse an encoded manifest. None if truncated or inconsistent.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }
        let total_len = u64::from_be_bytes(bytes[0..8].try_into().ok()?);
        let chunk_size = u32::from_be_bytes(bytes[8..12].try_into().ok()?);
        let count = u32::from_be_bytes(bytes[12..16].try_into().ok()?) as usize;
        let body = &bytes[16..];
        if body.len() != count * 32 {
            return None;
        }
        let mut chunks = Vec::with_capacity(count);
        for i in 0..count {
            let mut id = [0u8; 32];
            id.copy_from_slice(&body[i * 32..i * 32 + 32]);
            chunks.push(id);
        }
        Some(Self { total_len, chunk_size, chunks })
    }
}

/// Split a blob into content-addressed chunks. Returns the manifest plus the
/// UNIQUE chunks (deduped within this blob — a repeated piece is emitted once).
pub fn split(data: &[u8], chunk_size: usize) -> (MediaManifest, Vec<(ChunkId, Vec<u8>)>) {
    let cs = chunk_size.max(1);
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for piece in data.chunks(cs) {
        let id = chunk_id(piece);
        order.push(id);
        if seen.insert(id) {
            unique.push((id, piece.to_vec()));
        }
    }
    let manifest = MediaManifest {
        total_len: data.len() as u64,
        chunk_size: cs as u32,
        chunks: order,
    };
    (manifest, unique)
}

/// A content-addressed chunk store. Persists across the whole conversation, so
/// chunks from any prior media are available to dedup against.
#[derive(Debug, Default)]
pub struct ChunkStore {
    map: HashMap<ChunkId, Vec<u8>>,
}

impl ChunkStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert by content; returns the id. Idempotent.
    pub fn insert(&mut self, bytes: &[u8]) -> ChunkId {
        let id = chunk_id(bytes);
        self.map.entry(id).or_insert_with(|| bytes.to_vec());
        id
    }

    /// Insert a chunk received with a claimed id, verifying the bytes hash to it.
    /// Returns false (and stores nothing) if the id doesn't match — this is how a
    /// receiver rejects a corrupted/spoofed chunk.
    pub fn insert_verified(&mut self, id: &ChunkId, bytes: &[u8]) -> bool {
        if &chunk_id(bytes) != id {
            return false;
        }
        self.map.entry(*id).or_insert_with(|| bytes.to_vec());
        true
    }

    pub fn contains(&self, id: &ChunkId) -> bool {
        self.map.contains_key(id)
    }

    pub fn get(&self, id: &ChunkId) -> Option<&[u8]> {
        self.map.get(id).map(|v| v.as_slice())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The distinct chunk ids in `manifest` that `store` is missing — exactly what a
/// receiver needs to request. Deduped + in first-seen order.
pub fn missing(manifest: &MediaManifest, store: &ChunkStore) -> Vec<ChunkId> {
    let mut seen = HashSet::new();
    manifest
        .chunks
        .iter()
        .filter(|id| !store.contains(id) && seen.insert(**id))
        .copied()
        .collect()
}

/// Reassemble the blob from the store, verifying the total length. Returns None
/// if any chunk is missing (call `missing` first) or the length is inconsistent.
pub fn reassemble(manifest: &MediaManifest, store: &ChunkStore) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(manifest.total_len as usize);
    for id in &manifest.chunks {
        out.extend_from_slice(store.get(id)?);
    }
    if out.len() as u64 != manifest.total_len {
        return None;
    }
    Some(out)
}

// ─── FFI ─────────────────────────────────────────────────────────────────────
//
// Send side: add_blob chunks the media, stores the chunks locally to serve, and
// returns the manifest to broadcast. Receive side: missing lists what to fetch,
// insert_verified accepts fetched chunks, reassemble rebuilds the blob.

use std::os::raw::c_int;

/// New chunk store. Free with `sc_chunk_store_free`.
#[no_mangle]
pub extern "C" fn sc_chunk_store_new() -> *mut ChunkStore {
    Box::into_raw(Box::new(ChunkStore::new()))
}

/// # Safety
/// `handle` from `sc_chunk_store_new`, not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_chunk_store_free(handle: *mut ChunkStore) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Chunks currently stored (0 on null).
///
/// # Safety
/// `handle` from `sc_chunk_store_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_chunk_store_len(handle: *const ChunkStore) -> c_int {
    if handle.is_null() {
        return 0;
    }
    (*handle).len() as c_int
}

/// SEND: split `data` into chunks, store them here, and write the encoded
/// manifest to `out`. Returns 0; -1 (null), -2 (manifest buffer too small —
/// `out_len` = required).
///
/// # Safety
/// `data` valid for `len`; `out` writable for `out_cap`; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_chunk_store_add_blob(
    handle: *mut ChunkStore,
    data: *const u8,
    len: usize,
    chunk_size: usize,
    out_manifest: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if handle.is_null() || out_len.is_null() {
        return -1;
    }
    let blob = if data.is_null() || len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(data, len)
    };
    let cs = if chunk_size == 0 { DEFAULT_CHUNK_SIZE } else { chunk_size };
    let (manifest, chunks) = split(blob, cs);
    for (id, bytes) in chunks {
        (*handle).insert_verified(&id, &bytes);
    }
    let enc = manifest.encode();
    *out_len = enc.len();
    if out_manifest.is_null() || enc.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(enc.as_ptr(), out_manifest, enc.len());
    0
}

/// RECEIVE: distinct chunk ids in the encoded manifest that this store lacks.
/// Ids are written 32 bytes each into `out_ids` (capacity `max_ids` ids); the
/// count is written to `out_count`. Returns 0; -1 (null/bad manifest), -2
/// (`out_ids` too small — `out_count` = the number needed).
///
/// # Safety
/// `manifest` valid for `manifest_len`; `out_ids` writable for `max_ids*32`;
/// pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_chunk_store_missing(
    handle: *const ChunkStore,
    manifest: *const u8,
    manifest_len: usize,
    out_ids: *mut u8,
    max_ids: usize,
    out_count: *mut usize,
) -> c_int {
    if handle.is_null() || manifest.is_null() || out_count.is_null() {
        return -1;
    }
    let m = match MediaManifest::decode(std::slice::from_raw_parts(manifest, manifest_len)) {
        Some(m) => m,
        None => return -1,
    };
    let need = missing(&m, &*handle);
    *out_count = need.len();
    if out_ids.is_null() || need.len() > max_ids {
        return -2;
    }
    for (i, id) in need.iter().enumerate() {
        std::ptr::copy_nonoverlapping(id.as_ptr(), out_ids.add(i * 32), 32);
    }
    0
}

/// Insert a fetched chunk, verifying it hashes to `id32`. Returns 1 if accepted,
/// 0 if rejected (hash mismatch) or null.
///
/// # Safety
/// `id32` valid for 32 bytes; `bytes` valid for `len`; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_chunk_store_insert_verified(
    handle: *mut ChunkStore,
    id32: *const u8,
    bytes: *const u8,
    len: usize,
) -> c_int {
    if handle.is_null() || id32.is_null() {
        return 0;
    }
    let mut id = [0u8; 32];
    std::ptr::copy_nonoverlapping(id32, id.as_mut_ptr(), 32);
    let b = if bytes.is_null() || len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(bytes, len)
    };
    if (*handle).insert_verified(&id, b) {
        1
    } else {
        0
    }
}

/// Serve a stored chunk by id (the sender answering a request). Returns 0; -1
/// (null), -2 (`out` too small — `out_len` = size), -3 (not found).
///
/// # Safety
/// `id32` valid for 32 bytes; `out` writable for `out_cap`; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_chunk_store_get(
    handle: *const ChunkStore,
    id32: *const u8,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if handle.is_null() || id32.is_null() || out_len.is_null() {
        return -1;
    }
    let mut id = [0u8; 32];
    std::ptr::copy_nonoverlapping(id32, id.as_mut_ptr(), 32);
    match (*handle).get(&id) {
        Some(bytes) => {
            *out_len = bytes.len();
            if out.is_null() || bytes.len() > out_cap {
                return -2;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
            0
        }
        None => -3,
    }
}

/// Reassemble the blob described by the manifest from this store. Returns 0; -1
/// (null/bad manifest), -2 (`out` too small — `out_len` = total size), -3
/// (a chunk is missing — call `missing` and fetch first).
///
/// # Safety
/// `manifest` valid for `manifest_len`; `out` writable for `out_cap`; pointers valid.
#[no_mangle]
pub unsafe extern "C" fn sc_chunk_store_reassemble(
    handle: *const ChunkStore,
    manifest: *const u8,
    manifest_len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> c_int {
    if handle.is_null() || manifest.is_null() || out_len.is_null() {
        return -1;
    }
    let m = match MediaManifest::decode(std::slice::from_raw_parts(manifest, manifest_len)) {
        Some(m) => m,
        None => return -1,
    };
    *out_len = m.total_len as usize;
    if !missing(&m, &*handle).is_empty() {
        return -3;
    }
    match reassemble(&m, &*handle) {
        Some(blob) => {
            if out.is_null() || blob.len() > out_cap {
                return -2;
            }
            std::ptr::copy_nonoverlapping(blob.as_ptr(), out, blob.len());
            0
        }
        None => -3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn split_then_reassemble_roundtrips() {
        let blob = data(40_000);
        let (manifest, chunks) = split(&blob, DEFAULT_CHUNK_SIZE);
        let mut store = ChunkStore::new();
        for (id, bytes) in &chunks {
            assert!(store.insert_verified(id, bytes));
        }
        let back = reassemble(&manifest, &store).expect("reassemble");
        assert_eq!(back, blob);
    }

    #[test]
    fn repeated_content_dedupes_within_blob() {
        // 3 identical 1 KiB blocks → 1 unique chunk, 3 manifest entries
        let block = data(1024);
        let mut blob = Vec::new();
        for _ in 0..3 {
            blob.extend_from_slice(&block);
        }
        let (manifest, unique) = split(&blob, 1024);
        assert_eq!(manifest.chunks.len(), 3);
        assert_eq!(unique.len(), 1, "identical blocks share one chunk");
        // all three manifest ids are the same
        assert!(manifest.chunks.iter().all(|id| *id == manifest.chunks[0]));
    }

    #[test]
    fn conversation_dedup_skips_already_held_chunks() {
        // Store already holds media A's chunks; media B shares its first block.
        let shared = data(16 * 1024);
        let mut blob_a = shared.clone();
        blob_a.extend_from_slice(&data(16 * 1024)); // A = shared + unique-A
        let mut blob_b = shared.clone();
        blob_b.extend_from_slice(&[0xEE; 16 * 1024]); // B = shared + unique-B

        let (_, chunks_a) = split(&blob_a, 16 * 1024);
        let mut store = ChunkStore::new();
        for (id, bytes) in &chunks_a {
            store.insert_verified(id, bytes);
        }

        let (manifest_b, _) = split(&blob_b, 16 * 1024);
        let need = missing(&manifest_b, &store);
        // B has 2 chunks; the shared one is already held → only 1 must be fetched
        assert_eq!(manifest_b.chunks.len(), 2);
        assert_eq!(need.len(), 1, "shared block is not re-fetched");
    }

    #[test]
    fn missing_then_fetch_completes_reassembly() {
        let blob = data(50_000);
        let (manifest, chunks) = split(&blob, 16 * 1024);
        let mut store = ChunkStore::new();
        // receiver starts empty → everything missing
        assert_eq!(missing(&manifest, &store).len(), chunks.len());
        // fetch the missing chunks
        let lookup: HashMap<ChunkId, &Vec<u8>> = chunks.iter().map(|(i, b)| (*i, b)).collect();
        for id in missing(&manifest, &store) {
            assert!(store.insert_verified(&id, lookup[&id]));
        }
        assert!(missing(&manifest, &store).is_empty());
        assert_eq!(reassemble(&manifest, &store).unwrap(), blob);
    }

    #[test]
    fn insert_verified_rejects_wrong_id() {
        let mut store = ChunkStore::new();
        let bytes = b"real chunk bytes";
        let wrong = [0xAB; 32];
        assert!(!store.insert_verified(&wrong, bytes));
        assert!(!store.contains(&wrong));
        // correct id works
        let id = chunk_id(bytes);
        assert!(store.insert_verified(&id, bytes));
    }

    #[test]
    fn manifest_encode_decode_roundtrips() {
        let (manifest, _) = split(&data(70_000), 16 * 1024);
        let enc = manifest.encode();
        let dec = MediaManifest::decode(&enc).expect("decode");
        assert_eq!(dec, manifest);
        // truncated → None
        assert!(MediaManifest::decode(&enc[..enc.len() - 1]).is_none());
        assert!(MediaManifest::decode(&[0u8; 4]).is_none());
    }

    #[test]
    fn ffi_send_receive_roundtrip() {
        unsafe {
            let blob = data(40_000);
            let cs = 16 * 1024;

            // sender: chunk + store, get manifest
            let sender = sc_chunk_store_new();
            let mut manifest = vec![0u8; 4096];
            let mut mlen = 0usize;
            assert_eq!(
                sc_chunk_store_add_blob(sender, blob.as_ptr(), blob.len(), cs,
                    manifest.as_mut_ptr(), manifest.len(), &mut mlen),
                0
            );
            manifest.truncate(mlen);
            assert!(sc_chunk_store_len(sender) > 0);

            // receiver: query missing (size first, then fill)
            let receiver = sc_chunk_store_new();
            let mut count = 0usize;
            assert_eq!(
                sc_chunk_store_missing(receiver, manifest.as_ptr(), mlen,
                    std::ptr::null_mut(), 0, &mut count),
                -2
            );
            assert!(count > 0);
            let mut ids = vec![0u8; count * 32];
            assert_eq!(
                sc_chunk_store_missing(receiver, manifest.as_ptr(), mlen,
                    ids.as_mut_ptr(), count, &mut count),
                0
            );

            // fetch each from sender, insert into receiver
            for i in 0..count {
                let id = &ids[i * 32..i * 32 + 32];
                let mut chunk = vec![0u8; cs];
                let mut clen = 0usize;
                assert_eq!(
                    sc_chunk_store_get(sender, id.as_ptr(), chunk.as_mut_ptr(), chunk.len(), &mut clen),
                    0
                );
                assert_eq!(
                    sc_chunk_store_insert_verified(receiver, id.as_ptr(), chunk.as_ptr(), clen),
                    1
                );
            }

            // nothing missing now
            let mut c2 = 1usize;
            assert_eq!(
                sc_chunk_store_missing(receiver, manifest.as_ptr(), mlen,
                    ids.as_mut_ptr(), ids.len() / 32, &mut c2),
                0
            );
            assert_eq!(c2, 0);

            // reassemble
            let mut out = vec![0u8; blob.len()];
            let mut olen = 0usize;
            assert_eq!(
                sc_chunk_store_reassemble(receiver, manifest.as_ptr(), mlen,
                    out.as_mut_ptr(), out.len(), &mut olen),
                0
            );
            out.truncate(olen);
            assert_eq!(out, blob);

            sc_chunk_store_free(sender);
            sc_chunk_store_free(receiver);
        }
    }
}
