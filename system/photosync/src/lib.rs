// PhotoSync Deduplication Layer - Enhanced with XXH64 and content-defined chunking
// Core library for block-level deduplication with multiple hash algorithms

use std::fs;
use std::io::{self, Read, Write, BufReader};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use rusqlite::{Connection, params, OptionalExtension};
use sha2::{Sha256, Digest};
use xxhash_rust::xxh64::xxh64;
use zstd;

// Chunking strategies
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChunkingStrategy {
    FixedSize,           // Simple fixed-size chunks (1 MiB default)
    ContentDefined,      // CDC using rolling hash (better dedup)
}

// Hash algorithm selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HashAlgorithm {
    XXH64,      // Fast, non-cryptographic (default for speed)
    SHA256,     // Slower, cryptographic (for integrity verification)
}

/// What a device is allowed to do against this library's shared storage.
/// Ordered — `Ord` follows declaration order below, so `a >= Permission::Contribute`
/// is a real capability check, not just an equality test. Per-device, not
/// per-file: a shared photo library's access control is "can this person
/// touch the library at all", not per-photo ACLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    /// Can list and fetch (files, thumbnails). Cannot add or remove anything.
    ReadOnly,
    /// ReadOnly, plus can register new files. Cannot run `garbage_collect`
    /// or touch another device's files.
    Contribute,
    /// Full read/write, including garbage collection. Intended for a
    /// device's own primary owner, or a fully trusted co-owner.
    FullSync,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ReadOnly => "read_only",
            Permission::Contribute => "contribute",
            Permission::FullSync => "full_sync",
        }
    }
    pub fn from_str(s: &str) -> Option<Permission> {
        match s {
            "read_only" => Some(Permission::ReadOnly),
            "contribute" => Some(Permission::Contribute),
            "full_sync" => Some(Permission::FullSync),
            _ => None,
        }
    }
}

pub mod album;
pub mod exif;
pub mod gallery_grid;
pub mod peers;
pub mod chunk_sync;

// Configuration
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeduplicationConfig {
    pub chunk_size: usize,
    pub chunking_strategy: ChunkingStrategy,
    pub hash_algorithm: HashAlgorithm,
    pub compression_level: i32,
    pub min_chunk_size: usize,  // For CDC
    pub max_chunk_size: usize,  // For CDC
}

impl Default for DeduplicationConfig {
    fn default() -> Self {
        Self {
            chunk_size: 1024 * 1024,  // 1 MiB
            chunking_strategy: ChunkingStrategy::FixedSize,
            hash_algorithm: HashAlgorithm::XXH64,
            compression_level: 19,
            min_chunk_size: 256 * 1024,   // 256 KiB
            max_chunk_size: 8 * 1024 * 1024,  // 8 MiB
        }
    }
}

/// What `register_photo` records beyond the bytes.
#[derive(Debug, Clone, Copy)]
pub struct Photo<'a> {
    /// Unix seconds when the picture was taken — `exif::taken_at`, else the
    /// file's modification time. Stored as `created_at`, which the gallery
    /// groups by (`album::load`).
    pub taken_at: i64,
    /// Hash of the thumbnail already passed to `store_thumbnail`.
    pub thumbnail_hash: Option<&'a str>,
    pub metadata_json: Option<&'a str>,
}

pub struct PhotoSyncDB {
    conn: Connection,
    block_store: PathBuf,
    config: DeduplicationConfig,
}

// Content-Defined Chunking using Rabin fingerprinting
const ROLLING_WINDOW: usize = 64;
const POLYNOMIAL: u64 = 0x3DA3358B4DC173;  // Rabin polynomial

struct RollingHash {
    window: [u8; ROLLING_WINDOW],
    position: usize,
    hash: u64,
    mask: u64,  // For split points
}

impl RollingHash {
    fn new(target_size: usize) -> Self {
        // Calculate mask for average chunk size
        let bits = (target_size as f64).log2().floor() as u32;
        let mask = (1u64 << bits) - 1;
        
        Self {
            window: [0; ROLLING_WINDOW],
            position: 0,
            hash: 0,
            mask,
        }
    }
    
