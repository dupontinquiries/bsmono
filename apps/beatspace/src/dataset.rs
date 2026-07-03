//! Dataset: turn a folder of audio into mel-spectrogram crops for training.
//!
//! Each source clip is decoded once, converted to a full mel spectrogram, and
//! written to a binary cache file storing the mel **transposed** as
//! `(T, n_mels)` row-major. That layout makes a random time-crop of
//! `time_frames` consecutive frames a single contiguous block on disk — we can
//! seek + read_exact without loading the whole clip or pulling in mmap.
//!
//! Cache file layout (little-endian):
//!   magic: b"BSPC"  (4 bytes)
//!   n_mels: u32
//!   t:      u32
//!   data:   t * n_mels  f32   (row r = frame r, n_mels values)

use anyhow::{anyhow, Result};
use rand::Rng;
use rayon::prelude::*;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use candle_core::{Device, Tensor};

use crate::config::Config;
use crate::dsp::Mel;

const MAGIC: &[u8; 4] = b"BSPC";

pub struct Dataset {
    clips: Vec<ClipMeta>,
    n_mels: usize,
    time_frames: usize,
}

struct ClipMeta {
    path: PathBuf, // cache file
    t: usize,      // number of frames
}

/// Build (or refresh) the mel cache for everything in `cfg.data_dir`.
/// Returns a ready-to-sample Dataset.
pub fn build_or_load(cfg: &Config) -> Result<Dataset> {
    fs::create_dir_all(&cfg.cache_dir)?;

    // 1. enumerate source audio
    let mut sources: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&cfg.data_dir)
        .map_err(|e| anyhow!("read data_dir {:?}: {e}", cfg.data_dir))?
    {
        let p = entry?.path();
        if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if cfg.audio_exts.iter().any(|e| e == &ext) {
                sources.push(p);
            }
        }
    }
    if sources.is_empty() {
        return Err(anyhow!(
            "no audio files found in {:?} (looked for {:?})",
            cfg.data_dir,
            cfg.audio_exts
        ));
    }
    sources.sort();
    println!("[dataset] {} source files", sources.len());

    // 2. build cache in parallel (skip ones already cached & up to date)
    let done = AtomicUsize::new(0);
    let total = sources.len();
    let results: Vec<Option<ClipMeta>> = sources
        .par_iter()
        .map(|src| {
            let r = build_one(src, cfg);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            match &r {
                Ok(Some(_)) => println!("[dataset] ({n}/{total}) cached {src:?}"),
                Ok(None) => println!("[dataset] ({n}/{total}) skipped {src:?}"),
                Err(e) => eprintln!("[dataset] ({n}/{total}) FAILED {src:?}: {e}"),
            }
            r.ok().flatten()
        })
        .collect();

    let clips: Vec<ClipMeta> = results.into_iter().flatten().collect();
    if clips.is_empty() {
        return Err(anyhow!(
            "every clip was too short for one {}-frame crop, or all failed to decode",
            cfg.time_frames
        ));
    }
    println!(
        "[dataset] usable clips: {} (need >= {} frames each)",
        clips.len(),
        cfg.time_frames
    );

    Ok(Dataset {
        clips,
        n_mels: cfg.n_mels,
        time_frames: cfg.time_frames,
    })
}

fn cache_path(src: &Path, cfg: &Config) -> PathBuf {
    let stem = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip".into());
    cfg.cache_dir.join(format!("{stem}.bspc"))
}

