// gitmind-core/src/store.rs
//
// The chunk store. This is the answer to three of the requested goals
// at once — "no 1 GB limit", "no bad file compression", and "a database
// for storing files efficiently":
//
//   ingest(file):
//     stream the file through content-defined chunking (FastCDC) so we
//     never hold the whole file in RAM — a 10 GB asset ingests in
//     bounded memory, which is what actually removes the size ceiling.
//     Each chunk is BLAKE3-addressed; new chunks are zstd-compressed and
//     written to redb; chunks we've seen before are skipped (dedup). The
//     file becomes a small manifest: an ordered list of chunk addresses.
//
//   restore(blob_oid -> writer):
//     read the manifest, stream each chunk back out decompressed.
//
// Why this beats putting binaries in git: git stores each version of a
// binary as a fresh zlib-deflated blob (deltas rarely help on already-
// compressed media), so N near-identical exports cost ~N× size. Here,
// re-exporting a video with a small change re-stores only the chunks
// that actually changed. Same content, stored once, compressed better.
//
// Media (photos/video) is detected at ingest. Originals are ALWAYS
// stored losslessly through the same chunk path — we never silently
// transcode versioned source-of-truth bytes. When the `media-proxy`
// feature is on, we ADDITIONALLY enqueue an AV1/AVIF proxy for fast
// preview/scrub; the proxy references the original and never replaces
// it. Lossy replacement, if ever wanted, is an explicit caller choice.

use crate::oid::Oid;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;

// FastCDC parameters. These are FIXED FOREVER: changing them shifts all
// chunk boundaries and breaks dedup against everything already stored.
// 64 KiB average keeps per-chunk overhead low for large media while
// still deduping fine-grained edits in source files.
const CDC_MIN: u32 = 16 * 1024;
const CDC_AVG: u32 = 64 * 1024;
const CDC_MAX: u32 = 256 * 1024;

// zstd level 9: strong ratio, still fast. Tunable later; the stored
// bytes are self-describing so the level can change per-chunk without
// migration.
const ZSTD_LEVEL: i32 = 9;

// redb tables.
//   CHUNKS:  chunk Oid           -> zstd-compressed chunk bytes
//   BLOBS:   blob Oid (manifest) -> serialized BlobManifest (json)
const CHUNKS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("chunks");
const BLOBS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("blobs");
// Native git-object graph: every reachable object's git oid → (kind byte,
// BLAKE3 blob oid whose content the chunk store holds). Lets us walk and
// reconstruct the commit/tree/blob DAG with no .git present. REFS maps a
// ref name → git oid so branches/tags survive too.
const OBJECTS: TableDefinition<&str, &[u8]> = TableDefinition::new("objects");
const REFS: TableDefinition<&str, &str> = TableDefinition::new("refs");
/// Per-commit build/check status, JSON-encoded, keyed by commit oid.
const CHECKS: TableDefinition<&str, &[u8]> = TableDefinition::new("checks");

/// git object kinds, as stored in the OBJECTS table.
pub const KIND_COMMIT: u8 = 1;
pub const KIND_TREE: u8 = 2;
pub const KIND_BLOB: u8 = 3;
pub const KIND_TAG: u8 = 4;

/// Map a `git cat-file` type string to a kind byte.
pub fn kind_from_str(s: &str) -> Option<u8> {
    match s {
        "commit" => Some(KIND_COMMIT),
        "tree" => Some(KIND_TREE),
        "blob" => Some(KIND_BLOB),
        "tag" => Some(KIND_TAG),
        _ => None,
    }
}

/// Map a kind byte back to git's type string (for `git hash-object -t`).
pub fn kind_to_str(k: u8) -> &'static str {
    match k {
        KIND_COMMIT => "commit",
        KIND_TREE => "tree",
        KIND_BLOB => "blob",
        KIND_TAG => "tag",
        _ => "blob",
    }
}

/// What kind of content a blob is, decided at ingest. Drives whether a
/// proxy gets enqueued and how the UI previews it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Other,
    Image,
    Video,
}

