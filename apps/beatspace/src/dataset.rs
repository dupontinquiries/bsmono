//! Dataset: turn a folder of audio into mel-spectrogram crops for training.
//!
//! Each source clip is decoded once, analysed into `stems` mel planes plus a
//! beat phase per frame, and held **transposed** as one row per frame. That
//! layout makes a random time-crop of `time_frames` consecutive frames — or a
//! run of `seq_windows` consecutive crops — a single contiguous block, on
//! disk or in RAM, with no mmap.
//!
//! Cache file layout, v2 (little-endian):
//!   magic:  b"BSP2"  (4 bytes)
//!   n_mels: u32
//!   stems:  u32
//!   t:      u32
//!   data:   t rows of (stems * n_mels + 1) f32:
//!           [stem0 mel..., stem1 mel..., beat_phase]
//!
//! v1 (`BSPC`, header n_mels/t, rows of n_mels) is still READ, because
//! micsession writes it directly into a source directory and those crops are
//! recordings that exist nowhere else. A v1 row is one mixed stem with no
//! phase, so it only fits a `stems = 1` run; a v1 file in the CACHE is a
//! stale entry and is rebuilt.

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

const MAGIC_V1: &[u8; 4] = b"BSPC";
const MAGIC: &[u8; 4] = b"BSP2";

pub struct Dataset {
    clips: Vec<ClipMeta>,
    n_mels: usize,
    stems: usize,
    time_frames: usize,
    /// Frames between the starts of consecutive windows in a sequence.
    stride: usize,
}

/// Where a clip's frames live and how a row is laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    /// v1: `n_mels` values per row, one stem, no phase. Header is 12 bytes.
    V1,
    /// v2: `stems * n_mels + 1` values per row. Header is 16 bytes.
    V2 { stems: usize },
}

impl Layout {
    fn header_bytes(self) -> usize {
        match self {
            Layout::V1 => 12,
            Layout::V2 { .. } => 16,
        }
    }
    fn row_len(self, n_mels: usize) -> usize {
        match self {
            Layout::V1 => n_mels,
            Layout::V2 { stems } => stems * n_mels + 1,
        }
    }
    fn stems(self) -> usize {
        match self {
            Layout::V1 => 1,
            Layout::V2 { stems } => stems,
        }
    }
}

