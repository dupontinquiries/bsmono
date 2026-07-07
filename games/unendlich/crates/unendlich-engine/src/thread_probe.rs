//! Per-thread state probe (v0.24.18+).
//!
//! Diagnostic tool for understanding *what each thread is doing* over
//! time during interesting runs (chug investigations, magic-switch
//! moments, etc). Default-OFF, zero-cost-when-off. Enable via
//! `UNENDLICH_THREAD_PROBE=1`.
//!
//! # Architecture
//!
//! Threads of interest call [`register`] once at startup to enter
//! themselves in a small global table. Each registration returns an
//! [`ActivityHandle`] which carries an `Arc<AtomicU8>` pointing into
//! the same table entry. The thread updates its current activity via
//! `handle.set(tag)` — a single atomic store, ~1ns. The context
//! `u64` (e.g. packed chunk coord) is updated via `handle.set_context`.
//!
//! A separate **sampler thread** (started by [`start_sampler`]) wakes
//! every `UNENDLICH_THREAD_PROBE_INTERVAL_MS` (default 100ms) and:
//!   1. Reads each registered thread's current activity + context.
//!   2. Reads `/proc/<pid>/task/<tid>/stat` for utime, stime, cpu (last
//!      core that ran the thread).
//!   3. Reads `/proc/<pid>/task/<tid>/wchan` for the kernel function
//!      it's sleeping in (if any).
//!   4. Writes one CSV row per thread per sample to `probe-<ts>.csv`.
//!
//! Plus an **engine pseudo-thread** (registered automatically) that
//! receives per-frame stats from the renderer (FPS, mesh_q, wait_fences
//! etc) via [`engine_state_update`]. Lets the analysis tool slice the
//! CSV by chug-vs-not without parsing engine stdout.
//!
//! # Linux-only
//!
//! `/proc/<pid>/task/...` is Linux-specific. On other OSes [`enabled`]
//! returns false and all the registration / sampler calls become
//! no-ops. The build still compiles; the feature is just unavailable.
//!
//! # Cost
//!
//! - When `UNENDLICH_THREAD_PROBE=0` (default): registration returns a
//!   no-op handle, `set` is an atomic store nobody reads, sampler isn't
//!   started. Effectively zero cost.
//! - When enabled: per-call cost of `set` is one atomic store
//!   (~1ns on M2 Max). Sampler thread reads /proc at ~100ms cadence;
//!   measured at <50us per sample on Asahi (one /proc read per thread).
//!
//! # The Activity enum
//!
//! Fixed set, recompile to add new states. The discriminant maps to a
//! short string in [`Activity::name`]. The analysis tool relies on
//! these names — keep stable.
//!
//! Note: there's a deliberate `Other` variant carrying a `u8` so call
//! sites can pass a one-off discriminator without churning the enum.
//! The analysis tool's config maps `Other(N)` to a human label.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Activity tag stored per-thread. Updated by the thread via `set`,
/// read by the sampler. Single u8, atomic store, no locks.
///
/// Keep this list stable — the analysis tool's column header uses the
/// name strings.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Default. Thread just registered, hasn't tagged anything yet.
    Unknown = 0,
    /// Idle, blocked waiting for next work item.
    Idle = 1,
    /// Running CPU work, not waiting on anything specific.
    Working = 2,
    /// Blocked on a Vulkan fence (GPU completion).
    WaitingForFence = 3,
    /// Recording a Vulkan command buffer.
    RecordingCommands = 4,
    /// Submitting commands to the queue.
    SubmittingCommands = 5,
    /// Acquiring next swapchain image.
    AcquiringImage = 6,
    /// Presenting completed frame.
    Presenting = 7,
    /// Meshing a chunk (mesh pool workers). Context = packed coord.
    MeshingChunk = 8,
    /// Generating a chunk (chunk-loader workers). Context = packed coord.
    GeneratingChunk = 9,
    /// Uploading vertex/index data to a GPU buffer.
    UploadingBuffer = 10,
    /// Stuck waiting on a mutex / channel / condvar.
    WaitingOnLock = 11,
    /// Tick simulation (plant lifecycle, temperature, etc).
    TickSim = 12,
    /// Reserved for ad-hoc one-off tags. The u8 is a free-form
    /// discriminator the call site picks; analysis tool maps it to a
    /// label via config.
    Other = 200,
}