/// Decode + mel + write cache for one source. Returns Ok(Some(meta)) if usable,
/// Ok(None) if the clip is too short to crop.
fn build_one(src: &Path, cfg: &Config) -> Result<Option<ClipMeta>> {
    let cache = cache_path(src, cfg);

    // If a valid cache already exists, just read its header.
    if let Ok(Some(meta)) = try_read_header(&cache, cfg) {
        if meta.t >= cfg.time_frames {
            return Ok(Some(meta));
        } else {
            return Ok(None);
        }
    }

    let audio = crate::audio_io::decode_to_mono(src, cfg.sr)?;
    let mel = Mel::new(cfg);
    let (data, t) = mel.wav_to_mel(&audio); // (n_mels, T) row-major
    if t < cfg.time_frames {
        return Ok(None);
    }
    let n_mels = cfg.n_mels;

    // transpose (n_mels, T) -> (T, n_mels)
    let mut tdata = vec![0f32; t * n_mels];
    for m in 0..n_mels {
        for f in 0..t {
            tdata[f * n_mels + m] = data[m * t + f];
        }
    }

    let mut file = File::create(&cache)?;
    file.write_all(MAGIC)?;
    file.write_all(&(n_mels as u32).to_le_bytes())?;
    file.write_all(&(t as u32).to_le_bytes())?;
    let bytes: &[u8] = bytemuck_cast(&tdata);
    file.write_all(bytes)?;

    Ok(Some(ClipMeta { path: cache, t }))
}

fn try_read_header(cache: &Path, cfg: &Config) -> Result<Option<ClipMeta>> {
    if !cache.exists() {
        return Ok(None);
    }
    let mut f = File::open(cache)?;
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() || &magic != MAGIC {
        return Ok(None);
    }
    let mut b4 = [0u8; 4];
    f.read_exact(&mut b4)?;
    let n_mels = u32::from_le_bytes(b4) as usize;
    f.read_exact(&mut b4)?;
    let t = u32::from_le_bytes(b4) as usize;
    if n_mels != cfg.n_mels {
        return Ok(None); // cache built with a different config -> rebuild
    }
    Ok(Some(ClipMeta {
        path: cache.to_path_buf(),
        t,
    }))
}

impl Dataset {
    pub fn len(&self) -> usize {
        self.clips.len()
    }
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Read one random `(n_mels, time_frames)` crop, returned as a flat
    /// row-major `(n_mels, time_frames)` Vec (already transposed back).
    fn random_crop(&self) -> Result<Vec<f32>> {
        let mut rng = rand::thread_rng();
        let clip = &self.clips[rng.gen_range(0..self.clips.len())];
        let max_start = clip.t - self.time_frames;
        let start = if max_start == 0 {
            0
        } else {
            rng.gen_range(0..=max_start)
        };

        // contiguous block: rows [start, start+time_frames) each n_mels wide
        let n_mels = self.n_mels;
        let frames = self.time_frames;
        let header = 4 + 4 + 4;
        let offset = header + (start * n_mels) * 4;
        let mut f = File::open(&clip.path)?;
        f.seek(SeekFrom::Start(offset as u64))?;
        let mut buf = vec![0u8; frames * n_mels * 4];
        f.read_exact(&mut buf)?;
        let tdata: &[f32] = bytemuck_cast_slice(&buf); // (frames, n_mels)

        // transpose (frames, n_mels) -> (n_mels, frames)
        let mut out = vec![0f32; n_mels * frames];
        for fr in 0..frames {
            for m in 0..n_mels {
                out[m * frames + fr] = tdata[fr * n_mels + m];
            }
        }
        Ok(out)
    }

    /// Sample a batch as a tensor of shape (B, 1, n_mels, time_frames).
    pub fn sample_batch(&self, batch: usize, dev: &Device) -> Result<Tensor> {
        let n_mels = self.n_mels;
        let frames = self.time_frames;
        let mut all = Vec::with_capacity(batch * n_mels * frames);
        for _ in 0..batch {
            all.extend_from_slice(&self.random_crop()?);
        }
        let t = Tensor::from_vec(all, (batch, 1, n_mels, frames), dev)?;
        Ok(t)
    }
}

// ----------------------------------------------------------------------------
// tiny zero-dependency f32<->u8 reinterpret helpers (avoids the bytemuck dep)
// ----------------------------------------------------------------------------

fn bytemuck_cast(data: &[f32]) -> &[u8] {
    // Safe: f32 has no padding/invalid bit patterns for read, and the slice
    // length is exact. We only ever write these bytes back into f32 of the same
    // layout on the same machine.
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4) }
}

fn bytemuck_cast_slice(data: &[u8]) -> &[f32] {
    assert!(data.len() % 4 == 0);
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const f32, data.len() / 4) }
}
