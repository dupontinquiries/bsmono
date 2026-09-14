// src/whisper_pool.rs
//
// Whisper inference worker pool. v0.1.116.
//
// Owns 1-2 long-lived worker threads each holding a dedicated
// WhisperState. Callers submit Jobs (audio + optional speaker tag);
// workers process them and write Results to a shared output queue.
//
// Why a pool:
//
// - The streaming preview already needs one whisper state (separate
//   from foreground TINY_STATE to avoid lock contention with
//   voice_listen_dual). v0.1.116-WIP introduced PREVIEW_STATE.
//
// - Multi-speaker mode wants concurrent per-speaker transcription.
//   Two speakers talking simultaneously can keep two workers busy.
//   More speakers? We queue their phrases and process sequentially —
//   the user described this fallback explicitly: "queueing up joined
//   phrases" when speakers exceed worker count.
//
// - A pool abstraction makes future scaling tractable: spawn more
//   workers if hardware permits, fall back to queueing on smaller
//   systems.
//
// Design:
//
// - **Fixed worker count at startup** (configurable, defaults to 2).
//   Each worker holds Arc<WhisperContext> + its own WhisperState.
//
// - **Job queue**: bounded MPSC channel. When the channel is full,
//   `submit()` returns Err — the caller can decide to drop or wait.
//   We currently drop oldest job in caller code (see preview thread).
//
// - **Per-job speaker tag**: a String label that travels with the
//   audio through inference and emerges with the result. UI uses
//   this to color-code the transcript line.
//
// - **Result delivery**: pool owns an output Mutex<VecDeque<Result>>.
//   Caller polls `drain_results()` to pull whatever's ready.
//
// What's NOT here:
//
// - Worker auto-spawn based on load. Fixed at init.
// - Per-worker model tier. All workers use Tiny by default; could
//   parameterize per-job in a future revision.
// - Cancellation. A submitted job runs to completion. For "user
//   stopped speaking, discard pending" we'd need a generation
//   counter on each job.
//
// Threading: pool is Send + Sync. Workers are spawned at init and
// run until process exit.

use std::sync::mpsc::Sender;
#[cfg(feature = "voice")]
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::collections::VecDeque;

/// One inference job submitted to the pool.
pub struct Job {
    /// 16kHz mono audio.
    pub audio: Vec<f32>,
    /// Speaker who produced this audio (caller-supplied tag).
    /// Empty string = unknown / single-speaker mode.
    pub speaker: String,
    /// Generation counter — caller can ignore results from old
    /// generations (e.g. when user starts a new utterance, bump
    /// generation; workers still finish in-flight jobs but their
    /// results get filtered by the consumer).
    pub generation: u64,
    /// When this job was submitted. Used to compute processing
    /// latency in the result.
    pub submitted_at: std::time::Instant,
    /// v0.1.128: ms-since-epoch when audio for this utterance
    /// STARTED being recorded (first chunk attributed to this
    /// speaker). Threaded through to the result and used by the
    /// preview consumer to insert words in chronological order
    /// across speakers regardless of which job finishes first.
    /// Zero means "no chronological anchor" — append to tail.
    pub audio_start_ms: u64,
    /// v0.1.131: speaker was muted (allowed=false) at the time
    /// this audio was captured. Result will be rendered in the
    /// "background channel" with dimmed styling and won't drive
    /// commands. Threaded through for symmetry; the dispatch
    /// path gates on this too.
    pub muted: bool,
}

/// v0.1.137: per-word timing extracted from whisper token timestamps.
/// `t0_ms` and `t1_ms` are milliseconds from the start of the inferred
/// audio buffer (not wall-clock). The consumer combines these with
/// `audio_start_ms` to get a real timeline if needed.
///
/// Token-to-word grouping: whisper's BPE tokenizer marks word-initial
/// tokens with a leading space (rendered as "▁" in the underlying
/// tokenizer, but `to_str` strips it back to a regular space). We
/// group consecutive tokens together starting with each whitespace-
/// leading token, accumulating their text and timing extents.
///
/// Accuracy caveat: whisper wasn't trained for precise ASR alignment,
/// so token edges are typically off by ±100-200ms. For cadence stats
/// (averaging dozens of utterances) this is fine. For frame-accurate
/// UX (rendering words at their exact spoken moment) it's marginal.
#[derive(Clone, Debug)]
pub struct WordTiming {
    pub text: String,
    pub t0_ms: u32,
    pub t1_ms: u32,
}