/// A stored file as an ordered list of chunk references plus metadata.
/// The blob's own Oid is the BLAKE3 of this struct's serialized bytes,
/// so the entire file is content-addressed end to end.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobManifest {
    /// Ordered chunk addresses; restore concatenates these.
    pub chunks: Vec<Oid>,
    /// Per-chunk uncompressed lengths (parallel to `chunks`), so restore
    /// can pre-size buffers and verify length without decompressing.
    pub lens: Vec<u32>,
    /// Total uncompressed size of the original file.
    pub total_len: u64,
    pub media: MediaKind,
    /// Original filename hint (extension drives media detection + the
    /// "file prefixing in directories" worktree feature later).
    pub name: String,
}

/// Outcome of an ingest, surfaced to the wizard/CLI so the user sees the
/// dedup win ("ingested 1.9 GB, 38 MB new").
#[derive(Clone, Debug)]
pub struct IngestReport {
    pub blob_oid: Oid,
    pub total_len: u64,
    pub chunk_count: usize,
    pub new_chunks: usize,
    pub new_bytes_stored: u64,
    pub media: MediaKind,
}

/// Outcome of a delta repack pass.
#[derive(Clone, Debug)]
pub struct RepackReport {
    pub chunks_scanned: usize,
    pub chunks_deltified: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

pub struct ChunkStore {
    db: Database,
}

/// Decode a stored chunk value. Plain chunks are a bare zstd frame (first
/// byte 0x28). A delta chunk is tagged `'D'` and stores
/// `'D' || base_oid(32) || u32_le len || zstd(raw, dict = base raw)`; we
/// reconstruct the base (recursively, depth-guarded) and zstd-decode against
/// it. This is how cross-file similarity becomes storage savings. `get`
/// returns a base chunk's stored (still-encoded) value by oid.
fn decode_chunk(
    value: &[u8],
    get: &dyn Fn(&[u8]) -> Result<Option<Vec<u8>>, String>,
    depth: u8,
) -> Result<Vec<u8>, String> {
    if value.first() == Some(&b'D') {
        if depth > 16 {
            return Err("delta chain too deep".into());
        }
        if value.len() < 37 {
            return Err("corrupt delta chunk".into());
        }
        let base_oid = &value[1..33];
        let len = u32::from_le_bytes(value[33..37].try_into().unwrap()) as usize;
        let payload = &value[37..];
        let base_comp = get(base_oid)?.ok_or("missing delta base chunk")?;
        let base_raw = decode_chunk(&base_comp, get, depth + 1)?;
        zstd::bulk::Decompressor::with_dictionary(&base_raw)
            .map_err(|e| e.to_string())?
            .decompress(payload, len)
            .map_err(|e| format!("undelta: {e}"))
    } else {
        zstd::decode_all(value).map_err(|e| format!("unzstd: {e}"))
    }
}

impl ChunkStore {
    /// Open (or create) the store at `path` (a single redb file, e.g.
    /// `.git/gitmind/store.redb`).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let db = Database::create(path).map_err(|e| format!("open store: {e}"))?;
        // Touch both tables so the first read txn never fails on a
        // missing table.
        {
            let w = db.begin_write().map_err(|e| e.to_string())?;
            { w.open_table(CHUNKS).map_err(|e| e.to_string())?; }
            { w.open_table(BLOBS).map_err(|e| e.to_string())?; }
            { w.open_table(OBJECTS).map_err(|e| e.to_string())?; }
            { w.open_table(REFS).map_err(|e| e.to_string())?; }
            { w.open_table(CHECKS).map_err(|e| e.to_string())?; }
            w.commit().map_err(|e| e.to_string())?;
        }
        Ok(ChunkStore { db })
    }

