//! lanes.rs — a pool of CONCURRENT generation lanes over ONE resident model.
//!
//! The app's `Inference` is a single worker thread: every `Cmd::Generate` funnels through one channel
//! to one backend, so two long-running director tasks serialize (the fan's "round-robin" is
//! time-slicing, not parallelism). The engine underneath never had that limit — weights are read-only
//! after load (RULESET K9) and `serve.rs` already runs many threads against one `Arc<Model>`. This
//! module brings that to the app: `Engine::lane()` derives sibling engines sharing the resident
//! weights + device + pipeline cache (per-lane cost ≈ one KV cache), and `LanePool` runs N of them as
//! job-pulling threads. Two `:agent` explorations on a 3B then genuinely decode AT THE SAME TIME —
//! decode is bandwidth-bound, so concurrent lanes ride the same weight reads the way `run_batched`
//! does, without forcing every task into one lockstep batch.
//!
//! Discipline (matches fabric/dispatch/workspace_exec): the model sits behind the [`LaneGen`] seam, so
//! the pool's scheduling — FIFO pickup, per-job cancel, tagged token streams, shutdown — is unit-tested
//! here with scripted generators, no GPU. `spawn()` (the wgpu path) is the thin IO shell over
//! `spawn_with()` (the tested core).
//!
//! Event flow: `submit()` returns a `JobId`; the pool emits `LaneEvt::{Token, Done, Error}` tagged with
//! that id on `rx` — the UI drains it exactly like the `Inference` event stream, but tags let one
//! drain serve many in-flight generations (the fan maps job → branch).

use logscope::llm_backend::GenParams;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

/// The one thing a lane needs from a model: stream a completion. Mirrors `Engine::generate` exactly so
/// the real impl is a one-liner; tests script it.
pub trait LaneGen: Send + 'static {
    fn generate(&mut self, prompt: &str, p: &GenParams, cancel: &dyn Fn() -> bool,
                on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String>;
}

#[cfg(feature = "wgpu-engine")]
impl LaneGen for crate::backend::engine::Engine {
    fn generate(&mut self, prompt: &str, p: &GenParams, cancel: &dyn Fn() -> bool,
                on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
        crate::backend::engine::Engine::generate(self, prompt, p, cancel, on)
    }
}

pub type JobId = u64;

/// Pool → UI events, tagged by job so one drain serves many in-flight generations.
#[derive(Debug)]
pub enum LaneEvt {
    /// The async loader finished: the pool is serving with this many lanes on this model.
    Ready { lanes: usize, model: String, template: crate::backend::TemplateKind },
    /// The async loader failed; the pool will complete queued jobs with `Error`s.
    LoadError(String),
    Token { job: JobId, text: String },
    Done { job: JobId, text: String, tokens: usize, secs: f32, lane: usize },
    Error { job: JobId, err: String },
}

struct Job { id: JobId, prompt: String, params: GenParams, cancel: Arc<AtomicBool> }

type Queue = Arc<(Mutex<VecDeque<Job>>, Condvar)>;

/// N generation lanes pulling from one FIFO. See module docs.
pub struct LanePool {
    queue: Queue,
    pub rx: Receiver<LaneEvt>,
    evt_tx: Sender<LaneEvt>, // kept for the wgpu loader thread + late spawns
    cancels: Arc<Mutex<HashMap<JobId, Arc<AtomicBool>>>>,
    shutdown: Arc<AtomicBool>,
    next_id: JobId,
    lanes: usize,
}

impl LanePool {
    /// The tested core: run `gens` as lanes over a shared FIFO. Lanes start serving immediately.
    pub fn spawn_with(gens: Vec<Box<dyn LaneGen>>) -> Self {
        let (evt_tx, rx) = channel::<LaneEvt>();
        let pool = Self {
            queue: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
            rx, evt_tx: evt_tx.clone(),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            next_id: 1, lanes: gens.len(),
        };
        for (lane, g) in gens.into_iter().enumerate() {
            spawn_lane(lane, g, pool.queue.clone(), evt_tx.clone(), pool.cancels.clone(), pool.shutdown.clone());
        }
        pool
    }