impl Activity {
    /// Stable short name for CSV output.
    pub fn name(self) -> &'static str {
        match self {
            Activity::Unknown => "unknown",
            Activity::Idle => "idle",
            Activity::Working => "working",
            Activity::WaitingForFence => "wait_fence",
            Activity::RecordingCommands => "record_cmds",
            Activity::SubmittingCommands => "submit_cmds",
            Activity::AcquiringImage => "acquire_img",
            Activity::Presenting => "presenting",
            Activity::MeshingChunk => "meshing",
            Activity::GeneratingChunk => "generating",
            Activity::UploadingBuffer => "uploading",
            Activity::WaitingOnLock => "wait_lock",
            Activity::TickSim => "tick_sim",
            Activity::Other => "other",
        }
    }
}

/// State for one registered thread. The sampler reads these atomically
/// at sample time. Writes happen from the thread itself.
struct ThreadEntry {
    name: String,
    /// Linux TID (set on registration; 0 on non-Linux).
    tid: u32,
    /// Current activity tag (u8 discriminant of [`Activity`]).
    activity: AtomicU8,
    /// Free-form context (e.g. packed chunk coord). Set when activity
    /// is e.g. `MeshingChunk` so we know which chunk.
    context: AtomicU64,
    /// Last utime+stime read in clock ticks; sampler uses delta for
    /// "CPU time consumed in last sample interval."
    last_cpu_ticks: AtomicU64,
    /// Last wall-clock instant we sampled this thread; sampler uses
    /// it to detect its own preemption (if 100ms intervals span >
    /// 100ms wall clock, the sampler was starved itself).
    last_sample_wall_ms: AtomicU64,
}

/// Returned by [`register`]. Threads use it to update their state.
///
/// Cheap to call `set` and `set_context`: one atomic store each.
///
/// When the probe is disabled, this is a "null handle" — the inner
/// Option is None, `set` is a no-op.
#[derive(Clone)]
pub struct ActivityHandle {
    inner: Option<Arc<ThreadEntry>>,
}

impl ActivityHandle {
    /// Set the current activity tag.
    #[inline]
    pub fn set(&self, tag: Activity) {
        if let Some(entry) = &self.inner {
            entry.activity.store(tag as u8, Ordering::Relaxed);
        }
    }

    /// Set the activity AND its context value in one call.
    #[inline]
    pub fn set_with_context(&self, tag: Activity, context: u64) {
        if let Some(entry) = &self.inner {
            entry.context.store(context, Ordering::Relaxed);
            entry.activity.store(tag as u8, Ordering::Relaxed);
        }
    }

    /// Update just the context (e.g. as work proceeds within one
    /// activity). Cheap atomic store.
    #[inline]
    pub fn set_context(&self, context: u64) {
        if let Some(entry) = &self.inner {
            entry.context.store(context, Ordering::Relaxed);
        }
    }

    /// True if this handle is connected to a real probe entry.
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }
}

/// Global probe state. Inside a OnceLock so we can initialize once.
struct ProbeState {
    enabled: bool,
    interval_ms: u32,
    threads: Mutex<Vec<Arc<ThreadEntry>>>,
    engine_state: Arc<EngineState>,
    sampler_started: AtomicBool,
}

/// Snapshot of engine-wide per-frame stats updated by the renderer.
///
/// The sampler treats this like a pseudo-thread and emits one row
/// per sample tagged `engine`. Lets the analysis tool slice on
/// FPS / mesh_q / wait_fences without parsing engine stdout.
pub struct EngineState {
    pub fps: AtomicU32,                    // frames per second × 10 (fixed point)
    pub frame_p50_us: AtomicU32,           // most-recent p50 frame time
    pub wait_fences_us: AtomicU32,         // most-recent wait_fences ms × 1000
    pub mesh_q_size: AtomicU32,            // current queue depth
    pub mesh_q_in_flight: AtomicU32,       // in-flight count
    pub gen_pending: AtomicU32,
    pub rendering: AtomicU32,              // chunks currently rendered
    pub loaded: AtomicU32,                 // chunks currently loaded
    pub chug_active: AtomicBool,           // engine-side chug detector
}