    /// Stream a file into the store. Bounded memory regardless of file
    /// size — this is the property that removes the size ceiling.
    pub fn ingest(&self, name: &str, reader: impl Read) -> Result<IngestReport, String> {
        let media = detect_media(name);
        let mut chunks: Vec<Oid> = Vec::new();
        let mut lens: Vec<u32> = Vec::new();
        let mut total_len: u64 = 0;
        let mut new_chunks = 0usize;
        let mut new_bytes_stored: u64 = 0;

        let chunker = fastcdc::v2020::StreamCDC::new(reader, CDC_MIN, CDC_AVG, CDC_MAX);

        let w = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = w.open_table(CHUNKS).map_err(|e| e.to_string())?;
            for item in chunker {
                let chunk = item.map_err(|e| format!("chunking: {e}"))?;
                let data = chunk.data;
                let oid = Oid::hash(&data);
                total_len += data.len() as u64;
                lens.push(data.len() as u32);
                chunks.push(oid);

                // Dedup: only compress + store chunks we haven't seen.
                if t.get(oid.0.as_slice()).map_err(|e| e.to_string())?.is_none() {
                    let comp = zstd::encode_all(&data[..], ZSTD_LEVEL)
                        .map_err(|e| format!("zstd: {e}"))?;
                    new_bytes_stored += comp.len() as u64;
                    new_chunks += 1;
                    t.insert(oid.0.as_slice(), comp.as_slice())
                        .map_err(|e| e.to_string())?;
                }
            }
        }

        let manifest = BlobManifest {
            chunks: chunks.clone(),
            lens,
            total_len,
            media,
            name: name.to_string(),
        };
        let mbytes = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
        let blob_oid = Oid::hash(&mbytes);
        {
            let mut t = w.open_table(BLOBS).map_err(|e| e.to_string())?;
            t.insert(blob_oid.0.as_slice(), mbytes.as_slice())
                .map_err(|e| e.to_string())?;
        }
        w.commit().map_err(|e| e.to_string())?;

        // When media-proxy is built in and this is media, enqueue an
        // AV1/AVIF proxy. Stub for now (no codec dep linked yet); the
        // original is already safely stored above either way.
        #[cfg(feature = "media-proxy")]
        if media != MediaKind::Other {
            let _ = proxy::enqueue(blob_oid, media);
        }

