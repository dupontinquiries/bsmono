//! FAO library on-disk database — append-only log + memory index.
//!
//! Replaces (and reads back) the manifest.json format with a binary
//! append-only log inspired by the bitcask design (Riak). Why this
//! works for FAO's workload:
//!
//! - Writes are infrequent and small (a new canonical web is ~200B
//!   metadata).
//! - Reads are frequent (every chunk lookup during decode hits the
//!   in-memory UUID index).
//! - The library scales to tens of thousands of webs; JSON parse at
//!   that scale becomes noticeable (~10ms/1k webs linear).
//! - We control the format end-to-end and can change it ourselves
//!   without depending on an external DB.
//!
//! ## File layout
//! ```
//! library_dir/
//!     webs.log            current append-only log
//!     webs.log.bak.0      most recent backup (pre-compact or pre-rollback)
//!     webs.log.bak.1      ...
//!     webs.log.bak.4
//! ```
//!
//! ## Record format (binary, little-endian)
//! ```
//! offset 0:  magic "WLR1"          4 bytes
//! offset 4:  crc32 of (offset 8..) 4 bytes
//! offset 8:  kind                  1 byte   (0 = put, 1 = tombstone)
//! offset 9:  uuid                  16 bytes (binary; not hex)
//! offset 25: payload_len           4 bytes  (0 for tombstone)
//! offset 29: payload               payload_len bytes
//! ```
//!
//! ## Payload format (LibraryWebStub serialized)
//! ```
//! source_video_len   u16
//! source_video       N bytes (UTF-8)
//! source_chunk_start u32
//! source_chunk_end   u32
//! vertex_count       u32
//! triangle_count     u32
//! ref_count          u32
//! created_at         u64
//! last_referenced    u64
//! signature          112 bytes (WebSignature serialized by signature_to_bytes)
//! ```
//!
//! ## Crash safety
//! Each record write is a single std::io::Write::write_all of a small
//! buffer (~300 bytes). POSIX guarantees writes < PIPE_BUF (typically
//! 4096) are atomic, so a record either lands entirely or not at all.
//! Larger writes can tear; we keep records small to stay under that
//! limit.
//!
//! On open, we scan the log forward. If a record's CRC fails or EOF
//! hits mid-record, we TRUNCATE the log to the last valid record's
//! end. Crash recovery is implicit: bad bytes get cut off.
//!
//! ## Rollback
//! Same UI as manifest.json: replace webs.log with one of the .bak.N
//! files. Use `rollback_log(library_dir, slot)`.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::library::LibraryWebStub;
use crate::v8_reconstruction_codec::WebSignature;

pub const RECORD_MAGIC: &[u8; 4] = b"WLR1";
pub const RECORD_HEADER_BYTES: usize = 29; // magic + crc + kind + uuid + payload_len
pub const KIND_PUT: u8 = 0;
pub const KIND_DEL: u8 = 1;

/// Signature byte size (matches library::signature_to_bytes).
pub const SIGNATURE_BYTES: usize = 112;

#[derive(Debug)]
pub enum DbError {
    Io(std::io::Error),
    Corrupt(String),
    NotFound,
}

impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> Self { DbError::Io(e) }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Io(e) => write!(f, "I/O: {}", e),
            DbError::Corrupt(s) => write!(f, "corrupt: {}", s),
            DbError::NotFound => write!(f, "not found"),
        }
    }
}

/// CRC32 (IEEE polynomial 0xEDB88320). Hand-rolled to avoid a crate
/// dep. ~50ns per byte — acceptable for our record sizes.
/// 
/// Lookup table built lazily on first call via `OnceLock` — thread-safe 
/// without the older `static mut + Once` pattern that the Rust 2024 
/// edition lint flags as UB-prone.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for n in 0..256u32 {
            let mut c = n;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            t[n as usize] = c;
        }
        t
    });
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFFFFFF
}