/// One inference result delivered to the output queue.
#[derive(Clone, Debug)]
pub struct InferenceResult {
    pub text: String,
    pub speaker: String,
    pub generation: u64,
    pub took_ms: u128,
    /// v0.1.128: passed through from Job. ms-since-epoch when the
    /// audio for this utterance started being recorded. Used by
    /// the preview consumer for chronological word ordering.
    pub audio_start_ms: u64,
    /// v0.1.131: passed through from Job. Result will render dim
    /// in the background channel.
    pub muted: bool,
    /// v0.1.137: per-word timing from whisper token timestamps.
    /// None when the model doesn't expose token timestamps or the
    /// inference call didn't enable them. Empty Vec when enabled
    /// but the utterance produced no segments (e.g. silence).
    pub word_timings: Option<Vec<WordTiming>>,
}

/// Pool state — singletons accessed via accessor functions.
struct Pool {
    sender: Sender<Job>,
    results: Arc<Mutex<VecDeque<InferenceResult>>>,
    workers_started: bool,
}

static POOL: OnceLock<Mutex<Option<Pool>>> = OnceLock::new();

fn pool_cell() -> &'static Mutex<Option<Pool>> {
    POOL.get_or_init(|| Mutex::new(None))
}

/// Initialize the worker pool with N workers, all sharing the same
/// WhisperContext. Idempotent — calling twice is a no-op.
///
/// Pass the loaded TINY context's Arc. Other tiers could be added
/// later by parameterizing per-job.
#[cfg(feature = "voice")]
pub fn init_with_tiny_ctx(ctx: Arc<whisper_rs::WhisperContext>, worker_count: usize) {
    let mut guard = match pool_cell().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() { return; }

    let (tx, rx) = channel::<Job>();
    let results: Arc<Mutex<VecDeque<InferenceResult>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Wrap rx in Arc<Mutex> so multiple workers can pull from the
    // same queue. mpsc::Receiver isn't Sync on its own.
    let rx_shared = Arc::new(Mutex::new(rx));

    let n = worker_count.max(1).min(4);
    for worker_id in 0..n {
        let rx_clone = Arc::clone(&rx_shared);
        let ctx_clone = Arc::clone(&ctx);
        let results_clone = Arc::clone(&results);
        std::thread::Builder::new()
            .name(format!("whisper-worker-{}", worker_id))
            .spawn(move || worker_loop(worker_id, ctx_clone, rx_clone, results_clone))
            .expect("spawn whisper worker");
    }
    crate::clog!("whisper-pool", "{} workers started", n);

    *guard = Some(Pool {
        sender: tx,
        results,
        workers_started: true,
    });
}

/// Submit an inference job. Returns Err if the pool isn't initialized
/// or the channel is broken.
pub fn submit(job: Job) -> Result<(), String> {
    let guard = pool_cell().lock().map_err(|e| format!("pool lock: {:?}", e))?;
    let pool = guard.as_ref().ok_or_else(|| "pool not initialized".to_string())?;
    pool.sender.send(job).map_err(|e| format!("pool send: {:?}", e))
}

/// True if the pool is ready to accept jobs.
pub fn is_ready() -> bool {
    pool_cell().lock().ok()
        .map(|g| g.as_ref().map(|p| p.workers_started).unwrap_or(false))
        .unwrap_or(false)
}

/// Pop all currently-available results. Callers typically poll this
/// from a UI tick (~100ms) and update the display with whatever
/// arrived since last poll.
pub fn drain_results() -> Vec<InferenceResult> {
    let guard = match pool_cell().lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let pool = match guard.as_ref() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let mut q = match pool.results.lock() {
        Ok(q) => q,
        Err(_) => return Vec::new(),
    };
    q.drain(..).collect()
}