struct ClipMeta {
    path: PathBuf, // cache file
    t: usize,      // number of frames
    layout: Layout,
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
    // The window is bounded by RAM as well as by the batch multiple, because
    // the point of the window is to be the thing that FITS. Sizing it past
    // memory means the tail spills to disk and writes exactly what this is
    // meant to avoid — the batch multiple sets the ambition and memory sets
    // the ceiling.
    let mut budget = if cfg.cache_batches == 0 { usize::MAX }
                     else { cfg.cache_batches.saturating_mul(cfg.batch_size.max(1)) };
    let mem = mem_budget(cfg);
    if mem > 0 && budget != usize::MAX {
        // A conservative per-clip estimate: the real size is not known until
        // the clip is decoded, so this uses the observed order of magnitude
        // (~10 MiB of mel for a 3-minute track at this hop) and lets the
        // per-clip claim do the exact accounting. Wrong in either direction
        // only changes how many spill to disk, never correctness.
        let est_per_clip = 12u64 * 1024 * 1024;
        let fits = (mem / est_per_clip).max(1) as usize;
        if fits < budget {
            println!("[dataset] window capped by memory: {fits} clip(s), not {budget} \
({:.1} GiB budget at ~{} MiB each)", mem as f64 / 1.073_741_824e9, est_per_clip / 1_048_576);
            budget = fits;
        }
    }
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
    // The budget, set before the workers start. Everything that fits is held
    // and never written; the rest falls back to .bspc files.
    MEM_LEFT.store(mem_budget(cfg), std::sync::atomic::Ordering::Relaxed);
    let mem_start = mem_budget(cfg);

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
            "every clip was too short for one {}-frame sample ({} window(s) of {} at stride {}), \
or all failed to decode",
            min_frames(cfg), cfg.seq_windows, cfg.time_frames, cfg.window_stride()
        ));
    }
    println!(
        "[dataset] usable clips: {} (need >= {} frames each)",
        clips.len(),
        min_frames(cfg)
    );
    // Residency happened during the build, not after it: build_one claims from
    // the shared budget and keeps the mel it just computed rather than writing
    // it out and reading it back. A post-hoc pass would have written every file
    // first, which is the cost being avoided.
    let held = clips.iter().filter(|c| c.resident.is_some()).count();
    if held > 0 {
        let used = mem_start.saturating_sub(MEM_LEFT.load(std::sync::atomic::Ordering::Relaxed));
        println!("[dataset] in memory: {held} of {} clip(s), {:.1} MiB — {} disk write(s) avoided",
                 clips.len(), used as f64 / 1.048_576e6, held);
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
        stems: cfg.stems,
        time_frames: cfg.time_frames,
        stride: cfg.window_stride(),
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
                // A `.bspc` among the SOURCES is an already-built clip, not
                // audio to decode. That is how a microphone session's crops
                // reach the trainer: micsession writes this format directly, so
                // pointing --data-dir at a capture directory needs no decode
                // step and no new format.
                //
                // Sources only. A `.bspc` in cache_dir is still a cache entry
                // and still subject to trim_cache, which is exactly why
                // micsession refuses to write there.
                if ext == "bspc" || cfg.audio_exts.iter().any(|e| e == &ext) {
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
/// Bytes still available to hold clips in RAM this run.
///
/// Shared across the rayon workers, so the decision "RAM or disk" is made once
/// per clip against a live total rather than each thread assuming it has the
/// whole budget.
static MEM_LEFT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Claim `n` bytes, or fail. Compare-and-swap rather than fetch_sub, because
/// fetch_sub on a u64 that is about to go negative wraps to an enormous number
/// and every subsequent clip would think it had room.
fn claim_mem(n: u64) -> bool {
    use std::sync::atomic::Ordering::{Acquire, Relaxed};
    let mut cur = MEM_LEFT.load(Relaxed);
    loop {
        if cur < n { return false; }
        match MEM_LEFT.compare_exchange_weak(cur, cur - n, Acquire, Relaxed) {
            Ok(_) => return true,
            Err(actual) => cur = actual,
        }
    }
}

fn build_one(src: &Path, cfg: &Config) -> Result<Option<ClipMeta>> {
    // A source that IS a cache file is used where it lies: no decode, no copy
    // into cache_dir. Copying would put it under trim_cache's eviction, which
    // for captured audio that exists nowhere else would be deletion rather
    // than eviction.
    //
    // `try_read_header` does the validating — magic, n_mels agreement, length
    // against the header — so a corrupt or mismatched file is skipped with the
    // same rules as any other cache entry rather than trusted for its name.
    if src.extension().and_then(|e| e.to_str()) == Some("bspc") {
        // A v1 source is one mixed stem. It cannot stand in for a two-stem
        // row, and inventing a silent second stem would teach the generator
        // that drums are optional.
        return match try_read_header(src, cfg, true) {
            Ok(Some(meta)) if meta.layout.stems() != cfg.stems => Err(anyhow!(
                "{src:?} is a {}-stem crop and this run trains {} stems; use --stems {} \
or re-capture",
                meta.layout.stems(), cfg.stems, meta.layout.stems()
            )),
            Ok(Some(meta)) if meta.t >= min_frames(cfg) => Ok(Some(meta)),
            _ => Ok(None),
        };
    }

    let cache = cache_path(src, cfg);

    // If a valid cache already exists, just read its header.
    if let Ok(Some(meta)) = try_read_header(&cache, cfg, false) {
        if meta.t >= min_frames(cfg) {
            return Ok(Some(meta));
        } else {
            return Ok(None);
        }
    }

    let audio = crate::audio_io::decode_to_mono(src, cfg.sr)?;
    let mel = Mel::new(cfg);
    let a = mel.analyse(&audio, cfg.stems); // stems x (n_mels, T), phase (T)
    let t = a.t;
    if t < min_frames(cfg) {
        // RECORD THE REJECTION. Returning without writing meant every run
        // decoded every too-short clip again — full decode plus mel, for a
        // result that cannot change unless the file does. In a library where
        // most tracks are short that is the bulk of startup, repeated forever.
        //
        // A header with the real frame count IS the record: try_read_header
        // reads it back, sees t < time_frames, and returns Ok(None) without
        // touching the audio.
        //
        // THIS ONE IS WORTH THE WRITE and the others are not. It is 12 bytes,
        // it is written once per rejected clip ever rather than once per
        // rotation, and what it buys is not re-decoding a whole file to learn
        // again that it is too short. The full-mel writes below are megabytes
        // for data that is already in hand; this is a tombstone.
        let mut file = File::create(&cache)?;
        write_header(&mut file, cfg.n_mels, cfg.stems, t)?;
        return Ok(None);
    }
    let n_mels = cfg.n_mels;
    let layout = Layout::V2 { stems: cfg.stems };
    let row = layout.row_len(n_mels);

    // Transpose into rows: [stem0 mel..., stem1 mel..., phase] per frame.
    let mut tdata = vec![0f32; t * row];
    for (si, plane) in a.planes.iter().enumerate() {
        for m in 0..n_mels {
            for f in 0..t {
                tdata[f * row + si * n_mels + m] = plane[m * t + f];
            }
        }
    }
    for f in 0..t {
        tdata[f * row + row - 1] = a.phase[f];
    }

    // RAM FIRST, AND THEN NO WRITE AT ALL.
    //
    // The point is not saving a syscall per sample — it is not spending SSD
    // WRITE ENDURANCE to cache audio that fits in memory. And the rotation
    // added a commit ago makes that worse rather than better: each run caches a
    // DIFFERENT window, so a 664-clip library at ~10 MiB of mel per clip writes
    // gigabytes per full cycle, forever, for data that is derived and
    // reproducible from the source files.
    //
    // So a clip that fits in the budget is held and never written. The .bspc
    // file is the FALLBACK for what does not fit, not the primary store.
    let need = (t as u64) * (row as u64) * 4;
    if claim_mem(need) {
        return Ok(Some(ClipMeta {
            path: cache, t, layout, resident: Some(std::sync::Arc::new(tdata)),
        }));
    }

    let mut file = File::create(&cache)?;
    write_header(&mut file, n_mels, cfg.stems, t)?;
    let bytes: &[u8] = bytemuck_cast(&tdata);
    file.write_all(bytes)?;

    Ok(Some(ClipMeta { path: cache, t, layout, resident: None }))
}

/// Frames a clip must hold to yield one training sample: `seq_windows`
/// windows at `window_stride` apart.
fn min_frames(cfg: &Config) -> usize {
    cfg.time_frames + cfg.window_stride() * cfg.seq_windows.max(1).saturating_sub(1)
}

fn write_header(file: &mut File, n_mels: usize, stems: usize, t: usize) -> Result<()> {
    file.write_all(MAGIC)?;
    file.write_all(&(n_mels as u32).to_le_bytes())?;
    file.write_all(&(stems as u32).to_le_bytes())?;
    file.write_all(&(t as u32).to_le_bytes())?;
    Ok(())
}

/// Read a cache header. `accept_v1` is for SOURCE `.bspc` files (micsession's
/// crops); in the cache directory a v1 file is a stale entry and reads as
/// absent so it is rebuilt.
fn try_read_header(cache: &Path, cfg: &Config, accept_v1: bool) -> Result<Option<ClipMeta>> {
    if !cache.exists() {
        return Ok(None);
    }
    let mut f = File::open(cache)?;
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return Ok(None);
    }
    let mut b4 = [0u8; 4];
    let mut u32_at = |f: &mut File| -> Result<usize> {
        f.read_exact(&mut b4)?;
        Ok(u32::from_le_bytes(b4) as usize)
    };
    let (layout, n_mels, t) = if &magic == MAGIC {
        let n_mels = u32_at(&mut f)?;
        let stems = u32_at(&mut f)?;
        let t = u32_at(&mut f)?;
        (Layout::V2 { stems }, n_mels, t)
    } else if &magic == MAGIC_V1 && accept_v1 {
        let n_mels = u32_at(&mut f)?;
        let t = u32_at(&mut f)?;
        (Layout::V1, n_mels, t)
    } else {
        return Ok(None);
    };
    if n_mels != cfg.n_mels {
        return Ok(None); // built with a different config -> rebuild
    }
    if let Layout::V2 { stems } = layout {
        if stems != cfg.stems {
            return Ok(None); // a cache entry for the other stem count
        }
    }
    Ok(Some(ClipMeta {
        path: cache.to_path_buf(),
        t,
        layout,
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

    /// Read `frames` consecutive rows of one clip starting at `start`, as
    /// `(frames, row_len)` row-major in the clip's own layout.
    fn read_rows(&self, clip: &ClipMeta, start: usize, frames: usize) -> Result<Vec<f32>> {
        let row = clip.layout.row_len(self.n_mels);
        // Resident clips skip the File::open and the seek — two syscalls per
        // sample, times batch_size, times every step.
        match &clip.resident {
            Some(v) => Ok(v[start * row..(start + frames) * row].to_vec()),
            None => {
                let offset = clip.layout.header_bytes() + start * row * 4;
                let mut f = File::open(&clip.path)?;
                f.seek(SeekFrom::Start(offset as u64))?;
                let mut buf = vec![0u8; frames * row * 4];
                f.read_exact(&mut buf)?;
                Ok(f32s_from_le(&buf))
            }
        }
    }

    /// Transpose `frames` rows into `(stems, n_mels, frames)` planes and a
    /// `(frames,)` phase, in the dataset's stem count. A v1 row supplies its
    /// one stem and a zero phase.
    fn unpack(&self, rows: &[f32], layout: Layout, frames: usize, mel_out: &mut Vec<f32>, phase_out: &mut Vec<f32>) {
        let (n_mels, stems) = (self.n_mels, self.stems);
        let row = layout.row_len(n_mels);
        let base = mel_out.len();
        mel_out.resize(base + stems * n_mels * frames, 0.0);
        for s in 0..stems.min(layout.stems()) {
            for m in 0..n_mels {
                for fr in 0..frames {
                    mel_out[base + (s * n_mels + m) * frames + fr] = rows[fr * row + s * n_mels + m];
                }
            }
        }
        for fr in 0..frames {
            phase_out.push(match layout {
                Layout::V1 => 0.0,
                Layout::V2 { .. } => rows[fr * row + row - 1],
            });
        }
    }

    /// One random window: `(stems, n_mels, time_frames)` planes plus the
    /// beat phase of each frame.
    fn random_crop(&self) -> Result<(Vec<f32>, Vec<f32>)> {
        let (mel, phase) = self.random_sequence(1)?;
        Ok((mel, phase))
    }

    /// `k` consecutive windows of one clip, `stride` frames apart, as
    /// `(k, stems, n_mels, time_frames)` planes and `(k, time_frames)` phase.
    fn random_sequence(&self, k: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let mut rng = rand::thread_rng();
        let clip = &self.clips[rng.gen_range(0..self.clips.len())];
        let span = self.time_frames + self.stride * k.saturating_sub(1);
        if clip.t < span {
            return Err(anyhow!("clip {:?} has {} frames, a {k}-window sample needs {span}", clip.path, clip.t));
        }
        let max_start = clip.t - span;
        let start = if max_start == 0 { 0 } else { rng.gen_range(0..=max_start) };
        // One contiguous read covering every window, then each window is a
        // sub-range of it: windows overlap, so reading them separately would
        // read the overlap twice.
        let rows = self.read_rows(clip, start, span)?;
        let row = clip.layout.row_len(self.n_mels);
        let mut mel = Vec::with_capacity(k * self.stems * self.n_mels * self.time_frames);
        let mut phase = Vec::with_capacity(k * self.time_frames);
        for w in 0..k {
            let a = w * self.stride * row;
            let b = a + self.time_frames * row;
            self.unpack(&rows[a..b], clip.layout, self.time_frames, &mut mel, &mut phase);
        }
        Ok((mel, phase))
    }

    /// Sample a batch as a tensor of shape (B, stems, n_mels, time_frames).
    pub fn sample_batch(&self, batch: usize, dev: &Device) -> Result<Tensor> {
        Ok(self.sample_sequence_batch(batch, 1, dev)?.mel.squeeze(1)?)
    }

    /// Sample `batch` sequences of `k` consecutive windows each.
    ///
    /// Crops are independent, so the batch is assembled in parallel — with
    /// the cache on disk each one is an open, a seek and a read.
    pub fn sample_sequence_batch(&self, batch: usize, k: usize, dev: &Device) -> Result<SeqBatch> {
        let (n_mels, frames, stems) = (self.n_mels, self.time_frames, self.stems);
        let seqs: Vec<(Vec<f32>, Vec<f32>)> = (0..batch)
            .into_par_iter()
            .map(|_| self.random_sequence(k))
            .collect::<Result<_>>()?;
        let mut mel = Vec::with_capacity(batch * k * stems * n_mels * frames);
        let mut phase = Vec::with_capacity(batch * k * frames);
        for (m, p) in &seqs {
            mel.extend_from_slice(m);
            phase.extend_from_slice(p);
        }
        Ok(SeqBatch {
            mel: Tensor::from_vec(mel, (batch, k, stems, n_mels, frames), dev)?,
            phase: Tensor::from_vec(phase, (batch, k, frames), dev)?,
        })
    }
}

/// A batch of window sequences.
pub struct SeqBatch {
    /// `(B, K, stems, n_mels, time_frames)`.
    pub mel: Tensor,
    /// `(B, K, time_frames)` beat phase in [0, 1).
    pub phase: Tensor,
}

fn bytemuck_cast(data: &[f32]) -> &[u8] {
    // Safe: f32 has no padding/invalid bit patterns for read, and the slice
    // length is exact. We only ever write these bytes back into f32 of the same
    // layout on the same machine.
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4) }
}

/// Bytes -> f32, by value. The read buffer is a `Vec<u8>` whose alignment
/// is only guaranteed to 1, and `from_raw_parts` on a misaligned `*const
/// f32` is undefined behaviour, so this decodes rather than casts. A crop is
/// a few tens of thousands of floats; the copy is not the cost.
fn f32s_from_le(data: &[u8]) -> Vec<f32> {
    assert!(data.len() % 4 == 0);
    data.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

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

    /// A batch from a DISK-backed cache: the parallel sampler assembles the
    /// same (batch, 1, n_mels, frames) layout the serial one did, every crop
    /// transposed to (mel, frame), and a resident clip gives the same bytes
    /// as its file. Written through the cache format's own header.
    #[test]
    fn a_batch_from_disk_has_the_layout_and_the_values() {
        let dir = std::env::temp_dir().join(format!("bs-sampler-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (n_mels, t, frames) = (4usize, 6usize, 3usize);
        // (t, n_mels) row-major, value = frame*10 + mel, after a 12-byte header
        let mut bytes = vec![0u8; 12];
        let mut data = Vec::new();
        for fr in 0..t { for m in 0..n_mels { data.push((fr * 10 + m) as f32); } }
        bytes.extend_from_slice(bytemuck_cast(&data));
        let path = dir.join("clip.mel");
        std::fs::write(&path, &bytes).unwrap();
        let ds = Dataset {
            clips: vec![
                ClipMeta { path: path.clone(), t, layout: Layout::V1, resident: None },
                ClipMeta { path: path.clone(), t, layout: Layout::V1, resident: Some(std::sync::Arc::new(data.clone())) },
            ],
            n_mels, stems: 1, time_frames: frames, stride: frames,
        };
        let batch = 16;
        let tens = ds.sample_batch(batch, &Device::Cpu).unwrap();
        assert_eq!(tens.dims(), &[batch, 1, n_mels, frames]);
        let v: Vec<f32> = tens.flatten_all().unwrap().to_vec1().unwrap();
        for b in 0..batch {
            let crop = &v[b * n_mels * frames..(b + 1) * n_mels * frames];
            // out[m * frames + fr] = frame*10 + mel, for some start frame
            let start = (crop[0] / 10.0) as usize;
            for m in 0..n_mels { for fr in 0..frames {
                assert_eq!(crop[m * frames + fr], ((start + fr) * 10 + m) as f32, "batch {b} mel {m} frame {fr}");
            } }
            assert!(start + frames <= t);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
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
    fn a_clip_that_fits_in_memory_writes_no_file() {
        // The actual point. Not saving a syscall per sample — not spending SSD
        // WRITE ENDURANCE on a mel cache that is derived data, reproducible
        // from the source audio, and fits in RAM.
        //
        // The rotation makes this sharper rather than softer: each run caches a
        // different window, so a 664-clip library writes gigabytes per full
        // cycle if the writes are not avoided.
        MEM_LEFT.store(64 * 1024 * 1024, std::sync::atomic::Ordering::Relaxed);
        assert!(claim_mem(1024 * 1024), "a small clip should fit");
        assert!(claim_mem(60 * 1024 * 1024), "and so should a large one");
        assert!(!claim_mem(64 * 1024 * 1024), "but not past the budget");
    }

    #[test]
    fn an_exhausted_budget_does_not_wrap_into_an_enormous_one() {
        // fetch_sub on a u64 about to go negative wraps to ~1.8e19 and every
        // subsequent clip would believe it had room — holding the entire
        // library in RAM and getting the process killed. The claim is a
        // compare-and-swap for exactly this.
        MEM_LEFT.store(100, std::sync::atomic::Ordering::Relaxed);
        assert!(!claim_mem(101));
        assert_eq!(MEM_LEFT.load(std::sync::atomic::Ordering::Relaxed), 100,
                   "a refused claim must not move the counter");
        assert!(claim_mem(100));
        assert!(!claim_mem(1), "nothing left means nothing left");
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

    /// Build a v2 clip in memory: value = frame*100 + stem*10 + mel, phase =
    /// frame/t. Distinct in every field so a transposition error shows.
    fn v2_clip(n_mels: usize, stems: usize, t: usize) -> Vec<f32> {
        let row = stems * n_mels + 1;
        let mut data = vec![0f32; t * row];
        for fr in 0..t {
            for st in 0..stems { for m in 0..n_mels {
                data[fr * row + st * n_mels + m] = (fr * 100 + st * 10 + m) as f32;
            } }
            data[fr * row + row - 1] = fr as f32 / t as f32;
        }
        data
    }

    #[test]
    fn a_v2_sequence_batch_has_consecutive_windows_at_the_stride_from_disk_and_ram() {
        let dir = std::env::temp_dir().join(format!("bs-seq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (n_mels, stems, t, frames, stride, k) = (3usize, 2usize, 12usize, 4usize, 3usize, 3usize);
        let data = v2_clip(n_mels, stems, t);
        let path = dir.join("clip.bspc");
        {
            let mut f = File::create(&path).unwrap();
            write_header(&mut f, n_mels, stems, t).unwrap();
            f.write_all(bytemuck_cast(&data)).unwrap();
        }
        let ds = Dataset {
            clips: vec![
                ClipMeta { path: path.clone(), t, layout: Layout::V2 { stems }, resident: None },
                ClipMeta { path: path.clone(), t, layout: Layout::V2 { stems }, resident: Some(std::sync::Arc::new(data.clone())) },
            ],
            n_mels, stems, time_frames: frames, stride,
        };
        let batch = 12;
        let sb = ds.sample_sequence_batch(batch, k, &Device::Cpu).unwrap();
        assert_eq!(sb.mel.dims(), &[batch, k, stems, n_mels, frames]);
        assert_eq!(sb.phase.dims(), &[batch, k, frames]);
        let mel: Vec<f32> = sb.mel.flatten_all().unwrap().to_vec1().unwrap();
        let ph: Vec<f32> = sb.phase.flatten_all().unwrap().to_vec1().unwrap();
        let per_w = stems * n_mels * frames;
        for b in 0..batch {
            let start = (mel[b * k * per_w] / 100.0) as usize;
            assert!(start + frames + stride * (k - 1) <= t, "sequence runs off the clip");
            for w in 0..k {
                let base = (b * k + w) * per_w;
                for st in 0..stems { for m in 0..n_mels { for fr in 0..frames {
                    let want = ((start + w * stride + fr) * 100 + st * 10 + m) as f32;
                    assert_eq!(mel[base + (st * n_mels + m) * frames + fr], want, "b{b} w{w} s{st} m{m} f{fr}");
                } } }
                for fr in 0..frames {
                    let want = (start + w * stride + fr) as f32 / t as f32;
                    assert_eq!(ph[(b * k + w) * frames + fr], want, "phase b{b} w{w} f{fr}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_v1_source_is_read_as_one_stem_with_zero_phase_and_refused_for_two() {
        let dir = std::env::temp_dir().join(format!("bs-v1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (n_mels, t) = (4usize, 8usize);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC_V1);
        bytes.extend_from_slice(&(n_mels as u32).to_le_bytes());
        bytes.extend_from_slice(&(t as u32).to_le_bytes());
        let data: Vec<f32> = (0..t * n_mels).map(|i| i as f32).collect();
        bytes.extend_from_slice(bytemuck_cast(&data));
        let path = dir.join("crop-000000.bspc");
        std::fs::write(&path, &bytes).unwrap();

        let mut cfg = Config { n_mels, time_frames: 4, ..Config::default() };
        cfg.stems = 1;
        let meta = try_read_header(&path, &cfg, true).unwrap().expect("v1 source accepted");
        assert_eq!(meta.layout, Layout::V1);
        assert_eq!(meta.t, t);
        // In the CACHE a v1 file is stale, whatever the stem count.
        assert!(try_read_header(&path, &cfg, false).unwrap().is_none());
        let ds = Dataset { clips: vec![meta], n_mels, stems: 1, time_frames: 4, stride: 4 };
        let sb = ds.sample_sequence_batch(2, 2, &Device::Cpu).unwrap();
        assert_eq!(sb.mel.dims(), &[2, 2, 1, n_mels, 4]);
        let ph: Vec<f32> = sb.phase.flatten_all().unwrap().to_vec1().unwrap();
        assert!(ph.iter().all(|&p| p == 0.0), "a v1 row has no phase");

        cfg.stems = 2;
        let err = match build_one(&path, &cfg) { Err(e) => e.to_string(), Ok(_) => panic!("a v1 crop was accepted for two stems") };
        assert!(err.contains("1-stem crop") && err.contains("--stems 1"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cache_entry_for_the_other_stem_count_is_rebuilt() {
        let dir = std::env::temp_dir().join(format!("bs-stems-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.bspc");
        let mut f = File::create(&path).unwrap();
        write_header(&mut f, 8, 2, 100).unwrap();
        drop(f);
        let one = Config { n_mels: 8, stems: 1, ..Config::default() };
        let two = Config { n_mels: 8, stems: 2, ..Config::default() };
        assert!(try_read_header(&path, &one, false).unwrap().is_none());
        assert!(try_read_header(&path, &two, false).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn min_frames_covers_the_whole_sequence() {
        let c = Config { time_frames: 64, overlap_frames: 4, seq_windows: 3, ..Config::default() };
        assert_eq!(c.window_stride(), 60);
        assert_eq!(min_frames(&c), 64 + 60 * 2);
        let one = Config { seq_windows: 1, ..c };
        assert_eq!(min_frames(&one), 64);
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
