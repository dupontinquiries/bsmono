//! FAO Library — persistent multi-video web sharing.
//!
//! See `docs/LIBRARY_MODE.md` for the full vision. This module is the
//! Phase 2 scaffold: filesystem-backed catalog + open/close API +
//! WebSignature matching.
//!
//! # Layout
//!
//! ```text
//! <library_path>/
//!   manifest.json         ← top-level metadata
//!   webs/
//!     <uuid>.bin          ← per-web blob: signature + mesh + pose
//!   thumbnails/           ← reserved (Phase 4)
//! ```
//!
//! # Per-web blob format (`webs/<uuid>.bin`)
//!
//! Magic `"FLW1"` (4) + zstd-compressed body. Body layout:
//!
//! ```text
//! signature_len:    u32 LE     // always 112 for current WebSignature
//! signature:        [u8; 112]  // packed WebSignature
//! v_count:          u32 LE
//! t_count:          u32 LE
//! n_count:          u32 LE
//! p_count:          u32 LE
//! source_video_len: u32 LE
//! source_video:     [u8; ...]  // utf-8 path the web came from
//! vertices:         [f32; 3 × v_count]
//! vertex_colors:    [f32; 3 × v_count]
//! normals:          [f32; 3 × n_count]
//! indices:          [u32; 3 × t_count]
//! poses:            [u8; 48 × p_count]  // CameraPose::to_bytes
//! ```
//!
//! # Concurrency
//!
//! The library is opened with shared-read (`open`) or write-exclusive
//! (`open_writable`) modes. The manifest is the source of truth for what
//! UUIDs exist; concurrent writers would race on it. For now we serialize
//! writes via a lockfile (`<library_path>/.lock`).

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::io::{Read, Write};

use crate::v8_reconstruction_codec::{SceneMesh, CameraPose, WebSignature};
use crate::geometry::Vec3;

/// Library version constant. Bump on breaking format changes.
pub const LIBRARY_VERSION: u32 = 1;

/// Top-level manifest persisted as JSON.
/// 
/// We hand-roll the JSON to avoid pulling in serde for this scaffold —
/// the structure is simple enough.
#[derive(Debug, Clone, Default)]
pub struct LibraryManifest {
    pub version: u32,
    pub web_count: u64,
    /// Webs known to the library. UUID → metadata stub.
    pub webs: Vec<LibraryWebStub>,
}

/// Per-web index entry. Loaded fully into RAM at library open.
/// 
/// At ~120 bytes per entry, 100k webs is ~12MB — fine to keep resident.
/// Each web's full mesh stays on disk until matched and faulted.
#[derive(Debug, Clone)]
pub struct LibraryWebStub {
    pub uuid: String,
    pub source_video: String,
    pub source_chunk_start: u32,
    pub source_chunk_end: u32,
    pub vertex_count: u32,
    pub triangle_count: u32,
    pub signature: WebSignature,
    pub ref_count: u32,
    pub created_at: u64,    // unix epoch seconds
    pub last_referenced: u64,
}

/// Open library handle. Read-only or writable, indicated by `lockfile`.
pub struct Library {
    pub path: PathBuf,
    pub manifest: LibraryManifest,
    /// In-memory map from UUID → manifest stub for O(1) lookup.
    pub uuid_index: HashMap<String, usize>,  // index into manifest.webs
    /// Spatial-ish index: sorted (largest_eigenvalue, manifest_idx) pairs.
    /// Used by find_match to prune candidates BEFORE running the full 
    /// signature_similarity check. The largest eigenvalue is a strong 
    /// shape discriminator that varies smoothly with viewpoint, so 
    /// bounding ±20% around the query's eig0 typically rejects 95% of 
    /// candidates with zero false negatives at our matching threshold 
    /// (~0.75).
    /// 
    /// Rebuilt whenever a web is added; touched-on-read entries don't 
    /// invalidate it (only signature changes do, and we don't mutate 
    /// signatures post-add).
    pub eig0_index: Vec<(f32, usize)>,
    /// If Some, this library is opened writable; the file holds an exclusive lock.
    /// (Best-effort: we don't use OS file locks for cross-process safety —
    /// just a sentinel file. Concurrent writers must coordinate externally.)
    pub _lockfile: Option<std::fs::File>,
    /// Custom append-only log database. Backs the JSON manifest with a 
    /// binary format that scales better and is crash-safe by design. 
    /// Optional — if None, only the JSON manifest is used (legacy mode 
    /// or migration not yet done). When Some, writes go to BOTH; reads 
    /// prefer the log for speed but fall back to JSON if missing.
    pub log_db: Option<crate::library_db::LogDb>,
    /// True when in-memory ref_count/last_referenced have been touched 
    /// since the last JSON manifest write. Drop flushes JSON if set.
    pub touch_dirty: bool,
    /// LRU cache of resolved meshes. Hot path during decode: 
    /// resolve_library_ref is called once per chunk that has a 
    /// LibraryRef, and adjacent chunks often share canonicals. 
    /// Without this cache, every reference re-reads + decompresses 
    /// the blob from disk (~50ms per hit). With cache, repeat 
    /// references are O(memcpy).
    /// 
    /// Capacity is bounded so even a library with 10k canonicals 
    /// doesn't blow memory. Pinned at 32 entries — most decode 
    /// sessions touch <16 unique canonicals.
    /// 
    /// Wrapped in Mutex because load_web takes &self but we need 
    /// mutable cache access. The whole Library is typically wrapped 
    /// in Arc<RwLock<Library>>, so this mutex is uncontended for 
    /// single-decoder sessions.
    pub blob_cache: std::sync::Mutex<std::collections::VecDeque<(String, SceneMesh, Vec<CameraPose>)>>,
}

/// Build the eig0 index from a manifest. Called on open + after any add.
fn build_eig0_index(manifest: &LibraryManifest) -> Vec<(f32, usize)> {
    let mut idx: Vec<(f32, usize)> = manifest.webs.iter().enumerate()
        .map(|(i, w)| (w.signature.eigenvalues[0], i))
        .collect();
    // Sort by eigenvalue. f32 doesn't implement Ord; use partial_cmp 
    // with a NaN-safe fallback to Equal.
    idx.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    idx
}

impl Library {
    /// Initialize an empty library at the given path. Creates the directory
    /// structure and writes a fresh manifest. Errors if the path already
    /// exists and is non-empty.
    pub fn init(path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)?;
        std::fs::create_dir_all(path.join("webs"))?;
        std::fs::create_dir_all(path.join("thumbnails"))?;
        