    fn update(&mut self, byte: u8) -> bool {
        // Remove oldest byte from hash
        let old_byte = self.window[self.position % ROLLING_WINDOW];
        
        // Rabin fingerprint update
        self.hash = self.hash.wrapping_mul(POLYNOMIAL);
        self.hash ^= old_byte as u64;
        self.hash ^= byte as u64;
        
        // Store new byte
        self.window[self.position % ROLLING_WINDOW] = byte;
        self.position += 1;
        
        // Check if this is a chunk boundary
        (self.hash & self.mask) == 0
    }
}

impl PhotoSyncDB {
    /// The open connection underneath, for callers that need `album::load`
    /// or `album::save_albums` directly. Without this, `album::load` — the
    /// function that actually builds a gallery's `Library`, complete with
    /// albums — is unreachable from anything holding a `PhotoSyncDB` (the
    /// daemon, the sync server, the CLI, and any future egui shell all do;
    /// none of them have a bare `Connection` lying around), which made it
    /// dead code outside its own tests. `list_files` below is a different,
    /// narrower cut for a different purpose — see its doc.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn new(db_path: &Path, block_store: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        fs::create_dir_all(block_store)?;
        let conn = Connection::open(db_path)?;
        
        Self::create_schema(&conn)?;
        
        Ok(Self {
            conn,
            block_store: block_store.to_path_buf(),
            config: DeduplicationConfig::default(),
        })
    }
    
    pub fn with_config(
        db_path: &Path,
        block_store: &Path,
        config: DeduplicationConfig,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut db = Self::new(db_path, block_store)?;
        db.config = config;
        Ok(db)
    }
    
    fn create_schema(conn: &Connection) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Blocks table with hash algorithm tracking
        conn.execute(
            "CREATE TABLE IF NOT EXISTS blocks (
                hash TEXT PRIMARY KEY,
                hash_algorithm TEXT NOT NULL,
                size INTEGER NOT NULL,
                compressed_size INTEGER NOT NULL,
                ref_count INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                last_accessed INTEGER NOT NULL
            )",
            [],
        )?;
        
        // Files table with chunking metadata
        conn.execute(
            "CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                chunks TEXT NOT NULL,
                chunk_count INTEGER NOT NULL,
                chunking_strategy TEXT NOT NULL,
                hash_algorithm TEXT NOT NULL,
                device_id TEXT NOT NULL,
                modified_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                thumbnail_hash TEXT,
                metadata_json TEXT
            )",
            [],
        )?;
        
        // Block locations for deduplication tracking
        conn.execute(
            "CREATE TABLE IF NOT EXISTS block_locations (
                hash TEXT NOT NULL,
                file_path TEXT NOT NULL,
                chunk_index INTEGER NOT NULL,
                FOREIGN KEY (hash) REFERENCES blocks(hash),
                FOREIGN KEY (file_path) REFERENCES files(path),
                PRIMARY KEY (hash, file_path, chunk_index)
            )",
            [],
        )?;
        
        // Thumbnails
        conn.execute(
            "CREATE TABLE IF NOT EXISTS thumbnails (
                hash TEXT PRIMARY KEY,
                data BLOB NOT NULL,
                width INTEGER NOT NULL,
                height INTEGER NOT NULL,
                format TEXT NOT NULL
            )",
            [],
        )?;
        
        // Per-device access level for this library's shared storage — see
        // `Permission`. No row for a device means it has no access at all
        // (not `ReadOnly` — an explicit grant is required either way, but
        // this makes "never granted" and "granted ReadOnly" distinguishable
        // for an audit trail rather than both reading as "the weakest tier").
        conn.execute(
            "CREATE TABLE IF NOT EXISTS permissions (
                device_id TEXT PRIMARY KEY,
                level TEXT NOT NULL,
                granted_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        // Indexes for performance
        conn.execute("CREATE INDEX IF NOT EXISTS idx_files_device ON files(device_id)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_blocks_refcount ON blocks(ref_count)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_blocks_accessed ON blocks(last_accessed)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_block_locations_file ON block_locations(file_path)", [])?;
        
        Ok(())
    }
    
    pub fn chunk_and_store(&mut self, file_path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        println!("[Dedup] Processing file: {:?}", file_path);
        println!("[Dedup] Strategy: {:?}", self.config.chunking_strategy);
        println!("[Dedup] Hash algorithm: {:?}", self.config.hash_algorithm);
        
        let chunks = match self.config.chunking_strategy {
            ChunkingStrategy::FixedSize => self.chunk_fixed_size(file_path)?,
            ChunkingStrategy::ContentDefined => self.chunk_content_defined(file_path)?,
        };
        
        println!("[Dedup] File chunked into {} blocks", chunks.len());
        
        let mut chunk_hashes = Vec::new();
        let mut new_blocks = 0;
        let mut existing_blocks = 0;
        
        for chunk in chunks {
            let hash = self.hash_chunk(&chunk);
            
            if !self.block_exists(&hash)? {
                self.store_block(&hash, &chunk)?;
                new_blocks += 1;
            } else {
                self.increment_ref_count(&hash)?;
                existing_blocks += 1;
            }
            
            chunk_hashes.push(hash);
        }
        
        println!("[Dedup] New blocks: {}, Existing blocks: {}", new_blocks, existing_blocks);
        if existing_blocks > 0 {
            let dedup_ratio = (existing_blocks as f64 / (new_blocks + existing_blocks) as f64) * 100.0;
            println!("[Dedup] Deduplication ratio: {:.1}%", dedup_ratio);
        }
        
        Ok(chunk_hashes)
    }
    
    fn chunk_fixed_size(&self, file_path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
        let mut file = fs::File::open(file_path)?;
        let mut chunks = Vec::new();
        let mut buffer = vec![0u8; self.config.chunk_size];
        
        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            chunks.push(buffer[..bytes_read].to_vec());
        }
        
        Ok(chunks)
    }
    
    fn chunk_content_defined(&self, file_path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
        let file = fs::File::open(file_path)?;
        let mut reader = BufReader::new(file);
        let mut chunks = Vec::new();
        let mut current_chunk = Vec::new();
        let mut rolling_hash = RollingHash::new(self.config.chunk_size);
        
        let mut buffer = [0u8; 1];
        
        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                if !current_chunk.is_empty() {
                    chunks.push(current_chunk);
                }
                break;
            }
            
            let byte = buffer[0];
            current_chunk.push(byte);
            
            let is_boundary = rolling_hash.update(byte);
            
            if is_boundary && current_chunk.len() >= self.config.min_chunk_size {
                chunks.push(current_chunk);
                current_chunk = Vec::new();
            }
            
            if current_chunk.len() >= self.config.max_chunk_size {
                chunks.push(current_chunk);
                current_chunk = Vec::new();
            }
        }
        
        Ok(chunks)
    }
    
    fn hash_chunk(&self, chunk: &[u8]) -> String {
        match self.config.hash_algorithm {
            HashAlgorithm::XXH64 => {
                let hash = xxh64(chunk, 0);
                format!("xxh64:{:016x}", hash)
            }
            HashAlgorithm::SHA256 => {
                let mut hasher = Sha256::new();
                hasher.update(chunk);
                format!("sha256:{:x}", hasher.finalize())
            }
        }
    }
    
    fn block_exists(&self, hash: &str) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut stmt = self.conn.prepare("SELECT 1 FROM blocks WHERE hash = ?")?;
        Ok(stmt.exists(params![hash])?)
    }
    
    fn store_block(&mut self, hash: &str, data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let compressed = zstd::encode_all(data, self.config.compression_level)?;
        let block_path = self.block_store.join(hash.replace(":", "_"));
        
        let mut file = fs::File::create(&block_path)?;
        file.write_all(&compressed)?;
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        
        let hash_algorithm = hash.split(':').next().unwrap_or("unknown");
        
        self.conn.execute(
            "INSERT INTO blocks (hash, hash_algorithm, size, compressed_size, ref_count, created_at, last_accessed) 
             VALUES (?, ?, ?, ?, 1, ?, ?)",
            params![hash, hash_algorithm, data.len() as i64, compressed.len() as i64, timestamp, timestamp],
        )?;
        
        Ok(())
    }
    
    fn increment_ref_count(&mut self, hash: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        
        self.conn.execute(
            "UPDATE blocks SET ref_count = ref_count + 1, last_accessed = ? WHERE hash = ?",
            params![timestamp, hash],
        )?;
        Ok(())
    }
    
    pub fn reconstruct_file(&self, chunk_hashes: &[String], output_path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut output = fs::File::create(output_path)?;
        
        for hash in chunk_hashes {
            let block_path = self.block_store.join(hash.replace(":", "_"));
            let compressed = fs::read(&block_path)?;
            let decompressed = zstd::decode_all(&compressed[..])?;
            output.write_all(&decompressed)?;
        }
        
        Ok(())
    }
    
    pub fn register_file(
        &mut self,
        path: &str,
        size: i64,
        chunks: &[String],
        device_id: &str,
        metadata_json: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        self.register_photo(path, size, chunks, device_id, &Photo { taken_at: now, thumbnail_hash: None, metadata_json })
    }

    /// Import one photo the way the daemon does: store its thumbnail, chunk
    /// and deduplicate its bytes, and register it dated by when it was TAKEN
    /// (`exif::taken_at`, else the file's modification time) with its
    /// thumbnail hash in `thumbnail_hash`, where `album::load` reads it.
    ///
    /// Lived in the daemon binary, untested, which is how it came to pass the
    /// thumbnail hash in `register_file`'s METADATA position: every imported
    /// photo had `thumbnail_hash` NULL, a hash string for metadata, and
    /// `created_at` set to the import time — so the gallery filed a whole
    /// library under the day it was imported and never found a stored
    /// thumbnail.
    ///
    /// The thumbnail is made from the sensor pixels, as before (EXIF
    /// orientation not applied; the JPEG re-encode drops it). The gallery
    /// turns it using the original's header, which is right for thumbnails
    /// already stored by older daemons too.
    pub fn import_photo(&mut self, image_path: &Path, device_id: &str, thumbnail_size: u32)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let thumbnail_hash = image::open(image_path).ok().and_then(|img| {
            let thumb = img.thumbnail(thumbnail_size, thumbnail_size);
            let mut buf = Vec::new();
            thumb.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg).ok()?;
            let hash = format!("{:x}", Sha256::digest(&buf));
            self.store_thumbnail(&hash, &buf, img.width(), img.height()).ok()?;
            Some(hash)
        });
        let meta = fs::metadata(image_path)?;
        let mut head = Vec::new();
        fs::File::open(image_path)?.take(256 * 1024).read_to_end(&mut head)?;
        let taken_at = exif::taken_at(&head)
            .or_else(|| meta.modified().ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64))
            .unwrap_or(0);
        let chunks = self.chunk_and_store(image_path)?;
        self.register_photo(&image_path.to_string_lossy(), meta.len() as i64, &chunks, device_id,
            &Photo { taken_at, thumbnail_hash: thumbnail_hash.as_deref(), metadata_json: None })
    }

    /// Register a file with what the gallery shows: when it was TAKEN and
    /// its stored thumbnail. `register_file` has neither — it stamps
    /// `created_at` with the time of the call and leaves `thumbnail_hash`
    /// empty, which is right for a sync upload that carries no pixels and
    /// wrong for the daemon, whose every photo then filed under the day it
    /// was imported, with no thumbnail the gallery could find.
    pub fn register_photo(
        &mut self,
        path: &str,
        size: i64,
        chunks: &[String],
        device_id: &str,
        photo: &Photo,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let (created_at, thumbnail_hash, metadata_json) = (photo.taken_at, photo.thumbnail_hash, photo.metadata_json);
        
        let chunks_json = serde_json::to_string(chunks)?;
        let chunking_strategy = format!("{:?}", self.config.chunking_strategy);
        let hash_algorithm = format!("{:?}", self.config.hash_algorithm);
        
        self.conn.execute(
            "INSERT OR REPLACE INTO files 
             (path, size, chunks, chunk_count, chunking_strategy, hash_algorithm, 
              device_id, modified_at, created_at, thumbnail_hash, metadata_json) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                path, size, chunks_json, chunks.len() as i64,
                chunking_strategy, hash_algorithm,
                device_id, timestamp, created_at, thumbnail_hash, metadata_json
            ],
        )?;
        
        for (idx, hash) in chunks.iter().enumerate() {
            self.conn.execute(
                "INSERT OR REPLACE INTO block_locations (hash, file_path, chunk_index) VALUES (?, ?, ?)",
                params![hash, path, idx as i64],
            )?;
        }
        
        Ok(())
    }

    /// Every registered file, newest-taken first — for sync/replication
    /// callers that want "what files exist" without albums (a peer
    /// deciding what to pull, `peers.rs`'s eventual consumer). For gallery
    /// display, use `album::load(db.connection())` instead: it returns the
    /// same rows shaped as `album::Item` and also loads albums, which this
    /// deliberately doesn't.
    pub fn list_files(&self) -> Result<Vec<FileRecord>, Box<dyn std::error::Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, size, device_id, modified_at, created_at, thumbnail_hash, metadata_json
             FROM files ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FileRecord {
                path: row.get(0)?,
                size: row.get(1)?,
                device_id: row.get(2)?,
                modified_at: row.get(3)?,
                created_at: row.get(4)?,
                thumbnail_hash: row.get(5)?,
                metadata_json: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Grant (or replace) a device's access level. Re-granting the same
    /// device updates `granted_at` — there's no history kept here; an
    /// audit log of grants, if wanted, is a different table wrapping this
    /// one, not a job for the permission check itself.
    pub fn set_permission(&mut self, device_id: &str, level: Permission) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        self.conn.execute(
            "INSERT INTO permissions (device_id, level, granted_at) VALUES (?, ?, ?)
             ON CONFLICT(device_id) DO UPDATE SET level = excluded.level, granted_at = excluded.granted_at",
            params![device_id, level.as_str(), timestamp],
        )?;
        Ok(())
    }

    /// `None` means no grant exists — see the schema note on `permissions`
    /// for why that is kept distinct from `Some(Permission::ReadOnly)`.
    pub fn permission_for(&self, device_id: &str) -> Result<Option<Permission>, Box<dyn std::error::Error + Send + Sync>> {
        let level: Option<String> = self.conn
            .query_row(
                "SELECT level FROM permissions WHERE device_id = ?",
                params![device_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(level.and_then(|s| Permission::from_str(&s)))
    }

    /// Whether `device_id` currently holds at least `required`. This is a
    /// query, not an enforcement point — callers (a future sync server's
    /// request handler is the obvious one; see `sync_server.rs`) check it
    /// before acting rather than it being threaded into `register_file` or
    /// `garbage_collect` themselves, so a purely local, single-device use of
    /// this library (the CLI, today) is never blocked by a permission
    /// system it never asked for.
    pub fn authorize(&self, device_id: &str, required: Permission) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.permission_for(device_id)?.map_or(false, |granted| granted >= required))
    }
    
    pub fn get_storage_stats(&self) -> Result<StorageStats, Box<dyn std::error::Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "SELECT SUM(size), SUM(compressed_size), COUNT(*), SUM(ref_count) FROM blocks"
        )?;
        
        let mut rows = stmt.query([])?;
        
        if let Some(row) = rows.next()? {
            Ok(StorageStats {
                total_size: row.get(0).unwrap_or(0),
                compressed_size: row.get(1).unwrap_or(0),
                block_count: row.get(2).unwrap_or(0),
                total_refs: row.get(3).unwrap_or(0),
            })
        } else {
            Ok(StorageStats::default())
        }
    }
    
    pub fn garbage_collect(&mut self, dry_run: bool) -> Result<GCStats, Box<dyn std::error::Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "SELECT hash, size, compressed_size FROM blocks WHERE ref_count = 0"
        )?;
        
        let unreferenced: Vec<(String, i64, i64)> = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
        
        let mut stats = GCStats::default();
        
        for (hash, size, compressed_size) in unreferenced {
            if !dry_run {
                let block_path = self.block_store.join(hash.replace(":", "_"));
                if block_path.exists() {
                    fs::remove_file(&block_path)?;
                }
                self.conn.execute("DELETE FROM blocks WHERE hash = ?", params![hash])?;
            }
            
            stats.blocks_removed += 1;
            stats.space_reclaimed += size;
            stats.compressed_space_reclaimed += compressed_size;
        }
        
        Ok(stats)
    }
    
    pub fn store_thumbnail(&mut self, hash: &str, data: &[u8], width: u32, height: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.conn.execute(
            "INSERT OR REPLACE INTO thumbnails (hash, data, width, height, format) VALUES (?, ?, ?, ?, 'jpeg')",
            params![hash, data, width as i64, height as i64],
        )?;
        Ok(())
    }
    
    pub fn get_thumbnail(&self, hash: &str) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
        let mut stmt = self.conn.prepare("SELECT data FROM thumbnails WHERE hash = ?")?;
        let mut rows = stmt.query(params![hash])?;
        
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Default)]
pub struct StorageStats {
    pub total_size: i64,
    pub compressed_size: i64,
    pub block_count: i64,
    pub total_refs: i64,
}

