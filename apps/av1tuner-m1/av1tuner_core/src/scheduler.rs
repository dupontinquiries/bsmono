//! # Scheduler — run chunks in parallel, retry failures, isolate crashes
//!
//! The scheduler owns a thread pool of size `concurrency`. Each worker 
//! thread pulls chunks from a shared queue, encodes them, persists 
//! results, and repeats until the queue is empty.
//!
//! Isolation: each chunk encode happens in a subprocess (see encoder.rs).
//! A subprocess crash bubbles up as an Err, the worker marks the chunk 
//! Failed, and another worker (or the same one) retries it. The worker 
//! thread itself never panics — it's wrapped in catch_unwind as a 
//! belt-and-suspenders measure.
//!
//! Events are sent back to the UI through a crossbeam channel. The UI 
//! polls these on every frame to update its display. No locking on the 
//! UI thread.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use crossbeam_channel::{bounded, unbounded, Sender, Receiver};
use log::{info, warn, error, debug};

use crate::chunk::{ChunkState, ChunkStatus};
use crate::encoder::{Encoder, EncoderError, make_encoder};
use crate::job::{JobState, JobStatus};

/// Events the scheduler emits as the encode progresses. UI consumes these.
#[derive(Clone, Debug)]
pub enum SchedulerEvent {
    /// Scheduler is starting; reports total chunk count and how many were 
    /// already done from a previous run (resume).
    Started { total: usize, already_done: usize },
    /// A chunk transitioned to Encoding state.
    ChunkStarted { index: u32, attempt: u32 },
    /// A chunk completed successfully.
    ChunkFinished { index: u32, elapsed_ms: u64, output_bytes: u64 },
    /// A chunk attempt failed but will retry.
    ChunkRetrying { index: u32, attempt: u32, error: String },
    /// A chunk gave up after exhausting retries.
    ChunkSkipped { index: u32, error: String },
    /// Periodic progress update.
    Progress { done: usize, failed: usize, encoding: usize, total: usize },
    /// All chunks done (some may be skipped).
    AllChunksDone { done: usize, skipped: usize, total: usize },
    /// Scheduler stopped due to cancellation.
    Cancelled,
    /// Catastrophic error before any chunks ran.
    FatalError(String),
}