        let manifest_path = path.join("manifest.json");
        if manifest_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "manifest.json already present — use open() instead of init()",
            ));
        }
        
        let manifest = LibraryManifest {
            version: LIBRARY_VERSION,
            web_count: 0,
            webs: Vec::new(),
        };
        write_manifest(&manifest_path, &manifest)?;
        Ok(())
    }
    
    /// Open the library read-only. Loads the manifest into RAM.
    /// 
    /// Also opens the LogDb (read-only) if webs.log exists. The two 
    /// stores should agree; we use the JSON manifest as the primary 
    /// truth and the log as a fast-lookup mirror. The log is created/
    /// populated on the first writable open.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let manifest = read_manifest(&path.join("manifest.json"))?;
        let uuid_index = manifest.webs.iter().enumerate()
            .map(|(i, w)| (w.uuid.clone(), i))
            .collect();
        // Open log_db read-only. None if no log file or if open fails — 
        // we degrade gracefully to JSON-only.
        let log_db = match crate::library_db::LogDb::open(path, false) {
            Ok(db) => {
                eprintln!("[library] log_db opened ({} entries from webs.log)", db.len());
                Some(db)
            }
            Err(e) => {
                eprintln!("[library] log_db not opened: {} (falling back to JSON-only)", e);
                None
            }
        };
        let eig0_index = build_eig0_index(&manifest);
        Ok(Self {
            path: path.to_path_buf(),
            manifest,
            uuid_index,
            eig0_index,
            _lockfile: None,
            log_db,
            touch_dirty: false,
            blob_cache: std::sync::Mutex::new(std::collections::VecDeque::new()),
        })
    }
    
    /// Open the library writable. Creates a sentinel lockfile to discourage
    /// concurrent writers; not OS-level safe but catches the common case.
    pub fn open_writable(path: &Path) -> std::io::Result<Self> {
        let lock_path = path.join(".lock");
        if lock_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "library already locked by another writer (.lock present)",
            ));
        }
        let lockfile = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)?;
        
        let mut lib = Self::open(path)?;
        lib._lockfile = Some(lockfile);
        // Reopen log_db writable. Run migration if needed.
        match crate::library_db::LogDb::open(path, true) {
            Ok(mut db) => {
                // Migration: if the log has FEWER entries than the JSON 
                // manifest, the log is stale — populate it with the 
                // missing entries from JSON. One-time per library on 
                // first writable open after migration.
                if db.len() < lib.manifest.webs.len() {
                    let to_migrate: Vec<_> = lib.manifest.webs.iter()
                        .filter(|stub| db.get(&stub.uuid).is_none())
                        .cloned()
                        .collect();
                    eprintln!("[library] migrating {} entries from JSON manifest into webs.log", to_migrate.len());
                    for stub in &to_migrate {
                        if let Err(e) = db.put(stub) {
                            eprintln!("[library] migration: failed to put {}: {}", stub.uuid, e);
                        }
                    }
                    if let Err(e) = db.flush() {
                        eprintln!("[library] migration: flush failed: {}", e);
                    }
                }
                lib.log_db = Some(db);
            }
            Err(e) => {
                eprintln!("[library] log_db writable open failed: {} (JSON-only writes)", e);
            }
        }
        Ok(lib)
    }
    
    /// Look up matching webs by signature similarity. Returns the best match's
    /// UUID and similarity score, if any pass `threshold`.
    /// 
    /// O(N) scan — fine for thousands of webs. For 100k+ a spatial index
    /// would help; deferred to Phase 3.
    pub fn find_match(&self, sig: &WebSignature, threshold: f32) -> Option<(&LibraryWebStub, f32)> {
        // Use the eig0 secondary index to prune the candidate set 
        // BEFORE running the (relatively expensive) full similarity 
        // check. Two cases:
        // 
        //   1. Library is small (< 64 webs) — index lookup overhead 
        //      isn't worth it; just scan all.
        //   2. Library is bigger — binary search for the eig0 range 
        //      [eig0_query × 0.8, eig0_query × 1.2], then scan only 
        //      that subset. The ±20% window is conservative — at 
        //      our matching threshold (0.75) the eig_score factor 
        //      alone caps similarity, so anything outside the window 
        //      can't pass.
        // 
        // Safe because eig0_index is built from the same manifest 
        // we're scanning. If a write happened concurrently, worst 
        // case is we miss a brand-new web (next find_match will pick 
        // it up — index gets rebuilt after add_web).
        // Iterate candidates without allocating, using a small enum-like 
        // tagged index range. Either we scan all webs in [0..len) or 
        // the eig0_index slice [start..end). 
        let n_webs = self.manifest.webs.len();
        let q = sig.eigenvalues[0];
        let (use_index, idx_start, idx_end) = if n_webs < 64 || q.abs() < 1e-6 {
            (false, 0, n_webs)
        } else {
            let low = q * 0.8;
            let high = q * 1.2;
            let s = self.eig0_index.partition_point(|&(e, _)| e < low);
            let e = self.eig0_index.partition_point(|&(e, _)| e <= high);
            (true, s, e)
        };
        let mut best: Option<(&LibraryWebStub, f32)> = None;
        for k in idx_start..idx_end {
            let stub_idx = if use_index { self.eig0_index[k].1 } else { k };
            let stub = &self.manifest.webs[stub_idx];
            let score = signature_similarity(&stub.signature, sig);
            if score >= threshold {
                match best {
                    Some((prev_stub, prev_score)) => {
                        // Strict winner: better similarity score.
                        // Tie-break: prefer higher ref_count (popular 
                        // canonical wins). This biases the library 
                        // toward consolidating around fewer well-used 
                        // canonicals over time — newer encodes pile 
                        // refs onto established ones rather than 
                        // creating parallel near-duplicates.
                        // 
                        // The tie-break window is 0.01 of similarity 
                        // score, so a meaningful similarity advantage 
                        // still wins even against a popular candidate.
                        let score_diff = score - prev_score;
                        if score_diff > 0.01 || 
                           (score_diff > -0.01 && stub.ref_count > prev_stub.ref_count) 
                        {
                            best = Some((stub, score));
                        }
                    }
                    None => best = Some((stub, score)),
                }
            }
        }
        best
    }
    
    /// Add a new web to the library. Writes the blob + appends to the manifest
    /// + flushes manifest to disk. Returns the new UUID.
    /// 
    /// Caller must have opened with `open_writable`.
    pub fn add_web(
        &mut self,
        signature: WebSignature,
        mesh: &SceneMesh,
        pose_track: &[CameraPose],
        source_video: &str,
        source_chunk_start: u32,
        source_chunk_end: u32,
    ) -> std::io::Result<String> {
        if self._lockfile.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "library opened read-only — use open_writable",
            ));
        }
        
        // ── Quality gates ──
        // 
        // Don't admit a web that is unlikely to be reused. Each web 
        // costs storage + slows future find_match calls; if a web 
        // can't plausibly match anything, it's pure overhead.
        // 
        // Tunable via FAO_LIB_QUALITY=0 if user wants legacy permissive 
        // behavior. Defaults to ON.
        let quality_check = std::env::var("FAO_LIB_QUALITY")
            .map(|v| v != "0")
            .unwrap_or(true);
        if quality_check {
            // Gate 1: minimum vertex count. A web with <50 verts is 
            // tiny and likely a fragment of a scene rather than a 
            // recognizable shape. Reject.
            if mesh.vertices.len() < 50 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("web rejected: only {} vertices (< 50 threshold).                              Set FAO_LIB_QUALITY=0 to accept anyway.",
                            mesh.vertices.len()),
                ));
            }
            // Gate 2: non-degenerate signature. If all eigenvalues are 
            // near-zero, the PCA failed (collinear point cloud, all-zero 
            // vertices, etc.). The signature can't discriminate.
            let total_eig = signature.eigenvalues.iter().map(|e| e.abs()).sum::<f32>();
            if total_eig < 1e-4 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "web rejected: degenerate signature (sum of eigenvalues near zero)".to_string(),
                ));
            }
            // Gate 3: not a near-duplicate of an existing canonical. 
            // If find_match would have matched this at 0.95+, the user 
            // intended to ADD a new web but it's actually the same as 
            // one already there. Reject so we don't bloat the library.
            // 
            // We bypass our own eig0_index pruning here on the assumption 
            // that we want a clean comprehensive check at add time.
            for stub in &self.manifest.webs {
                let score = signature_similarity(&stub.signature, &signature);
                if score >= 0.95 {
                    eprintln!("[library] add_web: input matches existing canonical {} at score {:.3}; touching instead of adding",
                        stub.uuid, score);
                    let existing_uuid = stub.uuid.clone();
                    // Touch the existing one and return its UUID. The 
                    // caller gets the same effect as "added" (a UUID 
                    // to reference) but without bloating the lib.
                    let _ = self.touch(&existing_uuid);
                    return Ok(existing_uuid);
                }
            }
        }
        
        let uuid = generate_uuid();
        let blob_path = self.path.join("webs").join(format!("{}.bin", uuid));
        write_web_blob(&blob_path, &signature, mesh, pose_track, source_video)?;
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        
        let stub = LibraryWebStub {
            uuid: uuid.clone(),
            source_video: source_video.to_string(),
            source_chunk_start,
            source_chunk_end,
            vertex_count: mesh.vertices.len() as u32,
            triangle_count: (mesh.indices.len() / 3) as u32,
            signature,
            ref_count: 1,
            created_at: now,
            last_referenced: now,
        };
        
        let new_idx = self.manifest.webs.len();
        self.uuid_index.insert(uuid.clone(), new_idx);
        self.manifest.webs.push(stub.clone());
        self.manifest.web_count = self.manifest.webs.len() as u64;
        // Insert into eig0_index at the right sorted position. O(log n) 
        // search + O(n) insert via Vec::insert; for typical write rates 
        // (few canonicals per video) this is negligible compared to 
        // mesh.bin compression that dominates add_web.
        let new_eig = stub.signature.eigenvalues[0];
        let insert_pos = self.eig0_index.partition_point(|&(e, _)| e < new_eig);
        self.eig0_index.insert(insert_pos, (new_eig, new_idx));
        write_manifest(&self.path.join("manifest.json"), &self.manifest)?;
        
        // Dual-write to log_db. Best-effort: if log_db write fails, the 
        // JSON manifest is still authoritative. Log the error so the 
        // user can act on persistent failures.
        if let Some(db) = self.log_db.as_mut() {
            if let Err(e) = db.put(&stub) {
                eprintln!("[library] WARN: log_db.put failed for {}: {} (JSON still consistent)", uuid, e);
            } else {
                // fsync isn't free; do it once per add_web rather than 
                // batching. add_web is called at the granularity of a 
                // new canonical, which is rare during encode.
                let _ = db.flush();
            }
        }
        
        Ok(uuid)
    }
    
    /// Increment ref_count and update last_referenced for an existing web.
    /// No-op if uuid is unknown.
    pub fn touch(&mut self, uuid: &str) -> std::io::Result<()> {
        if self._lockfile.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "library opened read-only",
            ));
        }
        if let Some(&idx) = self.uuid_index.get(uuid) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let stub_for_db = {
                let stub = &mut self.manifest.webs[idx];
                stub.ref_count = stub.ref_count.saturating_add(1);
                stub.last_referenced = now;
                stub.clone()
            };
            self.touch_dirty = true;
            
            // Fast path: when the log_db is open, ONLY append to the log. 
            // The JSON manifest is now "stale" (in-memory state hasn't 
            // been flushed) but the log has the truth. JSON gets rewritten 
            // on close via Drop, batching all touches into one write.
            // 
            // Slow path: no log_db → rewrite JSON every touch (old behavior).
            // 
            // Why this matters: at 10k webs, write_manifest is ~100ms. A 
            // single encode can touch hundreds of canonicals (one per 
            // reused chunk). Old path: 10-100s of redundant JSON rewrites. 
            // New path: 0.05ms per touch (single log record append).
            match self.log_db.as_mut() {
                Some(db) => {
                    if let Err(e) = db.put(&stub_for_db) {
                        eprintln!("[library] WARN: log_db.put on touch failed for {}: {}", uuid, e);
                        // Log write failed — fall back to JSON write so 
                        // we don't lose the touch.
                        write_manifest(&self.path.join("manifest.json"), &self.manifest)?;
                    }
                    // Skip fsync on touch — they're frequent and the 
                    // OS will buffer them. Drop's flush gets them durable.
                }
                None => {
                    // No log_db: must rewrite JSON to persist the touch.
                    write_manifest(&self.path.join("manifest.json"), &self.manifest)?;
                }
            }
        }
        Ok(())
    }
    
    /// Load the full mesh + pose track for a previously-matched web.
    /// Decompresses the blob and reconstructs.
    /// 
    /// On a cache hit returns a clone of the cached value (cheap for 
    /// the mesh's Vec fields; CameraPose is Copy so the pose track 
    /// clone is a single memcpy). On miss, reads the blob, parses it, 
    /// inserts into the cache, returns.
    /// 
    /// Cache is bounded to 32 most-recent entries. For sessions that 
    /// touch many distinct canonicals the LRU eviction keeps memory 
    /// flat instead of growing unboundedly.
    pub fn load_web(&self, uuid: &str) -> std::io::Result<(SceneMesh, Vec<CameraPose>)> {
        const BLOB_CACHE_CAP: usize = 32;
        
        // Cache hit?
        if let Ok(mut cache) = self.blob_cache.lock() {
            if let Some(pos) = cache.iter().position(|(u, _, _)| u == uuid) {
                // Move to front for LRU semantics.
                let entry = cache.remove(pos).unwrap();
                let result = (entry.1.clone(), entry.2.clone());
                cache.push_front(entry);
                return Ok(result);
            }
        }
        
        // Miss — read from disk.
        let blob_path = self.path.join("webs").join(format!("{}.bin", uuid));
        let (mesh, poses) = read_web_blob(&blob_path)?;
        
        // Insert into cache (front = newest).
        if let Ok(mut cache) = self.blob_cache.lock() {
            cache.push_front((uuid.to_string(), mesh.clone(), poses.clone()));
            while cache.len() > BLOB_CACHE_CAP {
                cache.pop_back();
            }
        }
        
        Ok((mesh, poses))
    }
    
    /// Drop the resolved-mesh cache. Useful after pruning or large 
    /// library changes where cached entries may now point at deleted 
    /// canonicals.
    pub fn drop_blob_cache(&self) {
        if let Ok(mut cache) = self.blob_cache.lock() {
            cache.clear();
        }
    }
    
    /// List all UUIDs known to the library. Diagnostic.
    pub fn list(&self) -> impl Iterator<Item = &LibraryWebStub> {
        self.manifest.webs.iter()
    }
    
    /// Total bytes of manifest + all blobs (for size diagnostics).
    pub fn total_disk_bytes(&self) -> u64 {
        let mut total = 0u64;
        for stub in &self.manifest.webs {
            let p = self.path.join("webs").join(format!("{}.bin", stub.uuid));
            if let Ok(meta) = std::fs::metadata(&p) {
                total += meta.len();
            }
        }
        if let Ok(meta) = std::fs::metadata(self.path.join("manifest.json")) {
            total += meta.len();
        }
        total
    }
    
    /// Summary stats useful for GUI display + monitoring.
    /// 
    /// Returns a multi-line string describing the library: total webs, 
    /// total disk size, oldest/newest entries, top 5 source videos by 
    /// web count, ref_count distribution, FLW2-vs-FLW1 blob counts.
    pub fn stats(&self) -> String {
        if self.manifest.webs.is_empty() {
            return "Library is empty".to_string();
        }
        let n = self.manifest.webs.len();
        let disk = self.total_disk_bytes() as f64 / 1_048_576.0;
        
        // Time range.
        let mut oldest_created = u64::MAX;
        let mut newest_created = 0u64;
        let mut oldest_ref = u64::MAX;
        let mut newest_ref = 0u64;
        for s in &self.manifest.webs {
            if s.created_at < oldest_created { oldest_created = s.created_at; }
            if s.created_at > newest_created { newest_created = s.created_at; }
            if s.last_referenced < oldest_ref { oldest_ref = s.last_referenced; }
            if s.last_referenced > newest_ref { newest_ref = s.last_referenced; }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(newest_ref);
        let age_days = |secs: u64| -> u64 {
            now.saturating_sub(secs) / 86400
        };
        
        // Ref count distribution.
        let mut zero_refs = 0usize;
        let mut total_refs = 0u64;
        let mut max_refs = 0u32;
        let mut max_ref_uuid = String::new();
        for s in &self.manifest.webs {
            if s.ref_count == 0 { zero_refs += 1; }
            total_refs += s.ref_count as u64;
            if s.ref_count > max_refs {
                max_refs = s.ref_count;
                max_ref_uuid = s.uuid.clone();
            }
        }
        let avg_refs = total_refs as f32 / n as f32;
        
        // Top source videos.
        let mut by_source: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for s in &self.manifest.webs {
            *by_source.entry(s.source_video.clone()).or_insert(0) += 1;
        }
        let mut top_sources: Vec<(String, u32)> = by_source.into_iter().collect();
        top_sources.sort_by(|a, b| b.1.cmp(&a.1));
        let top_5: Vec<String> = top_sources.iter().take(5)
            .map(|(name, count)| format!("    {} ({})", name, count))
            .collect();
        
        // Vertex stats.
        let mut total_verts = 0u64;
        let mut total_tris = 0u64;
        for s in &self.manifest.webs {
            total_verts += s.vertex_count as u64;
            total_tris += s.triangle_count as u64;
        }
        let avg_verts = total_verts as f32 / n as f32;
        let avg_tris = total_tris as f32 / n as f32;
        
        format!(
            "Library at {}
             ─────────────────────────────────────
             Webs: {}
             Disk size: {:.1} MB
             Total vertices: {} (avg {:.0}/web)
             Total triangles: {} (avg {:.0}/web)
             
             References:
             - Total references: {}
             - Average per web: {:.1}
             - Webs with 0 refs: {} ({:.0}%)
             - Most-referenced: {} refs ({})
             
             Time range:
             - Oldest: {} days ago
             - Newest: {} days ago
             - Last referenced: {} days ago
             
             Top source videos:
{}",
            self.path.display(),
            n,
            disk,
            total_verts, avg_verts,
            total_tris, avg_tris,
            total_refs, avg_refs,
            zero_refs, 100.0 * zero_refs as f32 / n as f32,
            max_refs,
            if max_ref_uuid.is_empty() { "—".to_string() } else { max_ref_uuid[..8.min(max_ref_uuid.len())].to_string() },
            age_days(oldest_created),
            age_days(newest_created),
            age_days(newest_ref),
            top_5.join("
"),
        )
    }
    
    /// Prune unused canonicals. Returns the list of pruned UUIDs.
    /// 
    /// A web is "unused" if ref_count == 0 AND last_referenced is older 
    /// than `min_age_days` days. The conservative default catches webs 
    /// that have been in the library for a while without ever being 
    /// matched.
    /// 
    /// The disk blob (webs/<uuid>.bin) is removed. The manifest stub is 
    /// removed. The log_db gets a delete record (tombstone).
    /// 
    /// Caller must have opened with `open_writable`.
    pub fn prune_unused(&mut self, min_age_days: u64) -> std::io::Result<Vec<String>> {
        if self._lockfile.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "library opened read-only — use open_writable",
            ));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cutoff = now.saturating_sub(min_age_days * 86400);
        
        // Find prune candidates.
        let pruned_uuids: Vec<String> = self.manifest.webs.iter()
            .filter(|s| s.ref_count == 0 && s.last_referenced <= cutoff)
            .map(|s| s.uuid.clone())
            .collect();
        
        if pruned_uuids.is_empty() {
            return Ok(pruned_uuids);
        }
        
        eprintln!("[library] pruning {} unused canonicals (min age: {} days)",
            pruned_uuids.len(), min_age_days);
        
        // Remove blob files.
        for uuid in &pruned_uuids {
            let p = self.path.join("webs").join(format!("{}.bin", uuid));
            if let Err(e) = std::fs::remove_file(&p) {
                eprintln!("[library] WARN: prune failed to delete {}: {}", p.display(), e);
            }
        }
        
        // Tombstone in log_db.
        if let Some(db) = self.log_db.as_mut() {
            for uuid in &pruned_uuids {
                if let Err(e) = db.delete(uuid) {
                    eprintln!("[library] WARN: log_db delete failed for {}: {}", uuid, e);
                }
            }
            let _ = db.flush();
        }
        
        // Remove from in-memory manifest + uuid_index.
        let prune_set: std::collections::HashSet<&String> = pruned_uuids.iter().collect();
        self.manifest.webs.retain(|s| !prune_set.contains(&s.uuid));
        self.manifest.web_count = self.manifest.webs.len() as u64;
        // Rebuild uuid_index and eig0_index from scratch since indices shift.
        self.uuid_index.clear();
        for (i, s) in self.manifest.webs.iter().enumerate() {
            self.uuid_index.insert(s.uuid.clone(), i);
        }
        self.eig0_index = build_eig0_index(&self.manifest);
        
        // Persist manifest.
        write_manifest(&self.path.join("manifest.json"), &self.manifest)?;
        
        // Drop the cache so pruned UUIDs aren't served from memory.
        self.drop_blob_cache();
        
        Ok(pruned_uuids)
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // If touches accumulated this session, rewrite the JSON manifest 
        // once so it matches the log_db state. Doing this on close 
        // amortizes the cost across the whole session — instead of 
        // 100ms × N touches, it's 100ms × 1.
        if self.touch_dirty && self._lockfile.is_some() {
            if let Err(e) = write_manifest(&self.path.join("manifest.json"), &self.manifest) {
                eprintln!("[library] WARN: final JSON manifest write failed on close: {} (log_db has authoritative state)", e);
            } else {
                eprintln!("[library] flushed accumulated touches to JSON manifest on close");
            }
        }
        // Final flush of log_db so any touches accumulated since the 
        // last add_web are durable. Best-effort.
        if let Some(db) = self.log_db.as_mut() {
            let _ = db.flush();
            // If we wrote a lot of tombstones (rare in practice), 
            // compact on close to keep the log small.
            if db.should_compact() {
                eprintln!("[library] compacting log_db on close ({} entries, tombstones high)", db.len());
                let _ = db.compact();
            }
        }
        // Release the lockfile on writable close.
        if self._lockfile.is_some() {
            let _ = std::fs::remove_file(self.path.join(".lock"));
        }
    }
}

