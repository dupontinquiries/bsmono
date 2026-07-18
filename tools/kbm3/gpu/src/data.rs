//! Reads the exact on-disk cache produced by the Python pipeline (v5.3+,
//! hash-based sharding -- see gan-zai-v5.3.py's sharding fix / commit
//! 75f7ae7):
//!   - cache/manifest.json                : JSON array of source file paths
//!   - cache/shard-NNN-of-MMM.sqlite       : shard, table `mels(filepath TEXT PK, data BLOB)`
//!                                           shard_id = crc32(filepath) % num_shards
//!                                           (verified crc32fast matches Python's
//!                                           zlib.crc32 bit-for-bit)
//!   - each BLOB                          : zlib-compressed row-major f32
//!                                           [n_mels, frames], values in [-1, 1]
//! Shard assignment depends only on each file's own path, never on manifest
//! position or library size, so nothing can be orphaned by the library
//! growing/shrinking (that was the bug the v5.3 fix addressed).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;

use burn::prelude::*;
use burn::tensor::TensorData;
use rusqlite::{Connection, OpenFlags};

pub struct MelCache {
    cache_dir: PathBuf,
    num_shards: usize,
    n_mels: usize,
    time_frames: usize,
    manifest: Vec<String>,
    valid: Vec<usize>,
    conns: HashMap<usize, Connection>,
}

impl MelCache {
    /// `num_shards` must match the value the cache was built with (gan-zai-v5.3.py's
    /// Config.num_shards, default 32).
    pub fn open(cache_dir: &str, num_shards: usize, n_mels: usize, time_frames: usize) -> Self {
        let cache_dir = PathBuf::from(cache_dir);
        let manifest_txt = std::fs::read_to_string(cache_dir.join("manifest.json"))
            .expect("cache/manifest.json not found -- build the cache with the Python pipeline first");
        let manifest: Vec<String> =
            serde_json::from_str(&manifest_txt).expect("manifest.json is not a JSON string array");

        let mut c = MelCache {
            cache_dir,
            num_shards,
            n_mels,
            time_frames,
            manifest,
            valid: Vec::new(),
            conns: HashMap::new(),
        };
        c.build_valid();
        c
    }

    /// Stable shard assignment: depends only on this path + num_shards, never
    /// on how many other files exist or what order they were found in.
    fn shard_for_path(&self, path: &str) -> usize {
        (crc32fast::hash(path.as_bytes()) as usize) % self.num_shards
    }

    fn shard_name(&self, shard_id: usize) -> String {
        format!("shard-{shard_id:03}-of-{:03}.sqlite", self.num_shards)
    }

    fn open_shard(&self, shard_id: usize) -> Option<Connection> {
        let p = self.cache_dir.join(self.shard_name(shard_id));
        if !p.exists() {
            return None;
        }
        let conn = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let _ = conn.pragma_update(None, "query_only", true);
        Some(conn)
    }

    /// Determine which manifest indices actually have a row in their shard.
    /// Groups by each path's own (hash-based) shard, matching gan-zai-v5.3.py's
    /// _setup_valid_indices exactly.
    fn build_valid(&mut self) {
        let mut by_shard: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, path) in self.manifest.iter().enumerate() {
            by_shard.entry(self.shard_for_path(path)).or_default().push(i);
        }
        let mut total_rows_in_shards = 0usize;
        for (sid, indices) in by_shard {
            let Some(conn) = self.open_shard(sid) else {
                continue;
            };
            let mut stmt = conn.prepare("SELECT filepath FROM mels").unwrap();
            let present: HashSet<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            total_rows_in_shards += present.len();
            for i in indices {
                if present.contains(&self.manifest[i]) {
                    self.valid.push(i);
                }
            }
        }

