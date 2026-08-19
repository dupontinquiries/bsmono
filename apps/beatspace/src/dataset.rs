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

    // 1. enumerate source audio, RECURSIVELY.
    //
    // This was a single read_dir. A library pointed at 124 directories holding
    // 730 files reported "no audio files found" — every track was one level
    // down, which is how every music library on disk is arranged.
    let mut sources: Vec<PathBuf> = Vec::new();
    collect_audio(&cfg.data_dir, cfg, &mut sources)
        .map_err(|e| anyhow!("read data_dir {:?}: {e}", cfg.data_dir))?;
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
    if let Some(evicted) = trim_cache(cfg)? {
        println!("[dataset] cache trimmed: evicted {evicted} clip(s) over budget");
    }

    Ok(Dataset {
        clips,
        n_mels: cfg.n_mels,
        time_frames: cfg.time_frames,
    })
}

/// Walk `dir` and everything under it, collecting audio by extension.
///
/// Depth-first with an explicit stack rather than recursion: a music library
/// can be nested arbitrarily and a symlink loop would blow the call stack,
/// where a stack-based walk just revisits and finishes. Unreadable
/// subdirectories are skipped rather than failing the run — one permission
/// error in a library of 124 folders should not cost the other 123.
fn collect_audio(dir: &Path, cfg: &Config, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    while let Some(d) = stack.pop() {
        // Canonicalised so a symlink pointing at an ancestor is visited once
        // rather than forever.
        let key = d.canonicalize().unwrap_or_else(|_| d.clone());
        if !seen.insert(key) {
            continue;
        }
        let entries = match fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                let ext = ext.to_ascii_lowercase();
                if cfg.audio_exts.iter().any(|e| e == &ext) {
                    out.push(p);
                }
            }
        }
    }
    Ok(())
}

/// Where a source's mel cache lives.
///
/// Keyed by the FULL PATH, not the file name. With a recursive walk two
/// libraries routinely hold `01 Intro.mp3`, and a name-keyed cache makes them
/// overwrite each other on every run — which is the cache "clearing itself"
/// while nothing deletes anything. The name is kept in the filename so the
/// directory stays readable, with the hash making it unique.
fn cache_path(src: &Path, cfg: &Config) -> PathBuf {
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip".into());
    let h = path_hash(src);
    // Truncated so a deep path does not produce a filename the filesystem
    // refuses; the hash carries the uniqueness.
    let short: String = name.chars().take(48).collect();
    cfg.cache_dir.join(format!("{short}.{h:016x}.bspc"))
}