        Ok(IngestReport {
            blob_oid,
            total_len,
            chunk_count: manifest.chunks.len(),
            new_chunks,
            new_bytes_stored,
            media,
        })
    }

    /// Stream a stored blob back out to `writer`.
    pub fn restore(&self, blob_oid: Oid, mut writer: impl Write) -> Result<u64, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let blobs = r.open_table(BLOBS).map_err(|e| e.to_string())?;
        let raw = blobs
            .get(blob_oid.0.as_slice())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no such blob {}", blob_oid.short()))?;
        let manifest: BlobManifest =
            serde_json::from_slice(raw.value()).map_err(|e| e.to_string())?;
        drop(raw);

        let chunks = r.open_table(CHUNKS).map_err(|e| e.to_string())?;
        let get = |oid: &[u8]| -> Result<Option<Vec<u8>>, String> {
            Ok(chunks
                .get(oid)
                .map_err(|e| e.to_string())?
                .map(|g| g.value().to_vec()))
        };
        let mut written: u64 = 0;
        for coid in &manifest.chunks {
            let comp = chunks
                .get(coid.0.as_slice())
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("missing chunk {}", coid.short()))?;
            let data = decode_chunk(comp.value(), &get, 0)?;
            writer.write_all(&data).map_err(|e| e.to_string())?;
            written += data.len() as u64;
        }
        Ok(written)
    }

    /// Restore a blob's bytes into memory (convenience for reconstructing
    /// small git objects). For large blobs prefer the streaming `restore`.
    pub fn restore_bytes(&self, blob_oid: Oid) -> Result<Vec<u8>, String> {
        let mut buf = Vec::new();
        self.restore(blob_oid, &mut buf)?;
        Ok(buf)
    }

    /// Compact the underlying redb file, reclaiming pages freed by repack/
    /// overwrites/deletes. redb grows in pages and doesn't shrink the file in
    /// place, so this is what actually returns disk to the OS. Returns whether
    /// compaction moved anything.
    pub fn compact(&mut self) -> Result<bool, String> {
        self.db.compact().map_err(|e| format!("compact: {e}"))
    }

    /// Offline cross-file delta repack. Groups near-identical chunks (cheap
    /// length+prefix buckets), picks the largest in each group as a base, and
    /// re-encodes the rest in place as zstd-dictionary deltas when that saves
    /// at least `min_gain` (0.0–1.0) of the chunk's stored size. Bases stay
    /// plain, so delta depth is 1 and restore cost stays bounded. Oids are
    /// unchanged — this is pure storage compression. Returns before/after
    /// stored-byte totals.
    pub fn repack(&self, min_gain: f64) -> Result<RepackReport, String> {
        use std::collections::HashMap;
        // 1. Snapshot every chunk (oid, stored bytes, decoded raw, is_delta).
        let mut all: Vec<(Vec<u8>, usize, Vec<u8>, bool)> = Vec::new();
        {
            let r = self.db.begin_read().map_err(|e| e.to_string())?;
            let chunks = r.open_table(CHUNKS).map_err(|e| e.to_string())?;
            let get = |oid: &[u8]| -> Result<Option<Vec<u8>>, String> {
                Ok(chunks.get(oid).map_err(|e| e.to_string())?.map(|g| g.value().to_vec()))
            };
            for item in chunks.iter().map_err(|e| e.to_string())? {
                let (k, v) = item.map_err(|e| e.to_string())?;
                let oid = k.value().to_vec();
                let comp = v.value().to_vec();
                let is_delta = comp.first() == Some(&b'D');
                let raw = decode_chunk(&comp, &get, 0)?;
                all.push((oid, comp.len(), raw, is_delta));
            }
        }
        let bytes_before: u64 = all.iter().map(|(_, c, _, _)| *c as u64).sum();
        let scanned = all.len();

        // 2. Bucket plain chunks by (length band, leading bytes).
        let mut buckets: HashMap<(usize, [u8; 12]), Vec<usize>> = HashMap::new();
        for (i, (_, _, raw, is_delta)) in all.iter().enumerate() {
            if *is_delta {
                continue;
            }
            let mut pre = [0u8; 12];
            let n = raw.len().min(12);
            pre[..n].copy_from_slice(&raw[..n]);
            buckets.entry((raw.len() / 256, pre)).or_default().push(i);
        }

        // 3. Within each bucket, delta members against the largest one.
        let mut reencode: Vec<(usize, Vec<u8>)> = Vec::new();
        for idxs in buckets.values() {
            if idxs.len() < 2 {
                continue;
            }
            let base_i = *idxs.iter().max_by_key(|&&i| all[i].2.len()).unwrap();
            let base_raw = all[base_i].2.clone();
            let mut comp = zstd::bulk::Compressor::with_dictionary(ZSTD_LEVEL, &base_raw)
                .map_err(|e| e.to_string())?;
            for &i in idxs {
                if i == base_i {
                    continue;
                }
                let payload = comp.compress(&all[i].2).map_err(|e| format!("delta zstd: {e}"))?;
                let new_size = 1 + 32 + 4 + payload.len();
                if (new_size as f64) < (all[i].1 as f64) * (1.0 - min_gain) {
                    let mut val = Vec::with_capacity(new_size);
                    val.push(b'D');
                    val.extend_from_slice(&all[base_i].0);
                    val.extend_from_slice(&(all[i].2.len() as u32).to_le_bytes());
                    val.extend_from_slice(&payload);
                    reencode.push((i, val));
                }
            }
        }

        // 4. Commit the re-encodes.
        let deltified = reencode.len();
        let mut saved: u64 = 0;
        if !reencode.is_empty() {
            let w = self.db.begin_write().map_err(|e| e.to_string())?;
            {
                let mut t = w.open_table(CHUNKS).map_err(|e| e.to_string())?;
                for (i, val) in &reencode {
                    saved += all[*i].1.saturating_sub(val.len()) as u64;
                    t.insert(all[*i].0.as_slice(), val.as_slice()).map_err(|e| e.to_string())?;
                }
            }
            w.commit().map_err(|e| e.to_string())?;
        }

        Ok(RepackReport {
            chunks_scanned: scanned,
            chunks_deltified: deltified,
            bytes_before,
            bytes_after: bytes_before - saved,
        })
    }

    /// Record a native git object: its git oid → (kind, the BLAKE3 blob oid
    /// whose content the chunk store holds).
    pub fn put_object(&self, git_oid: &str, kind: u8, blob_oid: Oid) -> Result<(), String> {
        let mut val = Vec::with_capacity(33);
        val.push(kind);
        val.extend_from_slice(&blob_oid.0);
        let w = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = w.open_table(OBJECTS).map_err(|e| e.to_string())?;
            t.insert(git_oid, val.as_slice()).map_err(|e| e.to_string())?;
        }
        w.commit().map_err(|e| e.to_string())
    }

    pub fn get_object(&self, git_oid: &str) -> Result<Option<(u8, Oid)>, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let t = r.open_table(OBJECTS).map_err(|e| e.to_string())?;
        match t.get(git_oid).map_err(|e| e.to_string())? {
            Some(v) => {
                let b = v.value();
                if b.len() != 33 {
                    return Err("corrupt object record".into());
                }
                let mut oid = Oid([0u8; 32]);
                oid.0.copy_from_slice(&b[1..33]);
                Ok(Some((b[0], oid)))
            }
            None => Ok(None),
        }
    }

    pub fn list_objects(&self) -> Result<Vec<(String, u8, Oid)>, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let t = r.open_table(OBJECTS).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in t.iter().map_err(|e| e.to_string())? {
            let (k, v) = row.map_err(|e| e.to_string())?;
            let b = v.value();
            if b.len() != 33 {
                continue;
            }
            let mut oid = Oid([0u8; 32]);
            oid.0.copy_from_slice(&b[1..33]);
            out.push((k.value().to_string(), b[0], oid));
        }
        Ok(out)
    }

    pub fn object_count(&self) -> Result<usize, String> {
        Ok(self.list_objects()?.len())
    }

    pub fn put_ref(&self, name: &str, git_oid: &str) -> Result<(), String> {
        let w = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = w.open_table(REFS).map_err(|e| e.to_string())?;
            t.insert(name, git_oid).map_err(|e| e.to_string())?;
        }
        w.commit().map_err(|e| e.to_string())
    }

    pub fn delete_ref(&self, name: &str) -> Result<bool, String> {
        let w = self.db.begin_write().map_err(|e| e.to_string())?;
        let existed;
        {
            let mut t = w.open_table(REFS).map_err(|e| e.to_string())?;
            existed = t.remove(name).map_err(|e| e.to_string())?.is_some();
        }
        w.commit().map_err(|e| e.to_string())?;
        Ok(existed)
    }

    pub fn refs(&self) -> Result<Vec<(String, String)>, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let t = r.open_table(REFS).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in t.iter().map_err(|e| e.to_string())? {
            let (k, v) = row.map_err(|e| e.to_string())?;
            out.push((k.value().to_string(), v.value().to_string()));
        }
        Ok(out)
    }

    /// Record a commit's build/check status (JSON bytes), keyed by commit oid.
    pub fn put_check(&self, commit: &str, json: &[u8]) -> Result<(), String> {
        let w = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = w.open_table(CHECKS).map_err(|e| e.to_string())?;
            t.insert(commit, json).map_err(|e| e.to_string())?;
        }
        w.commit().map_err(|e| e.to_string())
    }

    pub fn get_check(&self, commit: &str) -> Result<Option<Vec<u8>>, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let t = r.open_table(CHECKS).map_err(|e| e.to_string())?;
        Ok(t.get(commit).map_err(|e| e.to_string())?.map(|v| v.value().to_vec()))
    }

    pub fn list_checks(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let t = r.open_table(CHECKS).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in t.iter().map_err(|e| e.to_string())? {
            let (k, v) = row.map_err(|e| e.to_string())?;
            out.push((k.value().to_string(), v.value().to_vec()));
        }
        Ok(out)
    }

    /// Load a manifest without restoring (for the UI to show size, chunk
    /// count, media kind without touching chunk bytes).
    pub fn manifest(&self, blob_oid: Oid) -> Result<BlobManifest, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let blobs = r.open_table(BLOBS).map_err(|e| e.to_string())?;
        let raw = blobs
            .get(blob_oid.0.as_slice())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no such blob {}", blob_oid.short()))?;
        serde_json::from_slice(raw.value()).map_err(|e| e.to_string())
    }

    /// All stored blobs as (oid, manifest). Used by near-dup search; for
    /// large stores this should become an indexed query, but a scan is
    /// correct and fine to start.
    pub fn list_blobs(&self) -> Result<Vec<(Oid, BlobManifest)>, String> {
        let r = self.db.begin_read().map_err(|e| e.to_string())?;
        let blobs = r.open_table(BLOBS).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let iter = blobs.iter().map_err(|e| e.to_string())?;
        for row in iter {
            let (k, v) = row.map_err(|e| e.to_string())?;
            let mut oid = Oid([0u8; 32]);
            oid.0.copy_from_slice(k.value());
            let m: BlobManifest = serde_json::from_slice(v.value()).map_err(|e| e.to_string())?;
            out.push((oid, m));
        }
        Ok(out)
    }
}