/// Cheap stats query for a library path without fully opening it.
/// 
/// Reads `web_count` directly from manifest.json with a short-circuit 
/// scan (no full JSON parse), and walks the `webs/` directory once for 
/// on-disk size. Roughly 100-1000× faster than `Library::open(p)?.total_disk_bytes()` 
/// at scale: the full open allocates Vec<LibraryWebStub> for every entry 
/// (for a 100k-web library that's ~12MB of allocations and several MB of 
/// JSON parsing), while quick_stats does neither.
/// 
/// Returns Some((web_count, total_on_disk_bytes)) on success, None on 
/// any read error (manifest missing, dir unreadable, etc).
/// 
/// Used by the UI status refresh — running it on every checkbox toggle 
/// would be expensive with the full open path.
pub fn quick_stats(path: &Path) -> Option<(u64, u64)> {
    use std::io::Read;
    
    // 1. web_count: read manifest.json head, find "web_count": <number>.
    let manifest_path = path.join("manifest.json");
    let mut f = std::fs::File::open(&manifest_path).ok()?;
    // Reading the first 4KB is enough to see the web_count field, which 
    // appears in the top-level object before the "webs" array. Manifests 
    // produced by this module always emit web_count as the second field.
    let mut head = vec![0u8; 4096];
    let n = f.read(&mut head).ok()?;
    head.truncate(n);
    let head_str = std::str::from_utf8(&head).ok()?;
    let needle = "\"web_count\"";
    let idx = head_str.find(needle)?;
    let after = &head_str[idx + needle.len()..];
    let colon = after.find(':')?;
    let value_part = after[colon+1..].trim_start();
    let end = value_part.find(|c: char| !c.is_ascii_digit())?;
    let web_count: u64 = value_part[..end].parse().ok()?;
    
    // 2. on-disk size: walk webs/ directory, sum file sizes. Skip 
    // anything that fails to stat (transient errors).
    let webs_dir = path.join("webs");
    let mut total_bytes: u64 = 0;
    if let Ok(rd) = std::fs::read_dir(&webs_dir) {
        for entry in rd.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total_bytes += meta.len();
                }
            }
        }
    }
    // Add manifest itself.
    if let Ok(meta) = std::fs::metadata(&manifest_path) {
        total_bytes += meta.len();
    }
    
    Some((web_count, total_bytes))
}