/// A stable 64-bit hash of a path. FxHash-style, no dependency.
fn path_hash(p: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
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
        // RECORD THE REJECTION. Returning without writing meant every run
        // decoded every too-short clip again — full decode plus mel, for a
        // result that cannot change unless the file does. In a library where
        // most tracks are short that is the bulk of startup, repeated forever.
        //
        // A header with the real frame count IS the record: try_read_header
        // reads it back, sees t < time_frames, and returns Ok(None) without
        // touching the audio.
        let mut file = File::create(&cache)?;
        file.write_all(MAGIC)?;
        file.write_all(&(cfg.n_mels as u32).to_le_bytes())?;
        file.write_all(&(t as u32).to_le_bytes())?;
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


/// Keep the cache to `cache_batches * batch_size` clips, evicting the
/// least-recently-used.
///
/// The cache is a WORKING SET, not a copy of the library. At 5,630 sources a
/// full mel cache is tens of gigabytes for a trainer that reads `batch_size`
/// clips at a time, and the disk it fills is the disk the checkpoints need.
///
/// LRU by access time rather than by creation: a sampler drawing randomly
/// touches the same clips repeatedly within a run, and evicting by age would
/// throw out the ones currently in rotation. Where a filesystem does not track
/// atime, mtime is the fallback and the policy degrades to
/// least-recently-written, which is still better than unbounded.
///
/// Returns how many were evicted, or None when unbounded.
fn trim_cache(cfg: &Config) -> Result<Option<usize>> {
    if cfg.cache_batches == 0 {
        return Ok(None);
    }
    let budget = cfg.cache_batches.saturating_mul(cfg.batch_size.max(1));
    let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    for e in fs::read_dir(&cfg.cache_dir)?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("bspc") {
            continue;
        }
        let md = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let when = md
            .accessed()
            .or_else(|_| md.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((when, md.len(), p));
    }
    if entries.len() <= budget {
        return Ok(Some(0));
    }
    // Oldest first, and drop from the front until the count fits.
    entries.sort_by_key(|(when, _, _)| *when);
    let excess = entries.len() - budget;
    let mut evicted = 0;
    for (_, _, p) in entries.into_iter().take(excess) {
        // A failed removal is not fatal: the cache is derived, and a file the
        // trainer is mid-read on will be evicted on the next run instead.
        if fs::remove_file(&p).is_ok() {
            evicted += 1;
        }
    }
    Ok(Some(evicted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_in(dir: &Path) -> Config {
        let mut c = Config::default();
        c.data_dir = dir.to_path_buf();
        c.cache_dir = dir.join("cache");
        c
    }

    #[test]
    fn a_nested_library_is_found() {
        // The failure this fixes: 124 directories holding 730 files reported
        // "no audio files found", because the scan was one read_dir and every
        // track was a level down — which is how music libraries are arranged.
        let root = std::env::temp_dir().join(format!("bs-nest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for sub in ["", "Album A", "Album B/Disc 2", "deep/deeper/deepest"] {
            let d = if sub.is_empty() { root.clone() } else { root.join(sub) };
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("track.mp3"), b"x").unwrap();
        }
        fs::write(root.join("notes.txt"), b"not audio").unwrap();

        let cfg = cfg_in(&root);
        let mut found = Vec::new();
        collect_audio(&root, &cfg, &mut found).unwrap();
        assert_eq!(found.len(), 4, "nested tracks missed: {found:?}");
        assert!(!found.iter().any(|p| p.ends_with("notes.txt")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn same_named_tracks_in_different_folders_do_not_share_a_cache() {
        // With a recursive scan two albums routinely hold "01 Intro.mp3". A
        // name-keyed cache makes them overwrite each other every run — the
        // cache "clearing itself" while nothing deletes anything.
        let root = std::env::temp_dir().join(format!("bs-key-{}", std::process::id()));
        let cfg = cfg_in(&root);
        let a = cache_path(&root.join("Album A/01 Intro.mp3"), &cfg);
        let b = cache_path(&root.join("Album B/01 Intro.mp3"), &cfg);
        assert_ne!(a, b, "two sources share one cache file");
        // And the readable name survives, so the directory can be inspected.
        assert!(a.to_string_lossy().contains("01 Intro.mp3"));
        // Stable across calls: a cache keyed by something that changes per run
        // would never hit.
        assert_eq!(a, cache_path(&root.join("Album A/01 Intro.mp3"), &cfg));
    }

    #[test]
    fn a_very_long_name_still_produces_a_usable_filename() {
        // A deep path plus a long title can exceed the 255-byte limit most
        // filesystems enforce, and the failure is a write error mid-build.
        let root = std::env::temp_dir().join("bs-long");
        let cfg = cfg_in(&root);
        let long = "x".repeat(400);
        let p = cache_path(&root.join(format!("{long}.mp3")), &cfg);
        let name = p.file_name().unwrap().to_string_lossy().len();
        assert!(name < 120, "cache filename is {name} bytes");
    }

    #[test]
    fn the_cache_is_trimmed_to_the_batch_budget() {
        // The cache is a working set, not a copy of the library: 5,630 sources
        // is tens of gigabytes of mel for a trainer that reads batch_size
        // clips at a time, on the disk the checkpoints need.
        let root = std::env::temp_dir().join(format!("bs-trim-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cfg = cfg_in(&root);
        cfg.batch_size = 4;
        cfg.cache_batches = 2; // budget = 8
        fs::create_dir_all(&cfg.cache_dir).unwrap();
        for i in 0..20 {
            fs::write(cfg.cache_dir.join(format!("c{i}.bspc")), b"x").unwrap();
            // Distinct timestamps so the LRU order is defined.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let evicted = trim_cache(&cfg).unwrap().unwrap();
        assert_eq!(evicted, 12, "expected 20 - 8");
        let left = fs::read_dir(&cfg.cache_dir).unwrap().count();
        assert_eq!(left, 8);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zero_budget_keeps_everything() {
        // Unbounded is still right for a small library, and it is the old
        // behaviour — a new policy should not silently delete work for
        // somebody whose set already fit.
        let root = std::env::temp_dir().join(format!("bs-unb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cfg = cfg_in(&root);
        cfg.cache_batches = 0;
        fs::create_dir_all(&cfg.cache_dir).unwrap();
        for i in 0..5 {
            fs::write(cfg.cache_dir.join(format!("c{i}.bspc")), b"x").unwrap();
        }
        assert_eq!(trim_cache(&cfg).unwrap(), None);
        assert_eq!(fs::read_dir(&cfg.cache_dir).unwrap().count(), 5);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn trimming_only_touches_cache_files() {
        // The cache dir may hold anything a user put there, and evicting a
        // checkpoint because it was old would be unrecoverable.
        let root = std::env::temp_dir().join(format!("bs-only-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cfg = cfg_in(&root);
        cfg.batch_size = 1;
        cfg.cache_batches = 1;
        fs::create_dir_all(&cfg.cache_dir).unwrap();
        fs::write(cfg.cache_dir.join("keep.ckpt"), b"precious").unwrap();
        for i in 0..4 {
            fs::write(cfg.cache_dir.join(format!("c{i}.bspc")), b"x").unwrap();
        }
        trim_cache(&cfg).unwrap();
        assert!(cfg.cache_dir.join("keep.ckpt").exists(), "a non-cache file was evicted");
        let _ = fs::remove_dir_all(&root);
    }
}