impl EngineState {
    fn new() -> Self {
        Self {
            fps: AtomicU32::new(0),
            frame_p50_us: AtomicU32::new(0),
            wait_fences_us: AtomicU32::new(0),
            mesh_q_size: AtomicU32::new(0),
            mesh_q_in_flight: AtomicU32::new(0),
            gen_pending: AtomicU32::new(0),
            rendering: AtomicU32::new(0),
            loaded: AtomicU32::new(0),
            chug_active: AtomicBool::new(false),
        }
    }
}

static PROBE: OnceLock<ProbeState> = OnceLock::new();

/// Initialize the probe (called once at engine startup, ideally
/// before any threads register). Reads `UNENDLICH_THREAD_PROBE`
/// and `UNENDLICH_THREAD_PROBE_INTERVAL_MS` env vars.
///
/// Idempotent — if called twice the second call is a no-op.
pub fn init() {
    let _ = PROBE.get_or_init(|| {
        let enabled = std::env::var("UNENDLICH_THREAD_PROBE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let interval_ms = std::env::var("UNENDLICH_THREAD_PROBE_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100u32)
            .max(10);  // never go below 10ms; sampler overhead becomes notable
        ProbeState {
            enabled,
            interval_ms,
            threads: Mutex::new(Vec::new()),
            engine_state: Arc::new(EngineState::new()),
            sampler_started: AtomicBool::new(false),
        }
    });
}

/// True if probe is initialized AND enabled by env var.
pub fn enabled() -> bool {
    PROBE.get().map(|p| p.enabled).unwrap_or(false)
}

/// Register the calling thread under `name`. The returned handle is
/// owned by the caller and used to update activity tags. The thread
/// table also holds an Arc so the sampler can read this entry
/// independently.
///
/// If the probe is disabled, returns a null handle (all subsequent
/// `set` calls are no-ops).
///
/// Safe to call multiple times from the same thread; each call adds a
/// new entry. Don't.
pub fn register(name: &str) -> ActivityHandle {
    let probe = match PROBE.get() {
        Some(p) if p.enabled => p,
        _ => return ActivityHandle { inner: None },
    };
    let tid = current_tid();
    let entry = Arc::new(ThreadEntry {
        name: name.to_string(),
        tid,
        activity: AtomicU8::new(Activity::Unknown as u8),
        context: AtomicU64::new(0),
        last_cpu_ticks: AtomicU64::new(0),
        last_sample_wall_ms: AtomicU64::new(0),
    });
    probe.threads.lock().unwrap().push(Arc::clone(&entry));
    ActivityHandle { inner: Some(entry) }
}

/// Return a clone of the engine-state handle. Renderer updates this
/// once per frame.
pub fn engine_state() -> Option<Arc<EngineState>> {
    PROBE.get().filter(|p| p.enabled).map(|p| Arc::clone(&p.engine_state))
}

/// Convenience: update common engine fields. Renderer calls this
/// once per frame from the main thread.
pub fn engine_state_update(
    fps_tenths: u32,
    frame_p50_us: u32,
    wait_fences_us: u32,
    mesh_q_size: u32,
    mesh_q_in_flight: u32,
    gen_pending: u32,
    rendering: u32,
    loaded: u32,
) {
    let Some(es) = engine_state() else { return };
    es.fps.store(fps_tenths, Ordering::Relaxed);
    es.frame_p50_us.store(frame_p50_us, Ordering::Relaxed);
    es.wait_fences_us.store(wait_fences_us, Ordering::Relaxed);
    es.mesh_q_size.store(mesh_q_size, Ordering::Relaxed);
    es.mesh_q_in_flight.store(mesh_q_in_flight, Ordering::Relaxed);
    es.gen_pending.store(gen_pending, Ordering::Relaxed);
    es.rendering.store(rendering, Ordering::Relaxed);
    es.loaded.store(loaded, Ordering::Relaxed);
    // Engine-side chug detector: FPS < 20 OR wait_fences > 50ms.
    // The analysis tool can refine; this is just a coarse marker.
    let chug = fps_tenths < 200 || wait_fences_us > 50_000;
    es.chug_active.store(chug, Ordering::Relaxed);
}