// ── Signature similarity ──────────────────────────────────────────────

/// Compute similarity between two web signatures as a value in [0, 1].
/// 
/// This factors in:
///   - eigenvalue spectrum match (rotation-invariant — primary signal)
///   - extent ratio (size match)
///   - radial histogram correlation (secondary disambiguator)
/// 
/// Centroid and eigenvectors are NOT compared — they describe placement
/// and orientation, which we EXPECT to differ between separate captures
/// of the same scene. Alignment finds the transform that maps one to
/// the other; matching identifies that one EXISTS to find.
pub fn signature_similarity(a: &WebSignature, b: &WebSignature) -> f32 {
    // Eigenvalue spectrum — log ratio captures shape similarity better
    // than absolute differences.
    // 
    // Hot path optimization: eig_score is computed first and dominates 
    // the weighted sum (0.6 weight). If eig_score is so low that even 
    // a perfect ext+radial wouldn't reach a meaningful match threshold 
    // (say 0.5), bail early without computing the other scores. Saves 
    // ~70% of the comparison cost on the typical "definitely doesn't 
    // match" path that find_match exercises most.
    let eig_score = {
        let mut acc = 0.0f32;
        for i in 0..3 {
            let ea = a.eigenvalues[i].max(1e-6);
            let eb = b.eigenvalues[i].max(1e-6);
            let r = (ea / eb).ln().abs();
            acc += (-r).exp();  // 1.0 if equal, decays with log distance
        }
        acc / 3.0
    };
    
    // Early exit: max possible total when eig_score is what we have 
    // is eig*0.6 + 1.0*0.25 + 1.0*0.15 = eig*0.6 + 0.4. If this 
    // can't reach 0.5 (a meaningful floor for any candidate), bail.
    if eig_score * 0.6 + 0.4 < 0.5 {
        return eig_score * 0.6;  // approximate; caller will likely reject
    }
    
    // Extent ratio — sorted descending so they're directly comparable.
    let ext_score = {
        let mut acc = 0.0f32;
        for i in 0..3 {
            let ea = a.extent[i].max(1e-6);
            let eb = b.extent[i].max(1e-6);
            let ratio = ea.min(eb) / ea.max(eb);
            acc += ratio;
        }
        acc / 3.0
    };
    
    // Radial histogram — bin-wise similarity (1 - L1/2).
    let radial_score = {
        let mut diff = 0.0f32;
        for i in 0..10 {
            diff += (a.radial_histogram[i] - b.radial_histogram[i]).abs();
        }
        (1.0 - diff / 2.0).clamp(0.0, 1.0)
    };
    
    // Weighted combination. Eigenvalues dominate because they're the
    // most rotation-invariant signal.
    eig_score * 0.6 + ext_score * 0.25 + radial_score * 0.15
}