        // Detect the "orphaned rows" failure mode: the shards contain many
        // more mel rows than the manifest can match. This happens when rows
        // were INSERTed under one path form (e.g. absolute) but the manifest
        // lists another (e.g. relative) -- so validity lookups miss almost
        // everything even though the data is physically present. Silently
        // training on the 2 matching tracks would be worse than saying so.
        if total_rows_in_shards > self.valid.len().saturating_mul(2)
            && total_rows_in_shards.saturating_sub(self.valid.len()) >= 5
        {
            eprintln!(
                "WARNING: cache looks inconsistent -- {} mel rows exist across the shards, \
                 but only {} of {} manifest entries match a row. The rest are ORPHANED \
                 (rows keyed by a different path form than the manifest lists -- e.g. \
                 absolute vs. relative, or a moved data dir). Training will only see the \
                 {} matching tracks. Rebuild the cache from the current data dir \
                 (`build-cache --data <dir> --cache <dir>`) so the manifest and row keys agree.",
                total_rows_in_shards, self.valid.len(), self.manifest.len(), self.valid.len()
            );
        }
    }

    pub fn len(&self) -> usize {
        self.valid.len()
    }

    fn conn(&mut self, shard_id: usize) -> &Connection {
        if !self.conns.contains_key(&shard_id) {
            let c = self.open_shard(shard_id).expect("shard vanished");
            self.conns.insert(shard_id, c);
        }
        self.conns.get(&shard_id).unwrap()
    }

    /// Fetch one track, decompress, and take a random [n_mels, time_frames] crop
    /// (silence-padded if the track is shorter than the crop).
    fn load_crop(&mut self, real_idx: usize) -> Vec<f32> {
        let (n_mels, tf) = (self.n_mels, self.time_frames);
        let path = self.manifest[real_idx].clone();
        let shard_id = self.shard_for_path(&path);

        let blob: Vec<u8> = {
            let conn = self.conn(shard_id);
            conn.query_row(
                "SELECT data FROM mels WHERE filepath = ?1",
                [&path],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .expect("row missing")
        };

        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&blob[..])
            .read_to_end(&mut raw)
            .expect("zlib inflate failed");

        // Format detection. New int8 blobs start with a 2-byte magic header
        // [0xB3, 0x08]; legacy f32 blobs (and Python-written ones) have no
        // header and are a raw f32 array whose byte length is a multiple of
        // 4*n_mels. This keeps existing f32 caches readable unchanged while
        // letting build-cache write ~4.6x smaller int8 caches.
        let flat: Vec<f32> = if raw.len() >= 2 && raw[0] == 0xB3 && raw[1] == 0x08 {
            // int8: each byte is a mel value * 127 in [-127, 127]
            raw[2..].iter().map(|&b| (b as i8) as f32 / 127.0).collect()
        } else {
            bytemuck::cast_slice::<u8, f32>(&raw).to_vec()
        };
        let frames = flat.len() / n_mels;

        let mut out = vec![-1.0f32; n_mels * tf];
        if frames >= tf {
            let t0 = fastrand::usize(0..(frames - tf + 1));
            for m in 0..n_mels {
                let src = &flat[m * frames + t0..m * frames + t0 + tf];
                out[m * tf..m * tf + tf].copy_from_slice(src);
            }
        } else {
            for m in 0..n_mels {
                let src = &flat[m * frames..m * frames + frames];
                out[m * tf..m * tf + frames].copy_from_slice(src);
            }
        }
        out
    }

    /// A random batch of real mels as (B, 1, n_mels, time_frames).
    pub fn next_batch<B: Backend>(&mut self, batch: usize, device: &B::Device) -> Tensor<B, 4> {
        let (n_mels, tf) = (self.n_mels, self.time_frames);
        let mut data = Vec::with_capacity(batch * n_mels * tf);
        for _ in 0..batch {
            let vi = fastrand::usize(0..self.valid.len());
            data.extend_from_slice(&self.load_crop(self.valid[vi]));
        }
        Tensor::from_data(TensorData::new(data, [batch, 1, n_mels, tf]), device)
    }
}

fn shard_name_for(shard_id: usize, num_shards: usize) -> String {
    format!("shard-{shard_id:03}-of-{:03}.sqlite", num_shards)
}

fn shard_for_path_str(path: &str, num_shards: usize) -> usize {
    (crc32fast::hash(path.as_bytes()) as usize) % num_shards
}

/// Recursively collect .opus / .wav files under `dir`.
fn find_audio(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(find_audio(&p));
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            if ext.eq_ignore_ascii_case("opus") || ext.eq_ignore_ascii_case("wav") {
                out.push(p);
            }
        }
    }
    out
}

