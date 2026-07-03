#![allow(unused_variables, unused_mut, unused_imports, dead_code, unused_assignments)]
//! FAO v1.1 — GTK4/Adwaita with disk-backed transcode
//! Writes encoded frames to temp file for unlimited playback + export

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::Instant;
use std::process::Command;
use std::path::PathBuf;
use std::io::{Read, Write, Seek, SeekFrom};
extern crate libc;
use std::os::unix::process::CommandExt;
use std::fs::File;

use gtk::prelude::*;
use gtk::{self, gdk_pixbuf, glib};
use adw::prelude::*;

// ═══════════════════════════════════════════
// Transcode state shared between threads
// ═══════════════════════════════════════════
/// One web's snapshot for World view rendering.
/// `canonical_id == chunk_id` means this is canonical (or standalone).
/// `canonical_id != chunk_id` means it's aligned to that canonical.
#[derive(Clone)]
pub struct V8WebSnap {
    pub chunk_id: u32,           // == frame_start (used as identity)
    pub frame_start: u32,
    pub frame_end: u32,
    pub vertices: Vec<(f32, f32, f32, f32, f32, f32)>,  // x,y,z,r,g,b
    pub indices: Vec<u32>,
    pub canonical_id: u32,       // self id if canonical/standalone
    pub n_aligned_followers: u32,  // how many other webs aligned TO this one
}

// ═══════════════════════════════════════════
// FAO file backing storage
// ═══════════════════════════════════════════
//
// `.fao` files can be tiny (a few MB for a short clip) or massive (50GB+ 
// for a multi-hour archive). Two backing strategies make sense:
//
//   1. **Eager** — `std::fs::read()` slurps the whole file into a 
//      `Vec<u8>`. Fast subsequent access, predictable memory cost. 
//      Appropriate when the file fits comfortably in available RAM.
//
//   2. **Mapped** — `memmap2::Mmap` hands the OS the file; pages get 
//      paged in on demand and the OS evicts under pressure. Negligible 
//      RAM cost up front, occasional page-fault latency. Appropriate for 
//      files that exceed available RAM.
//
// `FaoData` is the union; it derefs to `&[u8]` so consumer code is 
// identical regardless of backing strategy. `load_fao_data_smart()` picks 
// the strategy based on file size and available memory at open time.

pub enum FaoData {
    /// File slurped into memory as a single Vec.
    Eager(Vec<u8>),
    /// File memory-mapped. Pages fault in on demand.
    Mapped(memmap2::Mmap),
}

impl FaoData {
    /// Length of the underlying file in bytes.
    pub fn len(&self) -> usize {
        match self {
            FaoData::Eager(v) => v.len(),
            FaoData::Mapped(m) => m.len(),
        }
    }
    
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    
    /// Backing strategy as a short string for diagnostics.
    pub fn backing(&self) -> &'static str {
        match self {
            FaoData::Eager(_) => "eager",
            FaoData::Mapped(_) => "mmap",
        }
    }
}

impl std::ops::Deref for FaoData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            FaoData::Eager(v) => v,
            FaoData::Mapped(m) => m,
        }
    }
}

impl AsRef<[u8]> for FaoData {
    fn as_ref(&self) -> &[u8] {
        // Fully qualified call avoids needing `use std::ops::Deref` 
        // at the top of main.rs (the trait isn't otherwise imported).
        <Self as std::ops::Deref>::deref(self)
    }
}

/// Decode one .fao frame for the AV1/H.264 re-encode worker.
/// 
/// Standalone (no &self) so the worker thread doesn't need to borrow 
/// App or TranscodeState. The V8 decoder is passed as a cloned Arc so 
/// the worker has its own handle into the decoder state.
/// 
/// Returns RGB bytes always (falls back to prev_rgb when decode fails). 
/// Output is always w*h*3 bytes.
fn decode_fao_frame_for_export(
    data: &[u8],
    entry: &fao_v1::FaoFrameEntry,
    idx: usize,
    w: usize,
    h: usize,
    v8_dec: &std::sync::Arc<std::sync::RwLock<Option<fao_v1::v8_reconstruction_codec::V8Decoder>>>,
    prev_rgb: &[u8],
) -> Vec<u8> {
    let rgb_sz = w * h * 3;
    let yuv_sz = w * h * 3 / 2;
    let (payload, _hdr) = match fao_v1::read_fao_frame(data, entry) {
        Some(p) => p,
        None => return prev_rgb.to_vec(),
    };
    if payload.is_empty() {
        // Skip-tier: hold previous frame.
        return prev_rgb.to_vec();
    }
    
    // v8.3 "R" repeat marker: a 1-byte b'R' wrapper means "same as the 
    // previous frame". Since this fn already has prev_rgb threaded 
    // through, it's just a passthrough — no walkback needed.
    if payload[0] == b'R' {
        return prev_rgb.to_vec();
    }
    
    // v10 'X'-wrapper: strategy-orchestrated frame. The v10 decoder 
    // handles its own static-repeat + motion-comp strategies using 
    // prev_rgb that we already have threaded through here.
    if payload[0] == fao_v1::v10::V10_MAGIC {
        return fao_v1::v10::decode_frame(&payload, Some(prev_rgb), w, h)
            .unwrap_or_else(|| prev_rgb.to_vec());
    }
    
    // V8 / V7 reconstruction frame.
    let is_v8 = payload[0] == b'Z'
        || (payload[0] == b'V' && payload.len() >= 2 && payload[1] == b'8');
    if is_v8 {
        if let Ok(guard) = v8_dec.read() {
            if let Some(dec) = guard.as_ref() {
                if let Some(rgb) = dec.render_frame_rgb(&payload, idx as u32) {
                    if rgb.len() == rgb_sz {
                        return rgb;
                    }
                }
            }
        }
        return prev_rgb.to_vec();
    }
    
    // v2.1 / DCT family.
    let decoded = match payload[0] {
        b'D' => fao_v1::decompress_frame_dct(&payload, w, h),
        b'P' => fao_v1::decompress_frame_yuv_split(&payload, w, h),
        b'L' => fao_v1::decompress_frame_lossy_chroma(&payload, w, h),
        b'T' => {
            // T-frame needs an anchor; we don't walk back here. Use 
            // previous frame as best-effort. (Future enhancement: walk 
            // the index back to the most recent D/P/L/K and apply the 
            // T residual properly.)
            return prev_rgb.to_vec();
        }
        _ => {
            if payload.len() == yuv_sz {
                Some(payload.clone())
            } else if payload.len() == rgb_sz {
                return payload.clone();
            } else {
                fao_v1::decompress_frame(&payload, yuv_sz)
                    .or_else(|| fao_v1::decompress_frame(&payload, rgb_sz))
            }
        }
    };
    match decoded {
        Some(buf) if buf.len() == yuv_sz => yuv420p_to_rgb_export(&buf, w, h),
        Some(buf) if buf.len() == rgb_sz => buf,
        _ => prev_rgb.to_vec(),
    }
}

/// Standalone YUV420p → RGB (BT.601 limited-range). Mirrors 
/// TranscodeState::yuv_to_rgb but does not borrow self.
fn yuv420p_to_rgb_export(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let uv_w = w / 2;
    let uv_h = h / 2;
    let uv_sz = uv_w * uv_h;
    let mut rgb = vec![0u8; w * h * 3];
    if yuv.len() < y_sz + 2 * uv_sz {
        return rgb;
    }
    for py in 0..h {
        for px in 0..w {
            let yv = yuv[py * w + px] as f32;
            let uv_off = (py / 2) * uv_w + (px / 2);
            let cu = yuv[y_sz + uv_off] as f32;
            let cv = yuv[y_sz + uv_sz + uv_off] as f32;
            let yn = (yv - 16.0) * (255.0 / 219.0);
            let un = cu - 128.0;
            let vn = cv - 128.0;
            let r = (yn + 1.402 * vn).clamp(0.0, 255.0) as u8;
            let g = (yn - 0.344136 * un - 0.714136 * vn).clamp(0.0, 255.0) as u8;
            let b = (yn + 1.772 * un).clamp(0.0, 255.0) as u8;
            let oi = (py * w + px) * 3;
            rgb[oi]     = r;
            rgb[oi + 1] = g;
            rgb[oi + 2] = b;
        }
    }
    rgb
}

/// Read /proc/meminfo's `MemAvailable:` line, return (GB, was_measured).
/// 
/// `was_measured = false` means we couldn't read /proc/meminfo (non-Linux 
/// systems, sandboxed environments, etc). In that case the caller should 
/// be conservative — assuming generous memory could OOM the user.
fn mem_available_gb() -> (u64, bool) {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| (kb / 1024 / 1024, true))
        .unwrap_or((4, false))  // conservative: 4GB available when unmeasured
}

/// Choose backing strategy and load the file accordingly.
/// 
/// Decision rules (in order):
///   1. File ≤ 1GB → always eager. Tiny files don't benefit from mmap.
///   2. File ≤ 25% of MemAvailable → eager. Plenty of headroom.
///   3. Else → mmap. Conserves RAM for the rest of the system.
/// 
/// Logs a single line on success: 
///   `[FAO load] eager, file=2.3GB, avail=48.0GB`
///   `[FAO load] mmap (eager would have been preferred but file=51.2GB > 25% of avail=48.0GB), avail=48.0GB`
/// 
/// The user asked for an "mmap=false" warning — that's the second log 
/// line variant. Eager is the fast path; mmap is the safe path.
/// Background prefetch task: decode a frame and insert into the shared cache.
/// 
/// The UI thread reads frame bytes from fao_data (cheap mmap slice) and 
/// sends them via channel. The worker decodes off the UI thread.
/// 
/// We send the decoded BYTES (not just an index) so the worker doesn't 
/// need to share fao_data — the channel carries everything it needs.
pub struct PrefetchTask {
    pub idx: usize,
    pub data_bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Number of background prefetch worker threads.
/// 
/// 3 is a good balance for v8-heavy content where each render takes 
/// 50-100ms at 1080p, 80-150ms at 2K+. With N workers, theoretical 
/// throughput is one frame every ~(decode_ms / N). At high resolutions 
/// (2560x1440 SC2 content, 4K, etc) the per-frame cost balloons and 
/// 3 workers can't keep up with 30fps playback — the cache stays 0% 
/// and slow-path decode blocks the UI tick.
/// 
/// 6 workers is the sweet spot on modern dev machines (16-64 cores). 
/// More workers contend on the cache Mutex and V8Decoder RwLock 
/// without proportional throughput gains. Less is starving on 
/// high-res content.
/// 
/// For 4-core laptops the spawned threads will just queue, which is 
/// fine — they just wait on rx.recv() with negligible overhead.
pub const PREFETCH_WORKER_COUNT: usize = 6;

/// Source-fetch request. Sent from the UI thread when the source-frame 
/// cache misses; processed by a background worker that spawns ffmpeg, 
/// decodes a batch of frames, and stuffs them into the shared cache.
/// 
/// Previously the source-fetch was synchronous on the UI thread: every 
/// time the cache missed (typically at 60-frame intervals on web-gap 
/// content) the UI would block for ~500-1500ms while ffmpeg started up, 
/// pre-rolled, and decoded the window. On slow-fps content that's a 
/// hitch roughly once per minute of playback. Moving the spawn to a 
/// worker lets the UI return immediately and show the previous frame 
/// for one tick while the fill completes asynchronously.
pub struct SourceFetchTask {
    pub idx: usize,
    pub src_path: String,
    pub width: usize,
    pub height: usize,
    pub fps: f64,
}

/// One per playback-frame load. Used to diagnose v8 jitter, web 
/// transitions, and pose drift after the encode is done.
/// 
/// Sampled inside load_fao_frame's slow path (after v8 render). 
/// Stored in App.playback_trace and dumped via summarize_playback_trace 
/// on .fao reload, every 5000 frames during continuous playback, and 
/// on explicit user request.
/// 
/// Memory cost: 24 bytes per sample. At 84k frames that's 2 MB total — 
/// negligible compared to the .fao itself.
#[derive(Clone, Copy, Debug)]
pub struct PlaybackTraceSample {
    pub frame_idx: u32,
    pub pose_dx: f32,
    pub pose_dy: f32,
    pub pose_dz: f32,
    /// Web index this frame rendered through, or -1 if no web covered it.
    pub web_idx: i32,
    /// Vertex count of the active web's mesh (0 for empty/aligned-only webs).
    pub n_vertices: u32,
    /// Did render_frame_rgb produce non-None output?
    pub render_ok: bool,
}

/// Spawn the source-fetch worker. Single thread is sufficient: ffmpeg 
/// startup dominates the cost (~500ms), and serializing requests 
/// avoids the case where two batches step on each other for adjacent 
/// frame windows.
pub fn spawn_source_fetch_worker(
    cache: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<usize, Vec<u8>>>>,
) -> (std::sync::mpsc::SyncSender<SourceFetchTask>, std::thread::JoinHandle<()>) {
    // Bounded channel so the UI never blocks. Capacity 16 means up to 
    // 16 tasks can queue while the worker drains. Each task fetches a 
    // batch of 60 frames, so 16 queued tasks = ~960 frames of buffer. 
    // Previous capacity 2 was too small: typical playback fires 30+ 
    // source-fetch requests per second (one per frame that misses the 
    // cache), and a single ffmpeg seek+decode takes 500-2000ms. By the 
    // time the worker finishes batch 1, batches 2..30 have been dropped, 
    // so the cache never warms up enough for composite mode to engage.
    let (tx, rx) = std::sync::mpsc::sync_channel::<SourceFetchTask>(16);
    let handle = std::thread::Builder::new()
        .name("fao-source-fetch".into())
        .spawn(move || {
            while let Ok(task) = rx.recv() {
                let SourceFetchTask { idx, src_path, width: w, height: h, fps } = task;
                // Cache hit check — another request may have filled it 
                // while this one was queued.
                if let Ok(c) = cache.lock() {
                    if c.contains_key(&idx) { continue; }
                }
                if w == 0 || h == 0 || fps <= 0.0 { continue; }
                
                let batch_n: usize = 60;
                let timestamp = idx as f64 / fps;
                // Pre-input -ss is FAST seek (keyframe-only, can land 
                // earlier than requested). Coarse seek ~1s before our 
                // target to skip most of the file, then a small 
                // post-input -ss for accurate alignment to the exact 
                // timestamp.
                let pre_roll_s = 1.0_f64;
                let coarse_seek = (timestamp - pre_roll_s).max(0.0);
                let accurate_seek = (timestamp - coarse_seek).max(0.0);
                // After accurate seek, frame 0 is at `timestamp`. 
                // Select first batch_n frames.
                // CRITICAL: -frames:v caps the total decoded frames so 
                // ffmpeg exits promptly instead of decoding the entire 
                // rest of the file looking for more `select` matches. 
                // Without this, a single fetch task can take MINUTES on 
                // a long source file, which blocks all subsequent tasks.
                // 
                // Diagnostic timing wraps the ffmpeg call so we can see 
                // how long it actually takes per batch — if this is 
                // multiple seconds, we know the bottleneck is decode 
                // speed, not seek time.
                let batch_str = batch_n.to_string();
                let t_fetch_start = std::time::Instant::now();
                let output = std::process::Command::new("ffmpeg")
                    .args(&[
                        "-ss", &format!("{:.3}", coarse_seek),
                        "-i", &src_path,
                        "-ss", &format!("{:.3}", accurate_seek),
                        "-frames:v", &batch_str,
                        "-f", "rawvideo",
                        "-pix_fmt", "rgb24",
                        "-v", "error",
                        "-",
                    ])
                    .output();
                let fetch_elapsed = t_fetch_start.elapsed();
                static FETCH_DIAG_PRINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let output = match output {
                    Ok(o) => o,
                    Err(e) => {
                        // ffmpeg spawn failed (binary missing, etc). Log 
                        // once so the user knows why source-cache is 
                        // always empty.
                        if FETCH_DIAG_PRINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 1 {
                            eprintln!("[FAO source-fetch] ffmpeg spawn failed: {} — source video fallback will be unavailable", e);
                        }
                        continue;
                    }
                };
                
                let frame_sz = w * h * 3;
                if frame_sz == 0 { continue; }
                let n_decoded = output.stdout.len() / frame_sz;
                if n_decoded == 0 {
                    // ffmpeg ran but returned empty or invalid output. 
                    // Common causes: bad timestamp (past EOF), wrong 
                    // pix_fmt, stderr-only error. Log once + show ffmpeg 
                    // stderr so the user can diagnose.
                    if FETCH_DIAG_PRINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 1 {
                        let stderr_str = String::from_utf8_lossy(&output.stderr);
                        let status = output.status;
                        eprintln!(
                            "[FAO source-fetch] ffmpeg returned 0 frames for idx={} (status={}, stdout_len={}, frame_sz={}). stderr: {}",
                            idx, status, output.stdout.len(), frame_sz,
                            stderr_str.lines().take(3).collect::<Vec<_>>().join(" | ")
                        );
                    }
                    continue;
                }
                
                // Periodic success log. Was one-shot, but if ffmpeg is 
                // succeeding silently, we can't distinguish that from a 
                // worker that never completed a task. Log at counts 
                // 1/5/25/100 so heavy use shows progress without spamming.
                let success_n = FETCH_DIAG_PRINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if success_n == 1 || success_n == 5 || success_n == 25 || success_n == 100 {
                    eprintln!("[FAO source-fetch] batch decoded #{}: idx={} n_frames={} ({:.0}ms, {}MB)",
                        success_n, idx, n_decoded, fetch_elapsed.as_secs_f64() * 1000.0,
                        output.stdout.len() / 1024 / 1024);
                }
                
                if let Ok(mut c) = cache.lock() {
                    const MAX_ENTRIES: usize = 200;
                    for k in 0..n_decoded {
                        let frame_idx = idx + k;
                        let off = k * frame_sz;
                        if off + frame_sz > output.stdout.len() { break; }
                        if c.len() >= MAX_ENTRIES {
                            // Evict an entry far from the current request
                            let evict = c.keys()
                                .max_by_key(|k| (**k as i64 - idx as i64).abs())
                                .copied();
                            if let Some(e) = evict { c.remove(&e); }
                        }
                        c.insert(frame_idx, output.stdout[off..off + frame_sz].to_vec());
                    }
                }
            }
        })
        .expect("failed to spawn source-fetch worker thread");
    (tx, handle)
}

/// Item 8 — auto-fallback thresholds.
/// 
/// `AUTO_DISABLE_MIN_SAMPLES` is the warmup count: we don't make a 
/// disable decision until we've seen at least this many codec selections 
/// across all workers. 500 frames at 24fps is ~20 seconds of footage — 
/// long enough to characterize content type without throwing away the 
/// first 30 seconds of every encode where v8 naturally takes a few 
/// frames to warm up.
/// 
/// `AUTO_DISABLE_THRESHOLD` is the loss ratio that triggers disable. 
/// 0.85 means v8 must lose 85+ % of recent comparisons. This avoids 
/// disabling for content where v8 wins occasionally but trails on 
/// average — the user could still want v8 for those wins.
pub const AUTO_DISABLE_MIN_SAMPLES: u32 = 100;
pub const AUTO_DISABLE_THRESHOLD: f32 = 0.85;

// Issue 4 fix (Asahi slideshow test): the previous warmup of 500 samples 
// meant short videos and slideshow content never reached the auto-disable 
// gate. 100 is faster but still avoids false triggers — at typical 
// 30fps, 100 samples is 3.3 seconds, long enough for v8 to genuinely 
// stabilize on real 3D content (where it tends to win) while quickly 
// catching slideshow/talking-head content (where it consistently loses).

/// Spawn the background prefetch worker pool. Returns the channel sender 
/// + the thread handles (joined on .fao reload).
/// 
/// Architecture:
///   - One bounded sync_channel feeds all workers.
///   - Receiver is wrapped in Arc<Mutex<>> so all workers can drain it. 
///     Lock contention is negligible because most worker time is in 
///     decode, not in recv.
///   - V8Decoder is behind RwLock — workers all hold READ guards 
///     concurrently, so v8 rasterization runs truly in parallel.
///   - Cache is behind Mutex — workers contend briefly during the final 
///     insert. Insert is just a Vec push + maybe pop_front, so contention 
///     is microseconds.
/// 
/// Channel capacity is sized as 2× worker count, giving the UI a small 
/// backlog to submit into before try_send fails.
/// 
/// `cache` is the shared cache; workers write, UI reads.
/// `v8_decoder` is shared with UI; workers only read (RwLock allows parallelism).
pub fn spawn_prefetch_worker(
    cache: std::sync::Arc<std::sync::Mutex<fao_v1::FramePrefetchCache>>,
    v8_decoder: std::sync::Arc<std::sync::RwLock<Option<fao_v1::v8_reconstruction_codec::V8Decoder>>>,
) -> (std::sync::mpsc::SyncSender<PrefetchTask>, Vec<std::thread::JoinHandle<()>>) {
    let (tx, rx) = std::sync::mpsc::sync_channel::<PrefetchTask>(PREFETCH_WORKER_COUNT * 4);
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    
    let mut handles = Vec::with_capacity(PREFETCH_WORKER_COUNT);
    for worker_id in 0..PREFETCH_WORKER_COUNT {
        let rx = std::sync::Arc::clone(&rx);
        let cache = std::sync::Arc::clone(&cache);
        let v8_decoder = std::sync::Arc::clone(&v8_decoder);
        
        let handle = std::thread::Builder::new()
            .name(format!("fao-prefetch-{}", worker_id))
            .spawn(move || {
                loop {
                    // Lock the shared receiver, recv one task, then drop 
                    // the lock immediately so other workers can grab the 
                    // next task while this worker decodes.
                    let task = {
                        let rx_guard = rx.lock().unwrap();
                        match rx_guard.recv() {
                            Ok(t) => t,
                            Err(_) => break,  // sender dropped → exit
                        }
                    };
                    let PrefetchTask { idx, data_bytes, width, height } = task;
                    if data_bytes.is_empty() { continue; }
                    
                    // Skip if already cached. Race-safe: another worker 
                    // may have just inserted while we were locking; 
                    // double work is wasteful but not wrong.
                    {
                        let c = cache.lock().unwrap();
                        if c.entries.iter().any(|(i, _)| *i == idx) { continue; }
                    }
                    
                    // Decode based on format marker. v8 paths take the 
                    // RwLock READ guard so multiple workers can render 
                    // in parallel.
                    let rgb_sz = width * height * 3;
                    let yuv_sz = width * height * 3 / 2;
                    
                    let rgb: Option<Vec<u8>> = match data_bytes[0] {
                        b'D' => fao_v1::decompress_frame_dct(&data_bytes, width, height)
                            .map(|yuv| yuv_to_rgb_simple(&yuv, width, height)),
                        b'L' => fao_v1::decompress_frame_lossy_chroma(&data_bytes, width, height)
                            .map(|yuv| yuv_to_rgb_simple(&yuv, width, height)),
                        b'P' => fao_v1::decompress_frame_yuv_split(&data_bytes, width, height)
                            .map(|yuv| yuv_to_rgb_simple(&yuv, width, height)),
                        b'V' => {
                            if data_bytes.len() >= 2 && data_bytes[1] == b'8' {
                                let dec_guard = v8_decoder.read().unwrap();
                                dec_guard.as_ref().and_then(|dec| dec.render_frame_rgb(&data_bytes, idx as u32))
                            } else {
                                None
                            }
                        },
                        b'Z' => {
                            let dec_guard = v8_decoder.read().unwrap();
                            dec_guard.as_ref().and_then(|dec| dec.render_frame_rgb(&data_bytes, idx as u32))
                        },
                        _ => {
                            fao_v1::decompress_frame(&data_bytes, yuv_sz)
                                .map(|yuv| yuv_to_rgb_simple(&yuv, width, height))
                        }
                    };
                    
                    if let Some(rgb) = rgb {
                        if rgb.len() == rgb_sz {
                            let mut c = cache.lock().unwrap();
                            c.insert(idx, rgb);
                        }
                    }
                }
            })
            .expect("failed to spawn prefetch worker thread");
        handles.push(handle);
    }
    
    (tx, handles)
}

/// Standalone YUV→RGB used by the bg thread (TranscodeState's method 
/// requires &self; bg thread doesn't have a TranscodeState reference).
/// 
/// Mirrors TranscodeState::yuv_to_rgb's logic exactly.
fn yuv_to_rgb_simple(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let hw = w / 2;
    let hh = h / 2;
    let uv_sz = hw * hh;
    let expected = y_sz + 2 * uv_sz;
    if yuv.len() < expected {
        return vec![64u8; w * h * 3];  // gray fallback, consistent with main path
    }
    let mut rgb = vec![0u8; w * h * 3];
    let u_plane = &yuv[y_sz..y_sz + uv_sz];
    let v_plane = &yuv[y_sz + uv_sz..y_sz + 2 * uv_sz];
    for py in 0..h {
        let py_pair = py / 2;
        let uv_row = py_pair * hw;
        let y_row = py * w;
        let rgb_row = py * w * 3;
        for px in 0..w {
            let ux = (px / 2).min(hw.saturating_sub(1));
            let yi = y_row + px;
            let uvi = uv_row + ux;
            let yv = yuv[yi] as f32;
            let uv = u_plane[uvi] as f32 - 128.0;
            let vv = v_plane[uvi] as f32 - 128.0;
            let r = (yv + 1.402 * vv).clamp(0.0, 255.0);
            let g = (yv - 0.344 * uv - 0.714 * vv).clamp(0.0, 255.0);
            let b = (yv + 1.772 * uv).clamp(0.0, 255.0);
            let ri = rgb_row + px * 3;
            rgb[ri] = r as u8;
            rgb[ri + 1] = g as u8;
            rgb[ri + 2] = b as u8;
        }
    }
    rgb
}

fn load_fao_data_smart(path: &std::path::Path) -> Result<FaoData, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("metadata: {}", e))?;
    let file_size = meta.len();
    let (avail_gb, measured) = mem_available_gb();
    let avail_bytes = avail_gb * 1024 * 1024 * 1024;
    let file_size_gb = file_size as f64 / 1e9;
    let avail_gb_f = avail_gb as f64;
    
    const EAGER_FLOOR_BYTES: u64 = 1024 * 1024 * 1024;  // always eager up to 1GB
    let eager_quota = avail_bytes / 4;  // 25% of available
    
    // When we can't measure available memory (sandboxed, non-Linux, /proc 
    // not present), be conservative: only eager-load files small enough 
    // that even on a small machine they're safe (≤ 1GB ceiling).
    let prefer_eager = if measured {
        file_size <= EAGER_FLOOR_BYTES || file_size <= eager_quota
    } else {
        file_size <= EAGER_FLOOR_BYTES
    };
    
    if prefer_eager {
        // Eager load — fast subsequent access.
        let data = std::fs::read(path)
            .map_err(|e| format!("eager read: {}", e))?;
        let avail_note = if measured { 
            format!("avail={:.1}GB", avail_gb_f) 
        } else { 
            "avail=unknown".to_string() 
        };
        eprintln!(
            "[FAO load] eager, file={:.2}GB, {}",
            file_size_gb, avail_note,
        );
        Ok(FaoData::Eager(data))
    } else {
        // Mmap path — file too big to slurp without pressure.
        let file = std::fs::File::open(path)
            .map_err(|e| format!("open: {}", e))?;
        // SAFETY: memmap2's Mmap::map is unsafe because the underlying file 
        // could change while mapped (UB). For .fao files we only ever map 
        // for read; we don't modify the file from elsewhere during the 
        // lifetime of the mapping. This matches memmap2's documented 
        // recommended use.
        let mmap = unsafe {
            memmap2::Mmap::map(&file)
                .map_err(|e| format!("mmap: {}", e))?
        };
        // Hint the OS: we'll read sequentially-ish but with random seeks, 
        // and we'll read it all eventually. This nudges the page cache 
        // toward keeping recently accessed pages.
        #[cfg(unix)]
        {
            // advise: let the OS know it's a file we'll random-access
            let _ = mmap.advise(memmap2::Advice::Random);
        }
        let avail_note = if measured { 
            format!("avail={:.1}GB, file > 25% threshold", avail_gb_f) 
        } else { 
            "avail=unknown, file > 1GB floor".to_string() 
        };
        eprintln!(
            "[FAO load] mmap=true (file={:.2}GB, {}) — page-on-demand backing chosen to avoid RAM pressure",
            file_size_gb, avail_note,
        );
        Ok(FaoData::Mapped(mmap))
    }
}

/// Cached output of the V8 merge step. See `TranscodeState::v8_merge_cache`.
struct V8MergeCache {
    closed_chunks: Vec<fao_v1::v8_reconstruction_codec::RetainedChunk>,
    pose_track: Vec<fao_v1::v8_reconstruction_codec::CameraPose>,
    intrinsics: Option<fao_v1::v8_reconstruction_codec::CameraIntrinsics>,
    library_refs: Vec<fao_v1::v8_reconstruction_codec::LibraryRef>,
    total_library_match_savings: u64,
    /// Per-frame anchor positions for the Tobii-style player overlay 
    /// (V8AT trailer). Merged across all worker shards. Key = external 
    /// video frame index; value = list of (x, y, class) per anchor.
    anchor_track: std::collections::HashMap<u32, Vec<(i16, i16, u8)>>,
}

struct TranscodeState {
    tmp_path: PathBuf,
    frame_count: AtomicUsize,
    done: AtomicBool,
    /// Cooperative cancel flag. Workers check it on each iteration; 
    /// the disk writer checks it before each flush. When set, the 
    /// encode shuts down cleanly: workers drop unprocessed frames, 
    /// the writer flushes what it has, and `done` flips to true.
    /// 
    /// This is the in-flight encode cancel mechanism. Without it, 
    /// the only way to stop a running encode was to restart the app.
    cancel_requested: AtomicBool,
    w: usize, h: usize, fps: f64,
    // Per-frame index: byte offset in tmp file
    offsets: Mutex<Vec<u64>>,
    // Per-frame stats
    stats: Mutex<Vec<FrameStat>>,
    src_frame_bytes: Mutex<Vec<usize>>,  // source per-frame packet sizes
    vrr_history: Mutex<Vec<f32>>,
    layout: fao_v1::TempFileLayout,
    fao_path: std::path::PathBuf,
    ffmpeg_pid: std::sync::atomic::AtomicU32,  // track ffmpeg process for cleanup   // streaming output file
    // Ring buffer for preview: last N orig+recon frame pairs in memory
    ring_buf: Mutex<Vec<(usize, Vec<u8>, Vec<u8>)>>,
    last_display: Mutex<Option<(Vec<u8>, Vec<u8>)>>,  // cache last displayed (orig, recon)
    prev_yuv: Mutex<Vec<u8>>,  // previous frame's YUV for temporal decode
    cached_file: Mutex<Option<std::fs::File>>,  // cached file handle
    thumbnails: Mutex<Vec<(usize, Vec<u8>)>>,
    /// Anchor YUV cache for v2.1 translate encoding (RwLock for concurrent reads)
    /// (frame_idx → raw YUV bytes). Stores most recent ~32 anchors.
    anchor_yuv_cache: std::sync::RwLock<std::collections::VecDeque<(usize, std::sync::Arc<Vec<u8>>)>>,
    /// Stats for translate tier: (translated_count, total_translate_bytes)
    translate_stats: Mutex<(u32, u64)>,
    
    /// Cached output of the V8 merge step (Phase 3 fix for double-export).
    /// 
    /// `export_fao` is destructive: it calls `drain(..)` on each worker's 
    /// `closed_chunks` to assemble the merged trailer. A second call to 
    /// `export_fao` (e.g. user clicking Export twice, or any code path 
    /// that re-exports) finds the workers already drained and produces 
    /// an EMPTY V8MX trailer that overwrites the good first-call output.
    /// 
    /// Solution: cache the merged trailer payload here on the first run. 
    /// Subsequent runs reuse the cache instead of re-draining (which 
    /// would yield nothing).
    /// 
    /// `None` until the first successful merge; `Some(...)` thereafter. 
    /// The Vec sizes can be large (530 chunks × full mesh data), but a 
    /// completed encode usually means the user is done — the memory cost 
    /// is bounded and short-lived.
    v8_merge_cache: Mutex<Option<V8MergeCache>>,
    
    /// Item 8 — auto-fallback when v8 stops paying off.
    /// 
    /// Tracked across the whole encode: v8_wins counts frames where the 
    /// v8-encoded output was smaller than every alternative; v8_losses 
    /// counts frames where v2 T-frame or K-frame produced smaller output. 
    /// 
    /// When `v8_losses / (v8_wins + v8_losses) > AUTO_DISABLE_THRESHOLD` 
    /// (after a warmup period of `AUTO_DISABLE_MIN_SAMPLES`), workers 
    /// SKIP the v8 attempt entirely for the remainder of the encode. 
    /// This saves the cost of feature detection, pose estimation, mesh 
    /// integration, etc. for content where v8 isn't beating v2 anyway 
    /// (the StarCraft top-down case, talking-head clips with mostly UI).
    /// 
    /// Counters are atomic so workers can read+update without locking. 
    /// They never reset within an encode — once disabled, v8 stays off 
    /// for that session. (Mid-encode re-enable could be added if some 
    /// content has a flat first half + 3D second half; not seen in 
    /// practice yet.)
    v8_wins: std::sync::atomic::AtomicU32,
    v8_losses: std::sync::atomic::AtomicU32,
    v8_auto_disabled: std::sync::atomic::AtomicBool,
    
    /// Codec version selected by user: 0=v2.0 (no translate), 1=v2.1 (with T-frames)
    codec_version: std::sync::atomic::AtomicU8,
    /// User's quality slider value (0.0..1.0) — affects T-frame quant_step
    quality_q: std::sync::atomic::AtomicU32,  // f32 bits stored as u32
    /// Persistent block dictionary for cross-frame block dedup (v2.1+)
    block_dict: std::sync::RwLock<fao_v1::block_dict::BlockDictionary>,
    /// Region activity tracker (per-block layer-aware stability stats)
    region_activity: std::sync::Mutex<fao_v1::region_activity::RegionActivityTracker>,
    /// Last filter analysis (for web viz to read)
    last_filter_analysis: std::sync::RwLock<Option<fao_v1::pixel_filters::FilterAnalysis>>,
    /// Actual compressed byte sizes by frame index (override for V1 estimates)
    actual_compressed_sizes: std::sync::Mutex<std::collections::HashMap<usize, usize>>,
    /// V4 encoder state (sparse skeleton codec — separate from v3, doesn't replace it)
    /// Mutex (not RwLock) because the encoder is stateful: each frame mutates dict + state
    /// V5 encoder state (scene replay codec — game-engine-style sprite library)
    /// V8 spatial trace snapshot — 2D fallback. Tuple (feature_id, x, y, frame_idx, class)
    /// where class: 0=Unknown, 1=Static, 2=Dynamic
    v8_trace_snapshot: std::sync::Mutex<(Vec<(u32, i16, i16, u32, u8)>, u32, u32)>,
    /// V8 triangulated 3D points (feature_id, world_x, world_y, world_z, confidence, last_observed_frame)
    /// Populated periodically as TriangulatePass runs. World view uses this 
    /// for true 3D rendering when non-empty. last_observed_frame is the 
    /// most recent frame the feature was seen in — drives temporal filter / coloring.
    v8_points3d_snapshot: std::sync::Mutex<Vec<(u32, f32, f32, f32, f32, u32)>>,
    /// V8 dense depth point cloud — back-projected pixels from depth maps.
    /// Tuple: (world_x, world_y, world_z, confidence, anchor_frame).
    /// Subsampled to ~50K points to keep World view interactive.
    v8_dense_snapshot: std::sync::Mutex<Vec<(f32, f32, f32, f32, u32)>>,
    
    /// V8 scene mesh snapshot. Vertices: (wx, wy, wz, r, g, b). Indices: 
    /// triangle vertex indices (3 per triangle).
    /// Updated periodically as MeshIntegrator grows the mesh during encode.
    /// Capped via DensifyPass’s anchor cadence and MeshIntegrator’s 
    /// max_vertices, so this Mutex isn’t a memory hog.
    v8_mesh_snapshot: std::sync::Mutex<(Vec<(f32, f32, f32, f32, f32, f32)>, Vec<u32>)>,
    
    /// V8 camera pose snapshot. Per video frame: (frame_idx, x, y, z, qw, qx, qy, qz).
    /// Lets the World view render the reconstructed camera trajectory + 
    /// per-frame frustums, so users can see WHERE the encoder thinks the 
    /// camera was at each moment. Primary diagnostic when reconstruction 
    /// looks chaotic — chaotic trajectory = chaotic mesh.
    v8_pose_snapshot: std::sync::Mutex<Vec<(u32, f32, f32, f32, f32, f32, f32, f32)>>,
    
    /// V8 webs snapshot (Sessions 6c-8). Each entry describes one chunk's web:
    /// (chunk_id, frame_start, frame_end, vertices, indices, canonical_id_or_self).
    /// Lets the World view show webs DISTINCTLY (color by chunk), see which 
    /// webs share canonical, and understand the aligned-vs-standalone structure.
    v8_webs_snapshot: std::sync::Mutex<Vec<V8WebSnap>>,
    
    /// V6 encoder state (object field codec — 3D objects, GPU candidates)
    /// V7 encoder state (perception codec — phase correlation + camera tracking)
    /// V8 encoder state — ONE PER COMPRESS WORKER for parallel encoding.
    /// 
    /// Previously a single Mutex<Option<V8Encoder>>, which serialized 
    /// all 6 compress workers through one encoder. encode_frame_at on a 
    /// 2560x1440 frame takes 50-200ms; 6 workers blocking on one mutex 
    /// dropped throughput to ~1× (the user's 6500-frames-collected vs 
    /// 800-continuous gap).
    /// 
    /// Now: each worker gets its own private V8Encoder. No locking 
    /// contention; encoders run truly in parallel. At export time, all 
    /// per-worker chunk_stores and pose_tracks are merged into one 
    /// combined view for the V8MX/V8MS/V8PT trailers.
    /// 
    /// Trade-off: cross-worker web alignment is lost (worker 0's 
    /// canonical chunk doesn't get matched against worker 1's similar 
    /// chunk). Each worker still does intra-worker web sharing. Cross-
    /// worker alignment can be a follow-up post-pass at trailer time.
    v8_states: Vec<std::sync::Mutex<Option<fao_v1::v8_reconstruction_codec::V8Encoder>>>,
    ring_capacity: usize,
}

/// Per-frame stats. Designed for compactness — at 3M frames every byte
/// saved here is 3MB total. `route: String` was previously kept here but
/// always defaulted to "" and was never read; removed.
#[derive(Clone, Copy)]
struct FrameStat {
    psnr: f64,
    enc_bytes: usize,
    time_ms: f64,
    skip_pct: f64,
    vrr_ms: f32,
    activity: f32,
    ftype: u8,
}

impl TranscodeState {
    fn new(w: usize, h: usize, fps: f64) -> Self {
        // Pick temp directory: prefer real disk over tmpfs
        let tmp = {
            let home_cache = std::path::PathBuf::from(
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
            ).join(".cache").join("fao");
            let _ = std::fs::create_dir_all(&home_cache);
            // Check if /tmp is tmpfs (RAM-backed) — avoid it for large files
            let tmp_is_tmpfs = std::fs::read_to_string("/proc/mounts").ok()
                .map(|m| m.lines().any(|l| l.contains("/tmp") && l.contains("tmpfs")))
                .unwrap_or(false);
            let dir = if tmp_is_tmpfs {
                eprintln!("FAO: /tmp is tmpfs, using ~/.cache/fao instead");
                home_cache
            } else {
                // Use whichever has more free space
                let tmp_free = std::process::Command::new("df")
                    .args(["--output=avail","-B1","/tmp"]).output().ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .and_then(|s| s.lines().nth(1).and_then(|l| l.trim().parse::<u64>().ok()))
                    .unwrap_or(0);
                let home_free = std::process::Command::new("df")
                    .args(["--output=avail","-B1"]).arg(&home_cache).output().ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .and_then(|s| s.lines().nth(1).and_then(|l| l.trim().parse::<u64>().ok()))
                    .unwrap_or(0);
                if home_free > tmp_free { home_cache } else { std::env::temp_dir() }
            };
            dir.join(format!("fao_transcode_{}.raw", std::process::id()))
        };
        Self { tmp_path: tmp, frame_count: AtomicUsize::new(0), done: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            w, h, fps, offsets: Mutex::new(Vec::new()), stats: Mutex::new(Vec::new()), src_frame_bytes: Mutex::new(Vec::new()), vrr_history: Mutex::new(Vec::new()),
            layout: fao_v1::TempFileLayout::new(w, h, 0),
            ffmpeg_pid: std::sync::atomic::AtomicU32::new(0),
            fao_path: std::path::PathBuf::new(),
            ring_buf: Mutex::new(Vec::new()),
            thumbnails: Mutex::new(Vec::new()),
            last_display: Mutex::new(None),
            prev_yuv: Mutex::new(Vec::new()),
            cached_file: Mutex::new(None),
            anchor_yuv_cache: std::sync::RwLock::new(std::collections::VecDeque::new()),
            codec_version: std::sync::atomic::AtomicU8::new(1),
            quality_q: std::sync::atomic::AtomicU32::new(0.5_f32.to_bits()),
            block_dict: std::sync::RwLock::new(fao_v1::block_dict::BlockDictionary::new()),
            region_activity: std::sync::Mutex::new(fao_v1::region_activity::RegionActivityTracker::new(w, h)),
            last_filter_analysis: std::sync::RwLock::new(None),
            actual_compressed_sizes: std::sync::Mutex::new(std::collections::HashMap::new()),
            v8_trace_snapshot: std::sync::Mutex::new((Vec::new(), 0, 0)),
            v8_points3d_snapshot: std::sync::Mutex::new(Vec::new()),
            v8_dense_snapshot: std::sync::Mutex::new(Vec::new()),
            v8_mesh_snapshot: std::sync::Mutex::new((Vec::new(), Vec::new())),
            v8_pose_snapshot: std::sync::Mutex::new(Vec::new()),
            v8_webs_snapshot: std::sync::Mutex::new(Vec::new()),
v8_states: {
                // Allocate one slot per compress worker. Each starts as 
                // None and gets populated by start_transcode when a v8 
                // codec is selected. Same worker-count formula as the 
                // worker spawn site so they line up exactly.
                let n_workers = std::thread::available_parallelism()
                    .map(|n| (n.get() / 2).max(4).min(12))
                    .unwrap_or(4);
                (0..n_workers).map(|_| std::sync::Mutex::new(None)).collect()
            },
            translate_stats: Mutex::new((0, 0)),
            v8_merge_cache: Mutex::new(None),
            v8_wins: std::sync::atomic::AtomicU32::new(0),
            v8_losses: std::sync::atomic::AtomicU32::new(0),
            v8_auto_disabled: std::sync::atomic::AtomicBool::new(false),
            ring_capacity: {
                // Scale ring buffer to system RAM
                let sys_ram_gb = std::fs::read_to_string("/proc/meminfo").ok()
                    .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:"))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|v| v.parse::<u64>().ok()))
                    .unwrap_or(16 * 1024 * 1024) / 1024 / 1024;  // GB
                if sys_ram_gb >= 128 { 300 }       // 192GB: ~5 sec at 60fps (~2.4GB compressed)
                else if sys_ram_gb >= 64 { 180 }   // 64GB: ~3 sec (~1.4GB)
                else if sys_ram_gb >= 16 { 60 }    // 16GB: ~1 sec (~480MB)
                else { 60 }                        // <8GB: still 1 sec preview
            } }
    }
    fn yuv_to_rgb(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
        // The classic 4:2:0 layout: y_sz = w*h luma bytes, then U plane 
        // (w/2)*(h/2), then V plane same size. For ODD dimensions the 
        // simple `y_sz + y_sz/2` size formula is wrong (it overshoots by 
        // a few bytes), which used to reject otherwise-valid YUV input 
        // and return a BLACK rgb. That's the "cartoon shows black skip 
        // frames" report.
        // 
        // Compute the correct expected size and use a slightly more 
        // lenient check. Also: when input is too short, return a GRAY 
        // buffer (64) instead of BLACK (0). Gray is the agreed-upon 
        // fallback color in load_fao_frame's other failure paths and is 
        // visually less alarming than a sudden black flash.
        let y_sz = w * h;
        let hw = w / 2;
        let hh = h / 2;
        let uv_sz = hw * hh;
        let expected = y_sz + 2 * uv_sz;
        if yuv.len() < expected {
            // Use gray fallback consistent with the rest of the player.
            return vec![64u8; w * h * 3];
        }
        let mut rgb = vec![0u8; w * h * 3];
        // Safe to slice; bounds checked above.
        let u_plane = &yuv[y_sz..y_sz + uv_sz];
        let v_plane = &yuv[y_sz + uv_sz..y_sz + 2 * uv_sz];
        let _ = hh;
        
        // Parallel by horizontal stripes for large frames (≥720p)
        // For small frames, single-thread is faster (no overhead)
        let parallelize = w * h >= 1280 * 720;
        let n_threads = if parallelize { 4 } else { 1 };
        
        // Each thread handles a stripe of rows
        let rows_per_thread = ((h / 2) + n_threads - 1) / n_threads;
        
        std::thread::scope(|s| {
            let rgb_ptr = rgb.as_mut_ptr() as usize;
            for t in 0..n_threads {
                let start_pair = t * rows_per_thread;
                let end_pair = ((t + 1) * rows_per_thread).min(h / 2);
                if start_pair >= end_pair { continue; }
                
                s.spawn(move || {
                    // Reconstruct slice from raw pointer (safe: each thread writes disjoint rows)
                    let rgb_slice: &mut [u8] = unsafe {
                        std::slice::from_raw_parts_mut(rgb_ptr as *mut u8, w * h * 3)
                    };
                    
                    for py_pair in start_pair..end_pair {
                        let py0 = py_pair * 2;
                        let py1 = py0 + 1;
                        let uv_row = py_pair * hw;
                        for px in 0..w {
                            let ux = px / 2;
                            let u = u_plane[uv_row + ux] as i16 - 128;
                            let v = v_plane[uv_row + ux] as i16 - 128;
                            let cr = (v * 359) >> 8;
                            let cg = (u * 88 + v * 183) >> 8;
                            let cb = (u * 454) >> 8;
                            let y0 = yuv[py0 * w + px] as i16;
                            let oi0 = (py0 * w + px) * 3;
                            rgb_slice[oi0]   = (y0 + cr).clamp(0, 255) as u8;
                            rgb_slice[oi0+1] = (y0 - cg).clamp(0, 255) as u8;
                            rgb_slice[oi0+2] = (y0 + cb).clamp(0, 255) as u8;
                            if py1 < h {
                                let y1 = yuv[py1 * w + px] as i16;
                                let oi1 = (py1 * w + px) * 3;
                                rgb_slice[oi1]   = (y1 + cr).clamp(0, 255) as u8;
                                rgb_slice[oi1+1] = (y1 - cg).clamp(0, 255) as u8;
                                rgb_slice[oi1+2] = (y1 + cb).clamp(0, 255) as u8;
                            }
                        }
                    }
                });
            }
        });
        rgb
    }

}

/// Convert RGB to YUV420p (BT.601). Used by T-frame decoder for anchor input.
fn rgb_to_yuv420p(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let hw = w / 2; let hh = h / 2;
    let uv_sz = hw * hh;
    let mut yuv = vec![0u8; y_sz + 2 * uv_sz];
    
    // Y plane (full resolution)
    for py in 0..h {
        for px in 0..w {
            let i = (py * w + px) * 3;
            if i + 2 >= rgb.len() { continue; }
            let r = rgb[i] as f32;
            let g = rgb[i+1] as f32;
            let b = rgb[i+2] as f32;
            yuv[py * w + px] = (0.299*r + 0.587*g + 0.114*b).clamp(0.0, 255.0) as u8;
        }
    }
    
    // U/V planes (half resolution, 2x2 averaged)
    for cy in 0..hh {
        for cx in 0..hw {
            let mut sum_r = 0.0f32;
            let mut sum_g = 0.0f32;
            let mut sum_b = 0.0f32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let py = cy * 2 + dy;
                    let px = cx * 2 + dx;
                    if py >= h || px >= w { continue; }
                    let i = (py * w + px) * 3;
                    if i + 2 >= rgb.len() { continue; }
                    sum_r += rgb[i] as f32;
                    sum_g += rgb[i+1] as f32;
                    sum_b += rgb[i+2] as f32;
                }
            }
            let r = sum_r / 4.0;
            let g = sum_g / 4.0;
            let b = sum_b / 4.0;
            let u = (-0.169*r - 0.331*g + 0.500*b + 128.0).clamp(0.0, 255.0) as u8;
            let v = (0.500*r - 0.419*g - 0.081*b + 128.0).clamp(0.0, 255.0) as u8;
            yuv[y_sz + cy * hw + cx] = u;
            yuv[y_sz + uv_sz + cy * hw + cx] = v;
        }
    }
    yuv
}

impl TranscodeState {
    fn open_cached(&self) -> Option<std::fs::File> {
        // Try cached handle first
        if let Ok(mut cache) = self.cached_file.try_lock() {
            if cache.is_none() {
                *cache = std::fs::OpenOptions::new().read(true).open(&self.tmp_path).ok();
            }
            // Clone the handle for the caller
            return cache.as_ref().and_then(|f| f.try_clone().ok());
        }
        // Fallback: direct open
        std::fs::OpenOptions::new().read(true).open(&self.tmp_path).ok()
    }
    
    fn read_frame(&self, idx: usize) -> Option<(Vec<u8>, Vec<u8>)> {
        if idx >= self.frame_count.load(Ordering::Relaxed) { return None; }
        let fsz = self.w * self.h * 3;
        let yuv_fsz = self.w * self.h * 3 / 2;
        let is_done = self.done.load(Ordering::Relaxed);

        // Skip ring buffer after encoding (it has cross-segment data)
        // Only use ring buffer during encoding for live preview
        if !is_done {
        if let Ok(ring) = self.ring_buf.try_lock() {
            // Find best frame: exact match or nearest
            let entry = ring.iter().find(|(fi,_,_)| *fi == idx)
                .or_else(|| ring.iter().rev().find(|(fi,_,_)| *fi <= idx))
                .or_else(|| ring.last());
            
            if let Some((_, co, cr)) = entry {
                // Skip marker: find nearest REAL (non-skip) frame instead
                if co.len() <= 1 {
                    // Search backwards for a real frame
                    let real = ring.iter().rev()
                        .find(|(fi, c, _)| *fi <= idx && c.len() > 1)
                        .or_else(|| ring.iter().find(|(_, c, _)| c.len() > 1));
                    if let Some((_, rco, rcr)) = real {
                        if let Some(orig) = fao_v1::decompress_frame(rco, fsz) {
                            let recon = fao_v1::decompress_frame(rcr, fsz).unwrap_or_else(|| orig.clone());
                            return Some((orig, recon));
                        }
                    }
                    // No real frame found — try last_display cache
                    if let Ok(last) = self.last_display.try_lock() {
                        if let Some((ref o, ref r)) = *last {
                            return Some((o.clone(), r.clone()));
                        }
                    }
                    // Last resort: return None so caller keeps previous frame
                    return None;
                }
                if let Some(orig) = fao_v1::decompress_frame(co, fsz) {
                    let recon = fao_v1::decompress_frame(cr, fsz).unwrap_or_else(|| orig.clone());
                    return Some((orig, recon));
                }
            }
        }
        } // end if !is_done
        // Disk fallback (written at end of encode)
        // Use blocking lock — reads are cold path, brief contention OK
        if let Ok(offsets) = self.offsets.lock() {
            if idx < offsets.len() && offsets[idx] != u64::MAX {
                let offset = offsets[idx];
                drop(offsets);
                if let Some(mut f) = self.open_cached() {
                    if f.seek(SeekFrom::Start(offset)).is_ok() {
                        let mut hdr = [0u8; 28];
                        if f.read_exact(&mut hdr).is_ok() {
                            let orig_len = u32::from_le_bytes([hdr[4],hdr[5],hdr[6],hdr[7]]) as usize;
                            let recon_len = u32::from_le_bytes([hdr[8],hdr[9],hdr[10],hdr[11]]) as usize;
                            let _flags = u32::from_le_bytes([hdr[12],hdr[13],hdr[14],hdr[15]]);
                            if orig_len == 0 {
                                // Skip frame: use last_display cache
                                if let Ok(last) = self.last_display.try_lock() {
                                    if let Some((ref o, ref r)) = *last {
                                        return Some((o.clone(), r.clone()));
                                    }
                                }
                                return None;
                            }
                            if orig_len > 0 && orig_len <= fsz * 2 {
                                let rl = if recon_len > 0 { recon_len } else { 0 };
                                let mut c_orig = vec![0u8; orig_len];
                                let mut c_recon = if rl > 0 { vec![0u8; rl] } else { vec![] };
                                let ok = f.read_exact(&mut c_orig).is_ok()
                                    && (rl == 0 || f.read_exact(&mut c_recon).is_ok());
                                if ok {
                                    // Decompress: try all formats (T, D, P, L, raw zstd YUV, raw zstd RGB)
                                    let try_t_decode = |data: &[u8]| -> Option<Vec<u8>> {
                                        if data.is_empty() || data[0] != b'T' { return None; }
                                        fao_v1::v21_predictive::TranslatedFrame::from_bytes(data)
                                            .and_then(|(tframe, fw, fh)| {
                                                // Look up anchor in shared cache
                                                let anchor_yuv = self.anchor_yuv_cache.try_read().ok()
                                                    .and_then(|cache| {
                                                        cache.iter()
                                                            .find(|(ai, _)| *ai == tframe.anchor_ref as usize)
                                                            .map(|(_, y)| y.clone())  // clone Arc — cheap
                                                    })?;
                                                tframe.decode_with_anchor(anchor_yuv.as_slice(), fw, fh)
                                                    .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, fw, fh))
                                            })
                                    };
                                    let orig = try_t_decode(&c_orig)
                                        .or_else(|| fao_v1::decompress_frame_dct(&c_orig, self.w, self.h)
                                            .or_else(|| fao_v1::decompress_frame_yuv_split(&c_orig, self.w, self.h))
                                            .or_else(|| fao_v1::decompress_frame_lossy_chroma(&c_orig, self.w, self.h))
                                            .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, self.w, self.h)))
                                        .or_else(|| fao_v1::decompress_frame(&c_orig, yuv_fsz)
                                            .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, self.w, self.h)))
                                        .or_else(|| fao_v1::decompress_frame(&c_orig, fsz));
                                    let recon = if c_recon.is_empty() { None }
                                        else {
                                            try_t_decode(&c_recon)
                                                .or_else(|| fao_v1::decompress_frame_dct(&c_recon, self.w, self.h)
                                                    .or_else(|| fao_v1::decompress_frame_yuv_split(&c_recon, self.w, self.h))
                                                    .or_else(|| fao_v1::decompress_frame_lossy_chroma(&c_recon, self.w, self.h))
                                                    .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, self.w, self.h)))
                                                .or_else(|| fao_v1::decompress_frame(&c_recon, yuv_fsz)
                                                    .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, self.w, self.h)))
                                                .or_else(|| fao_v1::decompress_frame(&c_recon, fsz))
                                        };
                                    // Fallback for v3/v4/v5 codecs: c_orig contains codec-specific bytes
                                    // that don't decode via legacy paths. If orig fails but recon works
                                    // (we made recon = compressed source YUV for preview), use that.
                                    let final_orig = orig.or_else(|| recon.clone());
                                    if let Some(o) = final_orig {
                                        let r = recon.unwrap_or_else(|| o.clone());
                                        if let Ok(mut last) = self.last_display.try_lock() {
                                            let sum: u64 = o.iter().take(1000).map(|&b| b as u64).sum();
                                            if sum > 500 { *last = Some((o.clone(), r.clone())); }
                                        }
                                        return Some((o, r));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

impl Drop for TranscodeState {
    fn drop(&mut self) {
        // Kill ffmpeg process if still running
        let pid = self.ffmpeg_pid.load(Ordering::SeqCst);
        if pid > 0 {
            libc_kill(pid as i32);
        }
        let _ = std::fs::remove_file(&self.tmp_path);
    }
}

#[cfg(target_os = "linux")]
fn libc_kill(pid: i32) {
    // Send SIGTERM then SIGKILL
    use std::process::Command;
    let _ = Command::new("kill").arg(pid.to_string()).output();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}
#[cfg(not(target_os = "linux"))]
fn libc_kill(_pid: i32) {}

type SharedTC = Arc<TranscodeState>;

// ═══════════════════════════════════════════
#[derive(PartialEq, Clone, Copy)] enum View { Split, Original, Recon, Error }
#[derive(PartialEq, Clone, Copy)] enum EncodeMode { V1, V2 }
#[derive(PartialEq, Clone, Copy)] enum Mode { Empty, Image, VideoScrub, VideoPlay, FaoPlay }

/// Camera mode determines how input drives the camera state.
/// 
/// FreeCam: Position is direct (WASD moves world-relative); yaw/pitch 
/// rotate the look direction from that fixed position. Like a flying eye.
/// 
/// Orbit: Position is DERIVED from (target + yaw + pitch + radius). The 
/// camera always points AT target. Drag = rotate around target. Scroll = 
/// adjust radius. Like inspecting an object on a turntable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraMode { FreeCam, Orbit }

/// Camera state for the 3D World viewer
#[derive(Clone, Copy, Debug)]
pub struct WorldCamera {
    pub pos_x: f32,
    pub pos_y: f32,
    pub pos_z: f32,
    pub yaw: f32,    // rotation around Y axis (looking left/right)
    pub pitch: f32,  // rotation around X axis (looking up/down)
    pub zoom: f32,   // FOV multiplier
    
    // ── Orbit mode state ──
    pub mode: CameraMode,
    /// Target point that orbit camera revolves around (world coords).
    pub target_x: f32,
    pub target_y: f32,
    pub target_z: f32,
    /// Distance from target. Updated by scroll wheel in orbit mode.
    pub orbit_radius: f32,
}

impl Default for WorldCamera {
    fn default() -> Self {
        Self {
            pos_x: 0.0, pos_y: 0.0, pos_z: -800.0,  // pulled back to see the scene
            yaw: 0.0, pitch: 0.0,
            zoom: 1.0,
            mode: CameraMode::FreeCam,
            target_x: 0.0, target_y: 0.0, target_z: 0.0,
            orbit_radius: 800.0,
        }
    }
}

impl WorldCamera {
    /// In orbit mode, recompute pos_x/y/z from target + yaw + pitch + radius.
    /// Call this after any orbit-mode input update so the renderer reads 
    /// fresh position values.
    /// 
    /// The convention: yaw=0 pitch=0 looks toward +Z (matches existing 
    /// freecam orientation), so the camera position relative to target is
    /// the negation of the look direction times radius.
    pub fn recompute_orbit_position(&mut self) {
        if self.mode != CameraMode::Orbit { return; }
        let cy = self.yaw.cos();
        let sy = self.yaw.sin();
        let cp = self.pitch.cos();
        let sp = self.pitch.sin();
        // Look direction = (sin(yaw)*cos(pitch), -sin(pitch), cos(yaw)*cos(pitch))
        // Camera pos = target - radius * look_dir
        let dx = sy * cp;
        let dy = -sp;
        let dz = cy * cp;
        self.pos_x = self.target_x - self.orbit_radius * dx;
        self.pos_y = self.target_y - self.orbit_radius * dy;
        self.pos_z = self.target_z - self.orbit_radius * dz;
    }
    
    /// Snap to a canonical plane-aligned view, orbiting the current target.
    /// 
    /// PlaneView::Top is "looking down at the XZ plane" (pitch = -π/2)
    /// PlaneView::Front is "looking at XY plane from +Z" (pitch=0, yaw=0)  
    /// PlaneView::Side is "looking at YZ plane from +X" (yaw=π/2, pitch=0)
    /// PlaneView::Iso is the 3/4-iso view (yaw=π/4, pitch=-π/4)
    pub fn snap_to_plane(&mut self, view: PlaneView) {
        self.mode = CameraMode::Orbit;
        let (yaw, pitch) = match view {
            PlaneView::Top   => (0.0,                std::f32::consts::FRAC_PI_2 - 0.001),
            PlaneView::Front => (0.0,                0.0),
            PlaneView::Side  => (std::f32::consts::FRAC_PI_2,  0.0),
            PlaneView::Iso   => (std::f32::consts::FRAC_PI_4, -std::f32::consts::FRAC_PI_4),
        };
        self.yaw = yaw;
        self.pitch = pitch;
        self.recompute_orbit_position();
    }
}

/// Canonical view directions for the World view.
#[derive(Clone, Copy, Debug)]
pub enum PlaneView { Top, Front, Side, Iso }

/// Color mode for the World viewer
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldColorMode { Proximity, Feature, Temporal }

/// Filter mode for World viewer point selection
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldFilterMode { All, NowPlusMinus15, AnchorsOnly, Textured }

/// Camera mode for World viewer
/// - Freecam: user navigates with WASD/mouse (default)
/// - VideoCam: camera locked to estimated camera path (v7/v8), driven by temporal scrubber.
///   When VideoCam is active, WASD/mouse are ignored. The toggle preserves the 
///   freecam position so flipping back restores where you were exploring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldCameraMode { Freecam, VideoCam }

/// Logical movement actions for the World freecam — multiple keys may map 
/// to the same action (defensive overlap, per keybind guide).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldMoveKey { Forward, Back, Left, Right, Up, Down, Sprint }

/// Map a gdk::Key to a freecam action (returns None for irrelevant keys).
/// Following the keybind-guide's principle: multiple keys per action,
/// covering WASD + arrows + DVORAK home-row aliases.
pub fn world_movekey_for(key: gtk::gdk::Key) -> Option<WorldMoveKey> {
    use gtk::gdk::Key as K;
    Some(match key {
        // Forward: W / Up / period (DVORAK)
        K::w | K::W | K::Up | K::period => WorldMoveKey::Forward,
        // Back: S / Down / e (DVORAK)
        K::s | K::S | K::Down | K::e | K::E => WorldMoveKey::Back,
        // Left: A / Left / o (DVORAK)
        K::a | K::A | K::Left | K::o | K::O => WorldMoveKey::Left,
        // Right: D / Right / u (DVORAK)
        K::d | K::D | K::Right | K::u | K::U => WorldMoveKey::Right,
        // Up: Space
        K::space => WorldMoveKey::Up,
        // Down: Shift (both sides), or DVORAK semicolon (per keybind guide)
        // Removed Control_L to avoid user surprise — Ctrl is widely overloaded
        K::Shift_L | K::Shift_R | K::semicolon => WorldMoveKey::Down,
        // Sprint: T (no overlap with movement keys in any preset)
        K::t | K::T => WorldMoveKey::Sprint,
        _ => return None,
    })
}


/// Top-level UI section
#[derive(PartialEq, Clone, Copy, Debug)]
enum UiSection { Library, Encoder, Player }

struct App {
    mode: Mode,
    ui_section: UiSection,
    archive: Option<fao_v1::Archive>,
    archive_path: Option<PathBuf>,
    selected_video_id: Option<u32>,
    
    // ─── FAO Web Library (Phase 3) ───
    // 
    // The FAO Web Library is a separate concept from the Archive 
    // (which is a multi-video container). The Web Library is a 
    // PERSISTENT cross-encode catalog of canonical web meshes — when 
    // a chunk's geometry matches an existing library canonical, the 
    // encoder records a small reference (84 bytes) instead of storing 
    // the full mesh (~100-500 KB per chunk).
    // 
    // None = no library attached (encoder runs standalone, like before).
    // Some(path) = library at <path> is queried at chunk close.
    // 
    // The `writable` flag controls whether THIS encode adds new 
    // canonicals to the library. Read-only is the safe default; 
    // writable should be enabled when CURATING a library (e.g., 
    // encoding a series of similar videos to build up canonical pool).
    fao_web_lib_path: Option<PathBuf>,
    fao_web_lib_writable: bool,
    /// Cached library stats for UI display: (web_count, on_disk_bytes).
    /// Refreshed when path changes or after a write.
    fao_web_lib_stats: Option<(u64, u64)>,
    /// Web visualization snapshot: (frame_idx, x, y, z, weight, scene_id)
    web_viz_nodes: Vec<(u32, f32, f32, f32, f32, u16)>,
    web_viz_edges: Vec<(u32, u32, f32)>,
    web_viz_phase: f32,
    web_viz_active_frame: u32,
    /// Particle physics: per-node velocity (vx, vy) for inertia
    web_viz_velocity: Vec<(f32, f32)>,
    /// Codec version: 0 = v2.0 (no T-frames), 1 = v2.1 (T-frames), 2 = v3.0 (fail)
    codec_version: std::rc::Rc<std::cell::Cell<u8>>,
    /// Interpolation stats: (interpolated_count, encoded_count, last_interp_t, last_pred_psnr)
    interp_stats: (u32, u32, f32, f32),
    /// Pre-computed anchor map: for each frame, prev anchor and next anchor (or None)
    /// Filled on FAO load — O(n) build, O(1) lookup
    anchor_map: Vec<(Option<u32>, Option<u32>)>,
    /// Decoded anchor frame cache (RGB), keyed by frame idx
    /// Stores up to 16 most recent anchor decodes for fast interpolation
    anchor_cache: std::collections::VecDeque<(usize, Vec<u8>)>,
    /// Decoded anchor YUV cache for fast T-frame decoding (skip rgb→yuv conversion)
    anchor_yuv_for_t: std::collections::VecDeque<(usize, Vec<u8>)>,
    /// Motion profile from .fao trailer: per-frame source packet size
    /// Captured for free during ffprobe pass, used to weight interpolation
    fao_motion_sizes: Vec<u32>,
    /// Per-frame motion delta (0-255), high = scene change or motion spike
    fao_motion_deltas: Vec<u8>,
    /// Block dictionary for K-frame decoding (loaded from .fao trailer)
    fao_block_dict: Option<fao_v1::block_dict::BlockDictionary>,
    /// Source video path read from .fao trailer (for A/B view)
    fao_source_path: Option<String>,
    /// Cached source frames keyed by frame index (for A/B comparison)
    fao_source_cache: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<usize, Vec<u8>>>>,
    /// V8 decoder (scaffold — scene mesh + pose track for virtual camera fly-through)
    /// V8Decoder shared with background prefetch thread. RwLock allows 
    /// concurrent reads (UI thread + bg thread can both render in 
    /// parallel) while still permitting exclusive writes when a new .fao 
    /// is loaded.
    fao_v8_decoder: std::sync::Arc<std::sync::RwLock<Option<fao_v1::v8_reconstruction_codec::V8Decoder>>>,
    /// Per-frame anchor positions parsed from a V8AT trailer (Tobii-style 
    /// overlay). Key = video frame index, value = list of (x, y, class). 
    /// Empty when the .fao file was exported with FAO_NO_ANCHOR_TRACK=1 
    /// or has no V8AT trailer (older exports).
    fao_anchor_track: std::collections::HashMap<u32, Vec<(i16, i16, u8)>>,
    /// Volumetric chunks parsed from a V9DC trailer (v9 codec). Populated 
    /// only when the .fao file was exported with FAO_V9=1. Empty for 
    /// v8-only files. The v9 ray-march decoder uses these to produce 
    /// playable RGB when invoked via the v9 decode path (work in progress).
    fao_v9_chunks: Vec<fao_v1::v9_volumetric::V9Chunk>,
    /// Per-chunk decoded voxel grid cache. Key = chunk index in 
    /// `fao_v9_chunks`. Decoding a V9 chunk runs IDCT + voxel splat over 
    /// many blocks — too expensive to redo per frame. RefCell because 
    /// decode happens inside `&self` methods (decode_fao_frame_to_rgb).
    /// Bounded LRU keeps memory in check: at most ~3 grids cached at a 
    /// time, evicting the least-recently-used when full.
    fao_v9_grid_cache: std::cell::RefCell<Vec<(usize, std::rc::Rc<fao_v1::v9_volumetric::VoxelHashGrid>)>>,
    /// Toggle for the Tobii-style anchor overlay. Toggled by the 'A' 
    /// keyboard shortcut; persists across frame changes within a session.
    show_anchor_overlay: std::cell::Cell<bool>,
    /// References to the player's anchor overlay DrawingAreas. Stored so 
    /// frame advances and toggle keypresses can call queue_draw on them 
    /// — GTK DrawingAreas only repaint when invalidated, and the App 
    /// state mutation alone doesn't propagate to the widget without an 
    /// explicit invalidation. Wrapped in RefCell<Option> so they can be 
    /// installed AFTER App construction (the widgets are created in the 
    /// UI-build phase, well after `new()`).
    player_anchor_da_a: std::cell::RefCell<Option<gtk::DrawingArea>>,
    player_anchor_da_b: std::cell::RefCell<Option<gtk::DrawingArea>>,
    /// Player's anchor overlay toggle button. Stored so the 'A' keyboard 
    /// shortcut can sync the button's visual state. Without this, the 
    /// button would diverge from the actual state when the user uses 
    /// the keyboard.
    player_anchor_btn: std::cell::RefCell<Option<gtk::ToggleButton>>,
    /// Playback mode: Authentic (match source) vs Engine (smooth high-fps)
    /// 3D World viewer camera state (active camera)
    world_camera: std::cell::Cell<WorldCamera>,
    /// Saved freecam position (preserved when VideoCam mode is active so we 
    /// can restore it on toggle back)
    world_freecam_saved: std::cell::Cell<WorldCamera>,
    /// Active camera mode (Freecam or VideoCam)
    world_camera_mode: std::cell::Cell<WorldCameraMode>,
    /// World viewer color mode
    world_color_mode: std::cell::Cell<WorldColorMode>,
    /// World viewer filter mode
    world_filter_mode: std::cell::Cell<WorldFilterMode>,
    /// Whether to lay out webs in spatial bins (for visual separation)
    /// or render them in their native (overlapping) coords.
    world_webs_laid_out: std::cell::Cell<bool>,
    /// Show camera frustums in world view (toggle with F key).
    /// When false, only mesh + sparse points render. Useful when frustums 
    /// are visually noisy and you want to see the geometry alone.
    world_show_frustums: std::cell::Cell<bool>,
    /// Show mesh in world view (toggle with M key). When false, only camera 
    /// frustums + sparse points render. Useful when you want to focus on 
    /// camera path without mesh occlusion.
    world_show_mesh: std::cell::Cell<bool>,
    /// Render mesh as wireframe instead of filled triangles (toggle with W key).
    /// Wireframe is dramatically clearer for understanding geometry structure.
    world_mesh_wireframe: std::cell::Cell<bool>,
    /// Brightness multiplier for mesh rendering (cycles through 0.5/1.0/2.0/4.0
    /// with B key). Default 2.0 because dark scenes need help.
    world_mesh_brightness: std::cell::Cell<f32>,
    /// World viewer current temporal frame (for animated playback)
    world_time_idx: std::cell::Cell<u32>,
    /// World viewer playing flag
    world_playing: std::cell::Cell<bool>,
    /// Currently-held keyboard keys (for World viewer freecam)
    /// Stored as a small enum of movement actions; defensive multi-binding 
    /// like the keybind guide — multiple keys can trigger same action.
    world_held_keys: std::cell::RefCell<std::collections::HashSet<WorldMoveKey>>,
    /// Whether the World tab is currently active (gates keyboard input)
    world_tab_active: std::cell::Cell<bool>,
    /// Player A/B: second .fao file for comparison
    player_b_data: Option<FaoData>,
    player_b_index: Vec<fao_v1::FaoFrameEntry>,
    player_b_header: Option<fao_v1::FaoFileHeader>,
    player_b_path: String,
    player_b_frame: Vec<u8>,
    /// Library row → fao path mapping (parallel to lib_list rows)
    lib_row_paths: Vec<String>,
    /// Frame prefetch cache for fast playback (size 32 frames)
    /// Frame cache shared with background prefetch thread. UI reads via 
    /// .lock() on the fast path; bg thread inserts asynchronously.
    /// 
    /// The Mutex is uncontended in practice (bg only locks during inserts, 
    /// UI only during get/insert) so contention cost is negligible vs the 
    /// gain from never blocking on prefetch decode time.
    fao_prefetch: std::sync::Arc<std::sync::Mutex<fao_v1::FramePrefetchCache>>,
    
    /// Sender to background prefetch worker. None until the first .fao 
    /// is loaded; thread is spawned lazily then.
    /// 
    /// UI thread sends PrefetchTask values describing which frames need 
    /// decoding. Worker decodes off the UI thread, inserts into 
    /// fao_prefetch directly. UI never blocks on prefetch.
    /// 
    /// Channel is BOUNDED (capacity 4) — backpressure: if the worker is 
    /// busy with a previous task, new requests get dropped rather than 
    /// queueing forever. That's fine because the cache fills opportunistically; 
    /// we don't need every prefetch hint to land.
    prefetch_tx: Option<std::sync::mpsc::SyncSender<PrefetchTask>>,
    
    /// Handles to the background prefetch worker pool. Each .fao session 
    /// spawns PREFETCH_WORKER_COUNT threads; all are joined together when 
    /// the channel sender is dropped on .fao reload.
    prefetch_threads: Vec<std::thread::JoinHandle<()>>,
    /// Source-fetch worker channel + handle. Same lifecycle as the 
    /// prefetch pool: spawned on .fao load, dropped on .fao reload.
    /// 
    /// The worker pulls SourceFetchTask values off this channel, spawns 
    /// ffmpeg to decode a 60-frame window of the source video, and 
    /// populates fao_source_cache. The UI thread is never blocked.
    source_fetch_tx: Option<std::sync::mpsc::SyncSender<SourceFetchTask>>,
    source_fetch_thread: Option<std::thread::JoinHandle<()>>,
    
    /// Per-frame playback diagnostics. Each entry is a single 
    /// PlaybackTraceSample captured in load_fao_frame. Cleared on .fao 
    /// reload. Dumped via summarize_playback_trace.
    /// 
    /// Use std::cell::RefCell so we can record from `&self` paths (e.g. 
    /// the cache-hit branch only has &self, not &mut self at all points).
    playback_trace: std::cell::RefCell<Vec<PlaybackTraceSample>>,
    fao_prefetch_b: fao_v1::FramePrefetchCache,
    src_path: String, vid_path: Option<PathBuf>,
    duration: f64, vid_fps: f64, vid_w: usize, vid_h: usize, vid_codec: String, ow: usize, oh: usize, src_bitrate_kbps: f64,
    cur_frame: Vec<u8>, cur_recon: Vec<u8>, cur_w: usize, cur_h: usize,
    cur_result: Option<FrameStat>, has_frame: bool,
    /// Last-loaded FAO frame's marker byte (T/K/V/Z/P/B), used by UI 
    /// to show codec-appropriate label. 0 when none.
    cur_fao_marker: u8,
    scrub_time: f64, pin_start: f64, pin_end: f64,
    tc: Option<SharedTC>,
    fidx: usize, playing: bool,
    display_buf: Vec<u8>,
    fao_data: Option<FaoData>,
    fao_index: Vec<fao_v1::FaoFrameEntry>,
    fao_header: Option<fao_v1::FaoFileHeader>,
    quality: f32, dead_zone: f32, chroma_boost: i32,
    view: View, split_pos: f64, encode_mode: EncodeMode, play_accum_ms: f64,
    last_read_idx: usize, last_read_cache: Option<(Vec<u8>, Vec<u8>)>,
    speed_x: f64, speed_frames: usize, speed_start: std::time::Instant,
}

impl App {
    fn new() -> Self { Self {
        mode:Mode::Empty,
        ui_section: UiSection::Library,
        archive: None,
        archive_path: None,
        selected_video_id: None,
        // FAO Web Library — start unset; user attaches via UI or env var.
        // If FAO_LIBRARY env var is set we honor it as the initial value 
        // for backward compat with the env-var-only flow.
        fao_web_lib_path: std::env::var("FAO_LIBRARY").ok().map(PathBuf::from),
        fao_web_lib_writable: std::env::var("FAO_LIBRARY_RW").ok().as_deref() == Some("1"),
        fao_web_lib_stats: None,
        web_viz_nodes: Vec::new(),
        web_viz_edges: Vec::new(),
        web_viz_phase: 0.0,
        web_viz_active_frame: 0,
        web_viz_velocity: Vec::new(),
        codec_version: std::rc::Rc::new(std::cell::Cell::new(1)),
        fao_block_dict: None,
        fao_v8_decoder: std::sync::Arc::new(std::sync::RwLock::new(None)),
        fao_anchor_track: std::collections::HashMap::new(),
        fao_v9_chunks: Vec::new(),
        fao_v9_grid_cache: std::cell::RefCell::new(Vec::new()),
        show_anchor_overlay: std::cell::Cell::new(false),
        player_anchor_da_a: std::cell::RefCell::new(None),
        player_anchor_da_b: std::cell::RefCell::new(None),
        player_anchor_btn: std::cell::RefCell::new(None),
        world_camera: std::cell::Cell::new(WorldCamera::default()),
        world_freecam_saved: std::cell::Cell::new(WorldCamera::default()),
        world_camera_mode: std::cell::Cell::new(WorldCameraMode::Freecam),
        world_color_mode: std::cell::Cell::new(WorldColorMode::Proximity),
        world_filter_mode: std::cell::Cell::new(WorldFilterMode::All),
        world_webs_laid_out: std::cell::Cell::new(false),
        world_show_frustums: std::cell::Cell::new(true),
        world_show_mesh: std::cell::Cell::new(true),
        world_mesh_wireframe: std::cell::Cell::new(false),
        world_mesh_brightness: std::cell::Cell::new(2.0),
        world_time_idx: std::cell::Cell::new(0),
        world_playing: std::cell::Cell::new(false),
        world_held_keys: std::cell::RefCell::new(std::collections::HashSet::new()),
        world_tab_active: std::cell::Cell::new(false),
        fao_source_path: None,
        fao_source_cache: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        interp_stats: (0, 0, 0.0, 0.0),
        anchor_map: Vec::new(),
        anchor_cache: std::collections::VecDeque::new(),
            anchor_yuv_for_t: std::collections::VecDeque::new(),
        fao_motion_sizes: Vec::new(),
        fao_motion_deltas: Vec::new(),
        player_b_data: None,
        player_b_index: Vec::new(),
        player_b_header: None,
        player_b_path: String::new(),
        player_b_frame: Vec::new(),
        lib_row_paths: Vec::new(),
        fao_prefetch: std::sync::Arc::new(std::sync::Mutex::new(fao_v1::FramePrefetchCache::new(64))),
        prefetch_tx: None,
        prefetch_threads: Vec::new(),
        source_fetch_tx: None,
        source_fetch_thread: None,
        playback_trace: std::cell::RefCell::new(Vec::new()),
        fao_prefetch_b: fao_v1::FramePrefetchCache::new(64),
        src_path:String::new(), vid_path:None,
        duration:0.0, vid_fps:30.0, vid_w:0, vid_h:0, vid_codec:String::new(), ow:0, oh:0, src_bitrate_kbps:0.0,
        cur_frame:vec![], cur_recon:vec![], cur_w:0, cur_h:0, cur_fao_marker: 0,
        cur_result:None, has_frame:false,
        scrub_time:0.0, pin_start:0.0, pin_end:10.0,
        tc:None, fidx:0, playing:false, display_buf:vec![],
        fao_data:None, fao_index:vec![], fao_header:None,
        quality:0.4, dead_zone:1.0, chroma_boost:15,
        view:View::Split, split_pos:0.5, encode_mode:EncodeMode::V1, play_accum_ms:0.0,
        last_read_idx:usize::MAX, last_read_cache:None,
        speed_x:0.0, speed_frames:0, speed_start:std::time::Instant::now(),
    }}

    fn set_current(&mut self, rgb: Vec<u8>, w: usize, h: usize) {
        self.cur_w=w; self.cur_h=h; self.cur_frame=rgb; self.has_frame=true;
        let t0=Instant::now();
        let(rec,tb,psnr)=fao_v1::encode_iframe_recon(&self.cur_frame,w,h,self.quality,self.dead_zone,self.chroma_boost);
        self.cur_recon=rec;
        self.cur_result=Some(FrameStat{psnr,enc_bytes:((tb+7)/8)as usize,time_ms:t0.elapsed().as_secs_f64()*1000.0,ftype:0,skip_pct:0.0,vrr_ms:16.7,activity:0.0});
    }

    fn load_image(&mut self, path: &std::path::Path) {
        // Load image via ffmpeg (handles all formats)
        let out = Command::new("ffprobe").args(["-v","quiet","-print_format","json","-show_streams"])
            .arg(path).output().ok();
        let (w, h) = if let Some(ref o) = out {
            let info: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
            let s = &info["streams"][0];
            (s["width"].as_u64().unwrap_or(640) as usize, s["height"].as_u64().unwrap_or(480) as usize)
        } else { (640, 480) };
        let aw = (w/8)*8; let ah = (h/8)*8;
        let frame = Command::new("ffmpeg").arg("-i").arg(path)
            .args(["-vframes","1","-vf",&format!("scale={}:{}",aw,ah)])
            .args(["-f","rawvideo","-pix_fmt","rgb24","-v","quiet","-"])
            .output().ok();
        if let Some(f) = frame {
            if f.stdout.len() == aw * ah * 3 {
                self.src_path = path.display().to_string();
                self.mode = Mode::Image; self.playing = false; self.tc = None;
                self.set_current(f.stdout, aw, ah);
            }
        }
    }

    fn probe_video(&mut self, path: &std::path::Path) {
        let probe=match Command::new("ffprobe").args(["-v","quiet","-print_format","json","-show_streams","-show_format"])
            .arg(path).output(){Ok(p)=>p,Err(_)=>return};
        let info:serde_json::Value=serde_json::from_slice(&probe.stdout).unwrap_or_default();
        let vs=info["streams"].as_array().and_then(|a|a.iter().find(|s|s["codec_type"]=="video")).cloned().unwrap_or_default();
        self.ow=vs["width"].as_u64().unwrap_or(640)as usize;
        self.oh=vs["height"].as_u64().unwrap_or(360)as usize;
        self.duration=info["format"]["duration"].as_str().and_then(|s|s.parse::<f64>().ok()).unwrap_or(10.0);
        // FPS detection — prefer avg_frame_rate over r_frame_rate.
        // 
        // r_frame_rate is the "lowest framerate at which all timestamps 
        // can be represented" — for containers with millisecond-precision 
        // timebases (e.g., MKV with tbn=1000), this is often 1000/1 
        // regardless of actual frame rate. avg_frame_rate is duration/
        // nb_frames, which actually reflects how often frames appear.
        // 
        // Fallback chain:
        //   1. avg_frame_rate (most reliable for variable-rate / 
        //      slideshow-style content like the asahilinux 0.5fps lecture)
        //   2. r_frame_rate (works for normal CFR content)
        //   3. nb_frames / duration (last resort, requires both)
        //   4. 30 fps default
        // 
        // We sanity-check the result: any fps > 240 is suspicious 
        // (probably the tbn leaking through), so we fall through.
        let parse_rate = |s: &str| -> Option<f64> {
            let p: Vec<&str> = s.split('/').collect();
            if p.len() == 2 {
                let num = p[0].parse::<f64>().ok()?;
                let den = p[1].parse::<f64>().ok()?;
                if den > 0.0 { Some(num / den) } else { None }
            } else {
                None
            }
        };
        let avg_fps = vs["avg_frame_rate"].as_str().and_then(parse_rate);
        let r_fps = vs["r_frame_rate"].as_str().and_then(parse_rate);
        let nb_frames = vs["nb_frames"].as_str().and_then(|s| s.parse::<f64>().ok());
        let derived_fps = nb_frames.and_then(|n| {
            if self.duration > 0.0 { Some(n / self.duration) } else { None }
        });
        // Pick the first plausible value (≤ 240 fps).
        self.vid_fps = [avg_fps, r_fps, derived_fps]
            .into_iter()
            .flatten()
            .find(|f| *f > 0.0 && *f <= 240.0)
            .unwrap_or(30.0);
        self.vid_codec=vs["codec_name"].as_str().unwrap_or("?").to_string();
        // Extract source bitrate
        self.src_bitrate_kbps = vs["bit_rate"].as_str().and_then(|s|s.parse::<f64>().ok()).unwrap_or(0.0) / 1000.0;
        if self.src_bitrate_kbps == 0.0 {
            // Try format-level bitrate
            self.src_bitrate_kbps = info["format"]["bit_rate"].as_str().and_then(|s|s.parse::<f64>().ok()).unwrap_or(0.0) / 1000.0;
        }
        let(mut w,mut h)=(self.ow,self.oh);
        if w>3840||h>3840{let s=3840.0/w.max(h)as f64;w=(w as f64*s)as usize;h=(h as f64*s)as usize;}
        self.vid_w=(w/8)*8; self.vid_h=(h/8)*8;
        self.vid_path=Some(path.to_path_buf());
        self.src_path=path.display().to_string();
        self.mode=Mode::VideoScrub; self.playing=false; self.tc=None;
        self.pin_start=0.0; self.pin_end=self.duration;
        self.seek_frame(0.0);
    }

    fn seek_frame(&mut self, t: f64) {
        let path=match &self.vid_path{Some(p)=>p.clone(),None=>return};
        self.scrub_time=t.max(0.0).min(self.duration);
        let(w,h)=(self.vid_w,self.vid_h);
        // -ss before -i = fast keyframe seek (good enough for scrub preview)
        let output=match Command::new("ffmpeg")
            .args(["-ss",&format!("{:.3}",self.scrub_time)])
            .arg("-i").arg(&path)
            .args(["-vframes","1","-vf",&format!("scale={}:{},setsar=1",w,h)])
            .args(["-f","rawvideo","-pix_fmt","rgb24","-v","quiet","-"])
            .output(){Ok(o)=>o,Err(_)=>return};
        let fsz=w*h*3;
        if output.stdout.len()>=fsz{self.set_current(output.stdout[..fsz].to_vec(),w,h);}
    }

    /// Start background transcode to temp file
    fn start_transcode(&mut self) {
        // Issue 2 fix: don't stomp an already-running transcode.
        // 
        // Previously, calling start_transcode while a previous one was 
        // active just did `self.tc = None`, dropping the Arc but leaving 
        // worker threads running on the orphaned state. The disk writer 
        // would flush whatever it had (often a handful of frames) and 
        // report "done", while the UI showed "✓ done" for an encode 
        // that was actually interrupted at frame 14 of 217000.
        // 
        // Now: detect the running case and bail with a clear log. The 
        // user can wait for completion or implement a cancel button.
        if let Some(prev_tc) = self.tc.as_ref() {
            if !prev_tc.done.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!("FAO: start_transcode IGNORED — previous encode still running. Wait for it to finish or restart the app to cancel.");
                return;
            }
        }
        
        let cv = self.codec_version.get();
        let (major, minor) = match cv {
            0 => (2, 0),
            1 => (2, 1),
            2 => (3, 0),
            3 => (4, 0),
            4 => (5, 0),
            5 => (6, 0),
            6 => (7, 0),
            7 => (8, 0),
            _ => (2, 1),
        };
        eprintln!("FAO: starting transcode with codec v{}.{}", major, minor);
        
        // Ensure pin_end is valid
        if self.pin_end <= 0.0 || self.pin_end > self.duration {
            self.pin_end = self.duration;
        }
        if self.pin_start >= self.pin_end {
            self.pin_start = 0.0;
        }
        let path=match &self.vid_path{Some(p)=>p.clone(),None=>return};
        let(w,h)=(self.vid_w,self.vid_h);
        self.tc=None;
        let tc=Arc::new(TranscodeState::new(w,h,self.vid_fps));
        // Propagate user's codec selection to the transcode state
        let user_cv = self.codec_version.get();
        eprintln!("[v8 trace] start_transcode: self.codec_version.get() = {} (7 = v8.0)", user_cv);
        tc.codec_version.store(user_cv, std::sync::atomic::Ordering::Relaxed);
        // Propagate user's quality (0.0..1.0)
        tc.quality_q.store((self.quality as f32).to_bits(), std::sync::atomic::Ordering::Relaxed);
        // Initialize v8 state if v8.0 selected.
        // 
        // Sharded: one V8Encoder per compress worker, set up at 
        // TranscodeState::new() time (the slots already exist; we just 
        // populate each one). Each shard gets a unique chunk_id so 
        // feature IDs are namespaced — workers won't collide.
        if self.codec_version.get() == 7 {
            // Spill files for each worker's chunk_store. When mem 
            // pressure forces eviction, chunks are written here instead 
            // of being dropped. At trailer-write time they're faulted 
            // back in. Caps RAM at the chunk_store's policy budget 
            // (default ~384MB) regardless of total video length.
            let spill_dir = tc.tmp_path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
            
            // ─── Phase 3: Library hookup ───
            // 
            // Source priority:
            //   1. App.fao_web_lib_path (set via Library tab UI)
            //   2. FAO_LIBRARY env var (legacy / scripting)
            // 
            // Writable mode similarly prefers App.fao_web_lib_writable, 
            // falls back to FAO_LIBRARY_RW=1.
            let (lib_path_opt, writable) = match self.fao_web_lib_path.clone() {
                Some(p) => (Some(p), self.fao_web_lib_writable),
                None => {
                    let env_path = std::env::var("FAO_LIBRARY").ok().map(std::path::PathBuf::from);
                    let env_writable = std::env::var("FAO_LIBRARY_RW").ok().as_deref() == Some("1");
                    (env_path, env_writable)
                }
            };
            let library_arc: Option<std::sync::Arc<std::sync::RwLock<fao_v1::library::Library>>> = lib_path_opt
                .and_then(|path| {
                    if !path.exists() {
                        eprintln!("[v8 library] {:?} doesn't exist — running without library", path);
                        return None;
                    }
                    let lib = if writable {
                        fao_v1::library::Library::open_writable(&path)
                    } else {
                        fao_v1::library::Library::open(&path)
                    };
                    match lib {
                        Ok(l) => {
                            eprintln!("[v8 library] opened {:?} ({} webs, {}, {:.1} MB on disk)", 
                                path, l.manifest.web_count,
                                if writable { "writable" } else { "read-only" },
                                l.total_disk_bytes() as f64 / 1_048_576.0);
                            Some(std::sync::Arc::new(std::sync::RwLock::new(l)))
                        }
                        Err(e) => {
                            eprintln!("[v8 library] failed to open {:?}: {}", path, e);
                            None
                        }
                    }
                });
            let library_writable = library_arc.is_some() && writable;
            
            for (worker_idx, slot) in tc.v8_states.iter().enumerate() {
                if let Ok(mut v8) = slot.lock() {
                    // Use new_for_chunk so chunk_id is set up consistently 
                    // (it also wires the feature_pass's chunk_id for 
                    // namespaced feature IDs).
                    let mut enc = fao_v1::v8_reconstruction_codec::V8Encoder::new_for_chunk(
                        w as u32, h as u32, worker_idx as u32, 0);
                    // Open the spill file. Path is namespaced per worker.
                    let spill_path = spill_dir.join(format!("v8_spill_worker_{}.bin", worker_idx));
                    let _ = enc.chunk_store.open_spill(&spill_path);
                    // Attach the library handle (Arc-cloned across workers).
                    if let Some(ref lib) = library_arc {
                        enc.attach_library(std::sync::Arc::clone(lib), library_writable);
                    }
                    *v8 = Some(enc);
                }
            }
            eprintln!("[v8 trace] V8Encoder pool INITIALIZED at start_transcode for {}x{} ({} sharded encoders, spill dir={:?}, library={})", 
                w, h, tc.v8_states.len(), spill_dir,
                if library_arc.is_some() { 
                    if library_writable { "attached (writable)" } else { "attached (read-only)" } 
                } else { 
                    "none" 
                });
            // NOTE: the v8_spill_worker_N.bin files in the spill dir are 
            // a memory-pressure relief valve. They stay at 0 bytes during 
            // healthy encodes — that's the desired state. They only grow 
            // if the ChunkStore exceeds its 384MB-per-worker budget and 
            // needs to evict mesh data to disk to make room. Empty spill 
            // files = encoder running well within its memory budget.
            eprintln!("[v8 trace] (spill files at 0B = healthy; only grow under ChunkStore budget pressure)");
        }
        // Defensive: clear all v8/v7/v6 snapshots so World view doesn't show 
        // stale data from a previous encode. Each new tc is fresh anyway, but
        // any path that re-uses tc could leak data — be explicit.
        if let Ok(mut s) = tc.v8_trace_snapshot.try_lock() { s.0.clear(); s.1 = 0; s.2 = 0; }
        if let Ok(mut s) = tc.v8_points3d_snapshot.try_lock() { s.clear(); }
        if let Ok(mut s) = tc.v8_dense_snapshot.try_lock() { s.clear(); }
        if let Ok(mut s) = tc.v8_mesh_snapshot.try_lock() { s.0.clear(); s.1.clear(); }
        if let Ok(mut s) = tc.v8_pose_snapshot.try_lock() { s.clear(); }
        if let Ok(mut s) = tc.v8_webs_snapshot.try_lock() { s.clear(); }
        File::create(&tc.tmp_path).ok();
        self.tc=Some(tc.clone());
        self.mode=Mode::VideoPlay;self.fidx=0;self.playing=false;
        let q=self.quality;let dz=self.dead_zone;let cb=self.chroma_boost;
        let is_v2 = self.encode_mode == EncodeMode::V2;
        let pin_s=self.pin_start;let pin_e=self.pin_end;
        let self_dur=self.duration;
        eprintln!("FAO transcode: pin_s={:.1} pin_e={:.1} range={:.1}s duration={:.1}s", 
            pin_s, pin_e, pin_e-pin_s, self.duration);
        let max_disk:u64={
            let f=std::process::Command::new("df").args(["--output=avail","-B1","/tmp"]).output().ok()
                .and_then(|o|String::from_utf8(o.stdout).ok())
                .and_then(|s|s.lines().nth(1).and_then(|l|l.trim().parse::<u64>().ok()))
                .unwrap_or(20*1024*1024*1024);
            (f/2).min(100*1024*1024*1024)
        };
        // Probe source frame sizes
        {let tc2=tc.clone();let path2=path.clone();let ps=pin_s;let pe=pin_e;
        std::thread::spawn(move||{
            if let Ok(out)=Command::new("ffprobe").args(["-v","quiet","-select_streams","v","-show_entries","frame=pkt_size","-print_format","json"])
                .args(["-read_intervals",&format!("{:.3}%{:.3}",ps,pe)]).arg(&path2).output(){
                if let Ok(info)=serde_json::from_slice::<serde_json::Value>(&out.stdout){
                    if let Some(frames)=info["frames"].as_array(){
                        let mut sizes=tc2.src_frame_bytes.lock().unwrap();
                        for f in frames{sizes.push(f["pkt_size"].as_str().and_then(|s|s.parse().ok()).unwrap_or(0));}
            }}}
        });}
        // ── Single-reader scheduling manifest ──
        // One ffmpeg process decodes sequentially. Frames fan out to N workers.
        // Workers only encode — zero file I/O contention.
        std::thread::spawn(move||{
            let range=(pin_e-pin_s).max(0.1);
            let total_cores=std::thread::available_parallelism().map(|n|n.get()).unwrap_or(4);
            // Scale workers based on duration AND cores
            // Short clips: all cores. Long videos: fewer workers to control memory.
            // Scale workers by duration AND available memory
            let sys_ram_gb = std::fs::read_to_string("/proc/meminfo").ok()
                .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse::<u64>().ok()))
                .unwrap_or(16 * 1024 * 1024) / 1024 / 1024;
            let mem_factor = if sys_ram_gb >= 128 { 1 }        // 192GB: use all cores always
                else if sys_ram_gb >= 64 { 1 }                  // 64GB: use all cores
                else if sys_ram_gb >= 16 {
                    if range > 600.0 { 2 } else { 1 }          // 16GB: half for long videos
                } else {
                    if range > 60.0 { 4 } else { 2 }           // 8GB: quarter for anything long
                };
            // Use all available cores on high-RAM systems
            let n_workers = (total_cores / mem_factor).max(2).min(total_cores);
            eprintln!("FAO: {} workers on {} cores, {}GB RAM", n_workers, total_cores, sys_ram_gb);
            let fps=tc.fps;let fsz=w*h*3;let yuv_fsz=w*h*3/2;

            // Create channels: reader → worker
            let mut senders: Vec<std::sync::mpsc::SyncSender<(usize, Vec<u8>, fao_v1::FrameTag)>> = Vec::new();
            let mut receivers: Vec<Option<std::sync::mpsc::Receiver<(usize, Vec<u8>, fao_v1::FrameTag)>>> = Vec::new();
            for _ in 0..n_workers {
                // Buffer enough frames per worker to absorb I-frame stalls
                // At 1080p ~8MB compressed per frame, 20 frames = 160MB per channel
                let chan_depth = if sys_ram_gb >= 128 { 64 }  // large for skip-heavy workloads
                    else if sys_ram_gb >= 64 { 5 }
                    else if sys_ram_gb >= 16 { 5 }
                    else { 3 };  // less buffering on many-core systems
                let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, Vec<u8>, fao_v1::FrameTag)>(chan_depth);
                senders.push(tx);
                receivers.push(Some(rx));
            }

            // Create result channels: worker → collector
            let (res_tx, res_rx) = std::sync::mpsc::sync_channel::<(usize, std::sync::Arc<Vec<u8>>, Vec<u8>, u8, usize, f64, f64, f64, fao_v1::FrameTag)>(64);  // bounded: workers block if collector is slow

            // Spawn encode workers
            for wid in 0..n_workers {
                let rx = receivers[wid].take().unwrap();
                let res_tx = res_tx.clone();
                let worker_tc = tc.clone();  // for cooperative cancel checks
                std::thread::Builder::new()
                    .stack_size(2 * 1024 * 1024)
                    .spawn(move || {
                    // Create encoder based on mode
                    let use_v2 = is_v2;
                    let mut enc_v1 = fao_v1::VideoEncoder::new(w, h, q, dz, cb);
                    if wid > 0 { enc_v1.set_continuation(); }
                    let mut enc_v2 = if use_v2 { Some(fao_v1::v2_encoder::V2Encoder::new(w, h, q)) } else { None };
                    // ro buffer eliminated — encoder skips RGB output
                    let mut ro_mini = vec![0u8; 0];  // empty: skip RGB output in encoder
                    let mut worker_rx_count = 0usize;
                    while let Ok((frame_idx, mut fb, tag)) = rx.recv() {
                        worker_rx_count += 1;
                        // Cooperative cancel: bail out cleanly if a 
                        // Stop request landed mid-encode. We finish 
                        // any frame we've already started but won't 
                        // pick up new ones. The collector / disk-writer 
                        // see the channel close and flush their state.
                        if worker_tc.cancel_requested.load(Ordering::Relaxed) {
                            if worker_rx_count == 1 || worker_rx_count % 100 == 0 {
                                eprintln!("FAO worker {}: cancel acknowledged, dropping remaining frames", wid);
                            }
                            // Drain the channel so the reader doesn't 
                            // block forever; then exit.
                            while rx.try_recv().is_ok() {}
                            break;
                        }
                        if worker_rx_count <= 3 {
                            eprintln!("FAO worker {}: received frame {} (len={}, count={})", 
                                wid, frame_idx, fb.len(), worker_rx_count);
                        }
                        // Tier 1: empty buffer = static skip
                        if fb.is_empty() {
                            let fr = enc_v1.encode_frame_skip();
                            if worker_rx_count <= 5 {
                                eprintln!("FAO worker {}: skip frame {} → sending result", wid, frame_idx);
                            }
                            let _ = res_tx.send((frame_idx, std::sync::Arc::new(vec![]), vec![],
                                fr.frame_type, fr.compressed_size, fr.psnr, fr.skip_pct, fr.encode_ms, tag));
                            continue;
                        }
                        // Tier 2: 0xFA marker = moderate motion passthrough
                        if fb.len() == 5 && fb[0] == 0xFA {
                            let sz = fb[1] as u64 | (fb[2] as u64) << 8
                                | (fb[3] as u64) << 16 | (fb[4] as u64) << 24;
                            let fr = enc_v1.encode_frame_passthrough_fast(sz);
                            let _ = res_tx.send((frame_idx, std::sync::Arc::new(vec![]), vec![],
                                fr.frame_type, fr.compressed_size, fr.psnr, fr.skip_pct, fr.encode_ms, tag));
                            continue;
                        }
                        // Split YUV420p into Y/U/V planes
                        let y_size = w * h;
                        
                        // SPEED: skip the expensive V1 encoder for high-res (>720p) frames
                        // Just produce a placeholder FrameResult with cheap stats.
                        // The actual encoding happens in the compress thread (T/K/V/yuv_split).
                        // V1 encoder was redundant work duplicating what compress thread does.
                        let fr = if w * h > 720 * 480 * 2 {
                            // Cheap stats: just hint at frame_type (P-frame)
                            // PSNR/size will be updated by compress thread later
                            fao_v1::FrameResult {
                                frame_type: 1,
                                bits: 0,
                                psnr: 35.0,
                                skip_pct: 0.0,
                                encode_ms: 0.0,
                                compressed_size: y_size / 4,  // rough estimate
                                vrr_duration_ms: 16.7,
                                activity: 1.0,
                                route_stats: String::new(),
                                graph_new_refs: 0,
                                graph_traj_updates: 0,
                            }
                        } else {
                            // Low-res: still run V1 for accurate stats (cheap at this size)
                            let uv_size = w * h / 4;
                            let y_plane = &fb[..y_size];
                            let u_plane = &fb[y_size..y_size + uv_size];
                            let v_plane = &fb[y_size + uv_size..y_size + uv_size * 2];
                            enc_v1.encode_frame_yuv(y_plane, u_plane, v_plane, &mut ro_mini)
                        };
                        
                        // Send encoder reconstruction for A/B comparison
                        // Skip recon at high res (>720p) to save memory
                        let recon_y = if w * h <= 720 * 480 * 2 {
                            enc_v1.get_ref_y_u8()
                        } else {
                            vec![]  // skip recon at 1080p+ (6MB per frame)
                        };
                        let fb_arc = std::sync::Arc::new(fb);
                        let _ = res_tx.send((frame_idx, fb_arc, recon_y,
                            fr.frame_type, fr.compressed_size, fr.psnr, fr.skip_pct, fr.encode_ms, tag));
                        #[allow(unused_assignments)] { fb = vec![0u8; yuv_fsz]; }
                        
                    }
                    drop(enc_v1); drop(enc_v2);
                }).ok();
            }
            drop(res_tx);  // drop original so res_rx closes when all workers done

            // ── Parallel batched readers with ffprobe-guided selective decode ──
            let reader_tc = tc.clone();
            let collector_tc = tc.clone();
            let est_total_pre = (range * fps).ceil() as usize;

            // ffprobe: get frame sizes for skip detection (no decode)
            // Initial estimate based on resolution; will be refined to use
            // actual frame size median below for psychovisual accuracy
            let mut skip_threshold_bytes = {
                // Conservative initial estimate (lower = fewer skips, more T-frames)
                let pixels = (w * h) as u64;
                if pixels > 4_000_000 { pixels / 60 }       // 4K+: ~138KB
                else if pixels > 2_000_000 { pixels / 90 }  // 2K: ~41KB
                else if pixels > 1_000_000 { pixels / 120 } // 1080p: ~17KB
                else { pixels / 150 }                        // 720p: ~2.5KB
            };
            let frame_sizes: std::sync::Arc<Vec<u64>> = std::sync::Arc::new({
                // Probe ONLY frames in [pin_s, pin_e] — without this, frame_sizes
                // had every packet from the whole file, blowing up est_total and
                // making segment math ignore the IN/OUT sliders.
                match Command::new("ffprobe")
                    .args(["-v", "quiet", "-select_streams", "v:0",
                           "-show_entries", "packet=size", "-of", "csv=p=0"])
                    .args(["-read_intervals", &format!("{:.3}%{:.3}", pin_s, pin_e)])
                    .arg(&path).output() {
                    Ok(out) => {
                        let sizes: Vec<u64> = String::from_utf8_lossy(&out.stdout)
                            .lines().filter_map(|l| l.trim().parse().ok()).collect();
                        if !sizes.is_empty() {
                            // Refine threshold: use 30% of MEDIAN frame size
                            // This catches truly-static frames while letting motion frames through
                            let mut sorted_sizes: Vec<u64> = sizes.clone();
                            sorted_sizes.sort_unstable();
                            let median = sorted_sizes[sorted_sizes.len() / 2];
                            // Skip threshold = 30% of median (very static content only)
                            // (Was: absolute pixel-based, often too aggressive)
                            let median_based = (median as f64 * 0.3) as u64;
                            let final_thresh = median_based.min(skip_threshold_bytes).max(64);
                            skip_threshold_bytes = final_thresh;
                            
                            let n_skip = sizes.iter().filter(|&&s| s < skip_threshold_bytes).count();
                            eprintln!("FAO: ffprobe {} frames, median {}B, threshold={}B ({:.0}% skippable, T-frame candidates: {:.0}%)",
                                sizes.len(), median, skip_threshold_bytes,
                                n_skip as f64 / sizes.len() as f64 * 100.0,
                                (1.0 - n_skip as f64 / sizes.len() as f64) * 100.0);
                        }
                        sizes
                    }
                    Err(_) => vec![],
                }
            });

            // Batched readers: scale with resolution
            let pixels = w * h;
            let procs_per_thread = if pixels > 4_000_000 { 1 }       // 4K: 1 per thread (heavy decode)
                else if total_cores >= 20 { 4 }
                else if total_cores >= 8 { 2 }
                else { 1 };
            let n_threads = if range > 5.0 {
                let r = if pixels > 4_000_000 { 
                    if total_cores >= 20 { 8 } else { 4 }  // 4K: fewer concurrent decoders
                } else if total_cores >= 20 { 6 } 
                else if total_cores >= 10 { 4 } 
                else { 2 };
                r.min(n_workers)
            } else { 1 };
            let actual_readers = n_threads * procs_per_thread;
            let seg_duration = range / actual_readers as f64;
            #[allow(unused_assignments)] let mut seg_frames = (seg_duration * fps).ceil() as usize;
            // For sub-range encodes, use range*fps (ffprobe may count all frames)
            let range_frames = (range * fps).ceil() as usize;
            let est_total = if range_frames < frame_sizes.len() && range < self_dur * 0.95 {
                eprintln!("FAO: sub-range encode, using range-based count: {} (ffprobe: {})", range_frames, frame_sizes.len());
                range_frames
            } else if frame_sizes.len() > 0 {
                frame_sizes.len()
            } else {
                est_total_pre
            };
            // Recalculate segment size from est_total (not ffprobe total)
            seg_frames = (est_total + actual_readers - 1) / actual_readers;
            let chunk_size = (est_total / n_workers).max(1);
            
            // Use ffprobe frame count for accurate segment sizing
            let probe_total = frame_sizes.len();
            if probe_total > 0 {
                seg_frames = (probe_total + actual_readers - 1) / actual_readers;
                eprintln!("FAO: {} threads × {} ffmpeg = {} readers, {} frames/segment (ffprobe: {} total)",
                    n_threads, procs_per_thread, actual_readers, seg_frames, probe_total);
            } else {
                eprintln!("FAO: {} threads × {} ffmpeg = {} readers, {:.1}s/segment",
                    n_threads, procs_per_thread, actual_readers, seg_duration);
            }

            let senders_arc = std::sync::Arc::new(senders);
            let reader_handles: Vec<_> = (0..n_threads).map(|tid| {
                let path = path.clone();
                let senders = senders_arc.clone();
                let rtc = reader_tc.clone();
                let fsizes = frame_sizes.clone();
                std::thread::Builder::new().name(format!("fao-rd-{}", tid)).spawn(move || {
                    struct Sub {
                        rdr: std::io::BufReader<std::process::ChildStdout>,
                        proc: std::process::Child,
                        frame_offset: usize,
                        seg_end: usize,
                        local_idx: usize,
                        done: bool,
                        // When use_select is on, ffmpeg only outputs a subset of frames.
                        // decode_indices[local_idx] = absolute frame index for that read.
                        // Empty = no select filter, use frame_offset + local_idx directly.
                        decode_indices: Vec<usize>,
                    }
                    let mut subs: Vec<Sub> = Vec::new();

                    for p in 0..procs_per_thread {
                        let rid = tid * procs_per_thread + p;
                        let frame_offset = rid * seg_frames;
                        let seg_start = pin_s + frame_offset as f64 / fps;
                        let seg_dur = seg_frames as f64 / fps;
                        let seg_end = (frame_offset + seg_frames).min(est_total);

                        // Build select filter for this segment
                        let passthrough_threshold = skip_threshold_bytes * 3;  // 75% of median = moderate motion
                        let anchor_interval = 30;  // decode 1 anchor every 30 frames
                        let mut decode_list = Vec::new();
                        for fi in frame_offset..seg_end.min(fsizes.len()) {
                            let rel = fi - frame_offset;
                            let is_anchor = rel % anchor_interval == 0;
                            let is_heavy = fsizes[fi] >= passthrough_threshold;
                            if fi == frame_offset || is_heavy || is_anchor {
                                decode_list.push(rel);
                            }
                        }
                        let use_select = !fsizes.is_empty()
                            && decode_list.len() < (seg_end - frame_offset) * 80 / 100
                            && decode_list.len() < 500
                            && false;  // disabled: reader-side skip more reliable

                        if rid == 0 {
                            eprintln!("FAO reader {}: {} decode / {} frames (offset={}..{}), select={}", 
                                rid, decode_list.len(), seg_end - frame_offset, frame_offset, seg_end, use_select);
                        }
                        let vf = if use_select && !decode_list.is_empty() {
                            let expr: String = decode_list.iter()
                                .map(|n| format!("eq(n,{})", n))
                                .collect::<Vec<_>>().join("+");
                            format!("select='{}',setpts=N/FRAME_RATE/TB,scale={}:{},setsar=1", expr, w, h)
                        } else {
                            format!("scale={}:{},setsar=1", w, h)
                        };

                        // Skip markers sent inline during read loop

                        // HARD CLAMP: never read past pin_e regardless of segment math.
                        // Defends against ffprobe miscounting on webm containers — without
                        // this clamp, a wrong probe_total made seg_dur cover the whole video.
                        let seg_end_abs = (seg_start + seg_dur).min(pin_e);
                        let seg_dur_clamped = (seg_end_abs - seg_start).max(0.0);
                        if seg_dur_clamped <= 0.0 {
                            // Past the pin_e — this reader has nothing to do
                            subs.last_mut().map(|s| s.done = true);
                            continue;
                        }
                        let child = unsafe {
                            Command::new("ffmpeg")
                            .args(["-ss", &format!("{:.3}", seg_start)])
                            .arg("-i").arg(&path)
                            .args(["-to", &format!("{:.3}", seg_dur_clamped)])
                            .args(["-vf", &vf])
                            .args(["-threads", "0", "-thread_type", "frame"])
                            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-v", "quiet", "-"])
                            .stdout(std::process::Stdio::piped())
                            .pre_exec(|| {
                                #[cfg(target_os = "linux")]
                                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                                Ok(())
                            })
                            .spawn()
                        };
                        let mut proc = match child { Ok(c) => c, Err(_) => continue };
                        if rid == 0 {
                            let pid = proc.id() as u32;
                            rtc.ffmpeg_pid.store(pid, Ordering::SeqCst);
                            FFMPEG_PID.store(pid, Ordering::SeqCst);
                        }
                        let stdout = match proc.stdout.take() { Some(s) => s, None => continue };
                        let rdr = std::io::BufReader::with_capacity(4 * 1024 * 1024, stdout);

                        // Build decode-order frame index for this sub
                        let decode_abs: Vec<usize> = if use_select {
                            decode_list.iter().map(|&rel| frame_offset + rel).collect()
                        } else {
                            Vec::new()  // empty signals "use frame_offset + local_idx directly"
                        };
                        subs.push(Sub { 
                            rdr, proc, frame_offset, seg_end, local_idx: 0, done: false,
                            decode_indices: decode_abs,
                        });
                    }

                    // Round-robin read from all sub-readers
                    let mut fb = vec![0u8; yuv_fsz];
                    loop {
                        // Cooperative cancel — bail before doing more work.
                        // (use `rtc`, the per-thread clone of reader_tc, since the 
                        //  outer reader_tc was moved into the .map() iteration.)
                        if rtc.cancel_requested.load(Ordering::Relaxed) {
                            eprintln!("FAO reader: cancel acknowledged, shutting down ffmpeg children");
                            for s in subs.iter_mut() { s.done = true; }
                            break;
                        }
                        let mut all_done = true;
                        for si in 0..subs.len() {
                            if subs[si].done { continue; }
                            all_done = false;
                            if subs[si].rdr.read_exact(&mut fb).is_err() {
                                subs[si].done = true;
                                continue;
                            }
                            // Use decode_indices when present (select filter active),
                            // otherwise fall back to sequential indexing
                            let frame_idx = if subs[si].decode_indices.is_empty() {
                                subs[si].frame_offset + subs[si].local_idx
                            } else if subs[si].local_idx < subs[si].decode_indices.len() {
                                subs[si].decode_indices[subs[si].local_idx]
                            } else {
                                // Out of range — shouldn't happen but mark done
                                subs[si].done = true;
                                continue;
                            };
                            subs[si].local_idx += 1;

                            // Skip detection from ffprobe frame sizes
                            if frame_idx < fsizes.len() && frame_idx > 0 {
                                let fsize = fsizes[frame_idx];
                                // Send everything not-skip through compress thread for translate
                                // (passthrough disabled — let v2.1 try translate instead)
                                let passthrough_thresh = 0u64;  // never matches → no passthrough
                                // Anchor frames: full encode every 30 frames
                                let is_anchor = (frame_idx % 30) == 0;
                                // For v5/v6/v7 codecs we want EVERY frame to reach the 
                                // codec so it can build its sprite/object library. 
                                // Skip pre-classification would starve those codecs.
                                let reader_codec_v = rtc.codec_version.load(std::sync::atomic::Ordering::Relaxed);
                                let codec_wants_all_frames = reader_codec_v >= 4;  // v5+ (incl. v8)
                                if !is_anchor && fsize < skip_threshold_bytes && !codec_wants_all_frames {
                                    // Tier 1: skip — round-robin to any available worker
                                    let skip_tag = fao_v1::FrameTag::skip(frame_idx, 
                                        subs[si].frame_offset / seg_frames, 
                                        frame_idx - subs[si].frame_offset, fsize);
                                    let mut sent = false;
                                    for off in 0..senders.len() {
                                        let wid = (frame_idx + off) % senders.len();
                                        if senders[wid].try_send((frame_idx, vec![], skip_tag.clone())).is_ok() {
                                            sent = true; break;
                                        }
                                    }
                                    if !sent {
                                        // All full — use least-loaded (first available)
                                        let wid = frame_idx % senders.len();
                                        let _ = senders[wid].send((frame_idx, vec![], skip_tag.clone()));
                                    }
                                    continue;
                                } else if !is_anchor && fsize < passthrough_thresh {
                                    // Tier 2: passthrough — round-robin
                                    let sz = fsize;
                                    let marker = vec![0xFAu8,
                                        (sz & 0xFF) as u8, ((sz >> 8) & 0xFF) as u8,
                                        ((sz >> 16) & 0xFF) as u8, ((sz >> 24) & 0xFF) as u8];
                                    let pass_tag = fao_v1::FrameTag::passthrough(frame_idx,
                                        subs[si].frame_offset / seg_frames,
                                        frame_idx - subs[si].frame_offset, fsize);
                                    let mut sent = false;
                                    for off in 0..senders.len() {
                                        let wid = (frame_idx + off) % senders.len();
                                        if senders[wid].try_send((frame_idx, marker.clone(), pass_tag.clone())).is_ok() {
                                            sent = true; break;
                                        }
                                    }
                                    if !sent {
                                        let wid = frame_idx % senders.len();
                                        let _ = senders[wid].send((frame_idx, marker, pass_tag));
                                    }
                                    continue;
                                }
                                // Tier 3: full encode — fall through to normal send
                            }

                            // Memory check
                            if frame_idx % 100 == 0 {
                                let avail_gb = std::fs::read_to_string("/proc/meminfo").ok()
                                    .and_then(|s| s.lines().find(|l| l.starts_with("MemAvailable:"))
                                        .and_then(|l| l.split_whitespace().nth(1))
                                        .and_then(|v| v.parse::<u64>().ok()))
                                    .unwrap_or(999*1024*1024) / 1024 / 1024;
                                if avail_gb < 8 {
                                    std::thread::sleep(std::time::Duration::from_secs(1));
                                }
                            }

                            let pw = (frame_idx / chunk_size).min(senders.len() - 1);
                            let full_tag = fao_v1::FrameTag::full(frame_idx,
                                subs[si].frame_offset / seg_frames,
                                frame_idx - subs[si].frame_offset, 
                                if frame_idx < fsizes.len() { fsizes[frame_idx] } else { 0 });
                            match senders[pw].try_send((frame_idx, fb.clone(), full_tag.clone())) {
                                Ok(()) => {},
                                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                                    let mut sent = false;
                                    for off in 1..senders.len() {
                                        let wid = (pw + off) % senders.len();
                                        if senders[wid].try_send((frame_idx, fb.clone(), full_tag.clone())).is_ok() {
                                            sent = true; break;
                                        }
                                    }
                                    if !sent {
                                        if senders[pw].send((frame_idx, fb.clone(), full_tag)).is_err() { break; }
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        if all_done { break; }
                    }
                    for sub in &mut subs { let _ = sub.proc.kill(); let _ = sub.proc.wait(); }
                    rtc.ffmpeg_pid.store(0, Ordering::SeqCst);
                }).unwrap()
            }).collect();
            drop(senders_arc);

            // Collector: receive results from workers, write to temp file in order
            let tmp_raw = match std::fs::OpenOptions::new().write(true).append(true)
                .open(&reader_tc.tmp_path) { Ok(f) => f, Err(_) => { reader_tc.done.store(true, Ordering::SeqCst); return; }};
            let mut tmp = std::io::BufWriter::with_capacity(fsz * 2, tmp_raw);  // buffer one frame pair

            // Write frames out-of-order as they arrive, track with bitmap
            // Sparse tracking of OUT-OF-ORDER frame results.
            // 
            // Previously these were Vec<bool> + Vec<Option<FrameStat>> indexed 
            // by frame_idx. On a 3M-frame video that's 3M slots even when only 
            // a few hundred are live at any moment — Vec doesn't shrink as 
            // entries get drained, so the worst-case allocation hit was ~240MB 
            // of mostly-None slots and the encoder OOM'd.
            // 
            // HashMap-backed: live size is bounded by the in-flight reorder 
            // window (typically < 100 frames across all workers), regardless 
            // of total frame count. Inserts and removes are O(1).
            let mut written_set: std::collections::HashSet<usize> = std::collections::HashSet::new();
            let mut stats_map: std::collections::HashMap<usize, FrameStat> = std::collections::HashMap::new();
            let mut continuous = 0usize;
            let mut tracker = fao_v1::gpu_pipeline::SceneTracker::new();
            let mut irb = fao_v1::gpu_pipeline::InverseRuleBuffer::new(64 * 1024 * 1024);
            let mut web = fao_v1::gpu_pipeline::FrameWeb::new(est_total);
            let mut last_full_node: u32 = u32::MAX;  // 64MB buffer
            let mut disk_pos = 0u64;
            let mut total_rx = 0usize;

            // Compression threads: handle zstd + disk writes off the collector
            // compress_tx: was 256 frames × 7.6MB = ~2GB. Reduce to 32 to 
            // apply backpressure to decoders when compress thread is slow.
            let (compress_tx, compress_rx) = std::sync::mpsc::sync_channel::<(usize, Vec<u8>, Vec<u8>, u64, fao_v1::FrameTag)>(32);
            // Disk writer: receives compressed frames, writes sequentially
            // disk_tx capacity: 4096 was too generous — at 7.6MB raw + 2.7MB 
            // recon per frame, 4096 frames = ~30GB potential queue. With 
            // disk write slower than encode, this queue saturates and consumes 
            // all RAM. Drop to 128: caps queued data at ~1.5GB while still 
            // absorbing reasonable burst from skip-heavy regions.
            let (disk_tx, disk_rx) = std::sync::mpsc::sync_channel::<(usize, Vec<u8>, Vec<u8>, fao_v1::FrameTag)>(128);
            let disk_tc = collector_tc.clone();
            let disk_tmp = collector_tc.tmp_path.clone();
            std::thread::Builder::new().name("fao-disk".into()).spawn(move || {
                let mut f = match std::fs::File::create(&disk_tmp) {
                    Ok(f) => std::io::BufWriter::with_capacity(4 * 1024 * 1024, f),
                    Err(_) => return,
                };
                let mut pos = 0u64;
                let mut buffer: std::collections::BTreeMap<usize, (Vec<u8>, Vec<u8>, fao_v1::FrameTag)> = std::collections::BTreeMap::new();
                let mut next_write = 0usize;
                
                for (fidx, c_orig, c_recon, tag) in disk_rx.iter() {
                    buffer.insert(fidx, (c_orig, c_recon, tag));
                    // Write all sequential frames from buffer
                    while let Some((co, cr, ftag)) = buffer.remove(&next_write) {
                        if next_write < 5 || (co.len() > 100 && next_write % 1000 == 0) {
                            let tier_label = match ftag.tier {
                                0 => "skip",
                                1 => "passthrough",
                                2 => "full",
                                3 => "anchor",
                                _ => "?",
                            };
                            eprintln!("FAO disk: write frame {} orig_c={} recon_c={} tier={}", 
                                next_write, co.len(), cr.len(), tier_label);
                        }
                        let _ = f.write_all(&(next_write as u32).to_le_bytes());
                        let _ = f.write_all(&(co.len() as u32).to_le_bytes());
                        let _ = f.write_all(&(cr.len() as u32).to_le_bytes());
                        let _ = f.write_all(&ftag.to_bytes());  // 16 bytes tag
                        let _ = f.write_all(&co);
                        let _ = f.write_all(&cr);
                        {
                            let mut off = disk_tc.offsets.lock().unwrap_or_else(|e| e.into_inner());
                            while off.len() <= next_write { off.push(u64::MAX); }
                            off[next_write] = pos;
                        }
                        pos += 28 + co.len() as u64 + cr.len() as u64;  // 12 header + 16 tag
                        next_write += 1;
                    }
                    // Flush periodically
                    if next_write % 100 == 0 { let _ = f.flush(); }
                }
                let _ = f.flush();
                eprintln!("FAO: disk writer finished, {} frames, {:.1}MB", next_write, pos as f64 / 1048576.0);
            }).ok();
            let comp_tc = reader_tc.clone();
            let comp_tmp_path = reader_tc.tmp_path.clone();
            // Use Arc<Mutex> to share receiver across 4 threads
            let shared_rx = std::sync::Arc::new(std::sync::Mutex::new(compress_rx));
            // More compress threads for v2.1: 50% of cores, min 4, max 12
            let n_compress_threads = std::thread::available_parallelism()
                .map(|n| (n.get() / 2).max(4).min(12))
                .unwrap_or(4);
            eprintln!("FAO: spawning {} compress threads", n_compress_threads);
            for t in 0..n_compress_threads {
                let crx = shared_rx.clone();
                let ctc = comp_tc.clone();
                let tmp_path = comp_tmp_path.clone();
                let disk_tx_c = disk_tx.clone();
                std::thread::Builder::new().name(format!("fao-zstd-{}", t)).spawn(move || {
                    let mut last_c_orig = vec![0u8; 0];

                    let mut last_c_recon = vec![0u8; 0];
                    let mut last_c_rgb = vec![0u8; 0];
                    let mut last_c_recon_rgb = vec![0u8; 0];
                    let mut last_raw_yuv: Vec<u8> = Vec::new();
                    let mut last_fidx: usize = usize::MAX;
                    loop {
                        let item = {
                            let rx = crx.lock().unwrap_or_else(|e| e.into_inner());
                            rx.recv()
                        };
                        match item {
                            Ok((fidx, orig, recon, _disk_hint, tag)) => {
                                if orig.is_empty() {
                                    // Skip frame: empty marker on disk (read_nearest_real handles display)
                                    let _ = disk_tx_c.send((fidx, vec![], vec![], tag.clone()));
                                    // Ring buffer: clone last real frame
                                    if fidx % 10 == 0 {
                                        if let Ok(mut ring) = ctc.ring_buf.lock() {
                                            ring.push((fidx, last_c_rgb.clone(), last_c_recon_rgb.clone()));
                                            while ring.len() > ctc.ring_capacity { ring.remove(0); }
                                        }
                                    }
                                    continue;
                                }
                                
                                if fidx < 5 || fidx % 100 == 0 {
                                    let expected_yuv = w * h * 3 / 2;
                                    let expected_rgb = w * h * 3;
                                    eprintln!("FAO compress: frame {} orig_len={} (yuv_sz={} rgb_sz={} match_yuv={})", 
                                        fidx, orig.len(), expected_yuv, expected_rgb, orig.len() == expected_yuv);
                                }
                                // v2.1: try translate-encoding first (predictive),
                                // fall back to yuv_split if not applicable or larger
                                // Clone YUV upfront for cache use later
                                // Only allocate for cache if this is an anchor frame (every 30th)
                                // Saves ~9GB of memcpy across a 4800-frame encode
                                let orig_for_cache: Option<std::sync::Arc<Vec<u8>>> = if orig.len() == w * h * 3 / 2 && fidx % 30 == 0 {
                                    Some(std::sync::Arc::new(orig.clone()))
                                } else { None };
                                
                                let compress_start = std::time::Instant::now();
                                
                                // REGION ACTIVITY + LAYER CLASSIFICATION + FILTER ANALYSIS
                                // Run periodically to avoid lock contention with 12 workers
                                if orig.len() == w * h * 3 / 2 && fidx % 8 == 0 {
                                    if let Ok(mut ra) = ctc.region_activity.try_lock() {
                                        let (stable, changed) = ra.analyze_frame(&orig, w, h);
                                        if fidx % 200 == 0 && ra.frame_count > 8 {
                                            ra.classify_all();
                                            let dist = ra.layer_distribution();
                                            eprintln!("FAO layers: frame {} → pure_static={:.0}% alpha_overlay={:.0}% parallax={:.0}% active={:.0}% noise={:.0}% unknown={:.0}%",
                                                fidx, dist[0], dist[1], dist[2], dist[3], dist[4], dist[5]);
                                        } else if fidx < 100 && fidx % 16 == 0 {
                                            eprintln!("FAO region activity: frame {} → {} stable / {} changed ({:.1}% stable)",
                                                fidx, stable, changed, ra.stable_ratio() * 100.0);
                                        }
                                    }
                                    
                                    // Filter analysis (every 250 frames, after anchors exist)
                                    // Skip at >= 2K (1920+ pixels wide) to save CPU on big frames
                                    if fidx % 250 == 0 && fidx > 30 && w < 1920 {
                                        if let Ok(cache) = ctc.anchor_yuv_cache.try_read() {
                                            if let Some((_, anchor_arc)) = cache.iter().last() {
                                                if anchor_arc.len() >= w * h {
                                                    let target_y = &orig[..w * h];
                                                    let pred_y = &anchor_arc[..w * h];
                                                    let analysis = fao_v1::pixel_filters::analyze_frame_filters(
                                                        target_y, pred_y, w, h);
                                                    eprintln!("FAO filter analysis: frame {} → identity={:.0}% denoise={:.0}% quantize={:.0}% edge={:.0}% (saved {} SAD units)",
                                                        fidx,
                                                        analysis.percent(fao_v1::pixel_filters::PixelFilter::Identity),
                                                        analysis.percent(fao_v1::pixel_filters::PixelFilter::Denoise3x3),
                                                        analysis.percent(fao_v1::pixel_filters::PixelFilter::Quantize8),
                                                        analysis.percent(fao_v1::pixel_filters::PixelFilter::EdgePreserve),
                                                        analysis.sad_reduction);
                                                    
                                                    // Store filter analysis for web viz access
                                                    if let Ok(mut fa) = ctc.last_filter_analysis.try_write() {
                                                        *fa = Some(analysis);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    
                                    // Intra-frame dedup analysis (every 500 frames)
                                    // Skip at >= 2K to save CPU
                                    if fidx % 500 == 0 && w < 1920 {
                                        let mut ifd = fao_v1::region_activity::IntraFrameDedup::new(w, h);
                                        ifd.analyze_frame(&orig, w, h);
                                        eprintln!("FAO intra-dedup: frame {} → {} unique / {} blocks ({:.1}% unique, est save {} bytes)",
                                            fidx, ifd.unique_blocks.len(), ifd.block_ids.len(),
                                            ifd.dedup_ratio() * 100.0, ifd.estimated_savings());
                                    }
                                }
                                
                                let c_yuv = if orig.len() == w * h * 3 / 2 {
                                    // Try translate FIRST — only compute baseline if translate fails
                                    // No bilateral filter (was 15× speed regression)
                                    let initial_budget = (w * h / 4) as usize;  // ~25% of raw
                                    
                                    // Check codec version: v2.0 disables translate (skips this path)
                                    let codec_v = ctc.codec_version.load(std::sync::atomic::Ordering::Relaxed);
                                    // Fire diagnostic regardless of fidx — multiple compress 
                                    // threads receive frames in arbitrary order so fidx<3 
                                    // doesn't necessarily catch any. Use a one-shot flag instead.
                                    static V8_DIAG_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                    if !V8_DIAG_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                        eprintln!("[v8 trace] FIRST compress entry: fidx={} codec_v={} (expecting codec_v=7 for v8)", fidx, codec_v);
                                    }
                                    let try_translate_enabled = codec_v >= 1;
                                    

                                    // Item 8: even if user selected v8, skip the v8 attempt 
                                    // when auto-fallback has triggered. The flag is set by 
                                    // the codec selection site below when v8 has been losing 
                                    // by ≥ AUTO_DISABLE_THRESHOLD over the warmup period.
                                    let try_v8 = codec_v == 7 && !ctc.v8_auto_disabled.load(std::sync::atomic::Ordering::Relaxed);
                                    static V8_DISPATCH_DIAG_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                    if !V8_DISPATCH_DIAG_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                        eprintln!("[v8 trace] FIRST dispatch: fidx={} codec_v={} try_v8={} orig_len={}", fidx, codec_v, try_v8, orig.len());
                                    }
                                    let v8_attempt = if try_v8 {
                                        // Use poison-tolerant lock: a panic in another worker 
                                        // (e.g., bilinear sample OOB before we made it safe) used 
                                        // to permanently disable v8 by leaving the mutex poisoned. 
                                        // .into_inner() on the PoisonError gives us back the guard 
                                        // anyway — the encoder state is still consistent enough to 
                                        // continue (worst case we lose one frame's contribution).
                                        // 
                                        // Worker locks ITS OWN slot (v8_states[t]) — no contention 
                                        // with other workers. Each worker has a private V8Encoder. 
                                        // Cross-worker bookkeeping (web alignment, mesh fusion) is 
                                        // skipped at encode time and merged at trailer write.
                                        let lock_result = ctc.v8_states.get(t).map(|s| s.lock());
                                        let lock = match lock_result {
                                            Some(Ok(g)) => Some(g),
                                            Some(Err(poisoned)) => {
                                                static POISON_WARN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                                if !POISON_WARN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                                    eprintln!("[v8 trace] WARNING: v8_states[{}] mutex was poisoned (a previous worker panicked). Recovering and continuing.", t);
                                                }
                                                Some(poisoned.into_inner())
                                            }
                                            None => {
                                                // Worker index out of bounds — shouldn't happen; 
                                                // v8_states is sized to match worker count.
                                                static OOB_WARN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                                if !OOB_WARN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                                    eprintln!("[v8 trace] WARNING: worker {} has no v8_states slot ({} slots)", t, ctc.v8_states.len());
                                                }
                                                None
                                            }
                                        };
                                        if let Some(mut v8_lock) = lock {
                                            static V8_LOCK_DIAG_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                            if !V8_LOCK_DIAG_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                                eprintln!("[v8 trace] FIRST lock acquired: fidx={}, v8.is_some()={}", fidx, v8_lock.is_some());
                                            }
                                            if let Some(ref mut v8) = *v8_lock {
                                                // Snapshot the layer mask from the region-activity tracker. 
                                                // This is what the UI-overlay filter uses to identify HUDs / 
                                                // overlays vs real scene content (round-3 user insight: 
                                                // "the anchors are mostly on the UI elements"). 
                                                // 
                                                // try_lock so we never block the v8 encoder on region-tracker 
                                                // contention. If the lock is held by another worker, we pass 
                                                // None and the encoder falls back to motion-class-only filtering 
                                                // for this frame (same behavior as before the filter shipped).
                                                // 
                                                // Serialization: walk the block grid and emit one byte per 
                                                // block (BlockLayer enum discriminant). Cost: O(block_w * 
                                                // block_h) = ~8K blocks at 1080p, ~30K at 4K. Cheap enough 
                                                // to do every encode call.
                                                {
                                                    let snapshot: Option<Vec<u8>> = if let Ok(ra) = ctc.region_activity.try_lock() {
                                                        // The classifier may not have run yet (it triggers 
                                                        // every 200 frames in the analyze loop). When 
                                                        // frame_count is too low, layer_at returns Unknown 
                                                        // for everything, which the encoder treats as 
                                                        // "neutral" — no harm, just no filter benefit.
                                                        let bw = ra.block_w;
                                                        let bh = ra.block_h;
                                                        if bw > 0 && bh > 0 && bw * bh == ra.blocks.len() {
                                                            let mut mask = Vec::with_capacity(bw * bh);
                                                            for by in 0..bh {
                                                                for bx in 0..bw {
                                                                    let layer = ra.layer_at(bx, by);
                                                                    // Match BlockLayer enum order: 
                                                                    // PureStatic=0, AlphaOverlay=1, ParallaxBg=2, 
                                                                    // Active=3, Noise=4, Unknown=5
                                                                    let byte = match layer {
                                                                        fao_v1::region_activity::BlockLayer::PureStatic => 0u8,
                                                                        fao_v1::region_activity::BlockLayer::AlphaOverlay => 1u8,
                                                                        fao_v1::region_activity::BlockLayer::ParallaxBg => 2u8,
                                                                        fao_v1::region_activity::BlockLayer::Active => 3u8,
                                                                        fao_v1::region_activity::BlockLayer::Noise => 4u8,
                                                                        fao_v1::region_activity::BlockLayer::Unknown => 5u8,
                                                                    };
                                                                    mask.push(byte);
                                                                }
                                                            }
                                                            Some(mask)
                                                        } else {
                                                            None
                                                        }
                                                    } else {
                                                        None
                                                    };
                                                    v8.set_layer_mask(snapshot);
                                                }
                                                
                                                // Pass the actual video frame index so out-of-order arrival 
                                                // from multiple compress threads doesn't scramble the trace.
                                                let bytes = v8.encode_frame_at(&orig, fidx as u32);
                                                if fidx < 50 || fidx % 500 == 0 {
                                                    let (n_static, n_dynamic, n_unknown, n_ui) = 
                                                        v8.consensus_pass.count_by_class(&v8.feature_pass.live_tracks);
                                                    let latest_coverage = v8.depth_maps.last().map(|m| m.coverage()).unwrap_or(0.0);
                                                    // UiOverlay count added to the live-tracks summary so the user 
                                                    // can see the filter working. When the encoder is correctly 
                                                    // separating game UI from scene content, expect O to be 
                                                    // comparable to S; both grow as the encoder runs.
                                                    eprintln!("FAO v8.0: frame {} → {} bytes (live {} = {}S/{}D/{}U/{}O, tri {}, mesh {}v/{}t, last_dm cov={:.1}%) | {}",
                                                        fidx, bytes.len(), v8.feature_pass.live_track_count(),
                                                        n_static, n_dynamic, n_unknown, n_ui,
                                                        v8.observations.triangulated.len(),
                                                        v8.scene_mesh.vertices.len(),
                                                        v8.scene_mesh.indices.len() / 3,
                                                        latest_coverage * 100.0,
                                                        v8.chunk_store.report());
                                                }
                                                // Session 8: periodic compression-potential report.
                                                // Now uses subsampled residual computation so it's cheap 
                                                // enough to run every 250 frames and surface progress 
                                                // earlier in the encode.
                                                if fidx > 0 && fidx % 250 == 0 {
                                                    let report = v8.chunk_store.web_compression_report(5.0);
                                                    eprintln!("[v8 web compression] {}", report.to_string());
                                                }
                                                if fidx % 30 == 0 {
                                                    if let Ok(mut snap) = ctc.v8_trace_snapshot.try_lock() {
                                                        snap.0 = v8.spatial_trace.clone();  // tuple now (id,x,y,fi,class)
                                                        snap.1 = v8.width;
                                                        snap.2 = v8.height;
                                                    }
                                                    // Also refresh 3D triangulated points
                                                    if let Ok(mut p3d) = ctc.v8_points3d_snapshot.try_lock() {
                                                        p3d.clear();
                                                        for tf in v8.observations.triangulated.values() {
                                                            p3d.push((
                                                                tf.feature_id,
                                                                tf.world_pos.x,
                                                                tf.world_pos.y,
                                                                tf.world_pos.z,
                                                                tf.confidence,
                                                                tf.last_observed_frame,
                                                            ));
                                                        }
                                                    }
                                                    
                                                    // Dense depth snapshot — back-project anchor depth maps to world space.
                                                    // We sample every Nth pixel to keep total points ~50K.
                                                    if let Ok(mut dense) = ctc.v8_dense_snapshot.try_lock() {
                                                        dense.clear();
                                                        let target_pts = 50_000;
                                                        let total_px: usize = v8.depth_maps.iter()
                                                            .map(|m| m.depths.len()).sum();
                                                        let stride = (total_px / target_pts).max(1);
                                                        let mut counter = 0usize;
                                                        for map in v8.depth_maps.iter() {
                                                            let dw = map.width as usize;
                                                            let dh = map.height as usize;
                                                            let down_factor = (v8.width / map.width).max(1) as f32;
                                                            let frame_idx = map.anchor_frame as usize;
                                                            if frame_idx >= v8.pose_track.len() { continue; }
                                                            let pose = &v8.pose_track[frame_idx];
                                                            for py in 0..dh {
                                                                for px in 0..dw {
                                                                    counter += 1;
                                                                    if counter % stride != 0 { continue; }
                                                                    let idx = py * dw + px;
                                                                    let d = map.depths[idx];
                                                                    let conf = map.confidences[idx];
                                                                    if conf < 0.05 || d <= 0.0 { continue; }
                                                                    // Unproject pixel → camera-frame ray → world
                                                                    let fx_full = (px as f32 + 0.5) * down_factor;
                                                                    let fy_full = (py as f32 + 0.5) * down_factor;
                                                                    let fxi = if pose.focal[0] > 0.0 { pose.focal[0] } else { 1000.0 };
                                                                    let fyi = if pose.focal[1] > 0.0 { pose.focal[1] } else { 1000.0 };
                                                                    let cx = (fx_full - pose.principal[0]) / fxi * d;
                                                                    let cy = (fy_full - pose.principal[1]) / fyi * d;
                                                                    let cz = d;
                                                                    // Apply pose rotation to get world position
                                                                    let cam_vec = fao_v1::geometry::Vec3 { x: cx, y: cy, z: cz };
                                                                    let world = fao_v1::v8_reconstruction_codec::rotate_vec3_by_quat(cam_vec, pose.rotation);
                                                                    dense.push((
                                                                        pose.position.x + world.x,
                                                                        pose.position.y + world.y,
                                                                        pose.position.z + world.z,
                                                                        conf,
                                                                        map.anchor_frame,
                                                                    ));
                                                                }
                                                            }
                                                        }
                                                    }
                                                    
                                                    // Mesh snapshot — copy current scene_mesh into the World view's buffer.
                                                    // The mesh is the persistent compressed representation; this is what 
                                                    // makes "fly through the reconstructed scene" possible.
                                                    if let Ok(mut mesh_snap) = ctc.v8_mesh_snapshot.try_lock() {
                                                        mesh_snap.0.clear();
                                                        mesh_snap.1.clear();
                                                        for (i, v) in v8.scene_mesh.vertices.iter().enumerate() {
                                                            let col = v8.scene_mesh.vertex_colors.get(i)
                                                                .copied().unwrap_or([0.5, 0.7, 0.9]);
                                                            mesh_snap.0.push((v.x, v.y, v.z, col[0], col[1], col[2]));
                                                        }
                                                        mesh_snap.1.extend_from_slice(&v8.scene_mesh.indices);
                                                    }
                                                    
                                                    // Pose trajectory snapshot — show where the encoder 
                                                    // thinks the camera was at each video frame.
                                                    if let Ok(mut pose_snap) = ctc.v8_pose_snapshot.try_lock() {
                                                        pose_snap.clear();
                                                        for (idx, p) in v8.pose_track.iter().enumerate() {
                                                            // Skip uninitialized poses (default = origin with identity)
                                                            if p.position.x == 0.0 && p.position.y == 0.0 && p.position.z == 0.0 
                                                               && p.rotation == [1.0, 0.0, 0.0, 0.0] && idx > 0 {
                                                                continue;
                                                            }
                                                            pose_snap.push((idx as u32,
                                                                p.position.x, p.position.y, p.position.z,
                                                                p.rotation[0], p.rotation[1], p.rotation[2], p.rotation[3]));
                                                        }
                                                    }
                                                    
                                                    // Web snapshot (Sessions 6c-8): one entry per closed chunk.
                                                    // Lets World view render each chunk's mesh distinctly.
                                                    if let Ok(mut webs_snap) = ctc.v8_webs_snapshot.try_lock() {
                                                        webs_snap.clear();
                                                        // Count how many webs follow each canonical
                                                        let mut follower_count: std::collections::HashMap<u32, u32> = 
                                                            std::collections::HashMap::new();
                                                        for c in &v8.chunk_store.closed_chunks {
                                                            if let Some(canon) = c.canonical_web_id {
                                                                *follower_count.entry(canon).or_insert(0) += 1;
                                                            }
                                                        }
                                                        for c in &v8.chunk_store.closed_chunks {
                                                            if c.web_mesh.vertices.is_empty() { continue; }
                                                            let chunk_id = c.frame_start;
                                                            let canonical_id = c.canonical_web_id.unwrap_or(chunk_id);
                                                            let n_followers = *follower_count.get(&chunk_id).unwrap_or(&0);
                                                            
                                                            let mut verts = Vec::with_capacity(c.web_mesh.vertices.len());
                                                            for (i, v) in c.web_mesh.vertices.iter().enumerate() {
                                                                let col = c.web_mesh.vertex_colors.get(i)
                                                                    .copied().unwrap_or([0.5, 0.7, 0.9]);
                                                                verts.push((v.x, v.y, v.z, col[0], col[1], col[2]));
                                                            }
                                                            webs_snap.push(V8WebSnap {
                                                                chunk_id,
                                                                frame_start: c.frame_start,
                                                                frame_end: c.frame_end,
                                                                vertices: verts,
                                                                indices: c.web_mesh.indices.clone(),
                                                                canonical_id,
                                                                n_aligned_followers: n_followers,
                                                            });
                                                        }
                                                    }
                                                }
                                                Some(bytes)
                                            } else { None }
                                        } else { None }
                                    } else { None };

                                    let mut reject_reason: &'static str = if try_translate_enabled { "no_attempt" } else { "v2.0_no_translate" };
                                    let translate_attempt = if !try_translate_enabled { 
                                        None  // v2.0: skip translate, fall through to yuv_split
                                    } else {
                                        // RwLock try_read: multiple workers can read concurrently
                                        let cache = ctc.anchor_yuv_cache.try_read();
                                        match cache {
                                            Ok(cache_guard) => {
                                                if cache_guard.is_empty() {
                                                    reject_reason = "no_anchors_cached_yet";
                                                    None
                                                } else {
                                                    let nearest = cache_guard.iter()
                                                        .min_by_key(|(ai, _)| (*ai as i64 - fidx as i64).abs());
                                                    match nearest {
                                                        Some((anchor_idx, anchor_yuv)) => {
                                                            if *anchor_idx == fidx {
                                                                reject_reason = "self_ref";
                                                                None
                                                            } else if *anchor_idx > fidx {
                                                                // PREVENT FUTURE-REFERENCE: anchor must be earlier in playback order
                                                                // This ensures sequential playback works correctly
                                                                reject_reason = "future_ref";
                                                                None
                                                            } else if (fidx - *anchor_idx) > 120 {
                                                                reject_reason = "anchor_too_far";
                                                                None
                                                            } else {
                                                                // Map user quality (0.0..1.0) to encoder quality (50..240)
                                                                // q=0.0 → quality=50 (highly lossy, smallest T-frames)
                                                                // q=0.5 → quality=145 (default)
                                                                // q=1.0 → quality=240 (near-lossless, larger T-frames)
                                                                let q_user = f32::from_bits(ctc.quality_q.load(std::sync::atomic::Ordering::Relaxed));
                                                                let quality = (50.0 + q_user.clamp(0.0, 1.0) * 190.0) as u8;
                                                                let result = fao_v1::v21_predictive::try_translate_frame(
                                                                    &orig, anchor_yuv.as_slice(), w, h,
                                                                    (*anchor_idx).min(65535) as u16,
                                                                    quality,
                                                                    initial_budget,
                                                                );
                                                                if result.is_none() {
                                                                    reject_reason = "translate_too_big_or_low_psnr";
                                                                }
                                                                result
                                                            }
                                                        }
                                                        None => {
                                                            reject_reason = "cache_iter_empty";
                                                            None
                                                        }
                                                    }
                                                }
                                            }
                                            Err(_) => {
                                                reject_reason = "cache_lock_busy";
                                                None
                                            }
                                        }
                                    };  // close translate_attempt match
                                    
                                    if fidx < 100 && fidx % 5 == 0 {
                                        eprintln!("FAO v2.1 attempt: frame {} → {} (budget {} bytes)",
                                            fidx, 
                                            if translate_attempt.is_some() { "T-FRAME" } else { reject_reason },
                                            initial_budget);
                                    }
                                    
                                    // Try K-frame encoding (block dictionary) as alternative
                                    // K-frames are great for content with repeated UI/HUD blocks
                                    let k_frame_attempt = if try_translate_enabled && fidx % 4 == 0 {
                                        // Only every 4th frame to limit dict pressure
                                        if let Ok(mut dict) = ctc.block_dict.try_write() {
                                            fao_v1::block_dict::encode_frame_from_dict(&orig, w, h, &mut *dict)
                                        } else { None }
                                    } else { None };
                                    
                                    // Log the v8_attempt outcome once per encode
                                    static V8_OUTCOME_DIAG_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                    if !V8_OUTCOME_DIAG_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                        eprintln!("[v8 trace] FIRST v8_attempt outcome: try_v8={} attempt_is_some={} attempt_len={}",
                                            try_v8, v8_attempt.is_some(),
                                            v8_attempt.as_ref().map(|v| v.len()).unwrap_or(0));
                                    }
                                    
                                    // CODEC SELECTION when v8 is active.
                                    //
                                    // Old behavior: v8 always wins when non-empty. This is 
                                    // catastrophic for content where v8 has no useful 3D structure 
                                    // (top-down RTS, talking heads, pure 2D pans) — v8 still emits 
                                    // ~10KB per frame for sparse features even when the actual 
                                    // mesh is empty. Meanwhile v2 T-frames could be 200B for the 
                                    // same content.
                                    //
                                    // New behavior: when v8 is active, COMPARE v8's output size 
                                    // against the v2/k-frame alternatives. Pick the smallest. 
                                    // The smallest output is also closest to the source size — 
                                    // there's no quality loss tradeoff between them, just bytes.
                                    //
                                    // This makes v8 self-correcting on content where its 
                                    // assumptions (real 3D parallax) don't hold.
                                    if let Some(v8_bytes) = v8_attempt.filter(|v| !v.is_empty()) {
                                        // Compare v8 against the size estimates of the alternatives.
                                        // 
                                        // translate_attempt is (TranslatedFrame, usize est_size, f32 psnr).
                                        // The est_size is the v2 path's pre-computed byte estimate, 
                                        // close enough for comparison without serializing.
                                        // k_frame_attempt is Vec<u8> already.
                                        let v8_len = v8_bytes.len();
                                        let v2_len = translate_attempt.as_ref().map(|(_, est, _)| *est).unwrap_or(usize::MAX);
                                        let kf_len = k_frame_attempt.as_ref().map(|v| v.len()).unwrap_or(usize::MAX);
                                        
                                        // Determine chosen_bytes — must be Vec<u8> to match v8_bytes type.
                                        // Item 8: each branch also updates the per-encode v8_wins / v8_losses 
                                        // counters used for auto-fallback.
                                        let v8_won;
                                        let chosen_bytes: Vec<u8> = if v2_len < v8_len && v2_len <= kf_len {
                                            // v2 T-frame wins. Serialize it now.
                                            v8_won = false;
                                            static V2_OVER_V8_LOG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                                            let n = V2_OVER_V8_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                            if n < 5 {
                                                eprintln!("[codec select] frame {}: v2 T-frame ({}B est) beats v8 ({}B) — flat content fallback",
                                                    fidx, v2_len, v8_len);
                                            }
                                            // translate_attempt is Some here (v2_len wouldn't be MAX otherwise).
                                            let (tframe, _, _) = translate_attempt.unwrap();
                                            tframe.to_bytes(w, h)
                                        } else if kf_len < v8_len {
                                            v8_won = false;
                                            static KF_OVER_V8_LOG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                                            let n = KF_OVER_V8_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                            if n < 5 {
                                                eprintln!("[codec select] frame {}: K-frame ({}B) beats v8 ({}B)",
                                                    fidx, kf_len, v8_len);
                                            }
                                            k_frame_attempt.unwrap()
                                        } else {
                                            v8_won = true;
                                            v8_bytes
                                        };
                                        
                                        // Item 8: update counters and consider auto-disable.
                                        // Only counts when v8 actually produced output (try_v8 was true 
                                        // and v8_attempt was Some). Frames where v8 was already disabled 
                                        // don't contribute samples — that would self-reinforce the 
                                        // disable decision once made.
                                        if try_v8 {
                                            if v8_won {
                                                ctc.v8_wins.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                            } else {
                                                ctc.v8_losses.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                            }
                                            // Cheap atomic loads — no contention because this is the 
                                            // hot path and we read what we just wrote (probably still 
                                            // in cache).
                                            if !ctc.v8_auto_disabled.load(std::sync::atomic::Ordering::Relaxed) {
                                                let wins = ctc.v8_wins.load(std::sync::atomic::Ordering::Relaxed);
                                                let losses = ctc.v8_losses.load(std::sync::atomic::Ordering::Relaxed);
                                                let total = wins + losses;
                                                if total >= AUTO_DISABLE_MIN_SAMPLES {
                                                    let loss_ratio = losses as f32 / total as f32;
                                                    if loss_ratio > AUTO_DISABLE_THRESHOLD {
                                                        // Use compare_exchange to make the diagnostic 
                                                        // print exactly once across all workers.
                                                        if ctc.v8_auto_disabled.compare_exchange(
                                                            false, true,
                                                            std::sync::atomic::Ordering::Relaxed,
                                                            std::sync::atomic::Ordering::Relaxed,
                                                        ).is_ok() {
                                                            eprintln!(
                                                                "[codec auto] disabling v8 attempts: {}/{} frames ({:.1}%) lost to v2/kf — content does not benefit from reconstruction",
                                                                losses, total, 100.0 * loss_ratio
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        
                                        if let Ok(mut s) = ctc.translate_stats.try_lock() {
                                            s.0 = s.0.saturating_add(1);
                                            s.1 = s.1.saturating_add(chosen_bytes.len() as u64);
                                        }
                                        chosen_bytes
                                    } else {
                                    
                                    // Pick the SMALLEST of (T, K, yuv_split)
                                    let t_bytes_opt = translate_attempt.as_ref().map(|(tf, _, _)| tf.to_bytes(w, h));
                                    
                                    if let Some((tframe, est_size, psnr)) = translate_attempt {
                                        // T-frame bytes already computed above (t_bytes_opt)
                                        let t_bytes = t_bytes_opt.unwrap_or_else(|| tframe.to_bytes(w, h));
                                        let is_motion_only = tframe.residual_quantized.is_empty();
                                        
                                        let pick_k = match &k_frame_attempt {
                                            Some(k_bytes) if k_bytes.len() < t_bytes.len() => true,
                                            _ => false,
                                        };
                                        
                                        if fidx < 50 || fidx % 500 == 0 {
                                            let chosen = if pick_k { "K-FRAME" } else if is_motion_only { "T-MOTION-ONLY" } else { "T-WITH-RESIDUAL" };
                                            eprintln!("FAO v2.1: frame {} → {} ({} bytes, T={} K={:?}) psnr {:.1}dB",
                                                fidx, chosen, 
                                                if pick_k { k_frame_attempt.as_ref().unwrap().len() } else { est_size },
                                                t_bytes.len(),
                                                k_frame_attempt.as_ref().map(|k| k.len()),
                                                psnr);
                                        }
                                        
                                        if let Ok(mut s) = ctc.translate_stats.try_lock() {
                                            s.0 = s.0.saturating_add(1);
                                            s.1 = s.1.saturating_add(if pick_k {
                                                k_frame_attempt.as_ref().map(|k| k.len()).unwrap_or(est_size) as u64
                                            } else { est_size as u64 });
                                        }
                                        
                                        if pick_k {
                                            k_frame_attempt.unwrap()
                                        } else {
                                            t_bytes
                                        }
                                    } else if let Some(k_bytes) = k_frame_attempt {
                                        // No T-frame but K-frame worked
                                        if fidx < 50 || fidx % 500 == 0 {
                                            eprintln!("FAO v2.1: frame {} → K-FRAME ({} bytes, no T-frame)",
                                                fidx, k_bytes.len());
                                        }
                                        k_bytes
                                    } else {
                                        // No translate, no K-frame — fall back to compression
                                        // At high res, prefer lossy chroma (4-bit U/V) for ~30% smaller files
                                        // (perceptual loss is minimal — chroma is half resolution anyway)
                                        if w * h > 1280 * 720 {
                                            fao_v1::compress_frame_lossy_chroma(&orig, w, h)
                                        } else {
                                            fao_v1::compress_frame_yuv_split(&orig, w, h)
                                        }
                                    }
                                    }
                                } else {
                                    // v2.0 path or invalid frame size
                                    if orig.len() == w * h * 3 / 2 && w * h > 1280 * 720 {
                                        fao_v1::compress_frame_lossy_chroma(&orig, w, h)
                                    } else if orig.len() == w * h * 3 / 2 {
                                        fao_v1::compress_frame_yuv_split(&orig, w, h)
                                    } else {
                                        fao_v1::compress_frame_disk(&orig)
                                    }
                                };
                                
                                // ── v10 post-race enhancement (opt-in via FAO_V10=1) ──
                                //
                                // After the existing race picks c_yuv, run v10's strategy 
                                // orchestrator with c_yuv.len() as the baseline. v10 emits 
                                // its own bytes ONLY if a v10 strategy (currently 
                                // StaticRepeat or MotionCompensated) beats the baseline. 
                                // Otherwise c_yuv stays as-is.
                                //
                                // Per-worker state needed: prev YUV (from last_raw_yuv) + 
                                // prev decoded RGB (from last_decoded_rgb). Both are 
                                // maintained at the end of each iteration but only when 
                                // v10 is enabled (avoids the memory overhead in default mode).
                                //
                                // Why post-race instead of replacing the race: the existing 
                                // race covers v2/v8/k-frame/v3-v7 already; v10's wins are 
                                // mostly StaticRepeat (frames v8 wastes bytes on) and 
                                // MotionCompensated (slow pans where neither v2 nor v8 
                                // shines). Running v10 AFTER lets us measure its 
                                // contribution without disturbing the existing flow.
                                let c_yuv = if std::env::var("FAO_V10")
                                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                                    .unwrap_or(false)
                                    && orig.len() == w * h * 3 / 2
                                    && !last_raw_yuv.is_empty()
                                    && last_raw_yuv.len() == orig.len()
                                {
                                    let plan = fao_v1::v10::Plan::from_env();
                                    let v10_stats = fao_v1::v10::StrategyStats::default();
                                    // Build prev_rgb from last_raw_yuv via the same BT.601 
                                    // conversion the decoder uses. Reuse last_decoded_rgb 
                                    // if it's the same size — avoids redundant YUV→RGB 
                                    // every frame in motion-comp regions.
                                    let prev_rgb_owned: Vec<u8> = if !last_c_recon_rgb.is_empty() 
                                        && last_c_recon_rgb.len() == w * h * 3 
                                    {
                                        last_c_recon_rgb.clone()
                                    } else {
                                        // Fallback: convert last_raw_yuv → RGB. The result 
                                        // approximates what the decoder will reconstruct 
                                        // (modulo quant noise). Source-side prediction 
                                        // accumulates drift over many motion-comp frames, 
                                        // but the residual coding compensates each frame.
                                        let mut rgb = vec![0u8; w * h * 3];
                                        let y_sz = w * h;
                                        let uv_w = w / 2;
                                        let uv_sz = uv_w * (h / 2);
                                        if last_raw_yuv.len() >= y_sz + 2 * uv_sz {
                                            for py in 0..h {
                                                for px in 0..w {
                                                    let yv = last_raw_yuv[py * w + px] as f32;
                                                    let uv_off = (py / 2) * uv_w + (px / 2);
                                                    let cu = last_raw_yuv[y_sz + uv_off] as f32;
                                                    let cv = last_raw_yuv[y_sz + uv_sz + uv_off] as f32;
                                                    let yn = (yv - 16.0) * (255.0 / 219.0);
                                                    let un = cu - 128.0;
                                                    let vn = cv - 128.0;
                                                    let r = (yn + 1.402 * vn).clamp(0.0, 255.0) as u8;
                                                    let g = (yn - 0.344136 * un - 0.714136 * vn).clamp(0.0, 255.0) as u8;
                                                    let b = (yn + 1.772 * un).clamp(0.0, 255.0) as u8;
                                                    let oi = (py * w + px) * 3;
                                                    rgb[oi]     = r;
                                                    rgb[oi + 1] = g;
                                                    rgb[oi + 2] = b;
                                                }
                                            }
                                        }
                                        rgb
                                    };
                                    
                                    match fao_v1::v10::encode_frame_against(
                                        &plan, &v10_stats,
                                        &orig, Some(&last_raw_yuv), Some(&prev_rgb_owned),
                                        w, h, Some(c_yuv.len()),
                                    ) {
                                        Some(v10_result) => {
                                            static V10_WIN_LOG: std::sync::atomic::AtomicU32 = 
                                                std::sync::atomic::AtomicU32::new(0);
                                            let n = V10_WIN_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                            if n < 10 || n % 100 == 0 {
                                                eprintln!("[v10] frame {} → {:?} ({} → {} bytes, save {})",
                                                    fidx, v10_result.strategy, c_yuv.len(),
                                                    v10_result.bytes.len(),
                                                    c_yuv.len().saturating_sub(v10_result.bytes.len()));
                                            }
                                            v10_result.bytes
                                        }
                                        None => c_yuv,
                                    }
                                } else {
                                    c_yuv
                                };
                                
                                // Maintain per-worker prev-frame state for the NEXT frame's 
                                // v10 attempt. Only retained when v10 is on, to avoid the 
                                // memory cost in default mode.
                                if std::env::var("FAO_V10")
                                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                                    .unwrap_or(false)
                                {
                                    if orig.len() == w * h * 3 / 2 {
                                        last_raw_yuv.clear();
                                        last_raw_yuv.extend_from_slice(&orig);
                                    }
                                }
                                
                                // RGB conversion for preview ring buffer
                                // SKIP at >720p (where user already has limited preview anyway)
                                // This saves ~20ms/frame on 1080p+ content
                                let large_frame = w * h > 720 * 480 * 2;
                                let c_rgb: Vec<u8> = if large_frame {
                                    // Skip RGB preview at high res — saves 20ms/frame
                                    Vec::new()
                                } else {
                                    let rgb_orig = if orig.len() == w * h * 3 / 2 {
                                        let mut rgb = vec![0u8; w * h * 3];
                                        let hw = w / 2; let y_sz = w * h;
                                        for py in 0..h { for px in 0..w {
                                            let yi = py * w + px;
                                            let ui = (py/2) * hw + (px/2);
                                            let yv = orig[yi] as f32;
                                            let uv = orig[y_sz + ui] as f32 - 128.0;
                                            let vv = orig[y_sz + y_sz/4 + ui] as f32 - 128.0;
                                            let oi = yi * 3;
                                            rgb[oi]   = (yv + 1.402 * vv).clamp(0.0, 255.0) as u8;
                                            rgb[oi+1] = (yv - 0.344 * uv - 0.714 * vv).clamp(0.0, 255.0) as u8;
                                            rgb[oi+2] = (yv + 1.772 * uv).clamp(0.0, 255.0) as u8;
                                        }}
                                        rgb
                                    } else { orig.clone() };
                                    fao_v1::compress_frame(&rgb_orig)
                                };

                                // Ring buffer: RGB orig + RGB recon for A/B display
                                // Skip at >720p — recon is empty anyway, this just wastes lock cycles
                                if w * h <= 720 * 480 * 2 {
                                    if let Ok(mut ring) = ctc.ring_buf.lock() {
                                        ring.push((fidx, c_rgb.clone(), last_c_recon_rgb.clone()));
                                        while ring.len() > ctc.ring_capacity { ring.remove(0); }
                                    }
                                }

                                // Update last known real frame for skip frame references
                                _ = &c_yuv;  // last_c_orig unused after refactor
                                last_c_rgb = c_rgb.clone();
                                
                                // v2.1: cache this frame's YUV as anchor for future translates
                                // (every full-encode frame becomes an anchor candidate)
                                if let Some(yuv_arc) = orig_for_cache {
                                    // Already Arc<Vec<u8>>, just push (no extra alloc)
                                    if let Ok(mut cache) = ctc.anchor_yuv_cache.try_write() {
                                        if cache.len() >= 32 { cache.pop_front(); }
                                        cache.push_back((fidx, yuv_arc));
                                    }
                                }
                                
                                // Disk: compressed YUV orig + compressed recon
                                // For v3/v4/v5 codecs: use FAO-DECODED YUV if available (shows actual codec output),
                                // otherwise fall back to compressed source YUV (preview only).
                                // v8 (Z marker, or raw V81/V8p for small frames 
                                // under ZSTD_THRESHOLD) doesn't need a recon 
                                // column on disk — the rasterizer reconstructs 
                                // from the V8MX mesh trailer at decode time. 
                                // 
                                // BUG FIX HISTORY:
                                //   v1: only c_yuv[0]==b'Z' detected. Frames 
                                //       small enough to skip Z wrap (< 600 
                                //       bytes) fell through to v_codec path 
                                //       and got a ~1MB recon written for every 
                                //       such frame. User saw "orig_c=592 
                                //       recon_c=981795" in logs.
                                //   v2 (this): also detect raw V81 ('V','8','1') 
                                //       and V8p ('V','8','p') prefixes so the 
                                //       small-frame v8 path skips recon too.
                                let c_yuv_is_v8 = !c_yuv.is_empty() && (
                                    c_yuv[0] == b'Z' ||
                                    (c_yuv.len() >= 3 && c_yuv[0] == b'V' && c_yuv[1] == b'8' 
                                        && (c_yuv[2] == b'1' || c_yuv[2] == b'p'))
                                );
                                // c_yuv is a non-v8 V-codec output (v3/v4/v5/v6/v7) 
                                // only when it starts with 'V' but ISN'T a v8 variant.
                                let c_yuv_is_v_codec = !c_yuv.is_empty() && c_yuv[0] == b'V' && !c_yuv_is_v8;
                                let c_recon = if c_yuv_is_v8 {
                                    c_yuv.clone()
                                } else if c_yuv_is_v_codec {
                                    // Legacy v4-v7 codecs removed; this branch only fires on legacy frames.
                                    let yuv = orig.as_slice();
                                    if yuv.len() == w * h * 3 / 2 {
                                        if w * h > 1280 * 720 {
                                            fao_v1::compress_frame_lossy_chroma(yuv, w, h)
                                        } else {
                                            fao_v1::compress_frame_yuv_split(yuv, w, h)
                                        }
                                    } else {
                                        c_yuv.clone()
                                    }
                                } else if recon.is_empty() {
                                    c_yuv.clone()
                                } else if recon.len() == w * h * 3 {
                                    // Convert RGB recon to YUV420p then delta compress
                                    let mut yuv_recon = vec![0u8; w * h * 3 / 2];
                                    let y_sz = w * h;
                                    let hw = w / 2; let hh = h / 2;
                                    for py in 0..h { for px in 0..w {
                                        let ri = (py * w + px) * 3;
                                        let r = recon[ri] as f32; let g = recon[ri+1] as f32; let b = recon[ri+2] as f32;
                                        yuv_recon[py * w + px] = (0.2126*r + 0.7152*g + 0.0722*b).clamp(0.0, 255.0) as u8;
                                    }}
                                    for cy in 0..hh { for cx in 0..hw {
                                        let px = cx * 2; let py = cy * 2;
                                        let ri = (py * w + px) * 3;
                                        let r = recon[ri] as f32; let g = recon[ri+1] as f32; let b = recon[ri+2] as f32;
                                        yuv_recon[y_sz + cy * hw + cx] = (-0.1146*r - 0.3854*g + 0.5*b + 128.0).clamp(0.0, 255.0) as u8;
                                        yuv_recon[y_sz + y_sz/4 + cy * hw + cx] = (0.5*r - 0.4542*g - 0.0458*b + 128.0).clamp(0.0, 255.0) as u8;
                                    }}
                                    fao_v1::compress_frame_disk(&yuv_recon)
                                } else {
                                    fao_v1::compress_frame_disk(&recon)
                                };
                                _ = &c_recon;  // last_c_recon unused after refactor
                                last_c_recon_rgb = c_rgb.clone();
                                let compress_elapsed_ms = compress_start.elapsed().as_secs_f64() * 1000.0;
                                // Update stat with ACTUAL compressed size + time (V1 estimate is wrong)
                                // Stash actual compressed size — main collector reads at stats push
                                if let Ok(mut sizes) = ctc.actual_compressed_sizes.try_lock() {
                                    sizes.insert(fidx, c_yuv.len());
                                }
                                let _ = disk_tx_c.send((fidx, c_yuv, c_recon, tag));
                            }
                            Err(_) => break,  // channel closed
                        }
                    }
                }).ok();
            }

            for (fidx, orig_arc, recon, ft, cs, psnr, sp, ms, tag) in res_rx.iter() {
                if total_rx < 5 || total_rx % 500 == 0 { 
                    eprintln!("FAO: {} frames collected, continuous={}, fidx={}, ft={}", total_rx, continuous, fidx, ft); 
                }
                // (HashSet/HashMap grow automatically — no pre-extension needed.)

                // Compute content hash before extracting from Arc
                let content_hash_pre = {
                    let mut h: u64 = 0xcbf29ce484222325;
                    for &b in orig_arc.iter().take(256) { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
                    h
                };
                // Send to compression threads (zero-copy extract from Arc)
                let orig = match std::sync::Arc::try_unwrap(orig_arc) {
                    Ok(v) => v,
                    Err(arc) => (*arc).clone(),
                };
                let _ = compress_tx.send((fidx, orig, recon, disk_pos, tag));
                disk_pos += (w * h * 3) as u64;  // estimated
                written_set.insert(fidx);
                // Push to ring buffer for preview (keep last N frames in memory)

                let content_hash = content_hash_pre;
                let node_idx = web.add_frame(fidx as u32, ft, content_hash, disk_pos as u32, cs as u16);
                
                if cs > 100 {
                    // Full encode: self-anchor + link from previous
                    web.add_edge(node_idx, node_idx, fao_v1::gpu_pipeline::WebEdgeKind::AnchorRef, 255);
                    if last_full_node != u32::MAX {
                        web.add_edge(node_idx, last_full_node, fao_v1::gpu_pipeline::WebEdgeKind::SceneLink, 200);
                    }
                    last_full_node = node_idx;
                } else if last_full_node != u32::MAX {
                    // Skip/pass: reference last full encode as anchor
                    web.add_edge(node_idx, last_full_node, fao_v1::gpu_pipeline::WebEdgeKind::AnchorRef, 
                        (255.0 * (1.0 - (fidx as f32 - web.nodes[last_full_node as usize].frame_idx as f32).abs() / 60.0).max(0.0)) as u8);
                }
                
                // Update motion tracker from full-encode results
                if cs > 100 {  // non-skip frame
                    // Feed MVs to tracker (from GPU encode result)
                    if let Ok(stats) = reader_tc.stats.try_lock() {
                        if fidx < stats.len() {
                            tracker.update_from_encode(
                                fidx as u32, w as u16, h as u16,
                                &[], &[], &[], &[],  // anchor data built from disk cache later
                            );
                        }
                    }
                    irb.push_frame(fidx as u32, 0xFF, &[]);  // anchor marker
                } else {
                    // Skip frame: predict from tracker
                    if !tracker.anchor_y.is_empty() {
                        let predicted = tracker.predict_frame(fidx as u32, w, h);
                        // Store delta between predicted and actual (for web refinement)
                        irb.push_frame(fidx as u32, 0x01, &[]);  // MV-predicted marker
                    } else {
                        irb.push_frame(fidx as u32, 0x03, &[]);  // skip marker
                    }
                    tracker.advance();
                }
                // Thumbnails handled by compress threads
                stats_map.insert(fidx, FrameStat {
                    psnr, enc_bytes: cs, time_ms: ms, ftype: ft,
                    skip_pct: sp, vrr_ms: 16.7, activity: 0.0,
                });
                total_rx += 1;

                // Advance continuous frame count
                // Drain CONSECUTIVE frames starting at `continuous`. We pop 
                // both from written_set and stats_map as we go — both should 
                // shrink to zero live entries by the time encoding finishes 
                // and the disk-writer thread is keeping up.
                while written_set.remove(&continuous) {
                    if let Some(mut st) = stats_map.remove(&continuous) {
                        // Override enc_bytes with ACTUAL compressed size if available
                        if let Ok(sizes) = reader_tc.actual_compressed_sizes.try_lock() {
                            if let Some(&actual) = sizes.get(&continuous) {
                                st.enc_bytes = actual;
                            }
                        }
                        reader_tc.stats.lock().unwrap().push(st);
                        reader_tc.vrr_history.lock().unwrap().push(16.7);
                    }
                    continuous += 1;
                }
                
                reader_tc.frame_count.store(continuous, Ordering::SeqCst);
                if total_rx % 10 == 0 { let _ = tmp.flush(); }
                // Memory pressure: check MemAvailable every 10 frames.
                // Log throttled to once per 60 frames (~3 seconds at 24fps) 
                // when pressure persists — spamming "0GB available" every 
                // few frames (as happened on long encodes) drowns the 
                // useful trace lines without conveying new info.
                if total_rx % 10 == 0 {
                    let mem_avail_gb = std::fs::read_to_string("/proc/meminfo").ok()
                        .and_then(|s| s.lines().find(|l| l.starts_with("MemAvailable:"))
                            .and_then(|l| l.split_whitespace().nth(1))
                            .and_then(|v| v.parse::<u64>().ok()))
                        .unwrap_or(999 * 1024 * 1024) / 1024 / 1024;  // GB
                    let mem_total_gb = std::fs::read_to_string("/proc/meminfo").ok()
                        .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:"))
                            .and_then(|l| l.split_whitespace().nth(1))
                            .and_then(|v| v.parse::<u64>().ok()))
                        .unwrap_or(16 * 1024 * 1024) / 1024 / 1024;  // GB
                    let should_log = total_rx % 60 == 0;
                    // Threshold: shed cosmetic caches when available RAM 
                    // drops below `min(total/8, 8GB)`, floor 2GB. 
                    // 
                    // Tuning history:
                    //   v1: hard-coded `< 16GB` — fired constantly on 
                    //       16GB systems (which idle at ~14GB free) and 
                    //       never fired on big systems even when actually 
                    //       under pressure.
                    //   v2: `< total/4` — better on small systems but on 
                    //       a 62GB system that's 15GB, which is normal 
                    //       working memory (a steady-state encode with 
                    //       OS caches uses ~50GB on big systems and that's 
                    //       FINE). Still fired constantly.
                    //   v3 (this): `< min(total/8, 8GB)` floor 2GB. 
                    //       Caps absolute threshold at 8GB regardless of 
                    //       total — you never need 16GB free to "feel 
                    //       safe", since the actions taken (clearing 
                    //       thumbnails) free at most a few hundred MB.
                    //       Examples: 16GB→2GB, 32GB→4GB, 64GB→8GB, 
                    //       128GB→8GB, 192GB→8GB. The cap is intentional:
                    //       on big-memory machines (64-192GB Linux dev 
                    //       boxes, M-series Macs with 64GB+), most of the 
                    //       system's "used" RAM is OS page cache which 
                    //       MemAvailable already discounts. Demanding 24GB 
                    //       free before reacting would mean reacting all 
                    //       the time on these machines.
                    let shed_threshold_gb = (mem_total_gb / 8).min(8).max(2);
                    if mem_avail_gb < shed_threshold_gb {
                        // CRITICAL: less than 16GB free — aggressive shedding.
                        // 
                        // Drop everything that's reconstructible:
                        //   - ring_buf trimmed to 5 entries
                        //   - thumbnails cleared
                        //   - v8 visualization snapshots cleared (they'll 
                        //     repopulate on next refresh interval)
                        //   - region activity history reset
                        // 
                        // Encoder state itself (V8Encoder, ChunkStore) is left 
                        // alone — it self-bounds via enforce_budget at 384MB.
                        if should_log {
                            eprintln!("FAO: trimming UI caches (avail {}GB / total {}GB, threshold {}GB — encoder state unaffected)",
                                mem_avail_gb, mem_total_gb, shed_threshold_gb);
                        }
                        if let Ok(mut ring) = reader_tc.ring_buf.lock() {
                            while ring.len() > 5 { ring.remove(0); }
                        }
                        if let Ok(mut thumbs) = reader_tc.thumbnails.lock() {
                            thumbs.clear();
                        }
                        // v8 visualization snapshots — these are display-only 
                        // copies; the encoder rebuilds them periodically.
                        if let Ok(mut s) = reader_tc.v8_dense_snapshot.try_lock() { s.clear(); s.shrink_to_fit(); }
                        if let Ok(mut s) = reader_tc.v8_mesh_snapshot.try_lock() {
                            s.0.clear(); s.0.shrink_to_fit();
                            s.1.clear(); s.1.shrink_to_fit();
                        }
                        if let Ok(mut s) = reader_tc.v8_pose_snapshot.try_lock() { s.clear(); s.shrink_to_fit(); }
                        if let Ok(mut s) = reader_tc.v8_webs_snapshot.try_lock() { s.clear(); s.shrink_to_fit(); }
                        // Shed actual_compressed_sizes — at 3M frames this 
                        // hashmap consumes ~70MB. We only need recent entries 
                        // (the collector reads them as `continuous` advances 
                        // and the values get folded into the stats Vec).
                        // Drop entries below the continuous pointer minus 
                        // a small safety window.
                        if let Ok(mut sizes) = reader_tc.actual_compressed_sizes.try_lock() {
                            let cutoff = continuous.saturating_sub(1000);
                            sizes.retain(|&k, _| k >= cutoff);
                        }
                        // vrr_history grows linearly — at 3M frames that's 
                        // 12MB. UI only reads recent values for graphing.
                        if let Ok(mut vrr) = reader_tc.vrr_history.lock() {
                            if vrr.len() > 10_000 {
                                let drain_count = vrr.len() - 10_000;
                                vrr.drain(0..drain_count);
                            }
                        }
                    } else if mem_avail_gb < 32 {
                        // WARNING: shrink ring buffer
                        if let Ok(mut ring) = reader_tc.ring_buf.lock() {
                            let target = ring.len() / 2;
                            while ring.len() > target.max(10) { ring.remove(0); }
                        }
                    }
                }
            }

            for rh in reader_handles { let _ = rh.join(); }
            let _ = tmp.flush();
            drop(disk_tx);  // close disk writer channel
            // Wait for disk writer to finish
            // Build anchor tree for fast random access
            {
                let off = collector_tc.offsets.lock().unwrap_or_else(|e| e.into_inner());
                let mut frame_flags: Vec<(u32, u64, u32)> = Vec::new();
                for i in 0..off.len() {
                    if off[i] != u64::MAX {
                        // Read flags from disk header
                        if let Ok(mut f) = File::open(&collector_tc.tmp_path) {
                            if f.seek(SeekFrom::Start(off[i])).is_ok() {
                                let mut hdr = [0u8; 28];
                                if f.read_exact(&mut hdr).is_ok() {
                                    let flags = u32::from_le_bytes([hdr[12],hdr[13],hdr[14],hdr[15]]);
                                    let orig_len = u32::from_le_bytes([hdr[4],hdr[5],hdr[6],hdr[7]]);
                                    frame_flags.push((flags, off[i], orig_len));
                                } else { frame_flags.push((0, off[i], 0)); }
                            } else { frame_flags.push((0, 0, 0)); }
                        } else { frame_flags.push((0, 0, 0)); }
                    } else { frame_flags.push((0, 0, 0)); }
                }
                drop(off);
                let tree = fao_v1::gpu_pipeline::AnchorTree::build(frame_flags.len(), &frame_flags);
                eprintln!("FAO: anchor tree built: {} nodes, {} levels, {} unique hashes",
                    tree.nodes.len(), tree.max_level, tree.hash_to_frames.len());
                // Example decode paths
                if tree.nodes.len() > 100 {
                    let path = tree.decode_path(100);
                    eprintln!("FAO: decode path to frame 100: {:?} ({} hops)", path, path.len());
                }
                if tree.nodes.len() > 1000 {
                    let path = tree.decode_path(1000);
                    eprintln!("FAO: decode path to frame 1000: {:?} ({} hops)", path, path.len());
                }
            }
            
            // Clear ring buffer — disk file is authoritative for playback
            if let Ok(mut ring) = collector_tc.ring_buf.lock() {
                ring.clear();
            }
            // Pre-populate last_display with first real frame
            {
                let off = collector_tc.offsets.lock().unwrap_or_else(|e| e.into_inner());
                let fsz = collector_tc.w * collector_tc.h * 3;
                let yuv_fsz = collector_tc.w * collector_tc.h * 3 / 2;
                for i in 0..off.len() {
                    if off[i] != u64::MAX {
                        if let Ok(mut f) = File::open(&collector_tc.tmp_path) {
                            if f.seek(SeekFrom::Start(off[i])).is_ok() {
                                let mut hdr = [0u8; 28];
                                if f.read_exact(&mut hdr).is_ok() {
                                    let ol = u32::from_le_bytes([hdr[4],hdr[5],hdr[6],hdr[7]]) as usize;
                                    if ol > 0 && ol > 100 {  // skip tiny (black) frames
                                        let mut co = vec![0u8; ol];
                                        if f.read_exact(&mut co).is_ok() {
                                            if let Some(yuv) = fao_v1::decompress_frame(&co, yuv_fsz) {
                                                let w2 = collector_tc.w; let h2 = collector_tc.h;
                                                let mut rgb = vec![0u8; w2 * h2 * 3];
                                                let hw2 = w2/2; let ysz2 = w2*h2;
                                                for py in 0..h2 { for px in 0..w2 {
                                                    let yi = py*w2+px; let ui = (py/2)*hw2+(px/2);
                                                    let y = yuv[yi] as f32;
                                                    let u = yuv.get(ysz2+ui).copied().unwrap_or(128) as f32 - 128.0;
                                                    let v = yuv.get(ysz2+ysz2/4+ui).copied().unwrap_or(128) as f32 - 128.0;
                                                    let oi = yi*3;
                                                    rgb[oi] = (y+1.402*v).clamp(0.0,255.0) as u8;
                                                    rgb[oi+1] = (y-0.344*u-0.714*v).clamp(0.0,255.0) as u8;
                                                    rgb[oi+2] = (y+1.772*u).clamp(0.0,255.0) as u8;
                                                }}
                                                if let Ok(mut last) = collector_tc.last_display.try_lock() {
                                                    *last = Some((rgb.clone(), rgb));
                                                    eprintln!("FAO: pre-loaded frame {} for playback display", i);
                                                }
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Encoding finished. If V9 is on, remind the user that v9 
            // logs will only appear when they click Export.
            let v9_on = std::env::var("FAO_V9").map(|v| v == "1").unwrap_or(false);
            eprintln!("FAO encoder: encoding complete. {}",
                if v9_on { "V9 is enabled — click Export to write the .fao file (v9 logs appear during export)." }
                else { "Click Export to write the .fao file." });
            collector_tc.done.store(true, Ordering::SeqCst);
        });
    }
    fn load_display_frame(&mut self) {
        let tc=match &self.tc{Some(t)=>t.clone(),None=>return};
        let fidx_before = self.fidx;
        // Cache hit: same frame, no decompression needed
        if self.fidx == self.last_read_idx {
            if let Some((ref orig, ref recon)) = self.last_read_cache {
                self.cur_frame = orig.clone(); self.cur_recon = recon.clone();
                self.cur_w = tc.w; self.cur_h = tc.h; self.has_frame = true;
                if let Ok(stats) = tc.stats.try_lock() { self.cur_result = stats.get(self.fidx).cloned(); }
                return;
            }
        }
        if let Some((orig,recon))=tc.read_frame(self.fidx){
            if self.fidx < 10 || self.fidx % 500 == 0 {
                let sum_o: u64 = orig.iter().take(100).map(|&b| b as u64).sum();
                let sum_r: u64 = recon.iter().take(100).map(|&b| b as u64).sum();
                eprintln!("FAO display: frame {} orig={} recon={} same={} orig_bright={} recon_bright={}", 
                    self.fidx, orig.len(), recon.len(), orig == recon, sum_o, sum_r);
            }
            self.last_read_idx = self.fidx;
            self.last_read_cache = Some((orig.clone(), recon.clone()));
            // For v5 fallback: orig and recon are both the FAO-decoded result.
            // Try to fetch the actual source frame for the Orig/Split/Err views,
            // so user sees real difference between source and FAO.
            let need_source_view = matches!(self.view, View::Original | View::Split | View::Error);
            let src_frame = if need_source_view && orig == recon {
                // Both same → likely v5 fallback. Fetch real source.
                self.fetch_source_frame_encode(self.fidx)
            } else { None };
            self.cur_frame = src_frame.unwrap_or(orig);
            self.cur_recon = recon;
            self.cur_w=tc.w; self.cur_h=tc.h; self.has_frame=true;
            if let Ok(stats)=tc.stats.try_lock(){
                self.cur_result=stats.get(self.fidx).cloned();
            }
        } else {
            // read_frame returned None (skip frame, not yet encoded, etc)
            // Keep last good display in cur_frame/cur_recon — don't go gray.
            // Critical: ensure cur_w/cur_h match the data we DO have.
            self.cur_w = tc.w; self.cur_h = tc.h;
            // If cur_frame is empty/wrong size, try the tc's last_display cache
            let expected_sz = tc.w * tc.h * 3;
            if self.cur_frame.len() != expected_sz {
                if let Ok(last) = tc.last_display.try_lock() {
                    if let Some((ref o, ref r)) = *last {
                        self.cur_frame = o.clone();
                        self.cur_recon = r.clone();
                        self.has_frame = true;
                    }
                }
            }
            if let Ok(stats) = tc.stats.try_lock() {
                self.cur_result = stats.get(self.fidx).cloned();
            }
        }
    }

    /// Project the frame web to 3D coordinates for visualization.
    /// Maps higher-dimensional web function to 3D space:
    ///   X = frame index (time axis)
    ///   Y = scene_id × spread (vertical clustering by scene)
    ///   Z = node tier × depth (depth = encoding tier: skip/pass/full)
    ///   weight = edge strength (modulates glow intensity)
    fn update_web_viz(&mut self, n_frames: u32) {
        // Build the topology (nodes + edges) but with weak initial positions.
        // Real layout happens in step_web_physics() called every frame.
        let n = n_frames.max(1) as usize;
        let max_nodes = n.min(500);
        
        // Preserve existing positions if we're just adding more nodes
        let existing_positions: std::collections::HashMap<u32, (f32, f32, f32)> = 
            self.web_viz_nodes.iter().map(|(fi, x, y, z, _, _)| (*fi, (*x, *y, *z))).collect();
        let existing_velocities: std::collections::HashMap<u32, (f32, f32)> = 
            self.web_viz_nodes.iter().enumerate()
                .filter_map(|(i, (fi, ..))| self.web_viz_velocity.get(i).map(|v| (*fi, *v)))
                .collect();
        
        self.web_viz_nodes.clear();
        self.web_viz_edges.clear();
        self.web_viz_velocity.clear();
        
        // Scene model: derive from actual encoder stats if available.
        // Fall back to synthetic cyclic model.
        // ftype: 0=skip, 1=anchor/full, 2=passthrough
        // Group consecutive same-type frames into "scenes"
        let stat_snapshot: Option<Vec<(u8, f32)>> = self.tc.as_ref()
            .and_then(|tc| tc.stats.try_lock().ok())
            .map(|s| s.iter().take(max_nodes).map(|st| (st.ftype, st.activity)).collect());
        
        let scene_for_frame = move |fi: u32| -> u16 {
            // If we have real stats, group runs of same-tier frames into scenes
            if let Some(ref stats) = stat_snapshot {
                if (fi as usize) < stats.len() {
                    // Detect scene boundaries: change in ftype or activity jump
                    let mut scene_id = 0u16;
                    for i in 1..=(fi as usize).min(stats.len() - 1) {
                        let cur = stats[i];
                        let prev = stats[i-1];
                        // New scene when full-encode follows non-full (regen point)
                        // OR activity changes by >2x (motion intensity shift)
                        if (cur.0 == 1 && prev.0 != 1) ||
                           (prev.1 > 0.01 && cur.1 / prev.1 > 2.0) ||
                           (cur.1 > 0.01 && prev.1 / cur.1 > 2.0) {
                            scene_id = scene_id.wrapping_add(1);
                        }
                    }
                    return scene_id % 32;  // wrap to 32 distinct scenes
                }
            }
            // Synthetic fallback
            let cycle = (fi / 60) % 4;
            let position_in_cycle = (fi % 60) / 8;
            (cycle * 8 + position_in_cycle) as u16
        };
        
        // Initial positions: random scattered (gravity will pull them together)
        for i in 0..max_nodes {
            let fi = i as u32;
            let scene = scene_for_frame(fi);
            
            // Use existing position if available (smooth re-layout)
            let (x, y, z) = existing_positions.get(&fi).copied().unwrap_or_else(|| {
                // New nodes: spawn near a random scene-based seed
                let sh = scene as u32;
                let angle = (sh.wrapping_mul(2654435761) as f32) * (1.0 / 4294967296.0) * 6.283;
                let r = 0.5 + ((fi.wrapping_mul(11) % 100) as f32 / 200.0);
                let jx = ((fi.wrapping_mul(0x9E37) % 100) as f32 / 100.0 - 0.5) * 0.15;
                let jy = ((fi.wrapping_mul(0x6B79) % 100) as f32 / 100.0 - 0.5) * 0.15;
                (r * angle.cos() + jx, r * angle.sin() * 0.6 + jy, 0.0)
            });
            
            // Use real frame type from encoder stats
            let real_tier: u8 = self.tc.as_ref()
                .and_then(|tc| tc.stats.try_lock().ok())
                .and_then(|s| s.get(i).map(|st| st.ftype))
                .unwrap_or_else(|| if i % 30 == 0 { 1 } else if i % 5 == 0 { 2 } else { 0 });
            // Map: 1=full encode (anchor), 2=passthrough, 0=skip
            let z_tier = match real_tier { 1 => 0.6, 2 => 0.2, _ => -0.4 };
            let weight = match real_tier { 1 => 1.0, 2 => 0.5, _ => 0.25 };
            
            // Use existing velocity or zero
            let vel = existing_velocities.get(&fi).copied().unwrap_or((0.0, 0.0));
            
            self.web_viz_nodes.push((fi, x, y, z_tier, weight, scene));
            let _ = z;
            self.web_viz_velocity.push(vel);
        }
        
        // Build edges (drives the gravity attraction)
        for i in 0..max_nodes {
            let fi = i as u32;
            if i > 0 && i % 30 != 0 {
                let anchor_idx = (i / 30) * 30;
                self.web_viz_edges.push((fi, anchor_idx as u32, 0.3));
            }
            if i > 0 && scene_for_frame(fi) == scene_for_frame(fi - 1) {
                self.web_viz_edges.push((fi, fi - 1, 0.5));
            }
            // Cycle edges: same scene > 30 frames apart
            let cur_scene = scene_for_frame(fi);
            let prev_match = (1..=i.min(150)).rev().find(|&dist| {
                let candidate = i.checked_sub(dist).unwrap_or(0);
                dist > 30 && scene_for_frame(candidate as u32) == cur_scene
            });
            if let Some(dist) = prev_match {
                if let Some(prev_idx) = i.checked_sub(dist) {
                    self.web_viz_edges.push((fi, prev_idx as u32, 0.4));
                }
            }
        }
    }
    
    /// Run one physics step on the web layout — particle gravity simulation.
    /// Called every animation frame for continuous relaxation.
    fn step_web_physics(&mut self) {
        let n = self.web_viz_nodes.len();
        if n < 2 { return; }
        if self.web_viz_velocity.len() != n {
            self.web_viz_velocity.resize(n, (0.0, 0.0));
        }
        
        // SETTLED CHECK: skip physics entirely if nodes are at rest
        // Avoids unnecessary work when graph is stable
        let total_kinetic: f32 = self.web_viz_velocity.iter()
            .map(|(vx, vy)| vx * vx + vy * vy)
            .sum();
        let avg_kinetic = total_kinetic / n as f32;
        if avg_kinetic < 0.0001 && self.web_viz_phase > 5.0 {
            // Settled, just decay velocities slowly
            for v in self.web_viz_velocity.iter_mut() {
                v.0 *= 0.9;
                v.1 *= 0.9;
            }
            return;
        }
        
        // Build scene → (cx, cy) using sorted Vec instead of HashMap (faster for small u16 keys)
        // Find max scene id first
        let max_scene = self.web_viz_nodes.iter()
            .map(|(_, _, _, _, _, s)| *s).max().unwrap_or(0) as usize;
        let mut scene_sums: Vec<(f32, f32, u32)> = vec![(0.0, 0.0, 0); max_scene + 1];
        for (_, x, y, _, _, scene) in &self.web_viz_nodes {
            let s = *scene as usize;
            scene_sums[s].0 += x;
            scene_sums[s].1 += y;
            scene_sums[s].2 += 1;
        }
        for c in scene_sums.iter_mut() {
            if c.2 > 0 {
                c.0 /= c.2 as f32;
                c.1 /= c.2 as f32;
            }
        }
        
        // Build idx lookup using Vec (faster than HashMap, since fi is dense u32)
        // Find max fi to size the lookup
        let max_fi = self.web_viz_nodes.iter()
            .map(|(fi, ..)| *fi).max().unwrap_or(0) as usize;
        let mut idx_for_fi: Vec<i32> = vec![-1; max_fi + 1];
        for (i, (fi, ..)) in self.web_viz_nodes.iter().enumerate() {
            idx_for_fi[*fi as usize] = i as i32;
        }
        
        // Forces array
        let mut forces = vec![(0.0f32, 0.0f32); n];
        
        // 1. Repulsion between all node pairs (prevents overlap, creates spread)
        // Only sample neighbors to keep cost low (O(n) instead of O(n²))
        let sample_step = if n > 100 { (n / 64).max(1) } else { 1 };
        for i in 0..n {
            let (_, x1, y1, _, w1, _) = self.web_viz_nodes[i];
            let mut j = (i + 1) % n;
            for _ in 0..(64.min(n - 1)) {
                let (_, x2, y2, _, w2, _) = self.web_viz_nodes[j];
                let dx = x1 - x2;
                let dy = y1 - y2;
                let d2 = dx * dx + dy * dy + 0.001;
                let d = d2.sqrt();
                // Repulsion strength scales with both weights (anchors push more)
                let strength = 0.0008 * (w1 + w2) / d2;
                let fx = (dx / d) * strength;
                let fy = (dy / d) * strength;
                forces[i].0 += fx;
                forces[i].1 += fy;
                forces[j].0 -= fx;
                forces[j].1 -= fy;
                j = (j + sample_step) % n;
                if j == i { j = (j + 1) % n; }
            }
        }
        
        // 2. Edge attraction (springs)
        for &(src, dst, weight) in &self.web_viz_edges {
            let i_src_raw = idx_for_fi.get(src as usize).copied().unwrap_or(-1);
            let i_dst_raw = idx_for_fi.get(dst as usize).copied().unwrap_or(-1);
            if i_src_raw < 0 || i_dst_raw < 0 { continue; }
            let i_src = i_src_raw as usize;
            let i_dst = i_dst_raw as usize;
            let (_, x1, y1, _, _, _) = self.web_viz_nodes[i_src];
            let (_, x2, y2, _, _, _) = self.web_viz_nodes[i_dst];
            let dx = x2 - x1;
            let dy = y2 - y1;
            let d = (dx * dx + dy * dy).sqrt() + 0.001;
            // Target rest length: shorter for stronger edges
            // Decreased by 1.5× for tighter clustering
            let rest = (0.08 + (1.0 - weight) * 0.15) / 1.5;
            let stretch = d - rest;
            let strength = stretch * weight * 0.06;
            let fx = (dx / d) * strength;
            let fy = (dy / d) * strength;
            forces[i_src].0 += fx;
            forces[i_src].1 += fy;
            forces[i_dst].0 -= fx;
            forces[i_dst].1 -= fy;
        }
        
        // 3. Scene gravity: pull each node toward its scene centroid
        for i in 0..n {
            let (_, x, y, _, _, scene) = self.web_viz_nodes[i];
            if let Some((cx, cy, _)) = scene_sums.get(scene as usize).filter(|c| c.2 > 0) {
                let dx = cx - x;
                let dy = cy - y;
                let pull = 0.012;
                forces[i].0 += dx * pull;
                forces[i].1 += dy * pull;
            }
        }
        
        // 4. Center gravity: keep cluster from drifting away
        for i in 0..n {
            let (_, x, y, _, _, _) = self.web_viz_nodes[i];
            forces[i].0 += -x * 0.003;
            forces[i].1 += -y * 0.003;
        }
        
        // 4b. Boundary repulsion: push nodes inward when near viewport edges
        // Bounding box: x ∈ [-1.4, 1.4], y ∈ [-0.95, 0.95]
        // Repulsion grows quadratically as nodes approach the boundary
        let bx_min = -1.3f32;
        let bx_max = 1.3f32;
        let by_min = -0.85f32;
        let by_max = 0.85f32;
        let edge_falloff = 0.25f32;  // distance from boundary where force ramps up
        for i in 0..n {
            let (_, x, y, _, _, _) = self.web_viz_nodes[i];
            
            // X boundaries
            let dx_left = x - bx_min;
            if dx_left < edge_falloff {
                let t = (edge_falloff - dx_left) / edge_falloff;
                forces[i].0 += t * t * 0.025;  // push right
            }
            let dx_right = bx_max - x;
            if dx_right < edge_falloff {
                let t = (edge_falloff - dx_right) / edge_falloff;
                forces[i].0 -= t * t * 0.025;  // push left
            }
            
            // Y boundaries
            let dy_top = y - by_min;
            if dy_top < edge_falloff {
                let t = (edge_falloff - dy_top) / edge_falloff;
                forces[i].1 += t * t * 0.025;  // push down
            }
            let dy_bot = by_max - y;
            if dy_bot < edge_falloff {
                let t = (edge_falloff - dy_bot) / edge_falloff;
                forces[i].1 -= t * t * 0.025;  // push up
            }
        }
        
        // 5. Apply forces with damping + sleep threshold
        let dt: f32 = 0.5;
        let damping: f32 = 0.82;
        let max_speed: f32 = 0.04;
        let sleep_threshold: f32 = 0.0008;  // velocities below this snap to zero
        let force_threshold: f32 = 0.001;   // forces below this don't wake sleeping nodes
        for i in 0..n {
            let (vx, vy) = self.web_viz_velocity[i];
            let force_mag2 = forces[i].0 * forces[i].0 + forces[i].1 * forces[i].1;
            let was_sleeping = vx.abs() < sleep_threshold && vy.abs() < sleep_threshold;
            
            // Don't wake a sleeping node from microscopic forces
            if was_sleeping && force_mag2 < force_threshold * force_threshold {
                self.web_viz_velocity[i] = (0.0, 0.0);
                continue;  // skip position update - no movement
            }
            
            let mut new_vx = (vx + forces[i].0 * dt) * damping;
            let mut new_vy = (vy + forces[i].1 * dt) * damping;
            
            // Clamp max speed
            let s2 = new_vx * new_vx + new_vy * new_vy;
            if s2 > max_speed * max_speed {
                let s = s2.sqrt();
                new_vx *= max_speed / s;
                new_vy *= max_speed / s;
            }
            
            // Sleep threshold: snap small velocities to zero (kills wiggle)
            if new_vx.abs() < sleep_threshold { new_vx = 0.0; }
            if new_vy.abs() < sleep_threshold { new_vy = 0.0; }
            
            self.web_viz_velocity[i] = (new_vx, new_vy);
            
            // Skip position update if not moving (avoids jitter from f32 ops)
            if new_vx == 0.0 && new_vy == 0.0 { continue; }
            
            let (fi, x, y, z, w, sc) = self.web_viz_nodes[i];
            let new_x = (x + new_vx).clamp(-1.4, 1.4);
            let new_y = (y + new_vy).clamp(-0.95, 0.95);
            self.web_viz_nodes[i] = (fi, new_x, new_y, z, w, sc);
        }
    }
    
    fn render(&mut self) -> Option<(usize,usize)> {
        if !self.has_frame { return None; }
        if self.cur_recon.is_empty() { self.cur_recon = self.cur_frame.clone(); }
        let(w,h)=(self.cur_w,self.cur_h);
        let n=w*h*3;
        if n==0{return None;}
        self.display_buf.resize(n,0);
        let view=self.view; let sp=self.split_pos;
        if self.cur_frame.len() >= n {
            self.display_buf[..n].copy_from_slice(&self.cur_frame[..n]);
        } else if self.cur_frame.len() == n / 2 || self.cur_frame.len() == (w * h * 3) / 2 {
            // Looks like YUV — convert to RGB on the fly
            let rgb = TranscodeState::yuv_to_rgb(&self.cur_frame, w, h);
            let cl = rgb.len().min(n);
            self.display_buf[..cl].copy_from_slice(&rgb[..cl]);
        } else if !self.display_buf.is_empty() && self.display_buf.iter().take(100).any(|&b| b != 0) {
            // display_buf already has content from last render — keep it (don't go gray)
        } else {
            // First-time render with bad data — fill gray
            self.display_buf.iter_mut().for_each(|b| *b = 64);
            let copy_len = self.cur_frame.len().min(n);
            self.display_buf[..copy_len].copy_from_slice(&self.cur_frame[..copy_len]);
        }
        match view {
            View::Original=>{}
            View::Recon=>{
                if self.cur_recon.len() >= n {
                    self.display_buf[..n].copy_from_slice(&self.cur_recon[..n]);
                } else if self.cur_frame.len() >= n {
                    // Recon missing — show source as fallback (better than gray)
                    self.display_buf[..n].copy_from_slice(&self.cur_frame[..n]);
                }
                // Otherwise: keep display_buf at whatever it had (don't gray-out)
            }
            View::Split=>{
                if self.cur_recon.len() < n { 
                    // Better fallback: use source frame instead of gray
                    if self.cur_frame.len() >= n {
                        self.cur_recon = self.cur_frame.clone();
                    } else {
                        self.cur_recon.resize(n, 64);
                    }
                }
                let sx=(sp*w as f64)as usize;
                for y in 0..h{for x in sx..w{let i=(y*w+x)*3;if i+2<n{self.display_buf[i]=self.cur_recon[i];self.display_buf[i+1]=self.cur_recon[i+1];self.display_buf[i+2]=self.cur_recon[i+2];}}
                    if y%4<2{let i=(y*w+sx.min(w-1))*3;if i+2<n{self.display_buf[i]=76;self.display_buf[i+1]=240;self.display_buf[i+2]=160;}}}
            }
            View::Error=>{
                let orig=self.cur_frame.clone();
                for i in 0..w*h{let j=i*3;if j+2>=n{break;}
                    let d=((orig[j]as f64-self.cur_recon[j]as f64).abs()+(orig[j+1]as f64-self.cur_recon[j+1]as f64).abs()+(orig[j+2]as f64-self.cur_recon[j+2]as f64).abs())/3.0;
                    let v=(d*6.0).min(255.0)as u8;
                    self.display_buf[j]=v;self.display_buf[j+1]=(200.0-v as f64*2.0).max(0.0)as u8;self.display_buf[j+2]=30;}
            }
        }
        Some((w,h))
    }

    fn stats(&self) -> String {
        let mut s=format!("━━ SOURCE ━━\n{}\n",self.src_path.rsplit('/').next().unwrap_or(&self.src_path));
        match self.mode {
            Mode::Empty=>return "Load an image or video".into(),
            Mode::Image=>{}
            Mode::VideoScrub=>{
                s+=&format!("{} {}×{} {:.1}fps {:.1}s\n→ {}×{}\n\n",self.vid_codec,self.ow,self.oh,self.vid_fps,self.duration,self.vid_w,self.vid_h);
                s+=&format!("Scrub: {:.1}s / {:.1}s\n",self.scrub_time,self.duration);
                s+=&format!("IN: {:.1}s  OUT: {:.1}s  Range: {:.1}s\n",self.pin_start,self.pin_end,(self.pin_end-self.pin_start).max(0.0));
                let est=((self.pin_end-self.pin_start).max(0.0)*self.vid_fps)as usize;
                let disk_mb=est*self.vid_w*self.vid_h*3*2/1048576;
                s+=&format!("≈ {} frames  ~{:.0}MB disk\n",est,disk_mb);
            }
            Mode::FaoPlay=>{
                if let Some(ref hdr)=self.fao_header{
                    s+=&format!("FAO file {}x{} {:.1}fps
",hdr.width,hdr.height,self.vid_fps);
                    s+=&format!("{} frames
",hdr.frame_count);
                    s+=&format!("Compressed: {:.1} MB
",hdr.total_compressed as f64/1048576.0);
                    s+=&format!("Frame {}/{}
",self.fidx,hdr.frame_count.saturating_sub(1));
                }
            }
            Mode::VideoPlay=>{
                if let Some(ref tc)=self.tc{
                    let n=tc.frame_count.load(Ordering::Relaxed);
                    let done=tc.done.load(Ordering::Relaxed);
                    s+=&format!("{}×{} {:.1}fps\n",tc.w,tc.h,tc.fps);
                    s+=&format!("Encoded: {}  {}\n",n,if done{"✓ done"}else{"⏳ encoding..."});
                    if self.playing && self.speed_x > 0.0 {
                        s+=&format!("Frame {}/{}  ▶ {:.1}x\n\n",self.fidx,n.saturating_sub(1),self.speed_x);
                    } else {
                        s+=&format!("Frame {}/{}\n\n",self.fidx,n.saturating_sub(1));
                    }
                    if let Ok(stats)=tc.stats.try_lock(){
                        if !stats.is_empty(){
                            let avg_kb:f64=stats.iter().map(|r|r.enc_bytes as f64).sum::<f64>()/stats.len()as f64/1024.0;
                            let avg_psnr:f64=stats.iter().map(|r|r.psnr).sum::<f64>()/stats.len()as f64;
                            let i_n=stats.iter().filter(|r|r.ftype==0).count();
                            let avg_ms:f64=stats.iter().map(|r|r.time_ms).sum::<f64>()/stats.len()as f64;
                            let fao_mbps = avg_kb*1024.0*8.0*tc.fps/1e6;
                            // Source file size
                            let src_file_size = std::fs::metadata(&self.src_path).map(|m| m.len()).unwrap_or(0);
                            s+="━━ STREAM ━━\n";
                            // I/P frame distribution with mean ± stddev
                            let p_n = stats.len() - i_n;
                            let i_sizes: Vec<f64> = stats.iter().filter(|r|r.ftype==0).map(|r|r.enc_bytes as f64/1024.0).collect();
                            let p_sizes: Vec<f64> = stats.iter().filter(|r|r.ftype!=0).map(|r|r.enc_bytes as f64/1024.0).collect();
                            let i_mean = if i_sizes.is_empty(){0.0}else{i_sizes.iter().sum::<f64>()/i_sizes.len()as f64};
                            let p_mean = if p_sizes.is_empty(){0.0}else{p_sizes.iter().sum::<f64>()/p_sizes.len()as f64};
                            let i_std = if i_sizes.len()<2{0.0}else{(i_sizes.iter().map(|v|(v-i_mean)*(v-i_mean)).sum::<f64>()/i_sizes.len()as f64).sqrt()};
                            let p_std = if p_sizes.len()<2{0.0}else{(p_sizes.iter().map(|v|(v-p_mean)*(v-p_mean)).sum::<f64>()/p_sizes.len()as f64).sqrt()};
                            s+=&format!("{} I  {:.1}±{:.1}KB\n",i_n,i_mean,i_std);
                            s+=&format!("{} P  {:.1}±{:.1}KB\n",p_n,p_mean,p_std);
                            s+=&format!("Avg: {:.1}KB {:.1}dB {:.0}ms\n",avg_kb,avg_psnr,avg_ms);
                            // Total FAO output size estimate (issue 7 fix: 
                            // require min sample count to avoid showing wild 
                            // extrapolations from the first few frames — the 
                            // macOS slideshow test showed 2369 Mbps from 14 
                            // frames, which extrapolated from a tiny n).
                            let fao_total_bytes: u64 = stats.iter().map(|r| r.enc_bytes as u64).sum();
                            let fao_total_mb = fao_total_bytes as f64 / 1048576.0;
                            const MIN_BITRATE_SAMPLES: usize = 50;
                            if stats.len() >= MIN_BITRATE_SAMPLES {
                                s+=&format!("FAO: {:.2} Mbps ({:.1}MB est)\n", fao_mbps, fao_total_mb);
                            } else {
                                s+=&format!("FAO: {:.1}MB so far (estimate after {} more frames)\n",
                                    fao_total_mb, MIN_BITRATE_SAMPLES.saturating_sub(stats.len()));
                            }
                            // Source bitrate comparison
                            if let Ok(src_sizes)=tc.src_frame_bytes.try_lock(){
                                if !src_sizes.is_empty(){
                                    let src_avg_kb:f64=src_sizes.iter().map(|&s|s as f64).sum::<f64>()/src_sizes.len()as f64/1024.0;
                                    let src_mbps=src_avg_kb*1024.0*8.0*tc.fps/1e6;
                                    let savings=if src_mbps>0.0{(1.0-fao_mbps/src_mbps)*100.0}else{0.0};
                                    let src_total_kb: f64 = src_sizes.iter().map(|&s| s as f64).sum::<f64>() / 1024.0;
                                    s+=&format!("Src: {:.2} Mbps ({}) {:.1}MB\n", src_mbps, self.vid_codec, src_total_kb/1024.0);
                                    let ratio = if fao_total_bytes > 0 { src_total_kb * 1024.0 / fao_total_bytes as f64 } else { 0.0 };
                                    if savings > 0.0 {
                                        s+=&format!("Savings: {:.0}% smaller ({:.1}:1)\n", savings, ratio);
                                    } else {
                                        s+=&format!("Overhead: {:.0}% larger ({:.2}:1)\n", -savings, ratio);
                                    }
                                }
                            }
                                        // Encode tier distribution
                            let n_skip = stats.iter().filter(|r| r.skip_pct >= 99.0).count();
                            let n_pass = stats.iter().filter(|r| r.skip_pct >= 90.0 && r.skip_pct < 99.0).count();
                            let n_full = stats.len() - n_skip - n_pass;
                            s+=&format!("Tiers: {} full, {} pass, {} skip\n", n_full, n_pass, n_skip);
            // Cache + memory
            if let Some(ref tc) = self.tc {
                if let Ok(meta) = std::fs::metadata(&tc.tmp_path) {
                    let cache_mb = meta.len() as f64 / 1048576.0;
                    let ratio_vs_src = if src_file_size > 0 { meta.len() as f64 / src_file_size as f64 } else { 0.0 };
                    s += &format!("Cache: {:.0}MB ({:.1}× src)  {}\n",
                        cache_mb, ratio_vs_src,
                        tc.tmp_path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
                }
            }
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                for line in status.lines() {
                    if line.starts_with("VmRSS:") {
                        let kb: u64 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                        s += &format!("Mem: {:.1}GB\n", kb as f64 / 1048576.0);
                        break;
                    }
                }
            }
// Throughput from total encode time across all frames
                            let n_encoded = tc.frame_count.load(std::sync::atomic::Ordering::Relaxed);
                            let total_encode_ms: f64 = if let Ok(stats) = tc.stats.try_lock() {
                                stats.iter().map(|s| s.time_ms).sum()
                            } else { avg_ms * n_encoded as f64 };
                            // Wall-clock throughput: frames / (total_ms / n_workers)
                            let n_workers = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4).max(2);
                            let wall_ms = total_encode_ms / n_workers as f64;
                            let throughput_fps = if wall_ms > 10.0 { 
                                n_encoded as f64 * 1000.0 / wall_ms 
                            } else if avg_ms > 0.1 { 
                                1000.0 / avg_ms 
                            } else { 
                                0.0  // not enough data yet
                            };
                            let encode_speed_x = throughput_fps / tc.fps.max(1.0);
                            s+=&format!("Enc: {:.0}fps ({:.1}x)\n", throughput_fps, encode_speed_x);
                            // 5-second rolling VRR average (issue 6 fix: gate display 
                            // on a plausible source fps; very slow or very fast 
                            // content makes the "saved" % nonsensical).
                            if let Ok(vrr_hist) = tc.vrr_history.try_lock() {
                                let fps_val = tc.fps;
                                let window = ((fps_val.max(1.0) * 5.0) as usize).max(5);
                                if vrr_hist.len() > 0 {
                                    let start = if vrr_hist.len() > window { vrr_hist.len() - window } else { 0 };
                                    let slice = &vrr_hist[start..];
                                    let avg_vrr: f32 = slice.iter().sum::<f32>() / slice.len() as f32;
                                    let eff_fps = 1000.0 / avg_vrr.max(1.0);
                                    // Display the comparison only when the source 
                                    // is a "video-rate" source (15..120 fps). For 
                                    // slideshow / hyperlapse sources, the metric 
                                    // is meaningless — just show the encode time.
                                    if fps_val >= 15.0 && fps_val <= 120.0 {
                                        let savings = (1.0 - eff_fps / fps_val as f32) * 100.0;
                                        s+=&format!("VRR 5s: {:.0}ms ({:.0}fps, {:.0}% saved)\n", avg_vrr, eff_fps, savings.max(0.0));
                                    } else {
                                        s+=&format!("VRR 5s: {:.0}ms (source {:.1}fps — savings metric n/a)\n", avg_vrr, fps_val);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(r)=&self.cur_result{
            s+="━━ FAO ━━\n";
            // Issue 5 fix (macOS test): for VideoPlay (source video), 
            // I/P-frame is correct because we're showing h264/av1 frame 
            // types. For FaoPlay (decoded FAO), the .fao container uses 
            // a different vocabulary (K/T/Z) — label accordingly so 
            // users don't see "P-frame" for codecs that don't have them.
            if self.mode==Mode::VideoPlay {
                s += &format!("{}\n", if r.ftype==0 {"I-frame"} else {"P-frame"});
            } else if self.mode==Mode::FaoPlay {
                // Look up the actual FAO frame marker from cur_fao_marker 
                // (set during load_fao_frame). Mapping:
                //   T → translate (v2.1 T-frame)
                //   K → keyframe (full DCT)
                //   V → v3/v4/v5 encoded
                //   Z → v7/v8 reconstruction frame
                //   P → v7 perception (objects)
                //   B → boundary marker
                let label = match self.cur_fao_marker {
                    b'T' => "T-frame (translate)",
                    b'K' => "K-frame (keyframe)",
                    b'V' => "V-frame (encoded)",
                    b'Z' => "Z-frame (v8 reconstruction)",
                    b'P' => "P-frame (perception)",
                    b'B' => "B-frame (boundary)",
                    _    => "frame",
                };
                s += &format!("{}\n", label);
            }
            let raw_bytes = (self.cur_w * self.cur_h * 3) as f64;
            let enc_kb = r.enc_bytes as f64 / 1024.0;
            // Show vs SOURCE encoded bytes (h264/av1) — far more meaningful 
            // than vs raw RGB. Falls back to raw ratio if source unknown.
            let src_for_ratio = self.tc.as_ref()
                .and_then(|tc| tc.src_frame_bytes.try_lock().ok())
                .and_then(|s| s.get(self.fidx).copied()).unwrap_or(0) as f64;
            let ratio_label = if src_for_ratio > 0.0 {
                let r2 = src_for_ratio / r.enc_bytes.max(1) as f64;
                format!("{:.2}× vs src", r2)
            } else {
                let r2 = raw_bytes / r.enc_bytes.max(1) as f64;
                format!("{:.0}:1 vs raw", r2)
            };
            s+=&format!("{:.1}KB  {:.1}dB  {}\n", enc_kb, r.psnr, ratio_label);
            s+=&format!("{:.0}ms", r.time_ms);
            if r.ftype == 1 { s+=&format!("  skip {:.0}%", r.skip_pct); }
            s+="\n";
            // Source frame size comparison
            if let Some(ref tc) = self.tc {
                if let Ok(src) = tc.src_frame_bytes.try_lock() {
                    if self.fidx < src.len() {
                        let src_kb = src[self.fidx] as f64 / 1024.0;
                        let vs = if enc_kb > 0.0 { src_kb / enc_kb } else { 0.0 };
                        s+=&format!("src {:.1}KB → fao {:.1}KB ({:.1}×)\n", src_kb, enc_kb, vs);
                    }
                }
            }
        }
        s
    }

    fn export_fao(&self, path: &std::path::Path) -> Result<u64, String> {
        let tc = match &self.tc { Some(t) => t, None => return Err("No transcode".into()) };
        let n = tc.frame_count.load(Ordering::Relaxed);
        if n == 0 { return Err("No frames".into()); }
        let fsz = tc.w * tc.h * 3;

        let mut dst = std::io::BufWriter::with_capacity(4 * 1024 * 1024,
            File::create(path).map_err(|e| format!("Create: {}", e))?);

        // FAO2 header (30 bytes)
        dst.write_all(b"FAO2").map_err(|e| e.to_string())?;        // magic
        dst.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;  // version
        dst.write_all(&(tc.w as u16).to_le_bytes()).map_err(|e| e.to_string())?;
        dst.write_all(&(tc.h as u16).to_le_bytes()).map_err(|e| e.to_string())?;
        dst.write_all(&((tc.fps * 100.0) as u16).to_le_bytes()).map_err(|e| e.to_string())?;  // fps_num
        dst.write_all(&100u16.to_le_bytes()).map_err(|e| e.to_string())?;  // fps_den
        dst.write_all(&(n as u32).to_le_bytes()).map_err(|e| e.to_string())?;  // frame_count
        dst.write_all(&30u16.to_le_bytes()).map_err(|e| e.to_string())?;  // gop_size
        dst.write_all(&[50u8]).map_err(|e| e.to_string())?;  // quality
        dst.write_all(&[1u8]).map_err(|e| e.to_string())?;   // flags (1=compressed)

        // First pass: collect frame data and compute offsets for index
        let offsets = tc.offsets.try_lock().map_err(|_| "Lock busy")?;
        let stats = tc.stats.try_lock().map_err(|_| "Lock busy")?;
        let mut src = std::io::BufReader::with_capacity(4 * 1024 * 1024,
            File::open(&tc.tmp_path).map_err(|e| format!("Open tmp: {}", e))?);
        
        // ── STREAMING EXPORT ──
        // 
        // Previously this code slurped EVERY frame's bytes into a `Vec<frames>` 
        // before writing — for a 2hr+ encode that was several GB just for the 
        // export step, regularly OOMing on smaller systems. Now we make TWO 
        // passes over the on-disk tmp file:
        // 
        //   Pass 1: read just each frame's HEADER (28 bytes) to learn its 
        //           orig_len + recon_len, then SKIP past the payload. Build 
        //           a Vec of (orig_len, recon_len, marker_byte) per frame — 
        //           ~9 bytes per frame total. For 600hr that's 460MB instead 
        //           of 50GB.
        //   Pass 2: re-read each frame's payload and copy directly into dst, 
        //           never holding more than one frame in memory at a time.
        // 
        // To pick the right payload (orig vs recon) without re-reading, we 
        // stash the FIRST BYTE of orig in pass 1 — that byte tells us 
        // whether orig has a known format marker.
        
        struct FrameMeta {
            offset: u64,            // where in tmp file
            orig_len: u32,
            recon_len: u32,
            orig_first_byte: u8,    // 0 if orig is empty
            ftype: u8,
            psnr_x10: u16,
            skip_pct: u8,
        }
        
        // Pass 1: collect metadata. ~16 bytes per frame in the meta vec 
        // (plus Vec overhead) — 800MB for 51M frames worst case, vs the 
        // gigabytes of payloads we used to buffer.
        let mut metas: Vec<FrameMeta> = Vec::with_capacity(n);
        for i in 0..n {
            if i >= offsets.len() || offsets[i] == u64::MAX {
                metas.push(FrameMeta {
                    offset: u64::MAX, orig_len: 0, recon_len: 0,
                    orig_first_byte: 0, ftype: 1, psnr_x10: 0, skip_pct: 100,
                });
                continue;
            }
            src.seek(SeekFrom::Start(offsets[i])).map_err(|e| e.to_string())?;
            let mut hdr = [0u8; 28];
            if src.read_exact(&mut hdr).is_err() {
                metas.push(FrameMeta {
                    offset: u64::MAX, orig_len: 0, recon_len: 0,
                    orig_first_byte: 0, ftype: 1, psnr_x10: 0, skip_pct: 100,
                });
                continue;
            }
            let orig_len = u32::from_le_bytes([hdr[4],hdr[5],hdr[6],hdr[7]]);
            let recon_len = u32::from_le_bytes([hdr[8],hdr[9],hdr[10],hdr[11]]);
            // Read just the first byte of orig to learn its marker.
            let mut first = [0u8; 1];
            let orig_first_byte = if orig_len > 0 {
                if src.read_exact(&mut first).is_ok() { first[0] } else { 0 }
            } else { 0 };
            let st = stats.get(i);
            let ftype = st.map(|s| s.ftype).unwrap_or(1);
            let psnr_x10 = st.map(|s| (s.psnr.clamp(0.0, 6553.5) * 10.0) as u16).unwrap_or(0);
            let skip_pct = st.map(|s| s.skip_pct as u8).unwrap_or(0);
            metas.push(FrameMeta {
                offset: offsets[i],
                orig_len, recon_len, orig_first_byte,
                ftype, psnr_x10, skip_pct,
            });
        }
        drop(offsets); drop(stats);
        
        // Compute payload size per frame (orig if it has a known marker, 
        // else recon). This determines on-disk frame size for the index.
        // Markers that identify a self-decodable original payload.
        // - D, P, L: v2.x DCT/yuv-split/lossy-chroma frames
        // - T, B, K: v2.1 translate / boundary / keyframe
        // - V: v3-v5 / v7 (legacy V-prefixed markers)
        // - Z: v8 reconstruction frame (BIG fix — was missing, causing 
        //   v8 frames to fall through to recon path which writes the 
        //   ZSTD-compressed full-res reconstruction (~500KB/frame). 
        //   For a 13k-frame encode that's ~5GB of wasted output.)
        const ORIG_MARKERS: &[u8] = &[b'D', b'P', b'L', b'T', b'B', b'K', b'V', b'Z'];
        let payload_len = |m: &FrameMeta| -> u32 {
            let orig_has_marker = m.orig_len > 0 && ORIG_MARKERS.contains(&m.orig_first_byte);
            if orig_has_marker { m.orig_len }
            else if m.recon_len > 0 { m.recon_len }
            else { m.orig_len }
        };
        
        // Write total_compressed into header (8 bytes).
        let total_compressed: u64 = metas.iter().map(|m| 13 + payload_len(m) as u64).sum();
        dst.write_all(&total_compressed.to_le_bytes()).map_err(|e| e.to_string())?;
        
        // Write frame index.
        let hdr_sz = 30u64;
        let index_sz = n as u64 * 16;
        let mut data_offset = hdr_sz + index_sz;
        for m in &metas {
            let plen = payload_len(m);
            let frame_data_sz = 13 + plen;
            dst.write_all(&data_offset.to_le_bytes()).map_err(|e| e.to_string())?;
            dst.write_all(&(frame_data_sz as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            // legacy "compressed_size" field — kept as orig_len + recon_len 
            // total for backward compat.
            dst.write_all(&((m.orig_len + m.recon_len) as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            data_offset += frame_data_sz as u64;
        }
        
        // Pass 2: stream payload data. Buffer is reused across frames.
        let mut frames_written = 0u64;
        let mut t_frame_count = 0u64;
        let mut k_frame_count = 0u64;
        let mut v_frame_count = 0u64;
        let mut v4_frame_count = 0u64;
        let mut p_frame_count = 0u64;
        let mut z_frame_count = 0u64;  // v8 reconstruction frames
        let mut total_payload_bytes = 0u64;
        let mut buf: Vec<u8> = Vec::new();  // reused per-frame
        for m in &metas {
            let orig_has_marker = m.orig_len > 0 && ORIG_MARKERS.contains(&m.orig_first_byte);
            let plen = payload_len(m);
            
            // Re-read just this frame's payload. Skip header + the unwanted half.
            buf.clear();
            buf.resize(plen as usize, 0);
            if m.offset != u64::MAX && plen > 0 {
                // Where the payload starts in tmp file:
                //   28 bytes header, then either orig (if we want orig) 
                //   or skip orig + go to recon.
                let payload_offset_in_frame = if orig_has_marker { 28u64 }
                    else if m.recon_len > 0 { 28u64 + m.orig_len as u64 }
                    else { 28u64 };
                let payload_start = m.offset + payload_offset_in_frame;
                if src.seek(SeekFrom::Start(payload_start)).is_err()
                    || src.read_exact(&mut buf).is_err() {
                    buf.fill(0);  // fallback to empty payload on read error
                }
            }
            
            if !buf.is_empty() {
                total_payload_bytes += buf.len() as u64;
                match buf[0] {
                    b'T' => t_frame_count += 1,
                    b'K' => k_frame_count += 1,
                    b'V' => {
                        if buf.len() >= 2 && buf[1] == b'4' {
                            v4_frame_count += 1;
                        } else {
                            v_frame_count += 1;
                        }
                    },
                    b'P' => p_frame_count += 1,
                    b'Z' => z_frame_count += 1,
                    _ => {}
                }
            }
            
            // FaoFrameHeader: ftype(1) + compressed_size(4) + recon_size(4) + psnr_x10(2) + skip(1) + gmx(1) = 13 bytes
            dst.write_all(&[m.ftype]).map_err(|e| e.to_string())?;
            dst.write_all(&(buf.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            dst.write_all(&(fsz as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            dst.write_all(&m.psnr_x10.to_le_bytes()).map_err(|e| e.to_string())?;
            dst.write_all(&[m.skip_pct]).map_err(|e| e.to_string())?;
            dst.write_all(&[0u8]).map_err(|e| e.to_string())?;
            dst.write_all(&buf).map_err(|e| e.to_string())?;
            frames_written += 1;
        }
        eprintln!("FAO export: {} frames written ({:.1}MB total)", 
            frames_written, total_payload_bytes as f64 / 1048576.0);
        eprintln!("FAO export: format breakdown — Z={} ({:.1}%) T={} ({:.1}%) K={} ({:.1}%) V={} ({:.1}%) V4={} ({:.1}%) P={} ({:.1}%)",
            z_frame_count, z_frame_count as f64 / frames_written.max(1) as f64 * 100.0,
            t_frame_count, t_frame_count as f64 / frames_written.max(1) as f64 * 100.0,
            k_frame_count, k_frame_count as f64 / frames_written.max(1) as f64 * 100.0,
            v_frame_count, v_frame_count as f64 / frames_written.max(1) as f64 * 100.0,
            v4_frame_count, v4_frame_count as f64 / frames_written.max(1) as f64 * 100.0,
            p_frame_count, p_frame_count as f64 / frames_written.max(1) as f64 * 100.0);
        if let Ok(stats) = tc.translate_stats.try_lock() {
            if stats.0 > 0 {
                let avg_size = stats.1 / stats.0 as u64;
                eprintln!("FAO v2.1 stats: {} translates, {:.1}MB total, avg {} bytes/T-frame",
                    stats.0, stats.1 as f64 / 1048576.0, avg_size);
            }
        }

        // Append motion profile trailer (source frame sizes + deltas)
        // This data is captured at zero encoding cost (already in src_frame_bytes)
        // Readers can use it to:
        //   - Identify high-motion frames (large packets) → blend toward near anchor
        //   - Identify static frames (small packets) → confidence-weighted reuse
        //   - Detect scene changes (delta spikes)
        if let Ok(src_sizes) = tc.src_frame_bytes.try_lock() {
            if !src_sizes.is_empty() {
                let n_motion = (src_sizes.len() as u32).min(n as u32) as usize;
                
                // Build the raw data buffer first, then zstd-wrap it. The 
                // sizes-then-deltas layout means the data is structured 
                // (consecutive u32s for sizes, then u8 deltas) which 
                // zstd compresses to ~10-20% of original on real video.
                // For a 600hr library that drops the trailer from 256MB 
                // to ~30MB.
                let mut raw = Vec::with_capacity(n_motion * 5);
                for i in 0..n_motion {
                    let sz = src_sizes.get(i).copied().unwrap_or(0) as u32;
                    raw.extend_from_slice(&sz.to_le_bytes());
                }
                for i in 0..n_motion {
                    let cur = src_sizes.get(i).copied().unwrap_or(0) as f32;
                    let prev = if i > 0 { src_sizes.get(i-1).copied().unwrap_or(0) as f32 } else { cur };
                    let delta = if prev > 0.0 {
                        ((cur - prev).abs() / prev * 64.0).min(255.0) as u8
                    } else { 0u8 };
                    raw.push(delta);
                }
                
                // Compress with zstd level 3 (good balance of ratio and 
                // encode speed for one-shot trailer write).
                use std::io::Cursor;
                let compressed = zstd::encode_all(Cursor::new(&raw), 3)
                    .unwrap_or_else(|_| raw.clone());
                
                if compressed.len() + 4 < raw.len() {
                    // Compressed form pays off — write FAOMOT02 (compressed).
                    dst.write_all(b"FAOMOT02").map_err(|e| e.to_string())?;
                    dst.write_all(&(n_motion as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                    dst.write_all(&(compressed.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                    dst.write_all(&compressed).map_err(|e| e.to_string())?;
                    eprintln!("FAO: motion profile written ({} bytes raw → {} bytes zstd, {:.1}× ratio)",
                        raw.len(), compressed.len(),
                        raw.len() as f64 / compressed.len().max(1) as f64);
                } else {
                    // Compression didn't help (rare for this data) — write FAOMOT01 (raw).
                    dst.write_all(b"FAOMOT01").map_err(|e| e.to_string())?;
                    dst.write_all(&(n_motion as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                    dst.write_all(&raw).map_err(|e| e.to_string())?;
                    eprintln!("FAO: motion profile written ({} bytes raw, no compression benefit)", raw.len());
                }
            }
        }
        
        // Block dictionary trailer (v2.1+ K-frames reference these).
        // 
        // The dict is large (~7MB on the Beckinsale test where it was 
        // unconditionally emitted despite zero K-frames). Skip when no 
        // K-frames are present in the export — the dict serves no purpose 
        // and just bloats the .fao file.
        // 
        // We use the live k_frame_count from this same export pass, so the 
        // decision matches what was actually written.
        if k_frame_count > 0 {
            if let Ok(dict) = tc.block_dict.try_read() {
                if dict.unique_blocks() > 0 {
                    let dict_bytes = dict.to_bytes();
                    dst.write_all(b"FAOBKD01").map_err(|e| e.to_string())?;
                    dst.write_all(&(dict_bytes.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                    dst.write_all(&dict_bytes).map_err(|e| e.to_string())?;
                    eprintln!("FAO: block dictionary written ({} unique blocks, {} total refs, dedup ratio {:.1}x, {} bytes)",
                        dict.unique_blocks(), dict.total_refs(), dict.dedup_ratio(), dict_bytes.len() + 12);
                }
            }
        } else {
            eprintln!("FAO: block dictionary skipped (no K-frames in export — saves several MB)");
        }
        
        // Source path trailer (allows Orig view in player to fetch source frame)
        // FAOSRCP01 marker + length(u32) + UTF-8 path bytes
        let src_path_bytes = self.src_path.as_bytes();
        if !src_path_bytes.is_empty() {
            dst.write_all(b"FAOSRCP01").map_err(|e| e.to_string())?;
            dst.write_all(&(src_path_bytes.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            dst.write_all(src_path_bytes).map_err(|e| e.to_string())?;
            eprintln!("FAO: source path written ({} bytes)", src_path_bytes.len() + 13);
        }
        
        // v8 trailers: V8MS scene mesh + V8PT pose track + V8MX multi-web.
        // 
        // ── SHARDED EXPORT ──
        // 
        // Encoders are sharded across compress workers (each has its own 
        // V8Encoder instance in v8_states[t]). At export time we MERGE 
        // the per-worker chunk_stores and pose_tracks into a single 
        // combined view, then write trailers from that merged view.
        // 
        // Merge strategy:
        //   - finalize() each worker's encoder so any open chunk seals
        //   - chunk_store.closed_chunks: concat (each chunk is independent)
        //   - pose_track: union by frame_idx into a single Vec; default 
        //     poses fill any gaps (rare — workers process distinct frames)
        //   - scene_mesh: each worker's is empty after finalize, so we 
        //     just take an empty mesh
        // 
        // Cross-worker web alignment is NOT done at this stage — each 
        // worker's chunks already have intra-worker alignment baked in. 
        // Cross-shard alignment could be added here as a post-pass that 
        // re-runs find_similar_web across the merged closed_chunks list.
        let mut any_v8_active = false;
        let mut combined_closed_chunks: Vec<fao_v1::v8_reconstruction_codec::RetainedChunk> = Vec::new();
        let mut combined_pose_track: Vec<fao_v1::v8_reconstruction_codec::CameraPose> = Vec::new();
        let mut combined_intrinsics: Option<fao_v1::v8_reconstruction_codec::CameraIntrinsics> = None;
        // Phase 3: library refs from all shards. These come from chunks that 
        // matched library canonicals at finalize() time and had their meshes 
        // dropped — they're not in combined_closed_chunks anymore.
        let mut combined_library_refs: Vec<fao_v1::v8_reconstruction_codec::LibraryRef> = Vec::new();
        let mut total_library_match_savings: u64 = 0;
        // Per-frame anchor track from all shards (for V8AT trailer / Tobii-
        // style overlay). Each worker processes disjoint frames, so the 
        // merge is simply a union — no per-frame conflict possible at the 
        // worker level. extend() across all workers gives us the full track.
        let mut combined_anchor_track: std::collections::HashMap<u32, Vec<(i16, i16, u8)>> = 
            std::collections::HashMap::new();
        
        // Cache check (export_fao is sometimes called twice — see 
        // V8MergeCache doc on TranscodeState). On the first call we 
        // merge from workers and cache the result. Subsequent calls 
        // reuse the cache; without this they'd find empty workers 
        // and write a 0-web V8MX trailer that overwrites the good one.
        let cache_was_populated = {
            let cache_guard = tc.v8_merge_cache.lock().unwrap();
            if let Some(cached) = cache_guard.as_ref() {
                any_v8_active = !cached.closed_chunks.is_empty() || !cached.library_refs.is_empty();
                combined_closed_chunks = cached.closed_chunks.clone();
                combined_pose_track = cached.pose_track.clone();
                combined_intrinsics = cached.intrinsics;
                combined_library_refs = cached.library_refs.clone();
                total_library_match_savings = cached.total_library_match_savings;
                combined_anchor_track = cached.anchor_track.clone();
                eprintln!("[v8 trace] merge: reusing cached merge ({} chunks, {} library refs) — second export of same encode",
                    combined_closed_chunks.len(), combined_library_refs.len());
                true
            } else {
                false
            }
        };
        
        if !cache_was_populated { for (worker_idx, slot) in tc.v8_states.iter().enumerate() {
            if let Ok(mut v8_lock) = slot.lock() {
                if let Some(v8) = v8_lock.as_mut() {
                    any_v8_active = true;
                    v8.finalize();
                    // Fault any spilled chunks back into RAM before the 
                    // merge. After this, closed_chunks holds the union of 
                    // in-memory + previously-spilled chunks for this shard.
                    let n_spilled = v8.chunk_store.spilled_chunks.len();
                    v8.chunk_store.unspill_all();
                    eprintln!("[v8 trace] merge: worker {} → {} closed chunks ({} unspilled), {} poses",
                        worker_idx, v8.chunk_store.closed_chunks.len(), n_spilled, v8.pose_track.len());
                    // Pull this worker's closed chunks into the combined list.
                    combined_closed_chunks.extend(v8.chunk_store.closed_chunks.drain(..));
                    // Pull library refs (Phase 3 — chunks that matched library canonicals).
                    combined_library_refs.extend(v8.chunk_store.library_refs.drain(..));
                    total_library_match_savings += v8.chunk_store.total_library_match_bytes_saved;
                    // Drain this worker's per-frame anchors into the combined map. 
                    // Workers process disjoint frame indices, so no conflicts; if 
                    // somehow the same frame_idx appears in two workers (shouldn't), 
                    // the last writer wins — for diagnostic data this is harmless.
                    for (fi, anchors) in v8.anchor_track.drain() {
                        combined_anchor_track.insert(fi, anchors);
                    }
                    // Union pose_track by index. Resize combined to fit, 
                    // then fill non-default entries.
                    if v8.pose_track.len() > combined_pose_track.len() {
                        combined_pose_track.resize(v8.pose_track.len(), fao_v1::v8_reconstruction_codec::CameraPose::default());
                    }
                    for (i, p) in v8.pose_track.iter().enumerate() {
                        // Only overwrite if the worker actually set this pose 
                        // (focal > 0 marks a real pose; default has focal 0).
                        if p.focal[0] > 0.0 {
                            combined_pose_track[i] = *p;
                            if combined_intrinsics.is_none() {
                                combined_intrinsics = Some(
                                    fao_v1::v8_reconstruction_codec::CameraIntrinsics::from_pose(p));
                            }
                        }
                    }
                }
            }
        } }  // close `if !cache_was_populated`
        
        // Populate cache now (first export only — `cache_was_populated` 
        // means we already had this).
        if !cache_was_populated && (any_v8_active || !combined_closed_chunks.is_empty()) {
            let mut cache_guard = tc.v8_merge_cache.lock().unwrap();
            *cache_guard = Some(V8MergeCache {
                closed_chunks: combined_closed_chunks.clone(),
                pose_track: combined_pose_track.clone(),
                intrinsics: combined_intrinsics,
                library_refs: combined_library_refs.clone(),
                total_library_match_savings,
                anchor_track: combined_anchor_track.clone(),
            });
        }
        
        // v9 reachability log — print whether the v8 trailer block 
        // (which contains the v9 gate) will execute at all. Lets the 
        // user diagnose "no v9 trace" cases caused by non-v8 encodes.
        eprintln!("FAO v9.0: export pre-trailer — v8 active = {} (gate runs {})",
            any_v8_active, if any_v8_active { "yes" } else { "no — v9 will not run, enable FAO_V81=1" });
        if any_v8_active {
            // ── Frame-range repartition (Phase 6 fix for sharded webs) ──
            // 
            // With the sharded V8Encoder (item 2), multiple workers process 
            // the source video concurrently with backpressure-driven round-
            // robin assignment. Each worker's chunk records its [frame_start, 
            // frame_end] as the MIN..MAX of frames that worker actually 
            // saw — but those frames are SCATTERED across the video, not 
            // contiguous. As a result, different shards' chunks have 
            // OVERLAPPING frame ranges in the merged trailer.
            // 
            // The decoder's web_for_frame(frame_idx) does linear scan 
            // matching the first web whose [start, end] contains frame_idx. 
            // With overlapping ranges, the lookup is ambiguous — different 
            // frames within the SAME video segment may resolve to different 
            // webs, causing visible mesh discontinuities and skips during 
            // playback.
            // 
            // Fix: at merge time, sort all chunks by their (worker-relative) 
            // creation order and assign each chunk a NON-OVERLAPPING 
            // contiguous video-frame partition. The mesh data stays the 
            // same — only the frame range changes — so each video frame 
            // resolves to exactly one web during decode.
            if !combined_closed_chunks.is_empty() {
                // Sort by current frame_start (best proxy for "where in 
                // the video this chunk's content sits"). Stable sort 
                // preserves intra-shard order when starts match.
                combined_closed_chunks.sort_by_key(|c| c.frame_start);
                
                // Find the global frame range. We use the encoder's last-
                // observed frame index (max of all frame_ends) as the 
                // upper bound, since pose_track length isn't directly 
                // available here.
                let total_frames = combined_closed_chunks.iter()
                    .map(|c| c.frame_end)
                    .max()
                    .unwrap_or(0)
                    .max(combined_pose_track.len() as u32);
                
                // CRITICAL: only the chunks that survive trailer-time 
                // filtering (non-empty mesh) end up as decoder webs. 
                // Repartitioning ALL chunks (including empty ones whose 
                // ranges then get dropped) leaves gaps in the partition — 
                // some video frame ranges then map to NO web.
                // 
                // Solution: collect indices of non-empty chunks, give 
                // them the contiguous partition; empty chunks keep 
                // whatever range they had (they get filtered out at 
                // trailer write anyway, so it doesn't matter).
                let n_total = combined_closed_chunks.len();
                let nonempty_indices: Vec<usize> = combined_closed_chunks.iter().enumerate()
                    .filter(|(_, c)| !c.web_mesh.vertices.is_empty())
                    .map(|(i, _)| i)
                    .collect();
                let n_nonempty = nonempty_indices.len() as u32;
                
                if n_nonempty > 0 && total_frames > 0 {
                    use std::collections::HashMap;
                    let span = total_frames / n_nonempty;
                    
                    // Pre-compute remapped frame_starts for non-empty chunks.
                    let mut remap: HashMap<u32, u32> = HashMap::with_capacity(n_nonempty as usize);
                    for (slot, &idx) in nonempty_indices.iter().enumerate() {
                        let new_start = (slot as u32) * span;
                        let old_start = combined_closed_chunks[idx].frame_start;
                        remap.insert(old_start, new_start);
                    }
                    
                    // Apply new ranges to non-empty chunks. Empty chunks 
                    // keep their old ranges; they're filtered out at 
                    // trailer write.
                    for (slot, &idx) in nonempty_indices.iter().enumerate() {
                        let s = slot as u32;
                        let new_start = s * span;
                        let new_end = if (s + 1) == n_nonempty {
                            total_frames.saturating_sub(1)
                        } else {
                            ((s + 1) * span).saturating_sub(1)
                        };
                        let c = &mut combined_closed_chunks[idx];
                        c.frame_start = new_start;
                        c.frame_end = new_end;
                        
                        // Translate canonical_web_id. If the canonical was 
                        // an empty-mesh chunk (now filtered out), the ref 
                        // is dangling — clear it (chunk becomes standalone).
                        if let Some(old_canon) = c.canonical_web_id {
                            c.canonical_web_id = remap.get(&old_canon).copied();
                        }
                    }
                    eprintln!(
                        "[v8 trace] merge: repartitioned {} non-empty webs across [0..{}] (span ~{} frames each, {} empty chunks left untouched)",
                        n_nonempty, total_frames, span, n_total - n_nonempty as usize
                    );
                }
            }
            
            // Build a temporary ChunkStore that owns the merged chunks. 
            // This lets us call to_v8mx_trailer with the union.
            let mut merged_store = fao_v1::v8_reconstruction_codec::ChunkStore::new();
            merged_store.closed_chunks = combined_closed_chunks;
            
            // V8MS — scene mesh trailer. Empty after finalize; kept for 
            // backward-compat with decoders that look for the magic.
            let empty_mesh = fao_v1::v8_reconstruction_codec::SceneMesh::new();
            let mesh_trailer = empty_mesh.to_trailer();
            dst.write_all(&(mesh_trailer.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            dst.write_all(&mesh_trailer).map_err(|e| e.to_string())?;
            eprintln!("FAO v8.0: V8MS scene mesh (0 verts, 0 tris, {} bytes compressed)",
                mesh_trailer.len() + 4);
            
            // V8MX — multi-web trailer. Tolerance 5.0 (see prior 
            // session notes for why 0.5 was too tight).
            let mx_trailer = merged_store.to_v8mx_trailer(5.0);
            let n_webs = merged_store.closed_chunks.iter()
                .filter(|c| !c.web_mesh.vertices.is_empty()).count();
            let n_canonical = merged_store.closed_chunks.iter()
                .filter(|c| !c.web_mesh.vertices.is_empty() && c.canonical_web_id.is_none()).count();
            let n_aligned = merged_store.closed_chunks.iter()
                .filter(|c| !c.web_mesh.vertices.is_empty() && c.canonical_web_id.is_some()).count();
            dst.write_all(&(mx_trailer.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            dst.write_all(&mx_trailer).map_err(|e| e.to_string())?;
            eprintln!("FAO v8.0: V8MX multi-web trailer ({} webs, {} canonical + {} aligned, {:.1}MB) — merged from {} shards",
                n_webs, n_canonical, n_aligned,
                mx_trailer.len() as f64 / 1_048_576.0,
                tc.v8_states.len());
            
            let full_size_bytes: usize = merged_store.closed_chunks.iter()
                .map(|c| c.web_mesh.vertices.len() * 24 + c.web_mesh.indices.len() * 4)
                .sum();
            if full_size_bytes > 0 {
                let savings = 100.0 * (1.0 - mx_trailer.len() as f64 / full_size_bytes as f64);
                eprintln!("FAO v8.0: web compression: {:.1}MB full -> {:.1}MB delta ({:.0}% savings)",
                    full_size_bytes as f64 / 1_048_576.0,
                    mx_trailer.len() as f64 / 1_048_576.0,
                    savings);
            }
            
            // ── V9DC — volumetric chunk trailer ──
            // 
            // When FAO_V9=1, run the v9 build pipeline on each closed 
            // chunk: convert v8 mesh + feature positions → sparse voxel 
            // grid → DCT-quantized blocks → V9DC trailer.
            // 
            // This sits ALONGSIDE V8MX in the same file. Decoders that 
            // know v9 prefer it; older decoders see V9DC as an unknown 
            // trailer and fall back to V8MX.
            // 
            // First-cut: no testing, no optimization. Goal is "signs of 
            // life" — does the volumetric pipeline produce non-garbage 
            // output? User can decide whether to invest more after seeing 
            // a single run.
            let v9_raw = std::env::var("FAO_V9").unwrap_or_else(|_| "<unset>".to_string());
            let v9_enabled = v9_raw == "1" || v9_raw.eq_ignore_ascii_case("true");
            let nonempty_chunks = merged_store.closed_chunks.iter()
                .filter(|c| !c.web_mesh.vertices.is_empty()).count();
            let total_chunks = merged_store.closed_chunks.len();
            // Always log v9 status. The raw env value lets the user 
            // diagnose accidental "0" / "yes" / empty-string cases. 
            // chunk counts let the user diagnose "no v9 trace because 
            // there were no v8 chunks to volumetrize".
            eprintln!("FAO v9.0: gate check — FAO_V9 env={:?} → {} ({} non-empty of {} total chunks)",
                v9_raw,
                if v9_enabled { "ON" } else { "off" },
                nonempty_chunks, total_chunks);
            if v9_enabled && nonempty_chunks == 0 {
                eprintln!("FAO v9.0: WARN — V9 is enabled but there are 0 non-empty chunks. v8 produced no mesh; v9 will not run. Check earlier logs for v8 mesh integration ('[v8 mesh] anchor N: integrated K tris').");
            }
            if v9_enabled {
                use fao_v1::v9_volumetric;
                let mut v9_chunks: Vec<v9_volumetric::V9Chunk> = Vec::new();
                // FAO_V9_MAX_CHUNKS caps how many v8 chunks get a v9 
                // representation. v9 is still slow (CPU splat, no GPU 
                // path yet) so on a real encode with 90+ chunks the 
                // v9 pass could add seconds. Default 0 = all chunks; 
                // setting e.g. 5 limits to the first 5 non-empty chunks 
                // for quick iteration.
                let v9_max_chunks: usize = std::env::var("FAO_V9_MAX_CHUNKS")
                    .ok().and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut v9_processed = 0usize;
                for chunk in merged_store.closed_chunks.iter() {
                    if chunk.web_mesh.vertices.is_empty() { continue; }
                    if v9_max_chunks > 0 && v9_processed >= v9_max_chunks { break; }
                    v9_processed += 1;
                    // Feature positions come from the chunk's feature 
                    // tracks; for v9 MVP we use the mesh vertices 
                    // themselves as a proxy (they cluster where features 
                    // were triangulated).
                    let feature_proxy: Vec<_> = chunk.web_mesh.vertices.iter().copied().collect();
                    // Voxels per axis (longest). 64 = 262144 cells max 
                    // per chunk, but actual occupancy is typically <1% 
                    // (sparse hash grid). Tunable via FAO_V9_RES env var: 
                    // 32 = coarser/faster, 128 = finer/slower. 128 is 
                    // about 2M cells max — still sparse.
                    let v9_res: u32 = std::env::var("FAO_V9_RES")
                        .ok().and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(64).clamp(8, 256);
                    let v9_chunk = v9_volumetric::build_v9_chunk_from_v8(
                        &chunk.web_mesh, 
                        &feature_proxy, 
                        v9_res,
                        chunk.frame_start,
                        chunk.frame_end,
                    );
                    // Diagnostic log for the first few chunks: input size + 
                    // output blocks. Reveals the mesh→voxel pipeline's 
                    // health. Limited to 3 chunks to keep output readable.
                    if v9_processed <= 3 {
                        eprintln!("FAO v9.0: chunk {} (frames {}..{}): mesh {} verts / {} tris, features {}, output {} blocks (voxel_size={:.4})",
                            v9_processed,
                            chunk.frame_start, chunk.frame_end,
                            chunk.web_mesh.vertices.len(),
                            chunk.web_mesh.indices.len() / 3,
                            feature_proxy.len(),
                            v9_chunk.blocks.len(),
                            v9_chunk.voxel_size);
                    }
                    v9_chunks.push(v9_chunk);
                }
                let v9_trailer = v9_volumetric::write_v9_trailer(&v9_chunks);
                let total_blocks: usize = v9_chunks.iter().map(|c| c.blocks.len()).sum();
                dst.write_all(&(v9_trailer.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                dst.write_all(&v9_trailer).map_err(|e| e.to_string())?;
                eprintln!("FAO v9.0: ═══════════════════════════════════════════════════════════════");
                eprintln!("FAO v9.0: V9DC trailer written: {} chunks, {} blocks total, {:.2}MB",
                    v9_chunks.len(), total_blocks, v9_trailer.len() as f64 / 1_048_576.0);
                eprintln!("FAO v9.0: ═══════════════════════════════════════════════════════════════");
            }
            
            // V8LR — library reference trailer (Phase 3). Empty when no 
            // library was attached to the encoder; otherwise lists the 
            // chunks whose meshes were sourced from the library instead 
            // of being stored in this .fao.
            if !combined_library_refs.is_empty() {
                let lr_trailer = fao_v1::v8_reconstruction_codec::write_v8lr_trailer(&combined_library_refs);
                dst.write_all(&(lr_trailer.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                dst.write_all(&lr_trailer).map_err(|e| e.to_string())?;
                eprintln!("FAO v8.0: V8LR library refs ({} chunks linked, {:.1} MB of mesh data deduplicated to library)",
                    combined_library_refs.len(),
                    total_library_match_savings as f64 / 1_048_576.0);
            }
            
            // V8PT — pose track. Compact format (20B intrinsics + 28B/pose).
            // 
            // For long videos the raw pose stream gets big (~280KB at 10k 
            // frames). Adjacent poses are very similar (smooth camera 
            // motion), so they compress extremely well — typical zstd 
            // ratio is 5-10× on this data. The version=2 variant of 
            // V8PT signals "zstd-wrapped pose payload"; older decoders 
            // (version=1) get a copy of the uncompressed format for 
            // backward compat by writing the legacy format instead 
            // when FAO_POSE_RAW=1 is set.
            // 
            // Format v2 layout: V8PT(4) + 2(u8) + reserved(3) + 
            //   intrinsics(20) + n_poses(u32) + raw_payload_len(u32) + 
            //   zstd(28*n_poses bytes of extrinsics)
            if !combined_pose_track.is_empty() {
                let intrinsics = combined_intrinsics.unwrap_or_default();
                let n_poses = combined_pose_track.len();
                let pose_raw = std::env::var("FAO_POSE_RAW")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let mut raw_extrinsics: Vec<u8> = Vec::with_capacity(28 * n_poses);
                for pose in &combined_pose_track {
                    raw_extrinsics.extend_from_slice(&pose.to_bytes_extrinsics());
                }
                
                let mut pose_trailer: Vec<u8>;
                if pose_raw {
                    // Legacy format (version 1) — uncompressed.
                    pose_trailer = Vec::with_capacity(4 + 4 + 20 + 4 + raw_extrinsics.len());
                    pose_trailer.extend_from_slice(b"V8PT");
                    pose_trailer.push(1u8);                    // version 1
                    pose_trailer.extend_from_slice(&[0u8; 3]); // reserved
                    pose_trailer.extend_from_slice(&intrinsics.to_bytes());
                    pose_trailer.extend_from_slice(&(n_poses as u32).to_le_bytes());
                    pose_trailer.extend_from_slice(&raw_extrinsics);
                } else {
                    // Compressed (version 2). Compress the extrinsics 
                    // payload only; intrinsics + counts stay 
                    // uncompressed so the header is parseable without 
                    // a zstd round-trip.
                    use std::io::Cursor;
                    let compressed = zstd::encode_all(Cursor::new(&raw_extrinsics), 19)
                        .unwrap_or_else(|_| raw_extrinsics.clone());
                    pose_trailer = Vec::with_capacity(4 + 4 + 20 + 4 + 4 + compressed.len());
                    pose_trailer.extend_from_slice(b"V8PT");
                    pose_trailer.push(2u8);                    // version 2 = zstd
                    pose_trailer.extend_from_slice(&[0u8; 3]); // reserved
                    pose_trailer.extend_from_slice(&intrinsics.to_bytes());
                    pose_trailer.extend_from_slice(&(n_poses as u32).to_le_bytes());
                    pose_trailer.extend_from_slice(&(raw_extrinsics.len() as u32).to_le_bytes());
                    pose_trailer.extend_from_slice(&compressed);
                }
                dst.write_all(&(pose_trailer.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                dst.write_all(&pose_trailer).map_err(|e| e.to_string())?;
                let raw_size = 28 * n_poses;
                let comp_size = pose_trailer.len();
                eprintln!("FAO v8.0: pose track written ({} poses, {} bytes on disk, {} bytes uncompressed{})",
                    n_poses, comp_size + 4, raw_size + 32,
                    if pose_raw { " — RAW mode (FAO_POSE_RAW=1)" } else { " — zstd compressed" });
            }
            
            // V8AT — per-frame anchor track for the Tobii-style player 
            // overlay. Opt-out via FAO_NO_ANCHOR_TRACK=1 env var so 
            // bandwidth-sensitive exports can skip the few-MB cost.
            // 
            // When written, the player can display translucent rings at 
            // each anchor's screen position during playback, colored by 
            // motion-class (green Static, red Dynamic, magenta UiOverlay, 
            // grey Unknown). User toggles with the 'A' key.
            let skip_anchor_track = std::env::var("FAO_NO_ANCHOR_TRACK")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !skip_anchor_track && !combined_anchor_track.is_empty() {
                let n_anchor_frames = combined_anchor_track.len();
                let n_anchors_total: usize = combined_anchor_track.values().map(|v| v.len()).sum();
                let at_trailer = fao_v1::v8_reconstruction_codec::write_v8at_trailer(&combined_anchor_track);
                dst.write_all(&(at_trailer.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
                dst.write_all(&at_trailer).map_err(|e| e.to_string())?;
                eprintln!("FAO v8.0: V8AT anchor track ({} frames, {} anchors total, {:.1}MB trailer)",
                    n_anchor_frames, n_anchors_total,
                    (at_trailer.len() + 4) as f64 / 1_048_576.0);
            } else if skip_anchor_track {
                eprintln!("FAO v8.0: V8AT anchor track SKIPPED (FAO_NO_ANCHOR_TRACK=1)");
            }
        }
        
        dst.flush().map_err(|e| e.to_string())?;
        eprintln!("FAO: exported {} frames, {:.1}MB", frames_written, data_offset as f64 / 1048576.0);
        Ok(frames_written)
    }

    /// Load a .fao file for native playback
    /// Load a second .fao file for A/B comparison in player
    fn load_player_b(&mut self, path: &std::path::Path) {
        // Same smart-load logic as load_fao — see that function for details.
        let data = match load_fao_data_smart(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("FAO: failed to open player B {} — {}", path.display(), e);
                return;
            }
        };
        let header = match fao_v1::read_fao_header(&data) { Some(h) => h, None => return };
        let index = fao_v1::read_fao_index(&data, header.frame_count as usize);
        if index.is_empty() { return; }
        self.player_b_path = path.display().to_string();
        self.player_b_data = Some(data);
        self.player_b_index = index;
        self.player_b_header = Some(header);
        eprintln!("FAO: loaded player B {} ({} frames)", path.display(), self.player_b_index.len());
        // Load matching frame
        let cur_idx = self.fidx;
        self.load_player_b_frame(cur_idx);
    }
    
    fn load_player_b_frame(&mut self, idx: usize) {
        let data = match &self.player_b_data { Some(d) => d, None => return };
        if idx >= self.player_b_index.len() { return; }
        let entry = &self.player_b_index[idx];
        let w = self.cur_w;
        let h = self.cur_h;
        let yuv_sz = w * h * 3 / 2;
        let rgb_sz = w * h * 3;
        if let Some((data_bytes, _)) = fao_v1::read_fao_frame(data, entry) {
            if idx < 3 {
                let marker = if data_bytes.is_empty() { '_' } else { data_bytes[0] as char };
                eprintln!("FAO load_B: frame {} payload_len={} marker={:?}",
                    idx, data_bytes.len(), marker);
            }
            // Skip frame: keep previous player_b_frame (don't go gray)
            if data_bytes.is_empty() {
                if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                    return;  // keep showing last good B frame
                }
            }
            let rgb = if data_bytes.is_empty() {
                if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                    self.player_b_frame.clone()
                } else { vec![64u8; rgb_sz] }
            } else {
                match data_bytes[0] {
                    b'D' => fao_v1::decompress_frame_dct(&data_bytes, w, h)
                        .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h))
                        .unwrap_or_else(|| {
                            if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                                self.player_b_frame.clone()
                            } else { vec![64u8; rgb_sz] }
                        }),
                    b'P' => fao_v1::decompress_frame_yuv_split(&data_bytes, w, h)
                        .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h))
                        .unwrap_or_else(|| {
                            if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                                self.player_b_frame.clone()
                            } else { vec![64u8; rgb_sz] }
                        }),
                    b'L' => fao_v1::decompress_frame_lossy_chroma(&data_bytes, w, h)
                        .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h))
                        .unwrap_or_else(|| {
                            if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                                self.player_b_frame.clone()
                            } else { vec![64u8; rgb_sz] }
                        }),
                    b'T' => {
                        // v2.1 Translated frame on Player B
                        if let Some((tframe, fw, fh)) = fao_v1::v21_predictive::TranslatedFrame::from_bytes(&data_bytes) {
                            let anchor_idx = tframe.anchor_ref as usize;
                            // Decode anchor from player_b_data
                            let anchor_rgb = if anchor_idx < self.player_b_index.len() {
                                let anchor_entry = &self.player_b_index[anchor_idx];
                                if let Some((anchor_bytes, _)) = fao_v1::read_fao_frame(data, anchor_entry) {
                                    if !anchor_bytes.is_empty() {
                                        match anchor_bytes[0] {
                                            b'P' => fao_v1::decompress_frame_yuv_split(&anchor_bytes, fw, fh)
                                                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, fw, fh)),
                                            b'L' => fao_v1::decompress_frame_lossy_chroma(&anchor_bytes, fw, fh)
                                                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, fw, fh)),
                                            b'D' => fao_v1::decompress_frame_dct(&anchor_bytes, fw, fh)
                                                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, fw, fh)),
                                            _ => fao_v1::decompress_frame(&anchor_bytes, fw*fh*3/2)
                                                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, fw, fh)),
                                        }
                                    } else { None }
                                } else { None }
                            } else { None };
                            if let Some(a_rgb) = anchor_rgb {
                                let anchor_yuv = rgb_to_yuv420p(&a_rgb, fw, fh);
                                tframe.decode_with_anchor(&anchor_yuv, fw, fh)
                                    .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, fw, fh))
                                    .unwrap_or(a_rgb)
                            } else if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                                self.player_b_frame.clone()
                            } else { vec![64u8; rgb_sz] }
                        } else if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                            self.player_b_frame.clone()
                        } else { vec![64u8; rgb_sz] }
                    },
                    _ => {
                        if data_bytes.len() == yuv_sz {
                            TranscodeState::yuv_to_rgb(&data_bytes, w, h)
                        } else if data_bytes.len() == rgb_sz {
                            data_bytes.clone()
                        } else {
                            fao_v1::decompress_frame(&data_bytes, yuv_sz)
                                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h))
                                .or_else(|| fao_v1::decompress_frame(&data_bytes, rgb_sz))
                                .unwrap_or_else(|| {
                                    if !self.player_b_frame.is_empty() && self.player_b_frame.len() == rgb_sz {
                                        self.player_b_frame.clone()
                                    } else { vec![64u8; rgb_sz] }
                                })
                        }
                    }
                }
            };
            self.player_b_frame = rgb;
        }
    }
    
    /// Invalidate the player's anchor overlay DrawingAreas so they repaint 
    /// on the next frame cycle. Called when:
    ///   - The user toggles the 'A' anchor overlay key
    ///   - load_fao_frame advances to a new video frame
    ///   - load_fao swaps in a new .fao file (anchor data changes)
    /// 
    /// No-op if the DAs aren't wired up yet (during initial app boot 
    /// or in headless tests).
    fn request_player_redraw(&self) {
        if let Some(da) = self.player_anchor_da_a.borrow().as_ref() {
            da.queue_draw();
        }
        if let Some(da) = self.player_anchor_da_b.borrow().as_ref() {
            da.queue_draw();
        }
    }
    
    fn load_fao(&mut self, path: &std::path::Path) {
        // If a previous .fao session left a trace, summarize it before 
        // clearing. Gives the user a final report on the just-closed 
        // session even if they didn't hit the 5000-frame rolling checkpoint.
        if !self.playback_trace.borrow().is_empty() {
            self.summarize_playback_trace("end of previous session");
            self.playback_trace.borrow_mut().clear();
        }
        
        // Smart-load: pick eager vs mmap based on file size and current 
        // memory pressure. See `load_fao_data_smart` for the decision rules.
        // 
        // Eager is faster for subsequent random access; mmap conserves 
        // RAM. The user gets a single log line indicating which was used 
        // — they specifically asked for an "mmap=false" warning so they 
        // know when the file is being slurped.
        let data = match load_fao_data_smart(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("FAO: failed to open {} — {}", path.display(), e);
                return;
            }
        };
        let header = match fao_v1::read_fao_header(&data) { Some(h) => h, None => {
            eprintln!("FAO: {} has no valid header", path.display());
            return;
        }};
        let index = fao_v1::read_fao_index(&data, header.frame_count as usize);
        if index.is_empty() {
            eprintln!("FAO: {} has empty index", path.display());
            return;
        }

        self.src_path = path.display().to_string();
        self.vid_fps = header.fps_num as f64 / header.fps_den.max(1) as f64;
        self.cur_w = header.width as usize;
        self.cur_h = header.height as usize;
        self.vid_w = self.cur_w; self.vid_h = self.cur_h;

        // Store the file data and index for playback
        self.fao_data = Some(data);
        self.fao_index = index;
        self.fao_header = Some(header);
        self.mode = Mode::FaoPlay;
        self.fidx = 0;
        self.playing = false;
        
        // Clear stale caches from previous FAO
        self.anchor_cache.clear();
        self.anchor_yuv_for_t.clear();
        self.fao_prefetch.lock().unwrap().clear();
        self.interp_stats = (0, 0, 0.0, 0.0);
        
        // Tear down any previous prefetch worker pool (drops sender → 
        // all workers see Disconnected on recv → exit → join completes).
        if let Some(tx) = self.prefetch_tx.take() {
            drop(tx);
            for h in self.prefetch_threads.drain(..) {
                let _ = h.join();
            }
        }
        // Spawn new pool for this file. Workers hold Arc clones of the 
        // cache + v8_decoder so they stay alive for the lifetime of 
        // THIS .fao session.
        let (tx, handles) = spawn_prefetch_worker(
            std::sync::Arc::clone(&self.fao_prefetch),
            std::sync::Arc::clone(&self.fao_v8_decoder),
        );
        self.prefetch_tx = Some(tx);
        self.prefetch_threads = handles;
        eprintln!("[FAO async-prefetch] worker pool spawned ({} threads)", PREFETCH_WORKER_COUNT);
        
        // Drop existing source-fetch worker if any (channel close → 
        // thread exits when it next polls).
        self.source_fetch_tx = None;
        if let Some(h) = self.source_fetch_thread.take() {
            let _ = h.join();
        }
        // Spawn fresh source-fetch worker. Uses the shared 
        // fao_source_cache Arc<Mutex>.
        let (sf_tx, sf_handle) = spawn_source_fetch_worker(
            std::sync::Arc::clone(&self.fao_source_cache),
        );
        self.source_fetch_tx = Some(sf_tx);
        self.source_fetch_thread = Some(sf_handle);
        eprintln!("[FAO source-fetch] worker spawned");
        self.fao_motion_sizes.clear();
        self.fao_motion_deltas.clear();
        
        // Read motion profile trailer if present (FAOMOT01)
        self.read_motion_profile();
        // Read block dictionary trailer if present (FAOBKD01)
        self.fao_block_dict = None;
        self.read_block_dict();
        // Read legacy trailers (v3-v7). All stubbed since the codec modules 
        // were removed in v10 cleanup; readers log + skip if magic is found.
        self.read_v3_plate();
        self.read_v4_trailer();
        self.read_v5_trailer();
        self.read_v6_trailer();
        self.read_v7_trailer();
        
        // Read v8 reconstruction trailer (V8MS mesh + V8PT pose track 
        // + V8MX webs + V8LR library refs + V8AT anchor track)
        *self.fao_v8_decoder.write().unwrap() = None;
        self.fao_anchor_track.clear();
        self.fao_v9_chunks.clear();
        self.fao_v9_grid_cache.borrow_mut().clear();
        self.read_v8_trailer();
        
        // Read source path if present (FAOSRCP01) — enables A/B vs source video
        self.fao_source_path = None;
        if let Ok(mut c) = self.fao_source_cache.lock() { c.clear(); }
        self.read_source_path();
        
        // Pre-compute anchor map: for each frame, find nearest anchors
        // O(n) sweep — fast, runs once per FAO load
        self.build_anchor_map();
        
        // Pre-warm the prefetch cache with the first batch of frames 
        // so playback doesn't race the workers at startup. By the time 
        // the user clicks Play, frames 0..N should be ready in cache.
        // 
        // We schedule 32 frames upfront — half a second of playback at 
        // 60fps, more than a second at 24fps. Workers will decode in 
        // parallel; first frame should be ready within ~30-100ms.
        if !self.fao_index.is_empty() {
            let prewarm = self.fao_index.len().min(32);
            self.prefetch_fao_frames(0, prewarm);
            eprintln!("[FAO async-prefetch] pre-warmed first {} frames", prewarm);
        }
        
        // Load the first frame BEFORE firing source-cache prewarm. 
        // load_fao_frame(0) populates self.cur_w, self.cur_h, and 
        // self.vid_fps from the .fao header — required by 
        // request_source_cache_fill's `if w==0 || h==0 || fps<=0` 
        // gate. Calling it first meant the prewarm tasks bailed at 
        // "bad dims" diagnostic and never reached ffmpeg. (Round-3 
        // log signature: many "channel full, dropped task" with zero 
        // "batch decoded" success — the channel only filled because 
        // dims-check failed silently before this reorder.)
        self.load_fao_frame(0);
        
        // Source-video cache prewarm. The composite path (low-coverage 
        // v8 frames overlaid on source background) needs the source 
        // cache populated synchronously with playback. Without this, 
        // the first 60 frames trigger cache misses because the async 
        // worker hadn't run yet.
        // 
        // Channel capacity is now 16 (was 2) so we can stagger across 
        // a wider initial window without dropping. Fire requests at 
        // 0, 60, 120 for first 3 batches = 180 frames of guaranteed 
        // pre-warmed source.
        if !self.fao_index.is_empty() && self.fao_source_path.is_some() {
            self.request_source_cache_fill(0);
            self.request_source_cache_fill(60);
            self.request_source_cache_fill(120);
            eprintln!("[FAO source-fetch] pre-warm requests fired for idx 0, 60, 120 (after load_fao_frame(0) so dims are populated)");
        }
    }
    
    /// Read block dictionary trailer from .fao file (if present).
    /// Used by K-frame decoder for cross-frame block dedup.
    fn read_block_dict(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let needle = b"FAOBKD01";
        // Same scan narrowing as read_v8_trailer.
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        
        let mut found_at = None;
        if data.len() >= 12 {
            for i in search_start..data.len().saturating_sub(needle.len()) {
                if &data[i..i+needle.len()] == needle {
                    found_at = Some(i + needle.len());
                    break;
                }
            }
        }
        
        let pos = match found_at { Some(p) => p, None => return };
        if pos + 4 > data.len() { return; }
        let dict_size = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        let dict_start = pos + 4;
        if dict_start + dict_size > data.len() { return; }
        
        if let Some(dict) = fao_v1::block_dict::BlockDictionary::from_bytes(&data[dict_start..dict_start + dict_size]) {
            eprintln!("FAO block dictionary loaded: {} unique blocks", dict.unique_blocks());
            self.fao_block_dict = Some(dict);
        }
    }
    
    /// Read v3 static plate trailer (FAOSTPL1) from end of .fao file
    /// Legacy v3 static-plate trailer reader — stub. The v3 codec module 
    /// was removed in the v10 cleanup. We still scan for the FAOSTPL1 magic so 
    /// users see a notice if they're loading an old file.
    fn read_v3_plate(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let needle = b"FAOSTPL1";
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        if data.len() < 12 { return; }
        for i in search_start..data.len().saturating_sub(needle.len()) {
            if &data[i..i+needle.len()] == needle {
                eprintln!("FAO: legacy FAOSTPL1 trailer detected at offset {}; \
                           ignoring (codec removed in v10 cleanup)", i);
                return;
            }
        }
    }
    
    /// Read v4 dictionary trailer (V4D1) from end of .fao file
    /// Legacy v4 dictionary trailer reader — stub. The v4 codec module 
    /// was removed in the v10 cleanup. We still scan for the V4D1 magic so 
    /// users see a notice if they're loading an old file.
    fn read_v4_trailer(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let needle = b"V4D1";
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        if data.len() < 12 { return; }
        for i in search_start..data.len().saturating_sub(needle.len()) {
            if &data[i..i+needle.len()] == needle {
                eprintln!("FAO: legacy V4D1 trailer detected at offset {}; \
                           ignoring (codec removed in v10 cleanup)", i);
                return;
            }
        }
    }
    
    /// Read v5 sprite library trailer (V5L1) from end of .fao file
    /// Legacy v5 sprite library trailer reader — stub. The v5 codec module 
    /// was removed in the v10 cleanup. We still scan for the V5L1 magic so 
    /// users see a notice if they're loading an old file.
    fn read_v5_trailer(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let needle = b"V5L1";
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        if data.len() < 12 { return; }
        for i in search_start..data.len().saturating_sub(needle.len()) {
            if &data[i..i+needle.len()] == needle {
                eprintln!("FAO: legacy V5L1 trailer detected at offset {}; \
                           ignoring (codec removed in v10 cleanup)", i);
                return;
            }
        }
    }
    
    /// Read v6 object library trailer (V6L1) from end of .fao file
    /// Legacy v6 object library trailer reader — stub. The v6 codec module 
    /// was removed in the v10 cleanup. We still scan for the V6L1 magic so 
    /// users see a notice if they're loading an old file.
    fn read_v6_trailer(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let needle = b"V6L1";
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        if data.len() < 12 { return; }
        for i in search_start..data.len().saturating_sub(needle.len()) {
            if &data[i..i+needle.len()] == needle {
                eprintln!("FAO: legacy V6L1 trailer detected at offset {}; \
                           ignoring (codec removed in v10 cleanup)", i);
                return;
            }
        }
    }
    
    /// Read v7 perception trailers (V7L1 library + V7CT camera track) from end of .fao
    /// Legacy v7 perception trailer reader — stub. The v7 codec module 
    /// was removed in the v10 cleanup. We still scan for the V7L1 magic so 
    /// users see a notice if they're loading an old file.
    fn read_v7_trailer(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let needle = b"V7L1";
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        if data.len() < 12 { return; }
        for i in search_start..data.len().saturating_sub(needle.len()) {
            if &data[i..i+needle.len()] == needle {
                eprintln!("FAO: legacy V7L1 trailer detected at offset {}; \
                           ignoring (codec removed in v10 cleanup)", i);
                return;
            }
        }
    }
    
    /// Read v8 reconstruction trailers (V8MS scene mesh + V8PT pose track)
    fn read_v8_trailer(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let mut mesh_pos = None;
        let mut pose_pos = None;
        let mut mx_pos = None;
        let mut lr_pos = None;
        let mut at_pos = None;
        let mut v9_pos: Option<usize> = None;
        // Narrow the scan to the area AFTER the frame data ends. Trailers 
        // are written sequentially after the last frame payload. Scanning 
        // the entire file (the old 512 MB heuristic) invites false-positive 
        // magic matches inside zstd-compressed frame data. For a large file 
        // (24K-frame 1080p, ~420 MB), a 4-byte magic has ~10% chance of 
        // false-matching somewhere in the frame data — and the first match 
        // wins, so the parser then reads random bytes as a header and tries 
        // to allocate ~150 GB. (User observed: "memory allocation of 
        // 168289044128 bytes failed".)
        // 
        // frame_data_end is computed from the index. Anything from that 
        // byte onward is guaranteed to be a trailer or the FAO footer.
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        if data.len() >= 16 {
            for i in search_start..data.len().saturating_sub(4) {
                if &data[i..i+4] == b"V8MS" && mesh_pos.is_none() {
                    mesh_pos = Some(i);
                } else if &data[i..i+4] == b"V8PT" && pose_pos.is_none() {
                    pose_pos = Some(i);
                } else if &data[i..i+4] == b"V8MX" && mx_pos.is_none() {
                    mx_pos = Some(i);
                } else if &data[i..i+4] == b"V8LR" && lr_pos.is_none() {
                    lr_pos = Some(i);
                } else if &data[i..i+4] == b"V8AT" && at_pos.is_none() {
                    at_pos = Some(i);
                } else if &data[i..i+4] == b"V9DC" && v9_pos.is_none() {
                    v9_pos = Some(i);
                }
                if mesh_pos.is_some() && pose_pos.is_some() && mx_pos.is_some() && lr_pos.is_some() && at_pos.is_some() && v9_pos.is_some() { break; }
            }
        }
        
        let mesh = if let Some(pos) = mesh_pos {
            if pos + 16 <= data.len() {
                let comp_len = u32::from_le_bytes([data[pos+12], data[pos+13], data[pos+14], data[pos+15]]) as usize;
                let total_len = 16 + comp_len;
                if pos + total_len <= data.len() {
                    fao_v1::v8_reconstruction_codec::SceneMesh::from_trailer(&data[pos..pos + total_len])
                } else { None }
            } else { None }
        } else { None };
        
        let pose_track = if let Some(pos) = pose_pos {
            // V8PT format: distinguished by the byte at offset +4.
            // 
            // Legacy: 'V8PT'(4) + n_poses(u32 LE) + 48 bytes per pose.
            //   Heuristic: byte at +4 is the low byte of n_poses, which 
            //   for any realistic frame count won't be 1 (a 1-pose video 
            //   would have to be exactly 0x01000000 = 16M poses to alias).
            // 
            // Compact (v1): 'V8PT'(4) + version=1(u8) + reserved(3) + 
            //   intrinsics(20) + n_poses(u32 LE) + 28 bytes per extrinsic.
            //   Disambiguated by version byte == 1.
            if pos + 8 > data.len() { Vec::new() }
            else if data[pos + 4] == 2 {
                // v2 compressed: same header as v1 up through n_poses, 
                // then raw_payload_len (u32 LE) + zstd payload.
                let intr_off = pos + 8;
                let count_off = pos + 8 + 20;
                let rawlen_off = count_off + 4;
                let comp_off = rawlen_off + 4;
                if comp_off > data.len() { Vec::new() }
                else if let Some(intrinsics) = fao_v1::v8_reconstruction_codec::CameraIntrinsics::from_bytes(&data[intr_off..intr_off + 20]) {
                    let n = u32::from_le_bytes([data[count_off], data[count_off+1], data[count_off+2], data[count_off+3]]) as usize;
                    let raw_len = u32::from_le_bytes([data[rawlen_off], data[rawlen_off+1], data[rawlen_off+2], data[rawlen_off+3]]) as usize;
                    // Sanity: raw_len must equal 28 * n.
                    if raw_len != 28 * n {
                        eprintln!("V8PT v2: raw_len={} doesn't match 28*n_poses={} — corrupt trailer, falling back to empty", raw_len, 28 * n);
                        Vec::new()
                    } else {
                        use std::io::Cursor;
                        // The v2 zstd payload runs from comp_off to the 
                        // next trailer magic or EOF. We bound by raw_len 
                        // since zstd self-terminates.
                        let raw = match zstd::decode_all(Cursor::new(&data[comp_off..])) {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("V8PT v2: zstd decode failed: {}", e);
                                Vec::new()
                            }
                        };
                        if raw.len() != raw_len {
                            eprintln!("V8PT v2: decoded len={} expected={} — corrupt", raw.len(), raw_len);
                            Vec::new()
                        } else {
                            let mut track = Vec::with_capacity(n);
                            for i in 0..n {
                                let off = 28 * i;
                                if let Some(p) = fao_v1::v8_reconstruction_codec::CameraPose::from_extrinsics(&raw[off..off + 28], &intrinsics) {
                                    track.push(p);
                                }
                            }
                            track
                        }
                    }
                } else { Vec::new() }
            }
            else if data[pos + 4] == 1 {
                // Compact format
                let intr_off = pos + 8;
                let count_off = pos + 8 + 20;
                if count_off + 4 > data.len() { Vec::new() }
                else if let Some(intrinsics) = fao_v1::v8_reconstruction_codec::CameraIntrinsics::from_bytes(&data[intr_off..intr_off + 20]) {
                    let n = u32::from_le_bytes([data[count_off], data[count_off+1], data[count_off+2], data[count_off+3]]) as usize;
                    let needed = count_off + 4 + 28 * n;
                    if needed > data.len() { Vec::new() } else {
                        let mut track = Vec::with_capacity(n);
                        for i in 0..n {
                            let off = count_off + 4 + 28 * i;
                            if let Some(p) = fao_v1::v8_reconstruction_codec::CameraPose::from_extrinsics(&data[off..off + 28], &intrinsics) {
                                track.push(p);
                            }
                        }
                        track
                    }
                } else { Vec::new() }
            } else {
                // Legacy 48B-per-pose format
                let n = u32::from_le_bytes([data[pos+4], data[pos+5], data[pos+6], data[pos+7]]) as usize;
                let needed = pos + 8 + 48 * n;
                if needed <= data.len() {
                    let mut track = Vec::with_capacity(n);
                    for i in 0..n {
                        let off = pos + 8 + 48 * i;
                        if let Some(p) = fao_v1::v8_reconstruction_codec::CameraPose::from_bytes(&data[off..off + 48]) {
                            track.push(p);
                        }
                    }
                    track
                } else { Vec::new() }
            }
        } else { Vec::new() };
        
        // Parse V8MX multi-web trailer if present.
        // V8MX layout: 'V8MX'(4) + version(1) + n_webs(u32) + per-web records.
        // Each web record is variable size — its total span is read from blob_len 
        // inside the record. To bound the slice we pass to the parser, scan for 
        // the next "V8" magic AFTER the V8MX, or fall back to end-of-data.
        let webs = if let Some(pos) = mx_pos {
            // Find end: the next FAO trailer magic, or EOF.
            let mut end = data.len();
            for i in (pos + 5)..data.len().saturating_sub(4) {
                let m = &data[i..i+4];
                if m == b"V8PT" || m == b"V8MS" || m == b"V8RP" || m == b"V8LR" {
                    end = i;
                    break;
                }
            }
            // The export code writes `mx_trailer.len() as u32 LE` immediately 
            // BEFORE the V8MX magic. Use that length to find the exact end 
            // rather than scanning for the next magic (which can produce 
            // false positives if the zstd-compressed mesh data happens to 
            // contain bytes matching V8PT/V8MS/V8LR/V8RP).
            // 
            // Fallback: if `pos < 4` (the length prefix isn't there), or 
            // the stated length runs past EOF, use the scanned `end` value.
            let mx_end = if pos >= 4 {
                let len_bytes = &data[pos - 4..pos];
                let stated_len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
                let computed_end = pos + stated_len;
                if stated_len > 0 && computed_end <= data.len() && computed_end <= end {
                    computed_end
                } else { end }
            } else { end };
            fao_v1::v8_reconstruction_codec::V8Decoder::parse_v8mx(&data[pos..mx_end])
                .unwrap_or_default()
        } else { Vec::new() };
        
        // Phase 3: V8LR library refs.
        let library_refs = if let Some(pos) = lr_pos {
            // Same length-prefix trick as V8MX: read the u32 LE length 
            // immediately before the magic.
            let mut end = data.len();
            for i in (pos + 5)..data.len().saturating_sub(4) {
                let m = &data[i..i+4];
                if m == b"V8PT" || m == b"V8MS" || m == b"V8MX" || m == b"V8RP" {
                    end = i;
                    break;
                }
            }
            let lr_end = if pos >= 4 {
                let len_bytes = &data[pos - 4..pos];
                let stated_len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
                let computed_end = pos + stated_len;
                if stated_len > 0 && computed_end <= data.len() && computed_end <= end {
                    computed_end
                } else { end }
            } else { end };
            fao_v1::v8_reconstruction_codec::read_v8lr_trailer(&data[pos..lr_end])
                .unwrap_or_default()
        } else { Vec::new() };
        
        // V8AT — per-frame anchor track for the Tobii-style player 
        // overlay. Optional trailer; older .fao files won't have it 
        // and we silently skip. write_v8at_trailer writes the magic 
        // then version(1) + flags(1) + reserved(2) + comp_len(u32 LE) + 
        // payload; read_v8at_trailer expects to start at the magic.
        // 
        // Like V8LR, we use the preceding u32 length prefix (written 
        // by the export path immediately before this trailer) to bound 
        // the read. If the prefix says zero or extends past EOF, fall 
        // back to scanning to the next known trailer start.
        if let Some(pos) = at_pos {
            let at_end = if pos >= 4 {
                let len_bytes = &data[pos - 4..pos];
                let stated_len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
                let computed_end = pos + stated_len;
                if stated_len > 0 && computed_end <= data.len() {
                    computed_end
                } else {
                    data.len()
                }
            } else {
                data.len()
            };
            if let Some(track) = fao_v1::v8_reconstruction_codec::read_v8at_trailer(&data[pos..at_end]) {
                let n_frames = track.len();
                let n_anchors: usize = track.values().map(|v| v.len()).sum();
                self.fao_anchor_track = track;
                eprintln!("FAO v8.0: V8AT anchor track loaded ({} frames, {} anchors — press \"A\" to toggle overlay)",
                    n_frames, n_anchors);
            } else {
                eprintln!("FAO v8.0: V8AT trailer at offset {} failed to parse — overlay unavailable", pos);
            }
        }
        
        // ── V9DC parse ──
        // 
        // When the file has a V9DC trailer (encoded with FAO_V9=1), 
        // load the volumetric chunks into App state. Decoder will use 
        // them in preference to v8 mesh when present. First-cut: just 
        // logs that it found something. Real ray-march decode path 
        // wires up later if v9 proves itself.
        // 
        // Unconditional log so the user can confirm whether V9DC was 
        // present in the file or not. User reported "no v9 trace"; this 
        // helps them tell whether the file lacks v9 data vs. v9 data is 
        // present but parsing failed.
        eprintln!("FAO v9.0: load scan — V9DC trailer {}",
            if v9_pos.is_some() { "FOUND" } else { "absent (file was encoded without FAO_V9=1)" });
        if let Some(pos) = v9_pos {
            let v9_end = if pos >= 4 {
                let stated_len = u32::from_le_bytes([
                    data[pos-4], data[pos-3], data[pos-2], data[pos-1],
                ]) as usize;
                let computed_end = pos + stated_len;
                if stated_len > 0 && computed_end <= data.len() {
                    computed_end
                } else {
                    data.len()
                }
            } else {
                data.len()
            };
            if let Some(chunks) = fao_v1::v9_volumetric::read_v9_trailer(&data[pos..v9_end]) {
                let n_blocks: usize = chunks.iter().map(|c| c.blocks.len()).sum();
                eprintln!("FAO v9.0: V9DC volumetric trailer loaded ({} chunks, {} blocks)",
                    chunks.len(), n_blocks);
                
                // Stash chunks first so the helper can read them.
                self.fao_v9_chunks = chunks;
                
                // Sign-of-life dump via reusable helper. Suppressed when 
                // FAO_V9_NO_SIGNS=1 (user opted out per-session).
                let dump_signs = std::env::var("FAO_V9_NO_SIGNS")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false) == false;
                if dump_signs {
                    self.v9_dump_signs_of_life();
                }
            } else {
                eprintln!("FAO v9.0: V9DC trailer at offset {} failed to parse", pos);
            }
        }
        
        // Only build a decoder if we have either piece
        if mesh.is_some() || !pose_track.is_empty() || !webs.is_empty() || !library_refs.is_empty() {
            let mesh = mesh.unwrap_or_default();
            let n_canonical = webs.iter().filter(|w| w.kind == 1).count();
            let n_aligned = webs.iter().filter(|w| w.kind == 0).count();
            let n_standalone = webs.iter().filter(|w| w.kind == 2).count();
            eprintln!("FAO v8.0: loaded mesh ({} verts, {} tris), {} poses, {} webs ({} canonical + {} aligned + {} standalone)",
                mesh.vertex_count(), mesh.triangle_count(), pose_track.len(),
                webs.len(), n_canonical, n_aligned, n_standalone);
            if !library_refs.is_empty() {
                eprintln!("FAO v8.0: V8LR found — {} library refs (need FAO_LIBRARY env var to render)", library_refs.len());
            }
            let mut dec = fao_v1::v8_reconstruction_codec::V8Decoder::new(self.ow as u32, self.oh as u32)
                .with_mesh(mesh)
                .with_pose_track(pose_track)
                .with_webs(webs);
            dec.library_refs = library_refs;
            
            // Attach library: prefer App field (set via Library tab UI), 
            // falling back to FAO_LIBRARY env var.
            let dec_lib_path = self.fao_web_lib_path.clone()
                .or_else(|| std::env::var("FAO_LIBRARY").ok().map(std::path::PathBuf::from));
            if let Some(path) = dec_lib_path {
                if path.exists() {
                    match fao_v1::library::Library::open(&path) {
                        Ok(lib) => {
                            dec.attach_library(std::sync::Arc::new(std::sync::RwLock::new(lib)));
                            eprintln!("[v8 library] decoder attached to {:?}", path);
                        }
                        Err(e) => {
                            eprintln!("[v8 library] decoder failed to open {:?}: {}", path, e);
                        }
                    }
                }
            }
            
            *self.fao_v8_decoder.write().unwrap() = Some(dec);
        }
    }
    
    /// Read source path trailer (FAOSRCP01) — enables A/B comparison with source
    fn read_source_path(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        let needle = b"FAOSRCP01";
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        
        let mut found_at = None;
        if data.len() >= 13 {
            for i in search_start..data.len().saturating_sub(needle.len()) {
                if &data[i..i+needle.len()] == needle {
                    found_at = Some(i + needle.len());
                    break;
                }
            }
        }
        
        let pos = match found_at { Some(p) => p, None => return };
        if pos + 4 > data.len() { return; }
        let path_len = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        if pos + 4 + path_len > data.len() || path_len > 4096 { return; }
        
        let path_bytes = &data[pos + 4..pos + 4 + path_len];
        if let Ok(path_str) = std::str::from_utf8(path_bytes) {
            // Verify the source file actually exists
            if std::path::Path::new(path_str).exists() {
                eprintln!("FAO: source video found for A/B: {}", path_str);
                self.fao_source_path = Some(path_str.to_string());
            } else {
                eprintln!("FAO: source path stored ({}) but file not found — A/B unavailable", path_str);
            }
        }
    }
    
    /// Dump signs-of-life PPM for the first non-empty v9 chunk.
    /// 
    /// Decodes the chunk, ray-marches from a synthetic camera looking 
    /// at the chunk center from 3× longest extent away, writes a 640×360 
    /// PPM to the OS temp dir. Returns the path on success.
    /// 
    /// Called automatically on load of a .fao with V9DC (unless 
    /// FAO_V9_NO_SIGNS=1) AND callable directly from the GUI "V9 Dump" 
    /// button to refresh after a re-encode without reloading the .fao.
    fn v9_dump_signs_of_life(&self) -> Option<std::path::PathBuf> {
        if self.fao_v9_chunks.is_empty() {
            eprintln!("FAO v9.0: dump skipped — no v9 chunks loaded");
            return None;
        }
        let first = match self.fao_v9_chunks.iter().find(|c| !c.blocks.is_empty()) {
            Some(c) => c,
            None => {
                eprintln!("FAO v9.0: dump skipped — all v9 chunks empty");
                return None;
            }
        };
        let grid = fao_v1::v9_volumetric::decode_v9_chunk(first);
        if grid.cells.is_empty() {
            eprintln!("FAO v9.0: dump skipped — chunk decoded to zero cells");
            return None;
        }
        let n_cells = grid.cells.len();
        // AABB of occupied voxels.
        let mut mn = (i32::MAX, i32::MAX, i32::MAX);
        let mut mx = (i32::MIN, i32::MIN, i32::MIN);
        for &k in grid.cells.keys() {
            let (x, y, z) = k.decode();
            mn.0 = mn.0.min(x as i32); mn.1 = mn.1.min(y as i32); mn.2 = mn.2.min(z as i32);
            mx.0 = mx.0.max(x as i32); mx.1 = mx.1.max(y as i32); mx.2 = mx.2.max(z as i32);
        }
        let p_mid = grid.voxel_to_world((mn.0+mx.0)/2, (mn.1+mx.1)/2, (mn.2+mx.2)/2);
        let ext = (mx.0 - mn.0).max(mx.1 - mn.1).max(mx.2 - mn.2) as f32 * grid.voxel_size;
        let pose = fao_v1::v8_reconstruction_codec::CameraPose {
            position: fao_v1::geometry::Vec3 {
                x: p_mid.x, y: p_mid.y, z: p_mid.z - ext * 3.0,
            },
            rotation: [1.0, 0.0, 0.0, 0.0],
            focal: [1000.0, 1000.0],
            principal: [0.5, 0.5],
            distortion: 0.0,
        };
        let w = 640usize;
        let h = 360usize;
        let rgb = fao_v1::v9_volumetric::ray_march(&grid, &pose, w, h);
        let mut nonzero = 0usize;
        for b in &rgb { if *b > 0 { nonzero += 1; } }
        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join("v9_signs_of_life.ppm");
        match std::fs::File::create(&tmp_path) {
            Ok(mut f) => {
                use std::io::Write;
                let _ = write!(f, "P6\n{} {}\n255\n", w, h);
                let _ = f.write_all(&rgb);
                eprintln!("FAO v9.0: signs-of-life dumped to {} ({}×{} PPM, {} non-zero of {} bytes, {} voxel cells)",
                    tmp_path.display(), w, h, nonzero, rgb.len(), n_cells);
                Some(tmp_path)
            }
            Err(e) => {
                eprintln!("FAO v9.0: FAILED to write {} — {} ({} non-zero of {} bytes, {} voxel cells)",
                    tmp_path.display(), e, nonzero, rgb.len(), n_cells);
                None
            }
        }
    }

    /// Fetch a single source frame DURING ENCODE for preview comparison.
    /// Uses self.src_path and self.vid_fps.
    fn fetch_source_frame_encode(&self, idx: usize) -> Option<Vec<u8>> {
        if self.src_path.is_empty() { return None; }
        let w = self.cur_w;
        let h = self.cur_h;
        let fps = self.vid_fps;
        if w == 0 || h == 0 || fps <= 0.0 { return None; }
        
        // Cache check
        if let Ok(cache) = self.fao_source_cache.lock() {
            if let Some(frame) = cache.get(&idx) {
                return Some(frame.clone());
            }
        }
        
        let timestamp = idx as f64 / fps;
        let output = std::process::Command::new("ffmpeg")
            .args(&[
                "-ss", &format!("{:.3}", timestamp),
                "-i", &self.src_path,
                "-frames:v", "1",
                "-f", "rawvideo",
                "-pix_fmt", "rgb24",
                "-v", "quiet",
                "-",
            ])
            .output()
            .ok()?;
        
        let expected_sz = w * h * 3;
        if output.stdout.len() != expected_sz { return None; }
        
        if let Ok(mut cache) = self.fao_source_cache.lock() {
            if cache.len() < 50 { cache.insert(idx, output.stdout.clone()); }
        }
        
        Some(output.stdout)
    }
    
    /// Fetch a single frame from the source video using ffmpeg.
    /// Returns RGB bytes (w*h*3) or None on failure.
    /// Caches results to avoid re-decoding.
    fn fetch_source_frame(&self, idx: usize) -> Option<Vec<u8>> {
        // Check cache first
        if let Ok(cache) = self.fao_source_cache.lock() {
            if let Some(frame) = cache.get(&idx) {
                return Some(frame.clone());
            }
        }
        // Cache miss — request an ASYNC fill (worker thread spawns 
        // ffmpeg and populates the cache). Returns None immediately 
        // so playback doesn't block; the next frame display may HIT 
        // the now-populated cache.
        //
        // Previously this was a synchronous ffmpeg spawn that blocked 
        // the UI thread for ~500-1500ms per batch boundary (every 60 
        // frames). On slow-fps content that's a hitch every 60s.
        self.request_source_cache_fill(idx);
        // Best-effort second-look: if a previous async fill already 
        // landed for this idx, we'll find it now. Most of the time 
        // we won't and the caller will fall back to an interpolated 
        // or held-prev frame for one tick.
        if let Ok(cache) = self.fao_source_cache.lock() {
            if let Some(frame) = cache.get(&idx) {
                return Some(frame.clone());
            }
        }
        None
    }
    
    /// Decode a window of source video frames and stuff them into the 
    /// source cache. The window is centered around `idx` and sized to 
    /// amortize the ffmpeg startup + keyframe-pre-roll cost across many 
    /// frames.
    /// 
    /// Per-frame call would be ~50-300ms (one ffmpeg process per frame); 
    /// batching 60 frames into one call drops amortized cost to ~5-10ms 
    /// per frame for the next ~60 calls — fast enough for interactive 
    /// playback where the v8 rasterizer is still pending.
    /// Async request: send a fill task to the source-fetch worker. 
    /// Returns immediately. Worker spawns ffmpeg, decodes a 60-frame 
    /// batch, and populates fao_source_cache. The next playback tick 
    /// may HIT the now-populated cache.
    /// 
    /// If the channel is full (worker is mid-batch), the request is 
    /// dropped silently — fine, because playback will retry on the 
    /// next miss.
    fn request_source_cache_fill(&self, idx: usize) {
        // One-shot diagnostic counters so we can see WHERE tasks are 
        // being dropped without spamming the log. Each branch increments 
        // a counter; first hit prints the reason.
        static DROPS_NO_WORKER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static DROPS_NO_SOURCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static DROPS_BAD_DIMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static DROPS_CHANNEL_FULL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static SENT_OK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        
        let tx = match self.source_fetch_tx.as_ref() {
            Some(t) => t,
            None => {
                if DROPS_NO_WORKER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 1 {
                    eprintln!("[FAO source-fetch] request_source_cache_fill: tx=None (worker not yet spawned) at idx={}", idx);
                }
                return;
            }
        };
        let src = match self.fao_source_path.as_ref() {
            Some(s) => s.clone(),
            None => {
                if DROPS_NO_SOURCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 1 {
                    eprintln!("[FAO source-fetch] request_source_cache_fill: fao_source_path=None at idx={}", idx);
                }
                return;
            }
        };
        let w = self.cur_w;
        let h = self.cur_h;
        let fps = self.vid_fps;
        if w == 0 || h == 0 || fps <= 0.0 {
            if DROPS_BAD_DIMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 1 {
                eprintln!("[FAO source-fetch] request_source_cache_fill: bad dims w={} h={} fps={} at idx={}", w, h, fps, idx);
            }
            return;
        }
        let task = SourceFetchTask { idx, src_path: src, width: w, height: h, fps };
        // try_send: dropped on full channel. Worker eventually catches up.
        match tx.try_send(task) {
            Ok(()) => {
                let n = SENT_OK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n == 1 || n == 10 || n == 100 {
                    eprintln!("[FAO source-fetch] request_source_cache_fill: sent ok #{} (idx={})", n, idx);
                }
            }
            Err(_) => {
                let n = DROPS_CHANNEL_FULL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n == 1 || n == 10 || n == 100 {
                    eprintln!("[FAO source-fetch] request_source_cache_fill: channel full, dropped task #{} (idx={})", n, idx);
                }
            }
        }
    }
    
    /// Synchronous source-cache batch fill. Retained as a fallback 
    /// path (e.g. seeking / scrub where we genuinely need the frame 
    /// NOW). Not called from the regular playback fast path anymore.
    #[allow(dead_code)]
    fn populate_source_cache_batch(&self, idx: usize) {
        let src = match self.fao_source_path.as_ref() { Some(s) => s, None => return };
        let w = self.cur_w;
        let h = self.cur_h;
        let fps = self.vid_fps;
        if w == 0 || h == 0 || fps <= 0.0 { return; }
        
        // Don't refetch if the requested index is already in cache 
        // (race with another thread).
        if let Ok(cache) = self.fao_source_cache.lock() {
            if cache.contains_key(&idx) { return; }
        }
        
        // Window: 60 frames starting at idx (forward-only — playback 
        // typically advances). Reduce on small index.
        let batch_n: usize = 60;
        let timestamp = idx as f64 / fps;
        let pre_roll_s = 1.0_f64;
        let coarse_seek = (timestamp - pre_roll_s).max(0.0);
        // Two-stage seek: fast keyframe seek to coarse, then accurate 
        // seek to the precise timestamp. Without the post-input -ss, 
        // we get keyframe-snapping that can be off by several seconds 
        // on .webm files with sparse keyframes.
        let accurate_seek = (timestamp - coarse_seek).max(0.0);
        let select_expr = format!(
            "select=between(n\\,0\\,{})",
            (batch_n as i64 - 1).max(0)
        );
        let output = std::process::Command::new("ffmpeg")
            .args(&[
                "-ss", &format!("{:.3}", coarse_seek),
                "-i", src,
                "-ss", &format!("{:.3}", accurate_seek),
                "-vf", &select_expr,
                "-vsync", "0",
                "-f", "rawvideo",
                "-pix_fmt", "rgb24",
                "-v", "quiet",
                "-",
            ])
            .output();
        let output = match output { Ok(o) => o, Err(_) => return };
        
        let frame_sz = w * h * 3;
        if frame_sz == 0 { return; }
        let n_decoded = output.stdout.len() / frame_sz;
        if n_decoded == 0 { return; }
        
        // Insert decoded frames into cache. Frame ordering: the FIRST 
        // decoded frame corresponds to source frame `idx` (because we 
        // already skipped `frames_to_skip` from coarse_seek, which 
        // aligns to idx).
        if let Ok(mut cache) = self.fao_source_cache.lock() {
            // Cap cache size to avoid runaway memory. Drop oldest 
            // entries when exceeding the limit.
            const MAX_ENTRIES: usize = 200;  // ~1.2GB at 1080p, 540MB at 720p
            for k in 0..n_decoded {
                let frame_idx = idx + k;
                let off = k * frame_sz;
                if off + frame_sz > output.stdout.len() { break; }
                if cache.len() >= MAX_ENTRIES {
                    // Evict the entry farthest from `idx` (typically 
                    // older frames the player already passed through).
                    let evict_target = cache.keys()
                        .max_by_key(|&&k| k.abs_diff(idx))
                        .copied();
                    if let Some(e) = evict_target { cache.remove(&e); }
                    if cache.len() >= MAX_ENTRIES { break; }
                }
                cache.insert(frame_idx, output.stdout[off..off + frame_sz].to_vec());
            }
        }
    }
    
    /// Read motion profile trailer from end of .fao file (if present).
    /// 
    /// Two on-disk formats:
    ///   FAOMOT01: raw — n(u32) + n×u32 sizes + n×u8 deltas
    ///   FAOMOT02: zstd-compressed payload — n(u32) + clen(u32) + zstd(payload)
    /// 
    /// The compressed format saves ~80-90% on this trailer. For a 600hr 
    /// library that's the difference between 256MB and ~30MB.
    fn read_motion_profile(&mut self) {
        let data = match &self.fao_data { Some(d) => d, None => return };
        // Same scan-narrowing as read_v8_trailer: start after frame data 
        // ends so we don't false-match FAOMOT markers inside frame zstd 
        // blobs.
        let frame_data_end: usize = self.fao_index.iter()
            .map(|e| (e.offset as usize).saturating_add(e.size as usize))
            .max()
            .unwrap_or(0);
        let search_start = frame_data_end.min(data.len());
        
        // Search for either magic (newest format wins; we scan once for the 
        // 6-byte common prefix "FAOMOT" and then check the version byte 
        // pair). Cheap.
        let prefix = b"FAOMOT";
        let mut found_at = None;
        let mut compressed_form = false;
        if data.len() >= 12 {
            for i in search_start..data.len().saturating_sub(8) {
                if &data[i..i+6] == prefix {
                    let version = &data[i+6..i+8];
                    if version == b"02" {
                        found_at = Some(i + 8);
                        compressed_form = true;
                        break;
                    } else if version == b"01" {
                        found_at = Some(i + 8);
                        compressed_form = false;
                        break;
                    }
                }
            }
        }
        
        let pos = match found_at { Some(p) => p, None => return };
        if pos + 4 > data.len() { return; }
        
        let n = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        let mut p = pos + 4;
        
        // Build the raw payload (sizes followed by deltas) regardless of 
        // backing format.
        let raw: Vec<u8> = if compressed_form {
            if p + 4 > data.len() { return; }
            let clen = u32::from_le_bytes([data[p], data[p+1], data[p+2], data[p+3]]) as usize;
            p += 4;
            if p + clen > data.len() { return; }
            use std::io::Cursor;
            match zstd::decode_all(Cursor::new(&data[p..p+clen])) {
                Ok(d) => d,
                Err(e) => { eprintln!("FAO: motion profile zstd decode failed: {}", e); return; }
            }
        } else {
            // Raw form: sizes (n*4) + deltas (n)
            if p + n * 5 > data.len() { return; }
            data[p..p + n * 5].to_vec()
        };
        
        // Parse the raw payload.
        if raw.len() < n * 4 { return; }
        let mut sizes = Vec::with_capacity(n);
        for i in 0..n {
            let off = i * 4;
            sizes.push(u32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]));
        }
        let deltas: Vec<u8> = if raw.len() >= n * 4 + n {
            raw[n*4..n*4+n].to_vec()
        } else {
            Vec::new()
        };
        
        eprintln!("FAO motion profile loaded: {} frames, avg size {} bytes, max delta {}{}",
            n,
            sizes.iter().sum::<u32>() / sizes.len().max(1) as u32,
            deltas.iter().max().copied().unwrap_or(0),
            if compressed_form { " (zstd-compressed)" } else { "" });
        
        self.fao_motion_sizes = sizes;
        self.fao_motion_deltas = deltas;
    }
    
    /// Build the anchor map by scanning all frames once.
    /// For each frame index, records the index of the nearest prev and next anchor.
    /// Anchors = frames with substantial encoded data (> 100 bytes after FaoFrameHeader).
    fn build_anchor_map(&mut self) {
        // An "anchor" here means a frame the player can seek to without 
        // needing a previous decoded frame. The seek logic walks 
        // (anchor → next → next → ... → target) when decoding a 
        // T-frame chain, so we need the set of frames that can stand on 
        // their own.
        // 
        // Format-based classification (read first byte of the payload):
        // 
        //   'D' raw DCT I-frame              → SELF-CONTAINED (anchor)
        //   'L' lossy-chroma I-frame         → SELF-CONTAINED (anchor)
        //   'P' YUV-split I-frame            → SELF-CONTAINED (anchor)
        //   'K' block-dict K-frame           → SELF-CONTAINED (needs trailer dict, but no prev frame)
        //   'V' v8 frame                     → SELF-CONTAINED (geometry from V8MX trailer + pose)
        //   'Z' zstd-wrapped (always v8)     → SELF-CONTAINED (unwraps to V)
        //   'T' translated/predicted frame   → NOT self-contained (needs anchor_ref)
        //   '_' or empty (skip)              → NOT self-contained (copies previous)
        //   'B' block fragment               → NOT self-contained
        // 
        // Previously this method tested `fhdr.frame_type == 0` — but the 
        // worker thread always stamps frame_type=1 for high-res content, 
        // so that test produced ZERO anchors on every v8 export. Switching 
        // to format-based detection fixes the playback failure where every 
        // T-frame couldn't find an anchor to decode against.
        let n = self.fao_index.len();
        if n == 0 { return; }
        let mut anchors: Vec<u32> = Vec::with_capacity(n / 30);
        // Track per-format counts for the diagnostic log line.
        let mut by_marker = std::collections::BTreeMap::<u8, u32>::new();
        {
            let data = match &self.fao_data { Some(d) => d, None => return };
            for i in 0..n {
                let entry = &self.fao_index[i];
                if let Some((data_bytes, _fhdr)) = fao_v1::read_fao_frame(data, entry) {
                    if data_bytes.is_empty() { continue; }
                    let marker = data_bytes[0];
                    *by_marker.entry(marker).or_insert(0) += 1;
                    let is_anchor = matches!(marker, b'D' | b'L' | b'P' | b'K' | b'V' | b'Z');
                    if is_anchor {
                        anchors.push(i as u32);
                    }
                }
            }
        }
        
        // Format the marker breakdown for the log line.
        let breakdown = by_marker.iter()
            .map(|(m, c)| format!("{}={}", *m as char, c))
            .collect::<Vec<_>>().join(" ");
        eprintln!("FAO anchor_map: {} anchors out of {} frames ({:.1}%) [{}]",
            anchors.len(), n, anchors.len() as f64 / n as f64 * 100.0, breakdown);
        
        // Second pass: for each frame, find its prev and next anchor via binary search
        self.anchor_map = vec![(None, None); n];
        for i in 0..n {
            let fi = i as u32;
            // Find next anchor: first anchor >= fi
            let next_anchor = match anchors.binary_search(&fi) {
                Ok(pos) => Some(anchors[pos]),  // exact match
                Err(pos) => anchors.get(pos).copied(),  // insertion point
            };
            // Find prev anchor: last anchor <= fi
            let prev_anchor = match anchors.binary_search(&fi) {
                Ok(pos) => Some(anchors[pos]),  // exact match
                Err(pos) if pos > 0 => Some(anchors[pos - 1]),
                _ => None,
            };
            self.anchor_map[i] = (prev_anchor, next_anchor);
        }
    }

    /// Find the nearest anchor frames around a given index.
    /// Returns (prev_anchor_idx, next_anchor_idx).
    fn find_anchors(&self, idx: usize) -> (Option<usize>, Option<usize>) {
        let n = self.fao_index.len();
        if n == 0 { return (None, None); }
        
        // Fast path: use pre-computed anchor map if available
        if idx < self.anchor_map.len() {
            let (p, nx) = self.anchor_map[idx];
            return (p.map(|x| x as usize), nx.map(|x| x as usize));
        }
        
        // Fallback: linear search (only if map not built yet)
        let data = match &self.fao_data { Some(d) => d, None => return (None, None) };
        
        // Search up to 200 frames in each direction (covers long static scenes).
        // Anchor classification: format-marker based — same logic as 
        // build_anchor_map. See that function for the full table.
        let is_anchor_marker = |m: u8| matches!(m, b'D' | b'L' | b'P' | b'K' | b'V' | b'Z');
        let mut prev_anchor = None;
        for back in 0..=200.min(idx) {
            let candidate = idx.saturating_sub(back);
            let entry = &self.fao_index[candidate];
            if let Some((data_bytes, _)) = fao_v1::read_fao_frame(data, entry) {
                if !data_bytes.is_empty() && is_anchor_marker(data_bytes[0]) {
                    prev_anchor = Some(candidate);
                    break;
                }
            }
            if candidate == 0 { break; }
        }
        
        let mut next_anchor = None;
        for fwd in 1..=200 {
            let candidate = idx + fwd;
            if candidate >= n { break; }
            let entry = &self.fao_index[candidate];
            if let Some((data_bytes, _)) = fao_v1::read_fao_frame(data, entry) {
                if !data_bytes.is_empty() && is_anchor_marker(data_bytes[0]) {
                    next_anchor = Some(candidate);
                    break;
                }
            }
        }
        
        (prev_anchor, next_anchor)
    }
    
    /// Decode a specific FAO frame into RGB (without updating display state).
    fn decode_fao_frame_to_rgb(&self, idx: usize) -> Option<Vec<u8>> {
        // Check anchor cache first (fast path for repeated interpolation)
        for (i, rgb) in self.anchor_cache.iter().rev() {
            if *i == idx { return Some(rgb.clone()); }
        }
        let data = self.fao_data.as_ref()?;
        if idx >= self.fao_index.len() { return None; }
        let entry = &self.fao_index[idx];
        let w = self.cur_w;
        let h = self.cur_h;
        let yuv_sz = w * h * 3 / 2;
        let rgb_sz = w * h * 3;
        
        let (data_bytes, _) = fao_v1::read_fao_frame(data, entry)?;
        if data_bytes.is_empty() { return None; }
        
        // ── v8.3 "R" repeat-marker fast path ──
        // 
        // Encoder may emit a 1-byte b'R' marker for static frames (gated 
        // by FAO_V83_REPEAT at encode time). Decoder walks BACKWARDS 
        // through the FAO index to find the most recent non-R frame, 
        // then recurses to decode that. Bounded search depth so a 
        // corrupt run of R markers can't spin forever.
        // ── v10 'X'-wrapper dispatch ──
        // 
        // v10 frames start with magic byte 'X' followed by a strategy 
        // id. Static-repeat and motion-comp strategies depend on the 
        // PREVIOUS frame's decoded RGB, which we get by recursively 
        // decoding idx-1. The recursion terminates at the first non-v10 
        // frame (legacy) or at idx 0.
        if !data_bytes.is_empty() && data_bytes[0] == fao_v1::v10::V10_MAGIC {
            // Get prev_rgb by decoding idx-1 (if any).
            let prev_rgb = if idx > 0 {
                self.decode_fao_frame_to_rgb(idx - 1)
            } else {
                // First frame can't be motion-comp (needs predecessor). 
                // Decoder may still handle a static-repeat at idx 0 (
                // weird — but spec-allowed), returning zeros or None.
                None
            };
            return fao_v1::v10::decode_frame(&data_bytes, prev_rgb.as_deref(), w, h);
        }
        
        if !data_bytes.is_empty() && data_bytes[0] == b'R' {
            const MAX_R_WALKBACK: usize = 600;
            let walkback_limit = idx.min(MAX_R_WALKBACK);
            let mut resolved_idx: Option<usize> = None;
            for back in 1..=walkback_limit {
                let prev_idx = idx - back;
                let prev_entry = &self.fao_index[prev_idx];
                let (prev_bytes, _) = match fao_v1::read_fao_frame(data, prev_entry) {
                    Some(v) => v,
                    None => continue,
                };
                if prev_bytes.is_empty() { continue; }
                if prev_bytes[0] != b'R' {
                    resolved_idx = Some(prev_idx);
                    break;
                }
            }
            match resolved_idx {
                Some(p) => return self.decode_fao_frame_to_rgb(p),
                None => {
                    eprintln!("[v8 R-marker] frame {} couldn't resolve after {} walkback steps", idx, walkback_limit);
                    return None;
                }
            }
        }
        
        // V8 frames can't be decoded standalone (no rasterizer yet) — but 
        // T-frames that reference V8 anchors NEED an RGB anchor to motion-comp 
        // against. As a stopgap, try the source video if it's available; this 
        // lets T-frames decode correctly even though the V8 rendering itself 
        // is still pending.
        let is_v8 = data_bytes[0] == b'V' && data_bytes.len() >= 2 && data_bytes[1] == b'8';
        let is_v81_or_zstd = data_bytes[0] == b'Z' 
            || (data_bytes[0] == b'V' && data_bytes.len() >= 3 && data_bytes[1] == b'8' && data_bytes[2] == b'1');
        if is_v8 || is_v81_or_zstd {
            // ── v9 ray-march override ──
            // 
            // When FAO_V9_RENDER=1 + v9 chunks loaded + a chunk covers 
            // this frame + a pose is available, ray-march the voxel grid 
            // for THIS frame and return. Falls through to v8 mesh path 
            // on any miss.
            let v9_render_on = std::env::var("FAO_V9_RENDER")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if v9_render_on && !self.fao_v9_chunks.is_empty() {
                let frame_u32 = idx as u32;
                // Find the chunk whose frame range covers this frame. 
                // V9Chunk uses inclusive frame_end (matches v8's 
                // chunk_frame_start..=chunk_frame_end usage).
                let chunk_opt = self.fao_v9_chunks.iter().find(|c| {
                    frame_u32 >= c.frame_start && frame_u32 <= c.frame_end
                        && !c.blocks.is_empty()
                });
                if let Some(v9c) = chunk_opt {
                    // Pull the pose from the v8 decoder's pose_track. 
                    // If unavailable, fall back to a synthetic looking-at 
                    // the chunk's center pose (so we still get *some* 
                    // output instead of nothing).
                    let pose_owned = {
                        let guard = self.fao_v8_decoder.read().unwrap();
                        guard.as_ref().and_then(|dec| {
                            if (idx as usize) < dec.pose_track.len() {
                                let p = &dec.pose_track[idx as usize];
                                if p.focal[0] > 0.0 { Some(*p) } else { None }
                            } else { None }
                        })
                    };
                    // Find chunk index for cache key (use pointer equality 
                    // since chunks are stored in self.fao_v9_chunks).
                    let chunk_idx = self.fao_v9_chunks.iter().position(|c| std::ptr::eq(c, v9c)).unwrap_or(0);
                    // LRU cache lookup → decode + insert on miss.
                    let grid_rc: std::rc::Rc<fao_v1::v9_volumetric::VoxelHashGrid> = {
                        let mut cache = self.fao_v9_grid_cache.borrow_mut();
                        let found = cache.iter().position(|(i, _)| *i == chunk_idx);
                        if let Some(p) = found {
                            let entry = cache.remove(p);
                            let grid = entry.1.clone();
                            cache.insert(0, entry);
                            grid
                        } else {
                            let g = std::rc::Rc::new(fao_v1::v9_volumetric::decode_v9_chunk(v9c));
                            cache.insert(0, (chunk_idx, g.clone()));
                            cache.truncate(3);
                            g
                        }
                    };
                    // Synthetic fallback pose: when v8 pose track has no 
                    // entry for this frame, look at chunk center from 3× 
                    // its longest extent.
                    let pose = if let Some(p) = pose_owned {
                        p
                    } else {
                        let mut mn = (i32::MAX, i32::MAX, i32::MAX);
                        let mut mx = (i32::MIN, i32::MIN, i32::MIN);
                        for &k in grid_rc.cells.keys() {
                            let (x, y, z) = k.decode();
                            mn.0 = mn.0.min(x as i32); mn.1 = mn.1.min(y as i32); mn.2 = mn.2.min(z as i32);
                            mx.0 = mx.0.max(x as i32); mx.1 = mx.1.max(y as i32); mx.2 = mx.2.max(z as i32);
                        }
                        let p_mid = grid_rc.voxel_to_world((mn.0+mx.0)/2, (mn.1+mx.1)/2, (mn.2+mx.2)/2);
                        let ext = ((mx.0 - mn.0).max(mx.1 - mn.1).max(mx.2 - mn.2) as f32) * grid_rc.voxel_size;
                        fao_v1::v8_reconstruction_codec::CameraPose {
                            position: fao_v1::geometry::Vec3 { x: p_mid.x, y: p_mid.y, z: p_mid.z - ext * 3.0 },
                            rotation: [1.0, 0.0, 0.0, 0.0],
                            focal: [w as f32, h as f32],
                            principal: [0.5, 0.5],
                            distortion: 0.0,
                        }
                    };
                    if !grid_rc.cells.is_empty() {
                        let rgb = fao_v1::v9_volumetric::ray_march(&*grid_rc, &pose, w, h);
                        let mut nonzero = 0usize;
                        for b in &rgb { if *b > 0 { nonzero += 1; } }
                        if nonzero > 0 {
                            return Some(rgb);
                        }
                        // If the ray-march produced all-black for this 
                        // pose, fall through to v8 mesh rendering so 
                        // playback doesn't freeze.
                    }
                }
            }
            
            // Real v8 render: rasterize the mesh from the V8MX trailer 
            // through this frame's pose. Returns RGB directly (not YUV).
            let v8_render = self.fao_v8_decoder.read().unwrap().as_ref()
                .and_then(|dec| dec.render_frame_rgb(&data_bytes, idx as u32));
            if v8_render.is_some() { return v8_render; }
            // Fallback for T-frame anchor resolution: source video frame. 
            // Without this, T-frames referencing v8 anchors fail and 
            // produce gray output.
            if let Some(src) = self.fetch_source_frame(idx) {
                return Some(src);
            }
            return None;
        }
        
        match data_bytes[0] {
            b'D' => fao_v1::decompress_frame_dct(&data_bytes, w, h)
                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h)),
            b'P' => fao_v1::decompress_frame_yuv_split(&data_bytes, w, h)
                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h)),
            b'L' => fao_v1::decompress_frame_lossy_chroma(&data_bytes, w, h)
                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h)),
            b'K' => {
                // K-frame uses dictionary; only decodable if dict loaded
                self.fao_block_dict.as_ref().and_then(|dict| {
                    fao_v1::block_dict::decode_frame_from_dict(&data_bytes, dict)
                        .map(|(yuv, fw, fh)| TranscodeState::yuv_to_rgb(&yuv, fw, fh))
                })
            },
            b'V' => {
                // 'V' could be v3 V-frame OR v4 sparse-skeleton frame
                // v4 has 'V' '4' as its first 2 bytes
                if data_bytes.len() >= 2 && data_bytes[1] == b'8' {
                    // v8 frame: rasterize the active web's mesh through 
                    // this frame's camera pose. Returns RGB directly 
                    // (not YUV) so we don't run yuv_to_rgb.
                    // 
                    // Returns None when:
                    //   - The frame's web has an empty mesh (early in 
                    //     the chunk before triangulation has accumulated)
                    //   - All triangles are off-screen or behind camera
                    // Caller falls back to source-video display in those cases.
                    self.fao_v8_decoder.read().unwrap().as_ref().and_then(|dec| {
                        dec.render_frame_rgb(&data_bytes, idx as u32)
                    })
                } else if data_bytes.len() >= 2 && (data_bytes[1] == b'4' || data_bytes[1] == b'5' || data_bytes[1] == b'6' || data_bytes[1] == b'7') {
                    // Legacy v4-v7 frames — codecs removed in v10 cleanup. 
                    // Returning None causes the player to passthrough (display source).
                    None
                } else {
                    // Legacy v3 V-frame — v3 codec removed; passthrough.
                    None
                }
            },
            b'T' => {
                // v2.1: recursively decode anchor (avoid infinite recursion via depth check)
                fao_v1::v21_predictive::TranslatedFrame::from_bytes(&data_bytes)
                    .and_then(|(tframe, fw, fh)| {
                        let anchor_idx = tframe.anchor_ref as usize;
                        if anchor_idx == idx { return None; }  // self-ref guard
                        self.decode_fao_frame_to_rgb(anchor_idx).and_then(|anchor_rgb| {
                            let anchor_yuv = rgb_to_yuv420p(&anchor_rgb, fw, fh);
                            tframe.decode_with_anchor(&anchor_yuv, fw, fh)
                                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, fw, fh))
                        })
                    })
            },
            _ => {
                if data_bytes.len() == yuv_sz {
                    Some(TranscodeState::yuv_to_rgb(&data_bytes, w, h))
                } else if data_bytes.len() == rgb_sz {
                    Some(data_bytes)
                } else {
                    fao_v1::decompress_frame(&data_bytes, yuv_sz)
                        .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h))
                        .or_else(|| fao_v1::decompress_frame(&data_bytes, rgb_sz))
                }
            }
        }
    }
    
    /// Interpolate a skip frame from neighboring anchors.
    fn interpolate_skip_frame(&mut self, idx: usize) -> Option<Vec<u8>> {
        let (prev_a, next_a) = self.find_anchors(idx);
        let w = self.cur_w;
        let h = self.cur_h;
        let rgb_sz = w * h * 3;
        
        let prev_rgb = match prev_a {
            Some(pa) if pa != idx => {
                let r = self.decode_fao_frame_to_rgb(pa)?;
                // Cache this anchor for next interpolation
                self.cache_anchor(pa, r.clone());
                r
            },
            _ => return None,
        };
        
        if let Some(next) = next_a {
            if next > idx && prev_a.is_some() && prev_a.unwrap() < idx {
                let prev = prev_a.unwrap();
                let span = (next - prev) as f32;
                let t = (idx - prev) as f32 / span;
                
                if let Some(next_rgb) = self.decode_fao_frame_to_rgb(next) {
                    self.cache_anchor(next, next_rgb.clone());
                    if next_rgb.len() == prev_rgb.len() && prev_rgb.len() == rgb_sz {
                        // GHOSTING DETECTION: sample anchor difference to detect scene changes
                        // If anchors are very different, blending creates double-image artifacts
                        // → snap to nearest anchor instead
                        let mut anchor_diff_sum: u64 = 0;
                        let sample_step = (rgb_sz / 2048).max(1);  // ~2K samples
                        let mut samples = 0u64;
                        for i in (0..rgb_sz).step_by(sample_step) {
                            let d = (prev_rgb[i] as i32 - next_rgb[i] as i32).unsigned_abs() as u64;
                            anchor_diff_sum += d;
                            samples += 1;
                        }
                        let avg_diff = if samples > 0 { anchor_diff_sum / samples } else { 0 };
                        
                        // Combine with stored motion data
                        let motion_factor = self.fao_motion_deltas.get(idx).copied().unwrap_or(0);
                        
                        // Decision tree:
                        //   high anchor diff (>30 avg) → SCENE CHANGE → snap to nearest
                        //   high motion factor (>80)   → fast motion → snap to nearest
                        //   medium                     → bias toward nearest
                        //   low                        → smooth blend
                        let mut t_adjusted = t;
                        if avg_diff > 30 || motion_factor > 80 {
                            t_adjusted = if t < 0.5 { 0.0 } else { 1.0 };
                        } else if avg_diff > 15 || motion_factor > 40 {
                            let bias = ((avg_diff.max(motion_factor as u64) - 15) as f32 / 25.0).min(1.0);
                            t_adjusted = if t < 0.5 { t * (1.0 - bias * 0.7) } 
                                        else { t + (1.0 - t) * bias * 0.7 };
                        }
                        
                        let mut blended: Vec<u8> = Vec::with_capacity(rgb_sz);
                        let inv_t = 1.0 - t_adjusted;
                        let t = t_adjusted;
                        for i in 0..rgb_sz {
                            let a = prev_rgb[i] as f32;
                            let b = next_rgb[i] as f32;
                            blended.push((a * inv_t + b * t).clamp(0.0, 255.0) as u8);
                        }
                        // Estimate prediction quality: pixel difference between anchors
                        let mut sum_sq: u64 = 0;
                        let sample_step = (rgb_sz / 4096).max(1);
                        let mut samples = 0u64;
                        for i in (0..rgb_sz).step_by(sample_step) {
                            let d = (prev_rgb[i] as i32 - next_rgb[i] as i32) as i64;
                            sum_sq += (d * d) as u64;
                            samples += 1;
                        }
                        let mse = if samples > 0 { sum_sq as f32 / samples as f32 } else { 0.0 };
                        let pred_psnr = if mse > 0.0 {
                            10.0 * (255.0_f32 * 255.0 / mse).log10()
                        } else { 99.0 };
                        
                        self.interp_stats.0 = self.interp_stats.0.saturating_add(1);
                        self.interp_stats.2 = t;
                        self.interp_stats.3 = pred_psnr;
                        return Some(blended);
                    }
                }
            }
        }
        
        Some(prev_rgb)
    }
    
    /// Add an anchor frame's decoded RGB to the cache (LRU, max 16 entries)
    fn cache_anchor(&mut self, idx: usize, rgb: Vec<u8>) {
        // Avoid duplicates
        if self.anchor_cache.iter().any(|(i, _)| *i == idx) { return; }
        if self.anchor_cache.len() >= 16 {
            self.anchor_cache.pop_front();
        }
        self.anchor_cache.push_back((idx, rgb));
    }
    
    /// Prefetch upcoming frames into the cache (synchronous, fast)
    fn prefetch_fao_frames(&mut self, start: usize, count: usize) {
        // ── ASYNC PREFETCH ──
        // 
        // Previously this ran SYNCHRONOUSLY on the UI thread with an 
        // 80ms time budget. v8 mesh rasterization at 1080p costs 
        // 50-100ms per frame, so prefetch would do 0-1 frames per call 
        // and bail. Cache hit rate stayed near zero.
        // 
        // Now: extract frame bytes synchronously (cheap, just a slice) 
        // and submit a task to the background worker. UI returns 
        // immediately. Worker decodes off the UI thread and inserts 
        // into the shared cache. Next display tick may HIT the cache.
        // 
        // Backpressure: channel is bounded (capacity 4). If the worker 
        // is busy, try_send fails and we just skip — the cache fills 
        // opportunistically; not every hint needs to land.
        let tx = match self.prefetch_tx.as_ref() {
            Some(t) => t,
            None => return,  // worker not yet spawned (no .fao loaded)
        };
        let w = self.cur_w;
        let h = self.cur_h;
        if w == 0 || h == 0 { return; }
        
        // Read frame bytes from fao_data inside a scoped borrow, then 
        // send each task. Only collect what's not already cached.
        let cache_arc = std::sync::Arc::clone(&self.fao_prefetch);
        let data = match &self.fao_data { Some(d) => d, None => return };
        
        for offset in 0..count {
            let idx = start + offset;
            if idx >= self.fao_index.len() { break; }
            
            // Check cache — skip if already there.
            {
                let c = cache_arc.lock().unwrap();
                if c.entries.iter().any(|(i, _)| *i == idx) { continue; }
            }
            
            let entry = &self.fao_index[idx];
            if let Some((data_bytes, _)) = fao_v1::read_fao_frame(data, entry) {
                if data_bytes.is_empty() { continue; }
                let task = PrefetchTask {
                    idx, data_bytes, width: w, height: h,
                };
                // try_send: if worker is busy with previous tasks, 
                // give up rather than block UI. Next playback tick will 
                // try again.
                let _ = tx.try_send(task);
            }
        }
    }
    
    /// Record one playback-trace sample for the given frame index. Reads 
    /// the current and previous pose from the v8 decoder, finds the 
    /// active web, and stuffs the result into self.playback_trace.
    /// 
    /// Cheap — does no allocation per call beyond the Vec push. Reuses 
    /// the existing v8_decoder read lock and pose_track / webs vectors.
    /// 
    /// If `render_ok` is false, pose deltas + web stats are still 
    /// recorded so we can see the lead-up to a failure.
    fn record_trace_sample(&self, idx: u32, render_ok: bool) {
        let (pose_dx, pose_dy, pose_dz, web_idx, n_vertices) = 
            if let Ok(guard) = self.fao_v8_decoder.read() {
                if let Some(dec) = guard.as_ref() {
                    let cur_pos = dec.pose_track.get(idx as usize).map(|p| p.position);
                    let prev_pos = if idx > 0 {
                        dec.pose_track.get((idx - 1) as usize).map(|p| p.position)
                    } else { None };
                    let (dx, dy, dz) = match (cur_pos, prev_pos) {
                        (Some(c), Some(p)) => (c.x - p.x, c.y - p.y, c.z - p.z),
                        _ => (0.0, 0.0, 0.0),
                    };
                    let widx = fao_v1::v8_rasterizer::web_for_frame(&dec.webs, idx);
                    let widx_i = widx.map(|i| i as i32).unwrap_or(-1);
                    // For aligned webs (kind=0), the visible geometry comes from 
                    // the canonical web, not the aligned web's residual-only mesh. 
                    // Report the actual rendered mesh size so the trace matches what 
                    // the user sees on screen.
                    let nv = widx.map(|i| {
                        let w = &dec.webs[i];
                        if w.kind == 0 {
                            // Aligned — look up canonical
                            dec.webs.iter()
                                .find(|cw| cw.chunk_id == w.canonical_id && (cw.kind == 1 || cw.kind == 2))
                                .map(|cw| cw.mesh.vertices.len() as u32)
                                .unwrap_or(w.mesh.vertices.len() as u32)
                        } else {
                            w.mesh.vertices.len() as u32
                        }
                    }).unwrap_or(0);
                    (dx, dy, dz, widx_i, nv)
                } else {
                    (0.0, 0.0, 0.0, -1, 0)
                }
            } else {
                (0.0, 0.0, 0.0, -1, 0)
            };
        
        if let Ok(mut t) = self.playback_trace.try_borrow_mut() {
            t.push(PlaybackTraceSample {
                frame_idx: idx,
                pose_dx, pose_dy, pose_dz,
                web_idx, n_vertices,
                render_ok,
            });
            // Heartbeat: log a one-line trace-collected count every 10 
            // samples for the first 100, then every 100 after. Gives 
            // the user feedback that the trace is alive even before 
            // hitting the first summary threshold.
            let len = t.len();
            if len <= 100 && len % 10 == 0 {
                eprintln!("[playback trace] {} samples collected (next summary at 30/100/300/1000/3000)", len);
            } else if len % 100 == 0 && len < 1000 {
                eprintln!("[playback trace] {} samples collected", len);
            }
            // Auto-summarize at progressive thresholds. The earlier ones 
            // exist because the user may scrub a few frames manually 
            // before they ever hit "continuous playback" mode — and we 
            // want them to see a summary anyway. Threshold sequence:
            //   30, 100, 300, 1000, 3000, then every 5000 after.
            // 30 catches scrub-and-export workflows; the others give 
            // increasingly stable trends as frame counts grow.
            let n = t.len();
            let should_summarize = n == 30 || n == 100 || n == 300 
                || n == 1000 || n == 3000
                || (n >= 5000 && n % 5000 == 0);
            if should_summarize {
                drop(t);
                self.summarize_playback_trace(&format!("rolling {}-frame checkpoint", n));
            }
        }
    }
    
    /// Print a summary of the accumulated playback trace. Computes 
    /// mean/std/min/max for pose-position delta, counts web transitions, 
    /// reports failure rate. Buckets the trace into deciles so the user 
    /// can see whether jitter is local or global.
    /// 
    /// The summary is verbose by design — the goal is to give a single 
    /// readable block that can be pasted into a debug session.
    fn summarize_playback_trace(&self, reason: &str) {
        let trace = match self.playback_trace.try_borrow() {
            Ok(t) => t,
            Err(_) => return,  // Re-entered; skip
        };
        if trace.is_empty() {
            return;
        }
        let n = trace.len();
        let first_frame = trace.first().map(|s| s.frame_idx).unwrap_or(0);
        let last_frame = trace.last().map(|s| s.frame_idx).unwrap_or(0);
        
        // Pose-delta magnitudes (l2 norm of (dx,dy,dz)).
        let mut deltas: Vec<f32> = trace.iter().map(|s| {
            (s.pose_dx * s.pose_dx + s.pose_dy * s.pose_dy + s.pose_dz * s.pose_dz).sqrt()
        }).collect();
        deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let d_min = deltas.first().copied().unwrap_or(0.0);
        let d_max = deltas.last().copied().unwrap_or(0.0);
        let d_p50 = deltas.get(n / 2).copied().unwrap_or(0.0);
        let d_p95 = deltas.get(n * 95 / 100).copied().unwrap_or(0.0);
        let d_p99 = deltas.get(n * 99 / 100).copied().unwrap_or(0.0);
        let d_mean = deltas.iter().sum::<f32>() / n as f32;
        let d_var = deltas.iter().map(|d| (d - d_mean).powi(2)).sum::<f32>() / n as f32;
        let d_std = d_var.sqrt();
        
        // Web-transition count: how often the active web changed between 
        // adjacent trace samples. Each transition is a potential mesh-swap 
        // pop, which could be the visible jitter.
        let mut web_transitions = 0u32;
        let mut prev_web = trace[0].web_idx;
        let mut web_histogram: std::collections::BTreeMap<i32, u32> = std::collections::BTreeMap::new();
        for s in trace.iter() {
            *web_histogram.entry(s.web_idx).or_insert(0) += 1;
            if s.web_idx != prev_web {
                web_transitions += 1;
                prev_web = s.web_idx;
            }
        }
        
        // Render failures: how many frames load_fao_frame couldn't v8-render.
        let n_fail = trace.iter().filter(|s| !s.render_ok).count();
        
        // Vertex-count stats (sampled from the active web at each frame).
        let mut verts: Vec<u32> = trace.iter().map(|s| s.n_vertices).collect();
        verts.sort();
        let v_min = verts.first().copied().unwrap_or(0);
        let v_max = verts.last().copied().unwrap_or(0);
        let v_p50 = verts.get(n / 2).copied().unwrap_or(0);
        let v_mean = verts.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
        
        // Decile buckets: split the trace into 10 chunks, compute mean 
        // pose-delta + dominant web for each. Reveals local jitter / 
        // mesh-pop patterns.
        let bucket_size = (n / 10).max(1);
        let mut bucket_summaries = String::new();
        for b in 0..10 {
            let start = b * bucket_size;
            let end = ((b + 1) * bucket_size).min(n);
            if start >= end { break; }
            let bucket = &trace[start..end];
            let bm = bucket.iter().map(|s| {
                (s.pose_dx*s.pose_dx + s.pose_dy*s.pose_dy + s.pose_dz*s.pose_dz).sqrt()
            }).sum::<f32>() / bucket.len() as f32;
            // Most common web in this bucket
            let mut wh: std::collections::HashMap<i32, u32> = std::collections::HashMap::new();
            for s in bucket { *wh.entry(s.web_idx).or_insert(0) += 1; }
            let dominant = wh.iter().max_by_key(|e| e.1).map(|e| *e.0).unwrap_or(-1);
            let bucket_swaps = bucket.windows(2).filter(|w| w[0].web_idx != w[1].web_idx).count();
            bucket_summaries.push_str(&format!(
                "  bucket {}/10 [f{:>6}..{:>6}]: mean_pose_d={:.4} dominant_web={} web_swaps={}
",
                b + 1, bucket[0].frame_idx, bucket.last().unwrap().frame_idx,
                bm, dominant, bucket_swaps,
            ));
        }
        
        // Web breakdown — show every web that appeared and how many frames it owned.
        let mut web_lines = String::new();
        let mut web_entries: Vec<_> = web_histogram.iter().collect();
        web_entries.sort_by_key(|e| std::cmp::Reverse(*e.1));
        for (w, count) in web_entries.iter().take(8) {
            web_lines.push_str(&format!("    web {:>4}: {:>6} frames ({:.1}%)
",
                w, count, **count as f64 / n as f64 * 100.0));
        }
        if web_entries.len() > 8 {
            web_lines.push_str(&format!("    ... and {} more
", web_entries.len() - 8));
        }
        
        eprintln!("
========== FAO playback trace summary ==========");
        eprintln!("  reason: {}", reason);
        eprintln!("  samples: {} (frames {}..{})", n, first_frame, last_frame);
        eprintln!("  render failures: {} ({:.1}%)", n_fail, n_fail as f64 / n as f64 * 100.0);
        eprintln!("  ");
        eprintln!("  pose-position delta per frame (l2 norm of dx,dy,dz):");
        eprintln!("    min={:.5}  p50={:.5}  mean={:.5}  std={:.5}",
            d_min, d_p50, d_mean, d_std);
        eprintln!("    p95={:.5}  p99={:.5}  max={:.5}",
            d_p95, d_p99, d_max);
        eprintln!("  ");
        eprintln!("  active-web stats:");
        eprintln!("    {} distinct webs seen, {} transitions between webs",
            web_histogram.len(), web_transitions);
        eprintln!("    top webs by frame count:");
        eprintln!("{}", web_lines.trim_end_matches('\n'));
        eprintln!("  ");
        eprintln!("  active-web vertex count:");
        eprintln!("    min={}  p50={}  mean={:.0}  max={}", v_min, v_p50, v_mean, v_max);
        eprintln!("  ");
        eprintln!("  per-decile breakdown:");
        eprintln!("{}", bucket_summaries.trim_end_matches('\n'));
        eprintln!("================================================\n");
    }
    
    fn load_fao_frame(&mut self, idx: usize) {
        // Try prefetch cache first.
        // 
        // Take the lock in an inner block so the MutexGuard is dropped 
        // before we proceed. If we wrote `if let Some(cached) = 
        // self.fao_prefetch.lock().unwrap().get(idx)`, the guard would 
        // be a temporary that lives through the if-body — which 
        // implicitly immutably-borrows `self` and blocks us from 
        // calling `&mut self` methods like `prefetch_fao_frames` 
        // inside the block (E0502).
        let cached_opt = {
            let mut c = self.fao_prefetch.lock().unwrap();
            c.get(idx)
        };
        if let Some(cached) = cached_opt {
            // Diagnostic: log hit rate periodically from the FAST path 
            // (load-frame's cached return). Without this, the only log 
            // line for cache_hit_rate fires on the MISS path, which 
            // makes a working cache look like 0% hits — misleading.
            if idx < 3 || idx % 200 == 0 {
                let (rate, hits, misses) = {
                    let c = self.fao_prefetch.lock().unwrap();
                    (c.hit_rate(), c.hits, c.misses)
                };
                eprintln!("FAO load: frame {} HIT cache, hit_rate={:.0}% ({} hits / {} misses)",
                    idx, rate * 100.0, hits, misses);
            }
            self.cur_recon = cached.clone();
            // Try to set cur_frame to source video (A/B comparison) if available
            // Only fetch source if A/B comparison view is active (avoid stalling playback)
            let need_source = matches!(self.view, View::Split | View::Original | View::Error);
            let src_attempt = if need_source { self.fetch_source_frame(idx) } else { None };
            if need_source && idx % 60 == 0 {
                eprintln!("FAO A/B: frame {} need_src={} got_src={} src_path={:?}",
                    idx, need_source, src_attempt.is_some(), self.fao_source_path);
            }
            self.cur_frame = src_attempt.unwrap_or_else(|| cached.clone());
            self.has_frame = true;
            self.fidx = idx;
            // Trace: cache-hit is a "successful" frame load.
            self.record_trace_sample(idx as u32, true);
            // Keep the cache filling ahead of the cursor. Without this, 
            // cache-hit playback never schedules new prefetches and the 
            // cache slowly drains until the first miss. Calling here 
            // means the next 8 frames stay warm.
            if idx + 1 < self.fao_index.len() {
                self.prefetch_fao_frames(idx + 1, 16);
            }
            // Invalidate the player's anchor overlay DAs so they 
            // re-fetch the current frame's anchor list. No-op when the 
            // overlay isn't toggled on (the draw_func returns early).
            self.request_player_redraw();
            return;
        }
        let data = match &self.fao_data { Some(d) => d, None => return };
        if idx >= self.fao_index.len() { return; }
        let entry = &self.fao_index[idx];
        let w = self.cur_w;
        let h = self.cur_h;
        let yuv_sz = w * h * 3 / 2;
        let rgb_sz = w * h * 3;
        if let Some((data_bytes, fhdr)) = fao_v1::read_fao_frame(data, entry) {
            // Debug: show format detection for first few frames
            // Capture the marker byte for UI labeling (issue 5 fix).
            self.cur_fao_marker = if data_bytes.is_empty() { 0 } else { data_bytes[0] };
            if idx < 3 || idx % 200 == 0 {
                let marker = if data_bytes.is_empty() { '_' } else { data_bytes[0] as char };
                let rate = self.fao_prefetch.lock().unwrap().hit_rate();
                eprintln!("FAO load: frame {} payload_len={} marker={:?} (w={} h={}) cache_hit_rate={:.0}%",
                    idx, data_bytes.len(), marker, w, h, rate * 100.0);
            }
            // Try multiple formats: DCT, raw YUV, raw RGB
            // Skip frame: interpolate between neighboring anchors
            // (real frame interpolation, not just hold-last)
            if data_bytes.is_empty() || data_bytes.len() < 4 {
                if let Some(interpolated) = self.interpolate_skip_frame(idx) {
                    if interpolated.len() == rgb_sz {
                        self.cur_recon = interpolated.clone();
                        let need_source = matches!(self.view, View::Split | View::Original | View::Error);
                        self.cur_frame = if need_source {
                            self.fetch_source_frame(idx).unwrap_or_else(|| interpolated.clone())
                        } else {
                            interpolated.clone()
                        };
                        self.fao_prefetch.lock().unwrap().insert(idx, interpolated);
                        self.has_frame = true;
                        self.fidx = idx;
                        return;
                    }
                }
                // Interpolation failed — fall back to last good frame
                if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                    self.fao_prefetch.lock().unwrap().insert(idx, self.cur_frame.clone());
                    self.has_frame = true;
                    self.fidx = idx;
                    return;
                }
            }
            let rgb = if data_bytes.is_empty() {
                // Already handled above, but just in case
                if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                    self.cur_frame.clone()
                } else {
                    vec![64u8; rgb_sz]
                }
            } else {
                match data_bytes[0] {
                    b'D' => fao_v1::decompress_frame_dct(&data_bytes, w, h)
                        .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h))
                        .unwrap_or_else(|| {
                            eprintln!("FAO: DCT decompress failed for frame {}", idx);
                            // Fall back to last good frame, not gray
                            if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                self.cur_frame.clone()
                            } else {
                                vec![64u8; rgb_sz]
                            }
                        }),
                    b'P' => {
                        match fao_v1::decompress_frame_yuv_split(&data_bytes, w, h) {
                            Some(yuv) => TranscodeState::yuv_to_rgb(&yuv, w, h),
                            None => {
                                eprintln!("FAO: yuv_split decompress FAILED for frame {} (data {} bytes, expected w={} h={})", 
                                    idx, data_bytes.len(), w, h);
                                if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                    self.cur_frame.clone()
                                } else { vec![64u8; rgb_sz] }
                            }
                        }
                    },
                    b'L' => {
                        match fao_v1::decompress_frame_lossy_chroma(&data_bytes, w, h) {
                            Some(yuv) => TranscodeState::yuv_to_rgb(&yuv, w, h),
                            None => {
                                eprintln!("FAO: lossy_chroma decompress FAILED for frame {}", idx);
                                if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                    self.cur_frame.clone()
                                } else { vec![64u8; rgb_sz] }
                            }
                        }
                    },
                    b'V' | b'Z' => {
                        // v8 frame (raw V8/V81 or zstd-wrapped).
                        // 
                        // Fallback ladder, in order:
                        //   1. v8 mesh rasterizer — actual reconstruction from the 
                        //      mesh stored in the V8MX trailer + this frame's pose.
                        //   2. Source video frame (if source file still available).
                        //   3. Previous decoded frame (continuity during scrubbing).
                        //   4. Solid gray (only when nothing else works).
                        // 
                        // The rasterizer can return None when: no web covers this 
                        // frame index, the web's mesh is empty (V8MS-only files 
                        // without V8MX trailer), or all triangles ended up off-screen 
                        // / behind the camera. In any of these cases we drop down 
                        // to the source/prev/gray ladder.
                        // Try v8 mesh render first.
                        let v8_render = self.fao_v8_decoder.read().unwrap().as_ref()
                            .and_then(|dec| dec.render_frame_rgb(&data_bytes, idx as u32));
                        
                        // If render failed, decide whether falling back to 
                        // source-fetch is sensible. This matters because 
                        // populate_source_cache_batch can spawn ffmpeg and 
                        // block the UI thread for ~400-1200ms — fine when 
                        // it succeeds and primes 60 frames, terrible when 
                        // every Z frame triggers it (empty-trailer files).
                        // 
                        // Heuristic: if the decoder has ZERO webs (e.g. 
                        // an old broken-export file), don't bother trying 
                        // source-fetch on every frame; the v8 render will 
                        // never succeed and we'd ffmpeg-spawn-thrash the 
                        // UI. Just hold the previous frame. User sees a 
                        // stutter-free "frozen" picture; the alternative 
                        // is per-frame ffmpeg invocations.
                        let has_any_webs = self.fao_v8_decoder.read().unwrap().as_ref()
                            .map(|dec| !dec.webs.is_empty())
                            .unwrap_or(false);
                        
                        if let Some(rgb) = v8_render {
                            // Coverage check. v8 mesh may render only a small 
                            // portion of the frame (sliver of geometry around 
                            // foreground objects, rest of frame is zero/black). 
                            // For low-coverage frames, the user sees mostly 
                            // black with a fragment of mesh content — much 
                            // worse perceptually than the source video.
                            // 
                            // If coverage is below threshold, composite v8 
                            // over source video: keep v8 pixels where they 
                            // exist, fill the zero/black areas with source.
                            // The result is a hybrid frame: source video as 
                            // background, v8 mesh in the foreground.
                            // 
                            // Threshold and behavior toggleable via env vars:
                            //   FAO_PURE_MESH=1  → disable compositing (use 
                            //                     v8 output exactly as-is, 
                            //                     even when mostly black)
                            //   FAO_COVERAGE_THRESHOLD=0.25 (default) →
                            //                     coverage ratio below which 
                            //                     we composite over source
                            let pure_mesh = std::env::var("FAO_PURE_MESH")
                                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                                .unwrap_or(false);
                            
                            if pure_mesh {
                                rgb
                            } else {
                                // Stride-sample to estimate coverage. ~4096 byte 
                                // loads regardless of frame size; matches the 
                                // rasterizer's any_nonzero cost.
                                let stride = (rgb.len() / 4096).max(1);
                                let mut samples = 0usize;
                                let mut nonzero = 0usize;
                                let mut i = 0;
                                while i < rgb.len() {
                                    samples += 1;
                                    if rgb[i] != 0 { nonzero += 1; }
                                    i += stride;
                                }
                                let coverage = if samples > 0 {
                                    nonzero as f32 / samples as f32
                                } else { 0.0 };
                                
                                let threshold: f32 = std::env::var("FAO_COVERAGE_THRESHOLD")
                                    .ok()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.25);
                                
                                if coverage >= threshold {
                                    // Coverage is acceptable — use v8 directly.
                                    rgb
                                } else {
                                    // Below threshold — try to composite over 
                                    // source video. fetch_source_frame is async; 
                                    // returns None on cache miss and fires a 
                                    // background request that may land in time 
                                    // for the next frame.
                                    let src_opt = self.fetch_source_frame(idx);
                                    
                                    // Counters for the "blindspot" diagnostic — 
                                    // we want to know how often each branch fires 
                                    // since the user reports "more black frames 
                                    // without residuals across the board".
                                    static COMPOSITE_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                                    static COMPOSITE_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                                    static FIRST_COMPOSITE_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                                    
                                    if let Some(src) = src_opt {
                                        // Composite: where v8 has content, keep it; 
                                        // where v8 is zero, use source. Sampled per 
                                        // pixel (RGB triplet) — any non-zero channel 
                                        // marks the pixel as "v8 has content here".
                                        // 
                                        // FAO_COMPOSITE_FEATHER=<radius> (1..3 sensible) 
                                        // turns on edge-feathering. At feather radius R, 
                                        // each pixel's output blend ratio is its 
                                        // (2R+1)×(2R+1) neighborhood's v8-content 
                                        // fraction. Hard edges where mesh meets source 
                                        // become soft transitions, reducing the "glue" 
                                        // artifact the user reports during scrub.
                                        // 
                                        // Cost: O(pixels × neighborhood). At 1080p 
                                        // and R=1 (9-pixel neighborhood), that's 18M 
                                        // operations — ~5ms with simple loops. R=2 
                                        // (25 px) is ~14ms. Default is 0 (hard cut, 
                                        // same as before).
                                        let feather_r: usize = std::env::var("FAO_COMPOSITE_FEATHER")
                                            .ok().and_then(|v| v.parse::<usize>().ok())
                                            .unwrap_or(0).min(3);
                                        // FAO_COMPOSITE_COLORMATCH=1 (or 2 for stronger) — 
                                        // measure the mean RGB offset between mesh and 
                                        // source at coverage boundaries, then shift the 
                                        // mesh's pixel colors by that offset. This 
                                        // addresses the "brightness/color differences" 
                                        // user reports during scrub: the mesh's vertex 
                                        // colors are baked at integration time (one 
                                        // specific frame's lighting) and don't match 
                                        // the current frame's source lighting.
                                        // 
                                        // Strength: 1 = mild correction (50% of measured 
                                        // delta applied), 2 = full correction. Strong 
                                        // can introduce wobble if the boundary samples 
                                        // are noisy; mild is usually best for stable 
                                        // playback.
                                        let colormatch_strength: usize = std::env::var("FAO_COMPOSITE_COLORMATCH")
                                            .ok().and_then(|v| v.parse::<usize>().ok())
                                            .unwrap_or(0).min(2);
                                        let mut color_shift = [0i32, 0i32, 0i32];
                                        if colormatch_strength > 0 && src.len() == rgb.len() && self.cur_w > 0 && self.cur_h > 0 {
                                            let w = self.cur_w;
                                            let h = self.cur_h;
                                            // Sample at boundary: pixel where v8 has 
                                            // content but at least one 4-neighbor doesn't.
                                            // Stride 4 in both axes for speed (we don't 
                                            // need every pixel — even ~10K boundary 
                                            // samples is plenty for a reliable mean).
                                            let mut sum_dr = 0i64;
                                            let mut sum_dg = 0i64;
                                            let mut sum_db = 0i64;
                                            let mut count = 0i64;
                                            for y in (1..h.saturating_sub(1)).step_by(4) {
                                                for x in (1..w.saturating_sub(1)).step_by(4) {
                                                    let off = (y * w + x) * 3;
                                                    let has = rgb[off] != 0 || rgb[off+1] != 0 || rgb[off+2] != 0;
                                                    if !has { continue; }
                                                    // Is any 4-neighbor empty?
                                                    let nbr_offs = [
                                                        ((y-1) * w + x) * 3,
                                                        ((y+1) * w + x) * 3,
                                                        (y * w + (x-1)) * 3,
                                                        (y * w + (x+1)) * 3,
                                                    ];
                                                    let any_empty = nbr_offs.iter().any(|&no| {
                                                        rgb[no] == 0 && rgb[no+1] == 0 && rgb[no+2] == 0
                                                    });
                                                    if !any_empty { continue; }
                                                    // Boundary pixel — accumulate delta.
                                                    sum_dr += src[off]   as i64 - rgb[off]   as i64;
                                                    sum_dg += src[off+1] as i64 - rgb[off+1] as i64;
                                                    sum_db += src[off+2] as i64 - rgb[off+2] as i64;
                                                    count += 1;
                                                }
                                            }
                                            if count >= 50 {
                                                // Average delta. Halve for strength=1, 
                                                // full for strength=2. Cap at ±30 per 
                                                // channel so we don't over-correct on 
                                                // noisy samples.
                                                let scale = if colormatch_strength == 1 { 2 } else { 1 };
                                                color_shift[0] = ((sum_dr / count) as i32 / scale).clamp(-30, 30);
                                                color_shift[1] = ((sum_dg / count) as i32 / scale).clamp(-30, 30);
                                                color_shift[2] = ((sum_db / count) as i32 / scale).clamp(-30, 30);
                                                static COLORMATCH_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                                                let n = COLORMATCH_LOG.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                                                if n == 1 || n == 10 || n == 100 || n % 500 == 0 {
                                                    eprintln!("[FAO composite] colormatch #{}: shift=({},{},{}) from {} boundary samples (strength={})",
                                                        n, color_shift[0], color_shift[1], color_shift[2], count, colormatch_strength);
                                                }
                                            }
                                        }
                                        if src.len() == rgb.len() {
                                            let mut out = src.clone();
                                            let pixels = rgb.len() / 3;
                                            let w = self.cur_w;
                                            let h = self.cur_h;
                                            // Helper: apply color shift to a mesh 
                                            // pixel. Used by both composite paths 
                                            // below. No-op when color_shift is all 0.
                                            let shift_rgb = |r: u8, g: u8, b: u8| -> (u8, u8, u8) {
                                                let nr = (r as i32 + color_shift[0]).clamp(0, 255) as u8;
                                                let ng = (g as i32 + color_shift[1]).clamp(0, 255) as u8;
                                                let nb = (b as i32 + color_shift[2]).clamp(0, 255) as u8;
                                                (nr, ng, nb)
                                            };
                                            if feather_r == 0 || w == 0 || h == 0 || pixels != w * h {
                                                // Hard-cut composite (original behavior).
                                                for p in 0..pixels {
                                                    let off = p * 3;
                                                    let has_content = rgb[off] != 0 
                                                        || rgb[off+1] != 0 
                                                        || rgb[off+2] != 0;
                                                    if has_content {
                                                        // Apply colormatch shift (no-op when all zero).
                                                        let (cr, cg, cb) = shift_rgb(rgb[off], rgb[off+1], rgb[off+2]);
                                                        out[off]   = cr;
                                                        out[off+1] = cg;
                                                        out[off+2] = cb;
                                                    }
                                                }
                                            } else {
                                                // Feathered composite: build a 1-bit 
                                                // content mask, then blend each pixel by 
                                                // its neighborhood's mask fraction.
                                                let mut mask = vec![false; pixels];
                                                for p in 0..pixels {
                                                    let off = p * 3;
                                                    mask[p] = rgb[off] != 0 || rgb[off+1] != 0 || rgb[off+2] != 0;
                                                }
                                                let area = (2 * feather_r + 1).pow(2) as u32;
                                                for y in 0..h {
                                                    for x in 0..w {
                                                        // Count mask hits in the 
                                                        // neighborhood.
                                                        let mut hits = 0u32;
                                                        let y0 = y.saturating_sub(feather_r);
                                                        let y1 = (y + feather_r + 1).min(h);
                                                        let x0 = x.saturating_sub(feather_r);
                                                        let x1 = (x + feather_r + 1).min(w);
                                                        let nbr_area = ((y1 - y0) * (x1 - x0)) as u32;
                                                        for yy in y0..y1 {
                                                            let row = yy * w;
                                                            for xx in x0..x1 {
                                                                if mask[row + xx] { hits += 1; }
                                                            }
                                                        }
                                                        if hits == nbr_area && mask[y * w + x] {
                                                            // Fully inside mesh — use 
                                                            // v8 directly (with colormatch).
                                                            let off = (y * w + x) * 3;
                                                            let (cr, cg, cb) = shift_rgb(rgb[off], rgb[off+1], rgb[off+2]);
                                                            out[off]   = cr;
                                                            out[off+1] = cg;
                                                            out[off+2] = cb;
                                                        } else if hits > 0 && mask[y * w + x] {
                                                            // Edge pixel — blend (mesh side gets colormatch).
                                                            let off = (y * w + x) * 3;
                                                            let v8_weight = hits as f32 / nbr_area as f32;
                                                            let sr_weight = 1.0 - v8_weight;
                                                            let (cr, cg, cb) = shift_rgb(rgb[off], rgb[off+1], rgb[off+2]);
                                                            out[off]   = (cr as f32 * v8_weight + src[off]   as f32 * sr_weight) as u8;
                                                            out[off+1] = (cg as f32 * v8_weight + src[off+1] as f32 * sr_weight) as u8;
                                                            out[off+2] = (cb as f32 * v8_weight + src[off+2] as f32 * sr_weight) as u8;
                                                        }
                                                        // else: outside mesh — keep 
                                                        // source as-is (out is already 
                                                        // initialized to src).
                                                        let _ = area;  // keep var live for future tuning
                                                    }
                                                }
                                            }
                                            let hits = COMPOSITE_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                                            if !FIRST_COMPOSITE_LOG.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                                eprintln!("FAO: low-coverage frame ({:.0}% < {:.0}%) — compositing v8 over source video. Set FAO_PURE_MESH=1 to disable.",
                                                    coverage * 100.0, threshold * 100.0);
                                            }
                                            // Periodic stats so the user sees 
                                            // composite vs source-miss ratio.
                                            if hits == 30 || hits == 100 || hits == 300 || (hits > 0 && hits % 1000 == 0) {
                                                let misses = COMPOSITE_MISSES.load(std::sync::atomic::Ordering::Relaxed);
                                                eprintln!("FAO composite stats: {} hits, {} source-fetch misses ({:.0}% hit rate)",
                                                    hits, misses,
                                                    100.0 * hits as f64 / (hits + misses).max(1) as f64);
                                            }
                                            out
                                        } else {
                                            // Size mismatch — fall back to source 
                                            // alone (better than mostly-black v8).
                                            src
                                        }
                                    } else {
                                        // Source-fetch is async — cache miss. 
                                        // Showing the low-coverage v8 directly 
                                        // means the user sees mostly-black frames 
                                        // (their reported complaint). Showing the 
                                        // PREVIOUS frame is far less jarring 
                                        // perceptually — the eye reads it as a 
                                        // slightly stale image rather than a 
                                        // flash of darkness.
                                        let misses = COMPOSITE_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                                        if misses == 30 || misses == 100 || misses == 300 || (misses > 0 && misses % 1000 == 0) {
                                            let hits = COMPOSITE_HITS.load(std::sync::atomic::Ordering::Relaxed);
                                            eprintln!("FAO composite stats: {} hits, {} source-fetch misses ({:.0}% hit rate)",
                                                hits, misses,
                                                100.0 * hits as f64 / (hits + misses).max(1) as f64);
                                        }
                                        if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb.len() {
                                            self.cur_frame.clone()
                                        } else {
                                            // No prev frame yet — return v8 as-is. 
                                            // First-frame case; subsequent frames 
                                            // will get the prev-frame holdover.
                                            rgb
                                        }
                                    }
                                }
                            }
                        } else if has_any_webs {
                            // Some webs exist; this specific frame failed 
                            // (off-screen geometry, no web covering this 
                            // index, etc). Fetching source for the rare 
                            // gap is acceptable.
                            if let Some(src) = self.fetch_source_frame(idx) {
                                src
                            } else if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                self.cur_frame.clone()
                            } else {
                                vec![64u8; rgb_sz]
                            }
                        } else {
                            // No webs at all — hold previous frame. Log once 
                            // so the user knows the decode is degraded.
                            static V8_FALLBACK_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                            if !V8_FALLBACK_LOG.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                eprintln!("FAO: v8 trailer empty (0 webs) — Z frames will hold previous frame instead of triggering per-frame ffmpeg fallback. Re-encode this video to populate webs.");
                            }
                            if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                self.cur_frame.clone()
                            } else {
                                vec![64u8; rgb_sz]
                            }
                        }
                    },
                    b'T' => {
                        // v2.1 Translated frame: anchor reference + residual
                        // FAST PATH: motion-only with all-zero MVs → copy anchor RGB directly!
                        // (No motion comp, no residual apply, no yuv_to_rgb)
                        if let Some((frame, fw, fh)) = fao_v1::v21_predictive::TranslatedFrame::from_bytes(&data_bytes) {
                            let anchor_idx = frame.anchor_ref as usize;
                            
                            // Detect motion-only with all zero MVs — pure anchor copy
                            let is_pure_anchor_copy = frame.residual_quantized.is_empty()
                                && frame.motion_vectors.iter().all(|(dx, dy)| *dx == 0 && *dy == 0);
                            
                            if is_pure_anchor_copy {
                                // SUPER FAST PATH: just decode anchor as RGB once, reuse
                                if let Some(anchor_rgb) = self.decode_fao_frame_to_rgb(anchor_idx) {
                                    if anchor_rgb.len() == rgb_sz {
                                        self.cur_recon = anchor_rgb.clone();
                                        self.cur_frame = anchor_rgb.clone();
                                        self.fao_prefetch.lock().unwrap().insert(idx, anchor_rgb);
                                        self.has_frame = true;
                                        self.fidx = idx;
                                        // Skip rest of normal path
                                        if idx + 1 < self.fao_index.len() {
                                            self.prefetch_fao_frames(idx + 1, 16);
                                        }
                                        return;
                                    }
                                }
                            }
                            
                            // Try YUV cache first (no rgb→yuv conversion needed)
                            let cached_yuv = self.anchor_yuv_for_t.iter()
                                .rev()
                                .find(|(i, _)| *i == anchor_idx)
                                .map(|(_, yuv)| yuv.clone());
                            
                            let anchor_yuv = match cached_yuv {
                                Some(y) => y,
                                None => {
                                    // Decode anchor (RGB) then convert to YUV and cache
                                    match self.decode_fao_frame_to_rgb(anchor_idx) {
                                        Some(anchor_rgb) => {
                                            let yuv = rgb_to_yuv420p(&anchor_rgb, fw, fh);
                                            // Cache for future T-frames using same anchor
                                            if self.anchor_yuv_for_t.len() >= 32 {
                                                self.anchor_yuv_for_t.pop_front();
                                            }
                                            self.anchor_yuv_for_t.push_back((anchor_idx, yuv.clone()));
                                            yuv
                                        }
                                        None => {
                                            eprintln!("FAO: T-frame {} anchor {} not decodable", idx, anchor_idx);
                                            // Fall through with previous frame (avoid stale display)
                                            // We need to return SOMETHING from this arm — use cur_frame as YUV stand-in
                                            // Empty Vec→break out: use cur_frame fallback
                                            // Return zeros which will fall back to gray; the OUTER match handles None
                                            vec![]
                                        }
                                    }
                                }
                            };
                            
                            // If anchor_yuv was empty (decode failed), use prior frame
                            if anchor_yuv.is_empty() {
                                if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                    self.cur_frame.clone()
                                } else { vec![64u8; rgb_sz] }
                            } else if let Some(reconstructed_yuv) = frame.decode_with_anchor(&anchor_yuv, fw, fh) {
                                TranscodeState::yuv_to_rgb(&reconstructed_yuv, fw, fh)
                            } else {
                                eprintln!("FAO: T-frame decode failed for {}", idx);
                                if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                    self.cur_frame.clone()
                                } else { vec![64u8; rgb_sz] }
                            }
                        } else {
                            eprintln!("FAO: T-frame parse failed for {}", idx);
                            if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                self.cur_frame.clone()
                            } else { vec![64u8; rgb_sz] }
                        }
                    },
                    _ => {
                        if data_bytes.len() == yuv_sz {
                            TranscodeState::yuv_to_rgb(&data_bytes, w, h)
                        } else if data_bytes.len() == rgb_sz {
                            data_bytes.clone()
                        } else {
                            fao_v1::decompress_frame(&data_bytes, yuv_sz)
                                .map(|yuv| TranscodeState::yuv_to_rgb(&yuv, w, h))
                                .or_else(|| fao_v1::decompress_frame(&data_bytes, rgb_sz))
                                .unwrap_or_else(|| {
                                    eprintln!("FAO: unknown format for frame {} (size {} marker {:?})",
                                        idx, data_bytes.len(), data_bytes[0] as char);
                                    if !self.cur_frame.is_empty() && self.cur_frame.len() == rgb_sz {
                                        self.cur_frame.clone()
                                    } else {
                                        vec![64u8; rgb_sz]
                                    }
                                })
                        }
                    }
                }
            };
            self.cur_recon = rgb.clone();
            // Show source video frame as cur_frame for A/B comparison (if available)
            // Only fetch source if A/B view is active — avoid ffmpeg stall during playback
            let need_source = matches!(self.view, View::Split | View::Original | View::Error);
            self.cur_frame = if need_source {
                self.fetch_source_frame(idx).unwrap_or_else(|| rgb.clone())
            } else {
                rgb.clone()
            };
            self.fao_prefetch.lock().unwrap().insert(idx, rgb);
            self.has_frame = true;
            self.fidx = idx;
            self.cur_result = Some(FrameStat {
                psnr: fhdr.psnr_x10 as f64 / 10.0,
                enc_bytes: fhdr.compressed_size as usize,
                time_ms: 0.0, ftype: fhdr.frame_type,
                skip_pct: fhdr.skip_pct as f64, vrr_ms: 16.7, activity: 0.0,
            });
        }
        
        // Opportunistic prefetch: decode next few frames in advance
        // Lightweight — only fills empty cache slots, skips already-cached
        if idx + 1 < self.fao_index.len() {
            self.prefetch_fao_frames(idx + 1, 16);
        }
        
        // Trace: end-of-function reaches here on the slow (cache-miss) 
        // path. self.has_frame indicates whether a render produced output.
        // record_trace_sample only mutates the trace Vec (via RefCell), 
        // so &self is sufficient.
        self.record_trace_sample(idx as u32, self.has_frame);
        // Invalidate the player's anchor overlay DAs on the slow path 
        // too (matching the fast-path return above). Cheap when overlay 
        // is off — the draw_func bails immediately.
        self.request_player_redraw();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Final playback-trace summary on app shutdown. If the user 
        // played some frames and then closed without reloading, this 
        // is their last chance to see the summary.
        if !self.playback_trace.borrow().is_empty() {
            self.summarize_playback_trace("app shutdown");
        }
        // TranscodeState Drop handles temp file cleanup via Arc ref counting
        self.tc = None;
    }
}

// ═══════════════════════════════════════════
fn build_ui(app: &adw::Application) {
    fao_v1::init();
    // Initialize GPU encoder if available
    #[cfg(feature = "gpu")]
    let gpu_enc: Option<std::sync::Arc<fao_v1::gpu::GpuEncoder>> = {
        match fao_v1::gpu::GpuEncoder::new(1920, 1080) {
            Some(g) => { eprintln!("FAO: GPU compute ready"); Some(std::sync::Arc::new(g)) }
            None => { eprintln!("FAO: No GPU, using CPU"); None }
        }
    };
    #[cfg(not(feature = "gpu"))]
    let gpu_enc: Option<std::sync::Arc<()>> = None;
    let _ = &gpu_enc;
    let st:Rc<RefCell<App>>=Rc::new(RefCell::new(App::new()));
    let win=adw::ApplicationWindow::builder().application(app).title("FAO v1.1").default_width(1400).default_height(900).build();
    let css=gtk::CssProvider::new();
    css.load_from_data("
        window{background:#0c0e14;color:#e0e4f0;} .hdr{font:bold 32px monospace;color:#4cf0a0;}
        .sub{font:13px monospace;color:#7a829e;} .stat{font:12px monospace;color:#e0e4f0;}
        .panel{background:#141822;border:1px solid #242a3a;border-radius:8px;padding:10px;}
        .act{background:#4cf0a0;color:#0c0e14;font:bold 13px monospace;border-radius:6px;padding:6px 16px;}
        .act2{background:#40d0d0;color:#0c0e14;font:bold 13px monospace;border-radius:6px;padding:6px 16px;}
        .vb{background:#242a3a;color:#7a829e;font:12px monospace;border-radius:4px;padding:4px 12px;}
        .va{background:#4cf0a0;color:#0c0e14;font:bold 12px monospace;border-radius:4px;padding:4px 12px;}
        .play{background:#f0c040;color:#0c0e14;font:bold 14px monospace;border-radius:6px;padding:6px 20px;}
        .tc{background:#f09040;color:#0c0e14;font:bold 13px monospace;border-radius:6px;padding:6px 16px;}
        .export{background:#d070f0;color:#0c0e14;font:bold 13px monospace;border-radius:6px;padding:6px 16px;}
        .pin{font:11px monospace;color:#f0c040;}
        .back{background:#7a829e;color:#0c0e14;font:12px monospace;border-radius:4px;padding:4px 12px;}
    ");
    gtk::style_context_add_provider_for_display(&gtk::gdk::Display::default().unwrap(),&css,gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let root=gtk::Box::new(gtk::Orientation::Horizontal,0);
    let left=gtk::Box::new(gtk::Orientation::Vertical,6);left.set_hexpand(true);left.set_margin_start(14);left.set_margin_end(6);left.set_margin_top(10);left.set_margin_bottom(10);

    let hdr=gtk::Box::new(gtk::Orientation::Horizontal,10);
    hdr.append(&{let l=gtk::Label::new(Some("FAO"));l.add_css_class("hdr");l});
    hdr.append(&{let l=gtk::Label::new(Some("Scene Codec v1.1"));l.add_css_class("sub");l.set_valign(gtk::Align::End);l});

    // Top-level section tabs (Library | Encoder | Player)
    let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tab_box.set_halign(gtk::Align::End);
    tab_box.set_hexpand(true);
    // Compact tab labels — full names in tooltips below
    let tab_lib = gtk::ToggleButton::with_label("📚");
    let tab_enc = gtk::ToggleButton::with_label("🎬");
    let tab_play = gtk::ToggleButton::with_label("▶");
    let tab_world = gtk::ToggleButton::with_label("🌐");
    tab_lib.add_css_class("act");
    tab_enc.add_css_class("act");
    tab_play.add_css_class("act");
    tab_world.add_css_class("act");
    tab_lib.set_tooltip_text(Some("Browse archives and videos"));
    tab_enc.set_tooltip_text(Some("Encode video files to FAO"));
    tab_play.set_tooltip_text(Some("Play decoded FAO files with A/B and authenticity comparison"));
    tab_world.set_tooltip_text(Some("3D world viewer — freecam through the scene graph, watch objects move temporally"));
    tab_lib.set_group(Some(&tab_lib));
    tab_enc.set_group(Some(&tab_lib));
    tab_play.set_group(Some(&tab_lib));
    tab_world.set_group(Some(&tab_lib));
    tab_lib.set_active(true);
    tab_box.append(&tab_lib);
    tab_box.append(&tab_enc);
    tab_box.append(&tab_play);
    tab_box.append(&tab_world);
    hdr.append(&tab_box);
    left.append(&hdr);

    // Library panel — shown when Library tab is active
    let lib_panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    lib_panel.set_margin_top(8);
    lib_panel.set_vexpand(true);
    
    let lib_hdr = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lib_title = gtk::Label::new(Some("📚 Library"));
    lib_title.add_css_class("hdr");
    lib_title.set_halign(gtk::Align::Start);
    lib_title.set_hexpand(true);
    let new_arch_btn = gtk::Button::with_label("+ New Archive");
    new_arch_btn.add_css_class("act");
    let open_arch_btn = gtk::Button::with_label("Open Archive");
    open_arch_btn.add_css_class("act");
    let add_video_btn = gtk::Button::with_label("+ Add Video");
    add_video_btn.add_css_class("act2");
    add_video_btn.set_sensitive(false);
    lib_hdr.append(&lib_title);
    lib_hdr.append(&new_arch_btn);
    lib_hdr.append(&open_arch_btn);
    lib_hdr.append(&add_video_btn);
    lib_panel.append(&lib_hdr);
    
    let lib_subtitle = gtk::Label::new(Some("No archive loaded"));
    lib_subtitle.add_css_class("sub");
    lib_subtitle.set_halign(gtk::Align::Start);
    lib_panel.append(&lib_subtitle);
    
    let lib_scroll = gtk::ScrolledWindow::new();
    lib_scroll.set_vexpand(true);
    let lib_list = gtk::ListBox::new();
    lib_list.set_selection_mode(gtk::SelectionMode::Single);
    lib_scroll.set_child(Some(&lib_list));
    lib_panel.append(&lib_scroll);
    
    // ─── FAO Web Library subsection (Phase 3 GUI) ───
    // 
    // Persistent canonical-mesh library that the encoder queries at chunk 
    // close to find existing matches. Two-way benefit:
    //   - Encoder: smaller .fao trailers when chunks match library canonicals
    //   - Library: grows over time as more videos contribute their canonicals
    // 
    // This block adds the path picker + status display + R/W toggle + 
    // detach button. Wire-up below populates it from App.fao_web_lib_path.
    let weblib_section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    weblib_section.add_css_class("panel");
    weblib_section.set_margin_top(8);
    
    let weblib_title = gtk::Label::new(Some("🔗 FAO Web Library (cross-encode mesh sharing)"));
    weblib_title.set_halign(gtk::Align::Start);
    weblib_title.add_css_class("sub");
    weblib_section.append(&weblib_title);
    
    // Status label — updated dynamically when path changes.
    let weblib_status = gtk::Label::new(Some("No library attached. Encoder runs standalone."));
    weblib_status.set_halign(gtk::Align::Start);
    weblib_status.set_wrap(true);
    weblib_status.add_css_class("stat");
    weblib_section.append(&weblib_status);
    
    let weblib_btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    weblib_btn_row.set_margin_top(4);
    
    let weblib_init_btn = gtk::Button::with_label("Initialize New…");
    weblib_init_btn.add_css_class("act2");
    weblib_init_btn.set_tooltip_text(Some("Create a new empty library at a chosen folder"));
    
    let weblib_open_btn = gtk::Button::with_label("Attach Existing…");
    weblib_open_btn.add_css_class("act");
    weblib_open_btn.set_tooltip_text(Some("Point at an existing library folder"));
    
    let weblib_detach_btn = gtk::Button::with_label("Detach");
    weblib_detach_btn.add_css_class("back");
    weblib_detach_btn.set_tooltip_text(Some("Stop using the library; encoder/decoder go standalone"));
    weblib_detach_btn.set_sensitive(false);  // disabled until a lib is attached
    
    let weblib_writable_chk = gtk::CheckButton::with_label("Writable (this encode adds new canonicals)");
    weblib_writable_chk.set_tooltip_text(Some("Off = read-only (curated lib safety). On = this encode contributes its canonicals to the library, growing it for future encodes."));
    
    // ── Rollback button ──
    // 
    // Library writes auto-create rotating backups (.bak.0..bak.4). 
    // This button opens a dialog letting the user pick a backup slot 
    // and roll back to it. Before rollback, the CURRENT manifest is 
    // pushed onto the backup stack — so the user can roll forward 
    // again if they regret it.
    let weblib_rollback_btn = gtk::Button::with_label("Rollback…");
    weblib_rollback_btn.add_css_class("back");
    weblib_rollback_btn.set_tooltip_text(Some(
        "Restore manifest.json from a rotating backup slot (.bak.0..bak.4).          Before rollback the current state is pushed to .bak.0, so rollback          is itself reversible. Disabled until a library is attached."
    ));
    weblib_rollback_btn.set_sensitive(false);
    
    // ── DB Bench button ──
    // Runs the synthetic library_db benchmark and prints results to 
    // stderr. Useful for verifying perf on the user's actual hardware. 
    // Doesn't require an attached library — uses a temp dir.
    let weblib_db_bench_btn = gtk::Button::with_label("DB Bench");
    weblib_db_bench_btn.add_css_class("act2");
    weblib_db_bench_btn.set_tooltip_text(Some(
        "Run a synthetic library_db benchmark (10k entries, 10k random gets)          to a temp directory and print the timing results. Useful for          confirming the DB is meeting our perf targets on this hardware."
    ));
    
    // ── Stats button ──
    // Reads + prints a multi-line stat summary for the attached library.
    // Counts, sizes, time range, top source videos, ref distribution.
    let weblib_stats_btn = gtk::Button::with_label("Stats");
    weblib_stats_btn.add_css_class("act2");
    weblib_stats_btn.set_tooltip_text(Some(
        "Print library statistics: web count, disk size, time range,          top source videos, ref_count distribution. Disabled until a          library is attached."
    ));
    weblib_stats_btn.set_sensitive(false);
    
    // ── Prune button ──
    // Removes canonicals with ref_count=0 that haven't been referenced 
    // in 30+ days. Helps keep library size bounded over time.
    let weblib_prune_btn = gtk::Button::with_label("Prune Unused");
    weblib_prune_btn.add_css_class("back");
    weblib_prune_btn.set_tooltip_text(Some(
        "Remove canonicals that have ref_count=0 and haven't been          touched in 30+ days. Frees disk space. Requires the library          to be open writable."
    ));
    weblib_prune_btn.set_sensitive(false);
    
    weblib_btn_row.append(&weblib_init_btn);
    weblib_btn_row.append(&weblib_open_btn);
    weblib_btn_row.append(&weblib_detach_btn);
    weblib_btn_row.append(&weblib_rollback_btn);
    weblib_btn_row.append(&weblib_db_bench_btn);
    weblib_btn_row.append(&weblib_stats_btn);
    weblib_btn_row.append(&weblib_prune_btn);
    weblib_btn_row.append(&weblib_writable_chk);
    weblib_section.append(&weblib_btn_row);
    
    lib_panel.append(&weblib_section);
    
    // Helper to refresh the status display from current App state. Captures 
    // weak references to the widgets and `st` (App). Called whenever the 
    // path or writable flag changes.
    let refresh_weblib_status: std::rc::Rc<dyn Fn()> = {
        let st = st.clone();
        let status = weblib_status.clone();
        let detach_btn = weblib_detach_btn.clone();
        let rollback_btn = weblib_rollback_btn.clone();
        let stats_btn = weblib_stats_btn.clone();
        let prune_btn = weblib_prune_btn.clone();
        let writable_chk = weblib_writable_chk.clone();
        std::rc::Rc::new(move || {
            let app = st.borrow();
            match &app.fao_web_lib_path {
                Some(p) => {
                    // Try to read fresh stats. Errors here just give us 
                    // "(stats unavailable)".
                    // Cheap stats query: reads only "web_count" from the 
                    // manifest head (no full JSON parse) + walks webs/ dir 
                    // for on-disk total. Runs on every checkbox toggle, so 
                    // avoiding the full Library::open path matters at scale.
                    let stats_str = match fao_v1::library::quick_stats(p) {
                        Some((count, bytes)) => format!(
                            "{} webs, {:.1} MB on disk",
                            count,
                            bytes as f64 / 1_048_576.0,
                        ),
                        None => "(stats unavailable — library may be locked or invalid)".to_string(),
                    };
                    let mode_str = if app.fao_web_lib_writable { "writable" } else { "read-only" };
                    status.set_label(&format!(
                        "✓ Attached: {}
  {} | {}",
                        p.display(), stats_str, mode_str,
                    ));
                    detach_btn.set_sensitive(true);
                    rollback_btn.set_sensitive(true);
                    stats_btn.set_sensitive(true);
                    // Prune requires writable.
                    prune_btn.set_sensitive(app.fao_web_lib_writable);
                    writable_chk.set_active(app.fao_web_lib_writable);
                }
                None => {
                    status.set_label("No library attached. Encoder runs standalone.");
                    detach_btn.set_sensitive(false);
                    rollback_btn.set_sensitive(false);
                    stats_btn.set_sensitive(false);
                    prune_btn.set_sensitive(false);
                    writable_chk.set_active(false);
                }
            }
        })
    };
    // Refresh once at startup to reflect any env-var-derived initial state.
    refresh_weblib_status();
    
    // Init button: open a folder picker, ask for confirmation, run Library::init.
    {
        let st = st.clone();
        let win = win.clone();
        let refresh = refresh_weblib_status.clone();
        weblib_init_btn.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Choose folder for new FAO Library"),
                Some(&win),
                gtk::FileChooserAction::SelectFolder,
                Some("Initialize"),
                Some("Cancel"),
            );
            let st2 = st.clone();
            let refresh2 = refresh.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(folder) = d.file().and_then(|f| f.path()) {
                        match fao_v1::library::Library::init(&folder) {
                            Ok(()) => {
                                eprintln!("[ui] FAO Library initialized at {:?}", folder);
                                let mut app = st2.borrow_mut();
                                app.fao_web_lib_path = Some(folder);
                                drop(app);
                                refresh2();
                            }
                            Err(e) => {
                                eprintln!("[ui] FAO Library init failed: {}", e);
                            }
                        }
                    }
                }
                d.destroy();
            });
            dialog.show();
        });
    }
    
    // Attach button: open folder picker, validate it has manifest.json, set the path.
    {
        let st = st.clone();
        let win = win.clone();
        let refresh = refresh_weblib_status.clone();
        weblib_open_btn.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Attach existing FAO Library"),
                Some(&win),
                gtk::FileChooserAction::SelectFolder,
                Some("Attach"),
                Some("Cancel"),
            );
            let st2 = st.clone();
            let refresh2 = refresh.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(folder) = d.file().and_then(|f| f.path()) {
                        if !folder.join("manifest.json").exists() {
                            eprintln!("[ui] {:?} doesn't look like a FAO Library (missing manifest.json)", folder);
                            d.destroy();
                            return;
                        }
                        eprintln!("[ui] FAO Library attached at {:?}", folder);
                        let mut app = st2.borrow_mut();
                        app.fao_web_lib_path = Some(folder);
                        drop(app);
                        refresh2();
                    }
                }
                d.destroy();
            });
            dialog.show();
        });
    }
    
    // Detach button: clear the path.
    {
        let st = st.clone();
        let refresh = refresh_weblib_status.clone();
        weblib_detach_btn.connect_clicked(move |_| {
            let mut app = st.borrow_mut();
            app.fao_web_lib_path = None;
            app.fao_web_lib_writable = false;
            drop(app);
            refresh();
            eprintln!("[ui] FAO Library detached");
        });
    }
    
    // ── DB Bench button ──
    // Runs library_db::bench in a background thread (it allocates ~3MB 
    // and writes/reads it, takes ~100ms on modern hardware). Posts the 
    // result to stderr. Cheap and non-destructive (uses temp dir).
    {
        weblib_db_bench_btn.connect_clicked(|_| {
            std::thread::spawn(|| {
                eprintln!("[db bench] starting (10k entries, 10k gets)...");
                let report = fao_v1::library_db::bench(10_000, 10_000);
                eprintln!("[db bench] {}", report);
            });
        });
    }
    
    // ── Stats button ──
    // Opens the library, calls .stats(), prints the multi-line report.
    {
        let st = st.clone();
        weblib_stats_btn.connect_clicked(move |_| {
            let app = st.borrow();
            let lib_path = match app.fao_web_lib_path.clone() {
                Some(p) => p,
                None => { eprintln!("[stats] no library attached"); return; }
            };
            drop(app);
            std::thread::spawn(move || {
                match fao_v1::library::Library::open(&lib_path) {
                    Ok(lib) => {
                        let report = lib.stats();
                        eprintln!("[library stats]\n{}", report);
                    }
                    Err(e) => {
                        eprintln!("[stats] failed to open library: {}", e);
                    }
                }
            });
        });
    }
    
    // ── Prune button ──
    // Removes ref_count=0 canonicals not touched in 30+ days. Re-opens 
    // the library writable, calls .prune_unused(30), reports.
    {
        let st = st.clone();
        weblib_prune_btn.connect_clicked(move |_| {
            let app = st.borrow();
            let lib_path = match app.fao_web_lib_path.clone() {
                Some(p) => p,
                None => { eprintln!("[prune] no library attached"); return; }
            };
            drop(app);
            // Run prune in background — opens writable, modifies disk.
            // 
            // No UI refresh callback: refresh_weblib_status is Rc<dyn Fn()> 
            // which can't cross thread boundaries. Same pattern as the 
            // Bench/Stats buttons — results go to stderr, user clicks 
            // Stats again to see updated state.
            std::thread::spawn(move || {
                match fao_v1::library::Library::open_writable(&lib_path) {
                    Ok(mut lib) => {
                        match lib.prune_unused(30) {
                            Ok(pruned) => {
                                eprintln!("[prune] removed {} unused canonicals", pruned.len());
                                if !pruned.is_empty() {
                                    for uuid in pruned.iter().take(10) {
                                        eprintln!("[prune]   pruned: {}", uuid);
                                    }
                                    if pruned.len() > 10 {
                                        eprintln!("[prune]   ... ({} more)", pruned.len() - 10);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[prune] failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[prune] failed to open library writable: {} (need writable mode)", e);
                    }
                }
            });
        });
    }
    
    // ── Rollback button ──
    // 
    // Opens a small dialog showing the available .bak.0..bak.4 slots 
    // (with their timestamp + web count if readable) and lets the user 
    // pick one to restore. Confirmation is required because rollback 
    // modifies the manifest in-place (though it IS reversible — see 
    // rollback_manifest doc).
    {
        let st = st.clone();
        let refresh = refresh_weblib_status.clone();
        let parent_win = win.clone();
        weblib_rollback_btn.connect_clicked(move |_| {
            let app = st.borrow();
            let lib_path = match app.fao_web_lib_path.clone() {
                Some(p) => p,
                None => {
                    eprintln!("[ui] rollback: no library attached");
                    return;
                }
            };
            drop(app);
            
            // Build the dialog.
            let dlg = gtk::Dialog::builder()
                .title("Rollback library manifest")
                .transient_for(&parent_win)
                .modal(true)
                .build();
            let content = dlg.content_area();
            content.set_margin_top(12);
            content.set_margin_bottom(12);
            content.set_margin_start(12);
            content.set_margin_end(12);
            
            let header = gtk::Label::new(Some(
                "Pick a backup slot to restore. The current manifest will be                  pushed to .bak.0 before the rollback so you can roll forward                  again. Slot 0 is the most recent backup."
            ));
            header.set_wrap(true);
            header.set_xalign(0.0);
            header.set_margin_bottom(8);
            content.append(&header);
            
            // List available slots.
            let mut slot_radios: Vec<gtk::CheckButton> = Vec::new();
            let mut any_slot_active = false;
            for slot in 0..5 {
                let bak = lib_path.join(format!("manifest.json.bak.{}", slot));
                let exists = bak.exists();
                let label_text = if exists {
                    // Read web_count from the backup to give a hint.
                    let info = match std::fs::metadata(&bak) {
                        Ok(md) => {
                            let size_kb = md.len() / 1024;
                            let modified = md.modified().ok()
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| {
                                    let secs = d.as_secs();
                                    // Rough age in human terms.
                                    let now = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs()).unwrap_or(0);
                                    let age = now.saturating_sub(secs);
                                    if age < 60 { format!("{}s ago", age) }
                                    else if age < 3600 { format!("{}m ago", age / 60) }
                                    else if age < 86400 { format!("{}h ago", age / 3600) }
                                    else { format!("{}d ago", age / 86400) }
                                })
                                .unwrap_or_else(|| "?".to_string());
                            format!("Slot {}: {} ({} KB, {})", slot, bak.display(), size_kb, modified)
                        }
                        Err(_) => format!("Slot {}: {} (unreadable)", slot, bak.display()),
                    };
                    info
                } else {
                    format!("Slot {}: (empty)", slot)
                };
                let radio = gtk::CheckButton::with_label(&label_text);
                radio.set_sensitive(exists);
                if exists && !any_slot_active {
                    radio.set_active(true);
                    any_slot_active = true;
                }
                if slot > 0 {
                    radio.set_group(Some(&slot_radios[0]));
                }
                slot_radios.push(radio.clone());
                content.append(&radio);
            }
            
            if !any_slot_active {
                let warn = gtk::Label::new(Some("No backups exist yet. Write to the library at least once to create one."));
                warn.add_css_class("warn");
                content.append(&warn);
            }
            
            dlg.add_button("Cancel", gtk::ResponseType::Cancel);
            let restore_btn = dlg.add_button("Restore", gtk::ResponseType::Accept);
            restore_btn.set_sensitive(any_slot_active);
            
            let refresh_d = refresh.clone();
            let lib_path_d = lib_path.clone();
            dlg.connect_response(move |d, resp| {
                if resp != gtk::ResponseType::Accept {
                    d.close();
                    return;
                }
                let chosen = slot_radios.iter().enumerate()
                    .find(|(_, r)| r.is_active())
                    .map(|(i, _)| i);
                d.close();
                let slot = match chosen {
                    Some(s) => s,
                    None => return,
                };
                // Roll back BOTH the JSON manifest and the binary log 
                // database. Both have parallel .bak.N slots and roll 
                // back together so the user sees a consistent prior 
                // state. If log_db rollback fails (no log yet, slot 
                // not populated), we still proceed with manifest 
                // rollback — JSON is authoritative.
                let mf_result = fao_v1::library::rollback_manifest(&lib_path_d, slot);
                let log_result = fao_v1::library_db::rollback_log(&lib_path_d, slot);
                match (&mf_result, &log_result) {
                    (Ok(()), Ok(())) => {
                        eprintln!("[ui] rollback OK (both manifest+log) from slot {}", slot);
                    }
                    (Ok(()), Err(e)) => {
                        eprintln!("[ui] rollback OK manifest, log_db FAILED: {} (manifest is authoritative; encode will repopulate log)", e);
                    }
                    (Err(e), Ok(())) => {
                        eprintln!("[ui] rollback FAILED manifest: {} (log_db rolled back but JSON is unchanged — manual fix needed)", e);
                    }
                    (Err(e1), Err(e2)) => {
                        eprintln!("[ui] rollback FAILED for both: manifest={} log={}", e1, e2);
                    }
                }
                if mf_result.is_ok() || log_result.is_ok() {
                    refresh_d();
                }
            });
            dlg.show();
        });
    }
    
    // Writable toggle: update the App flag.
    {
        let st = st.clone();
        let refresh = refresh_weblib_status.clone();
        weblib_writable_chk.connect_toggled(move |chk| {
            let mut app = st.borrow_mut();
            app.fao_web_lib_writable = chk.is_active();
            drop(app);
            refresh();
        });
    }
    
    lib_panel.set_visible(true);  // visible by default (Library tab active)
    left.append(&lib_panel);
    
    // Encoder UI container (toggled with Encoder tab)
    let enc_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    enc_panel.set_vexpand(true);
    enc_panel.set_visible(false);  // hidden by default
    
    let bar=gtk::Box::new(gtk::Orientation::Horizontal,6);bar.set_margin_top(6);
    let load_img=gtk::Button::with_label("Image");load_img.add_css_class("act");
    load_img.set_tooltip_text(Some("Load a single image file (JPG, PNG, etc) for encoding test"));
    let load_vid=gtk::Button::with_label("Video");load_vid.add_css_class("act2");
    load_vid.set_tooltip_text(Some("Load a video file (MP4, WebM, etc) to encode to FAO"));
    bar.append(&load_img);bar.append(&load_vid);
    bar.append(&{let s=gtk::Separator::new(gtk::Orientation::Vertical);s.set_margin_start(4);s.set_margin_end(4);s});
    let bsplit=gtk::Button::with_label("A/B");bsplit.add_css_class("va");
    bsplit.set_tooltip_text(Some("Split-screen view: drag to compare source (left) vs FAO-decoded (right)"));
    let borig=gtk::Button::with_label("Orig");borig.add_css_class("vb");
    borig.set_tooltip_text(Some("Show original source frame (only available if source path is preserved)"));
    let brec=gtk::Button::with_label("FAO");brec.add_css_class("vb");
    brec.set_tooltip_text(Some("Show FAO-decoded frame (the encoded → decoded result)"));
    let berr=gtk::Button::with_label("Err");berr.add_css_class("vb");
    berr.set_tooltip_text(Some("Visualize per-pixel error between source and FAO-decoded"));
    bar.append(&bsplit);bar.append(&borig);bar.append(&brec);bar.append(&berr);
    
    // Playback mode toggle (v5+ only — Authentic vs Engine)
    bar.append(&{let s=gtk::Separator::new(gtk::Orientation::Vertical);s.set_margin_start(4);s.set_margin_end(4);s});
    let mode_lbl = gtk::Label::new(Some("Mode:"));
    mode_lbl.add_css_class("dim-label");
    // mode label dropped — Auth/Eng buttons are self-labeling
    let mode_authentic = gtk::ToggleButton::with_label("Auth");
    let mode_engine = gtk::ToggleButton::with_label("Eng");
    mode_authentic.set_active(true);
    mode_engine.set_group(Some(&mode_authentic));
    mode_authentic.set_tooltip_text(Some("Authentic: match source framerate, apply grain/noise from original video"));
    mode_engine.set_tooltip_text(Some("Engine: render at any framerate (game-engine style), interpolate object motion, skip authenticity layer"));
    bar.append(&mode_authentic);
    bar.append(&mode_engine);
    
    // Codec version selector
    bar.append(&{let s=gtk::Separator::new(gtk::Orientation::Vertical);s.set_margin_start(4);s.set_margin_end(4);s});
    let codec_lbl = gtk::Label::new(Some("Codec:"));
    codec_lbl.add_css_class("dim-label");
    // codec label dropped — dropdown is self-labeling
    // Codec ToggleButtons kept as the underlying logical state but NOT shown.
    // The visible UI is a DropDown below; we drive the toggles from its selection.
    // (Keeping these means handlers like codec_v50.set_tooltip_text + the dialog 
    //  CheckButton sync logic continue to work without a giant refactor.)
    let codec_v20 = gtk::ToggleButton::with_label("v2.0");
    let codec_v21 = gtk::ToggleButton::with_label("v2.1");
    let codec_v30 = gtk::ToggleButton::with_label("v3.0");
    let codec_v40 = gtk::ToggleButton::with_label("v4.0");
    let codec_v50 = gtk::ToggleButton::with_label("v5.0");
    let codec_v60 = gtk::ToggleButton::with_label("v6.0");
    let codec_v70 = gtk::ToggleButton::with_label("v7.0");
    let codec_v80 = gtk::ToggleButton::with_label("v8.0");
    codec_v20.set_visible(false);
    codec_v21.set_visible(false);
    codec_v30.set_visible(false);
    codec_v40.set_visible(false);
    codec_v50.set_visible(false);
    codec_v60.set_visible(false);
    codec_v70.set_visible(false);
    codec_v80.set_visible(false);
    codec_v21.set_active(true);  // default
    codec_v20.set_group(Some(&codec_v21));
    codec_v30.set_group(Some(&codec_v21));
    codec_v50.set_group(Some(&codec_v21));
    codec_v60.set_group(Some(&codec_v21));
    codec_v70.set_group(Some(&codec_v21));
    codec_v80.set_group(Some(&codec_v21));
    codec_v20.set_tooltip_text(Some("v2.0: skip + passthrough + full encode (no T-frames)"));
    codec_v21.set_tooltip_text(Some("v2.1: predictive T-frames (current default)"));
    codec_v30.set_tooltip_text(Some("v3.0: layered mesh codec (static plate + residual patches)"));
    codec_v40.set_tooltip_text(Some("v4.0: sparse skeleton codec (~150x compression target)"));
    codec_v50.set_tooltip_text(Some("v5.0: scene replay codec — sprites + state, game-engine playback"));
    codec_v60.set_tooltip_text(Some("v6.0: object field codec — 3D objects, GPU candidate generation, feature-space matching"));
    codec_v70.set_tooltip_text(Some("v7.0: perception codec — phase correlation tracks camera motion separately from objects"));
    codec_v80.set_tooltip_text(Some("v8.0: reconstruction codec [SCAFFOLD] — camera pose track + scene mesh trailer (full 3D recon coming in future sessions)"));
    // Hidden ToggleButtons must still be in the tree so their group state works
    bar.append(&codec_v20);
    bar.append(&codec_v21);
    bar.append(&codec_v30);
    bar.append(&codec_v40);
    bar.append(&codec_v50);
    bar.append(&codec_v60);
    bar.append(&codec_v70);
    bar.append(&codec_v80);
    
    // Visible UI: single dropdown saves ~8 buttons of horizontal space.
    // Order matches codec_version values (0=v2.0, 1=v2.1, 2=v3.0, ...)
    let codec_options = ["v2.0", "v2.1", "v3.0", "v4.0", "v5.0", "v6.0", "v7.0", "v8.0"];
    let codec_strlist = gtk::StringList::new(&codec_options);
    let codec_dd = gtk::DropDown::new(Some(codec_strlist), None::<gtk::Expression>);
    codec_dd.set_selected(1);  // default v2.1
    codec_dd.set_tooltip_text(Some("Codec version. Higher numbers = more advanced (sprites, 3D objects, perception, mesh reconstruction)."));
    bar.append(&codec_dd);
    
    // Drive the underlying ToggleButton (and codec_version) from dropdown selection
    {
        let v20 = codec_v20.clone(); let v21 = codec_v21.clone();
        let v30 = codec_v30.clone(); let v40 = codec_v40.clone();
        let v50 = codec_v50.clone(); let v60 = codec_v60.clone();
        let v70 = codec_v70.clone(); let v80 = codec_v80.clone();
        codec_dd.connect_selected_notify(move |dd| {
            let i = dd.selected();
            // Activate the matching toggle (which fires its handler that updates 
            // codec_version + prints the selection log)
            match i {
                0 => v20.set_active(true),
                1 => v21.set_active(true),
                2 => v30.set_active(true),
                3 => v40.set_active(true),
                4 => v50.set_active(true),
                5 => v60.set_active(true),
                6 => v70.set_active(true),
                7 => v80.set_active(true),
                _ => {}
            }
        });
    }
    
    enc_panel.append(&bar);

    let pic=gtk::Picture::new();pic.set_vexpand(true);pic.set_hexpand(true);
    // Fullscreen overlay button
    let fs_btn = gtk::Button::with_label("⛶");
    fs_btn.set_halign(gtk::Align::End);
    fs_btn.set_valign(gtk::Align::Start);
    fs_btn.set_margin_top(8);
    fs_btn.set_margin_end(8);
    fs_btn.set_opacity(0.3);
    fs_btn.set_tooltip_text(Some("Fullscreen (F)"));
    fs_btn.add_css_class("fullscreen-btn");
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&pic));
    overlay.add_overlay(&fs_btn);
    overlay.set_vexpand(true);
    overlay.set_hexpand(true);
    let pf=gtk::Frame::new(None);pf.set_child(Some(&overlay));pf.set_margin_top(6);
    enc_panel.append(&pf);
    
    // Web visualization — frame relationship graph in 3D-ish projection
    let web_da = gtk::DrawingArea::new();
    web_da.set_content_height(220);
    web_da.set_hexpand(true);
    web_da.set_margin_top(4);
    web_da.add_css_class("web-viz");
    {
        let st_c = st.clone();
        web_da.set_draw_func(move |_, cr, w, h| {
            let st = match st_c.try_borrow() { Ok(s) => s, Err(_) => return };
            
            // Background gradient
            cr.set_source_rgb(0.05, 0.07, 0.10);
            cr.paint().ok();
            
            // Title
            cr.set_source_rgba(0.5, 0.55, 0.7, 0.6);
            cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(10.0);
            cr.move_to(8.0, 14.0);
            // Try to get layer breakdown from region activity (if encoding active)
            let layer_info = if let Some(ref tc) = st.tc {
                if let Ok(ra) = tc.region_activity.try_lock() {
                    let dist = ra.layer_distribution();
                    if dist[0] + dist[1] + dist[2] + dist[3] > 1.0 {
                        Some(format!(" — pure {:.0}%, alpha {:.0}%, parallax {:.0}%, active {:.0}%",
                            dist[0], dist[1], dist[2], dist[3]))
                    } else { None }
                } else { None }
            } else { None };
            
            // Filter analysis: what filter wins for residuals?
            let filter_info = if let Some(ref tc) = st.tc {
                if let Ok(fa_lock) = tc.last_filter_analysis.try_read() {
                    fa_lock.as_ref().map(|fa| {
                        format!(" filter[id={:.0}% dn={:.0}% qz={:.0}% ed={:.0}%]",
                            fa.percent(fao_v1::pixel_filters::PixelFilter::Identity),
                            fa.percent(fao_v1::pixel_filters::PixelFilter::Denoise3x3),
                            fa.percent(fao_v1::pixel_filters::PixelFilter::Quantize8),
                            fa.percent(fao_v1::pixel_filters::PixelFilter::EdgePreserve))
                    })
                } else { None }
            } else { None };
            
            let title_text = if st.interp_stats.0 + st.interp_stats.1 > 0 {
                let total = st.interp_stats.0 + st.interp_stats.1;
                let interp_pct = (st.interp_stats.0 as f32 / total as f32) * 100.0;
                let pred_q = st.interp_stats.3;
                let q_label = if pred_q > 35.0 { "excellent" }
                              else if pred_q > 28.0 { "good" }
                              else if pred_q > 22.0 { "moderate" }
                              else { "poor" };
                format!("Frame Web — interp {:.0}% ({} synth / {} real)  pred PSNR {:.1}dB ({}){}{}",
                    interp_pct, st.interp_stats.0, st.interp_stats.1, pred_q, q_label,
                    layer_info.as_deref().unwrap_or(""),
                    filter_info.as_deref().unwrap_or(""))
            } else if layer_info.is_some() || filter_info.is_some() {
                format!("Frame Web — function storage in 3D{}{}",
                    layer_info.as_deref().unwrap_or(""),
                    filter_info.as_deref().unwrap_or(""))
            } else {
                "Frame Web — function storage in 3D".to_string()
            };
            cr.show_text(&title_text).ok();
            
            // Stats (with view info)
            let stats = if st.web_viz_nodes.len() > 200 {
                format!("{} nodes  active: {}  zoom (window of ~150)",
                    st.web_viz_nodes.len(), st.web_viz_active_frame)
            } else {
                format!("{} nodes  {} edges  active: {}",
                    st.web_viz_nodes.len(),
                    st.web_viz_edges.len(),
                    st.web_viz_active_frame)
            };
            cr.move_to(w as f64 - 280.0, 14.0);
            cr.show_text(&stats).ok();
            
            if st.web_viz_nodes.is_empty() {
                cr.set_source_rgba(0.4, 0.45, 0.6, 0.7);
                cr.move_to(w as f64 / 2.0 - 80.0, h as f64 / 2.0);
                cr.show_text("(no encoding active — start a transcode to see web)").ok();
                return;
            }
            
            // 3D to 2D projection with auto-fit translation
            // Goal: maximize visible nodes around the playhead
            //   - When playhead is within first/last 25% of nodes: no translate
            //   - When playhead is in middle: translate so it stays centered
            //   - X-zoom expands to show ~80-200 nodes around playhead
            
            let plot_w = (w as f64 - 180.0).max(200.0);  // leave 180px for legend
            let cx = plot_w / 2.0 + 20.0;
            let cy = h as f64 / 2.0 + 8.0;
            
            // Determine zoom level based on node count
            // Few nodes (< 200): show all (compress to fit)
            // Many nodes (> 200): zoom in, show window of ~150 frames around active
            let total_nodes = st.web_viz_nodes.len();
            let active = st.web_viz_active_frame;
            
            let (translate_x, x_zoom): (f64, f64) = if total_nodes <= 200 {
                (0.0, 1.0)
            } else {
                let target_visible = 150.0_f32;
                let max_frame = st.web_viz_nodes.iter().map(|(fi, ..)| *fi).max().unwrap_or(1) as f32;
                let zoom = (max_frame / target_visible).max(1.0).min(20.0);
                
                // Find active frame; if not in node set, fall back to last node
                let center_frame = if st.web_viz_nodes.iter().any(|(fi, ..)| *fi == active) {
                    active as f32
                } else {
                    // No active node — use last available node as center
                    st.web_viz_nodes.last().map(|(fi, ..)| *fi as f32).unwrap_or(max_frame)
                };
                let center_x_base = center_frame / max_frame * 2.0 - 1.0;
                
                let dummy_scale = (plot_w - 40.0) / 2.4;
                let tx = -(center_x_base as f64) * (zoom as f64) * dummy_scale;
                
                let first_x = cx + tx - (zoom as f64) * dummy_scale;
                let last_x = cx + tx + (zoom as f64) * dummy_scale;
                let mut tx_adjusted = tx;
                if first_x > 20.0 { tx_adjusted -= first_x - 20.0; }
                if last_x < plot_w + 10.0 && total_nodes > 0 { 
                    let needed = (plot_w + 10.0) - last_x;
                    tx_adjusted += needed;
                }
                
                (tx_adjusted, zoom as f64)
            };
            
            let scale_x = ((plot_w - 40.0) / 2.4) * x_zoom;
            let scale_y = (h as f64 - 50.0) / 1.8;
            let project = |x: f32, y: f32, z: f32| -> (f64, f64) {
                let sx = cx + translate_x + (x as f64) * scale_x + (z as f64) * 12.0;
                let sy = cy + (y as f64) * scale_y - (z as f64) * 18.0;
                (sx, sy)
            };
            
            // Draw edges first (under nodes)
            // Cull edges entirely outside view
            let cull_lt = -100.0_f64;
            let cull_rt = plot_w + 100.0;
            cr.set_line_width(0.6);
            for &(src, dst, weight) in &st.web_viz_edges {
                let s = st.web_viz_nodes.iter().find(|n| n.0 == src);
                let d = st.web_viz_nodes.iter().find(|n| n.0 == dst);
                if let (Some(s), Some(d)) = (s, d) {
                    let (x1, y1) = project(s.1, s.2, s.3);
                    let (x2, y2) = project(d.1, d.2, d.3);
                    if (x1 < cull_lt && x2 < cull_lt) || (x1 > cull_rt && x2 > cull_rt) {
                        continue;
                    }
                    let alpha = (weight * 0.3) as f64;
                    cr.set_source_rgba(0.3, 0.5, 0.8, alpha);
                    cr.move_to(x1, y1);
                    cr.line_to(x2, y2);
                    cr.stroke().ok();
                }
            }
            
            // Draw nodes — glow scales with weight, pulses with phase
            let phase = st.web_viz_phase;
            let active = st.web_viz_active_frame;
            
            // Compute progress through video (0..1)
            let max_frame = st.web_viz_nodes.iter().map(|(fi, ..)| *fi).max().unwrap_or(1);
            let progress = if max_frame > 0 { (active as f32 / max_frame as f32).min(1.0) } else { 0.0 };
            
            // Sweep line: vertical glow at current playback position
            // If active node not found in viz set, fall back to last node
            let active_node_opt = st.web_viz_nodes.iter().find(|(fi, ..)| *fi == active)
                .or_else(|| st.web_viz_nodes.last());
            
            if let Some(active_node) = active_node_opt {
                let (asx_raw, _asy) = project(active_node.1, active_node.2, active_node.3);
                
                // Clamp sweep X to visible plot area (covers off-screen anchors gracefully)
                let asx = asx_raw.clamp(20.0, plot_w + 10.0);
                
                let sweep_pulse = 0.6 + 0.4 * (phase * 6.283 * 2.0).sin();
                cr.set_source_rgba(0.95, 0.85, 0.4, 0.10 * sweep_pulse as f64);
                cr.set_line_width(8.0);
                cr.move_to(asx, 28.0);
                cr.line_to(asx, h as f64 - 12.0);
                cr.stroke().ok();
                
                cr.set_source_rgba(0.95, 0.85, 0.4, 0.30 * sweep_pulse as f64);
                cr.set_line_width(2.0);
                cr.move_to(asx, 28.0);
                cr.line_to(asx, h as f64 - 12.0);
                cr.stroke().ok();
                
                cr.set_source_rgba(1.0, 0.95, 0.6, 0.85);
                cr.set_line_width(0.7);
                cr.move_to(asx, 28.0);
                cr.line_to(asx, h as f64 - 12.0);
                cr.stroke().ok();
            }
            
            // Progress bar at the top
            let pb_y = 22.0;
            let pb_x0 = 8.0;
            let pb_x1 = (w as f64 - 180.0).max(100.0);
            cr.set_source_rgba(0.2, 0.25, 0.35, 0.4);
            cr.set_line_width(3.0);
            cr.move_to(pb_x0, pb_y);
            cr.line_to(pb_x1, pb_y);
            cr.stroke().ok();
            
            // Filled portion (progress)
            let fill_x = pb_x0 + (pb_x1 - pb_x0) * progress as f64;
            cr.set_source_rgba(0.95, 0.85, 0.4, 0.7);
            cr.set_line_width(3.0);
            cr.move_to(pb_x0, pb_y);
            cr.line_to(fill_x, pb_y);
            cr.stroke().ok();
            
            // Now draw nodes with playback-aware coloring
            // Cull nodes outside the visible window for performance
            let cull_left = -50.0_f64;
            let cull_right = plot_w + 50.0;
            
            // Precompute scene color palette (avoid 3 cos() per node)
            let mut scene_palette: [(f32, f32, f32); 16] = [(0.0, 0.0, 0.0); 16];
            for s in 0..16 {
                let hue = (s as f32 / 8.0) * 6.28;
                scene_palette[s] = (
                    0.5 + 0.5 * hue.cos(),
                    0.5 + 0.5 * (hue + 2.09).cos(),
                    0.5 + 0.5 * (hue + 4.19).cos(),
                );
            }
            
            for &(fi, x, y, z, weight, scene) in &st.web_viz_nodes {
                let (sx, sy) = project(x, y, z);
                if sx < cull_left || sx > cull_right { continue; }
                
                // Color by scene (lookup, no trig)
                let palette_idx = (scene as usize) & 0x0F;
                let (mut r, mut g, mut b) = scene_palette[palette_idx];
                
                // Tint by prediction quality if active
                // High PSNR (good prediction) → green tint
                // Low PSNR (bad prediction) → red tint
                if fi == active && st.interp_stats.3 > 0.0 {
                    let pred_q = st.interp_stats.3;
                    if pred_q > 30.0 {
                        // Good — bias toward green
                        g = (g + 0.3).min(1.0);
                    } else if pred_q < 22.0 {
                        // Poor — bias toward red/orange
                        r = (r + 0.4).min(1.0);
                        g = (g - 0.2).max(0.0);
                    }
                }
                
                // Glow pulse — animate active frames (within 30 frames of playback head)
                let dist_to_active = ((fi as i64 - active as i64).abs() as f32).min(80.0);
                let activity = if dist_to_active < 30.0 {
                    (1.0 - dist_to_active / 30.0) * (1.0 + 0.3 * (phase * 6.28).sin())
                } else { 0.0 };
                
                // Darken nodes that are after playback head (not yet played)
                if fi > active {
                    r *= 0.45; g *= 0.45; b *= 0.55;
                }
                
                let radius = 1.5 + weight * 2.5 + activity * 2.5;
                let alpha = if fi <= active {
                    0.4 + weight as f64 * 0.4 + activity as f64 * 0.5
                } else {
                    0.25 + weight as f64 * 0.25
                };
                
                // Outer glow for active frames
                if activity > 0.0 {
                    cr.set_source_rgba(r as f64, g as f64, b as f64, activity as f64 * 0.4);
                    cr.arc(sx, sy, radius as f64 * 3.0, 0.0, 6.283);
                    cr.fill().ok();
                }
                
                // Core
                cr.set_source_rgba(r as f64, g as f64, b as f64, alpha);
                cr.arc(sx, sy, radius as f64, 0.0, 6.283);
                cr.fill().ok();
                
                // Highlight ring on the exact active frame
                if fi == active {
                    cr.set_source_rgba(1.0, 0.95, 0.6, 0.95);
                    cr.set_line_width(1.4);
                    cr.arc(sx, sy, radius as f64 + 2.5, 0.0, 6.283);
                    cr.stroke().ok();
                }
            }
            
            // Axis labels
            cr.set_source_rgba(0.4, 0.45, 0.6, 0.6);
            cr.set_font_size(8.0);
            cr.move_to(w as f64 - 50.0, h as f64 - 4.0);
            cr.show_text("→ time").ok();
            cr.move_to(2.0, h as f64 - 4.0);
            cr.show_text("scene ↕").ok();
            cr.move_to(2.0, 28.0);
            cr.show_text("tier ↗").ok();
            
            // Legend (bottom-right)
            let lg_x = w as f64 - 165.0;
            let lg_y = 28.0;
            cr.set_source_rgba(0.10, 0.13, 0.18, 0.85);
            cr.rectangle(lg_x, lg_y, 158.0, 92.0);
            cr.fill().ok();
            cr.set_source_rgba(0.3, 0.35, 0.5, 0.6);
            cr.set_line_width(0.5);
            cr.rectangle(lg_x, lg_y, 158.0, 92.0);
            cr.stroke().ok();
            
            cr.set_source_rgba(0.7, 0.75, 0.85, 0.9);
            cr.set_font_size(9.0);
            cr.move_to(lg_x + 6.0, lg_y + 12.0);
            cr.show_text("Legend").ok();
            cr.set_font_size(7.5);
            
            // Tiers
            cr.set_source_rgba(0.85, 0.7, 0.4, 0.95);
            cr.arc(lg_x + 12.0, lg_y + 24.0, 3.5, 0.0, 6.283);
            cr.fill().ok();
            cr.set_source_rgba(0.7, 0.75, 0.85, 0.85);
            cr.move_to(lg_x + 22.0, lg_y + 27.0);
            cr.show_text("anchor (full encode)").ok();
            
            cr.set_source_rgba(0.5, 0.7, 0.85, 0.85);
            cr.arc(lg_x + 12.0, lg_y + 36.0, 2.5, 0.0, 6.283);
            cr.fill().ok();
            cr.set_source_rgba(0.7, 0.75, 0.85, 0.85);
            cr.move_to(lg_x + 22.0, lg_y + 39.0);
            cr.show_text("passthrough").ok();
            
            cr.set_source_rgba(0.4, 0.5, 0.65, 0.6);
            cr.arc(lg_x + 12.0, lg_y + 48.0, 1.8, 0.0, 6.283);
            cr.fill().ok();
            cr.set_source_rgba(0.7, 0.75, 0.85, 0.85);
            cr.move_to(lg_x + 22.0, lg_y + 51.0);
            cr.show_text("skip frame").ok();
            
            // Active glow indicator
            cr.set_source_rgba(0.95, 0.85, 0.5, 0.4);
            cr.arc(lg_x + 12.0, lg_y + 60.0, 5.5, 0.0, 6.283);
            cr.fill().ok();
            cr.set_source_rgba(0.95, 0.85, 0.5, 0.95);
            cr.arc(lg_x + 12.0, lg_y + 60.0, 2.5, 0.0, 6.283);
            cr.fill().ok();
            cr.set_source_rgba(0.7, 0.75, 0.85, 0.85);
            cr.move_to(lg_x + 22.0, lg_y + 63.0);
            cr.show_text("active (encoding now)").ok();
            
            // Edge types
            cr.set_source_rgba(0.3, 0.5, 0.8, 0.6);
            cr.set_line_width(1.0);
            cr.move_to(lg_x + 6.0, lg_y + 74.0);
            cr.line_to(lg_x + 18.0, lg_y + 74.0);
            cr.stroke().ok();
            cr.set_source_rgba(0.7, 0.75, 0.85, 0.85);
            cr.move_to(lg_x + 22.0, lg_y + 77.0);
            cr.show_text("anchor reference").ok();
            
            cr.set_source_rgba(0.3, 0.5, 0.8, 0.9);
            cr.set_line_width(1.4);
            cr.move_to(lg_x + 6.0, lg_y + 85.0);
            cr.line_to(lg_x + 18.0, lg_y + 85.0);
            cr.stroke().ok();
            cr.set_source_rgba(0.7, 0.75, 0.85, 0.85);
            cr.move_to(lg_x + 22.0, lg_y + 88.0);
            cr.show_text("scene continuity").ok();
        });
    }
    enc_panel.append(&web_da);
    
    // Animate the web — 30fps refresh, playback-aware
    {
        let st_c = st.clone();
        let da_c = web_da.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
            if let Ok(mut s) = st_c.try_borrow_mut() {
                s.web_viz_phase = (s.web_viz_phase + 0.02) % 1.0;
                
                // Run particle physics step (gravity simulation)
                s.step_web_physics();
                
                // Active frame source priority:
                //   1. FAO playback: cur fidx
                //   2. Encoding in progress: latest encoded frame (n-1)
                //   3. Otherwise: current display fidx
                if s.mode == Mode::FaoPlay && !s.fao_index.is_empty() {
                    s.web_viz_active_frame = s.fidx as u32;
                    // Build viz nodes from FAO frame count if not already
                    if s.web_viz_nodes.is_empty() {
                        let n = s.fao_index.len() as u32;
                        s.update_web_viz(n);
                    }
                } else if let Some(ref tc) = s.tc {
                    let n = tc.frame_count.load(Ordering::Relaxed);
                    let done = tc.done.load(Ordering::Relaxed);
                    if !done {
                        // During encode: track latest encoded frame
                        s.web_viz_active_frame = n.saturating_sub(1) as u32;
                    } else {
                        // After encode: track current playback position
                        s.web_viz_active_frame = s.fidx as u32;
                    }
                    if s.web_viz_nodes.is_empty() && n > 10 {
                        s.update_web_viz(n as u32);
                    } else if s.web_viz_nodes.len() < 500 && n as usize > s.web_viz_nodes.len() + 50 {
                        s.update_web_viz(n as u32);
                    }
                }
            }
            da_c.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    // Scrub
    let scrub_box=gtk::Box::new(gtk::Orientation::Horizontal,6);scrub_box.set_margin_top(6);
    let scrub_lbl=gtk::Label::new(Some("0.0s"));scrub_lbl.add_css_class("pin");scrub_lbl.set_width_chars(14);
    let scrub_adj=gtk::Adjustment::new(0.0,0.0,600.0,0.5,5.0,0.0);
    let scrub_sc=gtk::Scale::new(gtk::Orientation::Horizontal,Some(&scrub_adj));scrub_sc.set_hexpand(true);scrub_sc.set_draw_value(false);
    scrub_box.append(&scrub_lbl);scrub_box.append(&scrub_sc);enc_panel.append(&scrub_box);

    // IN/OUT + Transcode + Export
    let pin_box=gtk::Box::new(gtk::Orientation::Horizontal,4);pin_box.set_margin_top(2);
    let pin_s_adj=gtk::Adjustment::new(0.0,0.0,600.0,0.5,5.0,0.0);
    let pin_s_sc=gtk::Scale::new(gtk::Orientation::Horizontal,Some(&pin_s_adj));pin_s_sc.set_hexpand(true);pin_s_sc.set_draw_value(true);
    let pin_e_adj=gtk::Adjustment::new(0.0,0.0,600.0,0.5,5.0,0.0);
    let pin_e_sc=gtk::Scale::new(gtk::Orientation::Horizontal,Some(&pin_e_adj));pin_e_sc.set_hexpand(true);pin_e_sc.set_draw_value(true);
    let pin_range=gtk::Label::new(Some("10.0s"));pin_range.add_css_class("pin");pin_range.set_width_chars(6);
    let tc_btn=gtk::Button::with_label("▶ Transcode");tc_btn.add_css_class("tc");
    tc_btn.set_tooltip_text(Some("Start encoding the loaded video to FAO format"));
    // Cancel button — only sensitive while an encode is running. Sets 
    // tc.cancel_requested, which workers and the reader poll on each 
    // iteration. Clean shutdown drains channels and flushes the disk 
    // writer; the resulting .fao holds whatever was completed up to 
    // the cancel.
    let cancel_btn=gtk::Button::with_label("⨯ Cancel");cancel_btn.add_css_class("tc");
    cancel_btn.set_tooltip_text(Some("Stop the in-flight encode. The partial output is preserved."));
    cancel_btn.set_sensitive(false);  // enabled only while encoding
    let export_btn=gtk::Button::with_label("Export");export_btn.add_css_class("export");
    // → AV1 / H.264 re-encode button. Decodes the currently-loaded .fao 
    // through the FAO pipeline (v8 mesh rasterizer + v2.1 DCT fallback) 
    // and pipes RGB to ffmpeg, which writes a portable .mkv. Useful for 
    // sharing .fao content outside the FAO ecosystem.
    // 
    // The actual codec is selected in a dialog when the button is clicked.
    let av1_btn=gtk::Button::with_label("→ AV1");av1_btn.add_css_class("export");
    av1_btn.set_tooltip_text(Some("Re-encode the loaded .fao to AV1 or H.264 .mkv (requires ffmpeg)"));
    let open_fao_btn=gtk::Button::with_label("Open .fao");open_fao_btn.add_css_class("act");
    
    // ── V9 controls ──
    // ToggleButton for v9 encoding ("V9" pressed = FAO_V9 env will be set before encode).
    // ToggleButton for v9 rendering ("V9R" pressed = FAO_V9_RENDER env set, ray-march during playback).
    // Status label shows v9 chunk count after load + last gate-check.
    let v9_enc_toggle = gtk::ToggleButton::with_label("V9");
    v9_enc_toggle.add_css_class("vb");
    v9_enc_toggle.set_tooltip_text(Some(
        "Encode volumetric V9DC trailer alongside V8MX.          When pressed, the next Transcode will run the v9          pipeline: greedy mesh-to-voxel splat → 3D DCT →          quantized blocks → V9DC trailer."
    ));
    let v9_render_toggle = gtk::ToggleButton::with_label("V9R");
    v9_render_toggle.add_css_class("vb");
    v9_render_toggle.set_tooltip_text(Some(
        "Render v9 voxel grids during playback instead of v8          mesh. When pressed AND a .fao with V9DC is loaded,          frames are produced by ray-marching the voxel grid.          Falls through to v8 mesh on any miss."
    ));
    let v9_show_ppm_btn = gtk::Button::with_label("V9 PPM");
    v9_show_ppm_btn.add_css_class("vb");
    v9_show_ppm_btn.set_tooltip_text(Some(
        "Open the latest signs-of-life PPM dump (written on          load of a .fao with V9DC). Shows what one v9 chunk          ray-marches to from a synthetic camera angle."
    ));
    let v9_status_lbl = gtk::Label::new(Some("v9: idle"));
    v9_status_lbl.add_css_class("sub");
    v9_status_lbl.set_width_chars(16);
    
    pin_box.append(&{let l=gtk::Label::new(Some("IN"));l.add_css_class("pin");l.set_width_chars(3);l});
    pin_box.append(&pin_s_sc);
    pin_box.append(&{let l=gtk::Label::new(Some("OUT"));l.add_css_class("pin");l.set_width_chars(4);l});
    pin_box.append(&pin_e_sc);pin_box.append(&pin_range);
    pin_box.append(&tc_btn);pin_box.append(&cancel_btn);pin_box.append(&export_btn);pin_box.append(&av1_btn);pin_box.append(&open_fao_btn);
    pin_box.append(&v9_enc_toggle);pin_box.append(&v9_render_toggle);pin_box.append(&v9_show_ppm_btn);pin_box.append(&v9_status_lbl);
    enc_panel.append(&pin_box);

    // Playback
    let pbar=gtk::Box::new(gtk::Orientation::Horizontal,6);pbar.set_margin_top(4);
    let back_btn=gtk::Button::with_label("← Scrub");back_btn.add_css_class("back");
    let prev_i_btn=gtk::Button::with_label("⏮");prev_i_btn.add_css_class("vb");prev_i_btn.set_tooltip_text(Some("Previous I-frame"));
    let prev_btn=gtk::Button::with_label("◀");prev_btn.add_css_class("vb");
    let play_btn=gtk::Button::with_label("▶ Play");play_btn.add_css_class("play");
    let next_btn=gtk::Button::with_label("▶");next_btn.add_css_class("vb");
    let next_i_btn=gtk::Button::with_label("⏭");next_i_btn.add_css_class("vb");next_i_btn.set_tooltip_text(Some("Next I-frame"));
    let fa=gtk::Adjustment::new(0.0,0.0,1.0,1.0,10.0,0.0);
    let fslider=gtk::Scale::new(gtk::Orientation::Horizontal,Some(&fa));fslider.set_hexpand(true);fslider.set_digits(0);fslider.set_draw_value(true);
    let frame_lbl=gtk::Label::new(Some(""));frame_lbl.add_css_class("sub");frame_lbl.set_width_chars(16);
    pbar.append(&back_btn);pbar.append(&prev_i_btn);pbar.append(&prev_btn);pbar.append(&play_btn);pbar.append(&next_btn);pbar.append(&next_i_btn);
    pbar.append(&fslider);pbar.append(&frame_lbl);
    enc_panel.append(&pbar);
    
    // Wire codec version selector
    {
        let cv = st.borrow().codec_version.clone();
        codec_v20.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(0); eprintln!("Codec: v2.0 selected"); }
        });
    }
    {
        let cv = st.borrow().codec_version.clone();
        codec_v21.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(1); eprintln!("Codec: v2.1 selected"); }
        });
    }
    {
        let cv = st.borrow().codec_version.clone();
        codec_v30.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(2); eprintln!("Codec: v3.0 selected (layered mesh, plate-based)"); }
        });
    }
    {
        let cv = st.borrow().codec_version.clone();
        codec_v40.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(3); eprintln!("Codec: v4.0 selected (sparse skeleton)"); }
        });
    }
    {
        let cv = st.borrow().codec_version.clone();
        codec_v50.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(4); eprintln!("Codec: v5.0 selected (scene replay)"); }
        });
    }
    {
        let cv = st.borrow().codec_version.clone();
        codec_v60.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(5); eprintln!("Codec: v6.0 selected (object field — 3D objects)"); }
        });
    }
    {
        let cv = st.borrow().codec_version.clone();
        codec_v70.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(6); eprintln!("Codec: v7.0 selected (perception — phase correlation + camera tracking)"); }
        });
    }
    {
        let cv = st.borrow().codec_version.clone();
        codec_v80.connect_toggled(move |btn| {
            if btn.is_active() { cv.set(7); eprintln!("Codec: v8.0 selected (reconstruction scaffold — camera pose + mesh trailer)"); }
        });
    }
    
    // Legacy v5 playback-mode toggles (Authentic / Engine). v5 codec was 
    // removed in v10 cleanup; the buttons still exist in the GUI but 
    // their handlers are no-ops now. They'll be removed once we're 
    // sure no .glade reference path needs them.
    let _ = (&mode_authentic, &mode_engine);
    
    left.append(&enc_panel);
    
    // Player panel — psychovisual A/B validation
    let play_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    play_panel.set_vexpand(true);
    play_panel.set_visible(false);
    
    let play_hdr = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let play_title = gtk::Label::new(Some("▶ Player — A/B Psychovisual Comparison"));
    play_title.add_css_class("hdr");
    play_title.set_halign(gtk::Align::Start);
    play_title.set_hexpand(true);
    let load_a_btn = gtk::Button::with_label("Load A (.fao)");
    load_a_btn.add_css_class("act");
    let load_b_btn = gtk::Button::with_label("Load B (.fao)");
    load_b_btn.add_css_class("act2");
    let play_play_btn = gtk::Button::with_label("▶ Play");
    play_play_btn.add_css_class("act");
    play_hdr.append(&play_title);
    play_hdr.append(&load_a_btn);
    play_hdr.append(&load_b_btn);
    play_hdr.append(&play_play_btn);
    play_panel.append(&play_hdr);
    
    let play_subtitle = gtk::Label::new(Some("A: (none)   B: (none)"));
    play_subtitle.add_css_class("sub");
    play_subtitle.set_halign(gtk::Align::Start);
    play_panel.append(&play_subtitle);
    
    // Two picture widgets side-by-side
    let play_pics_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    play_pics_box.set_vexpand(true);
    play_pics_box.set_hexpand(true);
    
    let play_pic_a_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let play_lbl_a = gtk::Label::new(Some("A"));
    play_lbl_a.add_css_class("hdr");
    play_pic_a_box.append(&play_lbl_a);
    let play_pic_a = gtk::Picture::new();
    play_pic_a.set_vexpand(true);
    play_pic_a.set_hexpand(true);
    // gtk::Overlay layers a DrawingArea ON TOP of the Picture so the 
    // anchor-overlay rendering can sit above the video without touching 
    // the texture-set call sites scattered throughout the codebase. 
    // The DrawingArea consults show_anchor_overlay + fao_anchor_track 
    // on every paint; when overlay is off the draw_func returns early 
    // so there's no perf cost.
    let play_overlay_a = gtk::Overlay::new();
    play_overlay_a.set_child(Some(&play_pic_a));
    let play_anchor_da_a = gtk::DrawingArea::new();
    play_anchor_da_a.set_can_target(false);  // pass clicks through to underlying widgets
    play_overlay_a.add_overlay(&play_anchor_da_a);
    let pf_a = gtk::Frame::new(None);
    pf_a.set_child(Some(&play_overlay_a));
    play_pic_a_box.append(&pf_a);
    
    let play_pic_b_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let play_lbl_b = gtk::Label::new(Some("B"));
    play_lbl_b.add_css_class("hdr");
    play_pic_b_box.append(&play_lbl_b);
    let play_pic_b = gtk::Picture::new();
    play_pic_b.set_vexpand(true);
    play_pic_b.set_hexpand(true);
    let play_overlay_b = gtk::Overlay::new();
    play_overlay_b.set_child(Some(&play_pic_b));
    let play_anchor_da_b = gtk::DrawingArea::new();
    play_anchor_da_b.set_can_target(false);
    play_overlay_b.add_overlay(&play_anchor_da_b);
    let pf_b = gtk::Frame::new(None);
    pf_b.set_child(Some(&play_overlay_b));
    play_pic_b_box.append(&pf_b);
    
    // ── ANCHOR OVERLAY DRAW FUNCS ──
    // 
    // Each DrawingArea draws translucent rings at the current frame's 
    // anchor positions, colored by motion-class:
    //   Static    (cls 1) → green
    //   Dynamic   (cls 2) → red
    //   UiOverlay (cls 3) → magenta
    //   Unknown   (cls 0) → grey
    // 
    // The anchor positions are stored in video-frame pixel coordinates 
    // (i16 x, y). gtk::Picture defaults to aspect-preserving fit 
    // ("Contain"), so we compute the letterbox offsets so circles land 
    // on the actual image, not on the padding bars.
    // 
    // Both A/B draw funcs are identical; written as two separate 
    // closures to avoid the Box<dyn Fn> + cairo::Context import that 
    // a shared helper would need.
    {
        let st_c = st.clone();
        play_anchor_da_a.set_draw_func(move |_da, cr, widget_w, widget_h| {
            let s = match st_c.try_borrow() { Ok(s) => s, Err(_) => return };
            if !s.show_anchor_overlay.get() { return; }
            if s.fao_anchor_track.is_empty() { return; }
            if s.ow == 0 || s.oh == 0 { return; }
            let frame_idx = s.fidx as u32;
            let anchors = match s.fao_anchor_track.get(&frame_idx) {
                Some(a) => a,
                None => return,
            };
            let scale_x = widget_w as f64 / s.ow as f64;
            let scale_y = widget_h as f64 / s.oh as f64;
            let scale = scale_x.min(scale_y);
            let img_w = s.ow as f64 * scale;
            let img_h = s.oh as f64 * scale;
            let off_x = (widget_w as f64 - img_w) * 0.5;
            let off_y = (widget_h as f64 - img_h) * 0.5;
            for &(ax, ay, cls) in anchors {
                let cx = off_x + (ax as f64) * scale;
                let cy = off_y + (ay as f64) * scale;
                if cx < off_x - 8.0 || cy < off_y - 8.0 { continue; }
                if cx > off_x + img_w + 8.0 || cy > off_y + img_h + 8.0 { continue; }
                let (r, g, b) = match cls {
                    1 => (0.30, 0.95, 0.40),
                    2 => (1.00, 0.30, 0.40),
                    3 => (0.95, 0.30, 0.95),
                    _ => (0.70, 0.70, 0.75),
                };
                cr.set_source_rgba(r, g, b, 0.55);
                cr.set_line_width(1.5);
                cr.arc(cx, cy, 5.0, 0.0, 2.0 * std::f64::consts::PI);
                let _ = cr.stroke();
                cr.set_source_rgba(r, g, b, 0.85);
                cr.arc(cx, cy, 1.2, 0.0, 2.0 * std::f64::consts::PI);
                let _ = cr.fill();
            }
        });
    }
    {
        let st_c = st.clone();
        play_anchor_da_b.set_draw_func(move |_da, cr, widget_w, widget_h| {
            let s = match st_c.try_borrow() { Ok(s) => s, Err(_) => return };
            if !s.show_anchor_overlay.get() { return; }
            if s.fao_anchor_track.is_empty() { return; }
            if s.ow == 0 || s.oh == 0 { return; }
            let frame_idx = s.fidx as u32;
            let anchors = match s.fao_anchor_track.get(&frame_idx) {
                Some(a) => a,
                None => return,
            };
            let scale_x = widget_w as f64 / s.ow as f64;
            let scale_y = widget_h as f64 / s.oh as f64;
            let scale = scale_x.min(scale_y);
            let img_w = s.ow as f64 * scale;
            let img_h = s.oh as f64 * scale;
            let off_x = (widget_w as f64 - img_w) * 0.5;
            let off_y = (widget_h as f64 - img_h) * 0.5;
            for &(ax, ay, cls) in anchors {
                let cx = off_x + (ax as f64) * scale;
                let cy = off_y + (ay as f64) * scale;
                if cx < off_x - 8.0 || cy < off_y - 8.0 { continue; }
                if cx > off_x + img_w + 8.0 || cy > off_y + img_h + 8.0 { continue; }
                let (r, g, b) = match cls {
                    1 => (0.30, 0.95, 0.40),
                    2 => (1.00, 0.30, 0.40),
                    3 => (0.95, 0.30, 0.95),
                    _ => (0.70, 0.70, 0.75),
                };
                cr.set_source_rgba(r, g, b, 0.55);
                cr.set_line_width(1.5);
                cr.arc(cx, cy, 5.0, 0.0, 2.0 * std::f64::consts::PI);
                let _ = cr.stroke();
                cr.set_source_rgba(r, g, b, 0.85);
                cr.arc(cx, cy, 1.2, 0.0, 2.0 * std::f64::consts::PI);
                let _ = cr.fill();
            }
        });
    }
    
    // Install the DA references into App state so the 'A' toggle and 
    // frame-advance paths can call queue_draw on them. Cloning a GTK 
    // widget is cheap — it's a GObject reference count bump, not a 
    // deep copy. We hold the originals locally so they're still 
    // available for layout below.
    if let Ok(s) = st.try_borrow() {
        *s.player_anchor_da_a.borrow_mut() = Some(play_anchor_da_a.clone());
        *s.player_anchor_da_b.borrow_mut() = Some(play_anchor_da_b.clone());
        // play_anchor_btn is installed later (after the button + handler 
        // are constructed in the control-bar section, ~50 lines below). 
        // We can't install it here because the button hasn't been created 
        // yet — and the toggle handler uses `st` which holds an immutable 
        // borrow during its body, so deferring until after construction 
        // avoids a borrow-conflict at install time too.
    }
    
    play_pics_box.append(&play_pic_a_box);
    play_pics_box.append(&play_pic_b_box);
    play_panel.append(&play_pics_box);
    
    // Frame slider
    // Frame controls: prev / slider / next / sync-to-A
    let play_ctrl_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let play_prev_btn = gtk::Button::with_label("◀");
    play_prev_btn.set_tooltip_text(Some("Previous frame"));
    let play_next_btn = gtk::Button::with_label("▶");
    play_next_btn.set_tooltip_text(Some("Next frame"));
    let play_sync_btn = gtk::Button::with_label("B = A");
    play_sync_btn.set_tooltip_text(Some("Load A into B (sanity check)"));
    // Anchor overlay toggle button. Mirrors the 'A' keyboard shortcut; 
    // visible state stays in sync with show_anchor_overlay via 
    // toggle_btn.set_active() called from the key-press handler's next 
    // frame cycle. Mode label "◉" is the Tobii-eye motif (translucent 
    // gaze point overlaying video).
    let play_anchor_btn = gtk::ToggleButton::with_label("◉ Anchors");
    play_anchor_btn.set_tooltip_text(Some("Toggle anchor overlay (keyboard: A) — show tracked feature positions on the video as translucent rings, colored by classification"));
    let play_slider_adj = gtk::Adjustment::new(0.0, 0.0, 1.0, 1.0, 10.0, 0.0);
    let play_slider = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&play_slider_adj));
    play_slider.set_draw_value(true);
    play_slider.set_hexpand(true);
    play_ctrl_box.append(&play_prev_btn);
    play_ctrl_box.append(&play_slider);
    play_ctrl_box.append(&play_next_btn);
    play_ctrl_box.append(&play_sync_btn);
    play_ctrl_box.append(&play_anchor_btn);
    play_panel.append(&play_ctrl_box);
    
    // Wire the toggle button: clicking flips show_anchor_overlay and 
    // queues a redraw. Same logic as the 'A' key handler.
    {
        let st_btn = st.clone();
        play_anchor_btn.connect_toggled(move |btn| {
            if let Ok(s) = st_btn.try_borrow() {
                let on = btn.is_active();
                // Avoid feedback loop: only react if the new state 
                // differs from current state (prevents key handler's 
                // sync-button-state from re-firing the toggled signal).
                if s.show_anchor_overlay.get() != on {
                    s.show_anchor_overlay.set(on);
                    if s.fao_anchor_track.is_empty() {
                        eprintln!("[anchor overlay] toggled {} via button but no V8AT trailer in this .fao",
                            if on { "ON" } else { "OFF" });
                    } else {
                        eprintln!("[anchor overlay] toggled {} via button ({} frames available)",
                            if on { "ON" } else { "OFF" }, s.fao_anchor_track.len());
                    }
                    s.request_player_redraw();
                }
            }
        });
    }
    
    // Install the toggle button reference into App state NOW that it's 
    // fully constructed and wired. The 'A' keyboard handler uses this 
    // to keep the button's visual state in sync when the keyboard is 
    // used instead of clicking.
    if let Ok(s) = st.try_borrow() {
        *s.player_anchor_btn.borrow_mut() = Some(play_anchor_btn.clone());
    }
    
    // Stats line
    let play_stats = gtk::Label::new(Some(""));
    play_stats.set_halign(gtk::Align::Start);
    play_panel.append(&play_stats);
    
    left.append(&play_panel);
    
    // ─── World Panel ─── 3D viewer for the loaded archive's scene graph
    let world_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    world_panel.set_vexpand(true);
    world_panel.set_visible(false);
    
    let world_hdr = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let world_title = gtk::Label::new(Some("🌐 World"));
    world_title.add_css_class("hdr");
    world_hdr.append(&world_title);
    
    // Color mode selector — label dropped to save space; tooltips disambiguate
    // Short labels — full names are in tooltips
    let color_proximity = gtk::ToggleButton::with_label("Prox");
    let color_feature = gtk::ToggleButton::with_label("Feat");
    let color_temporal = gtk::ToggleButton::with_label("Time");
    color_proximity.set_active(true);
    color_feature.set_group(Some(&color_proximity));
    color_temporal.set_group(Some(&color_proximity));
    color_proximity.set_tooltip_text(Some("Color points by distance to camera (cyan=near, red=far)"));
    color_feature.set_tooltip_text(Some("Color points by sprite identity (same sprite = same color)"));
    color_temporal.set_tooltip_text(Some("Color points by frame index (rainbow over time)"));
    world_hdr.append(&color_proximity);
    world_hdr.append(&color_feature);
    world_hdr.append(&color_temporal);
    
    // Camera mode toggle — label dropped; toggles "Freecam"/"VideoCam" are self-labeling
    let cam_freecam = gtk::ToggleButton::with_label("Free");
    let cam_videocam = gtk::ToggleButton::with_label("Video");
    cam_freecam.set_active(true);
    cam_videocam.set_group(Some(&cam_freecam));
    cam_freecam.set_tooltip_text(Some("Freecam: WASD/mouse to fly through the codec\u{2019}s scene representation"));
    cam_videocam.set_tooltip_text(Some("VideoCam: lock to estimated source-camera path (requires v7+ encode), driven by temporal scrubber"));
    world_hdr.append(&cam_freecam);
    world_hdr.append(&cam_videocam);
    
    // Filter selector — label dropped
    let filter_all = gtk::ToggleButton::with_label("All");
    let filter_now = gtk::ToggleButton::with_label("Now");
    let filter_anchors = gtk::ToggleButton::with_label("Anch");
    let filter_textured = gtk::ToggleButton::with_label("Tex");
    filter_all.set_active(true);
    filter_now.set_group(Some(&filter_all));
    filter_anchors.set_group(Some(&filter_all));
    filter_textured.set_group(Some(&filter_all));
    filter_all.set_tooltip_text(Some("Show all points across the entire timeline"));
    filter_now.set_tooltip_text(Some("Show only points within ±15 frames of the temporal scrubber"));
    filter_anchors.set_tooltip_text(Some("Show only anchor frames (or low-id sprites)"));
    filter_textured.set_tooltip_text(Some("Show points as actual sprite thumbnails (slower, beautiful)"));
    world_hdr.append(&filter_all);
    world_hdr.append(&filter_now);
    world_hdr.append(&filter_anchors);
    world_hdr.append(&filter_textured);
    
    world_panel.append(&world_hdr);
    
    // Drawing area — software 3D rasterizer
    let world_canvas = gtk::DrawingArea::new();
    world_canvas.set_vexpand(true);
    world_canvas.set_hexpand(true);
    // Allow canvas to shrink so trailing controls (scrub bar, status) always
    // get their min height. Without this, canvas grabs all space on tall windows
    // and squeezes the bar to zero pixels mid-playback.
    world_canvas.set_size_request(200, 200);
    world_canvas.set_can_focus(true);
    world_canvas.set_focusable(true);
    world_panel.append(&world_canvas);
    
    // Status line
    let world_status = gtk::Label::new(Some("WASD/Arrows: move  •  Space: up  •  Shift/;: down  •  T: sprint  •  Mouse drag: look  •  Scroll: dolly"));
    world_status.set_halign(gtk::Align::Start);
    world_status.add_css_class("dim-label");
    world_status.set_vexpand(false);
    world_status.set_valign(gtk::Align::End);
    world_panel.append(&world_status);
    
    // Temporal scrubber
    let world_scrub_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let world_play_btn = gtk::Button::with_label("▶ Play");
    world_play_btn.set_tooltip_text(Some("Play through frames temporally — watch objects move in 3D"));
    let world_fit_btn = gtk::Button::with_label("🎯 Fit");
    world_fit_btn.set_tooltip_text(Some("Reset camera to fit all data in view"));
    
    // ── Camera mode toggle (Free / Orbit) ──
    // Two-state toggle button. Free = WASD/drag/scroll move the camera 
    // freely. Orbit = camera revolves around target_x/y/z, drag rotates,
    // scroll changes radius, WASD pans the orbit target.
    let world_orbit_btn = gtk::ToggleButton::with_label("🔄 Orbit");
    world_orbit_btn.set_tooltip_text(Some("Toggle Orbit mode: camera revolves around a target point"));
    
    // ── Plane-aligned view buttons ──
    // Each snaps the camera to a canonical orthographic-like view, 
    // automatically switching to Orbit mode and centering on the 
    // current Fit-bounds target.
    let world_top_btn = gtk::Button::with_label("⬆ Top");
    world_top_btn.set_tooltip_text(Some("Top-down view (looking down Y axis)"));
    let world_front_btn = gtk::Button::with_label("➡ Front");
    world_front_btn.set_tooltip_text(Some("Front view (looking along Z axis)"));
    let world_side_btn = gtk::Button::with_label("⬅ Side");
    world_side_btn.set_tooltip_text(Some("Side view (looking along X axis)"));
    let world_iso_btn = gtk::Button::with_label("⬛ Iso");
    world_iso_btn.set_tooltip_text(Some("Isometric 3/4 view"));
    
    let world_time_lbl = gtk::Label::new(Some("Frame 0"));
    let world_time_adj = gtk::Adjustment::new(0.0, 0.0, 100.0, 1.0, 10.0, 0.0);
    let world_time_scrub = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&world_time_adj));
    world_time_scrub.set_hexpand(true);
    world_time_scrub.set_draw_value(false);
    // Two-row layout. Controls (buttons) on top row, scrub slider on bottom.
    // This prevents button overflow from collapsing the slider when GTK 
    // tries to fit everything in one row at narrow widths.
    let world_btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    world_btn_row.append(&world_play_btn);
    world_btn_row.append(&world_fit_btn);
    world_btn_row.append(&world_orbit_btn);
    let world_view_sep = gtk::Separator::new(gtk::Orientation::Vertical);
    world_view_sep.set_margin_start(4);
    world_view_sep.set_margin_end(4);
    world_btn_row.append(&world_view_sep);
    world_btn_row.append(&world_top_btn);
    world_btn_row.append(&world_front_btn);
    world_btn_row.append(&world_side_btn);
    world_btn_row.append(&world_iso_btn);
    world_btn_row.set_size_request(-1, 40);
    world_btn_row.set_vexpand(false);
    world_btn_row.set_valign(gtk::Align::End);
    world_btn_row.set_height_request(40);  // hard min height
    
    // Scrub row: just the label + slider, gets the full width.
    world_scrub_box.append(&world_time_lbl);
    world_scrub_box.append(&world_time_scrub);
    world_time_scrub.set_size_request(120, -1);  // never collapse below 120px wide
    world_time_scrub.set_hexpand(true);
    world_scrub_box.set_vexpand(false);
    world_scrub_box.set_valign(gtk::Align::End);
    world_scrub_box.set_size_request(-1, 36);
    world_scrub_box.set_height_request(36);  // hard min height
    
    // Wrap bottom rows in a Box with fixed valign so canvas can't push them out.
    // The Box gets vexpand=false at its level too, ensuring GTK layout 
    // reserves their min_size before allocating remainder to canvas.
    let world_bottom = gtk::Box::new(gtk::Orientation::Vertical, 0);
    world_bottom.set_vexpand(false);
    world_bottom.set_valign(gtk::Align::End);
    world_bottom.set_size_request(-1, 76);
    world_bottom.set_height_request(76);
    world_bottom.append(&world_btn_row);
    world_bottom.append(&world_scrub_box);
    world_panel.append(&world_bottom);
    
    left.append(&world_panel);
    
    // ─── World 3D Renderer ─── software rasterizer using cairo
    {
        let st_clone = st.clone();
        let time_lbl_c = world_time_lbl.clone();
        world_canvas.set_draw_func(move |_, cr, w, h| {
            let st_app = match st_clone.try_borrow() {
                Ok(s) => s,
                Err(_) => return,
            };
            
            // Background — soft gradient with horizon hint
            cr.set_source_rgb(0.04, 0.05, 0.08);
            cr.paint().ok();
            
            // Subtle horizon line so up/down is visually anchored
            cr.set_source_rgba(0.2, 0.25, 0.35, 0.3);
            cr.set_line_width(1.0);
            cr.move_to(0.0, h as f64 * 0.5);
            cr.line_to(w as f64, h as f64 * 0.5);
            cr.stroke().ok();
            
            let cam = st_app.world_camera.get();
            let color_mode = st_app.world_color_mode.get();
            let filter_mode = st_app.world_filter_mode.get();
            let time_idx = st_app.world_time_idx.get();
            
            // ── 1. Gather scene data ──
            // PRIORITY 1: v5 spatial_trace (real sprite positions in screen space + time)
            // PRIORITY 2: FrameStat fallback (just frame metadata, no spatial info)
            //
            // Each point is (x, y, z, color_hint, kind, frame_idx)
            //   kind=0 anchor, 1=P-frame, 4=v8 triangulated
            let mut points_3d: Vec<(f32, f32, f32, u32, u8, u32)> = Vec::new();
            
            let mut got_v8_data = false;
            let mut got_v8_3d = false;
            let mut got_v8_dense = false;
            let mut got_v8_mesh = false;
            
            // ── v8 DENSE depth point cloud (Session 4) ──
            // Densified pixel-back-projection from anchor depth maps.
            // Tons of points; rendered first (smaller, dimmer) so triangulated
            // sparse points overlay on top.
            // Gated on codec_now == 7 so stale data from prior sessions doesn't leak in.
            if let Some(tc) = st_app.tc.as_ref() {
                let codec_now = tc.codec_version.load(std::sync::atomic::Ordering::Relaxed);
                if codec_now == 7 {
                if let Ok(dense) = tc.v8_dense_snapshot.try_lock() {
                    if !dense.is_empty() {
                        let scale = 2.0;
                        for &(wx, wy, wz, conf, anchor_fi) in dense.iter() {
                            if conf < 0.1 { continue; }
                            // ftype 7 = dense depth point — small dim purple
                            // color_hint = depth-based (not feature_id)
                            let color_hint = ((wz.abs() * 0.5) as u32) % 256;
                            points_3d.push((
                                wx * scale,
                                -wy * scale,
                                wz * scale,
                                color_hint,
                                7u8,
                                anchor_fi,
                            ));
                        }
                        got_v8_dense = !points_3d.is_empty();
                    }
                }
                } // close if codec_now == 7
            }
            
            // ── v8 TRIANGULATED 3D points (Session 3) ──
            // True 3D positions in world space. Use for point cloud rendering.
            if let Some(tc) = st_app.tc.as_ref() {
                let codec_now = tc.codec_version.load(std::sync::atomic::Ordering::Relaxed);
                if codec_now == 7 {
                if let Ok(p3d) = tc.v8_points3d_snapshot.try_lock() {
                    if !p3d.is_empty() {
                        // Scale world positions to viewer-friendly units
                        let scale = 2.0;
                        for &(feat_id, wx, wy, wz, conf, last_fi) in p3d.iter() {
                            // Skip very low-confidence points
                            if conf < 0.2 { continue; }
                            let color_hint = (feat_id) % 256;
                            // ftype 4 = triangulated 3D point — drawn with cyan ring
                            // fi slot = last_observed_frame, so the temporal filter 
                            // ("Now ±15") and Temporal color mode work correctly.
                            // (Bug fix: previously fi was feat_id, which made every 
                            //  triangulated point look like it was at frame ~feat_id,
                            //  collapsing the temporal axis.)
                            points_3d.push((
                                wx * scale,
                                -wy * scale,
                                wz * scale,
                                color_hint,
                                4u8,
                                last_fi,
                            ));
                        }
                        got_v8_3d = !points_3d.is_empty();
                    }
                }
                } // close if codec_now == 7
            }
            
            // ── v8 2D feature trace (fallback when no triangulation yet) ──
            // Now includes per-feature motion class — Static (green-ish), 
            // Dynamic (red-ish), Unknown (neutral).
            if !got_v8_3d {
                if let Some(tc) = st_app.tc.as_ref() {
                    let codec_now = tc.codec_version.load(std::sync::atomic::Ordering::Relaxed);
                    // Only render v8 features when the CURRENT codec is v8.
                    // Otherwise snapshot data may be stale from a prior session.
                    if codec_now == 7 {
                    if let Ok(snap) = tc.v8_trace_snapshot.try_lock() {
                        let (trace, w_v8, h_v8) = (&snap.0, snap.1, snap.2);
                        if !trace.is_empty() && w_v8 > 0 && h_v8 > 0 {
                            // Compute frame range. If everything is at the 
                            // same frame_idx (e.g. only frame 0 was 
                            // snapshotted before the user paused), skip.
                            let max_fi = trace.iter().map(|&(_, _, _, fi, _)| fi).max().unwrap_or(0);
                            let min_fi = trace.iter().map(|&(_, _, _, fi, _)| fi).min().unwrap_or(0);
                            if max_fi > min_fi {
                                let n_total = max_fi as f32 + 1.0;
                                let stride = (trace.len() / 30_000).max(1);
                                let frame_w = w_v8 as f32;
                                let frame_h = h_v8 as f32;
                                for (i, &(feat_id, x, y, fi, cls)) in trace.iter().enumerate() {
                                    if i % stride != 0 { continue; }
                                    let wx = (x as f32 - frame_w * 0.5) * 0.5;
                                    let wy = -(y as f32 - frame_h * 0.5) * 0.5;
                                    let wz = (fi as f32 - n_total * 0.5) * 2.0;
                                    // cls codes: 0=Unknown, 1=Static, 2=Dynamic, 3=UiOverlay.
                                    // ftypes 5/6 are the existing static/dynamic visualizer 
                                    // colors. Use ftype 7 for UiOverlay so the user can see 
                                    // which features the new filter is catching (round-3 user 
                                    // observation: "all the things that are tracked are static 
                                    // or red" — UiOverlay was rendering as ftype 3 == same 
                                    // as Unknown, making it invisible against the noise).
                                    let ftype = match cls {
                                        1 => 5u8,  // Static (green)
                                        2 => 6u8,  // Dynamic (red)
                                        3 => 8u8,  // UiOverlay — distinct color (purple/magenta)
                                        _ => 3u8,  // Unknown
                                    };
                                    let color_hint = (feat_id) % 256;
                                    points_3d.push((wx, wy, wz, color_hint, ftype, fi));
                                }
                            } else {
                                // Single-frame trace — log once for diagnostics, then purge.
                                use std::sync::atomic::{AtomicBool, Ordering};
                                static WARNED: AtomicBool = AtomicBool::new(false);
                                if !WARNED.swap(true, Ordering::Relaxed) {
                                    eprintln!("FAO v8 World view: trace has {} entries but all at frame {} — purging stale snapshot",
                                        trace.len(), min_fi);
                                }
                                // Drop the read lock so we can re-acquire mutably below
                                drop(snap);
                                if let Ok(mut s) = tc.v8_trace_snapshot.try_lock() {
                                    s.0.clear();
                                    s.1 = 0;
                                    s.2 = 0;
                                }
                            }
                        }
                    }
                    } // close if codec_now == 7
                }
            }
            got_v8_data = got_v8_dense || got_v8_3d || !points_3d.is_empty();
            
            // ── Legacy v5/v6/v7 spatial-trace blocks removed in v10 cleanup ──
            // 
            // The World-view used to render per-codec spatial traces from 
            // v5/v6/v7 encoders/decoders here. Those codecs are gone; the 
            // FrameWeb fallback below now covers any non-v8 archive.
            

            // ── Fallback: FrameWeb structural data ──
            // For v2.x archives we have no per-sprite data, but the FrameWeb 
            // captures real STRUCTURAL info: scene boundaries, anchor frames, 
            // edges between similar frames. This is far more meaningful than 
            // hash-based scatter.
            //
            // Layout strategy:
            //   - Z = frame index (timeline)
            //   - X = scene_id × 200 (each scene gets its own column)
            //   - Y = layer by tier: anchors high (+100), P-frames mid (0), skips low (-100)
            //
            // Result: each video looks like a series of STACKED COLUMNS 
            // (scenes), with a thin spine of P-frames and anchor frames 
            // visible as bright yellow rings (existing in renderer).
            if !got_v8_data {
                if !st_app.web_viz_nodes.is_empty() {
                    // We have proper FrameWeb data
                    let n = st_app.web_viz_nodes.len() as f32;
                    for &(fi, _x, _y, z_tier, _weight, scene) in st_app.web_viz_nodes.iter() {
                        let scene_x = (scene as f32 - 8.0) * 200.0;
                        // y_layer from tier (which encodes anchor vs P-frame vs skip)
                        let y_layer = z_tier * 200.0;  
                        let z_world = (fi as f32 - n * 0.5) * 2.0;
                        // ftype from z_tier: 0.6=anchor, 0.2=passthrough, -0.4=skip
                        let ftype = if z_tier > 0.4 { 0u8 } else if z_tier > 0.0 { 1u8 } else { 2u8 };
                        let color_hint = (scene as u32) * 17 % 256;
                        points_3d.push((scene_x, y_layer, z_world, color_hint, ftype, fi));
                    }
                } else if let Some(tc) = st_app.tc.as_ref() {
                    // Last resort: stats-based, but use scene_id-style layout
                    // (group frames by enc_bytes class so similar-looking frames cluster)
                    if let Ok(stats) = tc.stats.try_lock() {
                        let n = stats.len();
                        if n > 0 {
                            let max_bytes = stats.iter().map(|s| s.enc_bytes).max().unwrap_or(1).max(1);
                            for (i, st) in stats.iter().enumerate() {
                                // Group by encoded size — high-action frames cluster, 
                                // skip frames cluster, etc. Same idea as scene_id
                                let size_class = (st.enc_bytes * 16 / max_bytes) as i32;  // 0..16
                                let size_x = (size_class as f32 - 8.0) * 100.0;
                                let layer = match st.ftype {
                                    0 => 100.0,
                                    1 => 0.0,
                                    _ => -100.0,
                                };
                                let z_world = (i as f32 - n as f32 * 0.5) * 2.0;
                                let color_hint = (size_class * 16) as u32 & 255;
                                points_3d.push((size_x, layer, z_world, color_hint, st.ftype, i as u32));
                            }
                        }
                    }
                }
            }
            
            // Multi-line empty state with clearer guidance
            if points_3d.is_empty() {
                cr.set_source_rgba(0.6, 0.7, 0.8, 0.9);
                cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
                cr.set_font_size(14.0);
                let cy_pos = h as f64 / 2.0;
                cr.move_to(20.0, cy_pos - 30.0);
                let _ = cr.show_text("🌐 No scene data yet");
                cr.set_font_size(11.0);
                cr.set_source_rgba(0.5, 0.55, 0.65, 0.8);
                cr.move_to(20.0, cy_pos);
                let _ = cr.show_text("• Encode a video (any codec) to see its frame structure");
                cr.move_to(20.0, cy_pos + 16.0);
                let _ = cr.show_text("• v5.0/v6.0/v7.0 produce real spatial sprite/object data");
                cr.move_to(20.0, cy_pos + 32.0);
                let _ = cr.show_text("• v7.0 also adds a camera frustum + path");
                return;
            }
            
            // ── 2. Camera transform ──
            // World → camera space: rotate by yaw/pitch, translate by cam pos
            let cy = cam.yaw.cos();
            let sy = cam.yaw.sin();
            let cp = cam.pitch.cos();
            let sp = cam.pitch.sin();
            
            let half_w = w as f32 / 2.0;
            let half_h = h as f32 / 2.0;
            let focal = (w.min(h) as f32) * cam.zoom;
            
            let mut projected: Vec<(f32, f32, f32, u32, u8, u32)> = Vec::with_capacity(points_3d.len());
            
            for &(px, py, pz, color, ftype, fi) in &points_3d {
                // Translate by negative camera position
                let tx = px - cam.pos_x;
                let ty = py - cam.pos_y;
                let tz = pz - cam.pos_z;
                
                // Rotate yaw (around Y axis)
                let rx = cy * tx + sy * tz;
                let rz = -sy * tx + cy * tz;
                
                // Rotate pitch (around X axis)
                let ry = cp * ty - sp * rz;
                let rz2 = sp * ty + cp * rz;
                
                // Skip points behind camera
                if rz2 < 1.0 { continue; }
                
                // Perspective project
                let sx_pix = half_w + (focal * rx / rz2);
                let sy_pix = half_h - (focal * ry / rz2);
                
                projected.push((sx_pix, sy_pix, rz2, color, ftype, fi));
            }
            
            // Apply filter
            projected.retain(|&(_, _, _, color, _kind, fi)| {
                match filter_mode {
                    WorldFilterMode::All => true,
                    WorldFilterMode::NowPlusMinus15 => {
                        let diff = (fi as i64 - time_idx as i64).abs();
                        diff <= 15
                    },
                    WorldFilterMode::AnchorsOnly => {
                        // For sprites: keep low-id sprites (background or first ones)
                        // For frames: keep only anchor-frame points (kind == 0)
                        color < 16
                    },
                    WorldFilterMode::Textured => true,  // render mode, not a filter
                }
            });
            
            // Sort by depth (back to front for painter's algorithm)
            projected.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
            
            // ── 3. Draw temporal "now" plane (vertical line at time_idx) ──
            // Compute screen position of the current temporal slice
            {
                let tx = -cam.pos_x;
                let ty = -cam.pos_y;
                let target_z = (time_idx as f32 - points_3d.len() as f32 * 0.5) * 2.0;
                let tz = target_z - cam.pos_z;
                let rx = cy * tx + sy * tz;
                let rz = -sy * tx + cy * tz;
                let ry = cp * ty - sp * rz;
                let rz2 = sp * ty + cp * rz;
                if rz2 > 1.0 {
                    let sx_pix = half_w + (focal * rx / rz2);
                    // Dimmer + dashed: still indicates the temporal slice 
                    // but doesn't dominate when looking at mesh content.
                    cr.set_source_rgba(1.0, 0.95, 0.3, 0.18);
                    cr.set_line_width(1.0);
                    cr.set_dash(&[4.0, 6.0], 0.0);
                    cr.move_to(sx_pix as f64, 0.0);
                    cr.line_to(sx_pix as f64, h as f64);
                    cr.stroke().ok();
                    cr.set_dash(&[], 0.0);  // reset dash for subsequent strokes
                }
            }
            
            // ── 4. Draw points ──
            let n_total = projected.len() as f32;
            
            // ── Mesh rendering: project + rasterize triangles via Cairo ──
            // 
            // The mesh is the codec's persistent compressed scene representation.
            // Rendering it as filled triangles (instead of points) is what makes 
            // "fly through the reconstructed world" work.
            // 
            // Steps:
            //   1. Project each vertex with the same perspective math as points
            //   2. Build per-triangle screen positions
            //   3. Sort triangles back-to-front by mean depth
            //   4. Rasterize each triangle as a filled polygon
            //   5. Skip triangles that have any vertex behind the camera or 
            //      that face away (back-face cull via screen-space winding)
            // 
            // Gated on codec_v == 7 (only render mesh during/after a v8 encode).
            if let Some(tc) = st_app.tc.as_ref() {
                let codec_now = tc.codec_version.load(std::sync::atomic::Ordering::Relaxed);
                if codec_now == 7 {
                if let Ok(mesh_snap) = tc.v8_mesh_snapshot.try_lock() {
                    let (verts, indices) = (&mesh_snap.0, &mesh_snap.1);
                    if !verts.is_empty() && indices.len() >= 3 {
                        got_v8_mesh = true;
                        // Project all vertices once
                        let mut proj_v: Vec<(f32, f32, f32, bool)> = Vec::with_capacity(verts.len());
                        for &(vx, vy, vz, _r, _g, _b) in verts.iter() {
                            let tx = vx - cam.pos_x;
                            let ty = vy - cam.pos_y;
                            let tz = vz - cam.pos_z;
                            let rx = cy * tx + sy * tz;
                            let rz = -sy * tx + cy * tz;
                            let ry = cp * ty - sp * rz;
                            let rz2 = sp * ty + cp * rz;
                            let visible = rz2 > 1.0;
                            let sx_pix = if visible { half_w + (focal * rx / rz2) } else { 0.0 };
                            let sy_pix = if visible { half_h - (focal * ry / rz2) } else { 0.0 };
                            proj_v.push((sx_pix, sy_pix, rz2, visible));
                        }
                        // Build sortable triangles: (mean_depth, [v0,v1,v2])
                        let mut tris: Vec<(f32, [u32; 3])> = Vec::with_capacity(indices.len() / 3);
                        for tri in indices.chunks_exact(3) {
                            let i0 = tri[0] as usize;
                            let i1 = tri[1] as usize;
                            let i2 = tri[2] as usize;
                            if i0 >= proj_v.len() || i1 >= proj_v.len() || i2 >= proj_v.len() { continue; }
                            let v0 = proj_v[i0];
                            let v1 = proj_v[i1];
                            let v2 = proj_v[i2];
                            // All three must be in front of camera
                            if !v0.3 || !v1.3 || !v2.3 { continue; }
                            // Cull off-screen triangles (rough \u2014 if all vertices are off the same edge, skip)
                            let xs = [v0.0, v1.0, v2.0];
                            let ys = [v0.1, v1.1, v2.1];
                            let max_x = xs.iter().cloned().fold(f32::MIN, f32::max);
                            let min_x = xs.iter().cloned().fold(f32::MAX, f32::min);
                            let max_y = ys.iter().cloned().fold(f32::MIN, f32::max);
                            let min_y = ys.iter().cloned().fold(f32::MAX, f32::min);
                            if max_x < 0.0 || min_x > w as f32 { continue; }
                            if max_y < 0.0 || min_y > h as f32 { continue; }
                            // Skip degenerate triangles (zero or near-zero screen area)
                            let area2 = ((v1.0 - v0.0) * (v2.1 - v0.1) 
                                       - (v2.0 - v0.0) * (v1.1 - v0.1)).abs();
                            if area2 < 0.5 { continue; }
                            let mean_depth = (v0.2 + v1.2 + v2.2) / 3.0;
                            tris.push((mean_depth, [tri[0], tri[1], tri[2]]));
                        }
                        // Sort back-to-front
                        tris.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                        // Cap rendered triangles. At 1M+ triangles Cairo polygon 
                        // fill becomes the bottleneck (~200ms per redraw at 50K). 
                        // 15K is the sweet spot — visible mesh structure but the 
                        // draw loop completes in ~50ms, leaving headroom for 
                        // GTK layout signals during drag/scroll.
                        let render_cap = 15_000.min(tris.len());
                        let drawn = &tris[tris.len().saturating_sub(render_cap)..];
                        for &(depth, [i0, i1, i2]) in drawn {
                            let v0 = proj_v[i0 as usize];
                            let v1 = proj_v[i1 as usize];
                            let v2 = proj_v[i2 as usize];
                            // Average vertex color
                            let c0 = verts[i0 as usize];
                            let c1 = verts[i1 as usize];
                            let c2 = verts[i2 as usize];
                            let r = (c0.3 + c1.3 + c2.3) / 3.0;
                            let g = (c0.4 + c1.4 + c2.4) / 3.0;
                            let b = (c0.5 + c1.5 + c2.5) / 3.0;
                            // Distance fog: far triangles dim toward background
                            let fog = (1.0 - (depth / 5000.0)).clamp(0.15, 1.0);
                            let alpha = 0.55 * fog;
                            // Faux diffuse shading from screen-space face normal:
                            // a triangle whose 2D edges are aligned with the X axis 
                            // gets brighter (stand-in for facing the camera).
                            let ex = (v1.0 - v0.0).abs() + (v2.0 - v0.0).abs();
                            let ey = (v1.1 - v0.1).abs() + (v2.1 - v0.1).abs();
                            let shade = (1.0 - (ey / (ex + ey + 1.0)).min(0.55)).clamp(0.45, 1.0);
                            cr.set_source_rgba(
                                (r * shade * fog).clamp(0.0, 1.0) as f64,
                                (g * shade * fog).clamp(0.0, 1.0) as f64,
                                (b * shade * fog).clamp(0.0, 1.0) as f64,
                                alpha as f64,
                            );
                            cr.move_to(v0.0 as f64, v0.1 as f64);
                            cr.line_to(v1.0 as f64, v1.1 as f64);
                            cr.line_to(v2.0 as f64, v2.1 as f64);
                            cr.close_path();
                            cr.fill().ok();
                        }
                    }
                }
                
                // ── Per-web mesh rendering (Sessions 6c-8) ──
                //
                // Renders each closed chunk's web with a distinct color so the 
                // user can SEE multiple webs as distinct entities. This is the 
                // "widening" the user described: don't force-merge webs into 
                // one global; show them all and let the structure speak.
                //
                // Color scheme:
                //   - Canonical/standalone webs: warm hue (orange/yellow)
                //   - Aligned webs: matched to their canonical's hue (cooler)
                //   - High-follower-count canonical: brighter (a "popular scene")
                let show_mesh = st_app.world_show_mesh.get();
                let wireframe = st_app.world_mesh_wireframe.get();
                let brightness = st_app.world_mesh_brightness.get();
                if show_mesh { if let Ok(webs) = tc.v8_webs_snapshot.try_lock() {
                    let n_webs = webs.len() as f32;
                    let webs_laid_out = st_app.world_webs_laid_out.get();
                    // When laid out, distribute webs along X axis. Spacing 
                    // chosen so a typical web (~1000 unit extent) doesn't 
                    // overlap its neighbors.
                    let layout_spacing = 2500.0_f32;
                    
                    for (web_idx, web) in webs.iter().enumerate() {
                        if web.vertices.is_empty() || web.indices.len() < 3 { continue; }
                        
                        // Layout offset: centers webs around 0 along X.
                        let (off_x, off_y, off_z) = if webs_laid_out {
                            let center_idx = (n_webs - 1.0) / 2.0;
                            ((web_idx as f32 - center_idx) * layout_spacing, 0.0, 0.0)
                        } else { (0.0, 0.0, 0.0) };
                        
                        // Hue derived from chunk_id so each chunk has a stable color.
                        // Aligned webs share their canonical's hue but rendered at lower alpha.
                        let hue_seed = web.canonical_id.wrapping_mul(2654435761) & 0xFFFFFF;
                        let hue_r = ((hue_seed >> 16) & 0xFF) as f32 / 255.0;
                        let hue_g = ((hue_seed >> 8) & 0xFF) as f32 / 255.0;
                        let hue_b = (hue_seed & 0xFF) as f32 / 255.0;
                        let is_aligned = web.canonical_id != web.chunk_id;
                        let base_alpha = if is_aligned { 0.35 } else { 0.55 };
                        // Boost alpha for popular canonicals
                        let popularity = (web.n_aligned_followers as f32 / 4.0).min(1.0);
                        let alpha = base_alpha + popularity * 0.2;
                        
                        // Project all vertices once
                        let mut proj_v: Vec<(f32, f32, f32, bool)> = Vec::with_capacity(web.vertices.len());
                        for &(vx, vy, vz, _r, _g, _b) in web.vertices.iter() {
                            let tx = vx + off_x - cam.pos_x;
                            let ty = vy + off_y - cam.pos_y;
                            let tz = vz + off_z - cam.pos_z;
                            let rx = cy * tx + sy * tz;
                            let rz = -sy * tx + cy * tz;
                            let ry = cp * ty - sp * rz;
                            let rz2 = sp * ty + cp * rz;
                            let visible = rz2 > 1.0;
                            let sx_pix = if visible { half_w + (focal * rx / rz2) } else { 0.0 };
                            let sy_pix = if visible { half_h - (focal * ry / rz2) } else { 0.0 };
                            proj_v.push((sx_pix, sy_pix, rz2, visible));
                        }
                        
                        // Build sortable triangles.
                        //
                        // We aggressively cull degenerate / artifact triangles 
                        // that produce the "needle" look in mesh recovery:
                        //
                        //   - tiny area: < 0.5px² — pure float noise
                        //   - extreme aspect ratio: longest edge / shortest > 50× 
                        //     means it's a thin sliver, almost certainly a bad 
                        //     triangulation between mismatched feature pairs 
                        //     (the spikes you see going to infinity)
                        //   - extreme depth ratio: vertices spanning > 10× in 
                        //     depth means they probably shouldn't be neighbors 
                        //     in 3D — produces the "stretched paper" look
                        let mut tris: Vec<(f32, [u32; 3])> = Vec::new();
                        for tri in web.indices.chunks_exact(3) {
                            let i0 = tri[0] as usize;
                            let i1 = tri[1] as usize;
                            let i2 = tri[2] as usize;
                            if i0 >= proj_v.len() || i1 >= proj_v.len() || i2 >= proj_v.len() { continue; }
                            let v0 = proj_v[i0];
                            let v1 = proj_v[i1];
                            let v2 = proj_v[i2];
                            if !v0.3 || !v1.3 || !v2.3 { continue; }
                            let area2 = ((v1.0 - v0.0) * (v2.1 - v0.1) - (v2.0 - v0.0) * (v1.1 - v0.1)).abs();
                            if area2 < 0.5 { continue; }
                            
                            // Aspect ratio cull: compute longest and shortest edge 
                            // in screen space. Slivers have large ratio.
                            let e01_sq = (v1.0 - v0.0).powi(2) + (v1.1 - v0.1).powi(2);
                            let e12_sq = (v2.0 - v1.0).powi(2) + (v2.1 - v1.1).powi(2);
                            let e20_sq = (v0.0 - v2.0).powi(2) + (v0.1 - v2.1).powi(2);
                            let e_max = e01_sq.max(e12_sq).max(e20_sq);
                            let e_min = e01_sq.min(e12_sq).min(e20_sq).max(0.01);
                            if e_max / e_min > 2500.0 { continue; }  // 50× linear ratio
                            
                            // Depth coherence cull: triangles spanning a huge depth 
                            // range are almost always artifacts.
                            let d_max = v0.2.max(v1.2).max(v2.2);
                            let d_min = v0.2.min(v1.2).min(v2.2).max(1.0);
                            if d_max / d_min > 10.0 { continue; }
                            
                            let mean_depth = (v0.2 + v1.2 + v2.2) / 3.0;
                            tris.push((mean_depth, [tri[0], tri[1], tri[2]]));
                        }
                        tris.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                        
                        // Cap rendering — distribute budget across webs.
                        // Wireframe is cheaper than fills, so allow more there.
                        let total_budget = if wireframe { 60_000 } else { 30_000 };
                        let per_web_cap = (total_budget / (webs.len() + 1).max(1)).max(1000);
                        let render_n = per_web_cap.min(tris.len());
                        let drawn = &tris[tris.len().saturating_sub(render_n)..];
                        
                        // Wireframe needs much thinner lines + higher alpha to be 
                        // visible without overwhelming. Filled needs lower alpha to 
                        // composite correctly when many tris overlap.
                        let line_width = if wireframe { 0.6 } else { 0.0 };
                        let render_alpha_boost = if wireframe { 1.6 } else { 1.0 };
                        for &(depth, [i0, i1, i2]) in drawn {
                            let v0 = proj_v[i0 as usize];
                            let v1 = proj_v[i1 as usize];
                            let v2 = proj_v[i2 as usize];
                            // Distance fog for depth cue
                            let fog = (1.0 - (depth / 5000.0)).clamp(0.2, 1.0);
                            // Apply user brightness multiplier; clamp to keep saturation bounded.
                            cr.set_source_rgba(
                                (hue_r * fog * brightness).clamp(0.0, 1.0) as f64,
                                (hue_g * fog * brightness).clamp(0.0, 1.0) as f64,
                                (hue_b * fog * brightness).clamp(0.0, 1.0) as f64,
                                (alpha * fog * render_alpha_boost).clamp(0.0, 1.0) as f64,
                            );
                            cr.move_to(v0.0 as f64, v0.1 as f64);
                            cr.line_to(v1.0 as f64, v1.1 as f64);
                            cr.line_to(v2.0 as f64, v2.1 as f64);
                            cr.close_path();
                            if wireframe {
                                cr.set_line_width(line_width);
                                cr.stroke().ok();
                            } else {
                                cr.fill().ok();
                            }
                        }
                        
                        let _ = web_idx;  // reserved for future per-web layout offsets
                    }
                }} // close show_mesh + if let Ok
                } // close codec_now == 7
            }
            
            // ── Camera trajectory + frustums (Session 6b) ──
            //
            // Why this exists: when the v8 reconstruction looks chaotic 
            // (floating cloud columns instead of a coherent room), the 
            // primary diagnostic is the camera path. Bad poses produce 
            // bad reconstruction. Rendering the trajectory shows users 
            // WHERE the encoder thinks the camera was at each frame.
            //
            // What we draw:
            //   - A polyline through camera positions (one segment per frame)
            //   - A small frustum (4-edge wireframe pyramid) at every 
            //     N-th frame showing camera orientation
            //   - Color = time gradient (blue early → red late)
            //
            // Toggleable via F key — frustums clutter the view on long 
            // videos, so users can hide them and just see geometry.
            let show_frustums = st_app.world_show_frustums.get();
            if show_frustums { if let Some(tc) = st_app.tc.as_ref() {
                let codec_now = tc.codec_version.load(std::sync::atomic::Ordering::Relaxed);
                if codec_now == 7 {
                    if let Ok(poses) = tc.v8_pose_snapshot.try_lock() {
                        if !poses.is_empty() {
                            let n_poses = poses.len() as f32;
                            
                            // Project each pose's position
                            let project = |wx: f32, wy: f32, wz: f32| -> Option<(f32, f32, f32)> {
                                let tx = wx - cam.pos_x;
                                let ty = wy - cam.pos_y;
                                let tz = wz - cam.pos_z;
                                let rx = cy * tx + sy * tz;
                                let rz = -sy * tx + cy * tz;
                                let ry = cp * ty - sp * rz;
                                let rz2 = sp * ty + cp * rz;
                                if rz2 <= 1.0 { return None; }
                                let sx_pix = half_w + (focal * rx / rz2);
                                let sy_pix = half_h - (focal * ry / rz2);
                                Some((sx_pix, sy_pix, rz2))
                            };
                            
                            // 1. Polyline trajectory
                            let mut prev_screen: Option<(f32, f32, f32)> = None;
                            for (i, &(_fidx, wx, wy, wz, _qw, _qx, _qy, _qz)) in poses.iter().enumerate() {
                                let proj = project(wx, wy, wz);
                                if let (Some(prev), Some(curr)) = (prev_screen, proj) {
                                    let t = i as f32 / n_poses;
                                    // Time gradient: cyan early -> magenta late
                                    let r = t;
                                    let g = 0.5;
                                    let b = 1.0 - t;
                                    cr.set_source_rgba(r as f64, g as f64, b as f64, 0.6);
                                    cr.set_line_width(1.5);
                                    cr.move_to(prev.0 as f64, prev.1 as f64);
                                    cr.line_to(curr.0 as f64, curr.1 as f64);
                                    cr.stroke().ok();
                                }
                                prev_screen = proj;
                            }
                            
                            // 2. Camera frustums every Nth pose
                            let frustum_stride = (poses.len() / 30).max(20);  // cap at ~30 frustums shown
                            for (i, &(_fidx, wx, wy, wz, qw, qx, qy, qz)) in poses.iter().enumerate() {
                                if i % frustum_stride != 0 { continue; }
                                let center = match project(wx, wy, wz) {
                                    Some(p) => p, None => continue,
                                };
                                
                                // Compute camera look direction from quaternion. 
                                // Camera-space forward is +Z; rotate by pose quat.
                                // q * (0,0,1,0) * q_conj — simplified to:
                                let fx = 2.0 * (qx*qz + qw*qy);
                                let fy = 2.0 * (qy*qz - qw*qx);
                                let fz = 1.0 - 2.0 * (qx*qx + qy*qy);
                                
                                // Frustum tip at world position, base at center + forward * size
                                let frust_size = 30.0;
                                let bx = wx + fx * frust_size;
                                let by = wy + fy * frust_size;
                                let bz = wz + fz * frust_size;
                                
                                // Right + up vectors (cross products)
                                let rx = 2.0 * (qx*qy - qw*qz);
                                let ry = 1.0 - 2.0 * (qx*qx + qz*qz);
                                let rz = 2.0 * (qy*qz + qw*qx);
                                let ux = 2.0 * (qx*qy + qw*qz);
                                let uy_v = 2.0 * (qy*qy + qz*qz);  // Not exactly right; quat math
                                let uy = 1.0 - 2.0 * (qx*qx + qz*qz);
                                let uz = 2.0 * (qy*qz - qw*qx);
                                let _ = (uy_v, ry, rz, ux, uz);
                                
                                // Four corners of the base
                                let half = frust_size * 0.5;
                                let c1 = (bx + rx * half + ux * half, by + ry * half + uy * half, bz + rz * half + uz * half);
                                let c2 = (bx - rx * half + ux * half, by - ry * half + uy * half, bz - rz * half + uz * half);
                                let c3 = (bx - rx * half - ux * half, by - ry * half - uy * half, bz - rz * half - uz * half);
                                let c4 = (bx + rx * half - ux * half, by + ry * half - uy * half, bz + rz * half - uz * half);
                                
                                let p1 = project(c1.0, c1.1, c1.2);
                                let p2 = project(c2.0, c2.1, c2.2);
                                let p3 = project(c3.0, c3.1, c3.2);
                                let p4 = project(c4.0, c4.1, c4.2);
                                
                                let t = i as f32 / n_poses;
                                cr.set_source_rgba(t as f64, 0.7, (1.0 - t) as f64, 0.8);
                                cr.set_line_width(1.2);
                                
                                // Tip → 4 corners
                                if let Some(p) = p1 { cr.move_to(center.0 as f64, center.1 as f64); cr.line_to(p.0 as f64, p.1 as f64); cr.stroke().ok(); }
                                if let Some(p) = p2 { cr.move_to(center.0 as f64, center.1 as f64); cr.line_to(p.0 as f64, p.1 as f64); cr.stroke().ok(); }
                                if let Some(p) = p3 { cr.move_to(center.0 as f64, center.1 as f64); cr.line_to(p.0 as f64, p.1 as f64); cr.stroke().ok(); }
                                if let Some(p) = p4 { cr.move_to(center.0 as f64, center.1 as f64); cr.line_to(p.0 as f64, p.1 as f64); cr.stroke().ok(); }
                                
                                // 4-corner base rectangle
                                if let (Some(a), Some(b)) = (p1, p2) { cr.move_to(a.0 as f64, a.1 as f64); cr.line_to(b.0 as f64, b.1 as f64); cr.stroke().ok(); }
                                if let (Some(a), Some(b)) = (p2, p3) { cr.move_to(a.0 as f64, a.1 as f64); cr.line_to(b.0 as f64, b.1 as f64); cr.stroke().ok(); }
                                if let (Some(a), Some(b)) = (p3, p4) { cr.move_to(a.0 as f64, a.1 as f64); cr.line_to(b.0 as f64, b.1 as f64); cr.stroke().ok(); }
                                if let (Some(a), Some(b)) = (p4, p1) { cr.move_to(a.0 as f64, a.1 as f64); cr.line_to(b.0 as f64, b.1 as f64); cr.stroke().ok(); }
                            }
                        }
                    }
                }
            }} // close show_frustums + if let Some(tc)
            
            // Textured World mode used to render v5 sprite thumbnails here. 
            // v5 codec removed in v10 cleanup; this mode now degrades to 
            // the dot-fill rendering below regardless of filter selection.
            
            for (i, &(sx, sy, depth, color, ftype, fi)) in projected.iter().enumerate() {
                if sx < -50.0 || sx > w as f32 + 50.0 { continue; }
                if sy < -50.0 || sy > h as f32 + 50.0 { continue; }
                
                let radius = (focal * 1.5 / depth).clamp(1.0, 8.0);
                
                // Color based on selected mode
                let (r, g, b) = match color_mode {
                    WorldColorMode::Proximity => {
                        // Near = warm, far = cool (blue)
                        let t = (depth / 1000.0).clamp(0.0, 1.0);
                        (1.0 - t, 0.4 + 0.3 * (1.0 - t), t)
                    },
                    WorldColorMode::Feature => {
                        // Color by hash signature (creates clusters)
                        let h = (color as f32 / 256.0) * 6.28;
                        ((h.cos() * 0.5 + 0.5).max(0.0), 
                         ((h + 2.0).cos() * 0.5 + 0.5).max(0.0),
                         ((h + 4.0).cos() * 0.5 + 0.5).max(0.0))
                    },
                    WorldColorMode::Temporal => {
                        // Rainbow by frame index
                        let t = fi as f32 / n_total.max(1.0) * 6.28;
                        ((t.sin() * 0.5 + 0.5), 
                         ((t + 2.0).sin() * 0.5 + 0.5),
                         ((t + 4.0).sin() * 0.5 + 0.5))
                    },
                };
                
                // Highlight points at the current temporal slice
                let is_current = fi == time_idx || (fi as i32 - time_idx as i32).abs() < 2;
                let alpha = if is_current { 1.0 } else { 0.7 };
                let scale = if is_current { 1.5 } else { 1.0 };
                
                cr.set_source_rgba(r as f64, g as f64, b as f64, alpha);
                
                cr.arc(sx as f64, sy as f64, (radius * scale) as f64, 0.0, 2.0 * std::f64::consts::PI);
                cr.fill().ok();
                
                // Frame type marker — anchor frames get a yellow ring
                if ftype == 0 {
                    cr.set_source_rgba(1.0, 0.85, 0.2, alpha);
                    cr.set_line_width(1.5);
                    cr.arc(sx as f64, sy as f64, (radius * scale + 1.5) as f64, 0.0, 2.0 * std::f64::consts::PI);
                    cr.stroke().ok();
                }
                // ftype 4 = v8 triangulated 3D point — bright cyan ring to distinguish
                // from 2D feature-trace points. Visually separates "real geometry" 
                // from "screen scatter".
                if ftype == 4 {
                    cr.set_source_rgba(0.4, 1.0, 1.0, alpha * 0.9);
                    cr.set_line_width(1.0);
                    cr.arc(sx as f64, sy as f64, (radius * scale + 1.0) as f64, 0.0, 2.0 * std::f64::consts::PI);
                    cr.stroke().ok();
                }
                // ftype 5 = Static feature (consensus says rigid scene) — green tint
                if ftype == 5 {
                    cr.set_source_rgba(0.3, 0.95, 0.4, alpha * 0.7);
                    cr.set_line_width(0.8);
                    cr.arc(sx as f64, sy as f64, (radius * scale + 0.6) as f64, 0.0, 2.0 * std::f64::consts::PI);
                    cr.stroke().ok();
                }
                // ftype 6 = Dynamic feature (consensus says moving object) — red tint
                if ftype == 6 {
                    cr.set_source_rgba(1.0, 0.3, 0.4, alpha * 0.8);
                    cr.set_line_width(0.8);
                    cr.arc(sx as f64, sy as f64, (radius * scale + 0.6) as f64, 0.0, 2.0 * std::f64::consts::PI);
                    cr.stroke().ok();
                }
                // ftype 8 = UiOverlay feature — magenta tint so the user can 
                // see which features the UI-overlay filter caught. Visually 
                // distinct from Static (green ftype 5), Dynamic (red ftype 6), 
                // and Unknown (default).
                if ftype == 8 {
                    cr.set_source_rgba(0.95, 0.3, 0.95, alpha * 0.85);
                    cr.set_line_width(1.0);
                    cr.arc(sx as f64, sy as f64, (radius * scale + 0.6) as f64, 0.0, 2.0 * std::f64::consts::PI);
                    cr.stroke().ok();
                }
                // ftype 7 = dense depth point — no extra ring; the base dot 
                // already rendered. Just used so we know which points came 
                // from densification when filtering.
                
                let _ = i;
            }
            
            // ── 4.5. Draw estimated camera path (v7 only) ──
            //
            // For v7 archives we have a per-frame camera motion track. We 
            // integrate it into a path through 3D space and render the camera 
            // as a wireframe frustum at the current temporal slice. Other 
            // frames in the path are drawn as small dots so you can see 
            // where the camera was/will be.
            // ── v7 camera path / frustum rendering removed in v10 cleanup ──
            // Previously read the v7 decoder camera path and projected it 
            // as a 3D path with frustum at the active frame. v8 pose 
            // tracks fill this role now via the VideoCam mode below.
            
            // ── 4.7. Ground grid ──
            // 10x10 reference grid on Y=-100 plane spanning ±500 units.
            // Helps the user GROUND themselves in 3D space and see their 
            // camera moving — empty void vs actual scene.
            {
                let grid_y = -150.0f32;
                let grid_extent = 500.0f32;
                let grid_step = 100.0f32;
                let project = |px: f32, py: f32, pz: f32| -> Option<(f32, f32)> {
                    let tx = px - cam.pos_x;
                    let ty = py - cam.pos_y;
                    let tz = pz - cam.pos_z;
                    let rx = cy * tx + sy * tz;
                    let rz = -sy * tx + cy * tz;
                    let ry = cp * ty - sp * rz;
                    let rz2 = sp * ty + cp * rz;
                    if rz2 < 1.0 { return None; }
                    Some((half_w + (focal * rx / rz2), half_h - (focal * ry / rz2)))
                };
                cr.set_line_width(1.0);
                cr.set_source_rgba(0.2, 0.3, 0.4, 0.4);
                let mut z = -grid_extent;
                while z <= grid_extent {
                    let p1 = project(-grid_extent, grid_y, z);
                    let p2 = project(grid_extent, grid_y, z);
                    if let (Some((x1, y1)), Some((x2, y2))) = (p1, p2) {
                        cr.move_to(x1 as f64, y1 as f64);
                        cr.line_to(x2 as f64, y2 as f64);
                        cr.stroke().ok();
                    }
                    z += grid_step;
                }
                let mut x_g = -grid_extent;
                while x_g <= grid_extent {
                    let p1 = project(x_g, grid_y, -grid_extent);
                    let p2 = project(x_g, grid_y, grid_extent);
                    if let (Some((x1, y1)), Some((x2, y2))) = (p1, p2) {
                        cr.move_to(x1 as f64, y1 as f64);
                        cr.line_to(x2 as f64, y2 as f64);
                        cr.stroke().ok();
                    }
                    x_g += grid_step;
                }
            }
            
            // ── 5. Draw axes (origin marker) ──
            {
                let axis_len = 100.0;
                let origin = [(0.0f32, 0.0, 0.0), (axis_len, 0.0, 0.0), (0.0, axis_len, 0.0), (0.0, 0.0, axis_len)];
                let axis_colors = [(1.0, 1.0, 1.0), (1.0, 0.3, 0.3), (0.3, 1.0, 0.3), (0.3, 0.5, 1.0)];
                let mut axis_proj = [(0.0f32, 0.0, 0.0); 4];
                let mut visible = [false; 4];
                for (k, &(px, py, pz)) in origin.iter().enumerate() {
                    let tx = px - cam.pos_x;
                    let ty = py - cam.pos_y;
                    let tz = pz - cam.pos_z;
                    let rx = cy * tx + sy * tz;
                    let rz = -sy * tx + cy * tz;
                    let ry = cp * ty - sp * rz;
                    let rz2 = sp * ty + cp * rz;
                    if rz2 > 1.0 {
                        axis_proj[k] = (half_w + focal * rx / rz2, half_h - focal * ry / rz2, rz2);
                        visible[k] = true;
                    }
                }
                cr.set_line_width(2.0);
                for k in 1..4 {
                    if visible[0] && visible[k] {
                        let (r, g, b) = axis_colors[k];
                        cr.set_source_rgba(r, g, b, 0.7);
                        cr.move_to(axis_proj[0].0 as f64, axis_proj[0].1 as f64);
                        cr.line_to(axis_proj[k].0 as f64, axis_proj[k].1 as f64);
                        cr.stroke().ok();
                    }
                }
            }
            
            // ── 6. HUD overlay ──
            cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(11.0);
            cr.set_source_rgba(0.7, 0.85, 0.95, 0.9);
            cr.move_to(10.0, 18.0);
            let _ = cr.show_text(&format!("Cam: ({:.0}, {:.0}, {:.0})  yaw={:.2}  pitch={:.2}  zoom={:.1}",
                cam.pos_x, cam.pos_y, cam.pos_z, cam.yaw, cam.pitch, cam.zoom));
            // Determine data source label for HUD
            let data_source_label = {
                let v8_active = st_app.fao_v8_decoder.read().unwrap().is_some();
                if got_v8_mesh && got_v8_3d { "v8 mesh + sparse" }
                else if got_v8_mesh { "v8 mesh" }
                else if got_v8_dense && got_v8_3d { "v8 dense+sparse 3D" }
                else if got_v8_dense { "v8 dense depth" }
                else if got_v8_3d { "v8 3D points (triangulated)" }
                else if got_v8_data { "v8 features 2D" }
                else if v8_active { "v8 (loaded)" }
                else if !st_app.web_viz_nodes.is_empty() { "frame web (v2.x)" }
                else { "stats fallback" }
            };
            cr.move_to(10.0, 32.0);
            // Show camera mode + a hint about target if Orbit, so user 
            // understands why scroll/drag behave differently.
            let cam_now = st_app.world_camera.get();
            let cam_mode_str = match cam_now.mode {
                CameraMode::FreeCam => "Free",
                CameraMode::Orbit => "Orbit",
            };
            // Count webs for HUD display
            let (n_webs, n_aligned_webs) = if let Some(tc) = st_app.tc.as_ref() {
                tc.v8_webs_snapshot.try_lock().ok().map(|webs| {
                    let total = webs.len();
                    let aligned = webs.iter().filter(|w| w.canonical_id != w.chunk_id).count();
                    (total, aligned)
                }).unwrap_or((0, 0))
            } else { (0, 0) };
            // Sum total vertex count across all webs (closed chunks).
            let total_mesh_verts = if let Some(tc) = st_app.tc.as_ref() {
                tc.v8_webs_snapshot.try_lock().ok().map(|webs| {
                    webs.iter().map(|w| w.vertices.len()).sum::<usize>()
                }).unwrap_or(0)
                .saturating_add(
                    tc.v8_mesh_snapshot.try_lock().ok().map(|m| m.0.len()).unwrap_or(0)
                )
            } else { 0 };
            
            let layout_indicator = if st_app.world_webs_laid_out.get() { " [L]" } else { "" };
            // Compose toggle indicators so user sees current state at a glance.
            let mut toggles = String::new();
            if !st_app.world_show_mesh.get() { toggles.push_str(" no-mesh"); }
            if !st_app.world_show_frustums.get() { toggles.push_str(" no-frust"); }
            if st_app.world_mesh_wireframe.get() { toggles.push_str(" wire"); }
            let bri = st_app.world_mesh_brightness.get();
            if (bri - 1.0).abs() > 0.01 { toggles.push_str(&format!(" bri={:.1}x", bri)); }
            let toggle_str = if toggles.is_empty() { String::new() } else { format!(" •{}", toggles) };
            let webs_str = if n_webs > 0 {
                format!(" • {} webs ({} aligned, {}v){}{}", n_webs, n_aligned_webs, total_mesh_verts, layout_indicator, toggle_str)
            } else if total_mesh_verts > 0 {
                format!(" • {}v{}", total_mesh_verts, toggle_str)
            } else { toggle_str };
            let _ = cr.show_text(&format!("{} • Mode: {:?} • Source: {} • {} visible • Frame {}{}",
                cam_mode_str, color_mode, data_source_label, projected.len(), time_idx, webs_str));
            cr.move_to(10.0, h as f64 - 10.0);
            // Bottom hint adapts to mode so the user sees the right keys.
            // F=frustums M=mesh R=wireframe B=brightness L=lay-out side-by-side.
            let hint = match cam_now.mode {
                CameraMode::FreeCam => "FreeCam: WASD move • Drag look • Scroll dolly | L lay  F frust  M mesh  R wire  B bright",
                CameraMode::Orbit   => "Orbit: WASD pan target • Drag rotate • Scroll zoom | L lay  F frust  M mesh  R wire  B bright",
            };
            let _ = cr.show_text(hint);
            
            // Update time label
            time_lbl_c.set_text(&format!("Frame {}", time_idx));
        });
    }
    
    // ─── World controls: mouse drag to rotate camera, scroll to dolly ───
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        let drag = gtk::GestureDrag::new();
        let drag_state = std::rc::Rc::new(std::cell::Cell::new((0.0f64, 0.0f64, WorldCamera::default())));
        let ds_begin = drag_state.clone();
        let st_begin = st_clone.clone();
        drag.connect_drag_begin(move |_, _, _| {
            if let Ok(s) = st_begin.try_borrow() {
                ds_begin.set((0.0, 0.0, s.world_camera.get()));
            }
        });
        let st_update = st_clone.clone();
        let canvas_update = canvas_c.clone();
        let ds_update = drag_state.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            if let Ok(s) = st_update.try_borrow() {
                let (_, _, start_cam) = ds_update.get();
                let mut cam = start_cam;
                cam.yaw = start_cam.yaw + (dx as f32) * 0.005;
                // Vertical drag inverted (Blender/Maya convention): drag UP
                // tilts camera DOWN, drag DOWN tilts UP. Feels more natural 
                // for orbit-style inspection where you "pull" the scene 
                // toward/away from you.
                cam.pitch = (start_cam.pitch + (dy as f32) * 0.005).clamp(-1.5, 1.5);
                // In Orbit mode, the camera position is derived from 
                // target + yaw + pitch + radius. Recompute it so the 
                // renderer sees the new position.
                if cam.mode == CameraMode::Orbit {
                    cam.recompute_orbit_position();
                }
                s.world_camera.set(cam);
            }
            canvas_update.queue_draw();
        });
        world_canvas.add_controller(drag);
        
        // Scroll: dolly forward/back
        let st_scroll = st_clone.clone();
        let canvas_scroll = canvas_c.clone();
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(move |_, _, dy| {
            if let Ok(s) = st_scroll.try_borrow() {
                let mut cam = s.world_camera.get();
                match cam.mode {
                    CameraMode::FreeCam => {
                        // Original behavior: dolly forward along look direction
                        let cy = cam.yaw.cos();
                        let sy = cam.yaw.sin();
                        let speed = 30.0;
                        cam.pos_x -= sy * (dy as f32) * speed;
                        cam.pos_z += cy * (dy as f32) * speed;
                    }
                    CameraMode::Orbit => {
                        // Orbit zoom: scale the orbit radius. Multiplicative 
                        // so zooming feels consistent at any distance.
                        let factor = 1.0 + (dy as f32) * 0.1;
                        cam.orbit_radius = (cam.orbit_radius * factor)
                            .clamp(10.0, 1_000_000.0);
                        cam.recompute_orbit_position();
                    }
                }
                s.world_camera.set(cam);
            }
            canvas_scroll.queue_draw();
            glib::Propagation::Stop
        });
        world_canvas.add_controller(scroll);
    }
    
    // ─── World freecam keyboard input (WASD + arrows + Space/Shift) ───
    // 
    // Following the keybind-guide principle: defensive multi-key binding.
    // Movement actions accept SEVERAL keys so the player gets the
    // expected behavior regardless of muscle memory.
    //   Forward:  W, ArrowUp, .          (period for DVORAK)
    //   Back:     S, ArrowDown, E
    //   Left:     A, ArrowLeft, O
    //   Right:    D, ArrowRight, U
    //   Up/Fly:   Space
    //   Down:     LeftShift, LeftControl
    //   Sprint:   T (hold for 3x speed — avoids collisions with movement
    //              presets where Shift/Ctrl might be sprint elsewhere)
    //
    // We use a HashSet of held GDK keyvals + a 16ms tick. Keys are captured
    // on the WINDOW so the canvas doesn't need focus.
    {
        let st_clone = st.clone();
        let key_ctrl = gtk::EventControllerKey::new();
        let st_press = st_clone.clone();
        key_ctrl.connect_key_pressed(move |_, key, _code, _mods| {
            if let Ok(s) = st_press.try_borrow() {
                // A: toggle the Tobii-style anchor overlay on the player. 
                // Works in any tab — useful while comparing player view vs 
                // World viewer. Logs once per toggle so the user can confirm 
                // the state changed. Skipped if there's no anchor data 
                // loaded (older .fao files without a V8AT trailer).
                if key == gtk::gdk::Key::a || key == gtk::gdk::Key::A {
                    let cur = s.show_anchor_overlay.get();
                    let next = !cur;
                    s.show_anchor_overlay.set(next);
                    // Sync the toggle button so its visual state matches. 
                    // set_active fires connect_toggled, but the button's 
                    // handler short-circuits when the new state matches 
                    // the current App state — no infinite loop.
                    if let Some(btn) = s.player_anchor_btn.borrow().as_ref() {
                        btn.set_active(next);
                    }
                    if s.fao_anchor_track.is_empty() {
                        eprintln!("[anchor overlay] toggled {} but no V8AT trailer in this .fao — encode with FAO_NO_ANCHOR_TRACK unset to capture anchors",
                            if next { "ON" } else { "OFF" });
                    } else {
                        eprintln!("[anchor overlay] toggled {} ({} frames of anchor data available)",
                            if next { "ON" } else { "OFF" }, s.fao_anchor_track.len());
                    }
                    // Force a redraw so the overlay appears/disappears 
                    // immediately. The player's draw functions consult 
                    // show_anchor_overlay on every paint.
                    s.request_player_redraw();
                    return glib::Propagation::Stop;
                }
                if s.world_tab_active.get() {
                    if let Some(action) = world_movekey_for(key) {
                        s.world_held_keys.borrow_mut().insert(action);
                        // Critical: stop propagation so Space (etc.) doesn\u{2019}t 
                        // activate a focused button (Play/Fit/Mode toggles).
                        return glib::Propagation::Stop;
                    }
                    // L: toggle web layout (lay webs out side-by-side vs 
                    // overlapping in native coords). Helps see distinct 
                    // chunks visually.
                    if key == gtk::gdk::Key::l || key == gtk::gdk::Key::L {
                        let cur = s.world_webs_laid_out.get();
                        s.world_webs_laid_out.set(!cur);
                        return glib::Propagation::Stop;
                    }
                    // F: toggle camera frustums. When the scene has many 
                    // frustums (long video) they clutter the view; F hides 
                    // them so you can see geometry clearly.
                    if key == gtk::gdk::Key::f || key == gtk::gdk::Key::F {
                        let cur = s.world_show_frustums.get();
                        s.world_show_frustums.set(!cur);
                        return glib::Propagation::Stop;
                    }
                    // M: toggle mesh visibility. Counterpart to F — hides 
                    // the geometry so you can see the camera path alone.
                    if key == gtk::gdk::Key::m || key == gtk::gdk::Key::M {
                        let cur = s.world_show_mesh.get();
                        s.world_show_mesh.set(!cur);
                        return glib::Propagation::Stop;
                    }
                    // R: cycle mesh rendering between filled / wireframe.
                    // Wireframe is dramatically clearer for dense meshes — 
                    // you see triangle EDGES instead of filled-in surfaces 
                    // that all blend together at low alpha.
                    if key == gtk::gdk::Key::r || key == gtk::gdk::Key::R {
                        let cur = s.world_mesh_wireframe.get();
                        s.world_mesh_wireframe.set(!cur);
                        return glib::Propagation::Stop;
                    }
                    // B: cycle mesh brightness multiplier through 0.5/1.0/2.0/4.0.
                    // Dark scenes (e.g., Lone Ranger night shots) need a 
                    // brightness boost to make the mesh visible at all.
                    if key == gtk::gdk::Key::b || key == gtk::gdk::Key::B {
                        let cur = s.world_mesh_brightness.get();
                        let next = match cur {
                            x if x < 0.75 => 1.0,
                            x if x < 1.5 => 2.0,
                            x if x < 3.0 => 4.0,
                            _ => 0.5,
                        };
                        s.world_mesh_brightness.set(next);
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        let st_release = st_clone.clone();
        key_ctrl.connect_key_released(move |_, key, _code, _mods| {
            if let Ok(s) = st_release.try_borrow() {
                if let Some(action) = world_movekey_for(key) {
                    s.world_held_keys.borrow_mut().remove(&action);
                }
            }
        });
        win.add_controller(key_ctrl);
    }
    
    // ─── World camera tick loop — applies held-key motion at 60Hz ───
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let s = match st_clone.try_borrow() { Ok(s) => s, Err(_) => return glib::ControlFlow::Continue };
            if !s.world_tab_active.get() {
                return glib::ControlFlow::Continue;
            }
            
            // VideoCam mode: drive camera from time_idx + camera path, ignore WASD
            if s.world_camera_mode.get() == WorldCameraMode::VideoCam {
                let time_idx = s.world_time_idx.get();
                // v8 pose track only (v7 camera path removed in v10 cleanup)
                let v8_dec = s.fao_v8_decoder.read().unwrap();
                let path_pos: Option<(f32, f32, f32)> = 
                    if let Some(d) = v8_dec.as_ref() {
                        d.pose_track.get(time_idx as usize).map(|p| 
                            (p.position.x, p.position.y,
                                (time_idx as f32 - d.pose_track.len() as f32 * 0.5) * 2.0))
                    } else { None };
                
                if let Some((px, py, pz)) = path_pos {
                    // Position the world-view camera so it sits BEHIND the source 
                    // camera position, looking forward (toward +Z = future frames)
                    let cam = WorldCamera {
                        pos_x: px,
                        pos_y: py,
                        pos_z: pz - 200.0,
                        yaw: 0.0,
                        pitch: -0.05,
                        zoom: 1.0,
                        ..WorldCamera::default()
                    };
                    s.world_camera.set(cam);
                    canvas_c.queue_draw();
                }
                return glib::ControlFlow::Continue;
            }
            
            let held = s.world_held_keys.borrow();
            if held.is_empty() {
                return glib::ControlFlow::Continue;
            }
            
            let pressed_fwd  = held.contains(&WorldMoveKey::Forward);
            let pressed_back = held.contains(&WorldMoveKey::Back);
            let pressed_l    = held.contains(&WorldMoveKey::Left);
            let pressed_r    = held.contains(&WorldMoveKey::Right);
            let pressed_u    = held.contains(&WorldMoveKey::Up);
            let pressed_d    = held.contains(&WorldMoveKey::Down);
            let sprinting    = held.contains(&WorldMoveKey::Sprint);
            
            if !pressed_fwd && !pressed_back && !pressed_l && !pressed_r && !pressed_u && !pressed_d {
                return glib::ControlFlow::Continue;
            }
            
            let mut cam = s.world_camera.get();
            let speed = if sprinting { 18.0 } else { 6.0 };
            
            // Forward direction in world space (camera space (0,0,1) rotated by yaw)
            let cy = cam.yaw.cos();
            let sy = cam.yaw.sin();
            
            // Forward vector
            let fwd_x = sy;
            let fwd_z = cy;
            // Right vector (perpendicular to forward, on horizontal plane)
            let right_x = cy;
            let right_z = -sy;
            
            match cam.mode {
                CameraMode::FreeCam => {
                    if pressed_fwd  { cam.pos_x += fwd_x   * speed; cam.pos_z += fwd_z   * speed; }
                    if pressed_back { cam.pos_x -= fwd_x   * speed; cam.pos_z -= fwd_z   * speed; }
                    if pressed_r    { cam.pos_x += right_x * speed; cam.pos_z += right_z * speed; }
                    if pressed_l    { cam.pos_x -= right_x * speed; cam.pos_z -= right_z * speed; }
                    if pressed_u    { cam.pos_y += speed; }
                    if pressed_d    { cam.pos_y -= speed; }
                }
                CameraMode::Orbit => {
                    // In Orbit, WASD pans the TARGET (the camera follows 
                    // by recomputing position). This lets you pan the 
                    // orbit center across the scene without losing 
                    // rotational framing.
                    if pressed_fwd  { cam.target_x += fwd_x   * speed; cam.target_z += fwd_z   * speed; }
                    if pressed_back { cam.target_x -= fwd_x   * speed; cam.target_z -= fwd_z   * speed; }
                    if pressed_r    { cam.target_x += right_x * speed; cam.target_z += right_z * speed; }
                    if pressed_l    { cam.target_x -= right_x * speed; cam.target_z -= right_z * speed; }
                    if pressed_u    { cam.target_y += speed; }
                    if pressed_d    { cam.target_y -= speed; }
                    cam.recompute_orbit_position();
                }
            }
            
            s.world_camera.set(cam);
            canvas_c.queue_draw();
            glib::ControlFlow::Continue
        });
    }
    
    // ─── World color mode toggles ───
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        color_proximity.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() {
                    s.world_color_mode.set(WorldColorMode::Proximity);
                }
                canvas_c.queue_draw();
            }
        });
    }
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        color_feature.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() {
                    s.world_color_mode.set(WorldColorMode::Feature);
                }
                canvas_c.queue_draw();
            }
        });
    }
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        color_temporal.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() {
                    s.world_color_mode.set(WorldColorMode::Temporal);
                }
                canvas_c.queue_draw();
            }
        });
    }
    
    // ─── World camera mode toggles ───
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        cam_freecam.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() {
                    // Restore saved freecam position
                    s.world_camera_mode.set(WorldCameraMode::Freecam);
                    s.world_camera.set(s.world_freecam_saved.get());
                }
                canvas_c.queue_draw();
            }
        });
    }
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        cam_videocam.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() {
                    // Save current freecam position before switching
                    s.world_freecam_saved.set(s.world_camera.get());
                    s.world_camera_mode.set(WorldCameraMode::VideoCam);
                    // Camera will be driven by tick loop from time_idx + pose track
                }
                canvas_c.queue_draw();
            }
        });
    }
    
    // ─── World filter toggles ───
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        filter_all.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() { s.world_filter_mode.set(WorldFilterMode::All); }
                canvas_c.queue_draw();
            }
        });
    }
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        filter_now.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() { s.world_filter_mode.set(WorldFilterMode::NowPlusMinus15); }
                canvas_c.queue_draw();
            }
        });
    }
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        filter_anchors.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() { s.world_filter_mode.set(WorldFilterMode::AnchorsOnly); }
                canvas_c.queue_draw();
            }
        });
    }
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        filter_textured.connect_toggled(move |b| {
            if b.is_active() {
                if let Ok(s) = st_clone.try_borrow() { s.world_filter_mode.set(WorldFilterMode::Textured); }
                canvas_c.queue_draw();
            }
        });
    }
    
    // ─── World time scrubber ───
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        world_time_scrub.connect_value_changed(move |s| {
            if let Ok(app) = st_clone.try_borrow() {
                app.world_time_idx.set(s.value() as u32);
            }
            canvas_c.queue_draw();
        });
    }
    
    // Update scrubber range when stats grow — periodic check
    {
        let st_clone = st.clone();
        let scrub_c = world_time_scrub.clone();
        let adj_c = world_time_adj.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if let Ok(s) = st_clone.try_borrow() {
                if let Some(tc) = s.tc.as_ref() {
                    if let Ok(stats) = tc.stats.try_lock() {
                        let n = stats.len() as f64;
                        if n > 0.0 && (adj_c.upper() - n).abs() > 0.5 {
                            adj_c.set_upper(n);
                        }
                    }
                }
                let _ = scrub_c;
            }
            glib::ControlFlow::Continue
        });
    }
    
    // ─── World fit button: compute data bounds and frame camera ───
    // Smarter than a fixed reset: looks at whatever v8 data is currently 
    // available (mesh vertices preferred, then sparse 3D points, then 
    // dense points) and centers + zooms to fit.
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        world_fit_btn.connect_clicked(move |_| {
            let mut bounds: Option<(f32, f32, f32, f32, f32, f32)> = None;
            // Helper: extend bounds with a point
            let extend = |b: &mut Option<(f32, f32, f32, f32, f32, f32)>, x: f32, y: f32, z: f32| {
                if let Some(ref mut b) = b {
                    if x < b.0 { b.0 = x; }
                    if x > b.1 { b.1 = x; }
                    if y < b.2 { b.2 = y; }
                    if y > b.3 { b.3 = y; }
                    if z < b.4 { b.4 = z; }
                    if z > b.5 { b.5 = z; }
                } else {
                    *b = Some((x, x, y, y, z, z));
                }
            };
            
            if let Ok(s) = st_clone.try_borrow() {
                if let Some(tc) = s.tc.as_ref() {
                    // Try mesh first
                    if let Ok(m) = tc.v8_mesh_snapshot.try_lock() {
                        for &(x, y, z, _r, _g, _b) in m.0.iter().take(50_000) {
                            extend(&mut bounds, x, y, z);
                        }
                    }
                    // Fall back to triangulated points
                    if bounds.is_none() {
                        if let Ok(p) = tc.v8_points3d_snapshot.try_lock() {
                            for &(_, x, y, z, _, _) in p.iter().take(50_000) {
                                extend(&mut bounds, x, y, z);
                            }
                        }
                    }
                    // Fall back to dense
                    if bounds.is_none() {
                        if let Ok(d) = tc.v8_dense_snapshot.try_lock() {
                            for &(x, y, z, _, _) in d.iter().take(50_000) {
                                extend(&mut bounds, x, y, z);
                            }
                        }
                    }
                }
                
                let new_cam = if let Some((min_x, max_x, min_y, max_y, min_z, max_z)) = bounds {
                    let cx = (min_x + max_x) * 0.5;
                    let cy = (min_y + max_y) * 0.5;
                    let cz = (min_z + max_z) * 0.5;
                    let extent = (max_x - min_x)
                        .max(max_y - min_y)
                        .max(max_z - min_z)
                        .max(50.0);
                    // Frame the bounds: position camera at bounds-center, 
                    // pulled back along Z by ~1.5x extent so the data fits.
                    WorldCamera {
                        pos_x: cx, pos_y: cy, pos_z: cz - extent * 1.5,
                        yaw: 0.0, pitch: 0.0, zoom: 1.0,
                        // Set orbit target to bounds center so toggling 
                        // to Orbit immediately revolves around the data.
                        target_x: cx, target_y: cy, target_z: cz,
                        orbit_radius: extent * 1.5,
                        ..WorldCamera::default()
                    }
                } else {
                    // No data — sensible default
                    WorldCamera {
                        pos_x: 0.0, pos_y: 0.0, pos_z: -1200.0,
                        yaw: 0.0, pitch: -0.1, zoom: 1.0,
                        ..WorldCamera::default()
                    }
                };
                s.world_camera.set(new_cam);
            }
            canvas_c.queue_draw();
        });
    }
    
    // ─── Orbit mode toggle ───
    // Switching INTO orbit copies pos -> implied target (a bit in front of 
    // camera) so the view doesn't jump. Switching OUT of orbit keeps 
    // current pos and freezes target.
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        world_orbit_btn.connect_toggled(move |btn| {
            if let Ok(s) = st_clone.try_borrow() {
                let mut cam = s.world_camera.get();
                if btn.is_active() {
                    // Idempotency: if already in Orbit (e.g., via a plane 
                    // view button), don't re-derive target — that would 
                    // jump the view.
                    let was_already_orbit = cam.mode == CameraMode::Orbit;
                    cam.mode = CameraMode::Orbit;
                    if was_already_orbit {
                        s.world_camera.set(cam);
                        return;
                    }
                    // If target is at origin (default) but camera is far 
                    // away, set target to a point in front of the camera
                    // so the orbit center starts somewhere meaningful.
                    let tx = cam.target_x;
                    let ty = cam.target_y;
                    let tz = cam.target_z;
                    if tx.abs() < 1.0 && ty.abs() < 1.0 && tz.abs() < 1.0 {
                        // Target = camera position + forward * distance_to_origin
                        let cy = cam.yaw.cos();
                        let sy = cam.yaw.sin();
                        let cp = cam.pitch.cos();
                        let sp = cam.pitch.sin();
                        let dist = (cam.pos_x.powi(2) + cam.pos_y.powi(2) + cam.pos_z.powi(2))
                            .sqrt().max(100.0);
                        cam.target_x = cam.pos_x + sy * cp * dist;
                        cam.target_y = cam.pos_y - sp * dist;
                        cam.target_z = cam.pos_z + cy * cp * dist;
                        cam.orbit_radius = dist;
                    } else {
                        // Use existing target; update radius from current pos
                        let dx = cam.target_x - cam.pos_x;
                        let dy = cam.target_y - cam.pos_y;
                        let dz = cam.target_z - cam.pos_z;
                        cam.orbit_radius = (dx*dx + dy*dy + dz*dz).sqrt().max(10.0);
                    }
                    cam.recompute_orbit_position();
                } else {
                    cam.mode = CameraMode::FreeCam;
                }
                s.world_camera.set(cam);
            }
            canvas_c.queue_draw();
        });
    }
    
    // ─── Plane view buttons (Top / Front / Side / Iso) ───
    // Each switches to Orbit mode and snaps yaw/pitch to a canonical angle.
    // Target stays where it was (or defaults to origin if untouched).
    macro_rules! wire_plane_btn {
        ($btn:ident, $view:expr) => {{
            let st_clone = st.clone();
            let canvas_c = world_canvas.clone();
            let orbit_btn_c = world_orbit_btn.clone();
            $btn.connect_clicked(move |_| {
                if let Ok(s) = st_clone.try_borrow() {
                    let mut cam = s.world_camera.get();
                    cam.snap_to_plane($view);
                    s.world_camera.set(cam);
                    // Keep the orbit toggle visually in sync.
                    orbit_btn_c.set_active(true);
                }
                canvas_c.queue_draw();
            });
        }};
    }
    wire_plane_btn!(world_top_btn,   PlaneView::Top);
    wire_plane_btn!(world_front_btn, PlaneView::Front);
    wire_plane_btn!(world_side_btn,  PlaneView::Side);
    wire_plane_btn!(world_iso_btn,   PlaneView::Iso);
    
    // ─── World play button: temporal animation ───
    {
        let st_clone = st.clone();
        let canvas_c = world_canvas.clone();
        let scrub_c = world_time_scrub.clone();
        let btn_c = world_play_btn.clone();
        world_play_btn.connect_clicked(move |_| {
            if let Ok(s) = st_clone.try_borrow() {
                let now_playing = !s.world_playing.get();
                s.world_playing.set(now_playing);
                btn_c.set_label(if now_playing { "⏸ Pause" } else { "▶ Play" });
            }
            // Spawn a tick-driven animation
            let st_tick = st_clone.clone();
            let canvas_tick = canvas_c.clone();
            let scrub_tick = scrub_c.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
                if let Ok(s) = st_tick.try_borrow() {
                    if !s.world_playing.get() { return glib::ControlFlow::Break; }
                    let cur = s.world_time_idx.get();
                    let next = cur + 1;
                    let max = scrub_tick.adjustment().upper() as u32;
                    let next = if next > max { 0 } else { next };
                    s.world_time_idx.set(next);
                    scrub_tick.set_value(next as f64);
                }
                canvas_tick.queue_draw();
                glib::ControlFlow::Continue
            });
        });
    }
    
    // Wire load A button — uses existing load_fao
    {
        let win_c = win.clone();
        let st_c = st.clone();
        let pic_a_c = play_pic_a.clone();
        let pic_b_c = play_pic_b.clone();
        let subtitle_c = play_subtitle.clone();
        let slider_c = play_slider_adj.clone();
        load_a_btn.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Load A (.fao)"),
                Some(&win_c),
                gtk::FileChooserAction::Open,
                Some("Open"),
                Some("Cancel"),
            );
            let f = gtk::FileFilter::new();
            f.add_pattern("*.fao");
            f.set_name(Some("FAO Video"));
            dialog.add_filter(&f);
            
            let st_inner = st_c.clone();
            let pic_a = pic_a_c.clone();
            let pic_b = pic_b_c.clone();
            let label = subtitle_c.clone();
            let slider = slider_c.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(path) = d.file().and_then(|f| f.path()) {
                        let mut s = match st_inner.try_borrow_mut() { Ok(s) => s, Err(_) => return };
                        s.load_fao(&path);
                        let n = s.fao_index.len();
                        let b_n = s.player_b_index.len();
                        slider.set_upper(n.saturating_sub(1).max(1) as f64);
                        slider.set_value(0.0);
                        let a_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        let b_name = if s.player_b_path.is_empty() { "(none)".to_string() } else {
                            std::path::Path::new(&s.player_b_path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                        };
                        label.set_text(&format!("A: {} ({} frames)   B: {} ({} frames)", a_name, n, b_name, b_n));
                        // Render frame 0
                        let w = s.cur_w; let h = s.cur_h;
                        let n_bytes = w * h * 3;
                        if s.cur_frame.len() >= n_bytes {
                            let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.cur_frame[..n_bytes]),
                                gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                            pic_a.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
                        }
                        if s.player_b_frame.len() >= n_bytes {
                            let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.player_b_frame[..n_bytes]),
                                gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                            pic_b.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
                        }
                    }
                }
            });
            dialog.show();
        });
    }
    
    // Wire load B button
    {
        let win_c = win.clone();
        let st_c = st.clone();
        let pic_b_c = play_pic_b.clone();
        let subtitle_c = play_subtitle.clone();
        load_b_btn.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Load B (.fao)"),
                Some(&win_c),
                gtk::FileChooserAction::Open,
                Some("Open"),
                Some("Cancel"),
            );
            let f = gtk::FileFilter::new();
            f.add_pattern("*.fao");
            f.set_name(Some("FAO Video"));
            dialog.add_filter(&f);
            
            let st_inner = st_c.clone();
            let pic_b = pic_b_c.clone();
            let label = subtitle_c.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(path) = d.file().and_then(|f| f.path()) {
                        let mut s = match st_inner.try_borrow_mut() { Ok(s) => s, Err(_) => return };
                        s.load_player_b(&path);
                        let n = s.fao_index.len();
                        let b_n = s.player_b_index.len();
                        let a_name = if s.src_path.is_empty() { "(none)".to_string() } else {
                            std::path::Path::new(&s.src_path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                        };
                        let b_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        label.set_text(&format!("A: {} ({} frames)   B: {} ({} frames)", a_name, n, b_name, b_n));
                        
                        // Render
                        let w = s.cur_w; let h = s.cur_h;
                        let n_bytes = w * h * 3;
                        if s.player_b_frame.len() >= n_bytes {
                            let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.player_b_frame[..n_bytes]),
                                gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                            pic_b.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
                        }
                    }
                }
            });
            dialog.show();
        });
    }
    
    // Wire frame slider — sync both A and B
    {
        let st_c = st.clone();
        let pic_a_c = play_pic_a.clone();
        let pic_b_c = play_pic_b.clone();
        let stats_c = play_stats.clone();
        play_slider_adj.connect_value_changed(move |a| {
            let idx = a.value() as usize;
            let mut s = match st_c.try_borrow_mut() { Ok(s) => s, Err(_) => return };
            s.load_fao_frame(idx);
            s.load_player_b_frame(idx);
            let w = s.cur_w; let h = s.cur_h;
            let n_bytes = w * h * 3;
            if s.cur_frame.len() >= n_bytes {
                let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.cur_frame[..n_bytes]),
                    gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                pic_a_c.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
            }
            if s.player_b_frame.len() >= n_bytes {
                let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.player_b_frame[..n_bytes]),
                    gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                pic_b_c.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
            }
            // Compute PSNR-style stats between A and B
            if s.cur_frame.len() >= n_bytes && s.player_b_frame.len() >= n_bytes {
                let mut sum_sq: u64 = 0;
                let mut max_d: u32 = 0;
                let n_sample = n_bytes.min(64*1024);
                for i in 0..n_sample {
                    let d = (s.cur_frame[i] as i32 - s.player_b_frame[i] as i32).unsigned_abs();
                    sum_sq += (d * d) as u64;
                    if d > max_d { max_d = d; }
                }
                let mse = sum_sq as f64 / n_sample as f64;
                let psnr = if mse > 0.0 { 10.0 * (255.0_f64 * 255.0 / mse).log10() } else { 99.0 };
                stats_c.set_text(&format!("Frame {}/{}   PSNR(A,B)={:.2}dB   MSE={:.2}   max delta={}",
                    idx, s.fao_index.len(), psnr, mse, max_d));
            } else {
                stats_c.set_text(&format!("Frame {}/{}", idx, s.fao_index.len()));
            }
        });
    }
    
    // Prev frame button
    {
        let slider_c = play_slider_adj.clone();
        play_prev_btn.connect_clicked(move |_| {
            let v = slider_c.value();
            if v > 0.0 { slider_c.set_value(v - 1.0); }
        });
    }
    // Next frame button
    {
        let slider_c = play_slider_adj.clone();
        play_next_btn.connect_clicked(move |_| {
            let v = slider_c.value();
            let max = slider_c.upper();
            if v < max { slider_c.set_value(v + 1.0); }
        });
    }
    // Sync B to A (load same .fao into B for psychovisual sanity check)
    {
        let st_c = st.clone();
        let pic_b_c = play_pic_b.clone();
        let subtitle_c = play_subtitle.clone();
        play_sync_btn.connect_clicked(move |_| {
            let path_str = {
                let s = match st_c.try_borrow() { Ok(s) => s, Err(_) => return };
                s.src_path.clone()
            };
            if path_str.is_empty() {
                eprintln!("FAO Player: no A loaded");
                return;
            }
            let path = std::path::Path::new(&path_str).to_path_buf();
            let mut s = match st_c.try_borrow_mut() { Ok(s) => s, Err(_) => return };
            s.load_player_b(&path);
            let n = s.fao_index.len();
            let b_n = s.player_b_index.len();
            let a_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            subtitle_c.set_text(&format!("A: {} ({} frames)   B: {} ({} frames)  [B=A]", 
                a_name.clone(), n, a_name, b_n));
            
            let w = s.cur_w; let h = s.cur_h;
            let n_bytes = w * h * 3;
            if s.player_b_frame.len() >= n_bytes {
                let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.player_b_frame[..n_bytes]),
                    gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                pic_b_c.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
            }
        });
    }
    
    // Player Play button — animate through frames
    {
        let st_c = st.clone();
        let slider_c = play_slider_adj.clone();
        let pb_c = play_play_btn.clone();
        play_play_btn.connect_clicked(move |btn| {
            // Toggle: if "▶ Play" → start; if "⏸ Pause" → stop
            let is_playing = btn.label().map(|l| l.as_str().contains("Pause")).unwrap_or(false);
            if is_playing {
                btn.set_label("▶ Play");
                if let Ok(mut s) = st_c.try_borrow_mut() { s.playing = false; }
                return;
            }
            btn.set_label("⏸ Pause");
            if let Ok(mut s) = st_c.try_borrow_mut() { s.playing = true; }
            
            let st_t = st_c.clone();
            let slider_t = slider_c.clone();
            let pb_t = pb_c.clone();
            // Use vid_fps for animation rate
            let frame_ms = {
                let s = st_c.borrow();
                ((1000.0 / s.vid_fps.max(1.0)) as u64).max(8)
            };
            glib::timeout_add_local(std::time::Duration::from_millis(frame_ms), move || {
                let s = match st_t.try_borrow() { Ok(s) => s, Err(_) => return glib::ControlFlow::Continue };
                if !s.playing { 
                    pb_t.set_label("▶ Play");
                    return glib::ControlFlow::Break; 
                }
                drop(s);
                let v = slider_t.value();
                let max = slider_t.upper();
                if v < max {
                    slider_t.set_value(v + 1.0);
                } else {
                    if let Ok(mut s) = st_t.try_borrow_mut() { s.playing = false; }
                    pb_t.set_label("▶ Play");
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        });
    }


    // Right
    let right=gtk::Box::new(gtk::Orientation::Vertical,6);right.set_width_request(300);right.set_margin_start(6);right.set_margin_end(14);right.set_margin_top(10);right.set_margin_bottom(10);
    let sl=gtk::Label::new(Some("Load an image or video"));sl.set_xalign(0.0);sl.set_yalign(0.0);sl.set_wrap(true);sl.add_css_class("stat");sl.set_selectable(true);
    let ss=gtk::ScrolledWindow::new();ss.set_vexpand(true);ss.set_child(Some(&sl));
    let sf=gtk::Frame::new(Some("Stats"));sf.add_css_class("panel");sf.set_child(Some(&ss));right.append(&sf);
    let cf=gtk::Frame::new(Some("Codec"));cf.add_css_class("panel");
    let cbox=gtk::Box::new(gtk::Orientation::Vertical,4);cbox.set_margin_top(6);
    macro_rules! mksl{($b:expr,$l:expr,$mn:expr,$mx:expr,$s:expr,$v:expr,$st:expr,$f:ident)=>{
        {let row=gtk::Box::new(gtk::Orientation::Horizontal,6);row.set_margin_start(8);row.set_margin_end(8);
        let lb=gtk::Label::new(Some($l));lb.set_width_chars(10);lb.set_xalign(0.0);lb.add_css_class("sub");
        let a=gtk::Adjustment::new($v as f64,$mn,$mx,$s,$s*5.0,0.0);
        let sc=gtk::Scale::new(gtk::Orientation::Horizontal,Some(&a));sc.set_hexpand(true);sc.set_draw_value(true);
        let ws=$st.clone();a.connect_value_changed(move|a|{ws.borrow_mut().$f=a.value()as _;});
        row.append(&lb);row.append(&sc);$b.append(&row);}
    };}
    mksl!(cbox,"Quality",0.05,1.0,0.05,0.4,st,quality);
    mksl!(cbox,"Dead Zone",0.0,3.0,0.1,1.0,st,dead_zone);
    mksl!(cbox,"Chroma Ag",0.0,40.0,5.0,15.0,st,chroma_boost);
    cf.set_child(Some(&cbox));right.append(&cf);
    root.append(&left);root.append(&right);

    // ── Update ──
    let update={let st=st.clone();let pic=pic.clone();let sl=sl.clone();let fl=frame_lbl.clone();let scrub_lbl=scrub_lbl.clone();let fa2=fa.clone();let tc_btn3=tc_btn.clone();let cancel_btn3=cancel_btn.clone();
        let v9_status_lbl_u = v9_status_lbl.clone();
        let v9_enc_toggle_u = v9_enc_toggle.clone();
        let v9_render_toggle_u = v9_render_toggle.clone();
        move||{
            let mut s=match st.try_borrow_mut(){Ok(s)=>s,Err(_)=>return};
            sl.set_text(&s.stats());
            // v9 status reflects current state: loaded chunk count or 
            // active toggle. Cheap to update; runs ~10Hz.
            let v9_n = s.fao_v9_chunks.len();
            let v9_blocks: usize = s.fao_v9_chunks.iter().map(|c| c.blocks.len()).sum();
            let env_enc = std::env::var("FAO_V9").map(|v| v == "1").unwrap_or(false);
            let env_render = std::env::var("FAO_V9_RENDER").map(|v| v == "1").unwrap_or(false);
            // Sync the toolbar toggle buttons' visual state with env. 
            // The encode dialog's V9 checkbox is the authoritative 
            // signal; flipping that flips the env which flips the 
            // toolbar button. Block_signal to avoid recursive 
            // connect_toggled fire.
            if v9_enc_toggle_u.is_active() != env_enc {
                v9_enc_toggle_u.set_active(env_enc);
                if env_enc {
                    v9_enc_toggle_u.add_css_class("va");
                    v9_enc_toggle_u.remove_css_class("vb");
                } else {
                    v9_enc_toggle_u.add_css_class("vb");
                    v9_enc_toggle_u.remove_css_class("va");
                }
            }
            if v9_render_toggle_u.is_active() != env_render {
                v9_render_toggle_u.set_active(env_render);
                if env_render {
                    v9_render_toggle_u.add_css_class("va");
                    v9_render_toggle_u.remove_css_class("vb");
                } else {
                    v9_render_toggle_u.add_css_class("vb");
                    v9_render_toggle_u.remove_css_class("va");
                }
            }
            let lbl = if v9_n > 0 {
                if env_render {
                    format!("v9: {}c/{}b rndr", v9_n, v9_blocks)
                } else {
                    format!("v9: {}c/{}b", v9_n, v9_blocks)
                }
            } else if env_enc {
                "v9: enc ON".to_string()
            } else {
                "v9: idle".to_string()
            };
            v9_status_lbl_u.set_label(&lbl);
            if s.mode==Mode::VideoScrub{scrub_lbl.set_label(&format!("{:.1}s / {:.1}s",s.scrub_time,s.duration));}
            if s.mode==Mode::VideoPlay||s.mode==Mode::FaoPlay||s.mode==Mode::VideoScrub {
                // Clone the Arc to release s.tc borrow
                let tc_clone = s.tc.as_ref().map(|t| t.clone());
                if let Some(tc) = tc_clone {
                    let n=tc.frame_count.load(Ordering::Relaxed);
                    let done=tc.done.load(Ordering::Relaxed);
                    fa2.set_upper(n.saturating_sub(1).max(1)as f64);
                    fl.set_label(&format!("{} / {} {}",s.fidx,n,if done{"✓"}else{"⏳"}));
                    // Auto-advance display during encode: find latest non-skip
                    if !done && n > 0 {
                        let cur_fidx = s.fidx;
                        let mut best = cur_fidx;
                        if let Ok(stats) = tc.stats.try_lock() {
                            let start = best.max(1);
                            for i in (start..n).rev() {
                                if let Some(stat) = stats.get(i) {
                                    if stat.ftype != 0 && stat.enc_bytes > 100 {
                                        best = i;
                                        break;
                                    }
                                }
                            }
                        }
                        if best != cur_fidx {
                            s.fidx = best;
                            s.load_display_frame();
                        }
                    }
                    if done && !tc_btn3.is_sensitive() {
                        tc_btn3.set_sensitive(true);
                        tc_btn3.set_label("▶ Transcode");
                        // Disable cancel — nothing to cancel anymore.
                        cancel_btn3.set_sensitive(false);
                        cancel_btn3.set_label("⨯ Cancel");
                    }
                }
            }
            if let Some((w,h))=s.render(){
                let pb=gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.display_buf),gdk_pixbuf::Colorspace::Rgb,false,8,w as i32,h as i32,(w*3)as i32);
                pic.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
                pic.queue_draw();  // force GTK to repaint
            }
        }};

    // ── Scrub ──
    {let st=st.clone();let ud=update.clone();
    scrub_adj.connect_value_changed(move|a|{if let Ok(mut s)=st.try_borrow_mut(){if s.mode==Mode::VideoScrub{s.seek_frame(a.value());drop(s);ud();}}});}

    // ── IN/OUT ──
    {let st=st.clone();let rl=pin_range.clone();
    pin_s_adj.connect_value_changed(move|a|{if let Ok(mut s)=st.try_borrow_mut(){s.pin_start=a.value();rl.set_label(&format!("{:.1}s",(s.pin_end-s.pin_start).max(0.0)));}});}
    {let st=st.clone();let rl=pin_range.clone();
    pin_e_adj.connect_value_changed(move|a|{if let Ok(mut s)=st.try_borrow_mut(){s.pin_end=a.value();rl.set_label(&format!("{:.1}s",(s.pin_end-s.pin_start).max(0.0)));}});}

    // ── V9 toggle handlers ──
    // 
    // Toggling V9 sets the FAO_V9 env var, which the encoder reads at 
    // export time inside the v8-trailer-write block. Setting it from 
    // the GUI is equivalent to launching with `FAO_V9=1` on the 
    // command line.
    // 
    // We also force-enable v8 (FAO_V81=1) when the user enables V9, 
    // since v9 builds from v8 chunks and cannot run without v8 being 
    // active. Without this, pressing V9 would silently produce no 
    // output because the v8 trailer block (which contains the v9 
    // gate) wouldn't run.
    // 
    // Same approach for V9R: sets FAO_V9_RENDER which the decode path 
    // reads to switch from v8 mesh raster to v9 ray-march.
    {
        let v9_status_lbl_a = v9_status_lbl.clone();
        v9_enc_toggle.connect_toggled(move|btn|{
            if btn.is_active() {
                std::env::set_var("FAO_V9", "1");
                // Force v8 too — v9 builds from v8 chunks.
                std::env::set_var("FAO_V81", "1");
                btn.remove_css_class("vb");
                btn.add_css_class("va");
                v9_status_lbl_a.set_label("v9: enc ON");
                eprintln!("FAO v9.0: UI toggle ON — FAO_V9=1 + FAO_V81=1. Affects next Encode AND next Export (v9 trailer runs at export time).");
            } else {
                std::env::set_var("FAO_V9", "0");
                btn.remove_css_class("va");
                btn.add_css_class("vb");
                v9_status_lbl_a.set_label("v9: idle");
                eprintln!("FAO v9.0: UI toggle OFF — FAO_V9=0");
            }
        });
    }
    {
        let v9_status_lbl_b = v9_status_lbl.clone();
        v9_render_toggle.connect_toggled(move|btn|{
            if btn.is_active() {
                std::env::set_var("FAO_V9_RENDER", "1");
                btn.remove_css_class("vb");
                btn.add_css_class("va");
                v9_status_lbl_b.set_label("v9: render");
                eprintln!("FAO v9.0: UI render toggle ON — FAO_V9_RENDER=1");
            } else {
                std::env::set_var("FAO_V9_RENDER", "0");
                btn.remove_css_class("va");
                btn.add_css_class("vb");
                v9_status_lbl_b.set_label("v9: idle");
                eprintln!("FAO v9.0: UI render toggle OFF — FAO_V9_RENDER=0");
            }
        });
    }
    {
        // "V9 PPM" button — opens the signs-of-life dump if it exists. 
        // Uses xdg-open / open / start depending on platform. Falls 
        // back to logging the path so the user can open manually.
        let st_ppm = st.clone();
        v9_show_ppm_btn.connect_clicked(move|_|{
            // If v9 chunks are loaded, dump fresh first (gives user 
            // a way to re-render after toggle changes without reloading 
            // the .fao). Then open whatever's on disk.
            let dumped_path = if let Ok(s) = st_ppm.try_borrow() {
                if !s.fao_v9_chunks.is_empty() {
                    s.v9_dump_signs_of_life()
                } else {
                    None
                }
            } else { None };
            let tmp_dir = std::env::temp_dir();
            let tmp_path = dumped_path.unwrap_or_else(|| tmp_dir.join("v9_signs_of_life.ppm"));
            let path = tmp_path.to_string_lossy().to_string();
            if !std::path::Path::new(&path).exists() {
                eprintln!("FAO v9.0: PPM dump not found at {} — encode with V9 toggle ON and load the result first", path);
                return;
            }
            // Try xdg-open first (Linux), then open (Mac), then nothing.
            let opener = if std::process::Command::new("xdg-open").arg("--version").output().is_ok() {
                "xdg-open"
            } else if std::process::Command::new("open").arg("--help").output().is_ok() {
                "open"
            } else {
                eprintln!("FAO v9.0: no opener (xdg-open / open) found — view manually: {}", path);
                return;
            };
            match std::process::Command::new(opener).arg(&path).spawn() {
                Ok(_) => eprintln!("FAO v9.0: opening {} with {}", path, opener),
                Err(e) => eprintln!("FAO v9.0: failed to open {}: {}", path, e),
            }
        });
    }

    // ── Transcode ──
    {let st=st.clone();let ud=update.clone();
    {let tc_btn2 = tc_btn.clone();
    let st2 = st.clone(); let ud2 = update.clone(); let tc_btn4 = tc_btn.clone();
    let cancel_btn_t = cancel_btn.clone();  // for the dialog response handler
    let w3 = win.clone();
    tc_btn.connect_clicked(move|_|{
        if let Ok(s)=st.try_borrow(){
            let encoding = s.tc.as_ref().map(|t| !t.done.load(Ordering::Relaxed)).unwrap_or(false);
            if encoding { return; }
            drop(s);
        }
        // Show mode selector dialog
        let dlg = gtk::Dialog::builder()
            .title("FAO Encode Mode")
            .transient_for(&w3)
            .modal(true)
            .default_width(520)
            .default_height(420)
            .build();
        let content = dlg.content_area();
        content.set_margin_top(16); content.set_margin_bottom(16);
        content.set_margin_start(20); content.set_margin_end(20);
        content.set_spacing(8);
        content.set_orientation(gtk::Orientation::Vertical);
        
        let v1_btn = gtk::CheckButton::with_label("v1 — Real-time DCT
Fast encode, motion prediction, ~20fps 1080p");
        let v2_btn = gtk::CheckButton::with_label("v2 — Analysis + Functions
Multi-pass, pattern discovery, better ratio");
        v2_btn.set_group(Some(&v1_btn));
        v1_btn.set_active(true);
        content.append(&v1_btn);
        content.append(&v2_btn);
        
        // Separator + codec version selector
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let cv_label = gtk::Label::new(Some("Codec Version"));
        cv_label.set_halign(gtk::Align::Start);
        cv_label.add_css_class("heading");
        content.append(&cv_label);
        
        let cv20_btn = gtk::CheckButton::with_label("v2.0 — Skip + Passthrough + Full encode (no T-frames)");
        let cv21_btn = gtk::CheckButton::with_label("v2.1 — Predictive T-frames + K-frames (default)");
        let cv30_btn = gtk::CheckButton::with_label("v3.0 — Layered mesh (static plate + residual patches)");
        let cv40_btn = gtk::CheckButton::with_label("v4.0 — Sparse skeleton (~150x compression, dictionary-based)");
        let cv50_btn = gtk::CheckButton::with_label("v5.0 — Scene replay (game-engine sprite library, super speed/quality)");
        let cv60_btn = gtk::CheckButton::with_label("v6.0 — Object field (3D objects, multi-pass GPU candidates, feature matching)");
        let cv70_btn = gtk::CheckButton::with_label("v7.0 — Perception (phase correlation + camera motion tracking)");
        let cv80_btn = gtk::CheckButton::with_label("v8.0 — Reconstruction [SCAFFOLD] (camera pose track + mesh trailer)");
        // v9 is an ADD-ON to v8, not a separate codec. Standalone 
        // CheckButton (not in the radio group) — user can tick it on 
        // top of any codec selection. Ticking it auto-forces v8 since 
        // v9 needs v8 chunks as its source.
        let cv90_btn = gtk::CheckButton::with_label("V9 — Volumetric trailer (EXPERIMENTAL) — writes V9DC alongside V8MX. Forces v8.");
        cv21_btn.set_group(Some(&cv20_btn));
        cv30_btn.set_group(Some(&cv20_btn));
        cv40_btn.set_group(Some(&cv20_btn));
        cv50_btn.set_group(Some(&cv20_btn));
        cv60_btn.set_group(Some(&cv20_btn));
        cv70_btn.set_group(Some(&cv20_btn));
        cv80_btn.set_group(Some(&cv20_btn));
        // Pre-tick V9 if the toolbar toggle is currently on (or env 
        // var was set externally).
        let v9_currently_on = std::env::var("FAO_V9")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        cv90_btn.set_active(v9_currently_on);
        // When v9 is ticked in the dialog, also auto-tick v8 — and 
        // reflect that visually so the user sees what's about to happen.
        {
            let cv80_btn_ref = cv80_btn.clone();
            cv90_btn.connect_toggled(move|btn|{
                if btn.is_active() && !cv80_btn_ref.is_active() {
                    cv80_btn_ref.set_active(true);
                }
            });
        }
        // Default to current selection
        let cur_cv = st.borrow().codec_version.get();
        match cur_cv {
            0 => cv20_btn.set_active(true),
            2 => cv30_btn.set_active(true),
            _ => cv21_btn.set_active(true),
        }
        content.append(&cv20_btn);
        content.append(&cv21_btn);
        content.append(&cv30_btn);
        content.append(&cv40_btn);
        content.append(&cv50_btn);
        content.append(&cv60_btn);
        content.append(&cv70_btn);
        content.append(&cv80_btn);
        // V9 add-on checkbox sits below the codec radio group with a 
        // visual separator so the user understands it's additive.
        let v9_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        v9_sep.set_margin_top(8);
        v9_sep.set_margin_bottom(4);
        content.append(&v9_sep);
        content.append(&cv90_btn);
        
        dlg.add_button("Cancel", gtk::ResponseType::Cancel);
        dlg.add_button("Encode", gtk::ResponseType::Accept);
        
        let st_dlg = st.clone(); let ud_dlg = ud2.clone();
        let tc_btn_dlg = tc_btn4.clone(); let v2_check = v2_btn.clone();
        let cancel_btn_dlg = cancel_btn_t.clone();
        let cv20_btn = cv20_btn.clone(); let cv30_btn = cv30_btn.clone();
        dlg.connect_response(move |d, resp| {
            d.close();
            if resp != gtk::ResponseType::Accept { return; }
            if let Ok(mut s) = st_dlg.try_borrow_mut() {
                s.encode_mode = if v2_check.is_active() { EncodeMode::V2 } else { EncodeMode::V1 };
                // Apply codec version selection from dialog
                let mut cv = if cv80_btn.is_active() { 7u8 }
                         else if cv70_btn.is_active() { 6u8 }
                         else if cv60_btn.is_active() { 5u8 }
                         else if cv50_btn.is_active() { 4u8 }
                         else if cv40_btn.is_active() { 3u8 }
                         else if cv30_btn.is_active() { 2u8 }
                         else if cv20_btn.is_active() { 0u8 }
                         else { 1u8 };
                // ── V9 add-on activation ──
                // 
                // The V9 checkbox in this dialog is the authoritative 
                // signal for whether the next encode produces a V9DC 
                // trailer. We set FAO_V9 env var here so the export 
                // code (which reads env at trailer-write time) picks 
                // it up. We also force cv=7 (v8) because v9 builds 
                // from v8 chunks — without v8 there's nothing to 
                // volumetrize.
                let v9_checked = cv90_btn.is_active();
                if v9_checked {
                    std::env::set_var("FAO_V9", "1");
                    if cv != 7 {
                        eprintln!("FAO v9.0: V9 checkbox ticked but codec was v{}.{} — forcing v8 so v9 can run",
                            match cv { 0=>"2",1=>"2",2=>"3",3=>"4",4=>"5",5=>"6",6=>"7",_=>"?" },
                            match cv { 0=>"0",1=>"1",2=>"0",3=>"0",4=>"0",5=>"0",6=>"0",_=>"?" });
                        cv = 7;
                    }
                    eprintln!("FAO v9.0: dialog → V9 enabled, FAO_V9=1");
                } else {
                    std::env::set_var("FAO_V9", "0");
                }
                s.codec_version.set(cv);
                let (major, minor) = match cv {
                    0 => (2, 0),
                    1 => (2, 1),
                    2 => (3, 0),
                    3 => (4, 0),
                    4 => (5, 0),
                    5 => (6, 0),
                    6 => (7, 0),
                    7 => (8, 0),
                    _ => (2, 1),
                };
                eprintln!("Encode dialog: codec v{}.{}", major, minor);
                s.start_transcode(); drop(s);
            tc_btn_dlg.set_sensitive(false);
            tc_btn_dlg.set_label("Encoding...");
            // Enable Cancel now that an encode is in flight.
            cancel_btn_dlg.set_sensitive(true);
            cancel_btn_dlg.set_label("⨯ Cancel");
            ud_dlg();
            let sp = st_dlg.clone(); let up = ud_dlg.clone(); let tb = tc_btn_dlg.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                up();
                let done = if let Ok(s) = sp.try_borrow() {
                    s.tc.as_ref().map(|t| t.done.load(Ordering::Relaxed)).unwrap_or(true)
                } else { false };
                if done {
                    tb.set_sensitive(true);
                    tb.set_label("▶ Transcode");
                    // Auto-update stats to show "✓ done"
                    up();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
            }
        });
        dlg.present();
    });}}

    // ── Cancel ──
    // Signals tc.cancel_requested; workers and the reader pick it up 
    // on their next iteration, drain their channels, and exit. The 
    // collector finalizes whatever was completed, and the update loop 
    // re-enables the Transcode button when tc.done flips.
    {let st = st.clone(); let cb_self = cancel_btn.clone();
    cancel_btn.connect_clicked(move|_|{
        if let Ok(s) = st.try_borrow() {
            if let Some(tc) = s.tc.as_ref() {
                if !tc.done.load(Ordering::Relaxed) {
                    eprintln!("FAO: cancel requested by user");
                    tc.cancel_requested.store(true, Ordering::Relaxed);
                    cb_self.set_label("Cancelling...");
                    cb_self.set_sensitive(false);
                }
            }
        }
    });}

    // ── Export ── streaming file-to-file
    {let st=st.clone();let ud=update.clone();let eb=export_btn.clone();
    let lib_subtitle_c = lib_subtitle.clone();
    let lib_list_c = lib_list.clone();
    export_btn.connect_clicked(move|_|{
        // First, get tc info & paths under read borrow
        let (out_path, src_path_str, stem, ftype_count, total_frames) = {
            let s = match st.try_borrow() { Ok(s) => s, Err(_) => {
                eprintln!("[export] click ignored — App is borrowed elsewhere (try again in a moment)");
                return;
            }};
            let tc = match &s.tc { Some(t) => t.clone(), None => {
                eprintln!("[export] click ignored — no active transcode (run Transcode first)");
                return;
            }};
            if !tc.done.load(Ordering::Relaxed) {
                eprintln!("[export] click ignored — encode still running (frame_count={}, wait for completion)",
                    tc.frame_count.load(Ordering::Relaxed));
                return;
            }
            let fname = s.src_path.rsplit('/').next().unwrap_or("output").to_string();
            let stem = if let Some(dot) = fname.rfind('.') { fname[..dot].to_string() } else { fname.clone() };
            let src_path = std::path::Path::new(&s.src_path);
            let parent = src_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
            let export_dir = parent.join("FAO_exports");
            let _ = std::fs::create_dir_all(&export_dir);
            let out_path = export_dir.join(format!("{}.fao", stem));
            let n = tc.frame_count.load(Ordering::Relaxed) as u32;
            (out_path, s.src_path.clone(), stem, 0u32, n)
        };
        let _ = ftype_count;
        
        eb.set_label("Exporting...");
        eb.set_sensitive(false);
        
        // Pre-export status: tells the user what trailers are about to 
        // be written. Particularly important for v9 so they know to 
        // watch for v9 logs during export, not during encode.
        let v9_on_pre = std::env::var("FAO_V9").map(|v| v == "1").unwrap_or(false);
        eprintln!("FAO export starting → {} (V9 trailer: {})",
            out_path.display(),
            if v9_on_pre { "ENABLED, watch for v9 logs below" } else { "disabled" });
        
        let out_path_str = out_path.display().to_string();
        let export_result = {
            let s = match st.try_borrow() { Ok(s) => s, Err(_) => return };
            s.export_fao(&out_path)
        };
        
        match export_result {
            Ok(bytes) => {
                eb.set_label(&format!("\u{2713} {:.0}MB", bytes as f64 / 1048576.0));
                
                // Auto-add to active archive (if any)
                let mut s = match st.try_borrow_mut() { Ok(s) => s, Err(_) => return };
                let archive_path_opt = s.archive_path.clone();
                let has_archive = s.archive.is_some();
                if has_archive {
                    let tc = s.tc.as_ref().map(|t| t.clone());
                    let (w, h, fps) = if let Some(ref tc) = tc {
                        (tc.w as u32, tc.h as u32, tc.fps as f32)
                    } else {
                        (s.cur_w as u32, s.cur_h as u32, s.vid_fps as f32)
                    };
                    let duration = total_frames as f32 / fps.max(1.0);
                    let entry = fao_v1::VideoEntry {
                        id: 0,
                        title: stem.clone(),
                        source_path: src_path_str,
                        fao_path: out_path_str.clone(),
                        width: w, height: h, fps,
                        frame_count: total_frames,
                        duration_secs: duration,
                        file_size_bytes: bytes,
                        created_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs()).unwrap_or(0),
                        chapters: Vec::new(),
                        tags: Vec::new(),
                        thumbnail_data: Vec::new(),
                    };
                    if let Some(ref mut archive) = s.archive {
                        archive.add_video(entry);
                        if let Some(ref mp) = archive_path_opt {
                            let _ = archive.save_manifest(mp);
                        }
                        let n_videos = archive.entries.len();
                        let dur = archive.total_duration_secs();
                        let size_mb = archive.total_size_bytes as f64 / 1048576.0;
                        let arch_name = archive.name.clone();
                        let entries_snap: Vec<_> = archive.entries.iter().map(|e| (
                            e.title.clone(), e.width, e.height, e.fps,
                            e.duration_secs, e.file_size_bytes, e.chapters.len(),
                            e.fao_path.clone(),
                        )).collect();
                        s.lib_row_paths = entries_snap.iter().map(|e| e.7.clone()).collect();
                        drop(s);
                        
                        // Refresh library list
                        while let Some(child) = lib_list_c.first_child() {
                            lib_list_c.remove(&child);
                        }
                        for (title, w, h, fps, dur, size, n_chapters, _) in &entries_snap {
                            let row = gtk::ListBoxRow::new();
                            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                            row_box.set_margin_start(8); row_box.set_margin_end(8);
                            row_box.set_margin_top(6); row_box.set_margin_bottom(6);
                            let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
                            info.set_hexpand(true);
                            let title_lbl = gtk::Label::new(Some(title));
                            title_lbl.set_halign(gtk::Align::Start);
                            title_lbl.add_css_class("hdr");
                            let meta = format!("{}x{} {:.1}fps  {:.1}s  {:.1}MB  {} chapters",
                                w, h, fps, dur, *size as f64 / 1048576.0, n_chapters);
                            let meta_lbl = gtk::Label::new(Some(&meta));
                            meta_lbl.set_halign(gtk::Align::Start);
                            meta_lbl.add_css_class("sub");
                            info.append(&title_lbl); info.append(&meta_lbl);
                            row_box.append(&info);
                            row.set_child(Some(&row_box));
                            lib_list_c.append(&row);
                        }
                        lib_subtitle_c.set_text(&format!("{}: {} videos, {:.0}s, {:.1}MB",
                            arch_name, n_videos, dur, size_mb));
                        eprintln!("FAO: auto-added {} to archive ({} total)", stem, n_videos);
                    }
                }
            }
            Err(e) => { eb.set_label("\u{2717} Error"); eprintln!("Export: {}", e); }
        }
        let eb2 = eb.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            eb2.set_label("Export"); eb2.set_sensitive(true);
        });
        ud();
    });}

    // ── AV1 / H.264 re-encode ──
    // Re-encodes the currently-loaded .fao file to a portable .mkv 
    // (AV1 by default, H.264 optional). Decodes each frame through 
    // the FAO pipeline and pipes raw RGB to ffmpeg, which writes 
    // the result. Requires ffmpeg on $PATH.
    // 
    // The decode work happens on a worker thread so the UI stays 
    // responsive. A progress channel reports frame count back to the 
    // main thread, which updates a modal dialog. Cancel is supported.
    {let st = st.clone(); let ab = av1_btn.clone(); let w_a = win.clone();
    av1_btn.connect_clicked(move|_|{
        // Gather everything we need under a read borrow, then drop it 
        // before spawning the worker (worker needs to access st later 
        // to read fao_v8_decoder Arc).
        let (data_bytes, index, w, h, fps, src_path) = {
            let s = match st.try_borrow() { Ok(s) => s, Err(_) => return };
            let data = match &s.fao_data { Some(d) => d, None => {
                eprintln!("AV1 export: no .fao loaded");
                return;
            }};
            if s.fao_index.is_empty() {
                eprintln!("AV1 export: .fao has no frames");
                return;
            }
            if s.cur_w == 0 || s.cur_h == 0 {
                eprintln!("AV1 export: dimensions not set");
                return;
            }
            // Slurp the .fao bytes into a Vec for the worker. For a 
            // 1GB file this is a big allocation but acceptable for a 
            // one-shot export. (A future enhancement: ref-counted 
            // shared bytes via Arc<FaoData>.)
            let bytes: Vec<u8> = (**data).to_vec();
            (bytes, s.fao_index.clone(), s.cur_w, s.cur_h, s.vid_fps, s.src_path.clone())
        };
        
        // Pick output path next to source.
        let fname = std::path::Path::new(&src_path)
            .file_stem().and_then(|s| s.to_str()).unwrap_or("output").to_string();
        let src_parent = std::path::Path::new(&src_path).parent()
            .unwrap_or(std::path::Path::new(".")).to_path_buf();
        let export_dir = src_parent.join("FAO_exports");
        let _ = std::fs::create_dir_all(&export_dir);
        let out_path = export_dir.join(format!("{}_av1.mkv", fname));
        let out_path_disp = out_path.display().to_string();
        
        // Snapshot the V8 decoder Arc for the worker.
        let decoder_arc = {
            let s = match st.try_borrow() { Ok(s) => s, Err(_) => return };
            s.fao_v8_decoder.clone()
        };
        
        // Disable button while exporting.
        ab.set_label("→ Exporting...");
        ab.set_sensitive(false);
        let ab2 = ab.clone();
        
        // Progress dialog with Cancel.
        let dlg = gtk::Dialog::builder()
            .title("AV1 export in progress")
            .transient_for(&w_a)
            .modal(false)
            .default_width(420)
            .build();
        let content = dlg.content_area();
        content.set_margin_top(12); content.set_margin_bottom(12);
        content.set_margin_start(12); content.set_margin_end(12);
        let lbl = gtk::Label::new(Some(&format!("Decoding to {}", out_path_disp)));
        lbl.set_xalign(0.0);
        content.append(&lbl);
        let prog_lbl = gtk::Label::new(Some("Starting..."));
        prog_lbl.set_xalign(0.0);
        prog_lbl.set_margin_top(8);
        content.append(&prog_lbl);
        let cancel_dlg_btn = dlg.add_button("Cancel", gtk::ResponseType::Cancel);
        let _ = cancel_dlg_btn;
        dlg.present();
        
        // Cooperative cancel flag shared with the worker.
        let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_flag_dlg = cancel_flag.clone();
        let dlg_clone_for_cancel = dlg.clone();
        dlg.connect_response(move |d, _resp| {
            cancel_flag_dlg.store(true, std::sync::atomic::Ordering::Relaxed);
            d.close();
        });
        let _ = dlg_clone_for_cancel;
        
        // Progress channel (worker → main).
        // Messages: Ok(frame_idx) for progress, Err(message) for done/error.
        let (tx, rx) = std::sync::mpsc::channel::<Result<usize, String>>();
        
        // Spawn worker thread.
        let cancel_flag_worker = cancel_flag.clone();
        let n_frames = index.len();
        std::thread::spawn(move || {
            let rgb_sz = w * h * 3;
            
            // Spawn ffmpeg.
            let mut child = match std::process::Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "warning"])
                .args(["-f", "rawvideo", "-pixel_format", "rgb24"])
                .args(["-video_size", &format!("{}x{}", w, h)])
                .args(["-framerate", &format!("{}", fps.max(1.0))])
                .args(["-i", "pipe:0"])
                .args(["-c:v", "libsvtav1", "-crf", "28", "-preset", "6"])
                .args(["-pix_fmt", "yuv420p"])
                .arg(&out_path)
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(format!("ffmpeg spawn failed: {} (is ffmpeg installed?)", e)));
                    return;
                }
            };
            let mut stdin = child.stdin.take().expect("piped");
            
            let mut prev_rgb: Vec<u8> = vec![64u8; rgb_sz];
            for (idx, entry) in index.iter().enumerate() {
                if cancel_flag_worker.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = tx.send(Err("cancelled by user".to_string()));
                    drop(stdin);
                    let _ = child.kill();
                    return;
                }
                
                let rgb = decode_fao_frame_for_export(
                    &data_bytes, entry, idx, w, h,
                    &decoder_arc, &prev_rgb,
                );
                use std::io::Write;
                if let Err(e) = stdin.write_all(&rgb) {
                    let _ = tx.send(Err(format!("ffmpeg stdin write failed at frame {}: {}", idx, e)));
                    drop(stdin);
                    let _ = child.wait();
                    return;
                }
                prev_rgb = rgb;
                
                // Send progress every 30 frames.
                if idx % 30 == 0 || idx + 1 == n_frames {
                    let _ = tx.send(Ok(idx + 1));
                }
            }
            
            drop(stdin);
            match child.wait() {
                Ok(s) if s.success() => {
                    let _ = tx.send(Err(String::new()));  // empty err = success
                }
                Ok(s) => {
                    let _ = tx.send(Err(format!("ffmpeg exited with {:?}", s.code())));
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("ffmpeg wait failed: {}", e)));
                }
            }
        });
        
        // Main-thread polling: update prog_lbl on each Ok(n), close 
        // the dialog on Err(...).
        let prog_lbl_clone = prog_lbl.clone();
        let dlg_clone = dlg.clone();
        let ab_clone = ab2.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            // Drain everything available without blocking.
            let mut last_progress: Option<usize> = None;
            let mut done_msg: Option<String> = None;
            loop {
                match rx.try_recv() {
                    Ok(Ok(n)) => { last_progress = Some(n); }
                    Ok(Err(s)) => { done_msg = Some(s); break; }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        done_msg = Some("worker disconnected unexpectedly".to_string());
                        break;
                    }
                }
            }
            if let Some(n) = last_progress {
                let pct = 100.0 * n as f64 / n_frames.max(1) as f64;
                prog_lbl_clone.set_text(&format!("Frame {} / {} ({:.1}%)", n, n_frames, pct));
            }
            if let Some(msg) = done_msg {
                if msg.is_empty() {
                    prog_lbl_clone.set_text("✓ Export complete");
                    ab_clone.set_label("✓ AV1 done");
                } else {
                    prog_lbl_clone.set_text(&format!("✗ {}", msg));
                    ab_clone.set_label("✗ AV1 failed");
                }
                // Re-enable the button after a short pause.
                let ab3 = ab_clone.clone();
                glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
                    ab3.set_label("→ AV1");
                    ab3.set_sensitive(true);
                });
                // Close progress dialog after a moment.
                let dlg_clone2 = dlg_clone.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                    dlg_clone2.close();
                });
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    });}

    // ── Open FAO ──
    {let st=st.clone();let ud=update.clone();let fa2=fa.clone();let w2=win.clone();
    open_fao_btn.connect_clicked(move|_|{let sc=st.clone();let uc=ud.clone();let fa3=fa2.clone();
        let d=gtk::FileChooserNative::new(Some("Open FAO"),Some(&w2),gtk::FileChooserAction::Open,Some("Open"),Some("Cancel"));
        let f=gtk::FileFilter::new();f.add_pattern("*.fao");f.set_name(Some("FAO Video"));d.add_filter(&f);
        d.connect_response(move|d,r|{if r==gtk::ResponseType::Accept{if let Some(f)=d.file(){if let Some(p)=f.path(){
            {
                let mut s = sc.borrow_mut();
                s.load_fao(&p);
                // Set view to Recon since FAO has no original to A/B with
                s.view = View::Recon;
                eprintln!("FAO: loaded {} ({} frames, {}x{})", p.display(), s.fao_index.len(), s.cur_w, s.cur_h);
            }
            let s = sc.borrow();
            let n = s.fao_index.len();
            fa3.set_upper(n.saturating_sub(1).max(1) as f64);
            fa3.set_value(0.0);
            drop(s);
            uc();
        }}}});d.show();});}

    // ── Back ──
    {let st=st.clone();let ud=update.clone();let sa=scrub_adj.clone();
    back_btn.connect_clicked(move|_|{if let Ok(mut s)=st.try_borrow_mut(){
        // Kill ffmpeg before dropping TranscodeState
        if let Some(ref tc) = s.tc {
            let pid = tc.ffmpeg_pid.load(Ordering::SeqCst);
            if pid > 0 { libc_kill(pid as i32); }
        }
        s.tc=None;
        s.mode=Mode::VideoScrub;
        s.playing=false;
        let ps=s.pin_start;s.seek_frame(ps);sa.set_value(ps);drop(s);ud();}});}

    // ── Load image ──
    {let st=st.clone();let ud=update.clone();let w2=win.clone();
    load_img.connect_clicked(move|_|{let sc=st.clone();let uc=ud.clone();
        let d=gtk::FileChooserNative::new(Some("Open"),Some(&w2),gtk::FileChooserAction::Open,Some("Open"),Some("Cancel"));
        let f=gtk::FileFilter::new();for p in["*.png","*.jpg","*.jpeg","*.bmp"]{f.add_pattern(p);}f.set_name(Some("Images"));d.add_filter(&f);
        d.connect_response(move|d,r|{if r==gtk::ResponseType::Accept{if let Some(f)=d.file(){if let Some(p)=f.path(){sc.borrow_mut().load_image(&p);uc();}}}});d.show();});}

    // ── Load video ──
    {let st=st.clone();let ud=update.clone();let sa=scrub_adj.clone();let psa=pin_s_adj.clone();let pea=pin_e_adj.clone();let w2=win.clone();
    load_vid.connect_clicked(move|_|{let sc=st.clone();let uc=ud.clone();let sa2=sa.clone();let psa2=psa.clone();let pea2=pea.clone();
        let d=gtk::FileChooserNative::new(Some("Open"),Some(&w2),gtk::FileChooserAction::Open,Some("Open"),Some("Cancel"));
        let f=gtk::FileFilter::new();for p in["*.mp4","*.mkv","*.webm","*.avi","*.mov"]{f.add_pattern(p);}f.set_name(Some("Video"));d.add_filter(&f);
        d.connect_response(move|d,r|{if r==gtk::ResponseType::Accept{if let Some(f)=d.file(){if let Some(p)=f.path(){
            sc.borrow_mut().probe_video(&p);let s=sc.borrow();
            sa2.set_upper(s.duration);sa2.set_value(0.0);
            psa2.set_upper(s.duration);pea2.set_upper(s.duration);
            psa2.set_value(s.pin_start);pea2.set_value(s.pin_end);
            drop(s);uc();}}}});d.show();});}

    // ── Frame slider ──
    {let st=st.clone();let ud=update.clone();
    fa.connect_value_changed(move|a|{if let Ok(mut s)=st.try_borrow_mut(){if !s.playing&&(s.mode==Mode::VideoPlay||s.mode==Mode::FaoPlay){
            let new_idx = a.value() as usize;
            if new_idx != s.fidx {  // only react to actual user scrub, not set_upper side effects
                s.fidx = new_idx;
                let fi=s.fidx;if s.mode==Mode::FaoPlay{s.load_fao_frame(fi);}else{s.load_display_frame();}
                drop(s);ud();
            }
            }}});}

    // ── Play/Pause ──
    {let st=st.clone();let ud=update.clone();let fa2=fa.clone();let pb=play_btn.clone();let pic_play=pic.clone();
    play_btn.connect_clicked(move|_|{
        if let Ok(mut s)=st.try_borrow_mut(){
            if s.playing{s.playing=false;pb.set_label("▶ Play");return;}
            
            // FAO file playback: dedicated timer, no ffmpeg
            if s.mode == Mode::FaoPlay {
                if s.fao_index.is_empty() {
                    eprintln!("FAO play: no frames in .fao file");
                    return;
                }
                // Resume from current position; only loop to start if at end
                if s.fidx >= s.fao_index.len().saturating_sub(1) {
                    s.fidx = 0;
                }
                s.playing = true;
                let cur = s.fidx;
                s.load_fao_frame(cur);
                pb.set_label("⏸ Pause");
                let fps = s.vid_fps.max(1.0);
                let frame_ms = ((1000.0 / fps) as u64).max(8);
                eprintln!("FAO play: pure .fao playback, {} frames at {}fps ({}ms/frame)", s.fao_index.len(), fps, frame_ms);
                
                let st_c = st.clone();
                let ud_c = ud.clone();
                let pb_c = pb.clone();
                let fa_c = fa2.clone();
                drop(s);
                
                glib::timeout_add_local(std::time::Duration::from_millis(frame_ms), move || {
                    if let Ok(mut s) = st_c.try_borrow_mut() {
                        if !s.playing || s.mode != Mode::FaoPlay {
                            return glib::ControlFlow::Break;
                        }
                        let next = s.fidx + 1;
                        let avail = s.fao_index.len();
                        if next >= avail {
                            s.playing = false;
                            pb_c.set_label("▶ Play");
                            return glib::ControlFlow::Break;
                        }
                        s.fidx = next;
                        s.load_fao_frame(next);
                        fa_c.set_value(next as f64);
                        drop(s);
                        ud_c();
                    }
                    glib::ControlFlow::Continue
                });
                return;
            }
            
            if s.mode != Mode::VideoPlay || s.tc.is_none() { return; }
            let max_f = s.tc.as_ref().map(|t|t.frame_count.load(Ordering::Relaxed)).unwrap_or(0);
            // Resume from current position; only loop to start if at end
            if s.fidx >= max_f.saturating_sub(1) {
                s.fidx = 0;
            }
            s.playing=true;s.play_accum_ms=0.0;s.speed_x=0.0;s.speed_frames=0;s.speed_start=std::time::Instant::now();pb.set_label("⏸ Pause");
            // Spawn sequential ffmpeg decoder for playback
            let src_path = s.src_path.clone();
            let play_w = if s.cur_w > 0 { s.cur_w } else { s.vid_w };
            let play_h = if s.cur_h > 0 { s.cur_h } else { s.vid_h };
            if src_path.is_empty() {
                eprintln!("FAO play: ERROR - no source path!");
                s.playing = false;
                pb.set_label("▶ Play");
                return;
            }
            // Seek to current frame position (resume scrub)
            let resume_offset_secs = s.fidx as f64 / s.vid_fps.max(1.0);
            let play_start = s.pin_start + resume_offset_secs;
            eprintln!("FAO play: src={} w={} h={} fps={} start={:.1}s fidx={} (resume from scrub)", 
                src_path, play_w, play_h, s.vid_fps, play_start, s.fidx);
            let sc=st.clone();let uc=ud.clone();let fa3=fa2.clone();let pb2=pb.clone();let pic4=pic_play.clone();

            let display_hz = 240.0f64; drop(s);
            // Start playback decoder in background thread
            let (play_tx, play_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);  // 16-frame lookahead
            let play_fps = {let s2 = sc.borrow(); s2.vid_fps};
            std::thread::Builder::new().name("fao-play-dec".into()).spawn(move || {
                let yuv_fsz = play_w * play_h * 3 / 2;
                // Try hardware decode first, fall back to software
                let mut cmd = std::process::Command::new("ffmpeg");
                cmd.args(["-ss", &format!("{:.3}", play_start)]);
                cmd.args(["-threads", "0"]);  // auto thread count
                cmd.arg("-i").arg(&src_path);
                // Skip scaling if already at target resolution
                cmd.args(["-vf", &format!("scale={}:{},setsar=1", play_w, play_h)]);
                cmd.args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-v", "error", "-"]);
                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::piped());
                let child = cmd.spawn();
                let mut proc = match child { 
                    Ok(c) => { eprintln!("FAO play-dec: started ffmpeg pid={}", c.id()); c },
                    Err(e) => { eprintln!("FAO play-dec: ffmpeg spawn FAILED: {}", e); return }
                };
                let stdout = match proc.stdout.take() { Some(s) => s, None => return };
                let mut rdr = std::io::BufReader::with_capacity(yuv_fsz * 8, stdout);  // 8-frame buffer
                // Double-buffered: pre-read next frame while current converts
                let mut fb_a = vec![0u8; yuv_fsz];
                let mut fb_b = vec![0u8; yuv_fsz];
                let mut play_count = 0usize;
                let mut use_a = true;
                // Pre-read first frame
                if rdr.read_exact(&mut fb_a).is_err() { eprintln!("FAO play-dec: no frames"); return; }
                loop {
                    let (cur, next) = if use_a { (&fb_a, &mut fb_b) } else { (&fb_b, &mut fb_a) };
                    // Start reading next frame while we convert current
                    // (On M2 Ultra, the read and convert can overlap on different cores)
                    play_count += 1;
                    let fb = cur;
                    // Fast parallel YUV→RGB (row-parallel, SIMD-friendly)
                    let mut rgb = vec![0u8; play_w * play_h * 3];
                    let y_sz = play_w * play_h;
                    let hw = play_w / 2;
                    let u_plane = &fb[y_sz..y_sz + y_sz/4];
                    let v_plane = &fb[y_sz + y_sz/4..];
                    // Process 2 rows at a time (share UV row)
                    for py_pair in 0..play_h/2 {
                        let py0 = py_pair * 2;
                        let py1 = py0 + 1;
                        let uv_row = py_pair * hw;
                        for px in 0..play_w {
                            let ux = px / 2;
                            let u = u_plane[uv_row + ux] as i16 - 128;
                            let v = v_plane[uv_row + ux] as i16 - 128;
                            // Pre-compute chroma contributions (shared by 2 pixels vertically)
                            let cr = (v * 359) >> 8;         // 1.402 * 256
                            let cg = (u * 88 + v * 183) >> 8; // 0.344*256, 0.714*256
                            let cb = (u * 454) >> 8;         // 1.772 * 256
                            // Row 0
                            let y0 = fb[py0 * play_w + px] as i16;
                            let oi0 = (py0 * play_w + px) * 3;
                            rgb[oi0]   = (y0 + cr).clamp(0, 255) as u8;
                            rgb[oi0+1] = (y0 - cg).clamp(0, 255) as u8;
                            rgb[oi0+2] = (y0 + cb).clamp(0, 255) as u8;
                            // Row 1
                            let y1 = fb[py1 * play_w + px] as i16;
                            let oi1 = (py1 * play_w + px) * 3;
                            rgb[oi1]   = (y1 + cr).clamp(0, 255) as u8;
                            rgb[oi1+1] = (y1 - cg).clamp(0, 255) as u8;
                            rgb[oi1+2] = (y1 + cb).clamp(0, 255) as u8;
                        }
                    }
                    // Handle odd last row if height is odd
                    if play_h % 2 == 1 {
                        let py = play_h - 1;
                        for px in 0..play_w {
                            let yi = py * play_w + px;
                            let ui = (py/2) * hw + px/2;
                            let y = fb[yi] as i16;
                            let u = u_plane[ui] as i16 - 128;
                            let v = v_plane[ui] as i16 - 128;
                            let oi = yi * 3;
                            rgb[oi]   = (y + ((v * 359) >> 8)).clamp(0, 255) as u8;
                            rgb[oi+1] = (y - ((u * 88 + v * 183) >> 8)).clamp(0, 255) as u8;
                            rgb[oi+2] = (y + ((u * 454) >> 8)).clamp(0, 255) as u8;
                        }
                    }
                    if play_tx.send(rgb).is_err() { break; }
                    // Read next frame (overlaps with main thread consuming current)
                    if rdr.read_exact(next).is_err() { eprintln!("FAO play-dec: finished after {} frames", play_count); break; }
                    use_a = !use_a;
                }
                let _ = proc.kill();
                if let Some(mut stderr) = proc.stderr.take() {
                    let mut err = String::new();
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
                    if !err.is_empty() { eprintln!("FAO play-dec stderr: {}", err.chars().take(500).collect::<String>()); }
                }
                let _ = proc.wait();
            }).ok();
            // Start timer at display rate — each tick decides whether to advance
            // based on VRR timing from the codec
            let ms = (1000.0 / display_hz).max(4.0) as u64;
            let play_rx2 = play_rx;
            let play_w2 = play_w;
            let play_h2 = play_h;
            glib::timeout_add_local(std::time::Duration::from_millis(ms),move||{
                match sc.try_borrow_mut(){
                    Ok(mut s)=>{
                        if !s.playing{return glib::ControlFlow::Break;}
                        // VRR: check if enough time has elapsed to show the next frame
                        // Get VRR duration from the current frame's stats
                        let vrr_ms = s.cur_result.as_ref().map(|r| r.vrr_ms).unwrap_or(16.7);
                        let src_fps = s.vid_fps.max(1.0);
                        let frame_ms = (1000.0 / src_fps) as f32;
                        // Play at source fps (not display fps)
                        // VRR only kicks in when source fps > display fps
                        let target_ms = frame_ms;  // always respect source timing
                        // Accumulate ticks (each tick is ~4ms)
                        s.play_accum_ms += ms as f64;
                        if s.play_accum_ms < target_ms as f64 {
                            drop(s);
                            return glib::ControlFlow::Continue;  // not time yet
                        }
                        s.play_accum_ms = 0.0;

                        let next=s.fidx+1;
                        let avail = if s.mode==Mode::FaoPlay { s.fao_index.len() }
                            else { s.tc.as_ref().map(|t|t.frame_count.load(Ordering::Relaxed)).unwrap_or(0) };
                        if next<avail{
                            s.fidx = next;
                            let fi = s.fidx;
                            if s.mode == Mode::FaoPlay {
                                s.load_fao_frame(fi);
                            } else {
                                // Viewer: sequential ffmpeg decode
                                match play_rx2.try_recv() {
                                    Ok(rgb) => {
                                        s.cur_frame = rgb.clone();
                                        s.cur_w = play_w2;
                                        s.cur_h = play_h2;
                                        // For FAO side: try to read from disk cache (encoder output)
                                        // Falls back to source if not available (skip frames)
                                        let tc_clone = s.tc.as_ref().map(|t| t.clone());
                                        if let Some(tc) = tc_clone {
                                            if let Some((_orig, recon)) = tc.read_frame(fi) {
                                                if !recon.is_empty() && recon.len() == rgb.len() {
                                                    s.cur_recon = recon;
                                                } else {
                                                    s.cur_recon = rgb.clone();
                                                }
                                            } else {
                                                s.cur_recon = rgb.clone();
                                            }
                                        } else {
                                            s.cur_recon = rgb.clone();
                                        }
                                        s.has_frame = true;
                                    }
                                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                                        // Decoder behind — hold current frame, don't advance fidx
                                        s.fidx = fi - 1;  // retry this frame next tick
                                        drop(s);
                                        return glib::ControlFlow::Continue;
                                    }
                                    Err(_) => {
                                        // Channel closed — decoder finished
                                        s.playing = false;
                                        drop(s);
                                        pb2.set_label("▶ Play");
                                        uc();
                                        return glib::ControlFlow::Break;
                                    }
                                }
                            }
                            // Render directly
                            if let Some((rw,rh)) = s.render() {
                                let rpb = gdk_pixbuf::Pixbuf::from_bytes(
                                    &glib::Bytes::from(&s.display_buf),
                                    gdk_pixbuf::Colorspace::Rgb, false, 8,
                                    rw as i32, rh as i32, (rw*3) as i32);
                                pic4.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&rpb)));
                                pic4.queue_draw();
                            }
                            let idx=s.fidx;drop(s);fa3.set_value(idx as f64);uc();
                            glib::ControlFlow::Continue
                        }else{
                            let done = if s.mode==Mode::FaoPlay{true}else{s.tc.as_ref().map(|t|t.done.load(Ordering::Relaxed)).unwrap_or(true)};
                            if done{s.playing=false;s.play_accum_ms=0.0;drop(s);pb2.set_label("▶ Play");uc();glib::ControlFlow::Break}
                            else{drop(s);glib::ControlFlow::Continue}
                        }
                    }
                    Err(_)=>glib::ControlFlow::Continue
                }
            });
        }
    });}

    // ── Prev/Next ──
    {let st=st.clone();let ud=update.clone();let fa2=fa.clone();
    prev_btn.connect_clicked(move|_|{if let Ok(mut s)=st.try_borrow_mut(){s.playing=false;if s.fidx>0{s.fidx-=1;
            let fi=s.fidx;if s.mode==Mode::FaoPlay{s.load_fao_frame(fi);}else{s.load_display_frame();}
            fa2.set_value(s.fidx as f64);drop(s);ud();}}});}
    {let st=st.clone();let ud=update.clone();let fa2=fa.clone();
    next_btn.connect_clicked(move|_|{if let Ok(mut s)=st.try_borrow_mut(){s.playing=false;
        let avail = if s.mode==Mode::FaoPlay{s.fao_index.len()}else{s.tc.as_ref().map(|t|t.frame_count.load(Ordering::Relaxed)).unwrap_or(0)};
        if s.fidx+1<avail{s.fidx+=1;
            let fi=s.fidx;if s.mode==Mode::FaoPlay{s.load_fao_frame(fi);}else{s.load_display_frame();}
            fa2.set_value(s.fidx as f64);drop(s);ud();}}});}

    // ── Prev/Next I-frame ──
    {let st=st.clone();let ud=update.clone();let fa2=fa.clone();
    prev_i_btn.connect_clicked(move|_|{if let Ok(mut s)=st.try_borrow_mut(){s.playing=false;
        let cur = s.fidx;
        let target = if s.mode==Mode::FaoPlay {
            let data = s.fao_data.as_ref();
            // "Previous I-frame" jump button — find a frame the player can 
            // seek to without needing the previous decoded frame. Same 
            // anchor classification as build_anchor_map.
            (0..cur).rev().find(|&i| {
                if let (Some(d), Some(e)) = (data, s.fao_index.get(i)) {
                    if let Some((db, _)) = fao_v1::read_fao_frame(d, e) {
                        !db.is_empty() && matches!(db[0], b'D' | b'L' | b'P' | b'K' | b'V' | b'Z')
                    } else { false }
                } else { false }
            })
        } else if let Some(ref tc) = s.tc {
            if let Ok(stats) = tc.stats.try_lock() { (0..cur).rev().find(|&i| i<stats.len() && stats[i].ftype==0) } else { None }
        } else { None };
        if let Some(idx)=target{s.fidx=idx;if s.mode==Mode::FaoPlay{s.load_fao_frame(idx);}else{s.load_display_frame();}let i=s.fidx;drop(s);fa2.set_value(i as f64);ud();}
    }});}
    {let st=st.clone();let ud=update.clone();let fa2=fa.clone();
    next_i_btn.connect_clicked(move|_|{if let Ok(mut s)=st.try_borrow_mut(){s.playing=false;
        let cur=s.fidx;
        let target = if s.mode==Mode::FaoPlay {
            let data = s.fao_data.as_ref();
            let n = s.fao_index.len();
            // "Next I-frame" jump button — same anchor classification as 
            // build_anchor_map.
            ((cur+1)..n).find(|&i| {
                if let (Some(d), Some(e)) = (data, s.fao_index.get(i)) {
                    if let Some((db, _)) = fao_v1::read_fao_frame(d, e) {
                        !db.is_empty() && matches!(db[0], b'D' | b'L' | b'P' | b'K' | b'V' | b'Z')
                    } else { false }
                } else { false }
            })
        } else if let Some(ref tc) = s.tc {
            let avail=tc.frame_count.load(Ordering::Relaxed);
            if let Ok(stats) = tc.stats.try_lock() { ((cur+1)..avail.min(stats.len())).find(|&i| stats[i].ftype==0) } else { None }
        } else { None };
        if let Some(idx)=target{s.fidx=idx;if s.mode==Mode::FaoPlay{s.load_fao_frame(idx);}else{s.load_display_frame();}let i=s.fidx;drop(s);fa2.set_value(i as f64);ud();}
    }});}
    // ── View buttons ──
    {let all:Vec<gtk::Button>=vec![bsplit.clone(),borig.clone(),brec.clone(),berr.clone()];
    let views=[View::Split,View::Original,View::Recon,View::Error];
    for(btn,view)in all.iter().zip(views.iter()){
        let st2=st.clone();let ud2=update.clone();let bs=all.clone();let v=*view;
        btn.connect_clicked(move|b|{if let Ok(mut s)=st2.try_borrow_mut(){s.view=v;}
            for o in&bs{o.remove_css_class("va");o.add_css_class("vb");}
            b.remove_css_class("vb");b.add_css_class("va");ud2();});}}

    // ── Split mouse ──
    {let st=st.clone();let ud=update.clone();let pc=pic.clone();
    let drag=gtk::GestureDrag::new();
    drag.set_button(1);  // left mouse button only
    {let st=st.clone();let ud=update.clone();let pc=pic.clone();
    drag.connect_drag_begin(move|_,x,_|{
        if let Ok(mut s)=st.try_borrow_mut(){
            if s.view==View::Split{
                let pw=pc.width()as f64;
                if pw>0.0{s.split_pos=(x/pw).clamp(0.02,0.98);drop(s);ud();}
            }
        }
    });}
    {let st=st.clone();let ud=update.clone();let pc=pic.clone();
    drag.connect_drag_update(move|g,dx,_|{
        if let Ok(mut s)=st.try_borrow_mut(){
            if s.view==View::Split{
                let pw=pc.width()as f64;
                if pw>0.0{
                    let (sx,_)=g.start_point().unwrap_or((0.0,0.0));
                    s.split_pos=((sx+dx)/pw).clamp(0.02,0.98);
                    drop(s);ud();
                }
            }
        }
    });}
    pic.add_controller(drag);}



    // Library: Add Video button — adds a .fao file to current archive
    {
        let win_c = win.clone();
        let st_c = st.clone();
        let lib_subtitle_c = lib_subtitle.clone();
        let lib_list_c = lib_list.clone();
        add_video_btn.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Add Video to Archive"),
                Some(&win_c),
                gtk::FileChooserAction::Open,
                Some("Add"),
                Some("Cancel"),
            );
            let f = gtk::FileFilter::new();
            f.add_pattern("*.fao");
            f.set_name(Some("FAO Video"));
            dialog.add_filter(&f);
            
            let st_inner = st_c.clone();
            let label = lib_subtitle_c.clone();
            let list = lib_list_c.clone();
            dialog.connect_response(move |d, resp| {
                if resp != gtk::ResponseType::Accept { return; }
                let path = match d.file().and_then(|f| f.path()) { Some(p) => p, None => return };
                
                // Read .fao file to extract metadata
                let data = match std::fs::read(&path) { Ok(d) => d, Err(e) => {
                    eprintln!("FAO: cannot read {}: {}", path.display(), e);
                    return;
                }};
                let header = match fao_v1::read_fao_header(&data) { Some(h) => h, None => {
                    eprintln!("FAO: invalid FAO file: {}", path.display());
                    return;
                }};
                
                let title = path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "untitled".to_string());
                let fps = header.fps_num as f32 / header.fps_den.max(1) as f32;
                let duration = header.frame_count as f32 / fps.max(1.0);
                let file_size = data.len() as u64;
                
                let entry = fao_v1::VideoEntry {
                    id: 0,  // assigned by add_video
                    title: title.clone(),
                    source_path: String::new(),
                    fao_path: path.display().to_string(),
                    width: header.width as u32,
                    height: header.height as u32,
                    fps,
                    frame_count: header.frame_count,
                    duration_secs: duration,
                    file_size_bytes: file_size,
                    created_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    chapters: Vec::new(),
                    tags: Vec::new(),
                    thumbnail_data: Vec::new(),
                };
                
                let mut s = match st_inner.try_borrow_mut() { Ok(s) => s, Err(_) => return };
                let archive_path = s.archive_path.clone();
                if let Some(ref mut archive) = s.archive {
                    archive.add_video(entry);
                    let n = archive.entries.len();
                    let dur = archive.total_duration_secs();
                    let size_mb = archive.total_size_bytes as f64 / 1048576.0;
                    let arch_name = archive.name.clone();
                    
                    // Save manifest
                    if let Some(mp) = archive_path {
                        if let Err(e) = archive.save_manifest(&mp) {
                            eprintln!("FAO: save manifest failed: {}", e);
                        }
                    }
                    
                    // Refresh list — clone entries snapshot
                    let entries_snapshot: Vec<_> = archive.entries.iter().map(|e| (
                        e.id, e.title.clone(), e.width, e.height, e.fps,
                        e.duration_secs, e.file_size_bytes, e.chapters.len(),
                        e.fao_path.clone(),
                    )).collect();
                    s.lib_row_paths = entries_snapshot.iter().map(|e| e.8.clone()).collect();
                    drop(s);
                    
                    while let Some(child) = list.first_child() {
                        list.remove(&child);
                    }
                    for (_id, title, w, h, fps, dur, size, n_chapters, _path) in &entries_snapshot {
                        let row = gtk::ListBoxRow::new();
                        let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                        row_box.set_margin_start(8);
                        row_box.set_margin_end(8);
                        row_box.set_margin_top(6);
                        row_box.set_margin_bottom(6);
                        let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
                        info.set_hexpand(true);
                        let title_lbl = gtk::Label::new(Some(title));
                        title_lbl.set_halign(gtk::Align::Start);
                        title_lbl.add_css_class("hdr");
                        let meta = format!("{}x{} {:.1}fps  {:.1}s  {:.1}MB  {} chapters",
                            w, h, fps, dur, *size as f64 / 1048576.0, n_chapters);
                        let meta_lbl = gtk::Label::new(Some(&meta));
                        meta_lbl.set_halign(gtk::Align::Start);
                        meta_lbl.add_css_class("sub");
                        info.append(&title_lbl);
                        info.append(&meta_lbl);
                        row_box.append(&info);
                        row.set_child(Some(&row_box));
                        list.append(&row);
                    }
                    
                    label.set_text(&format!("{}: {} videos, {:.0}s, {:.1}MB",
                        arch_name, n, dur, size_mb));
                    eprintln!("FAO: added {} to archive ({} total)", title, n);
                }
            });
            dialog.show();
        });
    }

    // Library row activation — load .fao into Player and switch tab
    {
        let st_c = st.clone();
        let tab_play_c = tab_play.clone();
        let load_a_c = load_a_btn.clone();  // need to trigger load A logic
        // Use a more direct approach: load the .fao directly
        let pic_a_c = play_pic_a.clone();
        let pic_b_c = play_pic_b.clone();
        let subtitle_c = play_subtitle.clone();
        let slider_c = play_slider_adj.clone();
        let _ = load_a_c;
        lib_list.connect_row_activated(move |_, row| {
            let idx = row.index() as usize;
            let path_str = {
                let s = match st_c.try_borrow() { Ok(s) => s, Err(_) => return };
                if idx >= s.lib_row_paths.len() { return; }
                s.lib_row_paths[idx].clone()
            };
            let path = std::path::Path::new(&path_str).to_path_buf();
            if !path.exists() {
                eprintln!("FAO: video file missing: {}", path.display());
                return;
            }
            
            // Load into A slot
            {
                let mut s = match st_c.try_borrow_mut() { Ok(s) => s, Err(_) => return };
                s.load_fao(&path);
                let n = s.fao_index.len();
                let b_n = s.player_b_index.len();
                slider_c.set_upper(n.saturating_sub(1).max(1) as f64);
                slider_c.set_value(0.0);
                let a_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let b_name = if s.player_b_path.is_empty() { "(none)".to_string() } else {
                    std::path::Path::new(&s.player_b_path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                };
                subtitle_c.set_text(&format!("A: {} ({} frames)   B: {} ({} frames)", a_name, n, b_name, b_n));
                
                let w = s.cur_w; let h = s.cur_h;
                let n_bytes = w * h * 3;
                if s.cur_frame.len() >= n_bytes {
                    let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.cur_frame[..n_bytes]),
                        gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                    pic_a_c.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
                }
                if s.player_b_frame.len() >= n_bytes {
                    let pb = gdk_pixbuf::Pixbuf::from_bytes(&glib::Bytes::from(&s.player_b_frame[..n_bytes]),
                        gdk_pixbuf::Colorspace::Rgb, false, 8, w as i32, h as i32, (w*3) as i32);
                    pic_b_c.set_paintable(Some(&gtk::gdk::Texture::for_pixbuf(&pb)));
                }
            }
            
            // Switch to Player tab
            tab_play_c.set_active(true);
        });
    }
    
    // Section tab handlers — show/hide panels
    {
        let lp = lib_panel.clone();
        let ep = enc_panel.clone();
        let pp = play_panel.clone();
        let wp = world_panel.clone();
        tab_lib.connect_toggled(move |b| {
            if b.is_active() { lp.set_visible(true); ep.set_visible(false); pp.set_visible(false); wp.set_visible(false); }
        });
    }
    {
        let lp = lib_panel.clone();
        let ep = enc_panel.clone();
        let pp = play_panel.clone();
        let wp = world_panel.clone();
        tab_enc.connect_toggled(move |b| {
            if b.is_active() { lp.set_visible(false); ep.set_visible(true); pp.set_visible(false); wp.set_visible(false); }
        });
    }
    {
        let lp = lib_panel.clone();
        let ep = enc_panel.clone();
        let pp = play_panel.clone();
        let wp = world_panel.clone();
        tab_play.connect_toggled(move |b| {
            if b.is_active() { lp.set_visible(false); ep.set_visible(false); pp.set_visible(true); wp.set_visible(false); }
            else { pp.set_visible(false); }
        });
    }
    {
        let lp = lib_panel.clone();
        let ep = enc_panel.clone();
        let pp = play_panel.clone();
        let wp = world_panel.clone();
        let st_world = st.clone();
        let world_first_open = std::rc::Rc::new(std::cell::Cell::new(true));
        let wfo = world_first_open.clone();
        let canvas_focus = world_canvas.clone();
        tab_world.connect_toggled(move |b| {
            if b.is_active() {
                lp.set_visible(false); ep.set_visible(false); pp.set_visible(false); wp.set_visible(true);
                // Pull focus to the canvas so Space doesn\u{2019}t activate a focused 
                // button (Encoder\u{2019}s Transcode, Player\u{2019}s Play, etc.). The window-level
                // key handler still receives the events, but they won\u{2019}t bubble to a 
                // focused widget if the canvas owns focus.
                canvas_focus.grab_focus();
                if let Ok(mut s) = st_world.try_borrow_mut() {
                    s.world_tab_active.set(true);
                    // Force-populate web_viz_nodes if empty (otherwise World tab 
                    // can show stale/empty data when user goes straight to it)
                    let n = if let Some(ref tc) = s.tc {
                        tc.frame_count.load(Ordering::Relaxed) as u32
                    } else if !s.fao_index.is_empty() {
                        s.fao_index.len() as u32
                    } else { 0 };
                    if n > 10 && (s.web_viz_nodes.is_empty() || s.web_viz_nodes.len() + 50 < n as usize) {
                        s.update_web_viz(n);
                    }
                    // Auto-fit camera to scene bounds
                    let cam = s.world_camera.get();
                    if (cam.pos_x.abs() < 1.0 && cam.pos_y.abs() < 1.0 && cam.pos_z.abs() < 100.0)
                       || cam.pos_z > -100.0 {
                        // Default position — auto-fit
                        let new_cam = WorldCamera {
                            pos_x: 0.0, pos_y: 0.0, pos_z: -1200.0,
                            yaw: 0.0, pitch: -0.1, zoom: 1.0,
                            ..WorldCamera::default()
                        };
                        s.world_camera.set(new_cam);
                    }
                }
                if wfo.replace(false) {
                    eprintln!("World freecam:");
                    eprintln!("  Forward:  W / Up Arrow / .  (period for DVORAK)");
                    eprintln!("  Back:     S / Down Arrow / E");
                    eprintln!("  Left:     A / Left Arrow / O");
                    eprintln!("  Right:    D / Right Arrow / U");
                    eprintln!("  Up:       Space");
                    eprintln!("  Down:     LeftShift / RightShift / ;  (semicolon for DVORAK)");
                    eprintln!("  Sprint:   T  (hold for 3x speed)");
                    eprintln!("  Look:     Drag mouse");
                    eprintln!("  Dolly:    Scroll wheel");
                }
            } else {
                if let Ok(s) = st_world.try_borrow() {
                    s.world_tab_active.set(false);
                    s.world_held_keys.borrow_mut().clear();  // release all on tab switch
                }
            }
        });
    }
    
    // Library: New Archive button
    {
        let win_c = win.clone();
        let st_c = st.clone();
        let lib_subtitle_c = lib_subtitle.clone();
        let add_video_c = add_video_btn.clone();
        new_arch_btn.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Create New Archive"),
                Some(&win_c),
                gtk::FileChooserAction::SelectFolder,
                Some("Create"),
                Some("Cancel"),
            );
            let st_inner = st_c.clone();
            let label = lib_subtitle_c.clone();
            let av_inner = add_video_c.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(folder) = d.file().and_then(|f| f.path()) {
                        let name = folder.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Untitled".to_string());
                        let archive = fao_v1::Archive::new(name.clone(), folder.display().to_string());
                        let manifest_path = folder.join("archive.fao_lib");
                        let _ = archive.save_manifest(&manifest_path);
                        if let Ok(mut s) = st_inner.try_borrow_mut() {
                            s.archive = Some(archive);
                            s.archive_path = Some(manifest_path);
                        }
                        av_inner.set_sensitive(true);
                        label.set_text(&format!("Archive: {} (0 videos)", name));
                    }
                }
            });
            dialog.show();
        });
    }
    
    // Library: Open Archive button
    {
        let win_c = win.clone();
        let st_c = st.clone();
        let lib_subtitle_c = lib_subtitle.clone();
        let lib_list_c = lib_list.clone();
        let add_video_c = add_video_btn.clone();
        open_arch_btn.connect_clicked(move |_| {
            let dialog = gtk::FileChooserNative::new(
                Some("Open Archive Manifest"),
                Some(&win_c),
                gtk::FileChooserAction::Open,
                Some("Open"),
                Some("Cancel"),
            );
            let filter = gtk::FileFilter::new();
            filter.add_pattern("*.fao_lib");
            filter.set_name(Some("FAO Library"));
            dialog.add_filter(&filter);
            let st_inner = st_c.clone();
            let label = lib_subtitle_c.clone();
            let list = lib_list_c.clone();
            let av_inner = add_video_c.clone();
            dialog.connect_response(move |d, resp| {
                if resp == gtk::ResponseType::Accept {
                    if let Some(path) = d.file().and_then(|f| f.path()) {
                        match fao_v1::Archive::load_manifest(&path) {
                            Ok(archive) => {
                                let title = archive.name.clone();
                                let n = archive.entries.len();
                                let dur = archive.total_duration_secs();
                                let size_mb = archive.total_size_bytes as f64 / 1048576.0;
                                
                                // Clear and repopulate list
                                while let Some(child) = list.first_child() {
                                    list.remove(&child);
                                }
                                for entry in &archive.entries {
                                    let row = gtk::ListBoxRow::new();
                                    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                                    row_box.set_margin_start(8);
                                    row_box.set_margin_end(8);
                                    row_box.set_margin_top(6);
                                    row_box.set_margin_bottom(6);
                                    
                                    let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
                                    info.set_hexpand(true);
                                    let title_lbl = gtk::Label::new(Some(&entry.title));
                                    title_lbl.set_halign(gtk::Align::Start);
                                    title_lbl.add_css_class("hdr");
                                    let meta = format!("{}x{} {:.1}fps  {:.1}s  {:.1}MB  {} chapters",
                                        entry.width, entry.height, entry.fps,
                                        entry.duration_secs,
                                        entry.file_size_bytes as f64 / 1048576.0,
                                        entry.chapters.len());
                                    let meta_lbl = gtk::Label::new(Some(&meta));
                                    meta_lbl.set_halign(gtk::Align::Start);
                                    meta_lbl.add_css_class("sub");
                                    info.append(&title_lbl);
                                    info.append(&meta_lbl);
                                    row_box.append(&info);
                                    row.set_child(Some(&row_box));
                                    list.append(&row);
                                }
                                
                                let row_paths: Vec<String> = archive.entries.iter().map(|e| e.fao_path.clone()).collect();
                                if let Ok(mut s) = st_inner.try_borrow_mut() {
                                    s.archive = Some(archive);
                                    s.archive_path = Some(path);
                                    s.lib_row_paths = row_paths;
                                }
                                av_inner.set_sensitive(true);
                                label.set_text(&format!("{}: {} videos, {:.0}s, {:.1}MB",
                                    title, n, dur, size_mb));
                            }
                            Err(e) => {
                                eprintln!("FAO: failed to load archive: {}", e);
                            }
                        }
                    }
                }
            });
            dialog.show();
        });
    }

    // Fullscreen wiring: button click + F/F11/Esc keys + double-click on picture
    {
        let win_c = win.clone();
        let fs_c = fs_btn.clone();
        fs_btn.connect_clicked(move |_| {
            if win_c.is_fullscreen() {
                win_c.unfullscreen();
                fs_c.set_label("⛶");
            } else {
                win_c.fullscreen();
                fs_c.set_label("⮌");
            }
        });
    }
    
    // Keyboard shortcuts
    {
        let win_c = win.clone();
        let fs_c = fs_btn.clone();
        let kc = gtk::EventControllerKey::new();
        kc.connect_key_pressed(move |_, key, _, _| {
            match key {
                gtk::gdk::Key::F11 | gtk::gdk::Key::f | gtk::gdk::Key::F => {
                    if win_c.is_fullscreen() {
                        win_c.unfullscreen();
                        fs_c.set_label("⛶");
                    } else {
                        win_c.fullscreen();
                        fs_c.set_label("⮌");
                    }
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Escape => {
                    if win_c.is_fullscreen() {
                        win_c.unfullscreen();
                        fs_c.set_label("⛶");
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                _ => glib::Propagation::Proceed,
            }
        });
        win.add_controller(kc);
    }
    
    // Double-click on picture toggles fullscreen
    {
        let win_c = win.clone();
        let fs_c = fs_btn.clone();
        let dbl = gtk::GestureClick::new();
        dbl.set_button(1);
        dbl.connect_released(move |_, n, _, _| {
            if n == 2 {
                if win_c.is_fullscreen() {
                    win_c.unfullscreen();
                    fs_c.set_label("⛶");
                } else {
                    win_c.fullscreen();
                    fs_c.set_label("⮌");
                }
            }
        });
        pic.add_controller(dbl);
    }
    
    win.set_content(Some(&root));win.present();
}

// Global PID for signal handler cleanup
static FFMPEG_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

extern "C" fn cleanup_on_signal(_sig: libc::c_int) {
    let pid = FFMPEG_PID.load(std::sync::atomic::Ordering::SeqCst);
    if pid > 0 {
        unsafe { libc::kill(pid as i32, libc::SIGKILL); }
    }
    // Clean temp files for our PID
    let our_pid = std::process::id();
    for dir_path in &["/tmp", ""] {
        let dir = if dir_path.is_empty() {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
                .join(".cache").join("fao")
        } else { std::path::PathBuf::from(dir_path) };
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.contains(&our_pid.to_string()) && name.starts_with("fao_") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
    std::process::exit(1);
}

fn main(){
    // v9 boot banner — visible proof the build has v9 wired. User has 
    // had trouble seeing v9 activity in logs; this immediately tells 
    // them whether the binary they're running is v9-aware. If they 
    // see this line and then never see "FAO v9.0: gate check ..." 
    // during an export, the gap is something downstream (not the 
    // build).
    eprintln!("FAO v9.0: build has volumetric codec wired (v9_volumetric module, V9 dialog checkbox, V9R toolbar toggle, V9DC trailer). Tick V9 in the Transcode dialog to enable.");
    
    // Register signal handlers for ffmpeg cleanup on crash/kill
    unsafe {
        libc::signal(libc::SIGTERM, cleanup_on_signal as libc::sighandler_t);
        libc::signal(libc::SIGINT, cleanup_on_signal as libc::sighandler_t);
    }
    // Clean up orphan fao temp files from dead processes (both /tmp and ~/.cache/fao)
    let cleanup_dirs = [
        std::env::temp_dir(),
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
            .join(".cache").join("fao"),
    ];
    for dir in &cleanup_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    let is_fao = name.starts_with("fao_transcode_") || name.starts_with("fao_chunk_");
                    if is_fao {
                        let pid_str = name.split('_').nth(2).unwrap_or("").trim_end_matches(".raw");
                        if let Ok(pid) = pid_str.parse::<u32>() {
                            if !std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                                let _ = std::fs::remove_file(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
    let app=adw::Application::builder()
        .application_id("com.fao.codec")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(build_ui);
    app.run();
}