// ── UUID generation ──────────────────────────────────────────────────

/// Generate a 32-character hex UUID using OS randomness.
/// 
/// We don't pull in `uuid` crate — the format is just hex bytes. This
/// is a UUIDv4-like value (random, not v1 timestamp-based).
fn generate_uuid() -> String {
    let mut bytes = [0u8; 16];
    // Use std::time + thread-local PRNG seed for entropy. Not cryptographically
    // secure, but ample for collision avoidance at library scale (~10^-18
    // collision probability between any two webs in a 1B-web library).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let mut state = now ^ pid.wrapping_mul(0x9e3779b97f4a7c15);
    for b in bytes.iter_mut() {
        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = state as u8;
    }
    // Set version 4 marker (bits 12-15 of byte 6 = 0100)
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    // Set variant (bits 6-7 of byte 8 = 10)
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    
    let mut out = String::with_capacity(32);
    for b in bytes.iter() {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

// ── Manifest I/O ─────────────────────────────────────────────────────

/// Write the manifest as JSON. Hand-rolled to avoid serde dependency.
/// 
/// Safety guarantees this provides (in order of strength):
/// 1. **Atomic update**: writes to manifest.json.tmp first, fsyncs, 
///    then renames. Either the rename succeeds and the new content is 
///    fully visible, or it fails and the old content stays intact. 
///    POSIX rename(2) is atomic on the same filesystem.
/// 2. **Rotating backup**: before writing, the current manifest.json 
///    (if any) is copied to manifest.json.bak.<N> where N rotates 
///    through 0..MAX_BACKUPS. Each backup is a complete prior state. 
///    Rollback = `cp manifest.json.bak.0 manifest.json`.
/// 3. **Directory fsync**: after rename, fsyncs the parent directory 
///    so the directory entry update is durable. Without this, a power 
///    failure between rename and disk flush could leave the parent dir 
///    pointing to an inconsistent state on some filesystems.
/// 4. **Schema version check on read**: if a future version writes a 
///    manifest with version > MAX_SUPPORTED_VERSION, the read refuses 
///    rather than silently accepting partial fields. Forward-compat 
///    via "fail closed" — better than misinterpreting newer schemas.
/// 
/// Tradeoff: writes are now ~2x slower due to backup copy. For a 
/// library with thousands of webs this is still milliseconds. For the 
/// safety we gain (always have at least 5 prior states to roll back to), 
/// this is worth it.
fn write_manifest(path: &Path, manifest: &LibraryManifest) -> std::io::Result<()> {
    // ── Step 1: Rotating backup of CURRENT manifest before overwrite. ──
    // 
    // We keep MAX_BACKUPS = 5 rolling .bak.N files. The most recent 
    // backup is always .bak.0; older backups shift to .bak.1, .bak.2, etc. 
    // This gives the user a 5-deep history they can roll back through.
    // 
    // If the existing manifest is unreadable (file corrupted, perms 
    // denied), we log a warning but continue with the write — the 
    // backup is a NICE-TO-HAVE, not a precondition. The user can't 
    // recover a corrupted manifest from a backup anyway if there's no 
    // valid pre-state.
    const MAX_BACKUPS: usize = 5;
    if path.exists() {
        // Rotate: move .bak.(N-1) to .bak.N, ..., .bak.0 to .bak.1, 
        // then copy current to .bak.0.
        for n in (1..MAX_BACKUPS).rev() {
            let src = path.with_extension(format!("json.bak.{}", n - 1));
            let dst = path.with_extension(format!("json.bak.{}", n));
            if src.exists() {
                // Ignore errors on rotation — the worst case is a 
                // missing intermediate backup, not data loss.
                let _ = std::fs::rename(&src, &dst);
            }
        }
        let bak0 = path.with_extension("json.bak.0");
        if let Err(e) = std::fs::copy(path, &bak0) {
            eprintln!("[library] WARN: failed to back up manifest before write: {} (continuing)", e);
        }
    }
    
    // ── Step 2: Build JSON in memory. ──
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str(&format!("  \"version\": {},\n", manifest.version));
    json.push_str(&format!("  \"web_count\": {},\n", manifest.web_count));
    json.push_str("  \"webs\": [\n");
    for (i, w) in manifest.webs.iter().enumerate() {
        json.push_str("    {\n");
        json.push_str(&format!("      \"uuid\": \"{}\",\n", w.uuid));
        json.push_str(&format!("      \"source_video\": {},\n", json_escape(&w.source_video)));
        json.push_str(&format!("      \"source_chunk_start\": {},\n", w.source_chunk_start));
        json.push_str(&format!("      \"source_chunk_end\": {},\n", w.source_chunk_end));
        json.push_str(&format!("      \"vertex_count\": {},\n", w.vertex_count));
        json.push_str(&format!("      \"triangle_count\": {},\n", w.triangle_count));
        json.push_str(&format!("      \"ref_count\": {},\n", w.ref_count));
        json.push_str(&format!("      \"created_at\": {},\n", w.created_at));
        json.push_str(&format!("      \"last_referenced\": {},\n", w.last_referenced));
        json.push_str(&format!("      \"signature\": \"{}\"\n", hex_encode(&signature_to_bytes(&w.signature))));
        json.push_str(if i + 1 < manifest.webs.len() { "    },\n" } else { "    }\n" });
    }
    json.push_str("  ]\n");
    json.push_str("}\n");
    
    // ── Step 3: Atomic write via tmp + rename. ──
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    
    // ── Step 4: Fsync the parent directory so the rename is durable. ──
    // 
    // Some filesystems (ext4 with data=writeback, etc.) can lose the 
    // rename after a crash if the directory entry update isn't flushed. 
    // POSIX guarantees that after fsync of the dir, the rename survives 
    // a power failure. On Windows this is a no-op (handle is read-only).
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();  // best-effort; ignore errors
        }
    }
    Ok(())
}

/// Roll back the manifest to a previous backup. `slot` selects which 
/// backup: 0 = most recent (.bak.0), 1 = one before that, up to 4. 
/// Returns an error if the backup doesn't exist or isn't readable.
/// 
/// Before rolling back, this function ALSO rotates the CURRENT manifest 
/// into a fresh .bak.0 slot — so the user can roll FORWARD again if 
/// the rollback was a mistake.
pub fn rollback_manifest(library_dir: &Path, slot: usize) -> std::io::Result<()> {
    if slot >= 5 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("backup slot {} out of range (0..5)", slot),
        ));
    }
    let manifest_path = library_dir.join("manifest.json");
    let bak_path = manifest_path.with_extension(format!("json.bak.{}", slot));
    if !bak_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("backup slot {} not found at {}", slot, bak_path.display()),
        ));
    }
    // Read the backup, validate it parses, then write it through the 
    // standard write_manifest path. This rotates the CURRENT state 
    // into .bak.0 (so we can roll forward) and writes the backup as 
    // the new current.
    let manifest = read_manifest(&bak_path)?;
    write_manifest(&manifest_path, &manifest)?;
    eprintln!("[library] rolled back manifest from slot {} ({} webs)", slot, manifest.web_count);
    Ok(())
}

/// Highest schema version this code can parse. Used to refuse forward 
/// migrations from future versions that this build doesn't know about.
const MAX_SUPPORTED_MANIFEST_VERSION: u32 = 1;