    /// The wgpu shell: load `path` once on a loader thread, derive `lanes-1` weight-sharing siblings
    /// via `Engine::lane()`, then serve. Emits `Ready` (or `LoadError`) on `rx`. Jobs may be submitted
    /// immediately — they queue until the lanes come up.
    pub fn spawn(path: String, lanes: usize) -> Self {
        let lanes = lanes.max(1);
        let (evt_tx, rx) = channel::<LaneEvt>();
        let pool = Self {
            queue: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
            rx, evt_tx: evt_tx.clone(),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            shutdown: Arc::new(AtomicBool::new(false)),
            next_id: 1, lanes,
        };
        #[cfg(feature = "wgpu-engine")]
        {
            let (queue, cancels, shutdown) = (pool.queue.clone(), pool.cancels.clone(), pool.shutdown.clone());
            std::thread::spawn(move || {
                let primary = match crate::backend::engine::Engine::load(&path) {
                    Ok(e) => e,
                    Err(e) => { let _ = evt_tx.send(LaneEvt::LoadError(e)); return; }
                };
                let _ = evt_tx.send(LaneEvt::Ready {
                    lanes, model: primary.model_name().to_string(), template: primary.template(),
                });
                let mut engines: Vec<crate::backend::engine::Engine> = Vec::with_capacity(lanes);
                for n in 1..lanes { engines.push(primary.lane(n)); }
                engines.insert(0, primary);
                for (lane, e) in engines.into_iter().enumerate() {
                    spawn_lane(lane, Box::new(e), queue.clone(), evt_tx.clone(), cancels.clone(), shutdown.clone());
                }
            });
        }
        #[cfg(not(feature = "wgpu-engine"))]
        {
            let _ = path;
            let _ = evt_tx.send(LaneEvt::LoadError(
                "lanes need the clean-room engine — rebuild with --features wgpu-engine".into()));
        }
        pool
    }

    pub fn lanes(&self) -> usize { self.lanes }
    pub fn queued(&self) -> usize { self.queue.0.lock().map(|q| q.len()).unwrap_or(0) }

    /// Queue a generation; the tagged events for it arrive on `rx`.
    pub fn submit(&mut self, prompt: String, params: GenParams) -> JobId {
        let id = self.next_id; self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut m) = self.cancels.lock() { m.insert(id, cancel.clone()); }
        if let Ok(mut q) = self.queue.0.lock() { q.push_back(Job { id, prompt, params, cancel }); }
        self.queue.1.notify_one();
        id
    }

    /// Request cancellation of one job (queued → completes as `Error("cancelled")`; in flight → the
    /// lane's cancel closure stops it at the next token boundary, same as `Inference::cancel`).
    pub fn cancel(&self, id: JobId) {
        if let Ok(m) = self.cancels.lock() {
            if let Some(c) = m.get(&id) { c.store(true, Ordering::Relaxed); }
        }
    }

    /// Cancel everything — queued and in flight.
    pub fn cancel_all(&self) {
        if let Ok(m) = self.cancels.lock() {
            for c in m.values() { c.store(true, Ordering::Relaxed); }
        }
        self.queue.1.notify_all();
    }

    /// Stop the lanes. In-flight generations are cancelled; queued jobs complete as `Error`s.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.cancel_all();
    }
}

impl Drop for LanePool {
    fn drop(&mut self) { self.shutdown(); }
}