/// Build a cache on disk from a folder of audio, in the EXACT format the
/// Python pipeline (v5.3+) reads and writes -- manifest.json + hash-sharded
/// `shard-NNN-of-MMM.sqlite` files, each row a zlib-compressed row-major f32
/// `[n_mels, frames]` mel in [-1, 1]. Reuses the numerically-verified decode
/// + mel path from `audio.rs`, so a cache built here is byte-compatible with
/// one built by gan-zai-v5.3.py (same crc32 sharding, same blob layout) and
/// readable by either side.
///
/// Tracks too short to yield even one `time_frames`-wide crop are skipped
/// (and reported), matching the Python builder. `manifest.json` records the
/// paths of successfully-cached tracks, in the SAME string form used as the
/// SQLite primary key, so shard assignment and lookups agree.
#[allow(clippy::too_many_arguments)]
pub fn build_cache(
    data_dir: &str,
    cache_dir: &str,
    num_shards: usize,
    sr: u32,
    n_fft: usize,
    hop: usize,
    n_mels: usize,
    time_frames: usize,
    db_floor: f64,
    db_ceil: f64,
    int8: bool,
) {
    use std::io::Write as _;

    let files = find_audio(std::path::Path::new(data_dir));
    if files.is_empty() {
        eprintln!("build-cache: no .opus/.wav files found under '{data_dir}' -- nothing to do.");
        return;
    }
    std::fs::create_dir_all(cache_dir).expect("could not create cache dir");
    println!("build-cache: {} audio files under '{data_dir}' -> '{cache_dir}' ({num_shards} shards).", files.len());

    // One writable connection per shard, opened lazily.
    let mut conns: HashMap<usize, Connection> = HashMap::new();
    let cache_path = PathBuf::from(cache_dir);
    let min_samples = hop * time_frames; // need at least one full crop's worth

    let mut manifest: Vec<String> = Vec::new();
    let (mut done, mut skipped_short, mut skipped_err) = (0usize, 0usize, 0usize);

    for (i, path) in files.iter().enumerate() {
        // Use the path string exactly as given (relative to wherever the user
        // ran it) so it matches what Python would store; a stable key is all
        // that matters since shard assignment is a pure hash of it.
        let key = path.to_string_lossy().to_string();

        let y = match crate::audio::decode_audio(path, sr) {
            Ok(y) => y,
            Err(e) => {
                eprintln!("  skip (decode error): {key}: {e}");
                skipped_err += 1;
                continue;
            }
        };
        if y.len() < min_samples {
            skipped_short += 1;
            continue;
        }

        let (mel, frames) = crate::audio::wav_to_unit_mel(&y, sr as f64, n_fft, hop, n_mels, db_floor, db_ceil);
        if frames < time_frames {
            skipped_short += 1;
            continue;
        }

        // Encode the mel. int8 (default): quantize [-1,1] -> [-127,127] with
        // a 2-byte magic header, ~4.6x smaller after zlib; the reader
        // auto-detects the header. f32: raw little-endian, byte-compatible
        // with the Python pipeline's caches.
        let payload: Vec<u8> = if int8 {
            let mut v = Vec::with_capacity(2 + mel.len());
            v.push(0xB3);
            v.push(0x08);
            v.extend(mel.iter().map(|&x| (x.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8));
            v
        } else {
            bytemuck::cast_slice(&mel).to_vec()
        };
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(4));
        enc.write_all(&payload).expect("zlib write");
        let blob = enc.finish().expect("zlib finish");

        let sid = shard_for_path_str(&key, num_shards);
        let conn = conns.entry(sid).or_insert_with(|| {
            let p = cache_path.join(shard_name_for(sid, num_shards));
            let c = Connection::open(&p).expect("open shard for write");
            c.execute("CREATE TABLE IF NOT EXISTS mels (filepath TEXT PRIMARY KEY, data BLOB NOT NULL)", [])
                .expect("create table");
            c
        });
        conn.execute("INSERT OR REPLACE INTO mels (filepath, data) VALUES (?1, ?2)", rusqlite::params![key, blob])
            .expect("insert row");

        manifest.push(key);
        done += 1;
        if (i + 1) % 50 == 0 || i + 1 == files.len() {
            println!("  {}/{} processed ({done} cached, {skipped_short} too short, {skipped_err} errors)", i + 1, files.len());
        }
    }

    // Manifest: JSON array of the cached paths (Python-compatible).
    let manifest_json = serde_json::to_string(&manifest).expect("serialize manifest");
    std::fs::write(cache_path.join("manifest.json"), manifest_json).expect("write manifest.json");

    println!(
        "build-cache done: {done} tracks cached across {} shards, {skipped_short} skipped (too short), {skipped_err} skipped (decode error). manifest.json written.",
        conns.len()
    );
}