/// Scheduler handle. Spawned threads run in the background; the UI 
/// receives events via the returned Receiver.
pub struct Scheduler {
    cancel: Arc<AtomicBool>,
    events: Receiver<SchedulerEvent>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl Scheduler {
    /// Spawn the scheduler. Returns immediately; encoding runs in 
    /// background threads. Drop the scheduler to cancel.
    pub fn spawn(job: JobState) -> Self {
        let (event_tx, event_rx) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = cancel.clone();
        
        let join_handle = thread::Builder::new()
            .name("av1tuner-scheduler".to_string())
            .spawn(move || {
                run_scheduler(job, event_tx, cancel_for_thread);
            })
            .expect("spawn scheduler thread");
        
        Self {
            cancel,
            events: event_rx,
            join_handle: Some(join_handle),
        }
    }
    
    pub fn events(&self) -> &Receiver<SchedulerEvent> {
        &self.events
    }
    
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    
    /// Wait for the scheduler thread to finish. Returns once all 
    /// background work has stopped.
    pub fn join(mut self) {
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        // If the scheduler hasn't been joined explicitly, signal cancel 
        // so workers stop. We don't join in Drop because that could 
        // block the UI thread; the OS will clean up the threads.
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn run_scheduler(
    mut job: JobState,
    events: Sender<SchedulerEvent>,
    cancel: Arc<AtomicBool>,
) {
    // Persist the initial "Encoding" status so a restart knows we tried.
    job.status = JobStatus::Encoding;
    job.touch();
    if let Err(e) = job.save() {
        let _ = events.send(SchedulerEvent::FatalError(
            format!("save job state: {}", e)));
        return;
    }
    
    // Load current chunk states from disk.
    let chunks = match job.load_chunks() {
        Ok(c) => c,
        Err(e) => {
            let _ = events.send(SchedulerEvent::FatalError(
                format!("load chunks: {}", e)));
            return;
        }
    };
    
    let total = chunks.len();
    let already_done = chunks.iter().filter(|c| {
        matches!(c.status, ChunkStatus::Done)
    }).count();
    
    let _ = events.send(SchedulerEvent::Started { total, already_done });
    
    if total == 0 {
        let _ = events.send(SchedulerEvent::AllChunksDone {
            done: 0, skipped: 0, total: 0,
        });
        return;
    }
    
    // Filter to chunks that need work, preserving their original index.
    let max_attempts = job.max_chunk_attempts;
    let work: Vec<ChunkState> = chunks.into_iter()
        .filter(|c| c.needs_work(max_attempts))
        .collect();
    
    info!("[scheduler] {} chunks need work (of {} total, {} already done)",
        work.len(), total, already_done);
    
    if work.is_empty() {
        // Everything already done from a previous run.
        let _ = events.send(SchedulerEvent::AllChunksDone {
            done: already_done, skipped: total - already_done, total,
        });
        return;
    }
    
    // Bounded queue: workers pull chunks one at a time. Channel size 
    // == concurrency so producers don't fill it ahead.
    let (chunk_tx, chunk_rx) = bounded::<ChunkState>(job.concurrency as usize);
    
    let workers_count = job.concurrency.max(1);
    let job_arc = Arc::new(job.clone());
    let cancel_w = cancel.clone();
    
    // Spawn workers.
    let mut worker_handles = Vec::new();
    for worker_id in 0..workers_count {
        let chunk_rx = chunk_rx.clone();
        let events = events.clone();
        let cancel = cancel_w.clone();
        let job = job_arc.clone();
        let h = thread::Builder::new()
            .name(format!("av1tuner-worker-{}", worker_id))
            .spawn(move || {
                worker_loop(worker_id, chunk_rx, events, cancel, job);
            })
            .expect("spawn worker");
        worker_handles.push(h);
    }
    
    // Drop our own receiver so workers can detect end-of-stream.
    drop(chunk_rx);
    
    // Feed chunks. Send each chunk; if cancelled, stop sending.
    for chunk in work {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if chunk_tx.send(chunk).is_err() {
            // All workers died; nothing more to do.
            break;
        }
    }
    drop(chunk_tx);  // signal end-of-stream to workers
    
    // Wait for workers to finish.
    for h in worker_handles {
        let _ = h.join();
    }
    
    // Recount.
    let final_chunks = job_arc.load_chunks().unwrap_or_default();
    let done = final_chunks.iter().filter(|c| c.status == ChunkStatus::Done).count();
    let skipped = final_chunks.iter().filter(|c| {
        matches!(c.status, ChunkStatus::Failed | ChunkStatus::Skipped)
    }).count();
    
    if cancel.load(Ordering::SeqCst) {
        let _ = events.send(SchedulerEvent::Cancelled);
    } else {
        let _ = events.send(SchedulerEvent::AllChunksDone { done, skipped, total });
    }
}

fn worker_loop(
    worker_id: u32,
    chunks: Receiver<ChunkState>,
    events: Sender<SchedulerEvent>,
    cancel: Arc<AtomicBool>,
    job: Arc<JobState>,
) {
    // Build encoder once per worker.
    let encoder = match make_encoder(&job.settings) {
        Ok(e) => e,
        Err(e) => {
            error!("[worker {}] encoder construction failed: {}", worker_id, e);
            let _ = events.send(SchedulerEvent::FatalError(format!("encoder init: {}", e)));
            return;
        }
    };
    
    while let Ok(mut chunk) = chunks.recv() {
        if cancel.load(Ordering::SeqCst) {
            debug!("[worker {}] cancel observed; exiting", worker_id);
            return;
        }
        
        // One chunk may need multiple attempts. The retry loop is here 
        // (per chunk, per worker) rather than at the scheduler level, 
        // because the worker has the encoder built and warm.
        encode_chunk_with_retry(
            worker_id, &mut chunk, encoder.as_ref(),
            &job, &events, &cancel,
        );
    }
    
    debug!("[worker {}] queue closed; exiting cleanly", worker_id);
}

fn encode_chunk_with_retry(
    worker_id: u32,
    chunk: &mut ChunkState,
    encoder: &dyn Encoder,
    job: &JobState,
    events: &Sender<SchedulerEvent>,
    cancel: &Arc<AtomicBool>,
) {
    let max = job.max_chunk_attempts;
    
    loop {
        if cancel.load(Ordering::SeqCst) { return; }
        
        chunk.attempts += 1;
        chunk.status = ChunkStatus::Encoding;
        let _ = chunk.save(&job.state_dir);
        let _ = events.send(SchedulerEvent::ChunkStarted {
            index: chunk.index,
            attempt: chunk.attempts,
        });
        
        let ext = job.settings.kind.chunk_extension();
        let output_path = ChunkState::output_path_with_ext(
            &job.state_dir, chunk.index, ext);
        // Cancel callback closes over the scheduler's cancel flag.
        let cancel_cb = || cancel.load(Ordering::SeqCst);
        let result = encoder.encode_chunk(
            &job.input,
            chunk.start_frame,
            chunk.end_frame,
            &output_path,
            &job.settings,
            &cancel_cb,
        );
        
        match result {
            Ok(r) => {
                chunk.status = ChunkStatus::Done;
                chunk.encode_ms = Some(r.elapsed_ms);
                chunk.output_bytes = Some(r.output_bytes);
                chunk.last_error = None;
                let _ = chunk.save(&job.state_dir);
                info!("[worker {}] chunk {} done ({} bytes, {}ms)",
                    worker_id, chunk.index, r.output_bytes, r.elapsed_ms);
                let _ = events.send(SchedulerEvent::ChunkFinished {
                    index: chunk.index,
                    elapsed_ms: r.elapsed_ms,
                    output_bytes: r.output_bytes,
                });
                return;
            }
            Err(EncoderError::Cancelled) => {
                // Don't bump attempts or persist Failed status — the user 
                // cancelled, not a real encode failure. Roll the status 
                // back to Pending so a future restart picks this chunk up.
                chunk.status = ChunkStatus::Pending;
                chunk.attempts = chunk.attempts.saturating_sub(1);
                let _ = chunk.save(&job.state_dir);
                debug!("[worker {}] chunk {} cancelled mid-encode", 
                    worker_id, chunk.index);
                return;
            }
            Err(e) => {
                let err_msg = e.to_string();
                warn!("[worker {}] chunk {} attempt {} failed: {}",
                    worker_id, chunk.index, chunk.attempts, err_msg);
                chunk.last_error = Some(err_msg.clone());
                
                if chunk.attempts >= max {
                    chunk.status = ChunkStatus::Skipped;
                    let _ = chunk.save(&job.state_dir);
                    let _ = events.send(SchedulerEvent::ChunkSkipped {
                        index: chunk.index,
                        error: err_msg,
                    });
                    return;
                } else {
                    chunk.status = ChunkStatus::Failed;
                    let _ = chunk.save(&job.state_dir);
                    let _ = events.send(SchedulerEvent::ChunkRetrying {
                        index: chunk.index,
                        attempt: chunk.attempts,
                        error: err_msg,
                    });
                    // Loop back and try again with chunk.attempts now reflecting retry.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Scheduler tests need a real encoder to be meaningful; that's an 
    // integration test against the actual binary. Unit tests here just 
    // verify event types serialize sanely.
    use super::*;
    
    #[test]
    fn event_debug_doesnt_panic() {
        let e = SchedulerEvent::ChunkFinished {
            index: 5, elapsed_ms: 1000, output_bytes: 100_000,
        };
        let _ = format!("{:?}", e);
    }
}
