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

// Configuration
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
    pub fn new(db_path: &Path, block_store: &Path) -> Result<Self, Box<dyn std::error::Error>> {
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
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut db = Self::new(db_path, block_store)?;
        db.config = config;
        Ok(db)
    }
    
    fn create_schema(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
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
        
        // Indexes for performance
        conn.execute("CREATE INDEX IF NOT EXISTS idx_files_device ON files(device_id)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_blocks_refcount ON blocks(ref_count)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_blocks_accessed ON blocks(last_accessed)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_block_locations_file ON block_locations(file_path)", [])?;
        
        Ok(())
    }
    
    pub fn chunk_and_store(&mut self, file_path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
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
    
    fn chunk_fixed_size(&self, file_path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
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
    
    fn chunk_content_defined(&self, file_path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
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
    
    fn block_exists(&self, hash: &str) -> Result<bool, Box<dyn std::error::Error>> {
        let mut stmt = self.conn.prepare("SELECT 1 FROM blocks WHERE hash = ?")?;
        Ok(stmt.exists(params![hash])?)
    }
    
    fn store_block(&mut self, hash: &str, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
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
    
    fn increment_ref_count(&mut self, hash: &str) -> Result<(), Box<dyn std::error::Error>> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        
        self.conn.execute(
            "UPDATE blocks SET ref_count = ref_count + 1, last_accessed = ? WHERE hash = ?",
            params![timestamp, hash],
        )?;
        Ok(())
    }
    
    pub fn reconstruct_file(&self, chunk_hashes: &[String], output_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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
    ) -> Result<(), Box<dyn std::error::Error>> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        
        let chunks_json = serde_json::to_string(chunks)?;
        let chunking_strategy = format!("{:?}", self.config.chunking_strategy);
        let hash_algorithm = format!("{:?}", self.config.hash_algorithm);
        
        self.conn.execute(
            "INSERT OR REPLACE INTO files 
             (path, size, chunks, chunk_count, chunking_strategy, hash_algorithm, 
              device_id, modified_at, created_at, metadata_json) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                path, size, chunks_json, chunks.len() as i64,
                chunking_strategy, hash_algorithm,
                device_id, timestamp, timestamp, metadata_json
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
    
    pub fn get_storage_stats(&self) -> Result<StorageStats, Box<dyn std::error::Error>> {
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
    
    pub fn garbage_collect(&mut self, dry_run: bool) -> Result<GCStats, Box<dyn std::error::Error>> {
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
    
    pub fn store_thumbnail(&mut self, hash: &str, data: &[u8], width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
        self.conn.execute(
            "INSERT OR REPLACE INTO thumbnails (hash, data, width, height, format) VALUES (?, ?, ?, ?, 'jpeg')",
            params![hash, data, width as i64, height as i64],
        )?;
        Ok(())
    }
    
    pub fn get_thumbnail(&self, hash: &str) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
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

#[derive(Debug, Default)]
pub struct GCStats {
    pub blocks_removed: i64,
    pub space_reclaimed: i64,
    pub compressed_space_reclaimed: i64,
}