/// Detect media kind by extension. Magic-byte sniffing can be layered in
/// later; extension is enough to decide proxy eligibility.
fn detect_media(name: &str) -> MediaKind {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tiff" | "bmp" => MediaKind::Image,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => MediaKind::Video,
        _ => MediaKind::Other,
    }
}

// Proxy generation lives behind the feature so the default build links
// no codecs. Interface only for now; the real encoder (libaom/SVT-AV1
// for AV1, AVIF for stills, via Vulkan/VAAPI where available) lands with
// the media phase.
#[cfg(feature = "media-proxy")]
mod proxy {
    use super::*;
    pub fn enqueue(_blob: Oid, _kind: MediaKind) -> Result<(), String> {
        // TODO(media phase): push onto a background encode queue. Writes
        // proxy bytes into a PROXIES table keyed by original blob Oid.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn store() -> (ChunkStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = ChunkStore::open(dir.path().join("store.redb")).unwrap();
        (s, dir)
    }

    #[test]
    fn roundtrip_preserves_bytes() {
        let (s, _d) = store();
        let payload: Vec<u8> = (0..500_000u32).map(|i| (i % 251) as u8).collect();
        let rep = s.ingest("data.bin", Cursor::new(payload.clone())).unwrap();
        let mut out = Vec::new();
        let n = s.restore(rep.blob_oid, &mut out).unwrap();
        assert_eq!(n, payload.len() as u64);
        assert_eq!(out, payload);
    }

    #[test]
    fn dedup_second_identical_ingest_stores_nothing_new() {
        let (s, _d) = store();
        let payload: Vec<u8> = (0..400_000u32).map(|i| (i % 97) as u8).collect();
        let _a = s.ingest("a.bin", Cursor::new(payload.clone())).unwrap();
        let b = s.ingest("b.bin", Cursor::new(payload.clone())).unwrap();
        // Same content → every chunk already present → zero new chunks.
        assert_eq!(b.new_chunks, 0);
        assert_eq!(b.new_bytes_stored, 0);
    }

    #[test]
    fn detects_media_by_extension() {
        assert_eq!(detect_media("clip.MP4"), MediaKind::Video);
        assert_eq!(detect_media("photo.heic"), MediaKind::Image);
        assert_eq!(detect_media("main.rs"), MediaKind::Other);
    }
}