/// Encode a UUID hex string (32 hex chars, with or without dashes) to
/// 16 raw bytes. Returns None on bad input.
pub fn uuid_hex_to_bytes(hex: &str) -> Option<[u8; 16]> {
    let cleaned: String = hex.chars().filter(|c| *c != '-').collect();
    if cleaned.len() != 32 { return None; }
    let mut out = [0u8; 16];
    for i in 0..16 {
        let hi = hex_nibble(cleaned.as_bytes()[i*2])?;
        let lo = hex_nibble(cleaned.as_bytes()[i*2+1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

pub fn uuid_bytes_to_hex(b: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for &byte in b {
        s.push(hex_char(byte >> 4));
        s.push(hex_char(byte & 0xf));
    }
    s
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_char(n: u8) -> char {
    if n < 10 { (b'0' + n) as char } else { (b'a' + n - 10) as char }
}

/// Serialize a LibraryWebStub to the payload bytes (after the header).
pub fn stub_to_payload(stub: &LibraryWebStub) -> Vec<u8> {
    let mut out = Vec::with_capacity(160);
    let sv = stub.source_video.as_bytes();
    out.extend_from_slice(&(sv.len() as u16).to_le_bytes());
    out.extend_from_slice(sv);
    out.extend_from_slice(&stub.source_chunk_start.to_le_bytes());
    out.extend_from_slice(&stub.source_chunk_end.to_le_bytes());
    out.extend_from_slice(&stub.vertex_count.to_le_bytes());
    out.extend_from_slice(&stub.triangle_count.to_le_bytes());
    out.extend_from_slice(&stub.ref_count.to_le_bytes());
    out.extend_from_slice(&stub.created_at.to_le_bytes());
    out.extend_from_slice(&stub.last_referenced.to_le_bytes());
    out.extend_from_slice(&crate::library::signature_to_bytes_pub(&stub.signature));
    out
}

/// Deserialize a payload into a LibraryWebStub. UUID must be passed in 
/// separately since the wire format keeps it in the header.
pub fn payload_to_stub(uuid_hex: String, p: &[u8]) -> Option<LibraryWebStub> {
    let mut pos = 0;
    if p.len() < 2 { return None; }
    let sv_len = u16::from_le_bytes([p[0], p[1]]) as usize;
    pos += 2;
    if p.len() < pos + sv_len { return None; }
    let source_video = String::from_utf8(p[pos..pos+sv_len].to_vec()).ok()?;
    pos += sv_len;
    // Need at least 4*5 + 8*2 + 112 = 148 more bytes.
    if p.len() < pos + 148 { return None; }
    let read_u32 = |o: usize| u32::from_le_bytes([p[o], p[o+1], p[o+2], p[o+3]]);
    let read_u64 = |o: usize| u64::from_le_bytes([p[o], p[o+1], p[o+2], p[o+3], p[o+4], p[o+5], p[o+6], p[o+7]]);
    let source_chunk_start = read_u32(pos); pos += 4;
    let source_chunk_end = read_u32(pos); pos += 4;
    let vertex_count = read_u32(pos); pos += 4;
    let triangle_count = read_u32(pos); pos += 4;
    let ref_count = read_u32(pos); pos += 4;
    let created_at = read_u64(pos); pos += 8;
    let last_referenced = read_u64(pos); pos += 8;
    let signature = crate::library::signature_from_bytes_pub(&p[pos..pos+SIGNATURE_BYTES])?;
    Some(LibraryWebStub {
        uuid: uuid_hex,
        source_video,
        source_chunk_start,
        source_chunk_end,
        vertex_count,
        triangle_count,
        ref_count,
        created_at,
        last_referenced,
        signature,
    })
}

/// Build a full on-wire record (header + payload) for a put.
pub fn build_put_record(stub: &LibraryWebStub) -> Result<Vec<u8>, DbError> {
    let uuid = uuid_hex_to_bytes(&stub.uuid)
        .ok_or_else(|| DbError::Corrupt(format!("bad uuid hex: {}", stub.uuid)))?;
    let payload = stub_to_payload(stub);
    Ok(assemble_record(KIND_PUT, &uuid, &payload))
}

/// Build a tombstone record for a uuid.
pub fn build_del_record(uuid_hex: &str) -> Result<Vec<u8>, DbError> {
    let uuid = uuid_hex_to_bytes(uuid_hex)
        .ok_or_else(|| DbError::Corrupt(format!("bad uuid hex: {}", uuid_hex)))?;
    Ok(assemble_record(KIND_DEL, &uuid, &[]))
}

fn assemble_record(kind: u8, uuid: &[u8; 16], payload: &[u8]) -> Vec<u8> {
    // Body = kind + uuid + payload_len + payload
    let mut body = Vec::with_capacity(1 + 16 + 4 + payload.len());
    body.push(kind);
    body.extend_from_slice(uuid);
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.extend_from_slice(payload);
    let crc = crc32(&body);
    
    let mut rec = Vec::with_capacity(8 + body.len());
    rec.extend_from_slice(RECORD_MAGIC);
    rec.extend_from_slice(&crc.to_le_bytes());
    rec.extend_from_slice(&body);
    rec
}

/// An open log database. Reads index into memory at open time; writes
/// append to disk and update the in-memory index in lockstep.
pub struct LogDb {
    pub path: PathBuf,
    /// uuid_hex → in-memory stub (last write wins)
    pub entries: HashMap<String, LibraryWebStub>,
    /// Append-only file handle for writes. None = read-only.
    write_file: Option<std::fs::File>,
    /// Tombstone count, for compaction trigger.
    tombstones: usize,
}

impl LogDb {
    /// Open or create the log at library_dir/webs.log.
    /// 
    /// On open we scan forward and replay all records into the
    /// in-memory index. If a record's CRC fails or EOF hits mid-record,
    /// we TRUNCATE the file to the last valid record (best-effort
    /// crash recovery).
    /// 
    /// If `writable` is true, the file is opened for append. Otherwise
    /// the file is opened read-only and `put`/`delete` will fail.
    pub fn open(library_dir: &Path, writable: bool) -> Result<Self, DbError> {
        let path = library_dir.join("webs.log");
        let mut entries: HashMap<String, LibraryWebStub> = HashMap::new();
        let mut tombstones = 0usize;
        let mut last_good_end: u64 = 0;
        
        if path.exists() {
            // Read the whole file at once into a Vec<u8>. For large 
            // libraries (10MB+ log) this is dramatically faster than 
            // seek+read_exact per record — eliminates syscall overhead 
            // and lets the OS use one optimized readahead pass.
            // 
            // Memory cost: log file size. For 100k webs that's ~30MB 
            // which is acceptable. If logs ever exceed available RAM 
            // we'd want to switch to mmap or chunked reads.
            let buf: Vec<u8> = std::fs::read(&path)?;
            let total = buf.len() as u64;
            let mut pos: u64 = 0;
            while pos + RECORD_HEADER_BYTES as u64 <= total {
                let p = pos as usize;
                let hdr = &buf[p..p + RECORD_HEADER_BYTES];
                if &hdr[0..4] != RECORD_MAGIC {
                    // Bad magic — truncate from here.
                    break;
                }
                let stored_crc = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
                let kind = hdr[8];
                let mut uuid_bytes = [0u8; 16];
                uuid_bytes.copy_from_slice(&hdr[9..25]);
                let payload_len = u32::from_le_bytes([hdr[25], hdr[26], hdr[27], hdr[28]]) as usize;
                
                // Sanity-cap on payload to avoid OOM if record corrupt.
                if payload_len > 1_000_000 { break; }
                if pos + RECORD_HEADER_BYTES as u64 + payload_len as u64 > total { break; }
                
                let payload_start = p + RECORD_HEADER_BYTES;
                let payload = &buf[payload_start..payload_start + payload_len];
                
                // CRC over body = kind + uuid + len + payload. Compute 
                // without allocating a separate body buffer — feed each 
                // section to the CRC incrementally.
                // For simplicity right now we still allocate a body, 
                // but only for blocks that aren't pre-validated. Future: 
                // crc32_chunks() helper that hashes pieces.
                let mut body = Vec::with_capacity(1 + 16 + 4 + payload_len);
                body.push(kind);
                body.extend_from_slice(&uuid_bytes);
                body.extend_from_slice(&(payload_len as u32).to_le_bytes());
                body.extend_from_slice(payload);
                let computed_crc = crc32(&body);
                if computed_crc != stored_crc {
                    // CRC mismatch — this record (and everything after) is corrupt.
                    eprintln!("[library_db] CRC mismatch at offset {} — truncating remainder", pos);
                    break;
                }
                
                let uuid_hex = uuid_bytes_to_hex(&uuid_bytes);
                match kind {
                    KIND_PUT => {
                        if let Some(stub) = payload_to_stub(uuid_hex.clone(), &payload) {
                            entries.insert(uuid_hex, stub);
                        } else {
                            // Bad payload — skip the record but trust CRC said body is well-formed.
                            eprintln!("[library_db] could not decode payload for uuid {} (record skipped)", uuid_hex);
                        }
                    }
                    KIND_DEL => {
                        entries.remove(&uuid_hex);
                        tombstones += 1;
                    }
                    _ => {
                        eprintln!("[library_db] unknown record kind {} at offset {} — truncating", kind, pos);
                        break;
                    }
                }
                pos += RECORD_HEADER_BYTES as u64 + payload_len as u64;
                last_good_end = pos;
            }
            // Truncate any trailing junk.
            if last_good_end < total {
                eprintln!("[library_db] truncating {} bytes of trailing junk", total - last_good_end);
                if writable {
                    let f = std::fs::OpenOptions::new().write(true).open(&path)?;
                    f.set_len(last_good_end)?;
                }
            }
        }
        
        let write_file = if writable {
            Some(std::fs::OpenOptions::new().create(true).append(true).open(&path)?)
        } else {
            None
        };
        
        Ok(LogDb { path, entries, write_file, tombstones })
    }
    
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn get(&self, uuid_hex: &str) -> Option<&LibraryWebStub> { self.entries.get(uuid_hex) }
    pub fn all(&self) -> impl Iterator<Item = &LibraryWebStub> { self.entries.values() }
    
    /// Append a put record. Updates the in-memory index. fsync is
    /// caller's choice via `flush`.
    pub fn put(&mut self, stub: &LibraryWebStub) -> Result<(), DbError> {
        let f = self.write_file.as_mut().ok_or_else(|| DbError::Corrupt("read-only".into()))?;
        let rec = build_put_record(stub)?;
        f.write_all(&rec)?;
        self.entries.insert(stub.uuid.clone(), stub.clone());
        Ok(())
    }
    
    /// Append a tombstone. The uuid disappears from the in-memory index.
    pub fn delete(&mut self, uuid_hex: &str) -> Result<(), DbError> {
        let f = self.write_file.as_mut().ok_or_else(|| DbError::Corrupt("read-only".into()))?;
        let rec = build_del_record(uuid_hex)?;
        f.write_all(&rec)?;
        if self.entries.remove(uuid_hex).is_some() {
            self.tombstones += 1;
        }
        Ok(())
    }
    
    /// Fsync the underlying file. Caller decides cadence (after batch
    /// of puts, after each put, etc).
    pub fn flush(&mut self) -> Result<(), DbError> {
        if let Some(f) = self.write_file.as_mut() {
            f.sync_all()?;
        }
        Ok(())
    }
    
    /// Compact: rewrite the log with only live entries, rotate the old
    /// file into the backup slot 0, shift older backups down. Best-effort 
    /// — failures during compaction leave the original log intact 
    /// (compact file is named .compact and only renamed at the end).
    pub fn compact(&mut self) -> Result<(), DbError> {
        let parent = self.path.parent()
            .ok_or_else(|| DbError::Corrupt("no parent dir".into()))?
            .to_path_buf();
        let compact_path = parent.join("webs.log.compact");
        
        // Write all live entries to the compact file.
        {
            let mut cf = std::fs::OpenOptions::new()
                .create(true).write(true).truncate(true)
                .open(&compact_path)?;
            for stub in self.entries.values() {
                let rec = build_put_record(stub)?;
                cf.write_all(&rec)?;
            }
            cf.sync_all()?;
        }
        
        // Close current write file (will reopen after rotate).
        self.write_file = None;
        
        // Rotate backups: .bak.3 → .bak.4, .bak.2 → .bak.3, ..., .bak.0 → .bak.1.
        for n in (1..5).rev() {
            let src = parent.join(format!("webs.log.bak.{}", n - 1));
            let dst = parent.join(format!("webs.log.bak.{}", n));
            if src.exists() {
                let _ = std::fs::rename(&src, &dst);
            }
        }
        // Move current → .bak.0.
        let bak0 = parent.join("webs.log.bak.0");
        std::fs::rename(&self.path, &bak0)?;
        // Move compact → current.
        std::fs::rename(&compact_path, &self.path)?;
        
        // Reopen for writes.
        self.write_file = Some(std::fs::OpenOptions::new()
            .create(true).append(true).open(&self.path)?);
        self.tombstones = 0;
        
        // fsync parent dir for durability of the renames.
        if let Ok(dir) = std::fs::File::open(&parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }
    
    /// Compaction trigger: tombstones exceed 25% of total records.
    pub fn should_compact(&self) -> bool {
        let live = self.entries.len();
        let total = live + self.tombstones;
        total > 100 && self.tombstones * 4 > total
    }
    
    /// Range-scan entries sorted by (eigenvalue 0, eigenvalue 1) for
    /// similarity searches. Builds a transient BTreeMap each call. O(n log n).
    /// Not blazingly fast but good enough until we add a proper spatial index.
    pub fn by_signature_eig0(&self) -> BTreeMap<u32, &LibraryWebStub> {
        let mut out = BTreeMap::new();
        for stub in self.entries.values() {
            // Use eigenvalue 0 as the sort key, encoded as u32 bit pattern
            // (lexicographic over the bit pattern = monotonic over positive 
            // f32 values, which eigenvalues always are).
            let key = stub.signature.eigenvalues[0].to_bits();
            out.insert(key, stub);
        }
        out
    }
}

/// Rollback the log to one of the .bak.N slots. Same UX as the JSON
/// manifest's rollback: 0 = most recent, 4 = oldest.
pub fn rollback_log(library_dir: &Path, slot: usize) -> Result<(), DbError> {
    if slot >= 5 {
        return Err(DbError::Corrupt(format!("slot {} out of range (0..5)", slot)));
    }
    let bak = library_dir.join(format!("webs.log.bak.{}", slot));
    let current = library_dir.join("webs.log");
    if !bak.exists() {
        return Err(DbError::NotFound);
    }
    
    // Validate the backup parses cleanly before swapping. Same logic
    // as LogDb::open but in dry-run.
    let mut f = std::fs::File::open(&bak)?;
    let total = f.metadata()?.len();
    let mut pos = 0u64;
    while pos + RECORD_HEADER_BYTES as u64 <= total {
        f.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; RECORD_HEADER_BYTES];
        if f.read_exact(&mut hdr).is_err() { break; }
        if &hdr[0..4] != RECORD_MAGIC { break; }
        let payload_len = u32::from_le_bytes([hdr[25], hdr[26], hdr[27], hdr[28]]) as usize;
        if payload_len > 1_000_000 { break; }
        if pos + RECORD_HEADER_BYTES as u64 + payload_len as u64 > total { break; }
        pos += RECORD_HEADER_BYTES as u64 + payload_len as u64;
    }
    // Don't require ALL records to be valid — just that we can open it.
    // (Backups may be from before a corruption was detected.)
    
    // Rotate current into a new .bak.0 slot first so rollback is itself
    // reversible.
    let parent = library_dir.to_path_buf();
    for n in (1..5).rev() {
        let src = parent.join(format!("webs.log.bak.{}", n - 1));
        let dst = parent.join(format!("webs.log.bak.{}", n));
        if src.exists() {
            let _ = std::fs::rename(&src, &dst);
        }
    }
    if current.exists() {
        // The current state goes to .bak.0. The slot the user picked
        // gets restored as current.
        // 
        // Subtle: we just shifted everyone down, so the file the user
        // picked is now at slot+1. Read from there.
        let chosen = parent.join(format!("webs.log.bak.{}", slot + 1));
        // Push current to .bak.0 by copy (so .bak.0 reflects "what was
        // current before rollback").
        std::fs::copy(&current, &parent.join("webs.log.bak.0"))?;
        if !chosen.exists() {
            return Err(DbError::Corrupt(format!(
                "expected backup at slot {} after rotation but not found",
                slot + 1
            )));
        }
        std::fs::copy(&chosen, &current)?;
    } else {
        std::fs::copy(&bak, &current)?;
    }
    
    // fsync parent dir.
    if let Ok(dir) = std::fs::File::open(&parent) {
        let _ = dir.sync_all();
    }
    
    eprintln!("[library_db] rolled back log from slot {}", slot);
    Ok(())
}

/// Synthetic benchmark for the log DB. Public so it can be invoked 
/// from the GUI for live perf demos.
/// 
/// Creates a temp directory, fills the log with N synthetic webs, 
/// times the open scan, then times M random gets. Returns a report 
/// string for the GUI to display.
pub fn bench(n_entries: usize, n_gets: usize) -> String {
    use std::time::Instant;
    let tmp = std::env::temp_dir().join(format!("fao_db_bench_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);
    
    // Phase 1: write n_entries records.
    let write_start = Instant::now();
    {
        let mut db = match LogDb::open(&tmp, true) {
            Ok(d) => d,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                return format!("bench: open failed — {}", e);
            }
        };
        for i in 0..n_entries {
            let mut uuid = [0u8; 16];
            uuid[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            uuid[8..16].copy_from_slice(&(0xdeadbeefu64 ^ i as u64).to_le_bytes());
            let stub = LibraryWebStub {
                uuid: uuid_bytes_to_hex(&uuid),
                source_video: format!("synthetic_{}.mp4", i),
                source_chunk_start: i as u32,
                source_chunk_end: i as u32 + 100,
                vertex_count: 1000 + (i as u32 * 13) % 5000,
                triangle_count: 2000 + (i as u32 * 17) % 10000,
                ref_count: 1,
                created_at: i as u64,
                last_referenced: i as u64,
                signature: WebSignature::default(),
            };
            if let Err(e) = db.put(&stub) {
                let _ = std::fs::remove_dir_all(&tmp);
                return format!("bench: put failed at {} — {}", i, e);
            }
        }
        if let Err(e) = db.flush() {
            let _ = std::fs::remove_dir_all(&tmp);
            return format!("bench: flush failed — {}", e);
        }
    }
    let write_ms = write_start.elapsed().as_secs_f64() * 1000.0;
    
    // Phase 2: open + scan.
    let open_start = Instant::now();
    let db = match LogDb::open(&tmp, false) {
        Ok(d) => d,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return format!("bench: reopen failed — {}", e);
        }
    };
    let open_ms = open_start.elapsed().as_secs_f64() * 1000.0;
    if db.len() != n_entries {
        let _ = std::fs::remove_dir_all(&tmp);
        return format!("bench: reopen got {} entries, expected {}", db.len(), n_entries);
    }
    
    // Phase 3: random gets.
    let get_start = Instant::now();
    let mut hits = 0;
    for i in 0..n_gets {
        let idx = (i * 7919) % n_entries; // pseudo-random
        let mut uuid_b = [0u8; 16];
        uuid_b[0..8].copy_from_slice(&(idx as u64).to_le_bytes());
        uuid_b[8..16].copy_from_slice(&(0xdeadbeefu64 ^ idx as u64).to_le_bytes());
        let hex = uuid_bytes_to_hex(&uuid_b);
        if db.get(&hex).is_some() {
            hits += 1;
        }
    }
    let get_ms = get_start.elapsed().as_secs_f64() * 1000.0;
    
    // On-disk size.
    let log_size = std::fs::metadata(tmp.join("webs.log")).map(|m| m.len()).unwrap_or(0);
    
    let _ = std::fs::remove_dir_all(&tmp);
    
    format!(
        "DB bench ({} entries):
           write: {:.1}ms ({:.1}μs/entry)
           on-disk: {:.2}MB
           open scan: {:.1}ms ({:.1}μs/entry)
           {} random gets: {:.3}ms ({:.0} ns/get, {} hits)",
        n_entries,
        write_ms, write_ms * 1000.0 / n_entries as f64,
        log_size as f64 / 1_048_576.0,
        open_ms, open_ms * 1000.0 / n_entries as f64,
        n_gets, get_ms, get_ms * 1_000_000.0 / n_gets as f64, hits,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn uuid_roundtrip() {
        let hex = "deadbeefcafebabe0123456789abcdef";
        let bytes = uuid_hex_to_bytes(hex).unwrap();
        let back = uuid_bytes_to_hex(&bytes);
        assert_eq!(back, hex);
    }
    
    #[test]
    fn crc_known_value() {
        // Known CRC32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }
    
    #[test]
    fn bench_sanity() {
        // Quick sanity check that bench() returns sensible output 
        // structure without measuring actual perf (CI variance too high 
        // to assert numbers). Just confirms the round trip works.
        let out = bench(100, 100);
        assert!(out.contains("100 entries"));
        assert!(out.contains("100 random gets"));
        assert!(!out.contains("failed"));
    }
}