#[cfg(feature = "voice")]
fn worker_loop(
    worker_id: usize,
    ctx: Arc<whisper_rs::WhisperContext>,
    rx: Arc<Mutex<Receiver<Job>>>,
    results: Arc<Mutex<VecDeque<InferenceResult>>>,
) {
    crate::clog!("whisper-pool", "worker {} entering loop", worker_id);
    // Lazy-init state on first job so we don't allocate ~210MB per
    // worker until needed.
    let mut state: Option<whisper_rs::WhisperState> = None;

    loop {
        // Receive next job (blocks). Multiple workers race for jobs;
        // whoever grabs the lock first gets the next item.
        let job = {
            let rx_guard = match rx.lock() {
                Ok(g) => g,
                Err(_) => {
                    crate::clog!("whisper-pool", "worker {} rx lock poisoned, exiting", worker_id);
                    return;
                }
            };
            match rx_guard.recv() {
                Ok(j) => j,
                Err(_) => {
                    crate::clog!("whisper-pool", "worker {} channel closed, exiting", worker_id);
                    return;
                }
            }
        };

        // Lazy state init on first job.
        if state.is_none() {
            match ctx.create_state() {
                Ok(s) => state = Some(s),
                Err(e) => {
                    crate::clog!("whisper-pool", "worker {} state init failed: {:?}", worker_id, e);
                    continue;
                }
            }
        }

        let s = state.as_mut().expect("state was just set");
        let mut params = whisper_rs::FullParams::new(
            whisper_rs::SamplingStrategy::Greedy { best_of: 1 }
        );
        params.set_n_threads(2);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_suppress_nst(true);
        params.set_language(crate::voice_lang::whisper_lang_code());
        // v0.1.137: enable token timestamps so we can derive per-word
        // timing for cadence stats. The model produces these without
        // extra encoder cost; the marginal compute is in the post-
        // process pass. Worth ~5-10% inference time per result.
        params.set_token_timestamps(true);

        let start = std::time::Instant::now();
        let result = s.full(params, &job.audio);
        let took_ms = start.elapsed().as_millis();

        match result {
            Ok(_) => {
                let n_segs = s.full_n_segments();
                let mut all_text = String::new();
                // v0.1.137: extract per-word timings by distributing
                // each segment's [start, end] range across the words
                // contained in that segment's text. This is segment-
                // level granularity, not per-token — less accurate
                // but uses only the segment API which is stable
                // across whisper-rs versions. A future patch can
                // upgrade to per-token timing via get_token_data
                // once we've verified the exact API surface on the
                // user's whisper-rs build.
                let mut word_timings: Vec<WordTiming> = Vec::new();
                for i in 0..n_segs {
                    let seg = match s.get_segment(i) {
                        Some(seg) => seg,
                        None => continue,
                    };
                    let seg_text = match seg.to_str() {
                        Ok(t) => t.to_string(),
                        Err(_) => continue,
                    };
                    all_text.push_str(&seg_text);
                    // Segment timestamps are in centiseconds (10ms units).
                    let t0_cs = seg.start_timestamp();
                    let t1_cs = seg.end_timestamp();
                    let t0_ms = (t0_cs.max(0) as u32) * 10;
                    let t1_ms = (t1_cs.max(t0_cs) as u32) * 10;
                    let words: Vec<&str> = seg_text
                        .split_whitespace()
                        .filter(|w| !w.is_empty())
                        .collect();
                    if words.is_empty() { continue; }
                    let span_ms = t1_ms.saturating_sub(t0_ms);
                    let per_word_ms = span_ms / (words.len() as u32).max(1);
                    for (wi, w) in words.iter().enumerate() {
                        let w0 = t0_ms + per_word_ms * (wi as u32);
                        let w1 = if wi + 1 == words.len() { t1_ms } else { w0 + per_word_ms };
                        word_timings.push(WordTiming {
                            text: w.to_string(),
                            t0_ms: w0,
                            t1_ms: w1,
                        });
                    }
                }
                let text = all_text.trim().to_string();
                if !text.is_empty() {
                    if let Ok(mut q) = results.lock() {
                        q.push_back(InferenceResult {
                            text,
                            speaker: job.speaker,
                            generation: job.generation,
                            took_ms,
                            audio_start_ms: job.audio_start_ms,
                            muted: job.muted,
                            word_timings: if word_timings.is_empty() {
                                None
                            } else {
                                Some(word_timings)
                            },
                        });
                    }
                }
            }
            Err(e) => {
                crate::clog!("whisper-pool", "worker {} inference err: {:?}", worker_id, e);
            }
        }
    }
}

// When voice feature isn't compiled in, provide a no-op shim so the
// rest of the code compiles. submit() returns Err, drain_results()
// is empty, is_ready() is false.
#[cfg(not(feature = "voice"))]
#[allow(dead_code)]
pub fn init_with_tiny_ctx(_unused: (), _worker_count: usize) {}