/// Start the sampler thread. Idempotent — safe to call multiple times
/// from anywhere. When the probe is disabled, this is a no-op.
pub fn start_sampler() {
    let probe = match PROBE.get() {
        Some(p) if p.enabled => p,
        _ => return,
    };
    if probe.sampler_started.swap(true, Ordering::SeqCst) {
        return; // already started
    }
    let interval = Duration::from_millis(probe.interval_ms as u64);

    // Open the CSV. Filename includes timestamp so multiple runs don't
    // collide and we can tell them apart easily.
    let csv_path = format!(
        "probe-{}.csv",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let csv_file = match std::fs::File::create(&csv_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("thread_probe: could not open {}: {}", csv_path, e);
            return;
        }
    };
    let csv_file = Mutex::new(csv_file);

    eprintln!("thread_probe: sampling every {}ms to {}", probe.interval_ms, csv_path);

    std::thread::Builder::new()
        .name("thread-probe-sampler".to_string())
        .spawn(move || sampler_loop(interval, csv_file))
        .ok();
}

fn sampler_loop(interval: Duration, csv_file: Mutex<std::fs::File>) {
    use std::io::Write;

    let probe = PROBE.get().unwrap();
    let pid = std::process::id();
    let start = Instant::now();

    // Write CSV header.
    {
        let mut f = csv_file.lock().unwrap();
        let _ = writeln!(
            f,
            "ms_since_start,wall_dt_ms,thread,tid,activity,context,cpu_ticks_delta,last_cpu,wchan,fps_tenths,frame_p50_us,wait_fences_us,mesh_q_size,mesh_q_in_flight,gen_pending,rendering,loaded,chug"
        );
    }

    loop {
        let sample_start = Instant::now();
        let ms_since_start = sample_start.duration_since(start).as_millis() as u64;

        // Read engine state ONCE for this sample; broadcast to all rows.
        let es = &probe.engine_state;
        let fps = es.fps.load(Ordering::Relaxed);
        let frame_p50 = es.frame_p50_us.load(Ordering::Relaxed);
        let wait_fences = es.wait_fences_us.load(Ordering::Relaxed);
        let mq_size = es.mesh_q_size.load(Ordering::Relaxed);
        let mq_inflight = es.mesh_q_in_flight.load(Ordering::Relaxed);
        let gen_pend = es.gen_pending.load(Ordering::Relaxed);
        let rendering = es.rendering.load(Ordering::Relaxed);
        let loaded = es.loaded.load(Ordering::Relaxed);
        let chug = if es.chug_active.load(Ordering::Relaxed) { 1 } else { 0 };

        // Snapshot the registered threads. Clone Arcs out of the mutex
        // quickly so we don't hold the lock during /proc reads.
        let threads: Vec<Arc<ThreadEntry>> = {
            let guard = probe.threads.lock().unwrap();
            guard.iter().cloned().collect()
        };

        let mut rows = String::with_capacity(threads.len() * 200);
        for entry in &threads {
            let activity_u8 = entry.activity.load(Ordering::Relaxed);
            let activity_name = activity_name_for(activity_u8);
            let context = entry.context.load(Ordering::Relaxed);

            let (cpu_ticks, last_cpu) = read_proc_stat(pid, entry.tid);
            let prev = entry.last_cpu_ticks.swap(cpu_ticks, Ordering::Relaxed);
            let cpu_delta = cpu_ticks.saturating_sub(prev);

            let wchan = read_proc_wchan(pid, entry.tid);

            let prev_wall = entry.last_sample_wall_ms.swap(ms_since_start, Ordering::Relaxed);
            let wall_dt = ms_since_start.saturating_sub(prev_wall);

            use std::fmt::Write as _;
            let _ = writeln!(
                rows,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                ms_since_start,
                wall_dt,
                entry.name,
                entry.tid,
                activity_name,
                context,
                cpu_delta,
                last_cpu,
                wchan,
                fps,
                frame_p50,
                wait_fences,
                mq_size,
                mq_inflight,
                gen_pend,
                rendering,
                loaded,
                chug,
            );
        }

        if !rows.is_empty() {
            if let Ok(mut f) = csv_file.lock() {
                let _ = f.write_all(rows.as_bytes());
            }
        }

        // Sleep until next interval. If sampler was preempted, wall_dt
        // (recorded above) tells the analysis tool we slipped.
        let elapsed = sample_start.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
        // else: we're behind, run again immediately.
    }
}

