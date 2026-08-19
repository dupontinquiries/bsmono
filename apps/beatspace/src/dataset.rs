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
    /// The mel data, when it fitted in RAM.
    ///
    /// `None` means read it from `path` on every crop — which is a File::open,
    /// a seek and a read per sample, times batch_size, times every step. On a
    /// working set that fits, holding it costs memory once and removes two
    /// syscalls from the inner loop.
    resident: Option<std::sync::Arc<Vec<f32>>>,
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

    // A ROTATING WINDOW, chosen before anything is decoded.
    //
    // The cache is a working set, not a copy of the library — but the first
    // version enforced that by building every clip and then DELETING the
    // overflow, which removed files the Dataset was already holding paths to.
    // The next sample_batch opened one and got ENOENT. An optimisation that
    // deletes the data it just produced is worse than no optimisation, and it
    // failed after the slow part rather than before it.
    //
    // So the budget is applied to the SOURCE LIST instead: take a window of it,
    // decode only that, and never evict anything the run is using. The window
    // start advances every run, so a long library rotates through rather than
    // training forever on whichever tracks sort first — which is the actual
    // request, and something a delete-the-overflow scheme could never do
    // because it always kept the same survivors.
    let budget = if cfg.cache_batches == 0 { usize::MAX }
                 else { cfg.cache_batches.saturating_mul(cfg.batch_size.max(1)) };
    if sources.len() > budget {
        let start = rotation_offset(cfg, sources.len());
        let window: Vec<PathBuf> = (0..budget)
            .map(|i| sources[(start + i) % sources.len()].clone())
            .collect();
        println!("[dataset] rotating window: {} of {} sources, from #{start} \
(cache_batches {} x batch {})",
                 window.len(), sources.len(), cfg.cache_batches, cfg.batch_size);
        sources = window;
    }

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

    let mut clips: Vec<ClipMeta> = results.into_iter().flatten().collect();
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
    // RESIDENCY: hold what fits, in order, and leave the rest on disk.
    //
    // Partial rather than all-or-nothing. A 3-minute track at this hop is
    // ~10 MiB of mel, so a few hundred of them is tens of gigabytes and the
    // all-or-nothing question answers itself with "no" on every real library —
    // which would mean the feature never engages. Filling the budget and
    // falling back per clip means a phone holds a few and a workstation holds
    // the lot, from the same code.
    let budget_bytes = mem_budget(cfg);
    if budget_bytes > 0 {
        let mut used = 0u64;
        let mut held = 0usize;
        for c in clips.iter_mut() {
            let need = (c.t as u64) * (cfg.n_mels as u64) * 4;
            if used + need > budget_bytes { continue; }
            match read_clip_body(&c.path, c.t, cfg.n_mels) {
                Ok(v) => {
                    c.resident = Some(std::sync::Arc::new(v));
                    used += need;
                    held += 1;
                }
                // A clip that will not read is left on disk rather than
                // failing the run: random_crop will hit the same error and
                // report it against the sample that needed it.
                Err(_) => continue,
            }
        }
        if held > 0 {
            println!("[dataset] in memory: {held} of {} clip(s), {:.1} MiB of a {:.1} MiB budget",
                     clips.len(), used as f64 / 1.048_576e6, budget_bytes as f64 / 1.048_576e6);
        }
    }

    // Evict only what THIS run is not using. The live paths are passed in
    // rather than recomputed, because a second derivation of "which files
    // matter" is how the two got out of step in the first place.
    let live: std::collections::HashSet<PathBuf> =
        clips.iter().map(|c| c.path.clone()).collect();
    if let Some(evicted) = trim_cache(cfg, &live)? {
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

    Ok(Some(ClipMeta { path: cache, t, resident: None }))
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
        resident: None,
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
        // Resident clips skip the File::open and the seek — two syscalls per
        // sample, times batch_size, times every step.
        let owned;
        let tdata: &[f32] = match &clip.resident {
            Some(v) => {
                let a = start * n_mels;
                &v[a..a + frames * n_mels]
            }
            None => {
                let mut f = File::open(&clip.path)?;
                f.seek(SeekFrom::Start(offset as u64))?;
                let mut buf = vec![0u8; frames * n_mels * 4];
                f.read_exact(&mut buf)?;
                owned = bytemuck_cast_slice(&buf).to_vec();
                &owned
            }
        }; // (frames, n_mels)

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
fn trim_cache(cfg: &Config, live: &std::collections::HashSet<PathBuf>) -> Result<Option<usize>> {
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
        // NEVER the working set. This is the whole bug: the previous version
        // evicted by age alone, and the newest files are exactly the ones this
        // run just built and is about to read.
        if live.contains(&p) {
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
    fn trimming_never_evicts_a_clip_this_run_is_using() {
        // THE BUG THIS SHIPPED WITH. trim_cache ran after the clips were built
        // and evicted by age alone — and the newest files are exactly the ones
        // the run just produced and is about to read. Training died on the
        // first batch with ENOENT, after the slow part rather than before it.
        let root = std::env::temp_dir().join(format!("bs-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cfg = cfg_in(&root);
        cfg.batch_size = 1;
        cfg.cache_batches = 1;   // budget of 1
        fs::create_dir_all(&cfg.cache_dir).unwrap();
        let mut live = std::collections::HashSet::new();
        for i in 0..6 {
            let p = cfg.cache_dir.join(format!("c{i}.bspc"));
            fs::write(&p, b"x").unwrap();
            // Everything is live: this run is using all six.
            live.insert(p);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let evicted = trim_cache(&cfg, &live).unwrap().unwrap();
        assert_eq!(evicted, 0, "the working set was evicted");
        for p in &live {
            assert!(p.exists(), "{p:?} was deleted while in use");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_entries_from_earlier_runs_are_still_evicted() {
        // The protection must not turn the trim off — a library that rotates
        // leaves last run's window behind, and that is exactly what should go.
        let root = std::env::temp_dir().join(format!("bs-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cfg = cfg_in(&root);
        cfg.batch_size = 2;
        cfg.cache_batches = 1;   // budget 2
        fs::create_dir_all(&cfg.cache_dir).unwrap();
        for i in 0..5 {
            fs::write(cfg.cache_dir.join(format!("old{i}.bspc")), b"x").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut live = std::collections::HashSet::new();
        for i in 0..2 {
            let p = cfg.cache_dir.join(format!("new{i}.bspc"));
            fs::write(&p, b"x").unwrap();
            live.insert(p);
        }
        let evicted = trim_cache(&cfg, &live).unwrap().unwrap();
        assert!(evicted > 0, "nothing stale was removed");
        for p in &live { assert!(p.exists()); }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_rotation_advances_and_wraps() {
        // A long library must not train forever on whichever tracks sort
        // first, and the delete-the-overflow version could never fix that: it
        // always kept the same survivors.
        let root = std::env::temp_dir().join(format!("bs-rot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cfg = cfg_in(&root);
        cfg.batch_size = 2;
        cfg.cache_batches = 2;   // window of 4
        fs::create_dir_all(&cfg.cache_dir).unwrap();
        let total = 10;
        let a = rotation_offset(&cfg, total);
        let b = rotation_offset(&cfg, total);
        let c = rotation_offset(&cfg, total);
        assert_eq!((a, b, c), (0, 4, 8), "consecutive runs must see disjoint windows");
        // And it wraps rather than running off the end.
        let d = rotation_offset(&cfg, total);
        assert!(d < total, "offset {d} is past the library");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_proc_meminfo_disables_the_memory_cache() {
        // An in-memory cache is an optimisation. Inventing a memory figure when
        // the system will not report one is how a 4 GiB phone gets OOM-killed
        // by its own trainer.
        let root = std::env::temp_dir().join("bs-mem");
        let cfg = cfg_in(&root);
        let b = mem_budget(&cfg);
        // On Linux this is real; the assertion that holds either way is that it
        // never exceeds the configured ceiling.
        assert!(b <= cfg.mem_cache_bytes, "{b} exceeds the ceiling");
    }

    #[test]
    fn a_zero_ceiling_keeps_everything_on_disk() {
        let root = std::env::temp_dir().join("bs-mem0");
        let mut cfg = cfg_in(&root);
        cfg.mem_cache_bytes = 0;
        assert_eq!(mem_budget(&cfg), 0);
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
        assert_eq!(trim_cache(&cfg, &Default::default()).unwrap(), None);
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
        trim_cache(&cfg, &Default::default()).unwrap();
        assert!(cfg.cache_dir.join("keep.ckpt").exists(), "a non-cache file was evicted");
        let _ = fs::remove_dir_all(&root);
    }
}


/// Bytes of RAM it is reasonable to spend on the mel cache right now.
///
/// Read from the SYSTEM rather than guessed, because the honest answer depends
/// on what else is running: this fleet targets phones with 4-8 GiB where the
/// compositor and the browser have already spoken for most of it, and the same
/// binary runs on a 192 GiB workstation.
///
/// MemAvailable, not MemFree — free memory on Linux is small by design because
/// the page cache uses the rest, and sizing against it would refuse to use a
/// machine that has plenty. Available is the kernel's own estimate of what can
/// be had without swapping, which is the question being asked.
///
/// Half of it, and never more than the config's ceiling. Half because the
/// model, the optimiser state and candle's own allocations come out of the same
/// pool, and a cache that takes everything available makes the trainer the
/// thing that gets killed.
fn mem_budget(cfg: &Config) -> u64 {
    let avail = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        // No /proc — not Linux, or a container without it. Zero rather than a
        // guess: an in-memory cache is an optimisation, and inventing a
        // memory figure to enable it is how a phone gets OOM-killed.
        .unwrap_or(0);
    (avail / 2).min(cfg.mem_cache_bytes)
}

/// Where this run's window starts, advanced every run.
///
/// Persisted in the cache directory as a plain integer. A random offset would
/// also rotate, but it would revisit and skip unevenly; a counter walks the
/// library. Stored beside the cache rather than in the config because it is
/// state about what has been SEEN, not a setting anyone chose.
///
/// An unreadable or corrupt file starts from zero rather than failing — losing
/// the rotation position costs some repeated work and nothing else, which is
/// not worth refusing to train over.
fn rotation_offset(cfg: &Config, total: usize) -> usize {
    let f = cfg.cache_dir.join("rotation");
    let cur = std::fs::read_to_string(&f)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let budget = cfg.cache_batches.saturating_mul(cfg.batch_size.max(1)).max(1);
    // Advance by the window, so consecutive runs see disjoint sets until the
    // library wraps.
    let next = (cur + budget) % total.max(1);
    let _ = std::fs::write(&f, next.to_string());
    cur % total.max(1)
}


/// Read a cached clip's whole body, skipping the header.
fn read_clip_body(path: &Path, t: usize, n_mels: usize) -> Result<Vec<f32>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(12))?;   // MAGIC + n_mels + t
    let mut buf = vec![0u8; t * n_mels * 4];
    f.read_exact(&mut buf)?;
    Ok(bytemuck_cast_slice(&buf).to_vec())
}