fn spawn_lane(lane: usize, mut g: Box<dyn LaneGen>, queue: Queue, evt: Sender<LaneEvt>,
              cancels: Arc<Mutex<HashMap<JobId, Arc<AtomicBool>>>>, shutdown: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        loop {
            // Pull the next job (or exit on shutdown). The timeout re-checks shutdown so a quiet pool
            // still winds down promptly.
            let job = {
                let mut q = match queue.0.lock() { Ok(q) => q, Err(_) => return };
                loop {
                    if shutdown.load(Ordering::Relaxed) { return; }
                    if let Some(j) = q.pop_front() { break j; }
                    q = match queue.1.wait_timeout(q, std::time::Duration::from_millis(200)) {
                        Ok((g, _)) => g, Err(_) => return,
                    };
                }
            };
            if job.cancel.load(Ordering::Relaxed) || shutdown.load(Ordering::Relaxed) {
                if let Ok(mut m) = cancels.lock() { m.remove(&job.id); }
                let _ = evt.send(LaneEvt::Error { job: job.id, err: "cancelled".into() });
                continue;
            }
            let t0 = std::time::Instant::now();
            let (cflag, sflag) = (job.cancel.clone(), shutdown.clone());
            let cancel = move || cflag.load(Ordering::Relaxed) || sflag.load(Ordering::Relaxed);
            let mut text = String::new();
            let (etx, id) = (evt.clone(), job.id);
            let mut on = |s: &str| {
                text.push_str(s);
                let _ = etx.send(LaneEvt::Token { job: id, text: s.to_string() });
                true
            };
            let res = g.generate(&job.prompt, &job.params, &cancel, &mut on);
            drop(on);
            if let Ok(mut m) = cancels.lock() { m.remove(&job.id); }
            let secs = t0.elapsed().as_secs_f32();
            match res {
                _ if job.cancel.load(Ordering::Relaxed) || shutdown.load(Ordering::Relaxed) =>
                    { let _ = evt.send(LaneEvt::Error { job: job.id, err: "cancelled".into() }); }
                Ok(tokens) => { let _ = evt.send(LaneEvt::Done { job: job.id, text, tokens, secs, lane }); }
                Err(e) => { let _ = evt.send(LaneEvt::Error { job: job.id, err: e }); }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn params() -> GenParams { GenParams::default() }

    /// Scripted generator: replies "<tag>:<prompt>" in two token chunks. Optionally blocks until
    /// `release` (to hold a lane busy) and counts entries via `started`.
    struct Mock {
        tag: &'static str,
        started: Arc<AtomicUsize>,
        release: Option<Arc<AtomicBool>>,
    }
    impl LaneGen for Mock {
        fn generate(&mut self, prompt: &str, _p: &GenParams, cancel: &dyn Fn() -> bool,
                    on: &mut dyn FnMut(&str) -> bool) -> Result<usize, String> {
            self.started.fetch_add(1, Ordering::SeqCst);
            if let Some(r) = &self.release {
                let t0 = std::time::Instant::now();
                while !r.load(Ordering::SeqCst) {
                    if cancel() { return Ok(0); }
                    if t0.elapsed() > Duration::from_secs(5) { return Err("mock release timeout".into()); }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            on(&format!("{}:", self.tag));
            on(prompt);
            Ok(2)
        }
    }

    fn drain_until<F: FnMut(&LaneEvt) -> bool>(rx: &Receiver<LaneEvt>, mut want: F, n: usize) -> Vec<LaneEvt> {
        let mut hits = Vec::new();
        let t0 = std::time::Instant::now();
        while hits.len() < n && t0.elapsed() < Duration::from_secs(5) {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(e) => { if want(&e) { hits.push(e); } }
                Err(_) => {}
            }
        }
        hits
    }

    #[test]
    fn two_lanes_run_two_jobs_at_the_same_time() {
        // Both mocks block until released; if the pool serialized, only ONE would ever start.
        let started = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let gens: Vec<Box<dyn LaneGen>> = vec![
            Box::new(Mock { tag: "a", started: started.clone(), release: Some(release.clone()) }),
            Box::new(Mock { tag: "b", started: started.clone(), release: Some(release.clone()) }),
        ];
        let mut pool = LanePool::spawn_with(gens);
        let j1 = pool.submit("one".into(), params());
        let j2 = pool.submit("two".into(), params());
        let t0 = std::time::Instant::now();
        while started.load(Ordering::SeqCst) < 2 && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(started.load(Ordering::SeqCst), 2, "both jobs must be IN FLIGHT concurrently, not queued");
        release.store(true, Ordering::SeqCst);
        let dones = drain_until(&pool.rx, |e| matches!(e, LaneEvt::Done { .. }), 2);
        assert_eq!(dones.len(), 2);
        let ids: Vec<JobId> = dones.iter().map(|e| match e { LaneEvt::Done { job, .. } => *job, _ => 0 }).collect();
        assert!(ids.contains(&j1) && ids.contains(&j2));
    }

    #[test]
    fn a_single_lane_serves_fifo() {
        let started = Arc::new(AtomicUsize::new(0));
        let gens: Vec<Box<dyn LaneGen>> = vec![Box::new(Mock { tag: "m", started, release: None })];
        let mut pool = LanePool::spawn_with(gens);
        let a = pool.submit("first".into(), params());
        let b = pool.submit("second".into(), params());
        let c = pool.submit("third".into(), params());
        let dones = drain_until(&pool.rx, |e| matches!(e, LaneEvt::Done { .. }), 3);
        let order: Vec<JobId> = dones.iter().map(|e| match e { LaneEvt::Done { job, .. } => *job, _ => 0 }).collect();
        assert_eq!(order, vec![a, b, c], "one lane must preserve submission order");
    }

    #[test]
    fn tokens_stream_tagged_and_done_carries_the_whole_text() {
        let started = Arc::new(AtomicUsize::new(0));
        let gens: Vec<Box<dyn LaneGen>> = vec![Box::new(Mock { tag: "t", started, release: None })];
        let mut pool = LanePool::spawn_with(gens);
        let id = pool.submit("hello".into(), params());
        let mut streamed = String::new();
        let mut done_text = None;
        let t0 = std::time::Instant::now();
        while done_text.is_none() && t0.elapsed() < Duration::from_secs(5) {
            match pool.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(LaneEvt::Token { job, text }) => { assert_eq!(job, id); streamed.push_str(&text); }
                Ok(LaneEvt::Done { job, text, tokens, .. }) => { assert_eq!(job, id); assert_eq!(tokens, 2); done_text = Some(text); }
                _ => {}
            }
        }
        assert_eq!(done_text.as_deref(), Some("t:hello"));
        assert_eq!(streamed, "t:hello", "Done.text must equal the concatenated Token stream");
    }

    #[test]
    fn cancelling_a_queued_job_reports_cancelled_without_running_it() {
        let started = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let gens: Vec<Box<dyn LaneGen>> = vec![
            Box::new(Mock { tag: "x", started: started.clone(), release: Some(release.clone()) })];
        let mut pool = LanePool::spawn_with(gens);
        let running = pool.submit("busy".into(), params());
        let doomed = pool.submit("never".into(), params());
        // Wait for the first to occupy the lane, then cancel the queued one.
        let t0 = std::time::Instant::now();
        while started.load(Ordering::SeqCst) < 1 && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        pool.cancel(doomed);
        release.store(true, Ordering::SeqCst);
        let mut got_done = false; let mut got_cancel = false;
        let t0 = std::time::Instant::now();
        while (!got_done || !got_cancel) && t0.elapsed() < Duration::from_secs(5) {
            match pool.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(LaneEvt::Done { job, .. }) => { assert_eq!(job, running); got_done = true; }
                Ok(LaneEvt::Error { job, err }) => { assert_eq!(job, doomed); assert_eq!(err, "cancelled"); got_cancel = true; }
                _ => {}
            }
        }
        assert!(got_done && got_cancel);
        assert_eq!(started.load(Ordering::SeqCst), 1, "the cancelled job must never have started");
    }
}