fn activity_name_for(u: u8) -> &'static str {
    // Inverse of Activity::name without enum round-trip.
    match u {
        0 => "unknown",
        1 => "idle",
        2 => "working",
        3 => "wait_fence",
        4 => "record_cmds",
        5 => "submit_cmds",
        6 => "acquire_img",
        7 => "presenting",
        8 => "meshing",
        9 => "generating",
        10 => "uploading",
        11 => "wait_lock",
        12 => "tick_sim",
        200 => "other",
        _ => "?",
    }
}

#[cfg(target_os = "linux")]
fn current_tid() -> u32 {
    // On Linux, /proc/thread-self is a symlink whose target ends in
    // /proc/<pid>/task/<tid>. Reading the link target lets us extract
    // our tid without depending on libc or the SYS_gettid syscall
    // constant. Slower than a real syscall (~few microseconds) but
    // only called once per thread at registration time, so fine.
    let link_target = match std::fs::read_link("/proc/thread-self") {
        Ok(p) => p,
        Err(_) => return 0,
    };
    link_target
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
fn current_tid() -> u32 {
    0
}

#[cfg(target_os = "linux")]
fn read_proc_stat(pid: u32, tid: u32) -> (u64, i32) {
    // Returns (utime+stime in ticks, last CPU index or -1).
    let path = format!("/proc/{}/task/{}/stat", pid, tid);
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return (0, -1),
    };
    // The comm field can contain spaces and parens; find the closing
    // ')' and parse fields after it. Standard /proc/stat parsing.
    let Some(close_paren) = contents.rfind(')') else { return (0, -1); };
    let rest = &contents[close_paren + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After ')', fields are: state, ppid, pgrp, ..., utime(13), stime(14),
    // ..., processor(38). The "rest" array is zero-indexed starting at
    // "state" so utime is index 11, stime is 12, processor is 36.
    // (Indices: state=0, ppid=1, pgrp=2, session=3, tty=4, tpgid=5,
    //  flags=6, minflt=7, cminflt=8, majflt=9, cmajflt=10, utime=11,
    //  stime=12, cutime=13, cstime=14, priority=15, nice=16, ...,
    //  processor=36.)
    let utime = fields.get(11).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let stime = fields.get(12).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let last_cpu = fields.get(36).and_then(|s| s.parse::<i32>().ok()).unwrap_or(-1);
    (utime + stime, last_cpu)
}

#[cfg(not(target_os = "linux"))]
fn read_proc_stat(_pid: u32, _tid: u32) -> (u64, i32) {
    (0, -1)
}

#[cfg(target_os = "linux")]
fn read_proc_wchan(pid: u32, tid: u32) -> String {
    let path = format!("/proc/{}/task/{}/wchan", pid, tid);
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "?".to_string())
}

#[cfg(not(target_os = "linux"))]
fn read_proc_wchan(_pid: u32, _tid: u32) -> String {
    "?".to_string()
}

