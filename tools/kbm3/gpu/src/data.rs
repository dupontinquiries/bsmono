//! Reads the exact on-disk cache produced by the Python pipeline:
//!   - cache/manifest.json      : JSON array of source file paths (index = position)
//!   - cache/db-SSS-EEE.sqlite  : shard, table `mels(filepath TEXT PK, data BLOB)`
//!   - each BLOB                : zlib-compressed row-major f32 [n_mels, frames],
//!                                values already mapped to [-1, 1]
//! No format change, so the existing 5,630-track cache is reused as-is.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;

use burn::prelude::*;
use burn::tensor::TensorData;
use rusqlite::{Connection, OpenFlags};

pub struct MelCache {
    cache_dir: PathBuf,
    shard_size: usize,
    n_mels: usize,
    time_frames: usize,
    manifest: Vec<String>,
    valid: Vec<usize>,
    conns: HashMap<usize, Connection>,
}

impl MelCache {
    pub fn open(cache_dir: &str, shard_size: usize, n_mels: usize, time_frames: usize) -> Self {
        let cache_dir = PathBuf::from(cache_dir);
        let manifest_txt = std::fs::read_to_string(cache_dir.join("manifest.json"))
            .expect("cache/manifest.json not found -- build the cache with the Python pipeline first");
        let manifest: Vec<String> =
            serde_json::from_str(&manifest_txt).expect("manifest.json is not a JSON string array");

        let mut c = MelCache {
            cache_dir,
            shard_size,
            n_mels,
            time_frames,
            manifest,
            valid: Vec::new(),
            conns: HashMap::new(),
        };
        c.build_valid();
        c
    }

    fn shard_name(&self, shard_id: usize) -> String {
        let start = shard_id * self.shard_size;
        let end = start + self.shard_size - 1;
        format!("db-{start:03}-{end:03}.sqlite")
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
    fn build_valid(&mut self) {
        let n = self.manifest.len();
        let shards: BTreeSet<usize> = (0..n).map(|i| i / self.shard_size).collect();
        for sid in shards {
            let Some(conn) = self.open_shard(sid) else {
                continue;
            };
            let mut stmt = conn.prepare("SELECT filepath FROM mels").unwrap();
            let present: HashSet<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            let start = sid * self.shard_size;
            let end = ((sid + 1) * self.shard_size).min(n);
            for i in start..end {
                if present.contains(&self.manifest[i]) {
                    self.valid.push(i);
                }
            }
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
        let shard_id = real_idx / self.shard_size;

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
        let flat: &[f32] = bytemuck::cast_slice(&raw);
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