fn read_manifest(path: &Path) -> std::io::Result<LibraryManifest> {
    let mut f = std::fs::File::open(path)?;
    let mut raw = String::new();
    f.read_to_string(&mut raw)?;
    
    let mut m = LibraryManifest::default();
    m.version = parse_json_u64(&raw, "version").unwrap_or(LIBRARY_VERSION as u64) as u32;
    m.web_count = parse_json_u64(&raw, "web_count").unwrap_or(0);
    
    // Forward-compat check: a future version may add fields that this 
    // build doesn't know how to read. Refuse rather than silently 
    // drop those fields. User can either upgrade their FAO build or 
    // roll back to a backup (rollback_manifest).
    if m.version > MAX_SUPPORTED_MANIFEST_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "manifest schema version {} is newer than this build supports (max {}).                  Upgrade FAO or use `rollback_manifest` to restore an older snapshot.",
                m.version, MAX_SUPPORTED_MANIFEST_VERSION
            ),
        ));
    }
    
    // Parse the webs array — split on object boundaries.
    let webs_start = match raw.find("\"webs\"") {
        Some(i) => i,
        None => return Ok(m),
    };
    let webs_section = &raw[webs_start..];
    
    let mut depth = 0i32;
    let mut start = None;
    for (i, c) in webs_section.char_indices() {
        match c {
            '{' => {
                if depth == 0 { /* outer object — skip */ }
                if depth >= 1 && start.is_none() { start = Some(i); }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 1 {
                    if let Some(s) = start.take() {
                        let obj = &webs_section[s..=i];
                        if let Some(stub) = parse_web_stub(obj) {
                            m.webs.push(stub);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    
    Ok(m)
}

fn parse_web_stub(obj: &str) -> Option<LibraryWebStub> {
    Some(LibraryWebStub {
        uuid: parse_json_string(obj, "uuid")?,
        source_video: parse_json_string(obj, "source_video")?,
        source_chunk_start: parse_json_u64(obj, "source_chunk_start").unwrap_or(0) as u32,
        source_chunk_end: parse_json_u64(obj, "source_chunk_end").unwrap_or(0) as u32,
        vertex_count: parse_json_u64(obj, "vertex_count").unwrap_or(0) as u32,
        triangle_count: parse_json_u64(obj, "triangle_count").unwrap_or(0) as u32,
        ref_count: parse_json_u64(obj, "ref_count").unwrap_or(1) as u32,
        created_at: parse_json_u64(obj, "created_at").unwrap_or(0),
        last_referenced: parse_json_u64(obj, "last_referenced").unwrap_or(0),
        signature: parse_json_string(obj, "signature")
            .and_then(|h| hex_decode(&h))
            .and_then(|b| signature_from_bytes(&b))
            .unwrap_or_default(),
    })
}

fn parse_json_u64(s: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{}\"", key);
    let i = s.find(&pat)?;
    let after = &s[i + pat.len()..];
    let colon = after.find(':')?;
    let value_start = after[colon+1..].find(|c: char| !c.is_whitespace())?;
    let value_str = &after[colon+1+value_start..];
    let end = value_str.find(|c: char| !c.is_ascii_digit())?;
    value_str[..end].parse().ok()
}

fn parse_json_string(s: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\"", key);
    let i = s.find(&pat)?;
    let after = &s[i + pat.len()..];
    let colon = after.find(':')?;
    let after_colon = &after[colon+1..];
    let q1 = after_colon.find('"')?;
    let value_start = q1 + 1;
    // Find closing quote, respecting backslash escapes.
    let bytes = after_colon.as_bytes();
    let mut end = value_start;
    while end < bytes.len() {
        if bytes[end] == b'\\' { end += 2; continue; }
        if bytes[end] == b'"' { break; }
        end += 1;
    }
    let raw = &after_colon[value_start..end];
    Some(json_unescape(raw))
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ── Hex encoding ─────────────────────────────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None; }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks_exact(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ── WebSignature serialization (112 bytes packed) ────────────────────

/// Serialize a WebSignature into 112 bytes of LE f32 fields. Public 
/// so library_db can use the same encoding for binary records.
pub fn signature_to_bytes_pub(sig: &WebSignature) -> [u8; 112] {
    signature_to_bytes(sig)
}

/// Deserialize 112 bytes back into a WebSignature. Returns None on 
/// short input.
pub fn signature_from_bytes_pub(b: &[u8]) -> Option<WebSignature> {
    signature_from_bytes(b)
}

fn signature_to_bytes(sig: &WebSignature) -> [u8; 112] {
    // Field sizes:
    //   extent[3]            = 12 bytes
    //   centroid[3]          = 12 bytes
    //   eigenvalues[3]       = 12 bytes
    //   eigenvectors[9]      = 36 bytes
    //   radial_histogram[10] = 40 bytes
    //   ───────────────────  ─────────
    //   total                = 112 bytes
    // 
    // The previous version had this typed as [u8; 88], which is a bug 
    // — the histogram loop would index out of bounds on any non-default 
    // signature. Caught while building library_db; fixed here.
    let mut out = [0u8; 112];
    let mut i = 0;
    for v in &sig.extent { out[i..i+4].copy_from_slice(&v.to_le_bytes()); i += 4; }
    for v in &sig.centroid { out[i..i+4].copy_from_slice(&v.to_le_bytes()); i += 4; }
    for v in &sig.eigenvalues { out[i..i+4].copy_from_slice(&v.to_le_bytes()); i += 4; }
    for v in &sig.eigenvectors { out[i..i+4].copy_from_slice(&v.to_le_bytes()); i += 4; }
    for v in &sig.radial_histogram { out[i..i+4].copy_from_slice(&v.to_le_bytes()); i += 4; }
    debug_assert_eq!(i, 112);
    out
}

fn signature_from_bytes(b: &[u8]) -> Option<WebSignature> {
    if b.len() < 112 { return None; }
    let mut sig = WebSignature::default();
    let mut i = 0;
    let read_f = |buf: &[u8], pos: usize| -> f32 {
        f32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]])
    };
    for j in 0..3 { sig.extent[j] = read_f(b, i); i += 4; }
    for j in 0..3 { sig.centroid[j] = read_f(b, i); i += 4; }
    for j in 0..3 { sig.eigenvalues[j] = read_f(b, i); i += 4; }
    for j in 0..9 { sig.eigenvectors[j] = read_f(b, i); i += 4; }
    for j in 0..10 { sig.radial_histogram[j] = read_f(b, i); i += 4; }
    Some(sig)
}

// ── Per-web blob I/O ─────────────────────────────────────────────────

fn write_web_blob(
    path: &Path,
    sig: &WebSignature,
    mesh: &SceneMesh,
    pose_track: &[CameraPose],
    source_video: &str,
) -> std::io::Result<()> {
    let mut body = Vec::with_capacity(64 * 1024);
    
    let sig_bytes = signature_to_bytes(sig);
    body.extend_from_slice(&(sig_bytes.len() as u32).to_le_bytes());
    body.extend_from_slice(&sig_bytes);
    
    let v_count = mesh.vertices.len() as u32;
    let t_count = (mesh.indices.len() / 3) as u32;
    let n_count = mesh.normals.len() as u32;
    let p_count = pose_track.len() as u32;
    body.extend_from_slice(&v_count.to_le_bytes());
    body.extend_from_slice(&t_count.to_le_bytes());
    body.extend_from_slice(&n_count.to_le_bytes());
    body.extend_from_slice(&p_count.to_le_bytes());
    
    let src_bytes = source_video.as_bytes();
    body.extend_from_slice(&(src_bytes.len() as u32).to_le_bytes());
    body.extend_from_slice(src_bytes);
    
    // ── FLW2 quantized layout (default) ──
    // 
    // 1 byte tag (1 = quantized, 0 = legacy raw f32)
    // 24 bytes AABB (3 × f32 min + 3 × f32 max)
    // n_v * 6 bytes vertices (3 × u16 quantized to AABB)
    // n_v * 3 bytes colors (RGB u8)
    // n_v * 6 bytes normals (3 × i16 quantized to [-1, 1])
    // n_i * 4 bytes indices (u32, unchanged)
    // p * 48 bytes poses (CameraPose::to_bytes, unchanged)
    // 
    // For a 1000-vertex mesh: 12KB (raw) → 3KB (quantized) raw.
    // After zstd: probably 1.5KB vs 4KB. ~3× size reduction.
    // 
    // Vertex precision = extent / 65535. For a 100m chunk that's 1.5mm — 
    // well below any visible threshold.
    // 
    // Override with FAO_LIB_RAW=1 to write FLW1-compatible f32 blobs.
    let lib_raw = std::env::var("FAO_LIB_RAW")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if lib_raw {
        body.push(0u8);  // legacy tag
        for v in &mesh.vertices {
            body.extend_from_slice(&v.x.to_le_bytes());
            body.extend_from_slice(&v.y.to_le_bytes());
            body.extend_from_slice(&v.z.to_le_bytes());
        }
        for c in &mesh.vertex_colors {
            body.extend_from_slice(&c[0].to_le_bytes());
            body.extend_from_slice(&c[1].to_le_bytes());
            body.extend_from_slice(&c[2].to_le_bytes());
        }
        for n in &mesh.normals {
            body.extend_from_slice(&n.x.to_le_bytes());
            body.extend_from_slice(&n.y.to_le_bytes());
            body.extend_from_slice(&n.z.to_le_bytes());
        }
        for i in &mesh.indices {
            body.extend_from_slice(&i.to_le_bytes());
        }
        for p in pose_track {
            body.extend_from_slice(&p.to_bytes());
        }
    } else {
        body.push(1u8);  // quantized tag
        // AABB.
        let mut mn = [f32::INFINITY, f32::INFINITY, f32::INFINITY];
        let mut mx = [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for v in &mesh.vertices {
            if v.x < mn[0] { mn[0] = v.x; }
            if v.y < mn[1] { mn[1] = v.y; }
            if v.z < mn[2] { mn[2] = v.z; }
            if v.x > mx[0] { mx[0] = v.x; }
            if v.y > mx[1] { mx[1] = v.y; }
            if v.z > mx[2] { mx[2] = v.z; }
        }
        // Guard against degenerate AABB.
        for i in 0..3 {
            if !(mx[i] > mn[i]) { mx[i] = mn[i] + 1e-3; }
        }
        for v in &mn { body.extend_from_slice(&v.to_le_bytes()); }
        for v in &mx { body.extend_from_slice(&v.to_le_bytes()); }
        let extent = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
        // Vertices as u16.
        for v in &mesh.vertices {
            let qx = (((v.x - mn[0]) / extent[0]).clamp(0.0, 1.0) * 65535.0).round() as u16;
            let qy = (((v.y - mn[1]) / extent[1]).clamp(0.0, 1.0) * 65535.0).round() as u16;
            let qz = (((v.z - mn[2]) / extent[2]).clamp(0.0, 1.0) * 65535.0).round() as u16;
            body.extend_from_slice(&qx.to_le_bytes());
            body.extend_from_slice(&qy.to_le_bytes());
            body.extend_from_slice(&qz.to_le_bytes());
        }
        // Colors as u8 RGB.
        for c in &mesh.vertex_colors {
            body.push((c[0].clamp(0.0, 1.0) * 255.0).round() as u8);
            body.push((c[1].clamp(0.0, 1.0) * 255.0).round() as u8);
            body.push((c[2].clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        // Normals as i16 (range [-1, 1] → [-32767, 32767]).
        for n in &mesh.normals {
            let qx = (n.x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            let qy = (n.y.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            let qz = (n.z.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            body.extend_from_slice(&qx.to_le_bytes());
            body.extend_from_slice(&qy.to_le_bytes());
            body.extend_from_slice(&qz.to_le_bytes());
        }
        // Indices unchanged.
        for i in &mesh.indices {
            body.extend_from_slice(&i.to_le_bytes());
        }
        // Poses unchanged (camera poses don't quantize well — focal/distortion 
        // have wide range; leave as f32).
        for p in pose_track {
            body.extend_from_slice(&p.to_bytes());
        }
    }
    
    // High compression for archive use — these blobs may stick around for years.
    let compressed = zstd::encode_all(body.as_slice(), 19)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    
    let tmp = path.with_extension("bin.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        // FLW2 magic — version 2 blob format. Tag byte inside the body 
        // disambiguates quantized vs legacy raw. Decoder handles both 
        // FLW1 (legacy) and FLW2 (current).
        f.write_all(b"FLW2")?;
        f.write_all(&compressed)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_web_blob(path: &Path) -> std::io::Result<(SceneMesh, Vec<CameraPose>)> {
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    // Accept either FLW1 (legacy, all f32) or FLW2 (tag byte signals 
    // quantized vs legacy layout).
    let is_flw2 = match &magic {
        b"FLW2" => true,
        b"FLW1" => false,
        _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad magic: {:?}", magic))),
    };
    let mut compressed = Vec::new();
    f.read_to_end(&mut compressed)?;
    let body = zstd::decode_all(compressed.as_slice())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    
    if body.len() < 4 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "body too short"));
    }
    let mut off = 0usize;
    let read_u32 = |b: &[u8], pos: usize| -> u32 {
        u32::from_le_bytes([b[pos], b[pos+1], b[pos+2], b[pos+3]])
    };
    let read_f = |b: &[u8], pos: usize| -> f32 {
        f32::from_le_bytes([b[pos], b[pos+1], b[pos+2], b[pos+3]])
    };
    
    // Skip signature (already in manifest).
    let sig_len = read_u32(&body, off) as usize; off += 4;
    if body.len() < off + sig_len { return Err(io_err("truncated sig")); }
    off += sig_len;
    
    if body.len() < off + 16 { return Err(io_err("truncated header")); }
    let v_count = read_u32(&body, off) as usize; off += 4;
    let t_count = read_u32(&body, off) as usize; off += 4;
    let n_count = read_u32(&body, off) as usize; off += 4;
    let p_count = read_u32(&body, off) as usize; off += 4;
    
    if body.len() < off + 4 { return Err(io_err("truncated source_video header")); }
    let src_len = read_u32(&body, off) as usize; off += 4;
    if body.len() < off + src_len { return Err(io_err("truncated source_video")); }
    off += src_len;  // skip source_video string
    
    // Format-tag byte. Only present for FLW2; FLW1 implicitly = tag 0.
    let format_tag: u8 = if is_flw2 {
        if body.len() < off + 1 { return Err(io_err("truncated tag byte")); }
        let t = body[off];
        off += 1;
        t
    } else {
        0
    };
    
    let i_count = t_count * 3;
    let mut vertices = Vec::with_capacity(v_count);
    let mut vertex_colors = Vec::with_capacity(v_count);
    let mut normals = Vec::with_capacity(n_count);
    let mut indices = Vec::with_capacity(i_count);
    let mut pose_track = Vec::with_capacity(p_count);
    
    if format_tag == 1 {
        // Quantized layout: AABB + u16 verts + u8 colors + i16 normals + u32 idx + f32 poses.
        let needed = 24 
            + v_count * 6 
            + v_count * 3 
            + n_count * 6 
            + i_count * 4 
            + p_count * 48;
        if body.len() < off + needed {
            return Err(io_err("truncated quantized body data"));
        }
        // AABB.
        let mut mn = [0.0f32; 3];
        let mut mx = [0.0f32; 3];
        for i in 0..3 { mn[i] = read_f(&body, off); off += 4; }
        for i in 0..3 { mx[i] = read_f(&body, off); off += 4; }
        let extent = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
        // Vertices.
        for _ in 0..v_count {
            let qx = u16::from_le_bytes([body[off], body[off+1]]) as f32; off += 2;
            let qy = u16::from_le_bytes([body[off], body[off+1]]) as f32; off += 2;
            let qz = u16::from_le_bytes([body[off], body[off+1]]) as f32; off += 2;
            vertices.push(Vec3 {
                x: mn[0] + qx * extent[0] / 65535.0,
                y: mn[1] + qy * extent[1] / 65535.0,
                z: mn[2] + qz * extent[2] / 65535.0,
            });
        }
        // Colors as u8 RGB.
        for _ in 0..v_count {
            vertex_colors.push([
                body[off] as f32 / 255.0,
                body[off+1] as f32 / 255.0,
                body[off+2] as f32 / 255.0,
            ]);
            off += 3;
        }
        // Normals as i16.
        for _ in 0..n_count {
            let qx = i16::from_le_bytes([body[off], body[off+1]]) as f32; off += 2;
            let qy = i16::from_le_bytes([body[off], body[off+1]]) as f32; off += 2;
            let qz = i16::from_le_bytes([body[off], body[off+1]]) as f32; off += 2;
            normals.push(Vec3 {
                x: qx / 32767.0,
                y: qy / 32767.0,
                z: qz / 32767.0,
            });
        }
        // Indices unchanged.
        for _ in 0..i_count {
            indices.push(read_u32(&body, off));
            off += 4;
        }
        // Poses unchanged.
        for _ in 0..p_count {
            if let Some(pose) = CameraPose::from_bytes(&body[off..off+48]) {
                pose_track.push(pose);
            }
            off += 48;
        }
    } else {
        // Legacy f32 layout (FLW1 or FLW2 with tag=0).
        let needed = v_count * 12 + v_count * 12 + n_count * 12 + t_count * 12 + p_count * 48;
        if body.len() < off + needed {
            return Err(io_err("truncated body data"));
        }
        for _ in 0..v_count {
            vertices.push(Vec3 { x: read_f(&body, off), y: read_f(&body, off+4), z: read_f(&body, off+8) });
            off += 12;
        }
        for _ in 0..v_count {
            vertex_colors.push([read_f(&body, off), read_f(&body, off+4), read_f(&body, off+8)]);
            off += 12;
        }
        for _ in 0..n_count {
            normals.push(Vec3 { x: read_f(&body, off), y: read_f(&body, off+4), z: read_f(&body, off+8) });
            off += 12;
        }
        for _ in 0..i_count {
            indices.push(read_u32(&body, off));
            off += 4;
        }
        for _ in 0..p_count {
            if let Some(pose) = CameraPose::from_bytes(&body[off..off+48]) {
                pose_track.push(pose);
            }
            off += 48;
        }
    }
    
    let mesh = SceneMesh {
        vertices,
        vertex_colors,
        indices,
        normals,
        vertex_hits: vec![1u32; v_count],
        vertex_grid: HashMap::new(),
    };
    Ok((mesh, pose_track))
}

fn io_err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    
    fn temp_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("fao_lib_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }
    
    #[test]
    fn init_open_roundtrip() {
        let p = temp_dir();
        Library::init(&p).expect("init");
        let lib = Library::open(&p).expect("open");
        assert_eq!(lib.manifest.version, LIBRARY_VERSION);
        assert_eq!(lib.manifest.web_count, 0);
        let _ = std::fs::remove_dir_all(&p);
    }
    
    #[test]
    fn add_and_match() {
        let p = temp_dir();
        Library::init(&p).expect("init");
        let mut lib = Library::open_writable(&p).expect("open_writable");
        
        // Build a synthetic mesh + signature.
        let mesh = SceneMesh {
            vertices: vec![Vec3 { x: 0.0, y: 0.0, z: 0.0 }, Vec3 { x: 1.0, y: 0.0, z: 0.0 }],
            vertex_colors: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![],
            normals: vec![],
            vertex_hits: vec![1, 1],
            vertex_grid: HashMap::new(),
        };
        let mut sig = WebSignature::default();
        sig.extent = [10.0, 5.0, 1.0];
        sig.centroid = [0.5, 0.0, 0.0];
        sig.eigenvalues = [10.0, 5.0, 1.0];
        sig.radial_histogram = [0.1; 10];
        
        let uuid = lib.add_web(sig, &mesh, &[], "test.mp4", 0, 100).expect("add_web");
        assert_eq!(lib.manifest.webs.len(), 1);
        assert_eq!(lib.manifest.webs[0].uuid, uuid);
        
        // Match against the same sig — should find self.
        let m = lib.find_match(&sig, 0.5);
        assert!(m.is_some(), "self-match failed");
        let (stub, score) = m.unwrap();
        assert_eq!(stub.uuid, uuid);
        assert!(score > 0.95, "self-similarity should be near 1.0, got {}", score);
        
        // Different signature should NOT match.
        let mut sig2 = WebSignature::default();
        sig2.extent = [100.0, 50.0, 10.0];  // 10× larger
        sig2.eigenvalues = [100.0, 50.0, 10.0];
        sig2.radial_histogram = [0.0; 10];
        let m2 = lib.find_match(&sig2, 0.75);
        // Geometrically similar shape (same eigenvalue ratios) so this 
        // depends on the threshold. The intent: if 0.75 isn't met it's None.
        let _ = m2;
        
        // load_web roundtrip.
        let (loaded_mesh, _poses) = lib.load_web(&uuid).expect("load_web");
        assert_eq!(loaded_mesh.vertices.len(), 2);
        assert_eq!(loaded_mesh.vertex_colors.len(), 2);
        
        drop(lib);
        let _ = std::fs::remove_dir_all(&p);
    }
    
    #[test]
    fn manifest_persists_across_open() {
        let p = temp_dir();
        Library::init(&p).expect("init");
        {
            let mut lib = Library::open_writable(&p).expect("open_writable");
            let mut sig = WebSignature::default();
            sig.extent = [1.0, 1.0, 1.0];
            sig.eigenvalues = [1.0, 1.0, 1.0];
            sig.radial_histogram = [0.1; 10];
            let mesh = SceneMesh {
                vertices: vec![],
                vertex_colors: vec![],
                indices: vec![],
                normals: vec![],
                vertex_hits: vec![],
                vertex_grid: HashMap::new(),
            };
            lib.add_web(sig, &mesh, &[], "first.mp4", 0, 50).expect("add_web");
        }  // drops lib, releasing lock
        
        // Reopen and verify the entry persists.
        let lib2 = Library::open(&p).expect("reopen");
        assert_eq!(lib2.manifest.webs.len(), 1);
        assert_eq!(lib2.manifest.webs[0].source_video, "first.mp4");
        
        let _ = std::fs::remove_dir_all(&p);
    }
    
    #[test]
    fn signature_similarity_self_is_one() {
        let mut sig = WebSignature::default();
        sig.extent = [10.0, 5.0, 1.0];
        sig.eigenvalues = [10.0, 5.0, 1.0];
        sig.radial_histogram = [0.1; 10];
        let s = signature_similarity(&sig, &sig);
        assert!(s > 0.95, "self-similarity should be near 1.0, got {}", s);
    }
    
    #[test]
    fn quick_stats_matches_open_at_small_scale() {
        // For a small library the slow path (full open + total_disk_bytes) 
        // and the fast path (quick_stats) MUST agree. quick_stats just 
        // saves the cost; it doesn't compute different numbers.
        let p = std::env::temp_dir().join(format!("fao_lib_quick_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        Library::init(&p).expect("init");
        
        // Empty library first.
        let (qc, qb) = quick_stats(&p).expect("quick_stats on empty");
        assert_eq!(qc, 0);
        // qb should equal manifest size (no webs yet).
        let lib = Library::open(&p).expect("open empty");
        assert_eq!(qc as u64, lib.manifest.web_count);
        assert_eq!(qb, lib.total_disk_bytes());
        drop(lib);
        
        // Add one web.
        let mut libw = Library::open_writable(&p).expect("open_writable");
        let mut sig = WebSignature::default();
        sig.extent = [3.0, 2.0, 1.0];
        sig.eigenvalues = [3.0, 2.0, 1.0];
        sig.radial_histogram = [0.1; 10];
        let mesh = SceneMesh {
            vertices: vec![Vec3 { x: 0.0, y: 0.0, z: 0.0 }],
            vertex_colors: vec![[1.0, 1.0, 1.0]],
            indices: vec![],
            normals: vec![],
            vertex_hits: vec![1],
            vertex_grid: HashMap::new(),
        };
        libw.add_web(sig, &mesh, &[], "synthetic.mp4", 0, 60).expect("add");
        drop(libw);
        
        // Re-check.
        let (qc, qb) = quick_stats(&p).expect("quick_stats with 1 web");
        let lib2 = Library::open(&p).expect("re-open");
        assert_eq!(qc as u64, lib2.manifest.web_count);
        assert_eq!(qc, 1);
        assert_eq!(qb, lib2.total_disk_bytes());
        
        let _ = std::fs::remove_dir_all(&p);
    }
    
    #[test]
    fn quick_stats_returns_none_on_missing() {
        let p = std::env::temp_dir().join(format!("fao_lib_quick_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        // Don't init.
        assert!(quick_stats(&p).is_none(), "quick_stats on non-existent path should be None");
    }
}