impl StorageStats {
    pub fn compression_ratio(&self) -> f64 {
        if self.total_size > 0 {
            (self.compressed_size as f64 / self.total_size as f64) * 100.0
        } else {
            0.0
        }
    }
    
    pub fn space_saved(&self) -> i64 {
        self.total_size - self.compressed_size
    }
    
    pub fn deduplication_ratio(&self) -> f64 {
        if self.total_refs > 0 {
            (1.0 - (self.block_count as f64 / self.total_refs as f64)) * 100.0
        } else {
            0.0
        }
    }
}

/// One `files` row, as read back — see `PhotoSyncDB::list_files`. Keeps the
/// chunk list and hashing metadata out (a reader that wants to reconstruct
/// bytes goes back through `PhotoSyncDB` for that); this is just what a
/// gallery or a sync peer needs to know a file exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    pub path: String,
    pub size: i64,
    pub device_id: String,
    pub modified_at: i64,
    pub created_at: i64,
    pub thumbnail_hash: Option<String>,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Default)]
pub struct GCStats {
    pub blocks_removed: i64,
    pub space_reclaimed: i64,
    pub compressed_space_reclaimed: i64,
}

#[cfg(test)]
mod import_tests {
    use super::*;

    /// What the daemon writes is what the gallery reads: a photo's
    /// `created_at` is when it was TAKEN, and `thumbnail_hash` names a stored
    /// thumbnail — checked through `album::load`, the gallery's own reader.
    #[test]
    fn an_imported_photo_is_dated_by_exif_and_its_thumbnail_is_findable() {
        let dir = std::env::temp_dir().join(format!("photosync-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("blocks")).unwrap();
        let mut db = PhotoSyncDB::new(&dir.join("p.db"), &dir.join("blocks")).unwrap();

        // A JPEG taken 2024-07-14 21:30:05 on the camera's clock.
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image::RgbImage::from_pixel(640, 480, image::Rgb([200, 40, 40]))).unwrap();
        let mut with_exif = jpeg[..2].to_vec();
        with_exif.extend(exif::app1_segment(6, Some("2024:07:14 21:30:05"), None, false));
        with_exif.extend_from_slice(&jpeg[2..]);
        let shot = dir.join("IMG_0001.jpg");
        fs::write(&shot, &with_exif).unwrap();

        // A PNG with no EXIF, last modified 2020-01-02 03:04:05 UTC.
        let png = dir.join("scan.png");
        image::RgbImage::from_pixel(300, 200, image::Rgb([40, 40, 200])).save(&png).unwrap();
        let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_934_245);
        fs::File::options().write(true).open(&png).unwrap().set_modified(mtime).unwrap();

        db.import_photo(&shot, "phone", 256).unwrap();
        db.import_photo(&png, "phone", 256).unwrap();

        let lib = album::load(db.connection()).unwrap();
        let by_path = |p: &Path| lib.by_day().into_iter().flat_map(|(_, v)| v.into_iter().cloned())
            .find(|i| i.path == p.to_string_lossy()).expect("imported");
        let a = by_path(&shot);
        assert_eq!(a.day(), "2024-07-14", "dated by EXIF, not by the import");
        assert_eq!(a.taken_at, exif::parse_datetime("2024:07:14 21:30:05").unwrap());
        let hash = a.thumbnail.clone().expect("thumbnail_hash is set");
        let stored = db.get_thumbnail(&hash).unwrap().expect("and names a stored thumbnail");
        let t = image::load_from_memory(&stored).unwrap();
        assert!(t.width() <= 256 && t.height() <= 256 && t.width() > 0);
        assert_eq!(a.metadata_json, None, "the hash is no longer written as metadata");

        let b = by_path(&png);
        assert_eq!(b.taken_at, 1_577_934_245, "no EXIF: the file's modification time");
        assert!(b.thumbnail.is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