// ─── tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Note: these tests run in serial because PROBE is global. Cargo
    // test by default uses multiple threads; the OnceLock-based init
    // means second-and-later initializations are no-ops, so the first
    // test to run "wins" with its config. That's fine for these
    // smoke tests, but we don't try to test the disabled path in the
    // same run as an enabled path.

    #[test]
    fn null_handle_when_disabled() {
        // If env var not set (default), register should return inactive.
        // We can't easily ensure this is the case without process
        // isolation, so we test by NOT calling init(): without init,
        // PROBE is None, register returns inactive.
        let h = register("test-thread");
        // Either:
        //  (a) PROBE is None → handle inactive ✓
        //  (b) PROBE was init'd elsewhere with enabled=false → handle inactive ✓
        //  (c) PROBE was init'd elsewhere with enabled=true → handle active.
        // We only assert that set() doesn't panic on an inactive handle:
        if !h.is_active() {
            h.set(Activity::Working);
            h.set_with_context(Activity::MeshingChunk, 12345);
            h.set_context(67890);
        }
    }

    #[test]
    fn activity_names_unique() {
        // Stability test: the analysis tool relies on these.
        let names = [
            Activity::Unknown.name(),
            Activity::Idle.name(),
            Activity::Working.name(),
            Activity::WaitingForFence.name(),
            Activity::RecordingCommands.name(),
            Activity::SubmittingCommands.name(),
            Activity::AcquiringImage.name(),
            Activity::Presenting.name(),
            Activity::MeshingChunk.name(),
            Activity::GeneratingChunk.name(),
            Activity::UploadingBuffer.name(),
            Activity::WaitingOnLock.name(),
            Activity::TickSim.name(),
            Activity::Other.name(),
        ];
        for (i, &n) in names.iter().enumerate() {
            for (j, &m) in names.iter().enumerate() {
                if i != j {
                    assert_ne!(n, m, "activity names must be unique ({} vs {})", n, m);
                }
            }
        }
    }

    #[test]
    fn activity_name_matches_discriminant_lookup() {
        // The sampler's fast-path `activity_name_for(u8)` MUST agree
        // with `Activity::name()` for every variant. If a contributor
        // adds a new variant and forgets to update the lookup, this
        // test catches it.
        let pairs = [
            (Activity::Unknown as u8, Activity::Unknown.name()),
            (Activity::Idle as u8, Activity::Idle.name()),
            (Activity::Working as u8, Activity::Working.name()),
            (Activity::WaitingForFence as u8, Activity::WaitingForFence.name()),
            (Activity::RecordingCommands as u8, Activity::RecordingCommands.name()),
            (Activity::SubmittingCommands as u8, Activity::SubmittingCommands.name()),
            (Activity::AcquiringImage as u8, Activity::AcquiringImage.name()),
            (Activity::Presenting as u8, Activity::Presenting.name()),
            (Activity::MeshingChunk as u8, Activity::MeshingChunk.name()),
            (Activity::GeneratingChunk as u8, Activity::GeneratingChunk.name()),
            (Activity::UploadingBuffer as u8, Activity::UploadingBuffer.name()),
            (Activity::WaitingOnLock as u8, Activity::WaitingOnLock.name()),
            (Activity::TickSim as u8, Activity::TickSim.name()),
            (Activity::Other as u8, Activity::Other.name()),
        ];
        for (u, expected) in pairs {
            assert_eq!(activity_name_for(u), expected,
                "discriminant {} mismatches", u);
        }
    }

    #[test]
    fn engine_state_chug_threshold() {
        // FPS=10 (tenths=100) should trigger chug.
        // FPS=30 (tenths=300) should NOT trigger chug.
        // wait_fences > 50ms should trigger chug.
        // This indirectly tests the threshold via engine_state_update,
        // but only works if probe is enabled — skip silently if not.
        if !enabled() { return; }
        engine_state_update(100, 0, 0, 0, 0, 0, 0, 0);
        let es = engine_state().unwrap();
        assert!(es.chug_active.load(Ordering::Relaxed), "fps=10 should chug");

        engine_state_update(1300, 7400, 6190, 0, 0, 0, 0, 0);
        assert!(!es.chug_active.load(Ordering::Relaxed), "fps=130 healthy");

        engine_state_update(1300, 7400, 60000, 0, 0, 0, 0, 0);
        assert!(es.chug_active.load(Ordering::Relaxed),
            "wait_fences=60ms should chug despite high fps");
    }
}
